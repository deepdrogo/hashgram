// Package serviceproof implements the x/serviceproof module: Proof of Useful
// Service, which users see as "Hashgram Mining".
//
// There is no pointless hashing here, and nothing is paid for merely
// existing. Every role is rewarded only against evidence that survives an
// adversarial reading:
//
//   - Storage is paid on verified byte-hours. Bytes come from chain-recorded
//     assignments, not from the operator's own declaration, and are scaled by
//     the success rate on chain-issued Merkle challenges whose chunk indices
//     the operator cannot predict. Declaring a petabyte and being assigned
//     nothing earns nothing.
//
//   - Relay, retrieval and call work is paid on receipts signed by the
//     clients that were served. A provider cannot manufacture credit by
//     reporting numbers.
//
// Two attacks survive signature checking and are handled at the accounting
// layer, because no signature can rule them out:
//
//   - A provider serving itself is rejected outright and scored as fraud.
//   - Two nodes trading fake traffic can each legitimately sign for the
//     other. What defeats them is the per-counterparty concentration cap:
//     credit sourced overwhelmingly from one counterparty is discounted to a
//     fraction, so the ring must spend several times the resources to earn
//     what its receipts claim.
//
// Rewards come from a finite 500,000,000 HASH reserve funded once at genesis.
// There is no mint on this chain, so the reserve can only shrink as it pays
// out and grow only when slashed bond returns to it. Emission is a declining
// fraction of what remains, which is provably bounded and has no cliff.
//
// Honest limitations are documented in docs/SERVICE_REWARDS.md. In
// particular: a storage challenge proves a provider can produce a chunk hash
// and Merkle path at challenge time, not that the provider stores the data
// locally rather than fetching it from a colluding peer. Bounded challenge
// response windows raise the cost of that strategy without eliminating it.
package serviceproof

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

	"github.com/hashgram/hashgram/x/serviceproof/client/cli"
	"github.com/hashgram/hashgram/x/serviceproof/keeper"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// ConsensusVersion is the module's state-machine version.
const ConsensusVersion = 1

var (
	_ module.AppModuleBasic = AppModule{}
	_ module.HasGenesis     = AppModule{}
	_ module.HasServices    = AppModule{}

	_ appmodule.AppModule       = AppModule{}
	_ appmodule.HasBeginBlocker = AppModule{}
)

// AppModule implements the x/serviceproof module.
type AppModule struct {
	cdc    codec.Codec
	keeper keeper.Keeper
}

// NewAppModule constructs the x/serviceproof app module.
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

// GetTxCmd returns the transaction CLI.
func (AppModule) GetTxCmd() *cobra.Command { return cli.GetTxCmd() }

// GetQueryCmd returns the query CLI.
func (AppModule) GetQueryCmd() *cobra.Command { return cli.GetQueryCmd() }

// DefaultGenesis returns genesis with no registered assigners.
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

// BeginBlock advances epochs, settles rewards and expires challenges.
func (am AppModule) BeginBlock(ctx context.Context) error {
	return am.keeper.BeginBlocker(sdk.UnwrapSDKContext(ctx))
}
