package keeper

import (
	"context"
	"errors"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	feeroutertypes "github.com/hashgram/hashgram/x/feerouter/types"
	"github.com/hashgram/hashgram/x/username/types"
)

// Availability reports whether a name can be registered, and if not, why.
//
// A separate read-only path from Register, because a client should be able to
// tell a user "that is visually confusable with @alice" before they pay a
// registration fee, rather than after.
type Availability struct {
	Available       bool
	Normalized      string
	Skeleton        string
	Reason          string
	ConflictingName string
}

// CheckAvailability evaluates a requested name.
func (k Keeper) CheckAvailability(ctx context.Context, requested string) (Availability, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return Availability{}, err
	}

	normalized := types.Normalize(requested)
	skeleton := types.Skeleton(normalized)

	out := Availability{Normalized: normalized, Skeleton: skeleton}

	if err := types.Validate(normalized, params.MinLength, params.MaxLength, params.AllowNonAscii); err != nil {
		var ve *types.ValidationError
		if errors.As(err, &ve) {
			out.Reason = ve.Reason
		} else {
			out.Reason = types.ReasonInvalid
		}
		return out, nil
	}

	if params.IsReserved(normalized) {
		out.Reason = types.ReasonReserved
		return out, nil
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)

	existing, found, err := k.GetRegistration(ctx, normalized)
	if err != nil {
		return Availability{}, err
	}
	if found && !k.isClaimable(existing, params, sdkCtx.BlockHeight()) {
		out.Reason = types.ReasonTaken
		out.ConflictingName = existing.Name
		return out, nil
	}

	// Confusable collision. Checked against the skeleton index rather than by
	// scanning, so cost does not grow with the namespace.
	holder, held, err := k.SkeletonHolder(ctx, skeleton)
	if err != nil {
		return Availability{}, err
	}
	if held && holder != normalized {
		other, otherFound, err := k.GetRegistration(ctx, holder)
		if err != nil {
			return Availability{}, err
		}
		if otherFound && !k.isClaimable(other, params, sdkCtx.BlockHeight()) {
			out.Reason = types.ReasonConfusableWith
			out.ConflictingName = holder
			return out, nil
		}
	}

	out.Available = true
	return out, nil
}

// isClaimable reports whether an existing registration has lapsed far enough
// that somebody else may take the name.
//
// A name is claimable once it is past its expiry *and* past the grace period
// in which the previous owner retains an exclusive right to renew. The grace
// period exists so that a user who missed a renewal is not sniped by a bot
// within the hour.
func (k Keeper) isClaimable(r types.Registration, params types.Params, height int64) bool {
	if r.ExpiryHeight == 0 {
		return false // permanent registration
	}
	return height > r.ExpiryHeight+params.GracePeriodBlocks
}

