package model

import (
	"bytes"
	"strings"
	"testing"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	foundertypes "github.com/hashgram/hashgram/x/founder/types"
	serviceprooftypes "github.com/hashgram/hashgram/x/serviceproof/types"
	welcometypes "github.com/hashgram/hashgram/x/welcome/types"
)

func TestMain(m *testing.M) {
	hgparams.SetSDKConfig()
	m.Run()
}

// The genesis ledger must reconstruct the canonical supply exactly. If this
// fails, every number the tool produces is off by the same amount.
func TestGenesisLedgerMatchesCanonicalSupply(t *testing.T) {
	l, err := NewGenesisLedger()
	if err != nil {
		t.Fatalf("building the genesis ledger: %v", err)
	}

	if want := hgparams.MaxSupplyBase(); !l.Total().Equal(want) {
		t.Fatalf("genesis supply = %s, want %s", l.Total(), want)
	}

	// And each bucket must match the allocation table it came from.
	cases := []struct {
		bucket string
		hash   int64
	}{
		{BucketFounderLocked, hgparams.FounderVestedHash},
		{BucketFounderLiquid, hgparams.FounderUnlockedAtGenesisHash},
		{BucketServiceReserve, hgparams.AllocServiceReserveHash},
		{BucketTreasury, hgparams.AllocTreasuryHash},
		{BucketDevGrants, hgparams.AllocDevGrantsHash},
		{BucketLiquidity, hgparams.AllocLiquidityHash},
		{BucketGrowth, hgparams.AllocGrowthHash},
	}
	for _, c := range cases {
		if want := hgparams.HashToBase(c.hash); !l.Get(c.bucket).Equal(want) {
			t.Errorf("bucket %s = %s, want %s", c.bucket, l.Get(c.bucket), want)
		}
	}

	// Nobody starts with circulating coins except the Founder's unlocked
	// portion, which is a genesis fact rather than a modelling choice.
	for _, b := range []string{BucketUsers, BucketProviders, BucketValidators} {
		if l.Get(b).IsPositive() {
			t.Errorf("bucket %s starts at %s, want 0", b, l.Get(b))
		}
	}
}

// A ledger must refuse to pay out coins it does not hold, because a
// simulation that overdraws is modelling inflation.
func TestLedgerRefusesToOverdraw(t *testing.T) {
	l, err := NewGenesisLedger()
	if err != nil {
		t.Fatalf("building the genesis ledger: %v", err)
	}

	tooMuch := hgparams.HashToBase(hgparams.AllocTreasuryHash).AddRaw(1)
	if err := l.Move(BucketTreasury, BucketUsers, tooMuch); err == nil {
		t.Fatal("moving more than the treasury holds succeeded; it must fail")
	}

	if err := l.AssertInvariant(); err != nil {
		t.Fatalf("a refused move left the ledger inconsistent: %v", err)
	}

	// A negative move must also be refused rather than acting as a reverse
	// transfer.
	if err := l.Move(BucketTreasury, BucketUsers, math.NewInt(-1)); err == nil {
		t.Fatal("a negative move succeeded; it must fail")
	}
}

// The Welcome pool cap must hold no matter how many users the scenario
// invents. This is the ledger-level mirror of the on-chain cap.
func TestWelcomePoolCapIsEnforced(t *testing.T) {
	l, err := NewGenesisLedger()
	if err != nil {
		t.Fatalf("building the genesis ledger: %v", err)
	}

	poolCap := hgparams.HashToBase(hgparams.WelcomePoolHash)

	// Ask for twice the pool in one payment.
	paid, err := l.PayWelcome(poolCap.MulRaw(2))
	if err != nil {
		t.Fatalf("paying welcome: %v", err)
	}
	if !paid.Equal(poolCap) {
		t.Fatalf("first payment = %s, want the pool cap %s", paid, poolCap)
	}

	// Every further payment must be zero.
	again, err := l.PayWelcome(hgparams.HashToBase(50))
	if err != nil {
		t.Fatalf("paying welcome after exhaustion: %v", err)
	}
	if !again.IsZero() {
		t.Fatalf("payment after exhaustion = %s, want 0", again)
	}

	if err := l.AssertInvariant(); err != nil {
		t.Fatalf("invariant broken: %v", err)
	}
}

