package model

import (
	"fmt"
	"strings"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// EpochsPerYear is the number of settlement epochs in a simulated year.
//
// x/serviceproof's DefaultEpochBlocks is 21,600 blocks, which at the 4 second
// target block time is roughly one day. 365 epochs is therefore one year. The
// simulation works in epochs rather than blocks because the epoch is the unit
// the reward schedule is actually defined over.
const EpochsPerYear = 365

// Scenario is the set of inputs the chain cannot supply.
//
// Everything here is an assumption about adoption and usage, not a protocol
// constant. The protocol constants (supply, allocations, tier schedule,
// emission rate, Founder basis points) are imported from the chain packages
// and are deliberately not settable here: a simulator that lets you change
// the max supply is not simulating Hashgram.
type Scenario struct {
	// Name identifies the scenario in output.
	Name string

	// Description explains what the scenario is meant to represent.
	Description string

	// Years is the simulation horizon.
	Years int

	// UsersYear1 is the number of users who join during the first year.
	UsersYear1 int64

	// UserGrowthPercent is the year-over-year growth in new joiners,
	// expressed as a percentage. 100 means new joiners double each year; 0
	// means the same number joins every year; -50 means joining halves.
	UserGrowthPercent int64

	// MaxUsers caps cumulative users, turning unbounded exponential growth
	// into an S-curve.
	//
	// Without a ceiling, compounding growth over a twenty-year horizon
	// produces user counts larger than the human population, and the revenue
	// figures derived from them are not merely optimistic but impossible.
	// Zero means no ceiling, which is only sensible for short horizons.
	MaxUsers int64

	// ProvidersYear1 is the number of useful-service nodes at the start.
	ProvidersYear1 int64

	// ProviderGrowthPercent is the year-over-year growth in node count.
	ProviderGrowthPercent int64

	// TxPerUserPerDay is how many fee-paying transactions an active user
	// sends per day.
	TxPerUserPerDay float64

	// AvgTxFeeUhash is the average gas fee per transaction, in uhash.
	//
	// The chain does not fix a fee: minimum-gas-prices is an operator
	// setting and the actual fee depends on the transaction. This is the
	// single most speculative input in the model, which is why the report
	// prints it and why sensitivity to it is worth checking by re-running
	// with a different value.
	AvgTxFeeUhash int64

	// ServiceFeeShareOfGasPercent models non-gas protocol service fees
	// (username registration and renewal, storage and relay service fees)
	// as a percentage of gas revenue.
	//
	// Both paths route through x/feerouter and both are subject to the same
	// Founder share, so for supply purposes they differ only in volume.
	ServiceFeeShareOfGasPercent int64

	// ActiveUserPercent is the share of joined users who are transacting in
	// any given year. Cohorts do not stay fully active, and a model that
	// assumes they do overstates revenue substantially.
	ActiveUserPercent int64

	// LiquidityReleasePercentPerYear is the share of the remaining liquidity
	// reserve released into user hands per year, representing exchange
	// listings and market making.
	//
	// Users have to obtain HASH from somewhere in order to pay fees. Welcome
	// rewards alone are 1.85M HASH across a million users, which does not
	// fund years of transacting.
	LiquidityReleasePercentPerYear int64

	// TreasurySpendPercentPerYear is the share of the remaining treasury
	// disbursed per year by governance. Modelled as reaching users, since
	// grants and ecosystem spending end up in circulation.
	TreasurySpendPercentPerYear int64

	// DevGrantsSpendPercentPerYear is the equivalent for the dev grant pool.
	DevGrantsSpendPercentPerYear int64
}

// Validate rejects inputs that would produce meaningless output.
func (s Scenario) Validate() error {
	if strings.TrimSpace(s.Name) == "" {
		return fmt.Errorf("scenario name is required")
	}
	if s.Years <= 0 {
		return fmt.Errorf("years must be positive, got %d", s.Years)
	}
	if s.Years > 100 {
		return fmt.Errorf(
			"years is %d; beyond a few decades the adoption assumptions dominate "+
				"the protocol mechanics and the output is not informative", s.Years)
	}
	if s.UsersYear1 < 0 {
		return fmt.Errorf("users-year1 must not be negative")
	}
	if s.ProvidersYear1 < 0 {
		return fmt.Errorf("providers-year1 must not be negative")
	}
	if s.MaxUsers < 0 {
		return fmt.Errorf("max-users must not be negative")
	}
	if s.MaxUsers > 0 && s.MaxUsers < s.UsersYear1 {
		return fmt.Errorf(
			"max-users (%d) is below users-year1 (%d), so the first year would "+
				"already exceed the ceiling", s.MaxUsers, s.UsersYear1)
	}
	if s.TxPerUserPerDay < 0 {
		return fmt.Errorf("tx-per-user-per-day must not be negative")
	}
	if s.AvgTxFeeUhash < 0 {
		return fmt.Errorf("avg-tx-fee must not be negative")
	}
	for _, p := range []struct {
		name  string
		value int64
	}{
		{"active-user-percent", s.ActiveUserPercent},
		{"liquidity-release-percent", s.LiquidityReleasePercentPerYear},
		{"treasury-spend-percent", s.TreasurySpendPercentPerYear},
		{"dev-grants-spend-percent", s.DevGrantsSpendPercentPerYear},
		{"service-fee-share-percent", s.ServiceFeeShareOfGasPercent},
	} {
		if p.value < 0 || p.value > 100 {
			return fmt.Errorf("%s must be between 0 and 100, got %d", p.name, p.value)
		}
	}
	if s.UserGrowthPercent < -100 {
		return fmt.Errorf("user-growth-percent cannot be below -100")
	}
	if s.ProviderGrowthPercent < -100 {
		return fmt.Errorf("provider-growth-percent cannot be below -100")
	}
	return nil
}

// UsersJoiningInYear returns the number of new users in a given year, with
// year 1 being the first.
func (s Scenario) UsersJoiningInYear(year int) int64 {
	return growFrom(s.UsersYear1, s.UserGrowthPercent, year)
}

// ProvidersInYear returns the useful-service node count for a given year.
func (s Scenario) ProvidersInYear(year int) int64 {
	n := growFrom(s.ProvidersYear1, s.ProviderGrowthPercent, year)
	if n < 0 {
		return 0
	}
	return n
}

// growFrom compounds base by percent per year for (year-1) years.
//
// Integer arithmetic with an early exit at zero, so a shrinking scenario
// converges to zero rather than to a fractional tail.
func growFrom(base, percent int64, year int) int64 {
	if year <= 1 {
		return base
	}
	v := base
	for i := 1; i < year; i++ {
		if v == 0 {
			return 0
		}
		v = v + (v * percent / 100)
		if v < 0 {
			return 0
		}
	}
	return v
}

func userCeilingLabel(max int64) string {
	if max <= 0 {
		return "none"
	}
	return formatCount(max) + " cumulative"
}

// Presets are the scenarios shipped with the tool.
//
// Three shapes rather than one, because the interesting question is not
// "what happens" but "which conclusions survive across plausible futures".
// The reserve exhaustion timeline, for instance, is driven far more by node
// count than by user count, and that only becomes visible when comparing.
func Presets() []Scenario {
	return []Scenario{
		{
			Name: "conservative",
			Description: "Slow adoption. 50k users in year one growing 30% a year, " +
				"12 service nodes, light transaction volume. Twelve nodes is below " +
				"the twenty the per-provider cap requires, so this scenario shows " +
				"what the cap does to a small network's reward draw.",
			Years:                          20,
			UsersYear1:                     50_000,
			UserGrowthPercent:              30,
			MaxUsers:                       5_000_000,
			ProvidersYear1:                 12,
			ProviderGrowthPercent:          20,
			TxPerUserPerDay:                0.2,
			AvgTxFeeUhash:                  2_500,
			ServiceFeeShareOfGasPercent:    20,
			ActiveUserPercent:              30,
			LiquidityReleasePercentPerYear: 10,
			TreasurySpendPercentPerYear:    5,
			DevGrantsSpendPercentPerYear:   10,
		},
		{
			Name: "baseline",
			Description: "Steady adoption. 250k users in year one growing 60% a year, " +
				"100 service nodes, moderate transaction volume.",
			Years:                          20,
			UsersYear1:                     250_000,
			UserGrowthPercent:              60,
			MaxUsers:                       50_000_000,
			ProvidersYear1:                 100,
			ProviderGrowthPercent:          40,
			TxPerUserPerDay:                1.0,
			AvgTxFeeUhash:                  2_500,
			ServiceFeeShareOfGasPercent:    30,
			ActiveUserPercent:              40,
			LiquidityReleasePercentPerYear: 15,
			TreasurySpendPercentPerYear:    8,
			DevGrantsSpendPercentPerYear:   15,
		},
		{
			Name: "optimistic",
			Description: "Fast adoption. 1M users in year one growing 100% a year, " +
				"500 service nodes, heavy transaction volume.",
			Years:                          20,
			UsersYear1:                     1_000_000,
			UserGrowthPercent:              100,
			MaxUsers:                       500_000_000,
			ProvidersYear1:                 500,
			ProviderGrowthPercent:          60,
			TxPerUserPerDay:                3.0,
			AvgTxFeeUhash:                  2_500,
			ServiceFeeShareOfGasPercent:    40,
			ActiveUserPercent:              50,
			LiquidityReleasePercentPerYear: 20,
			TreasurySpendPercentPerYear:    10,
			DevGrantsSpendPercentPerYear:   20,
		},
		{
			Name: "reserve-drain",
			Description: "Stress test of reserve exhaustion. Enough nodes from day one " +
				"that the per-provider cap never limits the draw, so the reserve " +
				"is spent as fast as the schedule permits.",
			Years:                          40,
			UsersYear1:                     250_000,
			UserGrowthPercent:              60,
			MaxUsers:                       50_000_000,
			ProvidersYear1:                 5_000,
			ProviderGrowthPercent:          0,
			TxPerUserPerDay:                1.0,
			AvgTxFeeUhash:                  2_500,
			ServiceFeeShareOfGasPercent:    30,
			ActiveUserPercent:              40,
			LiquidityReleasePercentPerYear: 15,
			TreasurySpendPercentPerYear:    8,
			DevGrantsSpendPercentPerYear:   15,
		},
	}
}

// PresetByName looks up a shipped scenario.
func PresetByName(name string) (Scenario, error) {
	for _, s := range Presets() {
		if s.Name == name {
			return s, nil
		}
	}
	names := make([]string, 0, len(Presets()))
	for _, s := range Presets() {
		names = append(names, s.Name)
	}
	return Scenario{}, fmt.Errorf("unknown scenario %q; available: %s", name, strings.Join(names, ", "))
}

// Assumptions renders the scenario's inputs as label/value pairs, so that
// every result can be read next to what produced it.
func (s Scenario) Assumptions() [][2]string {
	return [][2]string{
		{"Horizon", fmt.Sprintf("%d years (%d epochs)", s.Years, s.Years*EpochsPerYear)},
		{"Users joining, year 1", formatCount(s.UsersYear1)},
		{"User growth", fmt.Sprintf("%+d%% per year", s.UserGrowthPercent)},
		{"User ceiling", userCeilingLabel(s.MaxUsers)},
		{"Active share of users", fmt.Sprintf("%d%%", s.ActiveUserPercent)},
		{"Service nodes, year 1", formatCount(s.ProvidersYear1)},
		{"Node growth", fmt.Sprintf("%+d%% per year", s.ProviderGrowthPercent)},
		{"Transactions per active user", fmt.Sprintf("%.2f per day", s.TxPerUserPerDay)},
		{"Average gas fee", fmt.Sprintf("%s uhash (%s HASH)",
			formatCount(s.AvgTxFeeUhash), formatFractionalHash(s.AvgTxFeeUhash))},
		{"Service fees", fmt.Sprintf("%d%% of gas revenue", s.ServiceFeeShareOfGasPercent)},
		{"Liquidity released", fmt.Sprintf("%d%% of remainder per year", s.LiquidityReleasePercentPerYear)},
		{"Treasury spend", fmt.Sprintf("%d%% of remainder per year", s.TreasurySpendPercentPerYear)},
		{"Dev grants spend", fmt.Sprintf("%d%% of remainder per year", s.DevGrantsSpendPercentPerYear)},
	}
}

// ProtocolConstants renders the values imported from the chain, so a reader
// can confirm the simulator is using the real ones.
func ProtocolConstants() [][2]string {
	return [][2]string{
		{"Max supply", fmt.Sprintf("%s HASH", formatCount(hgparams.MaxSupplyHash))},
		{"Base denom", hgparams.BaseCoinDenom},
		{"Founder allocation", fmt.Sprintf("%s HASH (%s unlocked, %s vested over %d years)",
			formatCount(hgparams.AllocFounderHash),
			formatCount(hgparams.FounderUnlockedAtGenesisHash),
			formatCount(hgparams.FounderVestedHash),
			hgparams.FounderVestingYears)},
		{"Founder vesting periods", fmt.Sprintf("%d monthly periods", hgparams.FounderVestingPeriods)},
		{"Founder revenue share", fmt.Sprintf("%d bps (%.2f%%), hard ceiling %d bps",
			hgparams.FounderFeeBasisPoints,
			float64(hgparams.FounderFeeBasisPoints)/100,
			hgparams.MaxFounderFeeBasisPoints)},
		{"Service reserve", fmt.Sprintf("%s HASH, finite, never topped up", formatCount(hgparams.AllocServiceReserveHash))},
		{"Welcome pool", fmt.Sprintf("%s HASH (%d/%d/%d HASH tiers)",
			formatCount(hgparams.WelcomePoolHash),
			hgparams.WelcomeTier1Hash, hgparams.WelcomeTier2Hash, hgparams.WelcomeTier3Hash)},
		{"Inflation", "none; x/mint is not wired into the application"},
	}
}
