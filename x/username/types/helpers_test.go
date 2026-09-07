package types_test

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// testOwner returns a valid DEVNET-only address, derived from label bytes so
// that nothing in the repository looks like an address with a key behind it.
func testOwner() string {
	b := make([]byte, 20)
	copy(b, "DEVNET-owner")
	return sdk.AccAddress(b).String()
}
