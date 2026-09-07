// Package cmd wires the hashgramd command tree: the CometBFT and Cosmos SDK
// server commands, key management, and the genesis subcommands.
package cmd

import (
	"os"

	dbm "github.com/cosmos/cosmos-db"
	"github.com/spf13/cobra"

	"cosmossdk.io/log"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/config"
	nodeservice "github.com/cosmos/cosmos-sdk/client/grpc/node"
	"github.com/cosmos/cosmos-sdk/server"
	simtestutil "github.com/cosmos/cosmos-sdk/testutil/sims"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	"github.com/hashgram/hashgram/app"
)

// NewRootCmd builds the hashgramd root command.
func NewRootCmd() *cobra.Command {
	// A throwaway in-memory app instance gives us the real codec, interface
	// registry and tx config, so client-side encoding always matches the
	// state machine this binary contains. Building these by hand is how
	// chains end up with a CLI that cannot decode its own transactions.
	tempApp := app.NewHashgramApp(
		log.NewNopLogger(),
		dbm.NewMemDB(),
		nil,
		true,
		simtestutil.NewAppOptionsWithFlagHome(app.DefaultNodeHome),
	)

	initClientCtx := client.Context{}.
		WithCodec(tempApp.AppCodec()).
		WithInterfaceRegistry(tempApp.InterfaceRegistry()).
		WithTxConfig(tempApp.TxConfig()).
		WithLegacyAmino(tempApp.LegacyAmino()).
		WithInput(os.Stdin).
		WithAccountRetriever(authtypes.AccountRetriever{}).
		WithHomeDir(app.DefaultNodeHome).
		WithViper("HASHGRAM")

	rootCmd := &cobra.Command{
		Use:   "hashgramd",
		Short: "Hashgram blockchain node",
		Long: `hashgramd is the Hashgram Mainnet node.

It runs consensus, serves the public read RPC and the local administrative
RPC, and signs blocks when a validator key is configured.

Hashgram Mainnet identity:
  network      Hashgram Mainnet
  network id   hashgram-mainnet
  chain id     hashgram-1
  coin         HASH (base denomination uhash, 1 HASH = 1000000 uhash)

Most operators should use 'hashgramctl' instead of calling hashgramd
directly. See docs/OPERATIONS.md.`,
		SilenceErrors: true,
		PersistentPreRunE: func(cmd *cobra.Command, _ []string) error {
			cmd.SetOut(cmd.OutOrStdout())
			cmd.SetErr(cmd.ErrOrStderr())

			clientCtx := initClientCtx.WithCmdContext(cmd.Context())
			clientCtx, err := client.ReadPersistentCommandFlags(clientCtx, cmd.Flags())
			if err != nil {
				return err
			}
			clientCtx, err = config.ReadFromClientConfig(clientCtx)
			if err != nil {
				return err
			}
			if err := client.SetCmdClientContextHandler(clientCtx, cmd); err != nil {
				return err
			}

			customAppTemplate, customAppConfig := initAppConfig()
			customCMTConfig := initCometBFTConfig()

			return server.InterceptConfigsPreRunHandler(cmd, customAppTemplate, customAppConfig, customCMTConfig)
		},
	}

	initRootCmd(rootCmd, tempApp.TxConfig(), tempApp.BasicModuleManager)

	autoCliOpts := tempApp.AutoCliOpts()
	autoCliOpts.ClientCtx = initClientCtx

	nodeCmds := nodeservice.NewNodeCommands()
	autoCliOpts.ModuleOptions[nodeCmds.Name()] = nodeCmds.AutoCLIOptions()

	if err := autoCliOpts.EnhanceRootCommand(rootCmd); err != nil {
		panic(err)
	}

	return rootCmd
}
