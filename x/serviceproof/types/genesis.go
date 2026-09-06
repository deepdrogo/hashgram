package types

import (
	"fmt"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

// DefaultGenesis returns the x/serviceproof genesis state.
//
// The assigner set is empty. Until governance registers one, no storage can
// be assigned, so no storage rewards can be earned. This is the correct
// default: a provider assigning work to itself would be self-reporting by
// another name, which is exactly what this module exists to prevent.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params: DefaultParams(),
		Reserve: ReserveState{
			Initial:      InitialReserve(),
			TotalEmitted: sdk.NewCoins(),
			TotalSlashed: sdk.NewCoins(),
		},
		Assigners:        nil,
		CurrentEpoch:     0,
		EpochStartHeight: 0,
		NextChallengeId:  1,
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}
	if err := gs.Reserve.Validate(); err != nil {
		return err
	}

	for i, a := range gs.Assigners {
		if _, err := sdk.AccAddressFromBech32(a); err != nil {
			return ErrInvalidParams.Wrapf("assigners[%d] %q: %v", i, a, err)
		}
	}

	seenProvider := make(map[string]bool, len(gs.Providers))
	for i, p := range gs.Providers {
		if _, err := sdk.AccAddressFromBech32(p.Operator); err != nil {
			return ErrProviderNotFound.Wrapf("providers[%d].operator %q: %v", i, p.Operator, err)
		}
		if _, err := sdk.AccAddressFromBech32(p.RewardAddress); err != nil {
			return ErrProviderNotFound.Wrapf("providers[%d].reward_address %q: %v", i, p.RewardAddress, err)
		}
		if seenProvider[p.Operator] {
			return ErrProviderExists.Wrapf("providers contains %s twice", p.Operator)
		}
		seenProvider[p.Operator] = true

		if len(p.Roles) == 0 {
			return ErrInvalidRole.Wrapf("providers[%d] offers no roles", i)
		}
		for _, r := range p.Roles {
			if err := ValidateRole(r); err != nil {
				return ErrInvalidRole.Wrapf("providers[%d]: %v", i, err)
			}
		}
		if !p.Bond.IsValid() || p.Bond.IsAnyNegative() {
			return ErrInsufficientBond.Wrapf("providers[%d].bond %q is invalid", i, p.Bond)
		}
	}

	seenAssignment := make(map[string]bool, len(gs.Assignments))
	for i, a := range gs.Assignments {
		if err := a.Validate(); err != nil {
			return ErrInvalidAssignment.Wrapf("assignments[%d]: %v", i, err)
		}
		key := assignmentDedupKey(a.BlobId, a.Provider, a.ReplicaIndex)
		if seenAssignment[key] {
			return ErrAssignmentExists.Wrapf("assignments contains %s twice", key)
		}
		seenAssignment[key] = true
	}

	for i, e := range gs.Epochs {
		if e.TotalCredit.IsNil() || e.TotalCredit.IsNegative() {
			return ErrInvalidParams.Wrapf("epochs[%d].total_credit is invalid", i)
		}
		if !e.Budget.IsValid() || !e.Distributed.IsValid() {
			return ErrInvalidParams.Wrapf("epochs[%d] has invalid coin amounts", i)
		}
		// An epoch cannot have distributed more than it was budgeted.
		if !e.Budget.IsAllGTE(e.Distributed) {
			return ErrReserveOverspend.Wrapf(
				"epochs[%d] distributed %s from a budget of %s", i, e.Distributed, e.Budget)
		}
	}

	for i, c := range gs.Credits {
		if _, err := sdk.AccAddressFromBech32(c.Operator); err != nil {
			return ErrProviderNotFound.Wrapf("credits[%d].operator %q: %v", i, c.Operator, err)
		}
		for name, v := range map[string]math.Int{
			"storage_credit":   c.StorageCredit,
			"relay_credit":     c.RelayCredit,
			"retrieval_credit": c.RetrievalCredit,
			"call_credit":      c.CallCredit,
		} {
			if v.IsNil() || v.IsNegative() {
				return ErrInvalidParams.Wrapf("credits[%d].%s is invalid", i, name)
			}
		}
	}

	if gs.NextChallengeId == 0 {
		return ErrInvalidParams.Wrap("next_challenge_id must be at least 1")
	}

	return nil
}

