package types

import "cosmossdk.io/errors"

// x/username error codes. Code 1 is reserved by the SDK.
var (
	ErrInvalidName = errors.Register(ModuleName, 2,
		"invalid username")

	ErrNameTaken = errors.Register(ModuleName, 3,
		"username is already registered")

	ErrNameReserved = errors.Register(ModuleName, 4,
		"username is reserved")

	// ErrConfusable rejects a name that renders similarly enough to an
	// existing one to be usable for impersonation.
	ErrConfusable = errors.Register(ModuleName, 5,
		"username is visually confusable with an existing registration")

	ErrNameNotFound = errors.Register(ModuleName, 6,
		"username is not registered")

	ErrNotOwner = errors.Register(ModuleName, 7,
		"caller does not own this username")

	ErrNotTransferable = errors.Register(ModuleName, 8,
		"username is locked against transfer")

	ErrInvalidParams = errors.Register(ModuleName, 9,
		"invalid username params")

	ErrParamsNotSet = errors.Register(ModuleName, 10,
		"username params are not set in state")

	ErrInvalidAuthority = errors.Register(ModuleName, 11,
		"invalid authority; only the governance module may update username params")

	ErrGracePeriod = errors.Register(ModuleName, 12,
		"username has expired but is within the previous owner's grace period")
)
