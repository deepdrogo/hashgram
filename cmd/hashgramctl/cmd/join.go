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
		p2pPeers    []string
		devnet      bool
		force       bool
	)

	cmd := &cobra.Command{
		Use:   "join-mainnet",
		Short: "Join an existing Hashgram network",
		Long: `Join an existing Hashgram network.

This does NOT create a genesis file. It obtains the existing one, verifies it
against the pinned hash, and pins that identity locally.

For Mainnet no arguments are needed. The canonical genesis file, its SHA-256
and a list of seed nodes are compiled into this binary, the way bitcoind
carries its genesis block and fixed seeds:

  hashgramctl join-mainnet

The binary you chose to run is the out-of-band source for the hash. If you
pass --genesis-hash and it differs from the built-in one, the command
refuses: either the file you were handed is a fork, or this binary is.

You may still supply the genesis from elsewhere (--genesis-url /
--genesis-file); it must hash to the built-in value. Peers you pass with
--peers become persistent_peers; peers you pass with --p2p-peers are added to
the built-in hashgram-node bootstrap list, not substituted for it.

For a DEVNET (--devnet) nothing is built in, so --genesis-hash and one of
--genesis-url / --genesis-file are required.

What may safely be copied from another server: the genesis file, the address
book, and the app.toml and config.toml (with the moniker changed).

What must NEVER be copied: priv_validator_key.json. Two validators signing
with the same consensus key is a double-sign, which is slashable and is the
one mistake in Hashgram operations that cannot be undone. See
docs/OPERATIONS.md.`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			genesisHash = strings.ToLower(genesisHash)
			if genesisHash != "" {
				if err := hgparams.ValidateGenesisHash(genesisHash); err != nil {
					return fmt.Errorf("--genesis-hash is malformed: %w", err)
				}
			}

			builtin := !devnet
			switch {
			case devnet && genesisHash == "":
				return fmt.Errorf(
					"--genesis-hash is required for a devnet.\n\n" +
						"Nothing is built in for development networks. Obtain the expected hash\n" +
						"from a source independent of the genesis file, then verify:\n" +
						"  sha256sum genesis.json")
			case devnet && genesisURL == "" && genesisFile == "":
				return fmt.Errorf("one of --genesis-url or --genesis-file is required for a devnet")
			case builtin && genesisHash == "":
				genesisHash = hgparams.MainnetGenesisHash
			case builtin && genesisHash != hgparams.MainnetGenesisHash:
				return fmt.Errorf(
					"GENESIS HASH DISAGREES WITH THIS BINARY - refusing to join\n\n"+
						"  built in  %s\n"+
						"  you gave  %s\n\n"+
						"This hashgramctl was built for %s, whose genesis hashes to the\n"+
						"built-in value. A different hash means the network you are being told to\n"+
						"join is not the one this software was released for. If you really mean a\n"+
						"development network, pass --devnet.",
					hgparams.MainnetGenesisHash, genesisHash, hgparams.NetworkNameMainnet)
			}

			var raw []byte
			var err error
			source := "built into this binary"
			switch {
			case genesisFile != "":
				raw, err = os.ReadFile(genesisFile)
				source = genesisFile
			case genesisURL != "":
				fmt.Fprintf(cmd.OutOrStdout(), "fetching genesis from %s\n", genesisURL)
				raw, err = download(genesisURL)
				source = genesisURL
			default:
				raw = hgparams.MainnetGenesis
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

			// Discovery. Persistent peers are what the operator asked for;
			// seeds and bootstrap peers are the built-in first-contact list,
			// merged with (never replaced by) anything the operator passed.
			var seeds, dnsSeeds []string
			if peers != "" {
				if err := writeCometString("persistent_peers", peers, false); err != nil {
					return err
				}
			}
			if builtin {
				seeds = hgparams.MainnetSeeds()
				if err := writeCometString("seeds", strings.Join(seeds, ","), true); err != nil {
					return err
				}
				p2pPeers = mergeUnique(p2pPeers, hgparams.MainnetBootstrapPeers())
				dnsSeeds = hgparams.MainnetDNSSeeds()
			}
			if len(p2pPeers) > 0 {
				if err := writeNodeTomlList("bootstrap_peers", p2pPeers); err != nil {
					return err
				}
			}
			if len(dnsSeeds) > 0 {
				if err := writeNodeTomlList("dnsaddr", dnsSeeds); err != nil {
					return err
				}
			}

			o := newOut()
			o.raw("")
			o.raw("JOINED " + expected.NetworkName)
			o.raw("")
			if err := adoptServiceOwnership(paths); err != nil {
				return err
			}
			o.row("Genesis file", target)
			o.row("Genesis source", source)
			o.row("Genesis hash", genesisHash)
			o.row("Network id", expected.NetworkID)
			o.row("Chain id", expected.ChainID)
			o.row("Pinned in", paths.NetworkFile())
			if peers != "" {
				o.row("Persistent peers", peers)
			}
			if len(seeds) > 0 {
				o.row("CometBFT seeds", strings.Join(seeds, ", "))
			}
			if len(p2pPeers) > 0 {
				o.row("P2P bootstrap peers", strings.Join(p2pPeers, ", "))
			}
			if len(dnsSeeds) > 0 {
				o.row("P2P DNS seeds", strings.Join(dnsSeeds, ", "))
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
		"expected lowercase hex sha256 of the genesis file; built in for Mainnet, required with --devnet")
	cmd.Flags().StringVar(&peers, "peers", "",
		"comma-separated persistent peers as nodeid@host:port (built-in seeds are used regardless)")
	cmd.Flags().StringArrayVar(&p2pPeers, "p2p-peers", nil,
		"extra hashgram-node bootstrap multiaddrs with /p2p/<peer-id> (repeatable); merged with the built-in list into node.toml")
	cmd.Flags().BoolVar(&devnet, "devnet", false, "join a DEVNET rather than Mainnet")
	cmd.Flags().BoolVar(&force, "force", false, "overwrite an existing genesis and network pin")

	return cmd
}

// writeCometString sets a top-level string key (persistent_peers, seeds) in
// config.toml. With onlyIfEmpty, a value the operator already set is kept:
// the built-in seed list is a default, not an override.
//
// Edited textually rather than through the CometBFT config loader, because
// rewriting the whole file would discard an operator's own comments and
// hand-tuned values.
func writeCometString(key, value string, onlyIfEmpty bool) error {
	path := paths.CometConfigFile()
	raw, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("reading %s: %w", path, err)
	}

	lines := strings.Split(string(raw), "\n")
	replaced := false
	for i, line := range lines {
		t := strings.TrimSpace(line)
		if !strings.HasPrefix(t, key+" ") && !strings.HasPrefix(t, key+"=") {
			continue
		}
		if onlyIfEmpty {
			_, cur, _ := strings.Cut(t, "=")
			if strings.Trim(strings.TrimSpace(cur), `"`) != "" {
				return nil
			}
		}
		lines[i] = fmt.Sprintf("%s = %q", key, value)
		replaced = true
		break
	}
	if !replaced {
		return fmt.Errorf("no %s setting found in %s", key, path)
	}

	return os.WriteFile(path, []byte(strings.Join(lines, "\n")), 0o644)
}

