package keeper

import (
	"context"
	"fmt"

	"cosmossdk.io/collections"
	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/serviceproof/types"
)

// AssignStorage records that a provider is responsible for a blob replica.
//
// Only a registered assigner may call this. That restriction is the whole
// point: an assignment is what turns declared disk into payable disk, so a
// provider assigning work to itself would be self-reporting under a different
// name. The assigner set is empty at genesis and populated by governance; the
// Rust storage layer becomes an assigner in phase 2.
func (k Keeper) AssignStorage(ctx context.Context, assigner string, a types.StorageAssignment) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}
	if !params.Enabled {
		return types.ErrDisabled
	}

	ok, err := k.IsAssigner(ctx, assigner)
	if err != nil {
		return err
	}
	if !ok {
		return types.ErrNotAnAssigner.Wrapf("%s", assigner)
	}
	if err := a.Validate(); err != nil {
		return err
	}

	provider, err := k.GetProvider(ctx, a.Provider)
	if err != nil {
		return err
	}
	if !provider.IsActive() {
		return types.ErrProviderJailed.Wrapf("%s is not accepting assignments", a.Provider)
	}
	if !provider.HasRole(types.SERVICE_ROLE_STORAGE) {
		return types.ErrRoleNotOffered.Wrapf("%s does not offer storage", a.Provider)
	}

	key := collections.Join3(a.Provider, a.BlobId, a.ReplicaIndex)
	exists, err := k.assignments.Has(ctx, key)
	if err != nil {
		return err
	}
	if exists {
		return types.ErrAssignmentExists.Wrapf(
			"blob %x replica %d is already assigned to %s", a.BlobId, a.ReplicaIndex, a.Provider)
	}

	// The provider's declared capacity is a cap on what may be assigned to
	// it. Declaring more disk than you have is not punished directly, but it
	// does mean the network will hand you data you then fail challenges on.
	assigned, err := k.AssignedBytes(ctx, a.Provider)
	if err != nil {
		return err
	}
	if provider.DeclaredStorageBytes > 0 && assigned+a.SizeBytes > provider.DeclaredStorageBytes {
		return types.ErrCapacityExceeded.Wrapf(
			"%s has %d of %d declared bytes assigned; cannot add %d",
			a.Provider, assigned, provider.DeclaredStorageBytes, a.SizeBytes)
	}

	epoch, err := k.CurrentEpochNumber(ctx)
	if err != nil {
		return err
	}

	a.AssignedEpoch = epoch
	// Credit begins accruing from the next epoch, not this one: a blob
	// assigned in the middle of an epoch has not been held for that epoch.
	a.LastCreditedEpoch = epoch
	a.Active = true

	if err := k.assignments.Set(ctx, key, a); err != nil {
		return err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeStorageAssigned,
		sdk.NewAttribute(types.AttributeKeyProvider, a.Provider),
		sdk.NewAttribute(types.AttributeKeyBlobID, fmt.Sprintf("%x", a.BlobId)),
		sdk.NewAttribute(types.AttributeKeySizeBytes, fmt.Sprintf("%d", a.SizeBytes)),
	))

	return nil
}

// ReleaseStorage ends an assignment.
func (k Keeper) ReleaseStorage(ctx context.Context, assigner string, blobID []byte, provider string, replicaIndex uint32) error {
	ok, err := k.IsAssigner(ctx, assigner)
	if err != nil {
		return err
	}
	if !ok {
		return types.ErrNotAnAssigner.Wrapf("%s", assigner)
	}

	key := collections.Join3(provider, blobID, replicaIndex)
	a, err := k.assignments.Get(ctx, key)
	if err != nil {
		return types.ErrAssignmentNotFound.Wrapf("blob %x replica %d for %s", blobID, replicaIndex, provider)
	}

	// Marked inactive rather than deleted, so that history and any pending
	// challenge against it remain resolvable.
	a.Active = false
	return k.assignments.Set(ctx, key, a)
}

// GetAssignment reads one assignment.
func (k Keeper) GetAssignment(ctx context.Context, provider string, blobID []byte, replicaIndex uint32) (types.StorageAssignment, error) {
	a, err := k.assignments.Get(ctx, collections.Join3(provider, blobID, replicaIndex))
	if err != nil {
		return types.StorageAssignment{}, types.ErrAssignmentNotFound.Wrapf(
			"blob %x replica %d for %s", blobID, replicaIndex, provider)
	}
	return a, nil
}

// SetAssignment writes an assignment, used by genesis.
func (k Keeper) SetAssignment(ctx context.Context, a types.StorageAssignment) error {
	return k.assignments.Set(ctx, collections.Join3(a.Provider, a.BlobId, a.ReplicaIndex), a)
}

