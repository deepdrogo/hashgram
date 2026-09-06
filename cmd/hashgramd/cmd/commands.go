package cmd

import (
	"errors"
	"io"

	cmtcfg "github.com/cometbft/cometbft/config"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/spf13/cobra"
	"github.com/spf13/viper"

	"cosmossdk.io/log"
	confixcmd "cosmossdk.io/tools/confix/cmd"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/debug"
	"github.com/cosmos/cosmos-sdk/client/keys"
	"github.com/cosmos/cosmos-sdk/client/pruning"
	"github.com/cosmos/cosmos-sdk/client/rpc"
	"github.com/cosmos/cosmos-sdk/client/snapshot"
	"github.com/cosmos/cosmos-sdk/server"
	serverconfig "github.com/cosmos/cosmos-sdk/server/config"
	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	"github.com/cosmos/cosmos-sdk/types/module"
	authcmd "github.com/cosmos/cosmos-sdk/x/auth/client/cli"
	genutilcli "github.com/cosmos/cosmos-sdk/x/genutil/client/cli"

	"github.com/hashgram/hashgram/app"
	hgparams "github.com/hashgram/hashgram/app/params"
)

// initCometBFTConfig sets Hashgram's CometBFT defaults.
//
// The important departures from the CometBFT defaults are all about not
// exposing an administrative surface to the internet by accident:
//   - RPC listens on loopback. Public read RPC is exposed deliberately, via a
//     reverse proxy, not by binding 0.0.0.0 here.
//   - The Prometheus listener is loopback-only.
func initCometBFTConfig() *cmtcfg.Config {
	cfg := cmtcfg.DefaultConfig()

	// Loopback by default. `hashgramctl configure-role` widens this only for
	// the P2P listener, which genuinely must be reachable.
	cfg.RPC.ListenAddress = "tcp://127.0.0.1:26657"
	cfg.RPC.CORSAllowedOrigins = []string{}

	cfg.Instrumentation.Prometheus = true
	cfg.Instrumentation.PrometheusListenAddr = "127.0.0.1:26660"

	// Hashgram targets roughly 5 second blocks. Messaging and social traffic
	// does not go through consensus, so there is no reason to push block
	// times low and pay for it in orphan risk and bandwidth.
	cfg.Consensus.TimeoutCommit = 4_000_000_000 // 4s, as time.Duration

	// A larger peer set than the CometBFT default, because Hashgram expects
	// many community-run full nodes rather than a small validator clique.
	cfg.P2P.MaxNumInboundPeers = 60
	cfg.P2P.MaxNumOutboundPeers = 20

	// Peer exchange is one of the bootstrap mechanisms that keeps the network
	// discoverable without a central directory.
	cfg.P2P.PexReactor = true

	return cfg
}

// HashgramAppConfig extends the SDK server config with Hashgram-specific
// fields that node roles and the operator CLI read.
type HashgramAppConfig struct {
	serverconfig.Config `mapstructure:",squash"`

	Hashgram HashgramConfig `mapstructure:"hashgram"`
}

// HashgramConfig is the [hashgram] section of app.toml.
type HashgramConfig struct {
	// NetworkID pins the network this node believes it belongs to.
	NetworkID string `mapstructure:"network-id"`

	// GenesisHash pins the expected genesis hash. A node whose genesis file
	// does not hash to this value refuses to start, which is what turns
	// "somebody handed me a genesis.json" into a verifiable claim.
	GenesisHash string `mapstructure:"genesis-hash"`

	// Roles is the comma-separated list of node roles this machine serves,
	// for example "validator,bootstrap" or "relay,store".
	Roles string `mapstructure:"roles"`
}

