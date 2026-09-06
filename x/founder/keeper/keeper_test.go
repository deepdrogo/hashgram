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
	"github.com/hashgram/hashgram/x/founder/keeper"
	hgtestutil "github.com/hashgram/hashgram/x/founder/keeper/testutil"
	"github.com/hashgram/hashgram/x/founder/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	testBeneficiary = testBeneficiaryAddr.String()
	otherAddress = otherAddr.String()
	os.Exit(m.Run())
}

// Test addresses are derived from labels rather than hardcoded bech32
// strings. Hardcoding invites transcription errors that surface as confusing
// checksum failures, and more importantly a literal address in a repository
// looks like it might have a key somewhere. These have no keys at all: they
// are label bytes padded to 20 bytes.
func testAddr(label string) sdk.AccAddress {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b)
}

var (
	testBeneficiaryAddr = testAddr("DEVNET-beneficiary")
	otherAddr           = testAddr("DEVNET-other")
)

// Rendered lazily: bech32 encoding depends on the prefix installed by
// TestMain, so these cannot be package-level initialised constants.
var (
	testBeneficiary string
	otherAddress    string
)

type fixture struct {
	ctx    sdk.Context
	keeper keeper.Keeper
	bank   *hgtestutil.Bank
	gov    string
}

func setup(t *testing.T) *fixture {
	t.Helper()

	key := storetypes.NewKVStoreKey(types.StoreKey)
	testCtx := testutil.DefaultContextWithDB(t, key, storetypes.NewTransientStoreKey("transient_test"))
	ctx := testCtx.Ctx.WithBlockHeight(1)

	cdc := codec.NewProtoCodec(codectypes.NewInterfaceRegistry())
	bank := hgtestutil.NewBank()
	accounts := hgtestutil.NewAccounts(types.ModuleName, authtypes.FeeCollectorName)
	gov := authtypes.NewModuleAddress("gov").String()

	k := keeper.NewKeeper(cdc, runtime.NewKVStoreService(key), accounts, bank, gov, log.NewNopLogger())

	return &fixture{ctx: ctx, keeper: k, bank: bank, gov: gov}
}

func uhash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewInt64Coin(hgparams.BaseCoinDenom, n))
}

func hash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(n)))
}

func configured(t *testing.T, f *fixture) {
	t.Helper()
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.NewGenesisState(testBeneficiary)))
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestInitGenesisSetsBeneficiaryAndFee(t *testing.T) {
	f := setup(t)
	configured(t, f)

	p, err := f.keeper.GetParams(f.ctx)
	require.NoError(t, err)
	require.Equal(t, testBeneficiary, p.Beneficiary)
	require.Equal(t, uint32(100), p.FeeBasisPoints)
	require.Equal(t, "1.00%", p.FeePercent())
	require.Equal(t, uint32(100), f.keeper.MaxFeeBasisPoints())

	addr, err := f.keeper.Beneficiary(f.ctx)
	require.NoError(t, err)
	require.Equal(t, testBeneficiary, addr.String())
}

// TestInitGenesisSeedsBeneficiaryHistory: the audit trail must start at
// launch, not at the first change, so an observer can see who was originally
// configured without replaying the chain.
func TestInitGenesisSeedsBeneficiaryHistory(t *testing.T) {
	f := setup(t)
	configured(t, f)

	history, err := f.keeper.BeneficiaryHistory(f.ctx)
	require.NoError(t, err)
	require.Len(t, history, 1)
	require.Equal(t, "", history[0].PreviousBeneficiary)
	require.Equal(t, testBeneficiary, history[0].NewBeneficiary)
}

func TestInitGenesisWithoutBeneficiary(t *testing.T) {
	f := setup(t)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.DefaultGenesis()))

	_, err := f.keeper.Beneficiary(f.ctx)
	require.Error(t, err)
	require.True(t, types.ErrBeneficiaryNotSet.Is(err), "got %v", err)

	history, err := f.keeper.BeneficiaryHistory(f.ctx)
	require.NoError(t, err)
	require.Empty(t, history, "no beneficiary means no history entry")
}

func TestExportGenesisRoundTrips(t *testing.T) {
	f := setup(t)
	configured(t, f)

	out, err := f.keeper.ExportGenesis(f.ctx)
	require.NoError(t, err)
	require.Equal(t, testBeneficiary, out.Params.Beneficiary)
	require.Len(t, out.BeneficiaryHistory, 1)
	require.NoError(t, out.Validate())
}

// ---------------------------------------------------------------------------
// FounderCut
// ---------------------------------------------------------------------------

