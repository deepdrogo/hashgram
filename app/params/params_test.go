package params_test

import (
	"strings"
	"testing"

	"cosmossdk.io/math"

	"github.com/hashgram/hashgram/app/params"
)

// TestMaxSupplyIsExactlyOneBillion pins the single most important number in
// the protocol. §116.1: HASH has a maximum canonical supply of 1,000,000,000.
func TestMaxSupplyIsExactlyOneBillion(t *testing.T) {
	if params.MaxSupplyHash != 1_000_000_000 {
		t.Fatalf("max supply is %d HASH, must be 1,000,000,000", params.MaxSupplyHash)
	}
	want := math.NewInt(1_000_000_000_000_000) // 1e15 uhash
	if got := params.MaxSupplyBase(); !got.Equal(want) {
		t.Fatalf("max supply base = %s uhash, want %s uhash", got, want)
	}
}

// TestGenesisAllocationsSumToMaxSupply guards against a distribution table that
// silently creates or destroys supply.
func TestGenesisAllocationsSumToMaxSupply(t *testing.T) {
	if got := params.TotalAllocatedHash(); got != params.MaxSupplyHash {
		t.Fatalf("allocations sum to %d HASH, want %d HASH", got, params.MaxSupplyHash)
	}

	// Every documented percentage must hold exactly.
	pct := func(hash int64) float64 {
		return float64(hash) / float64(params.MaxSupplyHash) * 100
	}
	for name, tc := range map[string]struct {
		amount  int64
		percent float64
	}{
		"founder":         {params.AllocFounderHash, 20},
		"service_reserve": {params.AllocServiceReserveHash, 50},
		"treasury":        {params.AllocTreasuryHash, 15},
		"growth":          {params.AllocGrowthHash, 5},
		"dev_grants":      {params.AllocDevGrantsHash, 5},
		"liquidity":       {params.AllocLiquidityHash, 5},
	} {
		if got := pct(tc.amount); got != tc.percent {
			t.Errorf("%s is %.4f%% of supply, want %.4f%%", name, got, tc.percent)
		}
	}
}

// TestFounderAllocationSplit checks §14: 20M unlocked + 180M vested = 200M.
func TestFounderAllocationSplit(t *testing.T) {
	if params.AllocFounderHash != 200_000_000 {
		t.Fatalf("founder allocation is %d HASH, must be 200,000,000", params.AllocFounderHash)
	}
	sum := params.FounderUnlockedAtGenesisHash + params.FounderVestedHash
	if sum != params.AllocFounderHash {
		t.Fatalf("20M + 180M = %d HASH, want %d HASH", sum, params.AllocFounderHash)
	}
	if params.FounderVestingPeriods != 96 {
		t.Fatalf("vesting periods = %d, want 96 (8 years monthly)", params.FounderVestingPeriods)
	}
}

// TestFounderFeeIsOnePercentAndHardCapped checks §15 and §116.3.
func TestFounderFeeIsOnePercentAndHardCapped(t *testing.T) {
	if params.FounderFeeBasisPoints != 100 {
		t.Fatalf("founder fee is %d bps, must be 100 bps (1%%)", params.FounderFeeBasisPoints)
	}
	if params.MaxFounderFeeBasisPoints != 100 {
		t.Fatalf("founder fee ceiling is %d bps, must be 100 bps; "+
			"raising it must require a binary upgrade", params.MaxFounderFeeBasisPoints)
	}
	if params.BasisPointDenominator != 10_000 {
		t.Fatalf("basis point denominator is %d, want 10000", params.BasisPointDenominator)
	}
}

// TestWelcomePoolMatchesTierSchedule checks §19: the worst-case payout of the
// tier schedule must equal the reserved pool, exactly.
func TestWelcomePoolMatchesTierSchedule(t *testing.T) {
	worst := int64(params.WelcomeTier1Limit)*params.WelcomeTier1Hash +
		int64(params.WelcomeTier2Limit-params.WelcomeTier1Limit)*params.WelcomeTier2Hash +
		int64(params.WelcomeTier3Limit-params.WelcomeTier2Limit)*params.WelcomeTier3Hash

	if worst != 1_850_000 {
		t.Fatalf("worst-case welcome payout is %d HASH, want 1,850,000 HASH", worst)
	}
	if params.WelcomePoolHash != worst {
		t.Fatalf("welcome pool is %d HASH but worst case is %d HASH",
			params.WelcomePoolHash, worst)
	}
	if params.WelcomePoolHash > params.AllocGrowthHash {
		t.Fatalf("welcome pool %d HASH does not fit in growth allocation %d HASH",
			params.WelcomePoolHash, params.AllocGrowthHash)
	}
}

// TestSupplyInvariantsHold runs the same check that init() runs.
func TestSupplyInvariantsHold(t *testing.T) {
	if err := params.ValidateSupplyInvariants(); err != nil {
		t.Fatalf("supply invariants violated: %v", err)
	}
}

// TestDenomAndPrefixes pins the user-visible identifiers.
func TestDenomAndPrefixes(t *testing.T) {
	if params.BaseCoinDenom != "uhash" {
		t.Errorf("base denom = %q, want uhash", params.BaseCoinDenom)
	}
	if params.HumanCoinDenom != "HASH" {
		t.Errorf("human denom = %q, want HASH", params.HumanCoinDenom)
	}
	if params.CoinDecimals != 6 || params.MicroUnit != 1_000_000 {
		t.Errorf("1 HASH must be 10^6 uhash, got decimals=%d micro=%d",
			params.CoinDecimals, params.MicroUnit)
	}
	if params.Bech32Prefix != "hash" {
		t.Errorf("bech32 prefix = %q, want hash", params.Bech32Prefix)
	}
	for _, p := range []string{
		params.Bech32PrefixAccAddr,
		params.Bech32PrefixValAddr,
		params.Bech32PrefixConsAddr,
	} {
		if !strings.HasPrefix(p, "hash") {
			t.Errorf("prefix %q does not start with hash", p)
		}
	}
}

// TestHashToBase checks the unit conversion used by all genesis tooling.
func TestHashToBase(t *testing.T) {
	for _, tc := range []struct {
		hash int64
		want string
	}{
		{0, "0"},
		{1, "1000000"},
		{100, "100000000"},
		{1_850_000, "1850000000000"},
		{200_000_000, "200000000000000"},
		{1_000_000_000, "1000000000000000"},
	} {
		if got := params.HashToBase(tc.hash); got.String() != tc.want {
			t.Errorf("HashToBase(%d) = %s, want %s", tc.hash, got, tc.want)
		}
	}
}
