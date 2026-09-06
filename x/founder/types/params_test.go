package types_test

import (
	"os"
	"testing"

	"github.com/stretchr/testify/require"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/founder/types"
)

// TestMain installs the Hashgram bech32 prefixes before any address in this
// package is parsed. Without it the SDK still expects the "cosmos" prefix and
// every hash1... address in these tests would be rejected for the wrong
// reason.
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

func onePercentParams() types.Params {
	p := types.DefaultParams()
	p.Beneficiary = "hash1wwch3vfg0y8nkh0ggm7yg7zeqkrv7e2e9qsqjf"
	return p
}

// TestFounderCutOnOneHundredHash is the arithmetic behind the specification's
// worked example (§15, §94): when the protocol earns 100 HASH of qualifying
// fee revenue, the Founder receives exactly 1 HASH. Not 0.999999, not
// 1.000001.
func TestFounderCutOnOneHundredHash(t *testing.T) {
	p := onePercentParams()

	qualifying := hash(100)
	require.Equal(t, "100000000uhash", qualifying.String(),
		"100 HASH must be 100,000,000 uhash")

	cut := p.FounderCut(qualifying)

	require.Equal(t, "1000000uhash", cut.String(),
		"founder share of 100 HASH of qualifying revenue must be exactly 1 HASH")
	require.True(t, cut.Equal(hash(1)))
}

// TestFounderCutIsExactAcrossMagnitudes checks the integer arithmetic at
// several scales, including the ones where a floating-point implementation
// would drift.
func TestFounderCutIsExactAcrossMagnitudes(t *testing.T) {
	p := onePercentParams()

	for _, tc := range []struct {
		name       string
		qualifying int64 // uhash
		wantCut    int64 // uhash
	}{
		{"zero", 0, 0},
		{"1 uhash truncates to nothing", 1, 0},
		{"99 uhash truncates to nothing", 99, 0},
		{"100 uhash yields 1 uhash", 100, 1},
		{"101 uhash still yields 1 uhash", 101, 1},
		{"199 uhash still yields 1 uhash", 199, 1},
		{"200 uhash yields 2 uhash", 200, 2},
		{"1 HASH yields 0.01 HASH", 1_000_000, 10_000},
		{"100 HASH yields 1 HASH", 100_000_000, 1_000_000},
		{"1000 HASH yields 10 HASH", 1_000_000_000, 10_000_000},
		{"whole supply yields 1% of supply", 1_000_000_000_000_000, 10_000_000_000_000},
	} {
		t.Run(tc.name, func(t *testing.T) {
			cut := p.FounderCut(uhash(tc.qualifying))
			got := int64(0)
			if !cut.IsZero() {
				got = cut.AmountOf(hgparams.BaseCoinDenom).Int64()
			}
			require.Equal(t, tc.wantCut, got,
				"FounderCut(%d uhash) = %d uhash, want %d uhash",
				tc.qualifying, got, tc.wantCut)
		})
	}
}

// TestFounderCutTruncatesTowardValidators: rounding must never favour the
// party who wrote the code. Any sub-uhash remainder stays with validators and
// delegators.
func TestFounderCutTruncatesTowardValidators(t *testing.T) {
	p := onePercentParams()

	for _, amount := range []int64{1, 7, 42, 99, 149, 199, 12_345, 999_999} {
		qualifying := uhash(amount)
		cut := p.FounderCut(qualifying)

		cutAmt := math.ZeroInt()
		if !cut.IsZero() {
			cutAmt = cut.AmountOf(hgparams.BaseCoinDenom)
		}

		// cut * 10000 <= amount * 100, i.e. the founder never receives more
		// than exactly 1% of the qualifying amount.
		lhs := cutAmt.MulRaw(int64(hgparams.BasisPointDenominator))
		rhs := math.NewInt(amount).MulRaw(int64(p.FeeBasisPoints))
		require.True(t, lhs.LTE(rhs),
			"founder cut %s of %d uhash exceeds %d bps", cutAmt, amount, p.FeeBasisPoints)

		// And the remainder is non-negative: the split cannot overdraw.
		remainder, negative := qualifying.SafeSub(cut...)
		require.False(t, negative)
		require.False(t, remainder.IsAnyNegative())
	}
}

// TestFounderCutOfZeroFeeIsZero: a governance-lowered fee of 0 bps must
// produce no accrual at all.
func TestFounderCutOfZeroFeeIsZero(t *testing.T) {
	p := onePercentParams()
	p.FeeBasisPoints = 0

	require.True(t, p.FounderCut(hash(1_000_000)).IsZero())
}

