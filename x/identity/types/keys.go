package types

const (
	// ModuleName is the x/identity module name.
	ModuleName = "identity"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	// ParamsKey stores the Params singleton.
	ParamsKey = []byte{0x01}

	// IdentityKey maps an address to its RootIdentity.
	IdentityKey = []byte{0x02}

	// DeviceKey maps (root address, device id) to a Device.
	DeviceKey = []byte{0x03}

	// DeviceKeyIndex maps a device public key to its (root address, device
	// id). This is what makes "which identity signed this event?" an index
	// lookup rather than a scan over every identity's devices.
	DeviceKeyIndex = []byte{0x04}

	// RecoveryKey maps a root address to its in-progress RecoveryRequest.
	RecoveryKey = []byte{0x05}
)

// Event types and attributes.
const (
	EventTypeIdentityCreated  = "identity_created"
	EventTypeDeviceAdded      = "device_added"
	EventTypeDeviceRevoked    = "device_revoked"
	EventTypeRootKeyRotated   = "root_key_rotated"
	EventTypeRecoveryOpened   = "recovery_initiated"
	EventTypeRecoveryApproved = "recovery_approved"
	EventTypeRecoveryCanceled = "recovery_cancelled"
	EventTypeRecoveryExecuted = "recovery_executed"
	EventTypeIdentityRevoked  = "identity_revoked"

	AttributeKeyAddress    = "address"
	AttributeKeyDeviceID   = "device_id"
	AttributeKeyLabel      = "label"
	AttributeKeyPlatform   = "platform"
	AttributeKeyRotation   = "rotation_count"
	AttributeKeyGuardian   = "guardian"
	AttributeKeyApprovals  = "approvals"
	AttributeKeyThreshold  = "threshold"
	AttributeKeyNewAddress = "new_address"
	AttributeKeyRevoked    = "devices_revoked"
)
