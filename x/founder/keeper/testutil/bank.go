// Package testutil provides an in-memory bank and account keeper for testing
// the Hashgram revenue modules.
//
// A hand-written double rather than a generated mock: these tests are about
// coin conservation, and a fake that actually moves balances between accounts
// can be asserted against. A mock that records calls cannot tell you whether
// the books balance.
package testutil

import (
	"context"
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
)

// Bank is an in-memory implementation of the bank behaviour the revenue
// modules rely on.
//
// It enforces the properties that matter for these tests:
//   - transfers debit the sender and credit the recipient,
//   - an overdraft fails rather than producing a negative balance,
//   - there is no mint function, so total supply can be asserted constant.
type Bank struct {
	balances map[string]sdk.Coins
	blocked  map[string]bool
}

// NewBank returns an empty in-memory bank.
func NewBank() *Bank {
	return &Bank{
		balances: make(map[string]sdk.Coins),
		blocked:  make(map[string]bool),
	}
}

// ModuleAddress returns the deterministic address of a module account.
func ModuleAddress(name string) sdk.AccAddress {
	return authtypes.NewModuleAddress(name)
}

// Fund credits an address without a counterparty.
//
// This is the only way coins enter the fake bank, and it exists solely so a
// test can establish a starting state. Production code has no equivalent:
// there is no minting module on Hashgram.
func (b *Bank) Fund(addr sdk.AccAddress, amt sdk.Coins) {
	b.balances[addr.String()] = b.balances[addr.String()].Add(amt...)
}

// FundModule credits a module account.
func (b *Bank) FundModule(name string, amt sdk.Coins) {
	b.Fund(ModuleAddress(name), amt)
}

// Block marks an address as unable to receive funds.
func (b *Bank) Block(addr sdk.AccAddress) { b.blocked[addr.String()] = true }

// TotalSupply sums every balance. Tests assert this is invariant across
// routing, which is what proves the split neither creates nor destroys HASH.
func (b *Bank) TotalSupply() sdk.Coins {
	total := sdk.NewCoins()
	for _, c := range b.balances {
		total = total.Add(c...)
	}
	return total
}

// --- BankKeeper interface ---------------------------------------------------

// GetAllBalances returns an address's balance.
func (b *Bank) GetAllBalances(_ context.Context, addr sdk.AccAddress) sdk.Coins {
	c := b.balances[addr.String()]
	if c == nil {
		return sdk.NewCoins()
	}
	return c
}

// BlockedAddr reports whether an address may not receive funds.
func (b *Bank) BlockedAddr(addr sdk.AccAddress) bool { return b.blocked[addr.String()] }

// SendCoinsFromModuleToAccount moves coins from a module account to an
// address.
func (b *Bank) SendCoinsFromModuleToAccount(_ context.Context, senderModule string, recipient sdk.AccAddress, amt sdk.Coins) error {
	if b.blocked[recipient.String()] {
		return fmt.Errorf("%s is not allowed to receive funds", recipient)
	}
	return b.move(ModuleAddress(senderModule), recipient, amt)
}

// SendCoinsFromModuleToModule moves coins between module accounts.
func (b *Bank) SendCoinsFromModuleToModule(_ context.Context, senderModule, recipientModule string, amt sdk.Coins) error {
	return b.move(ModuleAddress(senderModule), ModuleAddress(recipientModule), amt)
}

// SendCoinsFromAccountToModule moves coins from an address to a module
// account.
func (b *Bank) SendCoinsFromAccountToModule(_ context.Context, sender sdk.AccAddress, recipientModule string, amt sdk.Coins) error {
	return b.move(sender, ModuleAddress(recipientModule), amt)
}

// SendCoins moves coins between two addresses, used to model a plain user
// transfer in the "sending 100 HASH delivers 100 HASH" test.
func (b *Bank) SendCoins(_ context.Context, from, to sdk.AccAddress, amt sdk.Coins) error {
	if b.blocked[to.String()] {
		return fmt.Errorf("%s is not allowed to receive funds", to)
	}
	return b.move(from, to, amt)
}

func (b *Bank) move(from, to sdk.AccAddress, amt sdk.Coins) error {
	if amt.IsZero() {
		return nil
	}
	src := b.GetAllBalances(nil, from)
	remaining, negative := src.SafeSub(amt...)
	if negative {
		return fmt.Errorf("insufficient funds: %s has %s, needs %s", from, src, amt)
	}
	b.balances[from.String()] = remaining
	b.balances[to.String()] = b.GetAllBalances(nil, to).Add(amt...)
	return nil
}

// --- AccountKeeper interface ------------------------------------------------

// Accounts implements the account-keeper surface the revenue modules use.
type Accounts struct {
	// Registered is the set of module names that have an account. A module
	// that is not registered returns a nil address, which the keepers treat
	// as a fatal wiring error.
	Registered map[string]bool
}

// NewAccounts returns an account keeper with the given modules registered.
func NewAccounts(modules ...string) *Accounts {
	a := &Accounts{Registered: make(map[string]bool, len(modules))}
	for _, m := range modules {
		a.Registered[m] = true
	}
	return a
}

// GetModuleAddress returns a module account address, or nil if the module was
// never registered.
func (a *Accounts) GetModuleAddress(name string) sdk.AccAddress {
	if !a.Registered[name] {
		return nil
	}
	return ModuleAddress(name)
}
