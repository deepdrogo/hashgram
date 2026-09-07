package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/spf13/cobra"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
)

// cmdFinalizeGenesis collects the validator gentxs into the genesis file and
// pins the resulting hash.
//
// init-mainnet-genesis writes a file with the allocations and no validators,
// because the validators' gentxs can only be produced against that file.
// Adding them changes the bytes, so the hash init-mainnet-genesis printed is
// preliminary. This command produces the final file and the final hash, and
// rewrites the pin in network.json (and app.toml when it carries one) so
// that mainnet-preflight's "genesis hash pinned" check compares against the
// right value. Without it the operator would edit JSON by hand at the single
// most consequential moment of the launch.
func cmdFinalizeGenesis() *cobra.Command {
	var force bool
	cmd := &cobra.Command{
		Use:   "finalize-genesis",
		Short: "Collect validator gentxs into genesis and pin the final hash",
		Long: `Collect the validator gentxs into the genesis file and pin the final hash.

Run once, on the genesis machine, after every launch validator's gentx has
been copied into <node-home>/config/gentx/. It runs hashgramd's
collect-gentxs and validate-genesis, refuses a genesis with no validators,
recomputes the genesis hash over the final bytes and records it as this
node's network pin.

The hash this prints is the network's identity. Record it off this server
and publish it separately from the file.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, cancel := commandContext()
			defer cancel()

			network, err := hgconfig.LoadNetwork(paths)
			if err != nil {
				return fmt.Errorf("no network configuration: run init-mainnet-genesis first (%w)", err)
			}
			genesisPath := paths.GenesisFile()
			before, err := os.ReadFile(genesisPath)
			if err != nil {
				return fmt.Errorf("read %s: %w", genesisPath, err)
			}

			var doc struct {
				AppState struct {
					Genutil struct {
						GenTxs []json.RawMessage `json:"gen_txs"`
					} `json:"genutil"`
				} `json:"app_state"`
			}
			if err := json.Unmarshal(before, &doc); err != nil {
				return fmt.Errorf("parse %s: %w", genesisPath, err)
			}
			if len(doc.AppState.Genutil.GenTxs) > 0 && !force {
				return fmt.Errorf(
					"%s already contains %d validator gentx(s) and its hash is pinned as %s.\n\n"+
						"Finalising twice would change the network identity after it may have been\n"+
						"published. Pass --force only if you are certain nobody has this hash yet.",
					genesisPath, len(doc.AppState.Genutil.GenTxs), network.GenesisHash)
			}

			gentxDir := filepath.Join(filepath.Dir(genesisPath), "gentx")
			gentxs, _ := filepath.Glob(filepath.Join(gentxDir, "*.json"))
			if len(gentxs) == 0 {
				return fmt.Errorf(
					"no gentx files in %s.\n\n"+
						"Each launch validator produces one with 'hashgramd genesis gentx' against the\n"+
						"preliminary genesis file and sends it to you. Copy them here, then re-run.",
					gentxDir)
			}

			o := newOut()
			o.section("Collecting validator gentxs")
			for _, g := range gentxs {
				o.row("  gentx", filepath.Base(g))
			}

			if out, err := runHashgramd(ctx, "genesis", "collect-gentxs"); err != nil {
				return fmt.Errorf("collect-gentxs failed: %w\n%s", err, out)
			}
			if out, err := runHashgramd(ctx, "genesis", "validate-genesis"); err != nil {
				// Put the preliminary file back so the operator is not left
				// with a half-finalised genesis they did not ask for.
				_ = os.WriteFile(genesisPath, before, 0o644)
				return fmt.Errorf("the collected genesis does not validate; the preliminary file was restored: %w\n%s", err, out)
			}

			after, err := os.ReadFile(genesisPath)
			if err != nil {
				return err
			}
			if err := json.Unmarshal(after, &doc); err != nil {
				return fmt.Errorf("parse finalised genesis: %w", err)
			}
			if len(doc.AppState.Genutil.GenTxs) == 0 {
				_ = os.WriteFile(genesisPath, before, 0o644)
				return fmt.Errorf("collect-gentxs produced no validators; the preliminary file was restored")
			}

			finalHash := hgparams.ComputeGenesisHash(after)
			network.GenesisHash = finalHash
			network.CreatedBy = "hashgramctl finalize-genesis"
			if err := hgconfig.SaveNetwork(paths, network, true); err != nil {
				return err
			}
			appTomlPinned, err := pinAppTomlGenesisHash(paths.AppConfigFile(), finalHash)
			if err != nil {
				return err
			}
			if err := adoptServiceOwnership(paths); err != nil {
				return err
			}

			o.blank()
			o.section("GENESIS FINALISED")
			o.row("file", genesisPath)
			o.row("validators", len(doc.AppState.Genutil.GenTxs))
			o.row("chain id", network.ChainID)
			o.row("GENESIS HASH", finalHash)
			o.row("pinned in", paths.NetworkFile())
			if appTomlPinned {
				o.row("pinned in", paths.AppConfigFile())
			}
			o.row("finalised at", time.Now().UTC().Format(time.RFC3339))
			o.blank()
			o.raw("  This hash is the network's identity. Write it down off this server and")
			o.raw("  publish it through channels independent of the genesis file itself.")
			o.raw("  Anyone can verify it: sha256sum " + genesisPath)
			o.blank()
			o.raw("  Next:  hashgramctl mainnet-preflight")
			o.raw("         hashgramctl start && hashgramctl chain-status")
			return o.flush()
		},
	}
	cmd.Flags().BoolVar(&force, "force", false,
		"re-finalise a genesis that already has validators (changes the network identity)")
	return cmd
}

var appTomlGenesisHashLine = regexp.MustCompile(`(?m)^genesis-hash\s*=\s*".*"$`)

// pinAppTomlGenesisHash rewrites the genesis-hash line in app.toml when the
// file has one, so hashgramd's own pin agrees with hashgramctl's. Reports
// whether a line was rewritten.
func pinAppTomlGenesisHash(path, hash string) (bool, error) {
	raw, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return false, nil
		}
		return false, err
	}
	if !appTomlGenesisHashLine.Match(raw) {
		return false, nil
	}
	updated := appTomlGenesisHashLine.ReplaceAllString(string(raw), `genesis-hash = "`+hash+`"`)
	if strings.TrimSpace(updated) == strings.TrimSpace(string(raw)) {
		return true, nil
	}
	return true, os.WriteFile(path, []byte(updated), 0o644)
}
