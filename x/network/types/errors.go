package types

import "cosmossdk.io/errors"

// x/network error codes. Code 1 is reserved by the SDK for internal errors.
var (
	ErrNetworkInfoNotSet = errors.Register(ModuleName, 2,
		"network info is not set in state")

	ErrInvalidNetworkInfo = errors.Register(ModuleName, 3,
		"invalid network info")

	ErrChainIDMismatch = errors.Register(ModuleName, 4,
		"genesis network chain_id does not match the CometBFT chain-id")

	ErrNetworkInfoImmutable = errors.Register(ModuleName, 5,
		"network info is immutable after genesis")

	ErrUnknownSigningPurpose = errors.Register(ModuleName, 6,
		"unknown signing purpose")

	ErrForkIsolationWeakened = errors.Register(ModuleName, 7,
		"fork isolation checks cannot be disabled on this network")
)