// Register claims a name for an owner.
//
// Checks run before any state is written, and the fee is charged only once
// the name is certain to be granted: a user must not pay for a rejected
// registration.
func (k Keeper) Register(ctx context.Context, owner sdk.AccAddress, requested string) (types.Registration, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return types.Registration{}, err
	}

	availability, err := k.CheckAvailability(ctx, requested)
	if err != nil {
		return types.Registration{}, err
	}
	if !availability.Available {
		return types.Registration{}, availabilityError(availability)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	// If a lapsed registration is being taken over, clear the old one first
	// so its indexes do not linger pointing at the previous owner.
	if existing, found, err := k.GetRegistration(ctx, availability.Normalized); err != nil {
		return types.Registration{}, err
	} else if found {
		if err := k.removeRegistration(ctx, existing); err != nil {
			return types.Registration{}, err
		}
	}
	// The same applies to a lapsed registration that merely held the
	// skeleton under a different name.
	if holder, held, err := k.SkeletonHolder(ctx, availability.Skeleton); err != nil {
		return types.Registration{}, err
	} else if held && holder != availability.Normalized {
		if other, found, err := k.GetRegistration(ctx, holder); err != nil {
			return types.Registration{}, err
		} else if found && k.isClaimable(other, params, height) {
			if err := k.removeRegistration(ctx, other); err != nil {
				return types.Registration{}, err
			}
		}
	}

	if err := k.feeRouter.CollectServiceFee(ctx, owner, params.RegistrationFee,
		feeroutertypes.SERVICE_KIND_USERNAME); err != nil {
		return types.Registration{}, fmt.Errorf("collecting the registration fee: %w", err)
	}

	expiry := int64(0)
	if params.RegistrationPeriodBlocks > 0 {
		expiry = height + params.RegistrationPeriodBlocks
	}

	reg := types.Registration{
		Name:             availability.Normalized,
		Owner:            owner.String(),
		Skeleton:         availability.Skeleton,
		RegisteredHeight: height,
		ExpiryHeight:     expiry,
		Transferable:     true,
	}
	if err := k.setRegistration(ctx, reg); err != nil {
		return types.Registration{}, err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRegistered,
		sdk.NewAttribute(types.AttributeKeyName, reg.Name),
		sdk.NewAttribute(types.AttributeKeyOwner, reg.Owner),
		sdk.NewAttribute(types.AttributeKeyExpiry, fmt.Sprintf("%d", reg.ExpiryHeight)),
		sdk.NewAttribute(types.AttributeKeyFee, params.RegistrationFee.String()),
	))

	return reg, nil
}

func availabilityError(a Availability) error {
	switch a.Reason {
	case types.ReasonTaken:
		return types.ErrNameTaken.Wrapf("%q", a.Normalized)
	case types.ReasonReserved:
		return types.ErrNameReserved.Wrapf("%q", a.Normalized)
	case types.ReasonConfusableWith:
		return types.ErrConfusable.Wrapf(
			"%q folds to the same skeleton as the registered name %q", a.Normalized, a.ConflictingName)
	case types.ReasonTooShort, types.ReasonTooLong, types.ReasonMixedScript,
		types.ReasonNonASCIINotAllowed, types.ReasonInvalid:
		return types.ErrInvalidName.Wrapf("%q: %s", a.Normalized, a.Reason)
	default:
		return types.ErrInvalidName.Wrapf("%q is unavailable: %s", a.Normalized, a.Reason)
	}
}

// Renew extends a registration.
//
// Only the current owner may renew, and only while the name has not become
// claimable by others. Inside the grace period the owner still has the
// exclusive right, which is the point of the grace period.
func (k Keeper) Renew(ctx context.Context, owner sdk.AccAddress, requested string) (int64, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return 0, err
	}

	normalized := types.Normalize(requested)
	reg, found, err := k.GetRegistration(ctx, normalized)
	if err != nil {
		return 0, err
	}
	if !found {
		return 0, types.ErrNameNotFound.Wrapf("%q", normalized)
	}
	if reg.Owner != owner.String() {
		return 0, types.ErrNotOwner.Wrapf("%q is owned by %s", normalized, reg.Owner)
	}
	if reg.ExpiryHeight == 0 {
		// Permanent registration; nothing to renew.
		return 0, nil
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	if k.isClaimable(reg, params, height) {
		return 0, types.ErrNameNotFound.Wrapf(
			"%q lapsed at height %d and is past its grace period; it must be registered afresh",
			normalized, reg.ExpiryHeight)
	}

	if err := k.feeRouter.CollectServiceFee(ctx, owner, params.RenewalFee,
		feeroutertypes.SERVICE_KIND_USERNAME); err != nil {
		return 0, fmt.Errorf("collecting the renewal fee: %w", err)
	}

	// Extend from the later of now and the current expiry, so renewing early
	// does not forfeit remaining time.
	base := reg.ExpiryHeight
	if height > base {
		base = height
	}
	reg.ExpiryHeight = base + params.RegistrationPeriodBlocks

	if err := k.setRegistration(ctx, reg); err != nil {
		return 0, err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRenewed,
		sdk.NewAttribute(types.AttributeKeyName, reg.Name),
		sdk.NewAttribute(types.AttributeKeyOwner, reg.Owner),
		sdk.NewAttribute(types.AttributeKeyExpiry, fmt.Sprintf("%d", reg.ExpiryHeight)),
	))

	return reg.ExpiryHeight, nil
}

