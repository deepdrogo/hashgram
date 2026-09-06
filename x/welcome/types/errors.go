package types

import "cosmossdk.io/errors"

// x/welcome error codes. Code 1 is reserved by the SDK.
var (
	ErrDisabled = errors.Register(ModuleName, 2,
		"the welcome programme is not enabled")

	// ErrAlreadyClaimed is the double-claim guard. It is keyed on the
	// subject address, so one address receives at most one welcome reward for
	// the lifetime of the chain.
	ErrAlreadyClaimed = errors.Register(ModuleName, 3,
		"this address has already received a welcome reward")

	ErrUnknownAttestor = errors.Register(ModuleName, 4,
		"attestation is signed by an attestor that is not registered")

	ErrAttestorDisabled = errors.Register(ModuleName, 5,
		"attestation is signed by a suspended attestor")

	// ErrNonceReused is the replay guard. Each (attestor, nonce) pair may be
	// consumed once.
	ErrNonceReused = errors.Register(ModuleName, 6,
		"attestation nonce has already been used")

	ErrAttestationExpired = errors.Register(ModuleName, 7,
		"attestation has expired")

	ErrAttestationTooLong = errors.Register(ModuleName, 8,
		"attestation expiry is further in the future than the protocol permits")

	ErrInvalidSignature = errors.Register(ModuleName, 9,
		"attestation signature is invalid")

	ErrConfidenceTooLow = errors.Register(ModuleName, 10,
		"attestation confidence is below the required minimum")

	ErrMethodMismatch = errors.Register(ModuleName, 11,
		"attestation method does not match the registered attestor's method")

	// ErrExhausted is returned once the sequence has passed the last funded
	// tier. It is not an error condition in the network: it means the
	// programme finished.
	ErrExhausted = errors.Register(ModuleName, 12,
		"the welcome programme is exhausted")

	// ErrPoolInsufficient means the pool cannot cover the tier amount. It
	// should be unreachable, because the pool is funded with the schedule's
	// worst-case total, and it is checked anyway.
	ErrPoolInsufficient = errors.Register(ModuleName, 13,
		"the welcome pool cannot cover this reward")

	ErrAttestorCapReached = errors.Register(ModuleName, 14,
		"attestor has reached its claim cap for this epoch")

	ErrInvalidAuthority = errors.Register(ModuleName, 15,
		"invalid authority; only the governance module may update welcome params")

	ErrInvalidAttestor = errors.Register(ModuleName, 16,
		"invalid attestor registration")

	ErrInvalidParams = errors.Register(ModuleName, 17,
		"invalid welcome params")

	ErrParamsNotSet = errors.Register(ModuleName, 18,
		"welcome params are not set in state")

	ErrInvalidAttestation = errors.Register(ModuleName, 19,
		"invalid attestation")
)
