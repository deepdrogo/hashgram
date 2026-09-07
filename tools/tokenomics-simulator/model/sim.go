package model

import (
	"fmt"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	foundertypes "github.com/hashgram/hashgram/x/founder/types"
	serviceprooftypes "github.com/hashgram/hashgram/x/serviceproof/types"
)

// YearResult is one row of the simulation output.
type YearResult struct {
	Year int

	// Population
	NewUsers        int64
	CumulativeUsers int64
	ActiveUsers     int64
	Providers       int64

	// Flows during the year, all in uhash
	FounderVested     math.Int
	WelcomePaid       math.Int
	ServiceEmission   math.Int
	EmissionForfeited math.Int
	GasRevenue        math.Int
	ServiceFeeRevenue math.Int
	FounderRevenue    math.Int
	ValidatorRevenue  math.Int
	LiquidityReleased math.Int
	TreasurySpent     math.Int
	DevGrantsSpent    math.Int

	// Cumulative counters
	WelcomeClaims            int64
	CumulativeFounderRevenue math.Int
	CumulativeEmission       math.Int

	// End-of-year bucket balances
	Balances map[string]math.Int

	// Derived
	Circulating      math.Int
	ReserveRemaining math.Int
	ReservePercent   string
}

// Result is a completed simulation.
type Result struct {
	Scenario Scenario
	Years    []YearResult

	// FinalLedger is the ledger at the end of the run.
	FinalLedger *Ledger

	// Notes records things worth stating that the table cannot express:
	// exhaustion events, binding constraints, capped payouts.
	Notes []string

	// ServiceParams is the reward configuration used, imported from the
	// chain's DefaultParams.
	ServiceParams serviceprooftypes.Params

	// FounderParams is the revenue configuration used.
	FounderParams foundertypes.Params
}

