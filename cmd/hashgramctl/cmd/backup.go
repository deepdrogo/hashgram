package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgsys"
)

// excludedFromBackup are paths never written into a backup archive.
//
// The consensus signing key is excluded deliberately. §63 is explicit that
// private keys should not be casually packaged into server backups, and the
// reason is concrete: a backup gets copied to a laptop, a bucket and a
// colleague, and any copy restored onto a running network is a double-sign.
// The key is backed up separately and deliberately, by a human who knows
// what they are handling.
var excludedFromBackup = []string{
	"config/priv_validator_key.json",
	"keyring-file",
	"keyring-test",
	"keyring-os",
	// The P2P node's hot keys. The operator key can unbond the provider
	// bond and the node key is the node's peer identity; both are
	// regenerated on a rebuilt host rather than restored from an archive.
	"node/operator.key",
	"node/node_key",
	// Blob chunks are replicated on the network and repaired from other
	// providers; the archive keeps the manifests and every other store.
	"node/blobs.redb",
}

func cmdBackup() *cobra.Command {
	var (
		outputDir      string
		includeChainDB bool
	)

	cmd := &cobra.Command{
		Use:   "backup",
		Short: "Archive this node's configuration and state",
		Long: `Archive this node's configuration and state.

What is included:
  the network pin, the role configuration, the genesis file, the node key,
  app.toml and config.toml, the address book, and the validator signing
  state (priv_validator_state.json, which records the last height signed).

What is deliberately EXCLUDED:
  priv_validator_key.json and any keyring.

That exclusion is not an oversight. A backup gets copied to a laptop, a
bucket and a colleague, and any copy of the consensus key restored onto a
machine while the original is running is a slashable double-sign. Back the
consensus key up separately and deliberately; see docs/DISASTER_RECOVERY.md.

Chain data is excluded by default because it can be re-synced from the
network, and including it turns a small archive into a large one. Pass
--include-chain-db for a full snapshot, and stop the node first: copying a
live database produces a corrupt copy.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if outputDir == "" {
				outputDir = filepath.Join(paths.DataDir, "backups")
			}
			if err := os.MkdirAll(outputDir, 0o700); err != nil {
				return err
			}

			if includeChainDB {
				state := hgsys.Inspect(ctx, hgsys.UnitChain)
				if state.Running() {
					return fmt.Errorf(
						"the node is running.\n\n" +
							"Copying a live chain database produces a corrupt copy. Stop the node\n" +
							"first:\n\n  hashgramctl stop\n  hashgramctl backup --include-chain-db\n  hashgramctl start")
				}
			}

			stamp := time.Now().UTC().Format("20060102-150405")
			archive := filepath.Join(outputDir, fmt.Sprintf("hashgram-backup-%s.tar.gz", stamp))

			args := []string{"-czf", archive}
			for _, ex := range excludedFromBackup {
				args = append(args, "--exclude="+ex)
			}
			if !includeChainDB {
				args = append(args, "--exclude=data/application.db", "--exclude=data/blockstore.db",
					"--exclude=data/state.db", "--exclude=data/tx_index.db",
					"--exclude=data/evidence.db", "--exclude=data/snapshots",
					"--exclude=data/cs.wal")
			}

			// Absolute paths are stored relative to / so the archive is
			// inspectable and restorable without surprises. The P2P node's
			// state (peerstore, mailbox, social, safety, rewards) rides
			// along when present; the indexer's PostgreSQL is not here
			// because it is rebuilt with `hashgramctl indexer rebuild`.
			args = append(args, "-C", "/",
				strings.TrimPrefix(paths.NodeHome, "/"),
				strings.TrimPrefix(paths.ConfigDir, "/"))
			if nodeDir := filepath.Join(paths.DataDir, "node"); fileExists(nodeDir) {
				args = append(args, strings.TrimPrefix(nodeDir, "/"))
			}
			if safetyDir := filepath.Join(paths.DataDir, "safety"); fileExists(safetyDir) {
				args = append(args, "--exclude=safety/attestor.key", strings.TrimPrefix(safetyDir, "/"))
			}

			c := exec.CommandContext(ctx, "tar", args...)
			c.Stderr = cmd.ErrOrStderr()
			if err := c.Run(); err != nil {
				return fmt.Errorf("tar failed: %w", err)
			}

			info, err := os.Stat(archive)
			if err != nil {
				return err
			}
			// Backups can contain the node key and the signing state; not
			// secrets on the level of the consensus key, but not for
			// general reading either.
			if err := os.Chmod(archive, 0o600); err != nil {
				return err
			}

			o := newOut()
			o.row("Archive", archive)
			o.row("Size", hgsys.HumanBytes(info.Size()))
			o.row("Chain data included", includeChainDB)
			o.blank()
			o.raw("EXCLUDED from this archive, on purpose:")
			for _, ex := range excludedFromBackup {
				o.raw("  " + ex)
			}
			o.blank()
			o.raw("The consensus signing key is not in here. Back it up separately, and never")
			o.raw("restore it onto a machine while the original validator is still running.")
			o.blank()
			o.raw("Copy this archive off the server. A backup that only exists on the machine")
			o.raw("it protects is not a backup.")
			return o.flush()
		},
	}

	cmd.Flags().StringVar(&outputDir, "output-dir", "", "where to write the archive")
	cmd.Flags().BoolVar(&includeChainDB, "include-chain-db", false,
		"include the chain database (requires the node to be stopped)")

	return cmd
}

func cmdRestore() *cobra.Command {
	var archive string

	cmd := &cobra.Command{
		Use:   "restore [archive]",
		Short: "Restore configuration and state from a backup archive",
		Long: `Restore configuration and state from a backup archive.

The node must be stopped. Restoring over a running node produces
inconsistent state, and if the archive contains validator signing state,
restoring it while the node is signing risks a double-sign.

After restoring, verify the network identity before starting:

  hashgramctl network-info
  hashgramctl mainnet-preflight`,
		Args: cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			if len(args) == 1 {
				archive = args[0]
			}
			if archive == "" {
				return fmt.Errorf("an archive path is required: hashgramctl restore <archive>")
			}
			if !fileExists(archive) {
				return fmt.Errorf("%s does not exist", archive)
			}

			state := hgsys.Inspect(ctx, hgsys.UnitChain)
			if state.Running() {
				return fmt.Errorf(
					"the node is running.\n\n"+
						"Stop it first:\n\n  hashgramctl stop\n  hashgramctl restore %s", archive)
			}

			prompt := fmt.Sprintf("This will overwrite %s and %s from %s.",
				paths.NodeHome, paths.ConfigDir, archive)
			if err := confirm(prompt, "RESTORE"); err != nil {
				return err
			}

			c := exec.CommandContext(ctx, "tar", "-xzf", archive, "-C", "/")
			c.Stdout = cmd.OutOrStdout()
			c.Stderr = cmd.ErrOrStderr()
			if err := c.Run(); err != nil {
				return fmt.Errorf("tar extraction failed: %w", err)
			}

			o := newOut()
			o.row("Restored from", archive)

			if network, err := hgconfig.LoadNetwork(paths); err == nil {
				o.row("Network", fmt.Sprintf("%s (%s)", network.NetworkName, network.ChainID))
				o.row("Genesis hash", network.GenesisHash)
			} else {
				o.row("Network", "NOT CONFIGURED after restore: "+err.Error())
			}

			if !fileExists(paths.ValidatorKeyFile()) {
				o.blank()
				o.raw("No validator key is present. That is expected: backups exclude it.")
				o.raw("If this machine is meant to validate, restore the consensus key from")
				o.raw("wherever you stored it separately, and make certain no other machine")
				o.raw("is currently signing with it.")
			}

			o.blank()
			o.raw("Before starting, verify what you restored:")
			o.raw("  hashgramctl network-info")
			o.raw("  hashgramctl mainnet-preflight")
			return o.flush()
		},
	}

	return cmd
}

func cmdIndexer() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "indexer",
		Short: "Manage the PostgreSQL index",
		Long: `Manage the PostgreSQL index.

The index is a cache. It is derived entirely from chain state and from
signed, replicated off-chain data, so if it is destroyed it can be rebuilt,
and if it disagrees with the chain it is wrong. Nothing canonical lives in
PostgreSQL; see docs/OPERATIONS.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			return cmd.Help()
		},
	}

	rebuild := &cobra.Command{
		Use:   "rebuild",
		Short: "Rebuild the index from canonical data",
		Long: `Rebuild the index from canonical data.

Safe to run at any time: the index holds nothing canonical. It reads chain
state and signed off-chain events and reconstructs the query tables.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			state := hgsys.Inspect(ctx, hgsys.UnitIndexer)
			if !state.Exists {
				return fmt.Errorf(
					"the indexer service is not installed on this machine.\n\n"+
						"The indexer is part of the Rust node stack and is installed when this\n"+
						"machine is configured with the 'indexer' role:\n\n"+
						"  hashgramctl configure-role %s,indexer\n\n"+
						"Until then there is no index to rebuild.",
					mustRolesString())
			}

			roles, _ := hgconfig.LoadRoles(paths)
			if !roles.Has(hgconfig.RoleIndexer) {
				return fmt.Errorf(
					"this machine is not configured with the indexer role (currently: %s)",
					roles.String())
			}

			o := newOut()
			o.raw("Stopping the indexer, clearing derived tables and resynchronising.")
			if err := hgsys.Stop(ctx, hgsys.UnitIndexer); err != nil {
				return err
			}
			o.row("Indexer", "stopped")

			// The rebuild itself is driven by the indexer binary, which owns
			// the schema. Duplicating the schema knowledge here would create
			// two places that have to agree about table layout.
			out, err := exec.CommandContext(ctx, "hashgram-indexer", "rebuild",
				"--config", filepath.Join(paths.ConfigDir, "indexer.toml")).CombinedOutput()
			text := strings.TrimSpace(string(out))
			if err != nil {
				_ = hgsys.Start(ctx, hgsys.UnitIndexer)
				return fmt.Errorf("hashgram-indexer rebuild failed: %v\n%s", err, text)
			}
			o.row("Rebuild", "complete")
			if text != "" {
				o.raw(text)
			}

			if err := hgsys.Start(ctx, hgsys.UnitIndexer); err != nil {
				return err
			}
			o.row("Indexer", "started")
			return o.flush()
		},
	}

	cmd.AddCommand(rebuild)
	return cmd
}

func mustRolesString() string {
	roles, err := hgconfig.LoadRoles(paths)
	if err != nil || len(roles.Roles) == 0 {
		return "relay,store"
	}
	return roles.String()
}
