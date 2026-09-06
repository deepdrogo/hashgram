package types_test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/serviceproof/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

func uhash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewInt64Coin(hgparams.BaseCoinDenom, n))
}

func hash(n int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(n)))
}

// TestInitialReserveIsFiveHundredMillion pins the reserve size from §31.
func TestInitialReserveIsFiveHundredMillion(t *testing.T) {
	require.Equal(t, "500000000000000uhash", types.InitialReserve().String())
	require.Equal(t, int64(500_000_000), hgparams.AllocServiceReserveHash)

	// Exactly half the canonical supply.
	half := hgparams.MaxSupplyBase().QuoRaw(2)
	require.Equal(t, half.String(),
		types.InitialReserve().AmountOf(hgparams.BaseCoinDenom).String())
}

// TestEmissionDeclines checks that budgets fall monotonically, which is what
// "deterministic declining reward emission" means.
func TestEmissionDeclines(t *testing.T) {
	p := types.DefaultParams()
	projections := types.ProjectEmission(types.InitialReserve(), p, 200, 0)
	require.NotEmpty(t, projections)

	prev := math.NewInt(0)
	first := true
	for i, proj := range projections {
		amt := proj.Budget.AmountOf(hgparams.BaseCoinDenom)
		if first {
			prev = amt
			first = false
			continue
		}
		require.True(t, amt.LTE(prev),
			"epoch %d budget %s is larger than epoch %d's %s", proj.Epoch, amt, i-1, prev)
		prev = amt
	}
}

// TestEmissionNeverExceedsTheReserve is the finite-reserve guarantee from
// §31 and §116.6: however long the network runs, the schedule cannot ask for
// more than the reserve ever held.
func TestEmissionNeverExceedsTheReserve(t *testing.T) {
	p := types.DefaultParams()
	initial := types.InitialReserve()

	// 4000 epochs is roughly eleven years of daily epochs.
	projections := types.ProjectEmission(initial, p, 4_000, 0)

	total := sdk.NewCoins()
	for _, proj := range projections {
		total = total.Add(proj.Budget...)

		require.True(t, initial.IsAllGTE(total),
			"cumulative emission %s exceeded the initial reserve %s by epoch %d",
			total, initial, proj.Epoch)

		// The reserve after each epoch must equal initial minus cumulative.
		wantAfter, negative := initial.SafeSub(total...)
		require.False(t, negative)
		require.Equal(t, wantAfter.String(), proj.ReserveAfter.String(),
			"reserve_after drifted from initial minus cumulative at epoch %d", proj.Epoch)
	}

	// And it should be a substantial fraction, so the schedule is not
	// vacuously safe by paying nothing.
	require.True(t, total.AmountOf(hgparams.BaseCoinDenom).GT(
		initial.AmountOf(hgparams.BaseCoinDenom).QuoRaw(2)),
		"after 4000 epochs the schedule had emitted %s of %s, which is implausibly little",
		total, initial)
}

// TestAbsoluteCapEqualsTheFirstGeometricBudget.
//
// The cap is set to exactly the schedule's own first-epoch budget, so it
// binds at genesis and never afterwards: it is a backstop against a future
// parameter mistake, not a shape that overrides the schedule.
//
// The earlier draft used a much lower cap, which flattened the first two
// decades into a straight line. TestEmissionNeverExceedsTheReserve caught it.
func TestAbsoluteCapEqualsTheFirstGeometricBudget(t *testing.T) {
	p := types.DefaultParams()

	// 0.05% of 500,000,000 HASH is 250,000 HASH.
	uncapped := p
	uncapped.MaxEpochEmission = sdk.NewCoins()
	geometric := types.EmissionForEpoch(types.InitialReserve(), uncapped)
	require.Equal(t, hash(250_000).String(), geometric.String())

	capped := types.EmissionForEpoch(types.InitialReserve(), p)
	require.Equal(t, geometric.String(), capped.String(),
		"the cap alters the first epoch's budget; it should equal it exactly")

	// One epoch later the reserve has shrunk, so the proportion is below the
	// cap and the cap no longer participates.
	after, _ := types.InitialReserve().SafeSub(geometric...)
	require.True(t, types.EmissionForEpoch(after, p).AmountOf(hgparams.BaseCoinDenom).
		LT(geometric.AmountOf(hgparams.BaseCoinDenom)),
		"the budget did not decline after the first epoch")
}

