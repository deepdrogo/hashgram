package keeper_test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/log"
	storetypes "cosmossdk.io/store/types"

	"github.com/cosmos/cosmos-sdk/codec"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	"github.com/cosmos/cosmos-sdk/testutil"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/feerouter/keeper"
	"github.com/hashgram/hashgram/x/feerouter/types"
	founderkeeper "github.com/hashgram/hashgram/x/founder/keeper"
	hgtestutil "github.com/hashgram/hashgram/x/founder/keeper/testutil"
	foundertypes "github.com/hashgram/hashgram/x/founder/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

func testAddr(label string) sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b)
}

var (
	beneficiaryAddr = testAddr("DEVNET-beneficiary")
	aliceAddr       = testAddr("DEVNET-alice")
	bobAddr         = testAddr("DEVNET-bob")
)

type fixture struct {
	ctx      sdk.Context
	router   keeper.Keeper
	founder  founderkeeper.Keeper
	bank     *hgtestutil.Bank
	gov      string
	feePool  sdk.AccAddress
	pool     sdk.AccAddress
	founderA sdk.AccAddress
}

// setup builds a feerouter keeper wired to a real x/founder keeper over an
// in-memory store and an in-memory bank.
//
// The founder keeper is the real implementation, not a stub: the point of
// these tests is that the two modules together produce a split that balances,
// and a stubbed founder keeper would test only the plumbing.
func setup(t *testing.T, withBeneficiary bool) *fixture {
	t.Helper()

	routerKey := storetypes.NewKVStoreKey(types.StoreKey)
	founderKey := storetypes.NewKVStoreKey(foundertypes.StoreKey)
	testCtx := testutil.DefaultContextWithDB(t, routerKey, storetypes.NewTransientStoreKey("transient_test"))
	ctx := testCtx.Ctx.WithBlockHeight(1)

	// Mount the founder store into the same multistore.
	cms := testCtx.CMS
	cms.MountStoreWithDB(founderKey, storetypes.StoreTypeIAVL, testCtx.DB)
	require.NoError(t, cms.LoadLatestVersion())
	ctx = ctx.WithMultiStore(cms)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	bank := hgtestutil.NewBank()
	accounts := hgtestutil.NewAccounts(
		types.ModuleName,
		foundertypes.ModuleName,
		authtypes.FeeCollectorName,
	)
	gov := authtypes.NewModuleAddress("gov").String()

	fk := founderkeeper.NewKeeper(cdc, runtime.NewKVStoreService(founderKey), accounts, bank, gov, log.NewNopLogger())

	founderGenesis := foundertypes.DefaultGenesis()
	if withBeneficiary {
		founderGenesis = foundertypes.NewGenesisState(beneficiaryAddr.String())
	}
	require.NoError(t, fk.InitGenesis(ctx, *founderGenesis))

	rk := keeper.NewKeeper(
		cdc, runtime.NewKVStoreService(routerKey),
		accounts, bank, fk,
		authtypes.FeeCollectorName, gov, log.NewNopLogger(),
	)
	require.NoError(t, rk.InitGenesis(ctx, *types.DefaultGenesis()))

	return &fixture{
		ctx:      ctx,
		router:   rk,
		founder:  fk,
		bank:     bank,
		gov:      gov,
		feePool:  hgtestutil.ModuleAddress(authtypes.FeeCollectorName),
		pool:     hgtestutil.ModuleAddress(types.ModuleName),
		founderA: hgtestutil.ModuleAddress(foundertypes.ModuleName),
	}
}

func uhash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewInt64Coin(hgparams.BaseCoinDenom, n))
}

func hash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(n)))
}

// ---------------------------------------------------------------------------
// The headline property: transfers are not taxed
// ---------------------------------------------------------------------------

