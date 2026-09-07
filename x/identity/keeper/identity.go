package keeper

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/x/identity/types"
)

// CreateIdentity establishes a root identity, optionally with its first
// device.
func (k Keeper) CreateIdentity(ctx context.Context, msg *types.MsgCreateIdentity) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}

	if _, found, err := k.GetIdentity(ctx, msg.Address); err != nil {
		return err
	} else if found {
		return types.ErrIdentityExists.Wrapf("%s", msg.Address)
	}

	if _, err := types.PubKeyFromBytes(msg.RootPubkey, msg.RootKeyType); err != nil {
		return err
	}
	if err := msg.Recovery.Validate(params.MaxGuardians, params.MinRecoveryDelayBlocks); err != nil {
		return err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)

	identity := types.RootIdentity{
		Address:       msg.Address,
		RootPubkey:    msg.RootPubkey,
		RootKeyType:   msg.RootKeyType,
		CreatedHeight: sdkCtx.BlockHeight(),
		RotationCount: 0,
		Recovery:      msg.Recovery,
	}
	if err := k.SetIdentity(ctx, identity); err != nil {
		return err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeIdentityCreated,
		sdk.NewAttribute(types.AttributeKeyAddress, msg.Address),
	))

	// The initial device is optional; a zero certificate means none was
	// supplied. Bundling it saves a brand-new user a second transaction.
	if len(msg.InitialDevice.DevicePubkey) == 0 {
		return nil
	}
	return k.AddDevice(ctx, msg.Address, msg.InitialDevice,
		msg.InitialDeviceLabel, msg.InitialDevicePlatform)
}

// AddDevice authorises a device using a root-signed certificate.
//
// The certificate, not the transaction signature, is the authority. Whoever
// pays the gas for this transaction cannot insert a device: they would need
// the identity's root key to produce a certificate that verifies.
func (k Keeper) AddDevice(ctx context.Context, address string, cert types.DeviceCertificate, label, platform string) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}

	identity, found, err := k.GetIdentity(ctx, address)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrIdentityNotFound.Wrapf("%s", address)
	}
	if identity.Revoked {
		return types.ErrIdentityRevoked.Wrapf("%s", address)
	}

	if err := cert.ValidateBasic(params.MaxDeviceIdLength); err != nil {
		return err
	}
	if err := types.ValidateLabel(label, params.MaxDeviceLabelLength); err != nil {
		return err
	}
	if err := types.ValidateLabel(platform, params.MaxDeviceLabelLength); err != nil {
		return err
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	height := sdkCtx.BlockHeight()

	if cert.ExpiryHeight < height {
		return types.ErrCertificateExpired.Wrapf(
			"certificate expired at height %d, current height is %d", cert.ExpiryHeight, height)
	}
	if cert.ExpiryHeight > height+params.MaxCertificateAgeBlocks {
		return types.ErrCertificateTooLong.Wrapf(
			"certificate expiry %d is %d blocks away, maximum is %d",
			cert.ExpiryHeight, cert.ExpiryHeight-height, params.MaxCertificateAgeBlocks)
	}

	// A certificate issued under a superseded root key must not work. This
	// is what makes rotation meaningful rather than cosmetic.
	if cert.RotationCount != identity.RotationCount {
		return types.ErrRotationMismatch.Wrapf(
			"certificate is for rotation %d but %s is at rotation %d",
			cert.RotationCount, address, identity.RotationCount)
	}

	if err := k.verifyCertificate(ctx, identity, cert); err != nil {
		return err
	}

	if existing, found, err := k.GetDevice(ctx, address, cert.DeviceId); err != nil {
		return err
	} else if found {
		if !existing.Revoked {
			return types.ErrDeviceExists.Wrapf("%s already has device %q", address, cert.DeviceId)
		}
		// A revoked device id is not reusable: reusing it would make audit
		// trails ambiguous about which key an event came from.
		return types.ErrDeviceRevoked.Wrapf(
			"device id %q was revoked and cannot be reused; choose a new id", cert.DeviceId)
	}

	inUse, err := k.DeviceKeyInUse(ctx, cert.DevicePubkey)
	if err != nil {
		return err
	}
	if inUse {
		return types.ErrDeviceKeyInUse.Wrap(
			"this device public key is already registered; two identities sharing a device " +
				"key would make event attribution ambiguous")
	}

	count, err := k.ActiveDeviceCount(ctx, address)
	if err != nil {
		return err
	}
	if count >= params.MaxDevicesPerIdentity {
		return types.ErrTooManyDevices.Wrapf(
			"%s has %d of %d permitted devices; revoke one first",
			address, count, params.MaxDevicesPerIdentity)
	}

	device := types.Device{
		RootAddress:      address,
		DeviceId:         cert.DeviceId,
		DevicePubkey:     cert.DevicePubkey,
		DeviceKeyType:    cert.DeviceKeyType,
		Label:            label,
		Platform:         platform,
		AuthorizedHeight: height,
		Certificate:      cert,
	}
	if err := k.SetDevice(ctx, device); err != nil {
		return err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeDeviceAdded,
		sdk.NewAttribute(types.AttributeKeyAddress, address),
		sdk.NewAttribute(types.AttributeKeyDeviceID, cert.DeviceId),
		sdk.NewAttribute(types.AttributeKeyLabel, label),
		sdk.NewAttribute(types.AttributeKeyPlatform, platform),
	))

	return nil
}