// The headline property: across every preset, over the full horizon, total
// supply never changes.
func TestSupplyIsInvariantAcrossEveryPreset(t *testing.T) {
	for _, s := range Presets() {
		t.Run(s.Name, func(t *testing.T) {
			res, err := Run(s)
			if err != nil {
				t.Fatalf("running %s: %v", s.Name, err)
			}

			if want := hgparams.MaxSupplyBase(); !res.FinalLedger.Total().Equal(want) {
				t.Fatalf("final supply = %s, want %s", res.FinalLedger.Total(), want)
			}

			// Run asserts the invariant every epoch internally, so reaching
			// here already proves it held throughout. Check the per-year
			// balances too, since those are what the report prints.
			for _, y := range res.Years {
				sum := math.ZeroInt()
				for _, b := range Buckets() {
					sum = sum.Add(y.Balances[b])
				}
				if !sum.Equal(hgparams.MaxSupplyBase()) {
					t.Fatalf("year %d balances sum to %s, want %s",
						y.Year, sum, hgparams.MaxSupplyBase())
				}
			}
		})
	}
}

// The whole point of importing the chain's functions is parity. If the
// simulator's emission curve ever diverges from EmissionForEpoch, this fails.
func TestEmissionMatchesTheChainFunction(t *testing.T) {
	params := serviceprooftypes.DefaultParams()

	// Walk the chain's own projection and confirm the simulator's per-epoch
	// draw cannot exceed it. Enough providers that the cap does not bind.
	projection := serviceprooftypes.ProjectEmission(
		serviceprooftypes.InitialReserve(), params, 30, 1)
	if len(projection) == 0 {
		t.Fatal("the chain's projection returned nothing")
	}

	l, err := NewGenesisLedger()
	if err != nil {
		t.Fatalf("building the genesis ledger: %v", err)
	}

	const providers = 100 // 100 * 5% = 500% of budget, so the cap never binds

	for i, want := range projection {
		budget := serviceprooftypes.EmissionForEpoch(l.ServiceReserveCoins(), params)
		if !budget.Equal(want.Budget) {
			t.Fatalf("epoch %d: simulator budget %s, chain budget %s",
				i+1, budget, want.Budget)
		}

		perProvider := serviceprooftypes.ProviderRewardCap(budget, params).
			AmountOf(hgparams.BaseCoinDenom)
		drawable := perProvider.MulRaw(providers)
		payout := budget.AmountOf(hgparams.BaseCoinDenom)
		if drawable.LT(payout) {
			t.Fatalf("epoch %d: the cap bound unexpectedly with %d providers", i+1, providers)
		}

		if _, err := l.MoveUpTo(BucketServiceReserve, BucketProviders, payout); err != nil {
			t.Fatalf("epoch %d payout: %v", i+1, err)
		}

		if got := l.ServiceReserveCoins(); !got.Equal(want.ReserveAfter) {
			t.Fatalf("epoch %d: simulator reserve %s, chain reserve %s",
				i+1, got, want.ReserveAfter)
		}
	}
}

