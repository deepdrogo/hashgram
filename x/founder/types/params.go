package types

import (
	"fmt"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// DefaultPayoutPeriodBlocks batches automatic payouts.
//
// At the Hashgram target of roughly 4 second blocks, 7200 blocks is about 8
// hours. Batching matters because the beneficiary is expected to be a cold
// wallet or multisig: one dust transfer per block would be both wasteful and
// hostile to whoever has to reconcile it.
const DefaultPayoutPeriodBlocks uint64 = 7200

// DefaultMinPayoutHash is the accrual threshold below which an automatic
// payout is skipped, in whole HASH.
const DefaultMinPayoutHash int64 = 1

// DefaultParams returns the launch configuration with no beneficiary set.
//
// The beneficiary is empty on purpose. There is no plausible default: it must
// be a PUBLIC address the Founder generated on a machine that is not this
// server. `hashgramctl init-mainnet-genesis` requires it as an explicit
// input and refuses to invent one.
func DefaultParams() Params {
	return Params{
		Beneficiary:        "",
		FeeBasisPoints:     hgparams.FounderFeeBasisPoints,
		PayoutPeriodBlocks: DefaultPayoutPeriodBlocks,
		MinPayout:          sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, DefaultMinPayoutBase())),
	}
}

// DefaultMinPayoutBase is DefaultMinPayoutHash expressed in uhash.
func DefaultMinPayoutBase() math.Int {
	return hgparams.HashToBase(DefaultMinPayoutHash)
}

// Validate checks the parameter set.
//
// The fee ceiling check is the important one: it is what makes
// "governance quietly raises the Founder share" impossible. Governance may
// lower fee_basis_points freely, but raising it past
// hgparams.MaxFounderFeeBasisPoints requires a binary the validator set has
// consciously adopted.
func (p Params) Validate() error {
	// An empty beneficiary is tolerated so that a devnet can run without one
	// and so that genesis validation is not order-dependent. Accrual is
	// skipped while it is empty (see keeper.Accrue), so no funds are lost or
	// misdirected.
	if p.Beneficiary != "" {
		if _, err := sdk.AccAddressFromBech32(p.Beneficiary); err != nil {
			return ErrInvalidBeneficiary.Wrapf("%q: %v", p.Beneficiary, err)
		}
	}

	if p.FeeBasisPoints > hgparams.MaxFounderFeeBasisPoints {
		return ErrFeeExceedsCeiling.Wrapf(
			"fee_basis_points %d exceeds the ceiling of %d bps (%.2f%%) compiled into this binary",
			p.FeeBasisPoints, hgparams.MaxFounderFeeBasisPoints,
			float64(hgparams.MaxFounderFeeBasisPoints)/100)
	}

	if !p.MinPayout.IsValid() {
		return ErrInvalidMinPayout.Wrapf("min_payout %q is not a valid coin set", p.MinPayout)
	}
	if p.MinPayout.IsAnyNegative() {
		return ErrInvalidMinPayout.Wrap("min_payout must not be negative")
	}

	return nil
}

// FeePercent renders the fee as a human-readable percentage, e.g. "1.00%".
//
// Formatted from the integer basis points rather than through a decimal type:
// a LegacyDec renders eighteen decimal places, which turns 1% into
// "1.000000000000000000%" in operator output and in the launch runbook.
func (p Params) FeePercent() string {
	return fmt.Sprintf("%d.%02d%%", p.FeeBasisPoints/100, p.FeeBasisPoints%100)
}

// FounderCut computes the Founder share of an amount using integer arithmetic.
//
//	cut = floor(amount * fee_basis_points / 10000)
//
// Truncation is toward zero, which means any remainder stays with validators
// and delegators rather than with the Founder. That direction is deliberate:
// rounding errors should not accumulate in favour of the party who wrote the
// code.
//
// Worked example from the specification. Qualifying protocol revenue of
// 100 HASH is 100_000_000 uhash:
//
//	100_000_000 * 100 / 10_000 = 1_000_000 uhash = exactly 1 HASH
func (p Params) FounderCut(amount sdk.Coins) sdk.Coins {
	if p.FeeBasisPoints == 0 || amount.IsZero() {
		return sdk.NewCoins()
	}

	bps := math.NewIntFromUint64(uint64(p.FeeBasisPoints))
	denom := math.NewIntFromUint64(uint64(hgparams.BasisPointDenominator))

	out := make(sdk.Coins, 0, len(amount))
	for _, c := range amount {
		cut := c.Amount.Mul(bps).Quo(denom)
		if cut.IsPositive() {
			out = append(out, sdk.NewCoin(c.Denom, cut))
		}
	}
	// NewCoins sorts and drops zeroes, which keeps the result canonical.
	return sdk.NewCoins(out...)
}
