package types

import (
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

const (
	// DefaultMinConfidence is the minimum attestation confidence, in basis
	// points. 5000 bps is 50%: an attestor that is less than half sure must
	// not create free HASH.
	DefaultMinConfidence uint32 = 5_000

	// DefaultMaxAttestationAgeBlocks bounds how far in the future an
	// attestation may be valid. At roughly 4 second blocks, 21600 blocks is
	// about 24 hours. An attestation that never expires is a bearer token
	// that outlives the check behind it.
	DefaultMaxAttestationAgeBlocks int64 = 21_600

	// DefaultEpochBlocks is the window for per-attestor claim caps: about 24
	// hours at 4 second blocks.
	DefaultEpochBlocks uint64 = 21_600
)

// DefaultParams returns the launch configuration.
//
// The attestor set is empty. That means no welcome reward can be claimed
// until governance registers an attestor, which is the correct default: the
// specification is explicit that creating a private key must never by itself
// produce free HASH, and an empty attestor set is the only configuration in
// which that is unconditionally true.
func DefaultParams() Params {
	return Params{
		Enabled:                 true,
		Attestors:               nil,
		MinConfidence:           DefaultMinConfidence,
		MaxAttestationAgeBlocks: DefaultMaxAttestationAgeBlocks,
		EpochBlocks:             DefaultEpochBlocks,
	}
}

// Validate checks the parameter set.
func (p Params) Validate() error {
	if p.MinConfidence > BasisPointsMax {
		return ErrInvalidParams.Wrapf(
			"min_confidence %d exceeds %d basis points", p.MinConfidence, BasisPointsMax)
	}
	if p.MaxAttestationAgeBlocks <= 0 {
		return ErrInvalidParams.Wrap("max_attestation_age_blocks must be positive")
	}
	if p.EpochBlocks == 0 {
		return ErrInvalidParams.Wrap("epoch_blocks must be positive")
	}

	seen := make(map[string]bool, len(p.Attestors))
	for i, at := range p.Attestors {
		if err := at.Validate(); err != nil {
			return ErrInvalidAttestor.Wrapf("attestors[%d]: %v", i, err)
		}
		if seen[at.Address] {
			return ErrInvalidAttestor.Wrapf("attestor %s is registered twice", at.Address)
		}
		seen[at.Address] = true
	}
	return nil
}

// Attestor looks up a registered attestor by address.
func (p Params) Attestor(address string) (Attestor, bool) {
	for _, at := range p.Attestors {
		if at.Address == address {
			return at, true
		}
	}
	return Attestor{}, false
}

// DefaultGenesis returns the x/welcome genesis state.
//
// next_sequence starts at 1, so the first eligible user is user number 1 and
// receives the tier-1 reward of 50 HASH.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params:       DefaultParams(),
		NextSequence: 1,
		Claims:       nil,
		UsedNonces:   nil,
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}
	if gs.NextSequence == 0 {
		return ErrInvalidParams.Wrap(
			"next_sequence must be at least 1; sequence 0 is not a valid position")
	}

	seenSubject := make(map[string]bool, len(gs.Claims))
	seenSequence := make(map[uint64]bool, len(gs.Claims))

	for i, c := range gs.Claims {
		if _, err := sdk.AccAddressFromBech32(c.Subject); err != nil {
			return ErrInvalidAttestation.Wrapf("claims[%d].subject %q: %v", i, c.Subject, err)
		}
		if c.Sequence == 0 {
			return ErrInvalidParams.Wrapf("claims[%d] has sequence 0", i)
		}
		if c.Sequence >= gs.NextSequence {
			return ErrInvalidParams.Wrapf(
				"claims[%d] has sequence %d but next_sequence is %d",
				i, c.Sequence, gs.NextSequence)
		}
		// One reward per address, ever.
		if seenSubject[c.Subject] {
			return ErrAlreadyClaimed.Wrapf("claims contain %s twice", c.Subject)
		}
		seenSubject[c.Subject] = true

		// One address per sequence position.
		if seenSequence[c.Sequence] {
			return ErrInvalidParams.Wrapf("claims contain sequence %d twice", c.Sequence)
		}
		seenSequence[c.Sequence] = true

		// The recorded amount must match what the schedule says for that
		// position. A restored chain must not be able to rewrite history into
		// a more generous one.
		want := TierAmount(c.Sequence)
		if !c.Amount.Equal(want) {
			return ErrInvalidParams.Wrapf(
				"claims[%d] at sequence %d records %s but the schedule pays %s",
				i, c.Sequence, c.Amount, want)
		}
	}

	seenNonce := make(map[string]bool, len(gs.UsedNonces))
	for i, n := range gs.UsedNonces {
		if n.Attestor == "" {
			return ErrInvalidParams.Wrapf("used_nonces[%d] has an empty attestor", i)
		}
		key := fmt.Sprintf("%s/%d", n.Attestor, n.Nonce)
		if seenNonce[key] {
			return ErrNonceReused.Wrapf("used_nonces contains %s/%d twice", n.Attestor, n.Nonce)
		}
		seenNonce[key] = true
	}

	return nil
}