// TestFounderCutIsExactlyOnePercent is the §94 arithmetic at the keeper layer:
// 100 HASH of qualifying protocol revenue yields exactly 1 HASH.
func TestFounderCutIsExactlyOnePercent(t *testing.T) {
	f := setup(t)
	configured(t, f)

	cut, err := f.keeper.FounderCut(f.ctx, hash(100))
	require.NoError(t, err)
	require.Equal(t, "1000000uhash", cut.String())
}

// TestFounderCutIsZeroWithoutBeneficiary: an unconfigured chain must route
// everything to validators rather than accrue into an account nobody can
// withdraw from.
func TestFounderCutIsZeroWithoutBeneficiary(t *testing.T) {
	f := setup(t)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.DefaultGenesis()))

	cut, err := f.keeper.FounderCut(f.ctx, hash(1_000))
	require.NoError(t, err)
	require.True(t, cut.IsZero(), "an unconfigured founder module took a cut")
}

// TestFounderCutIsZeroWithoutParams: reading a completely uninitialised
// module must not error inside BeginBlock, because a fee-routing failure must
// never become a block-production failure.
func TestFounderCutIsZeroWithoutParams(t *testing.T) {
	f := setup(t)

	cut, err := f.keeper.FounderCut(f.ctx, hash(1_000))
	require.NoError(t, err, "an uninitialised module must not error in BeginBlock")
	require.True(t, cut.IsZero())
	require.Equal(t, uint32(0), f.keeper.FeeBasisPoints(f.ctx))
}

// ---------------------------------------------------------------------------
// Accrual
// ---------------------------------------------------------------------------

// TestAccrueMovesCoinsAndRecordsLedger checks that accrual moves real coins
// and that total supply is unchanged: x/founder must never create HASH.
func TestAccrueMovesCoinsAndRecordsLedger(t *testing.T) {
	f := setup(t)
	configured(t, f)

	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	supplyBefore := f.bank.TotalSupply()

	require.NoError(t, f.keeper.Accrue(f.ctx, authtypes.FeeCollectorName, hash(1), "gas"))

	require.Equal(t, "1000000uhash", f.keeper.Pending(f.ctx).String())
	require.Equal(t, "99000000uhash",
		f.bank.GetAllBalances(f.ctx, hgtestutil.ModuleAddress(authtypes.FeeCollectorName)).String())

	ledger, err := f.keeper.GetLedger(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "1000000uhash", ledger.TotalAccrued.String())
	require.True(t, ledger.TotalPaid.IsZero())

	require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String(),
		"accrual changed total supply; x/founder must only move existing coins")
}

// TestAccrueSkipsWithoutBeneficiary: coins must stay where they are rather
// than being stranded in a module account with no payout destination.
func TestAccrueSkipsWithoutBeneficiary(t *testing.T) {
	f := setup(t)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.DefaultGenesis()))

	f.bank.FundModule(authtypes.FeeCollectorName, hash(100))
	require.NoError(t, f.keeper.Accrue(f.ctx, authtypes.FeeCollectorName, hash(1), "gas"))

	require.True(t, f.keeper.Pending(f.ctx).IsZero(), "accrued despite no beneficiary")
	require.Equal(t, "100000000uhash",
		f.bank.GetAllBalances(f.ctx, hgtestutil.ModuleAddress(authtypes.FeeCollectorName)).String(),
		"coins left the fee collector with nowhere to go")
}

func TestAccrueRejectsInvalidAmount(t *testing.T) {
	f := setup(t)
	configured(t, f)

	// A negative coin cannot be built through sdk.NewCoins, so construct the
	// slice directly to exercise the guard.
	bad := sdk.Coins{sdk.Coin{Denom: hgparams.BaseCoinDenom, Amount: hgparams.HashToBase(1).Neg()}}
	require.Error(t, f.keeper.Accrue(f.ctx, authtypes.FeeCollectorName, bad, "gas"))
}

func TestAccrueZeroIsNoop(t *testing.T) {
	f := setup(t)
	configured(t, f)
	require.NoError(t, f.keeper.Accrue(f.ctx, authtypes.FeeCollectorName, sdk.NewCoins(), "gas"))
	require.True(t, f.keeper.Pending(f.ctx).IsZero())
}

// ---------------------------------------------------------------------------
// Payout
// ---------------------------------------------------------------------------