// verifyCertificate checks a device certificate against the identity's
// current root key.
//
// The digest comes from x/network, which mixes in the network magic, the
// protocol major version and the "device-cert" purpose. So a certificate
// minted on a devnet or a fork does not authorise a device on Mainnet, and
// these bytes cannot be reinterpreted as a service receipt or an eligibility
// attestation.
func (k Keeper) verifyCertificate(ctx context.Context, identity types.RootIdentity, cert types.DeviceCertificate) error {
	rootPub, err := types.PubKeyFromBytes(identity.RootPubkey, identity.RootKeyType)
	if err != nil {
		return err
	}

	payload := types.CanonicalCertificateBytes(identity.Address, cert)
	digest, err := k.networkKeeper.SigningDigest(ctx, hgparams.PurposeDeviceCert, payload)
	if err != nil {
		return err
	}

	if !rootPub.VerifySignature(digest[:], cert.Signature) {
		return types.ErrInvalidCertificate.Wrapf(
			"certificate for device %q does not verify against the root key of %s",
			cert.DeviceId, identity.Address)
	}
	return nil
}

// RevokeDevice withdraws a device's authorisation.
//
// The record is kept rather than deleted so that events signed before
// revocation remain attributable, and so a revoked device id cannot be
// quietly reused.
func (k Keeper) RevokeDevice(ctx context.Context, address, deviceID string) error {
	identity, found, err := k.GetIdentity(ctx, address)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrIdentityNotFound.Wrapf("%s", address)
	}
	_ = identity

	device, found, err := k.GetDevice(ctx, address, deviceID)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrDeviceNotFound.Wrapf("%s has no device %q", address, deviceID)
	}
	if device.Revoked {
		return types.ErrDeviceRevoked.Wrapf("%q", deviceID)
	}

	// Refusing to revoke the last device is a usability decision with a
	// security justification: an identity with no devices cannot sign
	// anything, including the transaction that would add a replacement, so
	// the user would be locked out with no recourse short of social recovery.
	count, err := k.ActiveDeviceCount(ctx, address)
	if err != nil {
		return err
	}
	if count <= 1 {
		return types.ErrLastDeviceRemove.Wrapf(
			"%s has only device %q; add a replacement before revoking it", address, deviceID)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	device.Revoked = true
	device.RevokedHeight = sdkCtx.BlockHeight()
	if err := k.SetDevice(ctx, device); err != nil {
		return err
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeDeviceRevoked,
		sdk.NewAttribute(types.AttributeKeyAddress, address),
		sdk.NewAttribute(types.AttributeKeyDeviceID, deviceID),
	))

	return nil
}