// Transfer hands a name to a new owner.
func (k Keeper) Transfer(ctx context.Context, owner sdk.AccAddress, requested string, newOwner sdk.AccAddress) error {
	normalized := types.Normalize(requested)

	reg, found, err := k.GetRegistration(ctx, normalized)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrNameNotFound.Wrapf("%q", normalized)
	}
	if reg.Owner != owner.String() {
		return types.ErrNotOwner.Wrapf("%q is owned by %s", normalized, reg.Owner)
	}
	if !reg.Transferable {
		return types.ErrNotTransferable.Wrapf(
			"%q is locked against transfer; unlock it first with MsgSetTransferable", normalized)
	}

	// Remove and re-add so the owner index does not keep pointing at the old
	// owner.
	if err := k.removeRegistration(ctx, reg); err != nil {
		return err
	}
	reg.Owner = newOwner.String()
	if err := k.setRegistration(ctx, reg); err != nil {
		return err
	}

	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeTransferred,
		sdk.NewAttribute(types.AttributeKeyName, reg.Name),
		sdk.NewAttribute(types.AttributeKeyOwner, owner.String()),
		sdk.NewAttribute(types.AttributeKeyNewOwner, reg.Owner),
	))

	return nil
}

// SetTransferable locks or unlocks transfer for a name.
//
// Locking is a defence against key compromise: an attacker who steals the
// owner key still cannot move a locked name, and unlocking is itself a
// transaction the owner can see on chain.
func (k Keeper) SetTransferable(ctx context.Context, owner sdk.AccAddress, requested string, transferable bool) error {
	normalized := types.Normalize(requested)

	reg, found, err := k.GetRegistration(ctx, normalized)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrNameNotFound.Wrapf("%q", normalized)
	}
	if reg.Owner != owner.String() {
		return types.ErrNotOwner.Wrapf("%q is owned by %s", normalized, reg.Owner)
	}

	reg.Transferable = transferable
	return k.setRegistration(ctx, reg)
}

// Release gives up a name voluntarily, returning it to circulation.
func (k Keeper) Release(ctx context.Context, owner sdk.AccAddress, requested string) error {
	normalized := types.Normalize(requested)

	reg, found, err := k.GetRegistration(ctx, normalized)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrNameNotFound.Wrapf("%q", normalized)
	}
	if reg.Owner != owner.String() {
		return types.ErrNotOwner.Wrapf("%q is owned by %s", normalized, reg.Owner)
	}

	if err := k.removeRegistration(ctx, reg); err != nil {
		return err
	}

	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeReleased,
		sdk.NewAttribute(types.AttributeKeyName, reg.Name),
		sdk.NewAttribute(types.AttributeKeyOwner, reg.Owner),
	))

	return nil
}

// Lookup resolves a name to its registration.
//
// An expired registration past its grace period is reported as not found:
// resolving a lapsed name to its former owner would be actively misleading.
func (k Keeper) Lookup(ctx context.Context, requested string) (types.Registration, bool, error) {
	params, err := k.GetParams(ctx)
	if err != nil {
		return types.Registration{}, false, err
	}

	normalized := types.Normalize(requested)
	reg, found, err := k.GetRegistration(ctx, normalized)
	if err != nil || !found {
		return types.Registration{}, false, err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	if k.isClaimable(reg, params, sdkCtx.BlockHeight()) {
		return types.Registration{}, false, nil
	}
	return reg, true, nil
}