// Run executes a scenario.
//
// The loop runs per epoch, not per year, because the emission schedule is
// defined per epoch and compounds: taking a yearly shortcut would change the
// numbers. Everything that touches supply goes through the Ledger, and the
// invariant is asserted after every epoch, so any modelling mistake surfaces
// immediately instead of as a plausible-looking wrong total.
func Run(s Scenario) (*Result, error) {
	if err := s.Validate(); err != nil {
		return nil, err
	}

	ledger, err := NewGenesisLedger()
	if err != nil {
		return nil, err
	}

	// Reward and revenue configuration come from the chain's own defaults.
	serviceParams := serviceprooftypes.DefaultParams()
	founderParams := foundertypes.DefaultParams()

	res := &Result{
		Scenario:      s,
		FinalLedger:   ledger,
		ServiceParams: serviceParams,
		FounderParams: founderParams,
	}

	// Founder vesting: FounderVestedHash released across FounderVestingPeriods
	// monthly periods. Integer division leaves a remainder, which the chain
	// puts in the final period; the same is done here.
	vestPerPeriod := hgparams.HashToBase(hgparams.FounderVestedHash).
		Quo(math.NewInt(hgparams.FounderVestingPeriods))
	vestRemainder := hgparams.HashToBase(hgparams.FounderVestedHash).
		Sub(vestPerPeriod.Mul(math.NewInt(hgparams.FounderVestingPeriods)))
	//
	// Period boundaries are derived from the epoch's position within the whole
	// schedule rather than from a fixed epochs-per-month figure. 365 does not
	// divide by 12, so a fixed 30-epoch month drifts five epochs a year and
	// pushes the last periods past the eight-year mark, which showed up as an
	// uneven final two years in the report.
	vestingEpochs := int64(hgparams.FounderVestingYears) * EpochsPerYear
	vestPeriodsDone := int64(0)

	var (
		welcomeSequence  uint64
		cumulativeUsers  int64
		welcomeClaims    int64
		cumFounderRev    = math.ZeroInt()
		cumEmission      = math.ZeroInt()
		reserveEmptyYear = 0
		welcomeEmptyYear = 0
		saturationYear   = 0
		capBindingYears  []int
	)

	totalEpochs := s.Years * EpochsPerYear

	for year := 1; year <= s.Years; year++ {
		newUsers := s.UsersJoiningInYear(year)
		providers := s.ProvidersInYear(year)

		// Apply the user ceiling. Compounding growth over a long horizon
		// otherwise produces populations larger than Earth's, and the fee
		// revenue derived from them would be arithmetic rather than analysis.
		if s.MaxUsers > 0 {
			room := s.MaxUsers - cumulativeUsers
			if room < 0 {
				room = 0
			}
			if newUsers > room {
				newUsers = room
				if saturationYear == 0 && room == 0 {
					saturationYear = year
				}
			}
		}

		yr := YearResult{
			Year:              year,
			NewUsers:          newUsers,
			Providers:         providers,
			FounderVested:     math.ZeroInt(),
			WelcomePaid:       math.ZeroInt(),
			ServiceEmission:   math.ZeroInt(),
			EmissionForfeited: math.ZeroInt(),
			GasRevenue:        math.ZeroInt(),
			ServiceFeeRevenue: math.ZeroInt(),
			FounderRevenue:    math.ZeroInt(),
			ValidatorRevenue:  math.ZeroInt(),
			LiquidityReleased: math.ZeroInt(),
			TreasurySpent:     math.ZeroInt(),
			DevGrantsSpent:    math.ZeroInt(),
		}

		// Annual, governance-driven flows happen once at the start of the
		// year: liquidity into circulation, treasury and grant spending.
		if released, err := releasePercent(ledger, BucketLiquidity, BucketUsers, s.LiquidityReleasePercentPerYear); err != nil {
			return nil, fmt.Errorf("year %d liquidity release: %w", year, err)
		} else {
			yr.LiquidityReleased = released
		}
		if spent, err := releasePercent(ledger, BucketTreasury, BucketUsers, s.TreasurySpendPercentPerYear); err != nil {
			return nil, fmt.Errorf("year %d treasury spend: %w", year, err)
		} else {
			yr.TreasurySpent = spent
		}
		if spent, err := releasePercent(ledger, BucketDevGrants, BucketUsers, s.DevGrantsSpendPercentPerYear); err != nil {
			return nil, fmt.Errorf("year %d dev grants spend: %w", year, err)
		} else {
			yr.DevGrantsSpent = spent
		}

		// Users joining is spread evenly across the year's epochs. The
		// remainder is added to the final epoch so the yearly total is exact.
		joinPerEpoch := newUsers / EpochsPerYear
		joinRemainder := newUsers - joinPerEpoch*EpochsPerYear

		for e := 0; e < EpochsPerYear; e++ {
			epochIndex := (year-1)*EpochsPerYear + e

			// --- Founder vesting ---------------------------------------
			// How many monthly periods should have vested by the end of this
			// epoch, computed from the epoch's position in the schedule so
			// that period 96 lands exactly at the eight-year mark.
			periodsDue := int64(hgparams.FounderVestingPeriods)
			if int64(epochIndex+1) < vestingEpochs {
				periodsDue = int64(epochIndex+1) * hgparams.FounderVestingPeriods / vestingEpochs
			}

			for vestPeriodsDone < periodsDue {
				amount := vestPerPeriod
				vestPeriodsDone++
				if vestPeriodsDone == hgparams.FounderVestingPeriods {
					// Integer division leaves a remainder; the chain's
					// PeriodicVestingAccount puts it in the final period.
					amount = amount.Add(vestRemainder)
				}
				moved, err := ledger.MoveUpTo(BucketFounderLocked, BucketFounderLiquid, amount)
				if err != nil {
					return nil, fmt.Errorf("epoch %d founder vesting: %w", epochIndex, err)
				}
				yr.FounderVested = yr.FounderVested.Add(moved)
			}

			// --- Welcome rewards ---------------------------------------
			joining := joinPerEpoch
			if e == EpochsPerYear-1 {
				joining += joinRemainder
			}
			if joining > 0 {
				paid, funded, err := payWelcomeBatch(ledger, welcomeSequence, joining)
				if err != nil {
					return nil, fmt.Errorf("epoch %d welcome payout: %w", epochIndex, err)
				}
				yr.WelcomePaid = yr.WelcomePaid.Add(paid)
				welcomeClaims += funded

				cumulativeUsers += joining
				welcomeSequence += uint64(joining)
			}

			// --- Useful-service emission -------------------------------
			// The chain's EmissionForEpoch, given the reserve that remains.
			budget := serviceprooftypes.EmissionForEpoch(ledger.ServiceReserveCoins(), serviceParams)
			budgetAmt := budget.AmountOf(hgparams.BaseCoinDenom)

			if budgetAmt.IsPositive() {
				// A budget is a ceiling, not a mandatory payout. The
				// per-provider cap means a small network cannot draw the
				// whole budget however much work it does, and the undrawn
				// remainder stays in the reserve.
				perProvider := serviceprooftypes.ProviderRewardCap(budget, serviceParams).
					AmountOf(hgparams.BaseCoinDenom)
				drawable := perProvider.Mul(math.NewInt(providers))

				payout := budgetAmt
				if drawable.LT(payout) {
					payout = drawable
					if len(capBindingYears) == 0 || capBindingYears[len(capBindingYears)-1] != year {
						capBindingYears = append(capBindingYears, year)
					}
				}

				moved, err := ledger.MoveUpTo(BucketServiceReserve, BucketProviders, payout)
				if err != nil {
					return nil, fmt.Errorf("epoch %d service emission: %w", epochIndex, err)
				}
				yr.ServiceEmission = yr.ServiceEmission.Add(moved)
				yr.EmissionForfeited = yr.EmissionForfeited.Add(budgetAmt.Sub(moved))
				cumEmission = cumEmission.Add(moved)
			}

			// --- Protocol revenue and the Founder share ----------------
			activeUsers := cumulativeUsers * s.ActiveUserPercent / 100
			gas := gasForEpoch(activeUsers, s.TxPerUserPerDay, s.AvgTxFeeUhash)
			serviceFees := gas.Mul(math.NewInt(s.ServiceFeeShareOfGasPercent)).Quo(math.NewInt(100))
			qualifying := gas.Add(serviceFees)

			if qualifying.IsPositive() {
				// Users can only pay what they hold. Charging more would
				// model a chain that mints fees out of nothing.
				collected, err := ledger.MoveUpTo(BucketUsers, BucketValidators, qualifying)
				if err != nil {
					return nil, fmt.Errorf("epoch %d fee collection: %w", epochIndex, err)
				}

				if collected.IsPositive() {
					// The chain's own FounderCut on the collected revenue.
					cut := founderParams.FounderCut(
						sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, collected)),
					).AmountOf(hgparams.BaseCoinDenom)

					if cut.IsPositive() {
						if err := ledger.Move(BucketValidators, BucketFounderLiquid, cut); err != nil {
							return nil, fmt.Errorf("epoch %d founder share: %w", epochIndex, err)
						}
					}

					// Attribute the collected total back to gas and service
					// fees in the proportion charged, so the report's two
					// revenue columns sum to what was actually collected.
					collectedGas := collected
					if qualifying.IsPositive() {
						collectedGas = collected.Mul(gas).Quo(qualifying)
					}
					yr.GasRevenue = yr.GasRevenue.Add(collectedGas)
					yr.ServiceFeeRevenue = yr.ServiceFeeRevenue.Add(collected.Sub(collectedGas))
					yr.FounderRevenue = yr.FounderRevenue.Add(cut)
					yr.ValidatorRevenue = yr.ValidatorRevenue.Add(collected.Sub(cut))
					cumFounderRev = cumFounderRev.Add(cut)
				}
			}

			// The assertion that makes the rest of the output trustworthy.
			if err := ledger.AssertInvariant(); err != nil {
				return nil, fmt.Errorf("epoch %d of %d: %w", epochIndex, totalEpochs, err)
			}
		}

		// Exhaustion events, recorded the first year they occur.
		if reserveEmptyYear == 0 && !ledger.Get(BucketServiceReserve).IsPositive() {
			reserveEmptyYear = year
		}
		if welcomeEmptyYear == 0 &&
			ledger.WelcomePaid().GTE(hgparams.HashToBase(hgparams.WelcomePoolHash)) {
			welcomeEmptyYear = year
		}

		yr.CumulativeUsers = cumulativeUsers
		yr.ActiveUsers = cumulativeUsers * s.ActiveUserPercent / 100
		yr.WelcomeClaims = welcomeClaims
		yr.CumulativeFounderRevenue = cumFounderRev
		yr.CumulativeEmission = cumEmission
		yr.Balances = ledger.Snapshot()
		yr.Circulating = circulating(ledger)
		yr.ReserveRemaining = ledger.Get(BucketServiceReserve)
		yr.ReservePercent = percentOf(
			yr.ReserveRemaining,
			hgparams.HashToBase(hgparams.AllocServiceReserveHash),
		)

		res.Years = append(res.Years, yr)
	}

	res.Notes = buildNotes(s, ledger, reserveEmptyYear, welcomeEmptyYear,
		saturationYear, capBindingYears, cumEmission, cumFounderRev)
	return res, nil
}