// The welcome payouts in a run must equal what the chain's tier function
// would award for the same sequence numbers.
func TestWelcomePayoutsMatchTheChainTierSchedule(t *testing.T) {
	// A scenario with a known, small number of joiners so the expected total
	// can be computed directly from the tier function.
	s := Scenario{
		Name:              "tier-parity",
		Description:       "parity check",
		Years:             1,
		UsersYear1:        3_650, // 10 per epoch, no remainder
		UserGrowthPercent: 0,
		ProvidersYear1:    0,
		TxPerUserPerDay:   0,
		AvgTxFeeUhash:     0,
		ActiveUserPercent: 0,
	}

	res, err := Run(s)
	if err != nil {
		t.Fatalf("running the scenario: %v", err)
	}

	expected := math.ZeroInt()
	for seq := uint64(1); seq <= uint64(s.UsersYear1); seq++ {
		expected = expected.Add(hgparams.HashToBase(welcometypes.TierAmountHash(seq)))
	}

	got := res.Years[0].WelcomePaid
	if !got.Equal(expected) {
		t.Fatalf("welcome paid = %s, want %s (from the chain's tier function)", got, expected)
	}

	// Every one of these is in tier one, so the amount is checkable by hand.
	byHand := hgparams.HashToBase(s.UsersYear1 * hgparams.WelcomeTier1Hash)
	if !got.Equal(byHand) {
		t.Fatalf("welcome paid = %s, want %s by hand", got, byHand)
	}
}

// The Founder share must be exactly the chain's FounderCut of collected
// revenue, and must never touch principal.
func TestFounderShareMatchesTheChainCut(t *testing.T) {
	fp := foundertypes.DefaultParams()

	// The worked example from the specification: 100 HASH of qualifying
	// revenue yields exactly 1 HASH.
	revenue := sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(100)))
	cut := fp.FounderCut(revenue).AmountOf(hgparams.BaseCoinDenom)
	if want := hgparams.HashToBase(1); !cut.Equal(want) {
		t.Fatalf("founder cut of 100 HASH revenue = %s, want %s", cut, want)
	}

	// In a full run, cumulative Founder revenue must equal exactly 1% of the
	// revenue actually collected, allowing for per-epoch truncation which can
	// only ever round down.
	res, err := Run(mustPreset(t, "baseline"))
	if err != nil {
		t.Fatalf("running baseline: %v", err)
	}

	totalRevenue := math.ZeroInt()
	totalFounder := math.ZeroInt()
	for _, y := range res.Years {
		totalRevenue = totalRevenue.Add(y.GasRevenue).Add(y.ServiceFeeRevenue)
		totalFounder = totalFounder.Add(y.FounderRevenue)
	}

	upperBound := totalRevenue.
		Mul(math.NewIntFromUint64(uint64(hgparams.FounderFeeBasisPoints))).
		Quo(math.NewIntFromUint64(uint64(hgparams.BasisPointDenominator)))

	if totalFounder.GT(upperBound) {
		t.Fatalf("founder revenue %s exceeds %d bps of collected revenue %s (bound %s)",
			totalFounder, hgparams.FounderFeeBasisPoints, totalRevenue, upperBound)
	}

	// Truncation loss is bounded by one uhash per epoch, so the shortfall
	// must be small. A large shortfall would mean revenue was being missed.
	epochs := int64(res.Scenario.Years * EpochsPerYear)
	if shortfall := upperBound.Sub(totalFounder); shortfall.GT(math.NewInt(epochs)) {
		t.Fatalf("founder revenue is %s below the 1%% bound, more than the %d uhash "+
			"that per-epoch truncation can explain", shortfall, epochs)
	}
}

// Founder vesting must release exactly FounderVestedHash and no more, and
// must not finish early.
func TestFounderVestingReleasesExactlyTheVestedAmount(t *testing.T) {
	s := mustPreset(t, "baseline")
	s.Years = int(hgparams.FounderVestingYears) + 2

	res, err := Run(s)
	if err != nil {
		t.Fatalf("running the scenario: %v", err)
	}

	totalVested := math.ZeroInt()
	for _, y := range res.Years {
		totalVested = totalVested.Add(y.FounderVested)
	}

	want := hgparams.HashToBase(hgparams.FounderVestedHash)
	if !totalVested.Equal(want) {
		t.Fatalf("total vested = %s, want exactly %s", totalVested, want)
	}

	// And nothing may remain locked once the horizon exceeds the schedule.
	if remaining := res.FinalLedger.Get(BucketFounderLocked); remaining.IsPositive() {
		t.Fatalf("%s uhash still locked after %d years, schedule is %d years",
			remaining, s.Years, hgparams.FounderVestingYears)
	}

	// Vesting must not have completed before the schedule allows. At the end
	// of year 4 of an 8 year schedule, roughly half should have vested.
	half := math.ZeroInt()
	for _, y := range res.Years {
		if y.Year > 4 {
			break
		}
		half = half.Add(y.FounderVested)
	}
	if half.GTE(want) {
		t.Fatalf("%s of %s vested within 4 years of an %d year schedule",
			half, want, hgparams.FounderVestingYears)
	}
}

