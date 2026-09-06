// Package network implements the x/network module: the on-chain
// cryptographic identity of a Hashgram network and its fork-isolation policy.
//
// The module has no messages. The identity is written once at InitGenesis and
// read thereafter; changing it requires a store migration inside a
// coordinated binary upgrade. This is what makes "copy the software" and
// "become Hashgram Mainnet" different operations: a fork starting from a
// different genesis file has a different genesis hash, and every Hashgram
// peer handshake rejects it.
package network

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

	"github.com/hashgram/hashgram/x/network/client/cli"
	"github.com/hashgram/hashgram/x/network/keeper"
	"github.com/hashgram/hashgram/x/network/types"
)

// ConsensusVersion is the module's state-machine version.
const ConsensusVersion = 1

var (
	_ module.AppModuleBasic = AppModule{}
	_ module.HasGenesis     = AppModule{}
	_ module.HasServices    = AppModule{}

	_ appmodule.AppModule = AppModule{}
)

// AppModule implements the x/network module.
type AppModule struct {
	cdc    codec.Codec
	keeper keeper.Keeper
}

// NewAppModule constructs the x/network app module.
func NewAppModule(cdc codec.Codec, k keeper.Keeper) AppModule {
	return AppModule{cdc: cdc, keeper: k}
}

// IsAppModule implements appmodule.AppModule.
func (AppModule) IsAppModule() {}

// IsOnePerModuleType implements depinject.OnePerModuleType.
func (AppModule) IsOnePerModuleType() {}

// Name returns the module name.
func (AppModule) Name() string { return types.ModuleName }

// RegisterLegacyAminoCodec registers amino types. x/network has none.
func (AppModule) RegisterLegacyAminoCodec(cdc *codec.LegacyAmino) {
	types.RegisterLegacyAminoCodec(cdc)
}

// RegisterInterfaces registers interface implementations. x/network has none.
func (AppModule) RegisterInterfaces(reg codectypes.InterfaceRegistry) {
	types.RegisterInterfaces(reg)
}

// RegisterGRPCGatewayRoutes wires the REST gateway.
func (AppModule) RegisterGRPCGatewayRoutes(clientCtx client.Context, mux *gwruntime.ServeMux) {
	if err := types.RegisterQueryHandlerClient(context.Background(), mux, types.NewQueryClient(clientCtx)); err != nil {
		panic(err)
	}
}

// RegisterServices registers the query service. There is no Msg service:
// the network identity is not transaction-mutable.
func (am AppModule) RegisterServices(cfg module.Configurator) {
	types.RegisterQueryServer(cfg.QueryServer(), keeper.NewQuerier(am.keeper))
}

// GetTxCmd returns nil: x/network has no transactions.
func (AppModule) GetTxCmd() *cobra.Command { return nil }

// GetQueryCmd returns the query CLI.
func (AppModule) GetQueryCmd() *cobra.Command { return cli.GetQueryCmd() }

// DefaultGenesis returns the default (devnet) genesis.
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
//
// A failure here panics, which aborts chain start. That is the correct
// outcome: continuing would mean running a node whose idea of which network
// it is on has already been shown to be wrong.
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