// gasForEpoch computes one epoch's gas revenue.
//
// The float multiplication is confined to the transaction count, which is an
// adoption assumption. It is converted to an integer before it touches any
// coin amount, so all coin arithmetic stays exact.
func gasForEpoch(activeUsers int64, txPerUserPerDay float64, avgFeeUhash int64) math.Int {
	if activeUsers <= 0 || txPerUserPerDay <= 0 || avgFeeUhash <= 0 {
		return math.ZeroInt()
	}
	txCount := int64(float64(activeUsers) * txPerUserPerDay)
	if txCount <= 0 {
		return math.ZeroInt()
	}
	return math.NewInt(txCount).Mul(math.NewInt(avgFeeUhash))
}

// releasePercent moves a whole-number percentage of a bucket's current
// balance to another bucket.
func releasePercent(l *Ledger, from, to string, percent int64) (math.Int, error) {
	if percent <= 0 {
		return math.ZeroInt(), nil
	}
	amount := l.Get(from).Mul(math.NewInt(percent)).Quo(math.NewInt(100))
	if !amount.IsPositive() {
		return math.ZeroInt(), nil
	}
	return l.MoveUpTo(from, to, amount)
}

// circulating is the supply in the hands of participants rather than sitting
// in a genesis reserve or locked in vesting.
//
// Founder liquid coins are counted as circulating because they are spendable.
// Whether they are actually spent is not something a model can know, and
// excluding them would flatter the number.
func circulating(l *Ledger) math.Int {
	return l.Get(BucketUsers).
		Add(l.Get(BucketProviders)).
		Add(l.Get(BucketValidators)).
		Add(l.Get(BucketFounderLiquid))
}

