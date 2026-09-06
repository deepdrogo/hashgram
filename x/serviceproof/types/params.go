package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// Defaults for the useful-service reward system.
//
// The numbers below are launch defaults, adjustable by governance within the
// bounds Validate enforces. The reasoning behind each is stated because a
// bare constant tells a future maintainer nothing about which way it is safe
// to move.
const (
	// DefaultEpochBlocks is roughly 24 hours at 4 second blocks.
	//
	// Long enough that settlement cost is amortised over many receipts and
	// that a brief outage does not zero a provider's day; short enough that
	// an operator sees earnings appear on a human timescale.
	DefaultEpochBlocks uint64 = 21_600

	// DefaultEmissionRateBps releases 0.05% of the remaining reserve per
	// epoch. With daily epochs that is about 16.6% of the remaining reserve
	// per year, which spends roughly half the reserve over four years and
	// asymptotes thereafter.
	DefaultEmissionRateBps uint32 = 5

	// DefaultMaxProviderShareBps caps one provider at 5% of an epoch's
	// budget. A network needs at least twenty providers before the cap stops
	// binding, which is a deliberate pressure toward many operators.
	DefaultMaxProviderShareBps uint32 = 500

	// DefaultMaxClientConcentrationBps caps the share of a provider's credit
	// that may come from one counterparty at 20%.
	//
	// This is the number that makes two nodes trading fake traffic
	// unprofitable: a ring of two gets at most 20% of the credit its raw
	// receipts claim, while an honest relay serving many clients is
	// unaffected. It is the single most load-bearing anti-abuse parameter in
	// the module.
	DefaultMaxClientConcentrationBps uint32 = 2_000

	// DefaultChallengesPerEpoch is how many storage challenges a provider
	// with assignments receives per epoch.
	DefaultChallengesPerEpoch uint32 = 4

	// DefaultChallengeResponseBlocks is about 20 minutes at 4 second blocks:
	// enough for a node to read a chunk from disk and broadcast, not enough
	// to fetch the data from somewhere else on demand.
	DefaultChallengeResponseBlocks int64 = 300

	// DefaultFraudScoreJailThreshold jails a provider at 100 points. A failed
	// challenge is 25 points, so four failures inside the decay window jail.
	DefaultFraudScoreJailThreshold uint32 = 100

	// DefaultFraudScoreDecayPerEpoch forgives 10 points per epoch, so one bad
	// day does not permanently condemn an otherwise honest operator.
	DefaultFraudScoreDecayPerEpoch uint32 = 10

	// DefaultSlashFractionBps takes 5% of bond when a provider is jailed.
	DefaultSlashFractionBps uint32 = 500

	// DefaultJailDurationBlocks is about 24 hours.
	DefaultJailDurationBlocks int64 = 21_600

	// DefaultUnbondingBlocks is about 21 days.
	//
	// It must outlast the window in which fraud could still surface,
	// otherwise an operator could cheat, start unbonding, and withdraw before
	// the evidence lands.
	DefaultUnbondingBlocks int64 = 453_600

	// DefaultReceiptMaxAgeBlocks is about 48 hours.
	DefaultReceiptMaxAgeBlocks int64 = 43_200

	// DefaultMaxReceiptUnits caps a single receipt at 64 GiB of units, so one
	// absurd receipt cannot dominate an epoch before the concentration cap is
	// even reached.
	DefaultMaxReceiptUnits uint64 = 64 * 1024 * 1024 * 1024

	// Credit conversion. These set the relative value of the roles.
	// Storage is per GiB held for a full epoch; relay and retrieval are per
	// GiB moved; calls are per hour of session.
	DefaultStorageCreditPerGiBEpoch uint64 = 100
	DefaultRelayCreditPerGiB        uint64 = 200
	DefaultRetrievalCreditPerGiB    uint64 = 150
	DefaultCallCreditPerHour        uint64 = 300

	// DefaultMinBondHash is the stake required to register, in whole HASH.
	DefaultMinBondHash int64 = 1_000

	// DefaultMaxEpochEmissionHash caps one epoch's budget in whole HASH.
	//
	// Set to exactly the geometric schedule's own first-epoch budget
	// (500,000,000 * 5 / 10,000 = 250,000 HASH). Since the reserve only
	// shrinks, the cap therefore binds at genesis and never afterwards: it is
	// a backstop against a future parameter mistake rather than a shape that
	// overrides the schedule.
	//
	// Setting it materially lower would flatten the early curve into a
	// straight line and defeat the declining-emission design; the earlier
	// draft of this file used 50,000 and a test caught it, because a flat
	// 50,000 per epoch spends only 40% of the reserve in eleven years.
	//
	// Note that the budget is a ceiling, not a mandatory payout. Unclaimed
	// budget stays in the reserve, and the per-provider cap means a network
	// with only a handful of providers cannot draw the whole budget anyway.
	// That is what actually protects the early network from overpaying.
	DefaultMaxEpochEmissionHash int64 = 250_000
)