// activeAssignments returns a provider's active assignments in a
// deterministic order.
func (k Keeper) activeAssignments(ctx context.Context, provider string) ([]types.StorageAssignment, error) {
	var out []types.StorageAssignment
	rng := collections.NewPrefixedTripleRange[string, []byte, uint32](provider)
	err := k.assignments.Walk(ctx, rng, func(_ collections.Triple[string, []byte, uint32], a types.StorageAssignment) (bool, error) {
		if a.Active {
			out = append(out, a)
		}
		return false, nil
	})
	return out, err
}

// ProviderAssignments returns all of a provider's assignments.
func (k Keeper) ProviderAssignments(ctx context.Context, provider string) ([]types.StorageAssignment, error) {
	var out []types.StorageAssignment
	rng := collections.NewPrefixedTripleRange[string, []byte, uint32](provider)
	err := k.assignments.Walk(ctx, rng, func(_ collections.Triple[string, []byte, uint32], a types.StorageAssignment) (bool, error) {
		out = append(out, a)
		return false, nil
	})
	return out, err
}

// AllAssignments returns every assignment, for genesis export.
func (k Keeper) AllAssignments(ctx context.Context) ([]types.StorageAssignment, error) {
	var out []types.StorageAssignment
	err := k.assignments.Walk(ctx, nil, func(_ collections.Triple[string, []byte, uint32], a types.StorageAssignment) (bool, error) {
		out = append(out, a)
		return false, nil
	})
	return out, err
}

// AssignedBytes returns how many bytes a provider is actively responsible
// for. This, not the declared figure, is what accrues byte-hours.
func (k Keeper) AssignedBytes(ctx context.Context, provider string) (uint64, error) {
	var total uint64
	active, err := k.activeAssignments(ctx, provider)
	if err != nil {
		return 0, err
	}
	for _, a := range active {
		total += a.SizeBytes
	}
	return total, nil
}

// AccrueStorageCredit credits verified byte-hours for a closing epoch.
//
// The formula is
//
//	credit = assigned_GiB * storage_credit_per_gib_epoch * challenges_passed / challenges_issued
//
// Three properties follow, all of them required by the specification:
//
//   - Declared but unassigned disk earns nothing, because assigned_GiB is
//     computed from assignments, not from the provider's own declaration.
//   - A provider that fails or ignores challenges earns proportionally less,
//     down to nothing at a zero success rate.
//   - A provider with assignments but no challenges yet earns nothing rather
//     than everything. Unchallenged storage is self-reported storage, and
//     the specification is explicit that self-reported values are not
//     evidence.
func (k Keeper) AccrueStorageCredit(ctx context.Context, epoch uint64, params types.Params) error {
	return k.IterateProviders(ctx, func(p types.Provider) (bool, error) {
		if !p.IsActive() || !p.HasRole(types.SERVICE_ROLE_STORAGE) {
			return false, nil
		}

		credit, err := k.GetCredit(ctx, epoch, p.Operator)
		if err != nil {
			return true, err
		}

		// No challenges issued means no verified storage this epoch.
		if credit.ChallengesIssued == 0 {
			return false, nil
		}

		active, err := k.activeAssignments(ctx, p.Operator)
		if err != nil {
			return true, err
		}

		var heldBytes uint64
		for i := range active {
			a := active[i]
			// Only count assignments held for the whole epoch.
			if a.LastCreditedEpoch >= epoch {
				continue
			}
			heldBytes += a.SizeBytes
			a.LastCreditedEpoch = epoch
			if err := k.SetAssignment(ctx, a); err != nil {
				return true, err
			}
		}
		if heldBytes == 0 {
			return false, nil
		}

		// Proportional to bytes held, in the same scaled credit units as
		// receipts (see creditScale): bytes * rate * scale / GiB, exact.
		base := math.NewIntFromUint64(heldBytes).
			Mul(math.NewIntFromUint64(creditScale)).
			Mul(math.NewIntFromUint64(params.StorageCreditPerGibEpoch)).
			Quo(math.NewIntFromUint64(gibibyte))
		scaled := base.
			Mul(math.NewIntFromUint64(credit.ChallengesPassed)).
			Quo(math.NewIntFromUint64(credit.ChallengesIssued))

		if scaled.IsPositive() {
			credit.StorageCredit = credit.StorageCredit.Add(scaled)
			if err := k.SetCredit(ctx, credit); err != nil {
				return true, err
			}
		}
		return false, nil
	})
}
