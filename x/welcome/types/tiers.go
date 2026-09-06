package types

import (
	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// TierAmountHash returns the welcome reward, in whole HASH, for a sequence
// number.
//
// The schedule is fixed by the published tokenomics (§19):
//
//	        1 ..    10,000   ->  50 HASH
//	   10,001 ..   100,000   ->   5 HASH
//	  100,001 .. 1,000,000   ->   1 HASH
//	1,000,001 and beyond     ->   0 HASH
//
// It lives in compile-time constants rather than in governance parameters. A
// chain that could quietly re-tier its own published schedule would not have
// published anything meaningful, so changing these numbers requires a new
// binary the validator set adopts.
//
// Sequence 0 is not a valid position and returns 0.
func TierAmountHash(sequence uint64) int64 {
	switch {
	case sequence == 0:
		return 0
	case sequence <= hgparams.WelcomeTier1Limit:
		return hgparams.WelcomeTier1Hash
	case sequence <= hgparams.WelcomeTier2Limit:
		return hgparams.WelcomeTier2Hash
	case sequence <= hgparams.WelcomeTier3Limit:
		return hgparams.WelcomeTier3Hash
	default:
		return 0
	}
}

// TierAmount returns the welcome reward for a sequence number as coins.
func TierAmount(sequence uint64) sdk.Coins {
	h := TierAmountHash(sequence)
	if h <= 0 {
		return sdk.NewCoins()
	}
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(h)))
}

// IsExhausted reports whether a sequence number is past the last funded tier.
func IsExhausted(sequence uint64) bool {
	return sequence == 0 || sequence > hgparams.WelcomeTier3Limit
}

// Tiers returns the full schedule, in ascending order of sequence.
func Tiers() []Tier {
	return []Tier{
		{
			MaxSequence: hgparams.WelcomeTier1Limit,
			Amount:      sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(hgparams.WelcomeTier1Hash))),
		},
		{
			MaxSequence: hgparams.WelcomeTier2Limit,
			Amount:      sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(hgparams.WelcomeTier2Hash))),
		},
		{
			MaxSequence: hgparams.WelcomeTier3Limit,
			Amount:      sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(hgparams.WelcomeTier3Hash))),
		},
	}
}

// MaxPool returns the worst-case total payout of the schedule, which is the
// amount the welcome pool is funded with at genesis.
//
// It is computed from the schedule rather than restated, so the pool and the
// tiers cannot drift apart.
func MaxPool() sdk.Coins {
	var total int64
	prev := uint64(0)
	for _, t := range Tiers() {
		count := int64(t.MaxSequence - prev)
		perClaim := t.Amount.AmountOf(hgparams.BaseCoinDenom)
		total += count * perClaim.Quo(math.NewInt(hgparams.MicroUnit)).Int64()
		prev = t.MaxSequence
	}
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(total)))
}
