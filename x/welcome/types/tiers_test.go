package types_test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/welcome/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	os.Exit(m.Run())
}

// TestTierBoundariesAreExact is §96 verbatim. Off-by-one errors at a tier
// boundary would either overpay or underpay by a factor of ten, and the
// boundary is exactly where such an error hides.
func TestTierBoundariesAreExact(t *testing.T) {
	for _, tc := range []struct {
		sequence uint64
		wantHash int64
	}{
		// The stated boundaries.
		{10_000, 50},
		{10_001, 5},
		{100_000, 5},
		{100_001, 1},
		{1_000_000, 1},
		{1_000_001, 0},

		// The surrounding values, because a boundary test that only checks
		// the boundary cannot distinguish "correct" from "shifted by one in
		// both directions".
		{1, 50},
		{2, 50},
		{9_999, 50},
		{10_002, 5},
		{99_999, 5},
		{100_002, 1},
		{999_999, 1},
		{1_000_002, 0},

		// Sequence 0 is not a valid position.
		{0, 0},

		// Far past the end.
		{2_000_000, 0},
		{1 << 40, 0},
	} {
		got := types.TierAmountHash(tc.sequence)
		require.Equal(t, tc.wantHash, got,
			"sequence %d pays %d HASH, want %d HASH", tc.sequence, got, tc.wantHash)
	}
}

// TestTierAmountCoins checks the coin conversion at each tier.
func TestTierAmountCoins(t *testing.T) {
	require.Equal(t, "50000000uhash", types.TierAmount(1).String())
	require.Equal(t, "50000000uhash", types.TierAmount(10_000).String())
	require.Equal(t, "5000000uhash", types.TierAmount(10_001).String())
	require.Equal(t, "5000000uhash", types.TierAmount(100_000).String())
	require.Equal(t, "1000000uhash", types.TierAmount(100_001).String())
	require.Equal(t, "1000000uhash", types.TierAmount(1_000_000).String())
	require.True(t, types.TierAmount(1_000_001).IsZero())
	require.True(t, types.TierAmount(0).IsZero())
}

func TestIsExhausted(t *testing.T) {
	require.True(t, types.IsExhausted(0), "sequence 0 is not a valid position")
	require.False(t, types.IsExhausted(1))
	require.False(t, types.IsExhausted(1_000_000))
	require.True(t, types.IsExhausted(1_000_001))
	require.True(t, types.IsExhausted(9_999_999))
}

// TestMaxPoolMatchesSchedule: the pool the genesis funds must be exactly the
// worst-case payout of the schedule. Computed from the tiers rather than
// restated, so the two cannot drift apart.
//
//	 10,000 x 50 = 500,000
//	 90,000 x  5 = 450,000
//	900,000 x  1 = 900,000
//	              ---------
//	              1,850,000
func TestMaxPoolMatchesSchedule(t *testing.T) {
	require.Equal(t, "1850000000000uhash", types.MaxPool().String())

	wantHash := int64(1_850_000)
	require.Equal(t, wantHash, hgparams.WelcomePoolHash,
		"the declared welcome pool does not match the schedule's worst case")
	require.Equal(t, hgparams.HashToBase(wantHash).String(),
		types.MaxPool().AmountOf(hgparams.BaseCoinDenom).String())
}

// TestMaxPoolEqualsBruteForceSum computes the payout by iterating every
// sequence position, which is slow but assumption-free, and compares it with
// the closed-form pool figure.
func TestMaxPoolEqualsBruteForceSum(t *testing.T) {
	var total int64
	for seq := uint64(1); seq <= hgparams.WelcomeTier3Limit; seq++ {
		total += types.TierAmountHash(seq)
	}
	require.Equal(t, hgparams.WelcomePoolHash, total,
		"summing every sequence position gives %d HASH but the pool is %d HASH",
		total, hgparams.WelcomePoolHash)
}

func TestTiersAreAscendingAndCoverTheSchedule(t *testing.T) {
	tiers := types.Tiers()
	require.Len(t, tiers, 3)

	prev := uint64(0)
	for i, tier := range tiers {
		require.Greater(t, tier.MaxSequence, prev,
			"tier %d max_sequence %d is not greater than the previous %d", i, tier.MaxSequence, prev)
		require.False(t, tier.Amount.IsZero(), "tier %d pays nothing", i)
		prev = tier.MaxSequence
	}
	require.Equal(t, hgparams.WelcomeTier3Limit, prev,
		"the last tier does not end at the documented final position")
}

// TestTierAmountsAreStrictlyDecreasing: an earlier user must never receive
// less than a later one, otherwise the schedule would reward waiting.
func TestTierAmountsAreStrictlyDecreasing(t *testing.T) {
	tiers := types.Tiers()
	for i := 1; i < len(tiers); i++ {
		prev := tiers[i-1].Amount.AmountOf(hgparams.BaseCoinDenom)
		cur := tiers[i].Amount.AmountOf(hgparams.BaseCoinDenom)
		require.True(t, cur.LT(prev),
			"tier %d pays %s which is not less than tier %d's %s", i, cur, i-1, prev)
	}
}
