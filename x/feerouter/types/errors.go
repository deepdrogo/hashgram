package types

import "cosmossdk.io/errors"

// x/feerouter error codes. Code 1 is reserved by the SDK.
var (
	ErrInvalidAuthority = errors.Register(ModuleName, 2,
		"invalid authority; only the governance module may update feerouter params")

	ErrInvalidServiceKind = errors.Register(ModuleName, 3,
		"invalid service kind")

	ErrInvalidFeeAmount = errors.Register(ModuleName, 4,
		"invalid service fee amount")

	// ErrSplitDoesNotBalance means the shares of a routed amount do not sum
	// back to the amount. It indicates a bug in the split arithmetic, and the
	// module refuses to proceed rather than silently creating or destroying
	// value.
	ErrSplitDoesNotBalance = errors.Register(ModuleName, 5,
		"revenue split does not balance")

	ErrParamsNotSet = errors.Register(ModuleName, 6,
		"feerouter params are not set in state")
)