func TestPayoutSendsPendingToBeneficiary(t *testing.T) {
	f := setup(t)
	configured(t, f)

	f.bank.FundModule(types.ModuleName, hash(7))
	supplyBefore := f.bank.TotalSupply()

	paid, err := f.keeper.Payout(f.ctx, types.TriggerClaim)
	require.NoError(t, err)
	require.Equal(t, "7000000uhash", paid.String())

	beneficiary := sdk.MustAccAddressFromBech32(testBeneficiary)
	require.Equal(t, "7000000uhash", f.bank.GetAllBalances(f.ctx, beneficiary).String())
	require.True(t, f.keeper.Pending(f.ctx).IsZero())

	ledger, err := f.keeper.GetLedger(f.ctx)
	require.NoError(t, err)
	require.Equal(t, "7000000uhash", ledger.TotalPaid.String())
	require.Equal(t, int64(1), ledger.LastPayoutHeight)
	require.Equal(t, testBeneficiary, ledger.LastPayoutBeneficiary)

	require.Equal(t, supplyBefore.String(), f.bank.TotalSupply().String())
}

func TestPayoutWithNothingPendingIsEmpty(t *testing.T) {
	f := setup(t)
	configured(t, f)

	paid, err := f.keeper.Payout(f.ctx, types.TriggerClaim)
	require.NoError(t, err)
	require.True(t, paid.IsZero())
}

func TestPayoutRequiresBeneficiary(t *testing.T) {
	f := setup(t)
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *types.DefaultGenesis()))

	_, err := f.keeper.Payout(f.ctx, types.TriggerClaim)
	require.Error(t, err)
	require.True(t, types.ErrBeneficiaryNotSet.Is(err), "got %v", err)
}

// TestPayoutRefusesBlockedBeneficiary catches the configuration mistake of
// pointing the beneficiary at a module account, which x/bank would refuse to
// credit, turning every payout into a recurring block-level error.
func TestPayoutRefusesBlockedBeneficiary(t *testing.T) {
	f := setup(t)
	configured(t, f)

	f.bank.Block(sdk.MustAccAddressFromBech32(testBeneficiary))
	f.bank.FundModule(types.ModuleName, hash(1))

	_, err := f.keeper.Payout(f.ctx, types.TriggerClaim)
	require.Error(t, err)
	require.Contains(t, err.Error(), "blocked")
}

// ---------------------------------------------------------------------------
// Automatic payout
// ---------------------------------------------------------------------------

func TestEndBlockerPaysOnPeriodBoundary(t *testing.T) {
	f := setup(t)
	configured(t, f)
	f.bank.FundModule(types.ModuleName, hash(10))

	// Not on a period boundary: nothing happens.
	ctx := f.ctx.WithBlockHeight(int64(types.DefaultPayoutPeriodBlocks) - 1)
	require.NoError(t, f.keeper.EndBlocker(ctx))
	require.Equal(t, "10000000uhash", f.keeper.Pending(ctx).String())

	// On the boundary: paid.
	ctx = f.ctx.WithBlockHeight(int64(types.DefaultPayoutPeriodBlocks))
	require.NoError(t, f.keeper.EndBlocker(ctx))
	require.True(t, f.keeper.Pending(ctx).IsZero())

	beneficiary := sdk.MustAccAddressFromBech32(testBeneficiary)
	require.Equal(t, "10000000uhash", f.bank.GetAllBalances(ctx, beneficiary).String())
}

// TestEndBlockerRespectsMinPayout: batching avoids burying a cold-wallet
// beneficiary under dust transfers.
func TestEndBlockerRespectsMinPayout(t *testing.T) {
	f := setup(t)
	configured(t, f)

	// Default min payout is 1 HASH; fund half of it.
	f.bank.FundModule(types.ModuleName, uhash(500_000))

	ctx := f.ctx.WithBlockHeight(int64(types.DefaultPayoutPeriodBlocks))
	require.NoError(t, f.keeper.EndBlocker(ctx))
	require.Equal(t, "500000uhash", f.keeper.Pending(ctx).String(),
		"paid out below the minimum threshold")
}

// TestEndBlockerNeverFailsABlock: a payout problem must not stop block
// production. Here the beneficiary is blocked, so payout cannot succeed.
func TestEndBlockerNeverFailsABlock(t *testing.T) {
	f := setup(t)
	configured(t, f)

	f.bank.Block(sdk.MustAccAddressFromBech32(testBeneficiary))
	f.bank.FundModule(types.ModuleName, hash(10))

	ctx := f.ctx.WithBlockHeight(int64(types.DefaultPayoutPeriodBlocks))
	require.NoError(t, f.keeper.EndBlocker(ctx),
		"a failed founder payout must not fail the block")
	require.Equal(t, "10000000uhash", f.keeper.Pending(ctx).String(),
		"revenue must remain pending and recoverable")
}

