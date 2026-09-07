// Package treasury implements the x/treasury module: the named genesis
// allocations other than the Founder's, held so that each is independently
// visible on chain.
//
// The specification requires every allocation to be transparently visible.
// Merging them into the distribution community pool would satisfy the letter
// of that and not the substance: "how much is left in developer grants?"
// would stop being answerable. So each reserve gets its own module
// sub-account with a deterministic address and an initial figure that never
// changes, which makes the difference between initial and live balance
// exactly what has been spent.
//
// Only governance can move these funds. There is no founder key, admin key or
// operator key with authority here, and every disbursement is recorded with
// its memo so the history does not depend on anybody's spreadsheet.
package treasury

import (
	"context"
	"encoding/json"
	"fmt"

	gwruntime "github.com/grpc-ecosystem/grpc-gateway/runtime"
	"github.com/spf13/cobra"

	"cosmossdk.io/core/appmodule"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"

	"github.com/hashgram/hashgram/x/treasury/keeper"
	"github.com/hashgram/hashgram/x/treasury/types"
)

// ConsensusVersion is the module's state-machine version.
const ConsensusVersion = 1

var (
	_ module.AppModuleBasic = AppModule{}
	_ module.HasGenesis     = AppModule{}
	_ module.HasServices    = AppModule{}

	_ appmodule.AppModule = AppModule{}
)

// AppModule implements the x/treasury module.
type AppModule struct {
	cdc    codec.Codec
	keeper keeper.Keeper
}

// NewAppModule constructs the x/treasury app module.
func NewAppModule(cdc codec.Codec, k keeper.Keeper) AppModule {
	return AppModule{cdc: cdc, keeper: k}
}

// IsAppModule implements appmodule.AppModule.
func (AppModule) IsAppModule() {}

// IsOnePerModuleType implements depinject.OnePerModuleType.
func (AppModule) IsOnePerModuleType() {}

// Name returns the module name.
func (AppModule) Name() string { return types.ModuleName }

// RegisterLegacyAminoCodec registers amino types.
func (AppModule) RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	types.RegisterLegacyAminoCodec(cdc)
}

// RegisterInterfaces registers message implementations.
func (AppModule) RegisterInterfaces(reg codectypes.InterfaceRegistry) {
	types.RegisterInterfaces(reg)
}

// RegisterGRPCGatewayRoutes wires the REST gateway.
func (AppModule) RegisterGRPCGatewayRoutes(clientCtx client.Context, mux *gwruntime.ServeMux) {
	if err := types.RegisterQueryHandlerClient(context.Background(), mux, types.NewQueryClient(clientCtx)); err != nil {
		panic(err)
	}
}

// RegisterServices registers the Msg and Query services.
func (am AppModule) RegisterServices(cfg module.Configurator) {
	types.RegisterMsgServer(cfg.MsgServer(), keeper.NewMsgServerImpl(am.keeper))
	types.RegisterQueryServer(cfg.QueryServer(), keeper.NewQuerier(am.keeper))
}

// GetTxCmd returns nil: spends happen through governance proposals, not a CLI.
func (AppModule) GetTxCmd() *cobra.Command { return nil }

// GetQueryCmd returns the query CLI.
func (AppModule) GetQueryCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:                        types.ModuleName,
		Short:                      "Query the named genesis allocations",
		DisableFlagParsing:         true,
		SuggestionsMinimumDistance: 2,
		RunE:                       client.ValidateCmd,
	}

	reserves := &cobra.Command{
		Use:   "reserves",
		Short: "Show every named allocation with its live balance",
		Long: `Show every named allocation with its live balance.

  initial  what the reserve was funded with at genesis; never changes
  spent    cumulative governance disbursements
  balance  the live on-chain balance, read from the bank

Cross-check any figure independently:

  hashgramd query bank balances <address>`,
		Args: cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Reserves(cmd.Context(), &types.QueryReservesRequest{})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(reserves)

	one := &cobra.Command{
		Use:   "reserve [name]",
		Short: "Show one named allocation",
		Args:  cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			res, err := types.NewQueryClient(ctx).Reserve(cmd.Context(),
				&types.QueryReserveRequest{Name: args[0]})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	flags.AddQueryFlagsToCmd(one)

	history := &cobra.Command{
		Use:   "disbursements",
		Short: "Show the governance spend history",
		Args:  cobra.NoArgs,
		RunE: func(cmd *cobra.Command, _ []string) error {
			ctx, err := client.GetClientQueryContext(cmd)
			if err != nil {
				return err
			}
			reserve, _ := cmd.Flags().GetString("reserve")
			res, err := types.NewQueryClient(ctx).Disbursements(cmd.Context(),
				&types.QueryDisbursementsRequest{Reserve: reserve})
			if err != nil {
				return err
			}
			return ctx.PrintProto(res)
		},
	}
	history.Flags().String("reserve", "", "filter to one allocation")
	flags.AddQueryFlagsToCmd(history)

	cmd.AddCommand(reserves, one, history)
	return cmd
}

// DefaultGenesis returns an empty treasury.
func (am AppModule) DefaultGenesis(cdc codec.JSONCodec) json.RawMessage {
	return cdc.MustMarshalJSON(types.DefaultGenesis())
}

// ValidateGenesis validates genesis JSON.
func (am AppModule) ValidateGenesis(cdc codec.JSONCodec, _ client.TxEncodingConfig, bz json.RawMessage) error {
	var gs types.GenesisState
	if err := cdc.UnmarshalJSON(bz, &gs); err != nil {
		return fmt.Errorf("failed to unmarshal %s genesis state: %w", types.ModuleName, err)
	}
	return gs.Validate()
}

// InitGenesis initialises module state.
func (am AppModule) InitGenesis(ctx sdk.Context, cdc codec.JSONCodec, bz json.RawMessage) {
	var gs types.GenesisState
	if err := cdc.UnmarshalJSON(bz, &gs); err != nil {
		panic(fmt.Errorf("failed to unmarshal %s genesis state: %w", types.ModuleName, err))
	}
	if err := am.keeper.InitGenesis(ctx, gs); err != nil {
		panic(fmt.Errorf("x/%s InitGenesis: %w", types.ModuleName, err))
	}
}

// ExportGenesis exports module state.
func (am AppModule) ExportGenesis(ctx sdk.Context, cdc codec.JSONCodec) json.RawMessage {
	gs, err := am.keeper.ExportGenesis(ctx)
	if err != nil {
		panic(err)
	}
	return cdc.MustMarshalJSON(gs)
}

// ConsensusVersion implements module.HasConsensusVersion.
func (AppModule) ConsensusVersion() uint64 { return ConsensusVersion }
