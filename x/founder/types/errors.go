package types

import "cosmossdk.io/errors"

// x/founder error codes. Code 1 is reserved by the SDK.
var (
	ErrInvalidBeneficiary = errors.Register(ModuleName, 2,
		"invalid founder beneficiary address")

	// ErrFeeExceedsCeiling is returned when a parameter update tries to raise
	// the Founder share above the compile-time ceiling. Raising it requires a
	// new binary adopted by the validator set, not a governance vote alone.
	ErrFeeExceedsCeiling = errors.Register(ModuleName, 3,
		"founder fee exceeds the compile-time ceiling")

	ErrInvalidAuthority = errors.Register(ModuleName, 4,
		"invalid authority; only the governance module may update founder params")

	ErrNothingToClaim = errors.Register(ModuleName, 5,
		"no founder revenue is pending")

	ErrBeneficiaryNotSet = errors.Register(ModuleName, 6,
		"founder beneficiary is not configured")

	ErrInvalidMinPayout = errors.Register(ModuleName, 7,
		"invalid minimum payout")

	ErrParamsNotSet = errors.Register(ModuleName, 8,
		"founder params are not set in state")
)
