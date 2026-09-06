package types

import (
	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// BasisPointsMax is 100% expressed in basis points.
const BasisPointsMax uint32 = 10_000

// EmissionForEpoch computes one epoch's reward budget from the reserve that
// remains.
//
//	budget = min( floor(remaining * emission_rate_bps / 10000), max_epoch_emission )
//
// Taking a fraction of what *remains*, rather than a fixed amount per epoch,
// is what makes the schedule both declining and provably bounded:
//
//   - Declining: each epoch's budget is a fixed proportion of a shrinking
//     balance, so budgets fall geometrically. Early epochs are subsidised
//     more heavily, which is the intended shape. A mature network is meant to
//     be funded increasingly by real service-fee revenue.
//
//   - Bounded: the sum of all budgets is a geometric series over the initial
//     reserve and converges to it without reaching it. There is no epoch at
//     which the schedule asks for coins the reserve does not hold, and no
//     cliff at which subsidy stops abruptly.
//
// The absolute cap exists because a proportion of a nearly full reserve is a
// large number: without it, the first epochs of the network would pay out
// disproportionately to whichever handful of providers happened to be online.
//
// Integer arithmetic throughout, truncating toward zero, so every validator
// computes the same budget from the same state.
func EmissionForEpoch(remaining sdk.Coins, p Params) sdk.Coins {
	if remaining.IsZero() || p.EmissionRateBps == 0 {
		return sdk.NewCoins()
	}

	rate := math.NewIntFromUint64(uint64(p.EmissionRateBps))
	denom := math.NewIntFromUint64(uint64(BasisPointsMax))

	out := make(sdk.Coins, 0, len(remaining))
	for _, c := range remaining {
		budget := c.Amount.Mul(rate).Quo(denom)
		if cap := p.MaxEpochEmission.AmountOf(c.Denom); cap.IsPositive() && budget.GT(cap) {
			budget = cap
		}
		if budget.IsPositive() {
			out = append(out, sdk.NewCoin(c.Denom, budget))
		}
	}
	return sdk.NewCoins(out...)
}

// ProjectEmission projects the schedule forward from a starting reserve.
//
// Used by the EmissionSchedule query and by tools/tokenomics-simulator, which
// imports this function rather than reimplementing it. A simulator that
// models a different schedule from the chain is worse than no simulator.
func ProjectEmission(startingReserve sdk.Coins, p Params, epochs int, firstEpoch uint64) []EmissionProjection {
	if epochs <= 0 {
		return nil
	}

	out := make([]EmissionProjection, 0, epochs)
	remaining := startingReserve

	for i := 0; i < epochs; i++ {
		budget := EmissionForEpoch(remaining, p)
		after, negative := remaining.SafeSub(budget...)
		if negative {
			// Unreachable: budget is a truncated fraction of remaining and is
			// therefore never greater than it. Guarded so that a future
			// change to the formula surfaces here instead of underflowing.
			break
		}
		out = append(out, EmissionProjection{
			Epoch:        firstEpoch + uint64(i),
			Budget:       budget,
			ReserveAfter: after,
		})
		remaining = after
		if remaining.IsZero() {
			break
		}
	}
	return out
}

// ProviderRewardCap returns the most a single provider may receive from one
// epoch's budget.
//
//	cap = floor(budget * max_provider_share_bps / 10000)
//
// Without a cap, one large operator could take the entire subsidy, and the
// reward system would be funding centralisation with the community's
// allocation.
func ProviderRewardCap(budget sdk.Coins, p Params) sdk.Coins {
	if p.MaxProviderShareBps == 0 || p.MaxProviderShareBps >= BasisPointsMax {
		return budget
	}

	share := math.NewIntFromUint64(uint64(p.MaxProviderShareBps))
	denom := math.NewIntFromUint64(uint64(BasisPointsMax))

	out := make(sdk.Coins, 0, len(budget))
	for _, c := range budget {
		amt := c.Amount.Mul(share).Quo(denom)
		if amt.IsPositive() {
			out = append(out, sdk.NewCoin(c.Denom, amt))
		}
	}
	return sdk.NewCoins(out...)
}

// ProRata computes budget * credit / totalCredit with integer arithmetic.
//
// Truncation means the sum of all providers' shares is at most the budget,
// never more. The rounding dust stays in the reserve and funds later epochs
// rather than being created out of nothing to make the numbers look tidy.
func ProRata(budget sdk.Coins, credit, totalCredit math.Int) sdk.Coins {
	if !credit.IsPositive() || !totalCredit.IsPositive() {
		return sdk.NewCoins()
	}
	if credit.GT(totalCredit) {
		credit = totalCredit
	}

	out := make(sdk.Coins, 0, len(budget))
	for _, c := range budget {
		amt := c.Amount.Mul(credit).Quo(totalCredit)
		if amt.IsPositive() {
			out = append(out, sdk.NewCoin(c.Denom, amt))
		}
	}
	return sdk.NewCoins(out...)
}

// InitialReserve returns the genesis size of the useful-service reserve:
// 500,000,000 HASH, half of the canonical supply.
func InitialReserve() sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(
		hgparams.BaseCoinDenom,
		hgparams.HashToBase(hgparams.AllocServiceReserveHash),
	))
}
