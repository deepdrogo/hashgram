// Package app wires the Hashgram blockchain application.
//
// Hashgram is a Cosmos SDK v0.53 / CometBFT v0.38 chain with a deliberately
// small trusted surface. Two wiring decisions are load-bearing and are called
// out here rather than buried:
//
//  1. There is no x/mint module. Hashgram has a fixed canonical supply of
//     1,000,000,000 HASH, all of which is created in the genesis block. Rather
//     than install the inflation module and configure it to zero, the module
//     is absent, and no module account holds the authtypes.Minter permission.
//     The result is that `MintCoins` cannot succeed for any module on this
//     chain: "no hidden mint" is a property of the wiring, not of a parameter
//     value that governance could later change. Validator and useful-service
//     rewards are paid by *transferring* from the finite reserves established
//     at genesis (see x/serviceproof), plus real transaction fees.
//
//  2. There is no x/circuit module. x/circuit lets an authority pause message
//     execution chain-wide. Even though its authority would be the governance
//     module rather than the Founder, shipping a chain-wide pause switch
//     contradicts the project's "no kill switch" requirement, so it is not
//     compiled in.
//
// See docs/ARCHITECTURE.md and docs/TOKENOMICS.md.
package app

import (
	"encoding/json"
	"fmt"
	"io"
	"maps"

	abci "github.com/cometbft/cometbft/abci/types"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/cosmos/gogoproto/proto"
	"github.com/spf13/cast"

	autocliv1 "cosmossdk.io/api/cosmos/autocli/v1"
	reflectionv1 "cosmossdk.io/api/cosmos/reflection/v1"
	"cosmossdk.io/client/v2/autocli"
	"cosmossdk.io/core/appmodule"
	"cosmossdk.io/log"
	storetypes "cosmossdk.io/store/types"
	"cosmossdk.io/x/evidence"
	evidencekeeper "cosmossdk.io/x/evidence/keeper"
	evidencetypes "cosmossdk.io/x/evidence/types"
	"cosmossdk.io/x/feegrant"
	feegrantkeeper "cosmossdk.io/x/feegrant/keeper"
	feegrantmodule "cosmossdk.io/x/feegrant/module"
	"cosmossdk.io/x/tx/signing"
	"cosmossdk.io/x/upgrade"
	upgradekeeper "cosmossdk.io/x/upgrade/keeper"
	upgradetypes "cosmossdk.io/x/upgrade/types"

	"github.com/cosmos/cosmos-sdk/baseapp"
	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/grpc/cmtservice"
	nodeservice "github.com/cosmos/cosmos-sdk/client/grpc/node"
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/address"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	runtimeservices "github.com/cosmos/cosmos-sdk/runtime/services"
	"github.com/cosmos/cosmos-sdk/server"
	"github.com/cosmos/cosmos-sdk/server/api"
	"github.com/cosmos/cosmos-sdk/server/config"
	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	"github.com/cosmos/cosmos-sdk/std"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"
	"github.com/cosmos/cosmos-sdk/x/auth"
	authcodec "github.com/cosmos/cosmos-sdk/x/auth/codec"
	authkeeper "github.com/cosmos/cosmos-sdk/x/auth/keeper"
	authsims "github.com/cosmos/cosmos-sdk/x/auth/simulation"
	authtx "github.com/cosmos/cosmos-sdk/x/auth/tx"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/cosmos/cosmos-sdk/x/auth/vesting"
	vestingtypes "github.com/cosmos/cosmos-sdk/x/auth/vesting/types"
	"github.com/cosmos/cosmos-sdk/x/authz"
	authzkeeper "github.com/cosmos/cosmos-sdk/x/authz/keeper"
	authzmodule "github.com/cosmos/cosmos-sdk/x/authz/module"
	"github.com/cosmos/cosmos-sdk/x/bank"
	bankkeeper "github.com/cosmos/cosmos-sdk/x/bank/keeper"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/cosmos/cosmos-sdk/x/consensus"
	consensusparamkeeper "github.com/cosmos/cosmos-sdk/x/consensus/keeper"
	consensusparamtypes "github.com/cosmos/cosmos-sdk/x/consensus/types"
	distr "github.com/cosmos/cosmos-sdk/x/distribution"
	distrkeeper "github.com/cosmos/cosmos-sdk/x/distribution/keeper"
	distrtypes "github.com/cosmos/cosmos-sdk/x/distribution/types"
	"github.com/cosmos/cosmos-sdk/x/genutil"
	genutiltypes "github.com/cosmos/cosmos-sdk/x/genutil/types"
	"github.com/cosmos/cosmos-sdk/x/gov"
	govclient "github.com/cosmos/cosmos-sdk/x/gov/client"
	govkeeper "github.com/cosmos/cosmos-sdk/x/gov/keeper"
	govtypes "github.com/cosmos/cosmos-sdk/x/gov/types"
	govv1beta1 "github.com/cosmos/cosmos-sdk/x/gov/types/v1beta1"
	"github.com/cosmos/cosmos-sdk/x/slashing"
	slashingkeeper "github.com/cosmos/cosmos-sdk/x/slashing/keeper"
	slashingtypes "github.com/cosmos/cosmos-sdk/x/slashing/types"
	"github.com/cosmos/cosmos-sdk/x/staking"
	stakingkeeper "github.com/cosmos/cosmos-sdk/x/staking/keeper"
	stakingtypes "github.com/cosmos/cosmos-sdk/x/staking/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// BuildDate is stamped by the linker; see the Makefile.
var BuildDate = "unknown"

// DefaultNodeHome is the default hashgramd home directory.
var DefaultNodeHome string

// AccountAddressPrefix is re-exported for convenience.
const AccountAddressPrefix = hgparams.Bech32Prefix

// maccPerms declares the module accounts and their permissions.
//
// Note what is NOT here: no entry carries authtypes.Minter. Hashgram has no
// module that is permitted to create HASH. Compare with the Cosmos SDK default
// where minttypes.ModuleName holds {Minter}.
//
// Burner is granted where the SDK requires it for correctness:
//   - staking pools burn slashed tokens,
//   - gov burns rejected/failed proposal deposits.
//
// Burning only ever reduces supply, so it cannot threaten the supply ceiling.
var maccPerms = map[string][]string{
	authtypes.FeeCollectorName:     nil,
	distrtypes.ModuleName:          nil,
	stakingtypes.BondedPoolName:    {authtypes.Burner, authtypes.Staking},
	stakingtypes.NotBondedPoolName: {authtypes.Burner, authtypes.Staking},
	govtypes.ModuleName:            {authtypes.Burner},
}

var (
	_ runtime.AppI            = (*HashgramApp)(nil)
	_ servertypes.Application = (*HashgramApp)(nil)
)

// HashgramApp is the Hashgram ABCI application.
type HashgramApp struct {
	*baseapp.BaseApp

	legacyAmino       *codec.LegacyAmino
	appCodec          codec.Codec
	txConfig          client.TxConfig
	interfaceRegistry codectypes.InterfaceRegistry

	keys map[string]*storetypes.KVStoreKey

	// Cosmos SDK keepers
	AccountKeeper         authkeeper.AccountKeeper
	BankKeeper            bankkeeper.BaseKeeper
	StakingKeeper         *stakingkeeper.Keeper
	SlashingKeeper        slashingkeeper.Keeper
	DistrKeeper           distrkeeper.Keeper
	GovKeeper             govkeeper.Keeper
	UpgradeKeeper         *upgradekeeper.Keeper
	EvidenceKeeper        evidencekeeper.Keeper
	ConsensusParamsKeeper consensusparamkeeper.Keeper
	FeeGrantKeeper        feegrantkeeper.Keeper
	AuthzKeeper           authzkeeper.Keeper

	ModuleManager      *module.Manager
	BasicModuleManager module.BasicManager

	configurator module.Configurator
}

func init() {
	// hgparams.SetSDKConfig is called by each binary's main() before cobra
	// construction; see cmd/hashgramd. The default home is resolved here.
	DefaultNodeHome = defaultNodeHome()
}

// NewHashgramApp constructs a Hashgram application.
func NewHashgramApp(
	logger log.Logger,
	db dbm.DB,
	traceStore io.Writer,
	loadLatest bool,
	appOpts servertypes.AppOptions,
	baseAppOptions ...func(*baseapp.BaseApp),
) *HashgramApp {
	interfaceRegistry, err := codectypes.NewInterfaceRegistryWithOptions(codectypes.InterfaceRegistryOptions{
		ProtoFiles: proto.HybridResolver,
		SigningOptions: signing.Options{
			AddressCodec: address.Bech32Codec{
				Bech32Prefix: hgparams.Bech32PrefixAccAddr,
			},
			ValidatorAddressCodec: address.Bech32Codec{
				Bech32Prefix: hgparams.Bech32PrefixValAddr,
			},
		},
	})
	if err != nil {
		panic(err)
	}

	appCodec := codec.NewProtoCodec(interfaceRegistry)
	legacyAmino := codec.NewLegacyAmino()
	txConfig := authtx.NewTxConfig(appCodec, authtx.DefaultSignModes)

	if err := interfaceRegistry.SigningContext().Validate(); err != nil {
		panic(err)
	}

	std.RegisterLegacyAminoCodec(legacyAmino)
	std.RegisterInterfaces(interfaceRegistry)

	bApp := baseapp.NewBaseApp(hgparams.AppName, logger, db, txConfig.TxDecoder(), baseAppOptions...)
	bApp.SetCommitMultiStoreTracer(traceStore)
	bApp.SetVersion(Version())
	bApp.SetInterfaceRegistry(interfaceRegistry)
	bApp.SetTxEncoder(txConfig.TxEncoder())

	keys := storetypes.NewKVStoreKeys(
		authtypes.StoreKey,
		banktypes.StoreKey,
		stakingtypes.StoreKey,
		distrtypes.StoreKey,
		slashingtypes.StoreKey,
		govtypes.StoreKey,
		consensusparamtypes.StoreKey,
		upgradetypes.StoreKey,
		feegrant.StoreKey,
		evidencetypes.StoreKey,
		authzkeeper.StoreKey,
	)

	if err := bApp.RegisterStreamingServices(appOpts, keys); err != nil {
		panic(err)
	}

	app := &HashgramApp{
		BaseApp:           bApp,
		legacyAmino:       legacyAmino,
		appCodec:          appCodec,
		txConfig:          txConfig,
		interfaceRegistry: interfaceRegistry,
		keys:              keys,
	}

	// The governance module account is the only authority for parameter
	// changes anywhere in this application. There is no separate admin key.
	govAuthority := authtypes.NewModuleAddress(govtypes.ModuleName).String()

	app.ConsensusParamsKeeper = consensusparamkeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[consensusparamtypes.StoreKey]),
		govAuthority,
		runtime.EventService{},
	)
	bApp.SetParamStore(app.ConsensusParamsKeeper.ParamsStore)

	app.AccountKeeper = authkeeper.NewAccountKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[authtypes.StoreKey]),
		authtypes.ProtoBaseAccount,
		maccPerms,
		authcodec.NewBech32Codec(hgparams.Bech32PrefixAccAddr),
		hgparams.Bech32PrefixAccAddr,
		govAuthority,
	)

	app.BankKeeper = bankkeeper.NewBaseKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[banktypes.StoreKey]),
		app.AccountKeeper,
		BlockedAddresses(),
		govAuthority,
		logger,
	)

	app.StakingKeeper = stakingkeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[stakingtypes.StoreKey]),
		app.AccountKeeper,
		app.BankKeeper,
		govAuthority,
		authcodec.NewBech32Codec(hgparams.Bech32PrefixValAddr),
		authcodec.NewBech32Codec(hgparams.Bech32PrefixConsAddr),
	)

	app.DistrKeeper = distrkeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[distrtypes.StoreKey]),
		app.AccountKeeper,
		app.BankKeeper,
		app.StakingKeeper,
		authtypes.FeeCollectorName,
		govAuthority,
	)

	app.SlashingKeeper = slashingkeeper.NewKeeper(
		appCodec,
		legacyAmino,
		runtime.NewKVStoreService(keys[slashingtypes.StoreKey]),
		app.StakingKeeper,
		govAuthority,
	)

	app.FeeGrantKeeper = feegrantkeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[feegrant.StoreKey]),
		app.AccountKeeper,
	)

	// StakingKeeper is held by pointer so these hooks land on the real keeper.
	app.StakingKeeper.SetHooks(
		stakingtypes.NewMultiStakingHooks(
			app.DistrKeeper.Hooks(),
			app.SlashingKeeper.Hooks(),
		),
	)

	app.AuthzKeeper = authzkeeper.NewKeeper(
		runtime.NewKVStoreService(keys[authzkeeper.StoreKey]),
		appCodec,
		app.MsgServiceRouter(),
		app.AccountKeeper,
	)

	skipUpgradeHeights := map[int64]bool{}
	for _, h := range cast.ToIntSlice(appOpts.Get(server.FlagUnsafeSkipUpgrades)) {
		skipUpgradeHeights[int64(h)] = true
	}
	homePath := cast.ToString(appOpts.Get(flags.FlagHome))
	app.UpgradeKeeper = upgradekeeper.NewKeeper(
		skipUpgradeHeights,
		runtime.NewKVStoreService(keys[upgradetypes.StoreKey]),
		appCodec,
		homePath,
		app.BaseApp,
		govAuthority,
	)

	govRouter := govv1beta1.NewRouter()
	govRouter.AddRoute(govtypes.RouterKey, govv1beta1.ProposalHandler)
	govConfig := govtypes.DefaultConfig()

	govKeeper := govkeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[govtypes.StoreKey]),
		app.AccountKeeper,
		app.BankKeeper,
		app.StakingKeeper,
		app.DistrKeeper,
		app.MsgServiceRouter(),
		govConfig,
		govAuthority,
	)
	govKeeper.SetLegacyRouter(govRouter)
	app.GovKeeper = *govKeeper.SetHooks(govtypes.NewMultiGovHooks())

	evidenceKeeper := evidencekeeper.NewKeeper(
		appCodec,
		runtime.NewKVStoreService(keys[evidencetypes.StoreKey]),
		app.StakingKeeper,
		app.SlashingKeeper,
		app.AccountKeeper.AddressCodec(),
		runtime.ProvideCometInfoService(),
	)
	app.EvidenceKeeper = *evidenceKeeper

	// ---------------------------------------------------------------------
	// Module manager
	// ---------------------------------------------------------------------

	app.ModuleManager = module.NewManager(
		genutil.NewAppModule(app.AccountKeeper, app.StakingKeeper, app, txConfig),
		auth.NewAppModule(appCodec, app.AccountKeeper, authsims.RandomGenesisAccounts, nil),
		vesting.NewAppModule(app.AccountKeeper, app.BankKeeper),
		bank.NewAppModule(appCodec, app.BankKeeper, app.AccountKeeper, nil),
		staking.NewAppModule(appCodec, app.StakingKeeper, app.AccountKeeper, app.BankKeeper, nil),
		slashing.NewAppModule(appCodec, app.SlashingKeeper, app.AccountKeeper, app.BankKeeper, app.StakingKeeper, nil, app.interfaceRegistry),
		distr.NewAppModule(appCodec, app.DistrKeeper, app.AccountKeeper, app.BankKeeper, app.StakingKeeper, nil),
		gov.NewAppModule(appCodec, &app.GovKeeper, app.AccountKeeper, app.BankKeeper, nil),
		evidence.NewAppModule(app.EvidenceKeeper),
		upgrade.NewAppModule(app.UpgradeKeeper, app.AccountKeeper.AddressCodec()),
		consensus.NewAppModule(appCodec, app.ConsensusParamsKeeper),
		feegrantmodule.NewAppModule(appCodec, app.AccountKeeper, app.BankKeeper, app.FeeGrantKeeper, app.interfaceRegistry),
		authzmodule.NewAppModule(appCodec, app.AuthzKeeper, app.AccountKeeper, app.BankKeeper, app.interfaceRegistry),
	)

	app.BasicModuleManager = module.NewBasicManagerFromManager(
		app.ModuleManager,
		map[string]module.AppModuleBasic{
			genutiltypes.ModuleName: genutil.NewAppModuleBasic(genutiltypes.DefaultMessageValidator),
			govtypes.ModuleName:     gov.NewAppModuleBasic([]govclient.ProposalHandler{}),
		})
	app.BasicModuleManager.RegisterLegacyAminoCodec(legacyAmino)
	app.BasicModuleManager.RegisterInterfaces(interfaceRegistry)

	// x/upgrade must run first so that a migration can execute before any
	// other module reads state written by an older version.
	app.ModuleManager.SetOrderPreBlockers(
		upgradetypes.ModuleName,
		authtypes.ModuleName,
	)

	// Slashing runs after distribution so the validator fee pool is empty when
	// slashing touches it, preserving the CanWithdraw invariant.
	app.ModuleManager.SetOrderBeginBlockers(
		distrtypes.ModuleName,
		slashingtypes.ModuleName,
		evidencetypes.ModuleName,
		stakingtypes.ModuleName,
		genutiltypes.ModuleName,
		authz.ModuleName,
	)

	app.ModuleManager.SetOrderEndBlockers(
		govtypes.ModuleName,
		stakingtypes.ModuleName,
		genutiltypes.ModuleName,
		feegrant.ModuleName,
	)

	// genutil must come after staking (pools need genesis-account tokens) and
	// after auth (it reads auth params).
	app.ModuleManager.SetOrderInitGenesis(
		authtypes.ModuleName,
		banktypes.ModuleName,
		distrtypes.ModuleName,
		stakingtypes.ModuleName,
		slashingtypes.ModuleName,
		govtypes.ModuleName,
		genutiltypes.ModuleName,
		evidencetypes.ModuleName,
		authz.ModuleName,
		feegrant.ModuleName,
		upgradetypes.ModuleName,
		vestingtypes.ModuleName,
		consensusparamtypes.ModuleName,
	)

	app.ModuleManager.SetOrderExportGenesis(
		consensusparamtypes.ModuleName,
		authtypes.ModuleName,
		banktypes.ModuleName,
		distrtypes.ModuleName,
		stakingtypes.ModuleName,
		slashingtypes.ModuleName,
		govtypes.ModuleName,
		genutiltypes.ModuleName,
		evidencetypes.ModuleName,
		authz.ModuleName,
		feegrant.ModuleName,
		upgradetypes.ModuleName,
		vestingtypes.ModuleName,
	)

	app.configurator = module.NewConfigurator(app.appCodec, app.MsgServiceRouter(), app.GRPCQueryRouter())
	if err := app.ModuleManager.RegisterServices(app.configurator); err != nil {
		panic(err)
	}

	app.RegisterUpgradeHandlers()

	autocliv1.RegisterQueryServer(app.GRPCQueryRouter(), runtimeservices.NewAutoCLIQueryService(app.ModuleManager.Modules))

	reflectionSvc, err := runtimeservices.NewReflectionService()
	if err != nil {
		panic(err)
	}
	reflectionv1.RegisterReflectionServiceServer(app.GRPCQueryRouter(), reflectionSvc)

	app.MountKVStores(keys)

	app.SetInitChainer(app.InitChainer)
	app.SetPreBlocker(app.PreBlocker)
	app.SetBeginBlocker(app.BeginBlocker)
	app.SetEndBlocker(app.EndBlocker)
	app.setAnteHandler(txConfig)
	app.setPostHandler()

	if loadLatest {
		if err := app.LoadLatestVersion(); err != nil {
			panic(fmt.Errorf("error loading last version: %w", err))
		}
	}

	return app
}

