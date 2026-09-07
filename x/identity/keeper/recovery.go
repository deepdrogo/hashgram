package keeper

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	"github.com/hashgram/hashgram/x/identity/types"
)

// InitiateRecovery opens a recovery request.
//
// Only a guardian may open one, and only one may be open at a time. The
// request does not take effect immediately: it becomes executable only after
// the identity's configured delay, which is the window in which the real
// owner can cancel it.
func (k Keeper) InitiateRecovery(ctx context.Context, msg *types.MsgInitiateRecovery) (int64, error) {
	identity, found, err := k.GetIdentity(ctx, msg.RootAddress)
	if err != nil {
		return 0, err
	}
	if !found {
		return 0, types.ErrIdentityNotFound.Wrapf("%s", msg.RootAddress)
	}
	if identity.Revoked {
		return 0, types.ErrIdentityRevoked.Wrapf("%s", msg.RootAddress)
	}

	if identity.Recovery.Threshold == 0 {
		return 0, types.ErrRecoveryDisabled.Wrapf("%s", msg.RootAddress)
	}
	if !identity.Recovery.IsGuardian(msg.Initiator) {
		return 0, types.ErrNotAGuardian.Wrapf(
			"%s is not a guardian of %s", msg.Initiator, msg.RootAddress)
	}

	if _, err := types.PubKeyFromBytes(msg.NewRootPubkey, msg.NewRootKeyType); err != nil {
		return 0, err
	}

	// The target address must be free. Recovering onto an address that
	// already has an identity would either clobber it or create two
	// identities for one address.
	if msg.NewAddress != msg.RootAddress {
		if _, taken, err := k.GetIdentity(ctx, msg.NewAddress); err != nil {
			return 0, err
		} else if taken {
			return 0, types.ErrTargetAddressInUse.Wrapf("%s", msg.NewAddress)
		}
	}

	if existing, found, err := k.GetRecovery(ctx, msg.RootAddress); err != nil {
		return 0, err
	} else if found && !existing.Cancelled {
		return 0, types.ErrRecoveryExists.Wrapf(
			"a recovery request for %s is already open, initiated at height %d",
			msg.RootAddress, existing.InitiatedHeight)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	request := types.RecoveryRequest{
		RootAddress:      msg.RootAddress,
		NewAddress:       msg.NewAddress,
		NewRootPubkey:    msg.NewRootPubkey,
		NewRootKeyType:   msg.NewRootKeyType,
		Approvals:        []string{msg.Initiator},
		InitiatedHeight:  height,
		ExecutableHeight: height + identity.Recovery.RecoveryDelayBlocks,
	}
	if err := k.SetRecovery(ctx, request); err != nil {
		return 0, err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRecoveryOpened,
		sdk.NewAttribute(types.AttributeKeyAddress, msg.RootAddress),
		sdk.NewAttribute(types.AttributeKeyNewAddress, msg.NewAddress),
		sdk.NewAttribute(types.AttributeKeyGuardian, msg.Initiator),
		sdk.NewAttribute(types.AttributeKeyThreshold, fmt.Sprintf("%d", identity.Recovery.Threshold)),
	))

	k.logger.Info("identity recovery initiated",
		"identity", msg.RootAddress,
		"new_address", msg.NewAddress,
		"initiator", msg.Initiator,
		"executable_height", request.ExecutableHeight,
		"note", "the identity owner can cancel this before the delay elapses")

	return request.ExecutableHeight, nil
}

// ApproveRecovery records a guardian's approval and reports the approvals
// collected so far alongside the threshold needed.
//
// Both are ints: approvals is a slice length and the threshold is bounded by
// the guardian list, which MaxGuardians caps at a small value.
func (k Keeper) ApproveRecovery(ctx context.Context, guardian, rootAddress string) (int, int, error) {
	identity, found, err := k.GetIdentity(ctx, rootAddress)
	if err != nil {
		return 0, 0, err
	}
	if !found {
		return 0, 0, types.ErrIdentityNotFound.Wrapf("%s", rootAddress)
	}
	if !identity.Recovery.IsGuardian(guardian) {
		return 0, 0, types.ErrNotAGuardian.Wrapf("%s is not a guardian of %s", guardian, rootAddress)
	}

	request, found, err := k.GetRecovery(ctx, rootAddress)
	if err != nil {
		return 0, 0, err
	}
	if !found {
		return 0, 0, types.ErrRecoveryNotFound.Wrapf("%s", rootAddress)
	}
	if request.Cancelled {
		return 0, 0, types.ErrRecoveryCancelled.Wrapf("%s", rootAddress)
	}
	if request.HasApproved(guardian) {
		return 0, 0, types.ErrAlreadyApproved.Wrapf("%s", guardian)
	}

	request.Approvals = append(request.Approvals, guardian)
	if err := k.SetRecovery(ctx, request); err != nil {
		return 0, 0, err
	}

	approvals := len(request.Approvals)

	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRecoveryApproved,
		sdk.NewAttribute(types.AttributeKeyAddress, rootAddress),
		sdk.NewAttribute(types.AttributeKeyGuardian, guardian),
		sdk.NewAttribute(types.AttributeKeyApprovals, fmt.Sprintf("%d", approvals)),
		sdk.NewAttribute(types.AttributeKeyThreshold, fmt.Sprintf("%d", identity.Recovery.Threshold)),
	))

	return approvals, int(identity.Recovery.Threshold), nil
}

