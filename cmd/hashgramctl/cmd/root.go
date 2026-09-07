// Package cmd implements the hashgramctl operator command tree.
//
// Commands here favour refusing an unsafe action over completing it:
// join-mainnet requires a pinned genesis hash, mainnet-preflight refuses to
// pass on a host with development keys or an exposed admin RPC, and backup
// excludes private key material rather than archiving it.
package cmd

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"

	"github.com/spf13/cobra"

	"github.com/hashgram/hashgram/app"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgrpc"
)

// Global flags, resolved once in PersistentPreRun.
var (
	flagConfigDir string
	flagDataDir   string
	flagNodeHome  string
	flagRPC       string
	flagNodeAPI   string
	flagJSON      bool
	flagYes       bool
)

// paths and rpc are the resolved globals every command uses.
var (
	paths hgconfig.Paths
	rpc   *hgrpc.Client
)

// NewRootCmd builds the hashgramctl root command.
func NewRootCmd() *cobra.Command {
	root := &cobra.Command{
		Use:   "hashgramctl",
		Short: "Hashgram node operator CLI",
		Long: `hashgramctl operates a Hashgram node.

It creates or joins a network, configures node roles, drives the systemd
services, reports status, runs pre-launch safety checks, and takes backups.

hashgramctl holds no keys and signs nothing. Key operations go through
hashgramd's keyring or, for the Founder, through a wallet on a machine that is
not this server. See docs/FOUNDER_LAUNCH_RUNBOOK.md.

Typical first launch on a fresh Ubuntu server:

  sudo ./scripts/install/bootstrap-ubuntu.sh
  hashgramctl init-mainnet-genesis --founder-address hash1... \
      --genesis-account hash1<validator-operator>=1000000HASH
  hashgramd genesis gentx <key> 900000000000uhash --chain-id hashgram-1 ...
  hashgramctl finalize-genesis
  hashgramctl mainnet-preflight
  hashgramctl start
  hashgramctl chain-status

Joining an existing network from a second server:

  sudo ./scripts/install/bootstrap-ubuntu.sh
  hashgramctl join-mainnet --genesis-url <url> --genesis-hash <sha256> --peers <id@host:port>
  hashgramctl configure-role relay,store
  hashgramctl start`,
		SilenceUsage:  true,
		SilenceErrors: true,
		PersistentPreRunE: func(_ *cobra.Command, _ []string) error {
			home := flagNodeHome
			if home == "" {
				home = app.DefaultNodeHome
				// On a bootstrapped host the service's home wins over the
				// invoking user's dotfile directory, unless the operator
				// pinned one explicitly through HASHGRAM_HOME.
				if os.Getenv(app.EnvNodeHome) == "" {
					if prod, ok := productionNodeHome(flagDataDir); ok {
						home = prod
					}
				}
			}

			// Resolve the three directory flags to absolute, cleaned paths
			// once, here, rather than letting every command join onto whatever
			// the operator typed.
			//
			// This is not a privilege boundary: hashgramctl runs with the
			// operator's own permissions, so a deliberately hostile --home is
			// an operator attacking themselves. It matters because of the
			// accident case. A relative --home means every path this tool
			// prints depends on the working directory it was run from, so a
			// backup taken from one directory and a restore run from another
			// silently touch different files, and an error message naming
			// "config/genesis.json" tells the reader nothing about where that
			// actually is. Absolute paths make the output say what it means.
			var err error
			if paths, err = resolvePaths(flagConfigDir, flagDataDir, home); err != nil {
				return err
			}

			rpc = hgrpc.New(flagRPC)
			return nil
		},
	}

	root.PersistentFlags().StringVar(&flagConfigDir, "config-dir",
		hgconfig.DefaultConfigDir, "operator configuration directory")
	root.PersistentFlags().StringVar(&flagDataDir, "data-dir",
		hgconfig.DefaultDataDir, "node data directory")
	root.PersistentFlags().StringVar(&flagNodeHome, "home", "",
		"hashgramd node home (defaults to $HASHGRAM_HOME, else /var/lib/hashgram/chain when it exists, else $HOME/.hashgram)")
	root.PersistentFlags().StringVar(&flagNodeAPI, "node-api", hgrpc.NodeAPIAddr,
		"local API of hashgram-node (loopback)")
	root.PersistentFlags().StringVar(&flagRPC, "rpc",
		hgrpc.DefaultEndpoint, "local CometBFT RPC endpoint")
	root.PersistentFlags().BoolVar(&flagJSON, "json", false, "emit machine-readable JSON")
	root.PersistentFlags().BoolVarP(&flagYes, "yes", "y", false,
		"skip interactive confirmation prompts")

	root.AddCommand(
		// Lifecycle
		cmdInstall(),
		cmdInit(),
		cmdInitMainnetGenesis(),
		cmdFinalizeGenesis(),
		cmdJoinMainnet(),
		cmdUpdate(),

		// Service control
		cmdStart(),
		cmdStop(),
		cmdRestart(),
		cmdLogs(),

		// Inspection
		cmdStatus(),
		cmdHealth(),
		cmdPeers(),
		cmdNetworkInfo(),
		cmdNodeInfo(),
		cmdChainStatus(),
		cmdWalletInfo(),
		cmdRewards(),
		cmdStorage(),

		// Configuration
		cmdConfigureRole(),
		cmdValidator(),
		cmdPropose(),

		// Data
		cmdBackup(),
		cmdRestore(),
		cmdIndexer(),

		// Safety
		cmdMainnetPreflight(),
	)

	return root
}

// hashgramdPath locates the hashgramd binary.
//
// The node binary is the authority on chain state, so hashgramctl delegates
// every chain query to it rather than reimplementing decoding. Looking it up
// on PATH first means a developer running from ./build works without extra
// flags.
func hashgramdPath() (string, error) {
	if p := os.Getenv("HASHGRAMD_BIN"); p != "" {
		return p, nil
	}
	if p, err := exec.LookPath("hashgramd"); err == nil {
		return p, nil
	}
	for _, candidate := range []string{
		"/usr/local/bin/hashgramd",
		filepath.Join("build", "hashgramd"),
		"./hashgramd",
	} {
		if _, err := os.Stat(candidate); err == nil {
			abs, err := filepath.Abs(candidate)
			if err == nil {
				return abs, nil
			}
		}
	}
	return "", fmt.Errorf(
		"hashgramd not found on PATH or in /usr/local/bin; " +
			"set HASHGRAMD_BIN or run 'make install'")
}