// Name returns the application name.
func (app *HashgramApp) Name() string { return app.BaseApp.Name() }

// PreBlocker runs before every block.
func (app *HashgramApp) PreBlocker(ctx sdk.Context, _ *abci.RequestFinalizeBlock) (*sdk.ResponsePreBlock, error) {
	return app.ModuleManager.PreBlock(ctx)
}

// BeginBlocker runs at the start of every block.
func (app *HashgramApp) BeginBlocker(ctx sdk.Context) (sdk.BeginBlock, error) {
	return app.ModuleManager.BeginBlock(ctx)
}

// EndBlocker runs at the end of every block.
func (app *HashgramApp) EndBlocker(ctx sdk.Context) (sdk.EndBlock, error) {
	return app.ModuleManager.EndBlock(ctx)
}

// Configurator exposes the module configurator for upgrade handlers.
func (app *HashgramApp) Configurator() module.Configurator { return app.configurator }

// InitChainer initialises state from genesis.
func (app *HashgramApp) InitChainer(ctx sdk.Context, req *abci.RequestInitChain) (*abci.ResponseInitChain, error) {
	var genesisState GenesisState
	if err := json.Unmarshal(req.AppStateBytes, &genesisState); err != nil {
		return nil, fmt.Errorf("failed to unmarshal genesis state: %w", err)
	}
	if err := app.UpgradeKeeper.SetModuleVersionMap(ctx, app.ModuleManager.GetVersionMap()); err != nil {
		return nil, err
	}
	return app.ModuleManager.InitGenesis(ctx, app.appCodec, genesisState)
}