// TestUserTransferIsNotTaxed is the specification's central promise (§15,
// §94, §116.3): if Alice sends Bob 100 HASH, Bob receives 100 HASH and the
// Founder receives nothing from it.
//
// The Founder share applies to protocol fee revenue, and a peer-to-peer
// transfer is not protocol fee revenue. This test drives a full block cycle
// around the transfer so that no BeginBlocker gets a chance to touch it.
func TestUserTransferIsNotTaxed(t *testing.T) {
	f := setup(t, true)

	f.bank.Fund(aliceAddr, hash(1_000))
	supplyBefore := f.bank.TotalSupply()

	// Alice sends Bob exactly 100 HASH.
	require.NoError(t, f.bank.SendCoins(f.ctx, aliceAddr, bobAddr, hash(100)))

	// Run the router as it would run at the start of the next block.
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	require.Equal(t, "100000000uhash", f.bank.GetAllBalances(f.ctx, bobAddr).String(),
		"Bob must receive the full 100 HASH; the Founder share is not a transfer tax")
	require.Equal(t, "900000000uhash", f.bank.GetAllBalances(f.ctx, aliceAddr).String(),
		"Alice must be debited exactly 100 HASH and nothing more")

	require.True(t, f.founder.Pending(f.ctx).IsZero(),
		"the Founder accrued from a peer-to-peer transfer")

	ledger, err := f.founder.GetLedger(f.ctx)
	require.NoError(t, err)
	require.True(t, ledger.TotalAccrued.IsZero(),
		"the founder ledger recorded accrual from a transfer")

	totals, err := f.router.GetTotals(f.ctx)
	require.NoError(t, err)
	require.True(t, totals.TotalQualifying.IsZero(),
		"a peer-to-peer transfer was counted as qualifying protocol revenue")

	require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String())
}

// ---------------------------------------------------------------------------
// The other headline property: 1% of qualifying revenue, exactly
// ---------------------------------------------------------------------------

// TestOneHundredHashOfGasYieldsExactlyOneHash is §94: when the protocol earns
// 100 HASH of qualifying fee revenue, the Founder receives exactly 1 HASH and
// validators receive exactly 99 HASH.
func TestOneHundredHashOfGasYieldsExactlyOneHash(t *testing.T) {
	f := setup(t, true)

	// 100 HASH of gas fees have accumulated in the fee collector.
	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	supplyBefore := f.bank.TotalSupply()

	require.NoError(t, f.router.BeginBlocker(f.ctx))

	require.Equal(t, "1000000uhash", f.founder.Pending(f.ctx).String(),
		"the Founder share of 100 HASH of qualifying revenue must be exactly 1 HASH")
	require.Equal(t, "99000000uhash", f.bank.GetAllBalances(f.ctx, f.feePool).String(),
		"validators and delegators must be left exactly 99 HASH")

	totals, err := f.router.GetTotals(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "100000000uhash", totals.TotalQualifying.String())
	require.Equal(t, "1000000uhash", totals.FounderShare.String())
	require.Equal(t, "99000000uhash", totals.ValidatorShare.String())
	require.NoError(t, totals.Validate(), "the recorded split does not balance")

	require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String(),
		"routing changed total supply")
}

// TestOneHundredHashOfServiceFeesYieldsExactlyOneHash is the same property for
// explicit service fees rather than gas.
func TestOneHundredHashOfServiceFeesYieldsExactlyOneHash(t *testing.T) {
	f := setup(t, true)

	f.bank.Fund(aliceAddr, hash(500))
	supplyBefore := f.bank.TotalSupply()

	require.NoError(t, f.router.CollectServiceFee(f.ctx, aliceAddr, hash(100), types.SERVICE_KIND_USERNAME))

	// Before the split the fee sits in the revenue pool.
	require.Equal(t, "100000000uhash", f.router.PendingPool(f.ctx).String())
	require.Equal(t, "400000000uhash", f.bank.GetAllBalances(f.ctx, aliceAddr).String())

	require.NoError(t, f.router.BeginBlocker(f.ctx))

	require.Equal(t, "1000000uhash", f.founder.Pending(f.ctx).String())
	require.Equal(t, "99000000uhash", f.bank.GetAllBalances(f.ctx, f.feePool).String())
	require.True(t, f.router.PendingPool(f.ctx).IsZero(), "the revenue pool was not fully swept")

	require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String())
}