// TestAbsoluteCapStillBindsIfTheRateIsRaised: the cap must actually do
// something if governance raises the emission rate.
func TestAbsoluteCapStillBindsIfTheRateIsRaised(t *testing.T) {
	p := types.DefaultParams()
	p.EmissionRateBps = 100 // 1% per epoch

	budget := types.EmissionForEpoch(types.InitialReserve(), p)
	require.Equal(t, hash(250_000).String(), budget.String(),
		"the cap did not bind after the rate was raised twentyfold")
}

// TestEmissionOfEmptyReserveIsZero
func TestEmissionOfEmptyReserveIsZero(t *testing.T) {
	p := types.DefaultParams()
	require.True(t, types.EmissionForEpoch(sdk.NewCoins(), p).IsZero())
}

// TestEmissionWithZeroRateIsZero: governance can turn subsidy off.
func TestEmissionWithZeroRateIsZero(t *testing.T) {
	p := types.DefaultParams()
	p.EmissionRateBps = 0
	require.True(t, types.EmissionForEpoch(types.InitialReserve(), p).IsZero())
}

// TestEmissionTruncatesDown: a tiny reserve yields nothing rather than
// rounding up into coins that do not exist.
func TestEmissionTruncatesDown(t *testing.T) {
	p := types.DefaultParams()
	// 0.05% of 100 uhash is 0.05 uhash, which truncates to zero.
	require.True(t, types.EmissionForEpoch(uhash(100), p).IsZero())
	// 0.05% of 20000 uhash is 10 uhash exactly.
	require.Equal(t, "10uhash", types.EmissionForEpoch(uhash(20_000), p).String())
}

// ---------------------------------------------------------------------------
// Provider cap and pro rata
// ---------------------------------------------------------------------------

// TestProviderRewardCapBoundsOneOperator is §30-adjacent: the subsidy must
// not be able to fund centralisation.
func TestProviderRewardCapBoundsOneOperator(t *testing.T) {
	p := types.DefaultParams()
	budget := hash(1_000)

	// 5% of 1000 HASH is 50 HASH.
	require.Equal(t, hash(50).String(), types.ProviderRewardCap(budget, p).String())

	// A network needs at least twenty providers before the cap stops binding.
	require.Equal(t, uint32(500), p.MaxProviderShareBps)
	require.Equal(t, 20, int(types.BasisPointsMax/p.MaxProviderShareBps))
}

func TestProviderRewardCapDisabledReturnsFullBudget(t *testing.T) {
	p := types.DefaultParams()
	p.MaxProviderShareBps = types.BasisPointsMax
	budget := hash(1_000)
	require.Equal(t, budget.String(), types.ProviderRewardCap(budget, p).String())
}

// TestProRataSumsToAtMostTheBudget: truncation must never distribute more
// than was budgeted. The dust stays in the reserve.
func TestProRataSumsToAtMostTheBudget(t *testing.T) {
	budget := uhash(1_000_000)

	for _, credits := range [][]int64{
		{1, 1, 1},
		{1, 2, 3, 4, 5, 6, 7},
		{333, 333, 334},
		{7, 11, 13, 17, 19, 23, 29, 31},
		{1, 999_999},
		{1_000_000_000, 1, 1},
	} {
		total := math.ZeroInt()
		for _, c := range credits {
			total = total.Add(math.NewInt(c))
		}

		sum := sdk.NewCoins()
		for _, c := range credits {
			sum = sum.Add(types.ProRata(budget, math.NewInt(c), total)...)
		}

		require.True(t, budget.IsAllGTE(sum),
			"pro rata over %v distributed %s from a budget of %s", credits, sum, budget)
	}
}

func TestProRataOfZeroCreditIsZero(t *testing.T) {
	require.True(t, types.ProRata(uhash(1_000), math.ZeroInt(), math.NewInt(100)).IsZero())
	require.True(t, types.ProRata(uhash(1_000), math.NewInt(10), math.ZeroInt()).IsZero())
}