// LoadHeight loads state at a specific height.
func (app *HashgramApp) LoadHeight(height int64) error { return app.LoadVersion(height) }

// LegacyAmino returns the legacy amino codec.
func (app *HashgramApp) LegacyAmino() *codec.LegacyAmino { return app.legacyAmino }

// AppCodec returns the application codec.
func (app *HashgramApp) AppCodec() codec.Codec { return app.appCodec }

// InterfaceRegistry returns the interface registry.
func (app *HashgramApp) InterfaceRegistry() codectypes.InterfaceRegistry {
	return app.interfaceRegistry
}

// TxConfig returns the transaction config.
func (app *HashgramApp) TxConfig() client.TxConfig { return app.txConfig }

// DefaultGenesis returns the default genesis state of all registered modules.
func (app *HashgramApp) DefaultGenesis() map[string]json.RawMessage {
	return app.BasicModuleManager.DefaultGenesis(app.appCodec)
}

// GetKey returns a KVStoreKey by name. Test helper.
func (app *HashgramApp) GetKey(storeKey string) *storetypes.KVStoreKey {
	return app.keys[storeKey]
}

// GetStoreKeys returns all mounted store keys.
func (app *HashgramApp) GetStoreKeys() []storetypes.StoreKey {
	out := make([]storetypes.StoreKey, 0, len(app.keys))
	for _, k := range app.keys {
		out = append(out, k)
	}
	return out
}