// mergeUnique appends the entries of extra that are not already in base,
// preserving order. Operator-supplied entries stay first.
func mergeUnique(base, extra []string) []string {
	seen := make(map[string]bool, len(base)+len(extra))
	out := make([]string, 0, len(base)+len(extra))
	for _, lst := range [][]string{base, extra} {
		for _, v := range lst {
			v = strings.TrimSpace(v)
			if v == "" || seen[v] {
				continue
			}
			seen[v] = true
			out = append(out, v)
		}
	}
	return out
}

// writeNodeTomlList sets a string-array key in /etc/hashgram/node.toml,
// replacing an existing line or appending. The node validates the values at
// startup; this only edits text.
func writeNodeTomlList(key string, values []string) error {
	path := filepath.Join(paths.ConfigDir, "node.toml")
	raw, err := os.ReadFile(path)
	if err != nil && !os.IsNotExist(err) {
		return err
	}
	quoted := make([]string, 0, len(values))
	for _, v := range values {
		quoted = append(quoted, fmt.Sprintf("%q", v))
	}
	line := fmt.Sprintf("%s = [%s]", key, strings.Join(quoted, ", "))
	lines := strings.Split(string(raw), "\n")
	replaced := false
	for i, l := range lines {
		t := strings.TrimSpace(l)
		if strings.HasPrefix(t, key+" ") || strings.HasPrefix(t, key+"=") {
			lines[i] = line
			replaced = true
		}
	}
	if !replaced {
		lines = append(lines, line)
	}
	out := strings.TrimRight(strings.Join(lines, "\n"), "\n") + "\n"
	return os.WriteFile(path, []byte(out), 0o644)
}