// ---------------------------------------------------------------------------
// Double taxation
// ---------------------------------------------------------------------------

// TestServiceFeeRemainderIsNotTaxedTwice is the reason the BeginBlocker runs
// gas before service fees and defers the service split by one block.
//
// A service fee's validator remainder is swept into the fee collector during
// the split. If the next block's gas pass measured the fee collector without
// x/distribution having emptied it, that remainder would be taxed a second
// time. Here we model the real block cycle: distribution empties the fee
// collector after the router runs.
func TestServiceFeeRemainderIsNotTaxedTwice(t *testing.T) {
	f := setup(t, true)
	f.bank.Fund(aliceAddr, hash(1_000))

	// Block 1: a 100 HASH service fee is collected. No gas.
	require.NoError(t, f.router.CollectServiceFee(f.ctx, aliceAddr, hash(100), types.SERVICE_KIND_USERNAME))

	// Block 2 BeginBlock: the router splits, then distribution empties the
	// fee collector.
	ctx := f.ctx.WithBlockHeight(2)
	require.NoError(t, f.router.BeginBlocker(ctx))
	require.Equal(t, "1000000uhash", f.founder.Pending(ctx).String())
	distributionSweep(t, f, ctx)

	// Block 3 BeginBlock: nothing new arrived, so nothing more may be taken.
	ctx = f.ctx.WithBlockHeight(3)
	require.NoError(t, f.router.BeginBlocker(ctx))

	require.Equal(t, "1000000uhash", f.founder.Pending(ctx).String(),
		"the service-fee remainder was taxed a second time")

	totals, err := f.router.GetTotals(ctx)
	require.NoError(t, err)
	require.Equal(t, "100000000uhash", totals.TotalQualifying.String(),
		"the same revenue was counted as qualifying twice")
	require.Equal(t, "1000000uhash", totals.FounderShare.String())
}

// distributionSweep models x/distribution's BeginBlocker, which pays the
// entire fee collector balance to validators and delegators, leaving it empty.
func distributionSweep(t *testing.T, f *fixture, ctx sdk.Context) {
	t.Helper()
	balance := f.bank.GetAllBalances(ctx, f.feePool)
	if balance.IsZero() {
		return
	}
	require.NoError(t, f.bank.SendCoinsFromModuleToModule(ctx, authtypes.FeeCollectorName, "distribution", balance))
}

// TestMultipleBlocksAccumulateCorrectly runs several block cycles with both
// gas and service fees and checks the cumulative books.
func TestMultipleBlocksAccumulateCorrectly(t *testing.T) {
	f := setup(t, true)
	f.bank.Fund(aliceAddr, hash(10_000))
	supplyBefore := f.bank.TotalSupply()

	const blocks = 5
	for i := 1; i <= blocks; i++ {
		ctx := f.ctx.WithBlockHeight(int64(i))

		// Gas from the previous block, and a service fee collected in it.
		f.bank.FundModule(authtypes.FeeCollectorName, hash(20))
		require.NoError(t, f.router.CollectServiceFee(ctx, aliceAddr, hash(30), types.SERVICE_KIND_IDENTITY))

		ctx = f.ctx.WithBlockHeight(int64(i + 1))
		require.NoError(t, f.router.BeginBlocker(ctx))
		distributionSweep(t, f, ctx)
	}

	// 5 blocks x (20 gas + 30 service) = 250 HASH qualifying, 2.5 HASH founder.
	totals, err := f.router.GetTotals(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "250000000uhash", totals.TotalQualifying.String())
	require.Equal(t, "2500000uhash", totals.FounderShare.String())
	require.Equal(t, "247500000uhash", totals.ValidatorShare.String())
	require.NoError(t, totals.Validate())

	require.Equal(t, "2500000uhash", f.founder.Pending(f.ctx).String())

	// The loop injected gas fees directly into the fee collector, which the
	// fake bank treats as new supply (there is no minting module to model).
	// Routing itself must add nothing beyond that.
	wantSupply := supplyBefore.Add(hash(20 * blocks)...)
	require.Equal(t, wantSupply.String(), f.bank.TotalSupply().String(),
		"routing created or destroyed coins")
}

