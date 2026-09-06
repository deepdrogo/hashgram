package types

import "cosmossdk.io/errors"

// x/serviceproof error codes. Code 1 is reserved by the SDK.
var (
	ErrDisabled            = errors.Register(ModuleName, 2, "the useful-service reward system is not enabled")
	ErrProviderExists      = errors.Register(ModuleName, 3, "provider is already registered")
	ErrProviderNotFound    = errors.Register(ModuleName, 4, "provider is not registered")
	ErrInsufficientBond    = errors.Register(ModuleName, 5, "bond is below the required minimum")
	ErrProviderJailed      = errors.Register(ModuleName, 6, "provider is jailed")
	ErrJailPeriodActive    = errors.Register(ModuleName, 7, "jail period has not elapsed")
	ErrUnbonding           = errors.Register(ModuleName, 8, "provider is unbonding")
	ErrNotUnbonding        = errors.Register(ModuleName, 9, "provider has not begun unbonding")
	ErrUnbondingIncomplete = errors.Register(ModuleName, 10, "unbonding period has not elapsed")
	ErrInvalidRole         = errors.Register(ModuleName, 11, "invalid or unspecified service role")
	ErrRoleNotOffered      = errors.Register(ModuleName, 12, "provider does not offer this service role")

	// ErrSelfTraffic rejects a provider serving itself. This is the trivial
	// version of the fake-traffic attack and is rejected outright rather than
	// merely discounted.
	ErrSelfTraffic = errors.Register(ModuleName, 13,
		"receipt client key belongs to the provider itself")

	ErrReceiptReplay      = errors.Register(ModuleName, 14, "receipt nonce has already been used")
	ErrReceiptExpired     = errors.Register(ModuleName, 15, "receipt has expired")
	ErrReceiptTooLarge    = errors.Register(ModuleName, 16, "receipt claims more units than permitted")
	ErrEpochSettled       = errors.Register(ModuleName, 17, "epoch is already settled; evidence must arrive before settlement")
	ErrEpochNotFound      = errors.Register(ModuleName, 18, "epoch not found")
	ErrInvalidSignature   = errors.Register(ModuleName, 19, "signature is invalid")
	ErrInvalidKey         = errors.Register(ModuleName, 20, "invalid public key")
	ErrChallengeNotFound  = errors.Register(ModuleName, 21, "challenge not found")
	ErrChallengeAnswered  = errors.Register(ModuleName, 22, "challenge has already been answered")
	ErrChallengeExpired   = errors.Register(ModuleName, 23, "challenge deadline has passed")
	ErrInvalidMerkleProof = errors.Register(ModuleName, 24,
		"merkle proof does not verify against the assignment root")
	ErrAssignmentExists   = errors.Register(ModuleName, 25, "storage assignment already exists")
	ErrAssignmentNotFound = errors.Register(ModuleName, 26, "storage assignment not found")
	ErrNotAnAssigner      = errors.Register(ModuleName, 27,
		"sender is not a registered storage assigner; a provider cannot assign work to itself")
	ErrInvalidAssignment = errors.Register(ModuleName, 28, "invalid storage assignment")
	ErrCapacityExceeded  = errors.Register(ModuleName, 29,
		"assignment would exceed the provider's declared storage capacity")
	ErrInvalidAuthority = errors.Register(ModuleName, 30,
		"invalid authority; only the governance module may update serviceproof params")
	ErrInvalidParams    = errors.Register(ModuleName, 31, "invalid serviceproof params")
	ErrParamsNotSet     = errors.Register(ModuleName, 32, "serviceproof params are not set in state")
	ErrReserveExhausted = errors.Register(ModuleName, 33, "the useful-service reserve is exhausted")

	// ErrReserveOverspend indicates the emission schedule tried to pay out
	// more than the reserve ever held. It should be unreachable; the module
	// refuses to settle rather than continue with books that do not balance.
	ErrReserveOverspend = errors.Register(ModuleName, 34,
		"settlement would emit more than the reserve's initial balance")

	ErrInvalidReceipt = errors.Register(ModuleName, 35, "invalid service receipt")
	ErrNoBond         = errors.Register(ModuleName, 36, "provider has no bond to withdraw")
)