// SimulationManager satisfies runtime.AppI. Hashgram does not ship the SDK
// simulator; fuzzing is done with Go fuzz targets instead (see scripts/dev).
func (app *HashgramApp) SimulationManager() *module.SimulationManager { return nil }

// AutoCliOpts builds the autocli options for the client.
func (app *HashgramApp) AutoCliOpts() autocli.AppOptions {
	modules := make(map[string]appmodule.AppModule)
	for _, m := range app.ModuleManager.Modules {
		named, ok := m.(module.HasName)
		if !ok {
			continue
		}
		if am, ok := named.(appmodule.AppModule); ok {
			modules[named.Name()] = am
		}
	}

	return autocli.AppOptions{
		Modules:               modules,
		ModuleOptions:         runtimeservices.ExtractAutoCLIOptions(app.ModuleManager.Modules),
		AddressCodec:          authcodec.NewBech32Codec(hgparams.Bech32PrefixAccAddr),
		ValidatorAddressCodec: authcodec.NewBech32Codec(hgparams.Bech32PrefixValAddr),
		ConsensusAddressCodec: authcodec.NewBech32Codec(hgparams.Bech32PrefixConsAddr),
	}
}

// RegisterAPIRoutes registers gRPC-gateway routes.
func (app *HashgramApp) RegisterAPIRoutes(apiSvr *api.Server, apiConfig config.APIConfig) {
	clientCtx := apiSvr.ClientCtx
	authtx.RegisterGRPCGatewayRoutes(clientCtx, apiSvr.GRPCGatewayRouter)
	cmtservice.RegisterGRPCGatewayRoutes(clientCtx, apiSvr.GRPCGatewayRouter)
	nodeservice.RegisterGRPCGatewayRoutes(clientCtx, apiSvr.GRPCGatewayRouter)
	app.BasicModuleManager.RegisterGRPCGatewayRoutes(clientCtx, apiSvr.GRPCGatewayRouter)

	if err := server.RegisterSwaggerAPI(apiSvr.ClientCtx, apiSvr.Router, apiConfig.Swagger); err != nil {
		panic(err)
	}
}