// ---------------------------------------------------------------------------
// Accounting identity
// ---------------------------------------------------------------------------

// TestSplitAlwaysBalances hammers the identity
// total_qualifying == founder + validator + treasury
// across amounts chosen to exercise truncation.
func TestSplitAlwaysBalances(t *testing.T) {
	amounts := []int64{
		1, 2, 3, 7, 99, 100, 101, 199, 200, 999,
		1_000, 9_999, 10_000, 123_456, 1_000_000, 999_999_999,
	}

	for _, amt := range amounts {
		f := setup(t, true)
		f.bank.FundModule(authtypes.FeeCollectorName, uhash(amt))
		supplyBefore := f.bank.TotalSupply()

		require.NoError(t, f.router.BeginBlocker(f.ctx))

		totals, err := f.router.GetTotals(f.ctx)
		require.NoError(t, err)
		require.NoError(t, totals.Validate(), "split does not balance for %d uhash", amt)

		require.Equal(t, uhash(amt).String(), totals.TotalQualifying.String())
		require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String(),
			"routing %d uhash changed total supply", amt)
	}
}

// ---------------------------------------------------------------------------
// Unconfigured chain
// ---------------------------------------------------------------------------

// TestWithoutBeneficiaryEverythingGoesToValidators: a chain launched before
// the Founder supplied a public address must keep working, paying all fee
// revenue to validators.
func TestWithoutBeneficiaryEverythingGoesToValidators(t *testing.T) {
	f := setup(t, false)

	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	require.True(t, f.founder.Pending(f.ctx).IsZero())
	require.Equal(t, "100000000uhash", f.bank.GetAllBalances(f.ctx, f.feePool).String(),
		"revenue was diverted despite no configured beneficiary")

	totals, err := f.router.GetTotals(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "100000000uhash", totals.TotalQualifying.String())
	require.True(t, totals.FounderShare.IsZero())
	require.Equal(t, "100000000uhash", totals.ValidatorShare.String())
	require.NoError(t, totals.Validate())
}

// TestDisabledRoutingTakesNothing
func TestDisabledRoutingTakesNothing(t *testing.T) {
	f := setup(t, true)

	next := types.Params{Enabled: false}
	require.NoError(t, f.router.UpdateParams(f.ctx, f.gov, next))

	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	require.True(t, f.founder.Pending(f.ctx).IsZero())
	require.Equal(t, "100000000uhash", f.bank.GetAllBalances(f.ctx, f.feePool).String())
}

// TestBeginBlockerOnEmptyChainIsNoop: the first blocks of a new chain have no
// fees at all and must not error.
func TestBeginBlockerOnEmptyChainIsNoop(t *testing.T) {
	f := setup(t, true)
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	totals, err := f.router.GetTotals(f.ctx)
	require.NoError(t, err)
	require.True(t, totals.TotalQualifying.IsZero())
}

// TestBeginBlockerWithoutParamsIsNoop: an uninitialised module must not stop
// block production.
func TestBeginBlockerWithoutParamsIsNoop(t *testing.T) {
	key := storetypes.NewKVStoreKey(types.StoreKey)
	testCtx := testutil.DefaultContextWithDB(t, key, storetypes.NewTransientStoreKey("transient_test"))
	founderKey := storetypes.NewKVStoreKey(foundertypes.StoreKey)
	cms := testCtx.CMS
	cms.MountStoreWithDB(founderKey, storetypes.StoreTypeIAVL, testCtx.DB)
	require.NoError(t, cms.LoadLatestVersion())
	ctx := testCtx.Ctx.WithMultiStore(cms)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	bank := hgtestutil.NewBank()
	accounts := hgtestutil.NewAccounts(types.ModuleName, foundertypes.ModuleName, authtypes.FeeCollectorName)
	gov := authtypes.NewModuleAddress("gov").String()

	fk := founderkeeper.NewKeeper(cdc, runtime.NewKVStoreService(founderKey), accounts, bank, gov, log.NewNopLogger())
	rk := keeper.NewKeeper(cdc, runtime.NewKVStoreService(key), accounts, bank, fk,
		authtypes.FeeCollectorName, gov, log.NewNopLogger())

	bank.FundModule(authtypes.FeeCollectorName, hash(100))
	require.NoError(t, rk.BeginBlocker(ctx), "an uninitialised router must not fail a block")
}