// A network with few providers cannot draw the whole epoch budget. This is
// the property that makes the reward cap meaningful, and it must show up in
// the simulation rather than being asserted only in the chain's unit tests.
func TestFewProvidersCannotDrainTheReserve(t *testing.T) {
	base := mustPreset(t, "baseline")
	base.Years = 10
	base.ProviderGrowthPercent = 0

	small := base
	small.ProvidersYear1 = 2
	large := base
	large.ProvidersYear1 = 5_000

	smallRes, err := Run(small)
	if err != nil {
		t.Fatalf("running the small network: %v", err)
	}
	largeRes, err := Run(large)
	if err != nil {
		t.Fatalf("running the large network: %v", err)
	}

	smallReserve := smallRes.FinalLedger.Get(BucketServiceReserve)
	largeReserve := largeRes.FinalLedger.Get(BucketServiceReserve)

	if !smallReserve.GT(largeReserve) {
		t.Fatalf("a 2 provider network left %s in the reserve and a 5000 provider "+
			"network left %s; the small network should have drawn far less",
			smallReserve, largeReserve)
	}

	// With a 5% per-provider cap, 2 providers can draw at most 10% of each
	// epoch's budget while 5,000 providers are unconstrained. Over 3,650
	// epochs the small network should therefore have spent a small fraction
	// of the reserve and the large network most of it.
	genesisReserve := hgparams.HashToBase(hgparams.AllocServiceReserveHash)
	smallDrawn := genesisReserve.Sub(smallReserve)
	largeDrawn := genesisReserve.Sub(largeReserve)

	if pct := smallDrawn.MulRaw(100).Quo(genesisReserve).Int64(); pct > 25 {
		t.Errorf("a 2 provider network drew %d%% of the reserve in ten years; the "+
			"per-provider cap should have held it far below that", pct)
	}
	if pct := largeDrawn.MulRaw(100).Quo(genesisReserve).Int64(); pct < 75 {
		t.Errorf("a 5,000 provider network drew only %d%% of the reserve in ten "+
			"years; with the cap not binding it should have drawn most of it", pct)
	}
	if !largeDrawn.GT(smallDrawn.MulRaw(4)) {
		t.Errorf("the large network drew %s and the small one %s; the cap should "+
			"produce a much wider gap than that", largeDrawn, smallDrawn)
	}

	// Both runs must still respect the supply invariant.
	for _, r := range []*Result{smallRes, largeRes} {
		if !r.FinalLedger.Total().Equal(hgparams.MaxSupplyBase()) {
			t.Errorf("scenario %s broke the supply invariant: %s",
				r.Scenario.Name, r.FinalLedger.Total())
		}
	}
}

// The reserve declines geometrically and must never go negative or be
// exceeded by a single epoch's draw.
func TestReserveNeverGoesNegative(t *testing.T) {
	s := mustPreset(t, "reserve-drain")
	res, err := Run(s)
	if err != nil {
		t.Fatalf("running reserve-drain: %v", err)
	}

	prev := hgparams.HashToBase(hgparams.AllocServiceReserveHash)
	for _, y := range res.Years {
		if y.ReserveRemaining.IsNegative() {
			t.Fatalf("year %d: reserve went negative (%s)", y.Year, y.ReserveRemaining)
		}
		if y.ReserveRemaining.GT(prev) {
			t.Fatalf("year %d: reserve grew from %s to %s; it is finite and must only shrink",
				y.Year, prev, y.ReserveRemaining)
		}
		prev = y.ReserveRemaining
	}

	// Cumulative emission can never exceed the genesis reserve.
	last := res.Years[len(res.Years)-1]
	genesisReserve := hgparams.HashToBase(hgparams.AllocServiceReserveHash)
	if last.CumulativeEmission.GT(genesisReserve) {
		t.Fatalf("cumulative emission %s exceeds the genesis reserve %s",
			last.CumulativeEmission, genesisReserve)
	}
}