// TestProRataFullCreditTakesFullBudget
func TestProRataFullCreditTakesFullBudget(t *testing.T) {
	budget := uhash(1_000)
	got := types.ProRata(budget, math.NewInt(100), math.NewInt(100))
	require.Equal(t, budget.String(), got.String())
}

// TestProRataClampsExcessCredit: a credit larger than the total cannot claim
// more than the whole budget.
func TestProRataClampsExcessCredit(t *testing.T) {
	budget := uhash(1_000)
	got := types.ProRata(budget, math.NewInt(500), math.NewInt(100))
	require.Equal(t, budget.String(), got.String())
}

// ---------------------------------------------------------------------------
// Params validation
// ---------------------------------------------------------------------------

func TestDefaultParamsAreValid(t *testing.T) {
	require.NoError(t, types.DefaultParams().Validate())
}

// TestConcentrationCapCannotBeDisabled: this is the parameter that makes two
// nodes trading fake traffic unprofitable, and governance must not be able to
// switch it off.
func TestConcentrationCapCannotBeDisabled(t *testing.T) {
	p := types.DefaultParams()
	p.MaxClientConcentrationBps = 0

	err := p.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "cannot be disabled")
}

// TestProviderShareCapCannotBeDisabled
func TestProviderShareCapCannotBeDisabled(t *testing.T) {
	p := types.DefaultParams()
	p.MaxProviderShareBps = 0
	require.Error(t, p.Validate())
}

// TestChallengesCannotBeDisabled: unchallenged storage is self-reported
// storage.
func TestChallengesCannotBeDisabled(t *testing.T) {
	p := types.DefaultParams()
	p.ChallengesPerEpoch = 0

	err := p.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "self-reported")
}

// TestBondCannotBeZero: without a bond, the cost of being caught cheating is
// the cost of a new keypair.
func TestBondCannotBeZero(t *testing.T) {
	p := types.DefaultParams()
	p.MinBond = sdk.NewCoins()

	err := p.Validate()
	require.Error(t, err)
	require.Contains(t, err.Error(), "new keypair")
}

func TestUnbondingCannotBeZero(t *testing.T) {
	p := types.DefaultParams()
	p.UnbondingBlocks = 0
	require.Error(t, p.Validate())
}

// TestUnbondingOutlastsChallengeWindow: an operator must not be able to
// cheat, unbond and withdraw before the evidence lands.
func TestUnbondingOutlastsChallengeWindow(t *testing.T) {
	p := types.DefaultParams()
	require.Greater(t, p.UnbondingBlocks, p.ChallengeResponseBlocks*10,
		"the unbonding period is not comfortably longer than the challenge window")
	require.Greater(t, p.UnbondingBlocks, int64(p.EpochBlocks)*7,
		"the unbonding period is shorter than a week of epochs")
}

// ---------------------------------------------------------------------------
// Reserve accounting
// ---------------------------------------------------------------------------

// TestReserveRejectsOverspend is the invariant asserted at every settlement.
func TestReserveRejectsOverspend(t *testing.T) {
	r := types.ReserveState{
		Initial:      hash(100),
		TotalEmitted: hash(101),
		TotalSlashed: sdk.NewCoins(),
	}
	err := r.Validate()
	require.Error(t, err)
	require.True(t, types.ErrReserveOverspend.Is(err), "got %v", err)
}

// TestSlashedBondRaisesTheCeiling: slashed bond returns to the reserve rather
// than being burned, so it legitimately funds honest providers.
func TestSlashedBondRaisesTheCeiling(t *testing.T) {
	r := types.ReserveState{
		Initial:      hash(100),
		TotalEmitted: hash(105),
		TotalSlashed: hash(10),
	}
	require.NoError(t, r.Validate(),
		"emission up to initial plus slashed must be permitted")
}

func TestDefaultGenesisIsValid(t *testing.T) {
	gs := types.DefaultGenesis()
	require.NoError(t, gs.Validate())
	require.Empty(t, gs.Assigners,
		"no assigners at genesis: a provider must not be able to assign work to itself")
	require.Equal(t, "500000000000000uhash", gs.Reserve.Initial.String())
}