// ---------------------------------------------------------------------------
// Service fee collection
// ---------------------------------------------------------------------------

func TestCollectServiceFeeRecordsPerServiceRevenue(t *testing.T) {
	f := setup(t, true)
	f.bank.Fund(aliceAddr, hash(1_000))

	require.NoError(t, f.router.CollectServiceFee(f.ctx, aliceAddr, hash(10), types.SERVICE_KIND_USERNAME))
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	all, err := f.router.AllServiceRevenue(f.ctx)
	require.NoError(t, err)
	require.NotEmpty(t, all)

	// Explicit service fees are recorded under SERVICE_KIND_OTHER by the
	// split, because the pool is fungible once several kinds have been paid
	// into it. The per-kind attribution lives in the emitted event at
	// collection time.
	total := sdk.NewCoins()
	for _, sr := range all {
		total = total.Add(sr.Amount...)
	}
	require.Equal(t, "10000000uhash", total.String())
}

func TestCollectServiceFeeRejectsUnspecifiedKind(t *testing.T) {
	f := setup(t, true)
	f.bank.Fund(aliceAddr, hash(10))

	err := f.router.CollectServiceFee(f.ctx, aliceAddr, hash(1), types.SERVICE_KIND_UNSPECIFIED)
	require.Error(t, err)
	require.True(t, types.ErrInvalidServiceKind.Is(err), "got %v", err)
}

func TestCollectServiceFeeFailsWithInsufficientFunds(t *testing.T) {
	f := setup(t, true)
	f.bank.Fund(aliceAddr, hash(1))

	require.Error(t, f.router.CollectServiceFee(f.ctx, aliceAddr, hash(100), types.SERVICE_KIND_USERNAME))
	require.True(t, f.router.PendingPool(f.ctx).IsZero(),
		"a failed fee collection left coins in the pool")
}

func TestCollectZeroServiceFeeIsNoop(t *testing.T) {
	f := setup(t, true)
	require.NoError(t, f.router.CollectServiceFee(f.ctx, aliceAddr, sdk.NewCoins(), types.SERVICE_KIND_USERNAME))
	require.True(t, f.router.PendingPool(f.ctx).IsZero())
}

// ---------------------------------------------------------------------------
// Governance
// ---------------------------------------------------------------------------

func TestUpdateParamsRequiresGovernance(t *testing.T) {
	f := setup(t, true)

	err := f.router.UpdateParams(f.ctx, beneficiaryAddr.String(), types.Params{Enabled: false})
	require.Error(t, err)
	require.True(t, types.ErrInvalidAuthority.Is(err), "got %v", err)

	require.NoError(t, f.router.UpdateParams(f.ctx, f.gov, types.Params{Enabled: false}))
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestGenesisRejectsUnbalancedTotals(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Totals.TotalQualifying = uhash(100)
	gs.Totals.FounderShare = uhash(1)
	gs.Totals.ValidatorShare = uhash(50) // should be 99

	err := gs.Validate()
	require.Error(t, err)
	require.True(t, types.ErrSplitDoesNotBalance.Is(err), "got %v", err)
}

func TestGenesisRejectsDuplicateServiceKinds(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.ServiceRevenue = []types.ServiceRevenue{
		{Kind: types.SERVICE_KIND_GAS, Amount: uhash(1)},
		{Kind: types.SERVICE_KIND_GAS, Amount: uhash(2)},
	}
	require.Error(t, gs.Validate())
}

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setup(t, true)
	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	require.NoError(t, f.router.BeginBlocker(f.ctx))

	out, err := f.router.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.NoError(t, out.Validate())
	require.Equal(t, "100000000uhash", out.Totals.TotalQualifying.String())
}