// RotateRootKey replaces an identity's root key.
//
// The outgoing root key must sign the rotation. Without that, an attacker who
// compromised only the account key could install their own root key and then
// authorise their own devices, taking over the identity's messaging and
// social presence without ever touching the root key.
func (k Keeper) RotateRootKey(ctx context.Context, msg *types.MsgRotateRootKey) (int, error) {
	identity, found, err := k.GetIdentity(ctx, msg.Address)
	if err != nil {
		return 0, err
	}
	if !found {
		return 0, types.ErrIdentityNotFound.Wrapf("%s", msg.Address)
	}
	if identity.Revoked {
		return 0, types.ErrIdentityRevoked.Wrapf("%s", msg.Address)
	}

	if _, err := types.PubKeyFromBytes(msg.NewRootPubkey, msg.NewRootKeyType); err != nil {
		return 0, err
	}

	oldPub, err := types.PubKeyFromBytes(identity.RootPubkey, identity.RootKeyType)
	if err != nil {
		return 0, err
	}

	payload := types.CanonicalRotationBytes(
		msg.Address, msg.NewRootPubkey, msg.NewRootKeyType, identity.RotationCount)
	digest, err := k.networkKeeper.SigningDigest(ctx, hgparams.PurposeDeviceCert, payload)
	if err != nil {
		return 0, err
	}
	if !oldPub.VerifySignature(digest[:], msg.SignatureByOldRoot) {
		return 0, types.ErrInvalidRotationSignature.Wrapf(
			"rotation for %s is not signed by the root key being replaced", msg.Address)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)

	identity.RootPubkey = msg.NewRootPubkey
	identity.RootKeyType = msg.NewRootKeyType
	identity.RotationCount++
	if err := k.SetIdentity(ctx, identity); err != nil {
		return 0, err
	}

	revoked := 0
	if msg.RevokeExistingDevices {
		// The right default after a suspected root-key compromise:
		// certificates issued by the old root can no longer be trusted, so
		// the devices they authorised should not be either.
		devices, err := k.ListDevices(ctx, msg.Address, false)
		if err != nil {
			return 0, err
		}
		for _, d := range devices {
			d.Revoked = true
			d.RevokedHeight = sdkCtx.BlockHeight()
			if err := k.SetDevice(ctx, d); err != nil {
				return 0, err
			}
			revoked++
		}
	}

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeRootKeyRotated,
		sdk.NewAttribute(types.AttributeKeyAddress, msg.Address),
		sdk.NewAttribute(types.AttributeKeyRotation, fmt.Sprintf("%d", identity.RotationCount)),
		sdk.NewAttribute(types.AttributeKeyRevoked, fmt.Sprintf("%d", revoked)),
	))

	return revoked, nil
}

// SetRecoveryConfig changes an identity's recovery configuration.
func (k Keeper) SetRecoveryConfig(ctx context.Context, address string, cfg types.RecoveryConfig) error {
	params, err := k.GetParams(ctx)
	if err != nil {
		return err
	}

	identity, found, err := k.GetIdentity(ctx, address)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrIdentityNotFound.Wrapf("%s", address)
	}
	if identity.Revoked {
		return types.ErrIdentityRevoked.Wrapf("%s", address)
	}

	if err := cfg.Validate(params.MaxGuardians, params.MinRecoveryDelayBlocks); err != nil {
		return err
	}

	identity.Recovery = cfg
	return k.SetIdentity(ctx, identity)
}

// RevokeIdentity abandons an identity.
//
// The record is kept rather than deleted so that historical signatures remain
// attributable and the address cannot be silently reused as if fresh.
func (k Keeper) RevokeIdentity(ctx context.Context, address string) error {
	identity, found, err := k.GetIdentity(ctx, address)
	if err != nil {
		return err
	}
	if !found {
		return types.ErrIdentityNotFound.Wrapf("%s", address)
	}
	if identity.Revoked {
		return types.ErrIdentityRevoked.Wrapf("%s", address)
	}

	sdkCtx := sdk.UnwrapSDKContext(ctx)
	identity.Revoked = true
	identity.RevokedHeight = sdkCtx.BlockHeight()
	if err := k.SetIdentity(ctx, identity); err != nil {
		return err
	}

	devices, err := k.ListDevices(ctx, address, false)
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

	sdkCtx.EventManager().EmitEvent(sdk.NewEvent(
		types.EventTypeIdentityRevoked,
		sdk.NewAttribute(types.AttributeKeyAddress, address),
	))

	return nil
}