// DefaultParams returns the launch configuration.
func DefaultParams() Params {
	return Params{
		Enabled:         true,
		EpochBlocks:     DefaultEpochBlocks,
		EmissionRateBps: DefaultEmissionRateBps,
		MaxEpochEmission: sdk.NewCoins(sdk.NewCoin(
			hgparams.BaseCoinDenom, hgparams.HashToBase(DefaultMaxEpochEmissionHash))),
		MinBond: sdk.NewCoins(sdk.NewCoin(
			hgparams.BaseCoinDenom, hgparams.HashToBase(DefaultMinBondHash))),
		MaxProviderShareBps:       DefaultMaxProviderShareBps,
		MaxClientConcentrationBps: DefaultMaxClientConcentrationBps,
		ChallengesPerEpoch:        DefaultChallengesPerEpoch,
		ChallengeResponseBlocks:   DefaultChallengeResponseBlocks,
		FraudScoreJailThreshold:   DefaultFraudScoreJailThreshold,
		FraudScoreDecayPerEpoch:   DefaultFraudScoreDecayPerEpoch,
		SlashFractionBps:          DefaultSlashFractionBps,
		JailDurationBlocks:        DefaultJailDurationBlocks,
		UnbondingBlocks:           DefaultUnbondingBlocks,
		ReceiptMaxAgeBlocks:       DefaultReceiptMaxAgeBlocks,
		MaxReceiptUnits:           DefaultMaxReceiptUnits,

		StorageCreditPerGibEpoch: DefaultStorageCreditPerGiBEpoch,
		RelayCreditPerGib:        DefaultRelayCreditPerGiB,
		RetrievalCreditPerGib:    DefaultRetrievalCreditPerGiB,
		CallCreditPerHour:        DefaultCallCreditPerHour,
	}
}

// FraudScoreForChallengeFailure is how much a failed or missed challenge adds
// to a provider's fraud score.
const FraudScoreForChallengeFailure uint32 = 25

// FraudScoreForInvalidEvidence is how much a forged or replayed receipt adds.
//
// Higher than a challenge failure, because a failed challenge can be an
// honest disk fault while a forged signature cannot.
const FraudScoreForInvalidEvidence uint32 = 50

// Validate checks the parameter set.
//
// The bounds here are the ones that must hold for the anti-abuse story to
// mean anything. Governance can tune within them; it cannot turn the
// protections off.
func (p Params) Validate() error {
	if p.EpochBlocks == 0 {
		return ErrInvalidParams.Wrap("epoch_blocks must be positive")
	}
	if p.EmissionRateBps > BasisPointsMax {
		return ErrInvalidParams.Wrapf(
			"emission_rate_bps %d exceeds %d", p.EmissionRateBps, BasisPointsMax)
	}
	if !p.MaxEpochEmission.IsValid() || p.MaxEpochEmission.IsAnyNegative() {
		return ErrInvalidParams.Wrapf("max_epoch_emission %q is invalid", p.MaxEpochEmission)
	}
	if !p.MinBond.IsValid() || p.MinBond.IsAnyNegative() {
		return ErrInvalidParams.Wrapf("min_bond %q is invalid", p.MinBond)
	}
	if p.MinBond.IsZero() {
		return ErrInvalidParams.Wrap(
			"min_bond must be positive; without a bond, the cost of being caught " +
				"cheating is the cost of generating a new keypair")
	}

	if p.MaxProviderShareBps == 0 || p.MaxProviderShareBps > BasisPointsMax {
		return ErrInvalidParams.Wrapf(
			"max_provider_share_bps must be in (0, %d]; got %d", BasisPointsMax, p.MaxProviderShareBps)
	}
	if p.MaxClientConcentrationBps == 0 || p.MaxClientConcentrationBps > BasisPointsMax {
		return ErrInvalidParams.Wrapf(
			"max_client_concentration_bps must be in (0, %d]; got %d. This is the "+
				"parameter that makes two nodes trading fake traffic unprofitable and "+
				"cannot be disabled", BasisPointsMax, p.MaxClientConcentrationBps)
	}

	if p.ChallengesPerEpoch == 0 {
		return ErrInvalidParams.Wrap(
			"challenges_per_epoch must be positive; unchallenged storage is self-reported storage")
	}
	if p.ChallengeResponseBlocks <= 0 {
		return ErrInvalidParams.Wrap("challenge_response_blocks must be positive")
	}
	if p.FraudScoreJailThreshold == 0 {
		return ErrInvalidParams.Wrap("fraud_score_jail_threshold must be positive")
	}
	if p.SlashFractionBps > BasisPointsMax {
		return ErrInvalidParams.Wrapf("slash_fraction_bps %d exceeds %d", p.SlashFractionBps, BasisPointsMax)
	}
	if p.JailDurationBlocks < 0 {
		return ErrInvalidParams.Wrap("jail_duration_blocks must not be negative")
	}
	if p.UnbondingBlocks <= 0 {
		return ErrInvalidParams.Wrap(
			"unbonding_blocks must be positive, and should exceed the window in which " +
				"fraud can still be discovered")
	}
	if p.ReceiptMaxAgeBlocks <= 0 {
		return ErrInvalidParams.Wrap("receipt_max_age_blocks must be positive")
	}
	if p.MaxReceiptUnits == 0 {
		return ErrInvalidParams.Wrap("max_receipt_units must be positive")
	}
	return nil
}
