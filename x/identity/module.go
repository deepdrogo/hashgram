// Package identity implements the x/identity module: decentralised
// cryptographic identity, multi-device authorisation and social recovery.
//
// Only public keys live here. The chain never receives, stores or transports
// a private key, and no Hashgram server holds one: the root key lives on the
// user's own hardware and each device holds its own key. This is the concrete
// reason a fully compromised Hashgram VPS cannot impersonate users. There is
// nothing on it to steal.
//
// The root key is deliberately not the account key that pays gas. Signing a
// transaction is a routine act; authorising a new device is not, and they
// should not share a key whose exposure is routine. Consequently the
// authority to add a device is the root-signed certificate, not the
// transaction signature: whoever pays the gas cannot insert a device.
//
// Root-key rotation is meaningful rather than cosmetic because the rotation
// count is inside the signed certificate preimage, so certificates issued
// under a superseded root key stop verifying. Rotation itself must be signed
// by the key being replaced, so an attacker holding only the account key
// cannot install their own root key and take over the identity's devices.
//
// Social recovery is guardian-threshold plus a mandatory delay. The delay is
// the substance of the mechanism: it is the window in which a user whose
// guardians have been socially engineered can notice and cancel. Without it,
// persuading the threshold would be an immediately final takeover. The chain
// holds no seed phrase, no key share and no secret of any kind for recovery,
// because a recovery mechanism that requires the network to hold a secret is
// one whose compromise loses every user at once.
package identity

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

	"github.com/hashgram/hashgram/x/identity/client/cli"
	"github.com/hashgram/hashgram/x/identity/keeper"
	"github.com/hashgram/hashgram/x/identity/types"
)

// ConsensusVersion is the module's state-machine version.
const ConsensusVersion = 1

var (
	_ module.AppModuleBasic = AppModule{}
	_ module.HasGenesis     = AppModule{}
	_ module.HasServices    = AppModule{}

	_ appmodule.AppModule = AppModule{}
)

// AppModule implements the x/identity module.
type AppModule struct {
	cdc    codec.Codec
	keeper keeper.Keeper
}

// NewAppModule constructs the x/identity app module.
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

// GetTxCmd returns nil: identity transactions carry signed certificates that
// a CLI cannot usefully assemble from flags. The developer test client and
// the client SDKs build them.
func (AppModule) GetTxCmd() *cobra.Command { return nil }

// GetQueryCmd returns the query CLI.
func (AppModule) GetQueryCmd() *cobra.Command { return cli.GetQueryCmd() }

// DefaultGenesis returns the default genesis state.
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