// RegisterTxService registers the tx service.
func (app *HashgramApp) RegisterTxService(clientCtx client.Context) {
	authtx.RegisterTxService(app.BaseApp.GRPCQueryRouter(), clientCtx, app.BaseApp.Simulate, app.interfaceRegistry)
}

// RegisterTendermintService registers the CometBFT query service.
func (app *HashgramApp) RegisterTendermintService(clientCtx client.Context) {
	cmtApp := server.NewCometABCIWrapper(app)
	cmtservice.RegisterTendermintService(clientCtx, app.BaseApp.GRPCQueryRouter(), app.interfaceRegistry, cmtApp.Query)
}

// RegisterNodeService registers the node service.
func (app *HashgramApp) RegisterNodeService(clientCtx client.Context, cfg config.Config) {
	nodeservice.RegisterNodeService(clientCtx, app.GRPCQueryRouter(), cfg)
}

// GetMaccPerms returns a copy of the module account permission table.
func GetMaccPerms() map[string][]string { return maps.Clone(maccPerms) }

// ModuleAccountAddrs returns every module account address.
func ModuleAccountAddrs() map[string]bool {
	out := make(map[string]bool)
	for acc := range GetMaccPerms() {
		out[authtypes.NewModuleAddress(acc).String()] = true
	}
	return out
}

// BlockedAddresses returns addresses that may not receive funds via x/bank.
//
// Module accounts are blocked so that users cannot accidentally send HASH into
// a module's escrow and lose it. The gov module account is unblocked because
// governance proposals legitimately fund it.
func BlockedAddresses() map[string]bool {
	blocked := ModuleAccountAddrs()
	delete(blocked, authtypes.NewModuleAddress(govtypes.ModuleName).String())
	return blocked
}

// MintAuthorityCount reports how many module accounts hold the Minter
// permission. On Hashgram this must always be zero; a test asserts it.
func MintAuthorityCount() int {
	n := 0
	for _, perms := range maccPerms {
		for _, p := range perms {
			if p == authtypes.Minter {
				n++
			}
		}
	}
	return n
}