// Determinism: the same scenario must produce byte-identical output.
func TestRunsAreDeterministic(t *testing.T) {
	s := mustPreset(t, "conservative")
	s.Years = 5

	var first, second bytes.Buffer
	for _, buf := range []*bytes.Buffer{&first, &second} {
		res, err := Run(s)
		if err != nil {
			t.Fatalf("running the scenario: %v", err)
		}
		if err := res.WriteJSON(buf); err != nil {
			t.Fatalf("writing JSON: %v", err)
		}
	}

	if first.String() != second.String() {
		t.Fatal("two runs of the same scenario produced different output")
	}
}

// Invalid scenarios must be refused rather than producing nonsense.
func TestInvalidScenariosAreRejected(t *testing.T) {
	base := mustPreset(t, "baseline")

	cases := []struct {
		name  string
		mutch func(*Scenario)
	}{
		{"zero years", func(s *Scenario) { s.Years = 0 }},
		{"negative years", func(s *Scenario) { s.Years = -1 }},
		{"absurd horizon", func(s *Scenario) { s.Years = 500 }},
		{"negative users", func(s *Scenario) { s.UsersYear1 = -1 }},
		{"negative fee", func(s *Scenario) { s.AvgTxFeeUhash = -1 }},
		{"active percent over 100", func(s *Scenario) { s.ActiveUserPercent = 101 }},
		{"empty name", func(s *Scenario) { s.Name = "  " }},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			s := base
			c.mutch(&s)
			if _, err := Run(s); err == nil {
				t.Fatalf("scenario with %s was accepted; it must be rejected", c.name)
			}
		})
	}
}

// The report must state the supply result rather than leaving the reader to
// work it out, and must not claim protocol facts the chain does not have.
func TestReportStatesTheSupplyResult(t *testing.T) {
	s := mustPreset(t, "conservative")
	s.Years = 3

	res, err := Run(s)
	if err != nil {
		t.Fatalf("running the scenario: %v", err)
	}

	var buf bytes.Buffer
	if err := res.WriteTable(&buf); err != nil {
		t.Fatalf("writing the table: %v", err)
	}
	out := buf.String()

	for _, want := range []string{
		"SUPPLY INVARIANT",
		"UNCHANGED",
		"x/mint is not wired into the application",
		"1000000000000000", // the canonical supply in uhash
	} {
		if !strings.Contains(out, want) {
			t.Errorf("the report does not mention %q", want)
		}
	}

	if strings.Contains(out, "BROKEN") {
		t.Error("the report says the supply invariant was broken")
	}
}

// CSV must be parseable and must carry one row per year.
func TestCSVHasOneRowPerYear(t *testing.T) {
	s := mustPreset(t, "baseline")
	s.Years = 4

	res, err := Run(s)
	if err != nil {
		t.Fatalf("running the scenario: %v", err)
	}

	var buf bytes.Buffer
	if err := res.WriteCSV(&buf); err != nil {
		t.Fatalf("writing CSV: %v", err)
	}

	lines := strings.Split(strings.TrimSpace(buf.String()), "\n")
	if got, want := len(lines), s.Years+1; got != want {
		t.Fatalf("CSV has %d lines, want %d (header plus %d years)", got, want, s.Years)
	}
	if !strings.HasPrefix(lines[0], "year,") {
		t.Errorf("CSV header does not start with the year column: %q", lines[0])
	}
}

func mustPreset(t *testing.T, name string) Scenario {
	t.Helper()
	s, err := PresetByName(name)
	if err != nil {
		t.Fatalf("looking up preset %q: %v", name, err)
	}
	return s
}