// Validate checks reserve accounting.
func (r ReserveState) Validate() error {
	for name, c := range map[string]sdk.Coins{
		"initial":       r.Initial,
		"total_emitted": r.TotalEmitted,
		"total_slashed": r.TotalSlashed,
	} {
		if !c.IsValid() || c.IsAnyNegative() {
			return ErrInvalidParams.Wrapf("reserve.%s %q is invalid", name, c)
		}
	}

	// The reserve can never have emitted more than it started with plus what
	// was slashed back into it. This is the finite-reserve guarantee, checked
	// at genesis as well as at every settlement.
	ceiling := r.Initial.Add(r.TotalSlashed...)
	if !ceiling.IsAllGTE(r.TotalEmitted) {
		return ErrReserveOverspend.Wrapf(
			"total_emitted %s exceeds initial %s plus slashed %s",
			r.TotalEmitted, r.Initial, r.TotalSlashed)
	}
	return nil
}

// Validate checks a storage assignment.
func (a StorageAssignment) Validate() error {
	if len(a.BlobId) == 0 {
		return ErrInvalidAssignment.Wrap("blob_id must not be empty")
	}
	if len(a.BlobId) > 64 {
		return ErrInvalidAssignment.Wrapf("blob_id is %d bytes, maximum is 64", len(a.BlobId))
	}
	if _, err := sdk.AccAddressFromBech32(a.Provider); err != nil {
		return ErrInvalidAssignment.Wrapf("provider %q: %v", a.Provider, err)
	}
	if a.SizeBytes == 0 {
		return ErrInvalidAssignment.Wrap("size_bytes must be positive")
	}
	if a.ChunkSize == 0 {
		return ErrInvalidAssignment.Wrap("chunk_size must be positive")
	}
	if a.ChunkCount == 0 {
		return ErrInvalidAssignment.Wrap("chunk_count must be positive")
	}
	if len(a.MerkleRoot) != ChunkHashSize {
		return ErrInvalidAssignment.Wrapf(
			"merkle_root is %d bytes, expected %d", len(a.MerkleRoot), ChunkHashSize)
	}

	// The declared size must be consistent with the chunking, otherwise a
	// provider could claim byte-hours for a large blob while proving a tiny
	// one.
	minSize := uint64(a.ChunkCount-1) * uint64(a.ChunkSize)
	maxSize := uint64(a.ChunkCount) * uint64(a.ChunkSize)
	if a.SizeBytes <= minSize || a.SizeBytes > maxSize {
		return ErrInvalidAssignment.Wrapf(
			"size_bytes %d is inconsistent with %d chunks of %d bytes (expected %d < size <= %d)",
			a.SizeBytes, a.ChunkCount, a.ChunkSize, minSize, maxSize)
	}

	return nil
}

func assignmentDedupKey(blobID []byte, provider string, replica uint32) string {
	return fmt.Sprintf("%x/%s/%d", blobID, provider, replica)
}

// NewCredit returns a zeroed credit record for a provider and epoch.
func NewCredit(operator string, epoch uint64) ProviderEpochCredit {
	return ProviderEpochCredit{
		Operator:        operator,
		Epoch:           epoch,
		StorageCredit:   math.ZeroInt(),
		RelayCredit:     math.ZeroInt(),
		RetrievalCredit: math.ZeroInt(),
		CallCredit:      math.ZeroInt(),
		Paid:            sdk.NewCoins(),
	}
}

// RawTotal is the sum of every credit component, before the concentration
// discount is applied.
func (c ProviderEpochCredit) RawTotal() math.Int {
	total := math.ZeroInt()
	for _, v := range []math.Int{c.StorageCredit, c.RelayCredit, c.RetrievalCredit, c.CallCredit} {
		if !v.IsNil() {
			total = total.Add(v)
		}
	}
	return total
}
