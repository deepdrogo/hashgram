// Package founder implements the x/founder module: the Founder beneficiary
// address, the Founder share of qualifying protocol fee revenue, and the
// accrual and payout ledger.
//
// Three properties are worth stating plainly, because the whole point of this
// module is that they be checkable rather than promised:
//
//  1. The Founder share applies only to fee revenue the protocol itself
//     collected. It is not a tax on transferred value. Sending 100 HASH
//     delivers 100 HASH.
//
//  2. The share cannot be raised past a compile-time ceiling. Governance may
//     lower it; raising it requires a new binary the validator set adopts.
//
//  3. The Founder private key is never needed on Hashgram infrastructure.
//     Payouts are pushed to the configured address automatically, and
//     MsgClaimFounderRevenue is permissionless: anyone may trigger a payout,
//     and it always goes to the configured beneficiary.
//
// Being the beneficiary confers no privilege over consensus, over other
// accounts' balances, or over this module's own configuration.
package founder

import (
	"context"
	"encoding/json"
	"fmt"

	gwruntime "github.com/grpc-ecosystem/grpc-gateway/runtime"
	"github.com/spf13/cobra"

	"cosmossdk.io/core/appmodule"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"

	"github.com/hashgram/hashgram/x/founder/client/cli"
	"github.com/hashgram/hashgram/x/founder/keeper"
	"github.com/hashgram/hashgram/x/founder/types"
)

// ConsensusVersion is the module's state-machine version.
const ConsensusVersion = 1

var (
	_ module.AppModuleBasic = AppModule{}
	_ module.HasGenesis     = AppModule{}
	_ module.HasServices    = AppModule{}

	_ appmodule.AppModule     = AppModule{}
	_ appmodule.HasEndBlocker = AppModule{}
)

// AppModule implements the x/founder module.
type AppModule struct {
	cdc    codec.Codec
	keeper keeper.Keeper
}

// NewAppModule constructs the x/founder app module.
func NewAppModule(cdc codec.Codec, k keeper.Keeper) AppModule {
	return AppModule{cdc: cdc, keeper: k}
}

// IsAppModule implements appmodule.AppModule.
func (AppModule) IsAppModule() {}

// IsOnePerModuleType implements depinject.OnePerModuleType.
func (AppModule) IsOnePerModuleType() {}

// Name returns the module name.
func (AppModule) Name() string { return types.ModuleName }

// RegisterLegacyAminoCodec registers amino types for hardware-wallet signing.
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

// GetTxCmd returns the transaction CLI.
func (AppModule) GetTxCmd() *cobra.Command { return cli.GetTxCmd() }

// GetQueryCmd returns the query CLI.
func (AppModule) GetQueryCmd() *cobra.Command { return cli.GetQueryCmd() }

// DefaultGenesis returns genesis with no beneficiary configured.
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

// EndBlock performs the periodic automatic payout.
func (am AppModule) EndBlock(ctx context.Context) error {
	return am.keeper.EndBlocker(sdk.UnwrapSDKContext(ctx))
}