func TestEndBlockerDisabledByZeroPeriod(t *testing.T) {
	f := setup(t)
	gs := types.NewGenesisState(testBeneficiary)
	gs.Params.PayoutPeriodBlocks = 0
	require.NoError(t, f.keeper.InitGenesis(f.ctx, *gs))

	f.bank.FundModule(types.ModuleName, hash(10))
	require.NoError(t, f.keeper.EndBlocker(f.ctx.WithBlockHeight(7200)))
	require.Equal(t, "10000000uhash", f.keeper.Pending(f.ctx).String())
}

// ---------------------------------------------------------------------------
// Governance
// ---------------------------------------------------------------------------

// TestUpdateParamsRequiresGovernance is §6 and §116.7 at the module level:
// there is no key other than governance that can reconfigure this module, and
// in particular the beneficiary cannot reconfigure itself.
func TestUpdateParamsRequiresGovernance(t *testing.T) {
	f := setup(t)
	configured(t, f)

	next := types.DefaultParams()
	next.Beneficiary = otherAddress

	for _, impostor := range []string{
		testBeneficiary, // the Founder's own beneficiary
		otherAddress,
		authtypes.NewModuleAddress("staking").String(),
	} {
		err := f.keeper.UpdateParams(f.ctx, impostor, next)
		require.Error(t, err, "%s was allowed to change founder params", impostor)
		require.True(t, types.ErrInvalidAuthority.Is(err), "got %v", err)
	}

	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, next))
}

// TestUpdateParamsCannotRaiseFeeAboveCeiling: even governance cannot exceed
// the compile-time ceiling.
func TestUpdateParamsCannotRaiseFeeAboveCeiling(t *testing.T) {
	f := setup(t)
	configured(t, f)

	next := types.DefaultParams()
	next.Beneficiary = testBeneficiary
	next.FeeBasisPoints = 500 // 5%

	err := f.keeper.UpdateParams(f.ctx, f.gov, next)
	require.Error(t, err)
	require.True(t, types.ErrFeeExceedsCeiling.Is(err), "got %v", err)

	// The stored value is untouched.
	p, err := f.keeper.GetParams(f.ctx)
	require.NoError(t, err)
	require.Equal(t, uint32(100), p.FeeBasisPoints)
}

// TestBeneficiaryChangeSettlesWithOutgoingBeneficiary: revenue that accrued
// under the old configuration belongs to the old address. Carrying it across
// a governance change would silently reassign it.
func TestBeneficiaryChangeSettlesWithOutgoingBeneficiary(t *testing.T) {
	f := setup(t)
	configured(t, f)

	f.bank.FundModule(types.ModuleName, hash(5))

	next := types.DefaultParams()
	next.Beneficiary = otherAddress
	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, next))

	old := sdk.MustAccAddressFromBech32(testBeneficiary)
	new := sdk.MustAccAddressFromBech32(otherAddress)

	require.Equal(t, "5000000uhash", f.bank.GetAllBalances(f.ctx, old).String(),
		"pre-change revenue was not settled with the outgoing beneficiary")
	require.True(t, f.bank.GetAllBalances(f.ctx, new).IsZero())
	require.True(t, f.keeper.Pending(f.ctx).IsZero())
}

// TestBeneficiaryChangeIsRecordedInHistory: a silent swap must not be
// possible.
func TestBeneficiaryChangeIsRecordedInHistory(t *testing.T) {
	f := setup(t)
	configured(t, f)

	next := types.DefaultParams()
	next.Beneficiary = otherAddress
	require.NoError(t, f.keeper.UpdateParams(f.ctx.WithBlockHeight(500), f.gov, next))

	history, err := f.keeper.BeneficiaryHistory(f.ctx)
	require.NoError(t, err)
	require.Len(t, history, 2)

	require.Equal(t, "", history[0].PreviousBeneficiary)
	require.Equal(t, testBeneficiary, history[0].NewBeneficiary)

	require.Equal(t, testBeneficiary, history[1].PreviousBeneficiary)
	require.Equal(t, otherAddress, history[1].NewBeneficiary)
	require.Equal(t, int64(500), history[1].Height)
}

// TestFeeOnlyUpdateDoesNotTouchHistory keeps the audit trail meaningful: it
// records beneficiary changes, not every parameter tweak.
func TestFeeOnlyUpdateDoesNotTouchHistory(t *testing.T) {
	f := setup(t)
	configured(t, f)

	next := types.DefaultParams()
	next.Beneficiary = testBeneficiary
	next.FeeBasisPoints = 50
	require.NoError(t, f.keeper.UpdateParams(f.ctx, f.gov, next))

	history, err := f.keeper.BeneficiaryHistory(f.ctx)
	require.NoError(t, err)
	require.Len(t, history, 1)
}
