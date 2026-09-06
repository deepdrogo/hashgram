package types

const (
	// ModuleName is the x/network module name.
	ModuleName = "network"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
//
// x/network holds two singletons and nothing else. Using explicit single-byte
// prefixes rather than string keys keeps the store layout stable and cheap to
// reason about during a migration.
var (
	// NetworkInfoKey stores the immutable NetworkInfo singleton.
	NetworkInfoKey = []byte{0x01}

	// ForkIsolationKey stores the ForkIsolationPolicy singleton.
	ForkIsolationKey = []byte{0x02}
)
