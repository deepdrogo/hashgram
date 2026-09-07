package cmd

import (
	"fmt"
	"os"
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
  hashgramctl init-mainnet-genesis --founder-address hash1...
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
			}
			paths = hgconfig.Paths{
				ConfigDir: flagConfigDir,
				DataDir:   flagDataDir,
				NodeHome:  home,
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
		"hashgramd node home (defaults to $HASHGRAM_HOME or $HOME/.hashgram)")
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
	if p, err := exec_LookPath("hashgramd"); err == nil {
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
