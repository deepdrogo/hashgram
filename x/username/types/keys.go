package types

const (
	// ModuleName is the x/username module name.
	ModuleName = "username"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	// ParamsKey stores the Params singleton.
	ParamsKey = []byte{0x01}

	// RegistrationKey maps a normalised name to its Registration.
	RegistrationKey = []byte{0x02}

	// SkeletonKey maps a confusable skeleton to the name that claimed it.
	// This makes the confusable-collision check an index lookup rather than a
	// scan over every registration.
	SkeletonKey = []byte{0x03}

	// OwnerIndexKey maps (owner, name) so that reverse lookup is also an
	// index scan rather than a full walk.
	OwnerIndexKey = []byte{0x04}
)

// Event types and attributes.
const (
	EventTypeRegistered  = "username_registered"
	EventTypeRenewed     = "username_renewed"
	EventTypeTransferred = "username_transferred"
	EventTypeReleased    = "username_released"

	AttributeKeyName     = "name"
	AttributeKeyOwner    = "owner"
	AttributeKeyNewOwner = "new_owner"
	AttributeKeyExpiry   = "expiry_height"
	AttributeKeyFee      = "fee"
)
