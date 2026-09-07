package cmd

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/spf13/cobra"

	cmttypes "github.com/cometbft/cometbft/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
)

func cmdJoinMainnet() *cobra.Command {
	var (
		genesisURL  string
		genesisFile string
		genesisHash string
		peers       string
		devnet      bool
		force       bool
	)

	cmd := &cobra.Command{
		Use:   "join-mainnet",
		Short: "Join an existing Hashgram network",
		Long: `Join an existing Hashgram network.

This does NOT create a genesis file. It obtains the existing one, verifies it
against the hash you supply, and pins that identity locally.

The --genesis-hash argument is not optional convenience. Without it, joining
means trusting whoever gave you the file and the URL, and a fork with a
plausible-looking genesis would be indistinguishable from the real network.
Obtain the hash from a source independent of the genesis file itself.

  hashgramctl join-mainnet \
    --genesis-url https://example/genesis.json \
    --genesis-hash <sha256> \
    --peers <nodeid>@<host>:26656,<nodeid>@<host>:26656

What may safely be copied from another server: the genesis file, the address
book, and the app.toml and config.toml (with the moniker changed).

What must NEVER be copied: priv_validator_key.json. Two validators signing
with the same consensus key is a double-sign, which is slashable and is the
one mistake in Hashgram operations that cannot be undone. See
docs/OPERATIONS.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			if genesisHash == "" {
				return fmt.Errorf(
					"--genesis-hash is required.\n\n" +
						"Without it, joining a network means trusting whoever gave you the file.\n" +
						"Obtain the expected hash from a source independent of the genesis file,\n" +
						"then verify: sha256sum genesis.json")
			}
			if err := hgparams.ValidateGenesisHash(strings.ToLower(genesisHash)); err != nil {
				return fmt.Errorf("--genesis-hash is malformed: %w", err)
			}
			genesisHash = strings.ToLower(genesisHash)

			if genesisURL == "" && genesisFile == "" {
				return fmt.Errorf("one of --genesis-url or --genesis-file is required")
			}

			var raw []byte
			var err error
			switch {
			case genesisFile != "":
				raw, err = os.ReadFile(genesisFile)
			default:
				fmt.Fprintf(cmd.OutOrStdout(), "fetching genesis from %s\n", genesisURL)
				raw, err = download(genesisURL)
			}
			if err != nil {
				return err
			}

			// Verify before writing anything. A file that fails the hash
			// check must never reach the node home, or a later start would
			// pick it up.
			actual := hgparams.ComputeGenesisHash(raw)
			if actual != genesisHash {
				return fmt.Errorf(
					"GENESIS HASH MISMATCH - refusing to join\n\n"+
						"  expected  %s\n"+
						"  received  %s\n\n"+
						"The file you were given is not the genesis of the network you intended to\n"+
						"join. This is exactly the check that separates Hashgram Mainnet from a\n"+
						"fork of the software with its own genesis.",
					genesisHash, actual)
			}

			var doc cmttypes.GenesisDoc
			if err := json.Unmarshal(raw, &doc); err != nil {
				return fmt.Errorf("the genesis file does not parse: %w", err)
			}
			if err := doc.ValidateAndComplete(); err != nil {
				return fmt.Errorf("the genesis file is invalid: %w", err)
			}

			expected := hgparams.MainnetIdentity(genesisHash)
			if devnet {
				expected = hgparams.DevnetIdentity(genesisHash)
			}
			if doc.ChainID != expected.ChainID {
				return fmt.Errorf(
					"the genesis file is for chain-id %q but %s expects %q; "+
						"pass --devnet if you meant to join a development network",
					doc.ChainID, expected.NetworkName, expected.ChainID)
			}

			target := paths.GenesisFile()
			if fileExists(target) && !force {
				existing, readErr := os.ReadFile(target)
				if readErr == nil && hgparams.ComputeGenesisHash(existing) == genesisHash {
					fmt.Fprintf(cmd.OutOrStdout(),
						"genesis already present and matches; leaving it in place\n")
				} else {
					return fmt.Errorf(
						"%s already exists with a different hash.\n\n"+
							"Refusing to overwrite: this node is currently pinned to a different\n"+
							"network. Move the existing file aside, or pass --force.", target)
				}
			} else {
				if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
					return err
				}
				if err := os.WriteFile(target, raw, 0o644); err != nil {
					return err
				}
			}

			network := hgconfig.Network{
				NetworkName:          expected.NetworkName,
				NetworkID:            expected.NetworkID,
				ChainID:              expected.ChainID,
				NetworkMagic:         string(expected.NetworkMagic[:]),
				ProtocolMajorVersion: expected.ProtocolMajorVersion,
				GenesisHash:          genesisHash,
				GenesisTime:          doc.GenesisTime.Format(time.RFC3339),
				CreatedBy:            "hashgramctl join-mainnet",
			}
			if err := hgconfig.SaveNetwork(paths, network, force); err != nil {
				return err
			}

			if peers != "" {
				if err := writePersistentPeers(peers); err != nil {
					return err
				}
			}

			o := newOut()
			o.raw("")
			o.raw("JOINED " + expected.NetworkName)
			o.raw("")
			o.row("Genesis file", target)
			o.row("Genesis hash", genesisHash)
			o.row("Network id", expected.NetworkID)
			o.row("Chain id", expected.ChainID)
			o.row("Pinned in", paths.NetworkFile())
			if peers != "" {
				o.row("Persistent peers", peers)
			}
			o.blank()
			o.raw("Next: configure this machine's roles, then start it.")
			o.raw("")
			o.raw("  hashgramctl configure-role relay,store")
			o.raw("  hashgramctl start")
			o.raw("  hashgramctl chain-status")
			o.raw("")
			o.raw("Do NOT copy priv_validator_key.json from another machine. Two validators")
			o.raw("signing with the same consensus key is a slashable double-sign.")
			return o.flush()
		},
	}

	cmd.Flags().StringVar(&genesisURL, "genesis-url", "", "URL to fetch genesis.json from")
	cmd.Flags().StringVar(&genesisFile, "genesis-file", "", "path to an existing genesis.json")
	cmd.Flags().StringVar(&genesisHash, "genesis-hash", "",
		"expected lowercase hex sha256 of the genesis file; required")
	cmd.Flags().StringVar(&peers, "peers", "",
		"comma-separated persistent peers as nodeid@host:port")
	cmd.Flags().BoolVar(&devnet, "devnet", false, "join a DEVNET rather than Mainnet")
	cmd.Flags().BoolVar(&force, "force", false, "overwrite an existing genesis and network pin")

	return cmd
}

// writePersistentPeers sets persistent_peers in config.toml.
//
// Edited textually rather than through the CometBFT config loader, because
// rewriting the whole file would discard an operator's own comments and
// hand-tuned values.
func writePersistentPeers(peers string) error {
	path := paths.CometConfigFile()
	raw, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("reading %s: %w", path, err)
	}

	lines := strings.Split(string(raw), "\n")
	replaced := false
	for i, line := range lines {
		if strings.HasPrefix(strings.TrimSpace(line), "persistent_peers ") ||
			strings.HasPrefix(strings.TrimSpace(line), "persistent_peers=") {
			lines[i] = fmt.Sprintf("persistent_peers = %q", peers)
			replaced = true
			break
		}
	}
	if !replaced {
		return fmt.Errorf("no persistent_peers setting found in %s", path)
	}

	return os.WriteFile(path, []byte(strings.Join(lines, "\n")), 0o644)
}