// TestFounderCutHandlesMultipleDenoms. HASH is the only denomination at
// launch, but the split must not silently drop a second denomination if one is
// ever routed.
func TestFounderCutHandlesMultipleDenoms(t *testing.T) {
	p := onePercentParams()

	qualifying := sdk.NewCoins(
		sdk.NewInt64Coin(hgparams.BaseCoinDenom, 100_000_000),
		sdk.NewInt64Coin("uother", 500_000),
	)
	cut := p.FounderCut(qualifying)

	require.Equal(t, int64(1_000_000), cut.AmountOf(hgparams.BaseCoinDenom).Int64())
	require.Equal(t, int64(5_000), cut.AmountOf("uother").Int64())
}

// ---------------------------------------------------------------------------
// The ceiling
// ---------------------------------------------------------------------------

// TestParamsRejectFeeAboveCeiling is the guarantee behind §15: there must be
// no way for governance to quietly raise the Founder share. Raising it
// requires a new binary that the validator set consciously adopts.
func TestParamsRejectFeeAboveCeiling(t *testing.T) {
	require.Equal(t, uint32(100), hgparams.MaxFounderFeeBasisPoints,
		"the ceiling itself must be 100 bps")

	for _, bps := range []uint32{101, 150, 500, 1_000, 10_000, 65_535} {
		p := onePercentParams()
		p.FeeBasisPoints = bps

		err := p.Validate()
		require.Error(t, err, "%d bps was accepted", bps)
		require.True(t, types.ErrFeeExceedsCeiling.Is(err), "got %v", err)
		require.Contains(t, err.Error(), "ceiling")
	}
}

// TestParamsAllowLoweringFee: governance may reduce the Founder share.
func TestParamsAllowLoweringFee(t *testing.T) {
	for _, bps := range []uint32{0, 1, 50, 99, 100} {
		p := onePercentParams()
		p.FeeBasisPoints = bps
		require.NoError(t, p.Validate(), "%d bps was rejected", bps)
	}
}

// ---------------------------------------------------------------------------
// Beneficiary
// ---------------------------------------------------------------------------

func TestParamsRejectMalformedBeneficiary(t *testing.T) {
	for _, addr := range []string{
		"not-an-address",
		"hash1invalid",
		"cosmos1wwch3vfg0y8nkh0ggm7yg7zeqkrv7e2ewkm5rt", // wrong prefix
	} {
		p := onePercentParams()
		p.Beneficiary = addr

		err := p.Validate()
		require.Error(t, err, "beneficiary %q was accepted", addr)
		require.True(t, types.ErrInvalidBeneficiary.Is(err), "got %v", err)
	}
}

// TestParamsAllowEmptyBeneficiary. An unset beneficiary is legitimate before
// the Founder has supplied a public address. Accrual is skipped while it is
// empty, so no value is misdirected or stranded.
func TestParamsAllowEmptyBeneficiary(t *testing.T) {
	p := types.DefaultParams()
	require.Equal(t, "", p.Beneficiary)
	require.NoError(t, p.Validate())
}

// TestDefaultParamsHaveNoBeneficiary. The Founder address cannot be invented
// by the software; there must be no plausible default that would let a chain
// launch paying revenue to somebody arbitrary.
func TestDefaultParamsHaveNoBeneficiary(t *testing.T) {
	p := types.DefaultParams()
	require.Empty(t, p.Beneficiary,
		"DefaultParams must not ship a beneficiary address")
	require.Equal(t, hgparams.FounderFeeBasisPoints, p.FeeBasisPoints)
}

func TestFeePercentRendering(t *testing.T) {
	p := onePercentParams()
	require.Equal(t, "1.00%", p.FeePercent())

	p.FeeBasisPoints = 25
	require.Equal(t, "0.25%", p.FeePercent())

	p.FeeBasisPoints = 0
	require.Equal(t, "0.00%", p.FeePercent())
}

// ---------------------------------------------------------------------------
// Genesis
// ---------------------------------------------------------------------------

func TestGenesisRejectsPaidExceedingAccrued(t *testing.T) {
	gs := types.DefaultGenesis()
	gs.Ledger.TotalAccrued = uhash(100)
	gs.Ledger.TotalPaid = uhash(101)

	require.Error(t, gs.Validate(),
		"a ledger claiming more paid than accrued describes coins that never existed")
}

func TestNewGenesisStateSetsBeneficiary(t *testing.T) {
	addr := "hash1wwch3vfg0y8nkh0ggm7yg7zeqkrv7e2e9qsqjf"
	gs := types.NewGenesisState(addr)

	require.Equal(t, addr, gs.Params.Beneficiary)
	require.Equal(t, hgparams.FounderFeeBasisPoints, gs.Params.FeeBasisPoints)
	require.NoError(t, gs.Validate())
	require.True(t, gs.Ledger.TotalAccrued.IsZero(), "a fresh chain must start with nothing accrued")
	require.True(t, gs.Ledger.TotalPaid.IsZero())
}