// CancelRecovery lets the identity owner reject a recovery attempt.
//
// This is what makes the delay protective rather than decorative: a user
// whose guardians have been socially engineered has the delay window to
// notice and refuse. Without cancellation, meeting the threshold would be an
// immediately final takeover.
func (k Keeper) CancelRecovery(ctx context.Context, address string) error {
	request, found, err := k.GetRecovery(ctx, address)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrRecoveryNotFound.Wrapf("%s", address)
	}
	if request.Cancelled {
		return types.ErrRecoveryCancelled.Wrapf("%s", address)
	}

	// Kept rather than deleted, so an attempted takeover leaves a trace the
	// user can point at.
	request.Cancelled = true
	if err := k.SetRecovery(ctx, request); err != nil {
		return err
	}

	sdk.UnwrapSDKContext(ctx).EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRecoveryCanceled,
		sdk.NewAttribute(types.AttributeKeyAddress, address),
	))

	k.logger.Info("identity recovery cancelled by the owner", "identity", address)
	return nil
}

// ExecuteRecovery completes an approved recovery whose delay has elapsed.
//
// Any address may submit this. The authorisation is the recorded guardian
// approvals plus the elapsed delay, not the sender's identity, which means a
// recovering user who has lost access to every funded account can still have
// somebody submit the final step for them.
func (k Keeper) ExecuteRecovery(ctx context.Context, rootAddress string) error {
	identity, found, err := k.GetIdentity(ctx, rootAddress)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrIdentityNotFound.Wrapf("%s", rootAddress)
	}
	if identity.Revoked {
		return types.ErrIdentityRevoked.Wrapf("%s", rootAddress)
	}

	request, found, err := k.GetRecovery(ctx, rootAddress)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrRecoveryNotFound.Wrapf("%s", rootAddress)
	}
	if request.Cancelled {
		return types.ErrRecoveryCancelled.Wrapf("%s", rootAddress)
	}

	if len(request.Approvals) < int(identity.Recovery.Threshold) {
		return types.ErrThresholdNotMet.Wrapf(
			"%d of %d required approvals", len(request.Approvals), identity.Recovery.Threshold)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	if sdkCtx.BlockHeight() < request.ExecutableHeight {
		return types.ErrRecoveryDelayActive.Wrapf(
			"recovery is executable at height %d, current height is %d",
			request.ExecutableHeight, sdkCtx.BlockHeight())
	}

	// Every existing device is revoked. The devices were authorised by a root
	// key the user has lost control of, so continuing to trust them would
	// defeat the purpose of recovering.
	devices, err := k.ListDevices(ctx, rootAddress, false)
	if err != nil {
		return err
	}
	for _, d := range devices {
		d.Revoked = true
		d.RevokedHeight = sdkCtx.BlockHeight()
		if err := k.SetDevice(ctx, d); err != nil {
			return err
		}
	}

	if request.NewAddress == rootAddress {
		// Recovering in place: same administering account, new root key.
		identity.RootPubkey = request.NewRootPubkey
		identity.RootKeyType = request.NewRootKeyType
		identity.RotationCount++
		if err := k.SetIdentity(ctx, identity); err != nil {
			return err
		}
	} else {
		// Moving to a new administering account. The old record is marked
		// revoked rather than deleted so historical signatures stay
		// attributable.
		identity.Revoked = true
		identity.RevokedHeight = sdkCtx.BlockHeight()
		if err := k.SetIdentity(ctx, identity); err != nil {
			return err
		}

		recovered := types.RootIdentity{
			Address:       request.NewAddress,
			RootPubkey:    request.NewRootPubkey,
			RootKeyType:   request.NewRootKeyType,
			CreatedHeight: sdkCtx.BlockHeight(),
			RotationCount: identity.RotationCount + 1,
			Recovery:      identity.Recovery,
		}
		if err := k.SetIdentity(ctx, recovered); err != nil {
			return err
		}
	}

	request.Cancelled = true // closed
	if err := k.SetRecovery(ctx, request); err != nil {
		return err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRecoveryExecuted,
		sdk.NewAttribute(types.AttributeKeyAddress, rootAddress),
		sdk.NewAttribute(types.AttributeKeyNewAddress, request.NewAddress),
		sdk.NewAttribute(types.AttributeKeyRevoked, fmt.Sprintf("%d", len(devices))),
	))

	k.logger.Info("identity recovery executed",
		"identity", rootAddress,
		"new_address", request.NewAddress,
		"devices_revoked", len(devices))

	return nil
}
