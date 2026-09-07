package types

import "cosmossdk.io/errors"

// x/identity error codes. Code 1 is reserved by the SDK.
var (
	ErrIdentityExists   = errors.Register(ModuleName, 2, "an identity already exists for this address")
	ErrIdentityNotFound = errors.Register(ModuleName, 3, "no identity exists for this address")
	ErrIdentityRevoked  = errors.Register(ModuleName, 4, "identity has been revoked")

	ErrInvalidKey = errors.Register(ModuleName, 5, "invalid public key")

	// ErrInvalidCertificate rejects a device certificate that is not signed
	// by the identity's current root key. Paying the gas for a transaction is
	// not authority to insert a device.
	ErrInvalidCertificate = errors.Register(ModuleName, 6,
		"device certificate is not signed by the identity's current root key")

	ErrCertificateExpired = errors.Register(ModuleName, 7, "device certificate has expired")
	ErrCertificateTooLong = errors.Register(ModuleName, 8,
		"device certificate expiry is further in the future than the protocol permits")

	// ErrRotationMismatch rejects a certificate issued under a superseded
	// root key. This is what makes root-key rotation meaningful.
	ErrRotationMismatch = errors.Register(ModuleName, 9,
		"device certificate was issued under a superseded root key")

	ErrDeviceExists     = errors.Register(ModuleName, 10, "a device with this id already exists")
	ErrDeviceNotFound   = errors.Register(ModuleName, 11, "device not found")
	ErrDeviceRevoked    = errors.Register(ModuleName, 12, "device has been revoked")
	ErrDeviceKeyInUse   = errors.Register(ModuleName, 13, "this device public key is already registered")
	ErrTooManyDevices   = errors.Register(ModuleName, 14, "identity has reached its device limit")
	ErrInvalidDeviceID  = errors.Register(ModuleName, 15, "invalid device id")
	ErrLastDeviceRemove = errors.Register(ModuleName, 16,
		"cannot revoke the only remaining device; add a replacement first")

	ErrInvalidRotationSignature = errors.Register(ModuleName, 17,
		"root key rotation is not signed by the key being replaced")

	ErrRecoveryDisabled    = errors.Register(ModuleName, 18, "social recovery is not configured for this identity")
	ErrNotAGuardian        = errors.Register(ModuleName, 19, "sender is not a guardian of this identity")
	ErrRecoveryExists      = errors.Register(ModuleName, 20, "a recovery request is already open")
	ErrRecoveryNotFound    = errors.Register(ModuleName, 21, "no recovery request is open")
	ErrRecoveryCancelled   = errors.Register(ModuleName, 22, "recovery request has been cancelled")
	ErrAlreadyApproved     = errors.Register(ModuleName, 23, "guardian has already approved")
	ErrThresholdNotMet     = errors.Register(ModuleName, 24, "recovery approval threshold has not been met")
	ErrRecoveryDelayActive = errors.Register(ModuleName, 25, "recovery delay has not elapsed")
	ErrInvalidRecovery     = errors.Register(ModuleName, 26, "invalid recovery configuration")
	ErrInvalidParams       = errors.Register(ModuleName, 27, "invalid identity params")
	ErrParamsNotSet        = errors.Register(ModuleName, 28, "identity params are not set in state")
	ErrNotIdentityOwner    = errors.Register(ModuleName, 29, "sender does not administer this identity")
	ErrTargetAddressInUse  = errors.Register(ModuleName, 30, "the recovery target address already has an identity")
)
