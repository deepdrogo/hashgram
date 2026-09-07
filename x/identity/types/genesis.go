package types

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

const (
	// DefaultMaxDevicesPerIdentity bounds an identity's device set.
	//
	// A bound is necessary because every device is a signing key that other
	// users' clients must fetch and check. Twenty is generous for a person
	// with several phones, tablets and computers, and small enough that
	// verifying a sender's device set stays cheap.
	DefaultMaxDevicesPerIdentity uint32 = 20

	// DefaultMaxGuardians bounds the recovery guardian set.
	DefaultMaxGuardians uint32 = 10

	// DefaultMinRecoveryDelayBlocks is about 48 hours at 4 second blocks.
	//
	// A floor rather than a suggestion. The delay is the window in which a
	// user whose guardians have been socially engineered can notice and
	// cancel; a user who could set it to zero would have no such window, and
	// social recovery would become a single-step takeover for anyone who can
	// persuade enough guardians.
	DefaultMinRecoveryDelayBlocks int64 = 43_200

	// DefaultMaxCertificateAgeBlocks is about 24 hours: a device certificate
	// is meant to be minted and presented promptly, not held indefinitely.
	DefaultMaxCertificateAgeBlocks int64 = 21_600

	// DefaultMaxDeviceLabelLength and DefaultMaxDeviceIDLength bound
	// client-supplied strings.
	DefaultMaxDeviceLabelLength uint32 = 64
	DefaultMaxDeviceIDLength    uint32 = 64
)

// DefaultParams returns the launch configuration.
func DefaultParams() Params {
	return Params{
		MaxDevicesPerIdentity:   DefaultMaxDevicesPerIdentity,
		MaxGuardians:            DefaultMaxGuardians,
		MinRecoveryDelayBlocks:  DefaultMinRecoveryDelayBlocks,
		MaxCertificateAgeBlocks: DefaultMaxCertificateAgeBlocks,
		MaxDeviceLabelLength:    DefaultMaxDeviceLabelLength,
		MaxDeviceIdLength:       DefaultMaxDeviceIDLength,
	}
}

// Validate checks the parameter set.
func (p Params) Validate() error {
	if p.MaxDevicesPerIdentity == 0 {
		return ErrInvalidParams.Wrap("max_devices_per_identity must be positive")
	}
	if p.MaxCertificateAgeBlocks <= 0 {
		return ErrInvalidParams.Wrap("max_certificate_age_blocks must be positive")
	}
	if p.MinRecoveryDelayBlocks <= 0 {
		return ErrInvalidParams.Wrap(
			"min_recovery_delay_blocks must be positive; a zero delay makes social " +
				"recovery a single-step takeover for anyone who can persuade enough guardians")
	}
	if p.MaxDeviceIdLength == 0 {
		return ErrInvalidParams.Wrap("max_device_id_length must be positive")
	}
	return nil
}

// DefaultGenesis returns the x/identity genesis state.
func DefaultGenesis() *GenesisState {
	return &GenesisState{
		Params:           DefaultParams(),
		Identities:       nil,
		Devices:          nil,
		RecoveryRequests: nil,
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	if err := gs.Params.Validate(); err != nil {
		return err
	}

	identities := make(map[string]RootIdentity, len(gs.Identities))
	for i, id := range gs.Identities {
		if _, err := sdk.AccAddressFromBech32(id.Address); err != nil {
			return ErrIdentityNotFound.Wrapf("identities[%d].address %q: %v", i, id.Address, err)
		}
		if _, err := PubKeyFromBytes(id.RootPubkey, id.RootKeyType); err != nil {
			return ErrInvalidKey.Wrapf("identities[%d]: %v", i, err)
		}
		if _, dup := identities[id.Address]; dup {
			return ErrIdentityExists.Wrapf("identities contains %s twice", id.Address)
		}
		if err := id.Recovery.Validate(gs.Params.MaxGuardians, gs.Params.MinRecoveryDelayBlocks); err != nil {
			return ErrInvalidRecovery.Wrapf("identities[%d]: %v", i, err)
		}
		identities[id.Address] = id
	}

	// Device public keys must be globally unique. Two identities sharing a
	// device key would make "which identity signed this?" ambiguous, which is
	// exactly the question the key index exists to answer.
	seenKey := make(map[string]string, len(gs.Devices))
	deviceCount := make(map[string]uint32, len(gs.Identities))

	for i, d := range gs.Devices {
		identity, ok := identities[d.RootAddress]
		if !ok {
			return ErrIdentityNotFound.Wrapf(
				"devices[%d] belongs to %s, which has no identity", i, d.RootAddress)
		}
		if err := ValidateDeviceID(d.DeviceId, gs.Params.MaxDeviceIdLength); err != nil {
			return ErrInvalidDeviceID.Wrapf("devices[%d]: %v", i, err)
		}
		if _, err := PubKeyFromBytes(d.DevicePubkey, d.DeviceKeyType); err != nil {
			return ErrInvalidKey.Wrapf("devices[%d]: %v", i, err)
		}

		keyHex := string(d.DevicePubkey)
		if owner, dup := seenKey[keyHex]; dup {
			return ErrDeviceKeyInUse.Wrapf(
				"devices[%d] shares a public key with a device of %s", i, owner)
		}
		seenKey[keyHex] = d.RootAddress

		if !d.Revoked {
			deviceCount[d.RootAddress]++
			if deviceCount[d.RootAddress] > gs.Params.MaxDevicesPerIdentity {
				return ErrTooManyDevices.Wrapf(
					"%s has more than %d active devices", d.RootAddress, gs.Params.MaxDevicesPerIdentity)
			}
		}

		// The recorded certificate must still be consistent with the
		// identity's rotation count, or a restored chain would contain
		// devices that a live chain would have refused.
		if !d.Revoked && d.Certificate.RotationCount != identity.RotationCount {
			return ErrRotationMismatch.Wrapf(
				"devices[%d] certificate is for rotation %d but %s is at rotation %d",
				i, d.Certificate.RotationCount, d.RootAddress, identity.RotationCount)
		}
	}

	for i, r := range gs.RecoveryRequests {
		if _, ok := identities[r.RootAddress]; !ok {
			return ErrIdentityNotFound.Wrapf(
				"recovery_requests[%d] targets %s, which has no identity", i, r.RootAddress)
		}
		if _, err := sdk.AccAddressFromBech32(r.NewAddress); err != nil {
			return ErrInvalidRecovery.Wrapf("recovery_requests[%d].new_address: %v", i, err)
		}
		if _, err := PubKeyFromBytes(r.NewRootPubkey, r.NewRootKeyType); err != nil {
			return ErrInvalidKey.Wrapf("recovery_requests[%d]: %v", i, err)
		}
	}

	return nil
}