func percentOf(part, whole math.Int) string {
	if !whole.IsPositive() {
		return "0.00%"
	}
	// Two decimal places via integer arithmetic: part * 10000 / whole.
	scaled := part.Mul(math.NewInt(10_000)).Quo(whole)
	return fmt.Sprintf("%d.%02d%%", scaled.Int64()/100, scaled.Int64()%100)
}

func buildNotes(
	s Scenario,
	l *Ledger,
	reserveEmptyYear, welcomeEmptyYear, saturationYear int,
	capBindingYears []int,
	cumEmission, cumFounderRev math.Int,
) []string {
	var notes []string

	if saturationYear > 0 {
		notes = append(notes, fmt.Sprintf(
			"User growth hit the ceiling of %s in year %d and stopped there. "+
				"Revenue after that year reflects a saturated network rather than "+
				"continued growth.", formatCount(s.MaxUsers), saturationYear))
	}

	// Reserve.
	reserve := l.Get(BucketServiceReserve)
	if reserveEmptyYear > 0 {
		notes = append(notes, fmt.Sprintf(
			"The service reserve reached zero in year %d. After that point, "+
				"useful-service operators are paid from protocol service-fee revenue "+
				"only. No mechanism tops the reserve back up.", reserveEmptyYear))
	} else {
		notes = append(notes, fmt.Sprintf(
			"The service reserve still holds %s HASH (%s of its genesis size) "+
				"after %d years. It declines geometrically and never reaches zero, "+
				"which is the intended shape: there is no cliff at which subsidy stops.",
			formatHashFromBase(reserve),
			percentOf(reserve, hgparams.HashToBase(hgparams.AllocServiceReserveHash)),
			s.Years))
	}

	// Per-provider cap.
	if len(capBindingYears) > 0 {
		last := capBindingYears[len(capBindingYears)-1]
		notes = append(notes, fmt.Sprintf(
			"The per-provider cap of %d bps limited the epoch payout in %d of %d "+
				"years, most recently year %d. With that cap, a network needs at "+
				"least %d providers before the full epoch budget can be drawn. This "+
				"is why node count moves the reserve timeline more than user count does.",
			serviceprooftypes.DefaultMaxProviderShareBps,
			len(capBindingYears), s.Years, last,
			int(serviceprooftypes.BasisPointsMax/serviceprooftypes.DefaultMaxProviderShareBps)))
	} else {
		notes = append(notes, fmt.Sprintf(
			"The per-provider cap never limited payouts: the scenario has enough "+
				"nodes (%d or more) that the full epoch budget was always drawable.",
			s.ProvidersYear1))
	}

	// Welcome pool.
	if welcomeEmptyYear > 0 {
		notes = append(notes, fmt.Sprintf(
			"The Welcome pool of %s HASH was fully paid out in year %d. Users "+
				"joining after that receive no welcome reward, which is the "+
				"schedule's design rather than a failure: the fourth tier is zero.",
			formatCount(hgparams.WelcomePoolHash), welcomeEmptyYear))
	} else {
		spent := l.WelcomePaid()
		notes = append(notes, fmt.Sprintf(
			"The Welcome pool paid out %s HASH of its %s HASH cap. The remainder "+
				"stays in the growth allocation.",
			formatHashFromBase(spent), formatCount(hgparams.WelcomePoolHash)))
	}

	// Founder revenue in context.
	notes = append(notes, fmt.Sprintf(
		"Cumulative Founder revenue share over %d years is %s HASH, against a "+
			"genesis allocation of %s HASH. The share is %d bps of protocol fee "+
			"revenue only; it is never taken from transferred principal, so a "+
			"100 HASH transfer still delivers 100 HASH.",
		s.Years,
		formatHashFromBase(cumFounderRev),
		formatCount(hgparams.AllocFounderHash),
		hgparams.FounderFeeBasisPoints))

	// User balance constraint. If users ran dry, revenue was limited by what
	// they held rather than by the assumed volume, and the reader needs to
	// know that before reading the revenue column.
	if !l.Get(BucketUsers).IsPositive() {
		notes = append(notes, "User balances reached zero. Fee revenue in later years was "+
			"limited by what users actually held, not by the assumed transaction "+
			"volume. Raise the liquidity release rate or lower the fee assumption "+
			"to model a network where users can keep transacting.")
	}

	// Supply.
	notes = append(notes, fmt.Sprintf(
		"Total supply is %s uhash at every epoch of the run, unchanged from "+
			"genesis. Emissions move coins out of the finite reserve; they do not "+
			"create them.", l.Total()))

	return notes
}