// initAppConfig returns the app.toml template and defaults.
func initAppConfig() (string, interface{}) {
	srvCfg := serverconfig.DefaultConfig()

	// A non-zero minimum gas price by default. Leaving this empty makes the
	// node halt on startup; setting it to zero invites free spam, which on a
	// chain that also carries identity and username registrations is a real
	// griefing vector.
	srvCfg.MinGasPrices = "0.0025" + hgparams.BaseCoinDenom

	// Public read APIs are opt-in and, when enabled, still bind loopback so
	// that exposure is an explicit reverse-proxy decision.
	srvCfg.API.Enable = false
	srvCfg.API.Address = "tcp://127.0.0.1:1317"
	srvCfg.API.Swagger = false
	srvCfg.GRPC.Enable = true
	srvCfg.GRPC.Address = "127.0.0.1:9090"
	srvCfg.GRPCWeb.Enable = false

	srvCfg.Telemetry.Enabled = true
	srvCfg.Telemetry.PrometheusRetentionTime = 60

	// Keep enough history for state sync serving and for an indexer rebuild
	// without unbounded disk growth.
	srvCfg.StateSync.SnapshotInterval = 1000
	srvCfg.StateSync.SnapshotKeepRecent = 5

	customCfg := HashgramAppConfig{
		Config: *srvCfg,
		Hashgram: HashgramConfig{
			// Deliberately empty: `hashgramctl init-mainnet-genesis` and
			// `hashgramctl join-mainnet` write real values. A hardcoded
			// default here would let a node silently accept the wrong network.
			NetworkID:   "",
			GenesisHash: "",
			Roles:       "",
		},
	}

	template := serverconfig.DefaultConfigTemplate + `
###############################################################################
###                          Hashgram Configuration                         ###
###############################################################################

[hashgram]

# network-id pins the network this node belongs to, for example
# "hashgram-mainnet". Leave empty only before the node has been initialised.
network-id = "{{ .Hashgram.NetworkID }}"

# genesis-hash is the expected lowercase hex sha256 of the genesis file. The
# node refuses to start if the genesis file on disk does not match. This is
# what makes "is this really Hashgram Mainnet?" a checkable question.
# Verify independently with: sha256sum ~/.hashgram/config/genesis.json
genesis-hash = "{{ .Hashgram.GenesisHash }}"

# roles is the comma-separated list of node roles this machine serves.
# Valid roles: validator, relay, store, media, indexer, bootstrap, call, safety
roles = "{{ .Hashgram.Roles }}"
`

	return template, customCfg
}

func initRootCmd(rootCmd *cobra.Command, txConfig client.TxConfig, basicManager module.BasicManager) {
	rootCmd.AddCommand(
		genutilcli.InitCmd(basicManager, app.DefaultNodeHome),
		debug.Cmd(),
		confixcmd.ConfigCommand(),
		pruning.Cmd(newApp, app.DefaultNodeHome),
		snapshot.Cmd(newApp),
	)

	server.AddCommandsWithStartCmdOptions(
		rootCmd,
		app.DefaultNodeHome,
		newApp,
		appExport,
		server.StartCmdOptions{},
	)

	rootCmd.AddCommand(
		server.StatusCommand(),
		genesisCommand(txConfig, basicManager),
		queryCommand(),
		txCommand(),
		keys.Commands(),
	)
}

func genesisCommand(txConfig client.TxConfig, basicManager module.BasicManager, extra ...*cobra.Command) *cobra.Command {
	cmd := genutilcli.Commands(txConfig, basicManager, app.DefaultNodeHome)
	for _, c := range extra {
		cmd.AddCommand(c)
	}
	return cmd
}

func queryCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        "query",
		Aliases:                    []string{"q"},
		Short:                      "Querying subcommands",
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(
		rpc.WaitTxCmd(),
		server.QueryBlockCmd(),
		server.QueryBlocksCmd(),
		server.QueryBlockResultsCmd(),
		authcmd.QueryTxCmd(),
		authcmd.QueryTxsByEventsCmd(),
	)
	return cmd
}

func txCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        "tx",
		Short:                      "Transactions subcommands",
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}
	cmd.AddCommand(
		authcmd.GetSignCommand(),
		authcmd.GetSignBatchCommand(),
		authcmd.GetMultiSignCommand(),
		authcmd.GetMultiSignBatchCmd(),
		authcmd.GetValidateSignaturesCommand(),
		authcmd.GetBroadcastCommand(),
		authcmd.GetEncodeCommand(),
		authcmd.GetDecodeCommand(),
		authcmd.GetSimulateCmd(),
	)
	return cmd
}

func newApp(logger log.Logger, db dbm.DB, traceStore io.Writer, appOpts servertypes.AppOptions) servertypes.Application {
	return app.NewHashgramApp(
		logger, db, traceStore, true, appOpts,
		server.DefaultBaseappOptions(appOpts)...,
	)
}

func appExport(
	logger log.Logger,
	db dbm.DB,
	traceStore io.Writer,
	height int64,
	forZeroHeight bool,
	jailAllowedAddrs []string,
	appOpts servertypes.AppOptions,
	modulesToExport []string,
) (servertypes.ExportedApp, error) {
	viperOpts, ok := appOpts.(*viper.Viper)
	if !ok {
		return servertypes.ExportedApp{}, errors.New("appOpts is not *viper.Viper")
	}
	viperOpts.Set(server.FlagInvCheckPeriod, 1)
	appOpts = viperOpts

	var hgApp *app.HashgramApp
	if height != -1 {
		hgApp = app.NewHashgramApp(logger, db, traceStore, false, appOpts)
		if err := hgApp.LoadHeight(height); err != nil {
			return servertypes.ExportedApp{}, err
		}
	} else {
		hgApp = app.NewHashgramApp(logger, db, traceStore, true, appOpts)
	}

	return hgApp.ExportAppStateAndValidators(forZeroHeight, jailAllowedAddrs, modulesToExport)
}
