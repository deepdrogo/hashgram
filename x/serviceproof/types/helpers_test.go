package types_test

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// testProviderAddress returns a valid DEVNET-only bech32 address for tests.
//
// Derived from label bytes rather than hardcoded, so it is always a valid
// checksum and so nothing in the repository looks like an address that might
// have a key behind it. It has no key at all.
func testProviderAddress() string {
	b := make([]byte, 20)
	copy(b, "DEVNET-provider")
	return sdk.AccAddress(b).String()
}
