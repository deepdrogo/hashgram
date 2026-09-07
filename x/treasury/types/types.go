package types

import (
	"regexp"

	"cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

const (
	// ModuleName is the x/treasury module name.
	ModuleName = "treasury"

	// StoreKey is the primary store key.
	StoreKey = ModuleName

	// QuerierRoute is the legacy querier route.
	QuerierRoute = ModuleName
)

// Store key prefixes.
var (
	ReserveKey         = []byte{0x01}
	DisbursementKey    = []byte{0x02}
	DisbursementSeqKey = []byte{0x03}
)

// Event types and attributes.
const (
	EventTypeDisbursed = "treasury_disbursed"

	AttributeKeyReserve   = "reserve"
	AttributeKeyRecipient = "recipient"
	AttributeKeyAmount    = "amount"
	AttributeKeyMemo      = "memo"
	AttributeKeyRemaining = "remaining"
)

// Reserve names. These are the genesis allocations from docs/TOKENOMICS.md
// other than the Founder's, which is an ordinary vesting account, and the
// useful-service reserve and welcome pool, which their own modules own.
const (
	ReserveTreasury  = "treasury"
	ReserveDevGrants = "dev_grants"
	ReserveLiquidity = "liquidity"
	ReserveGrowth    = "growth"
)

// x/treasury error codes. Code 1 is reserved by the SDK.
var (
	ErrReserveNotFound  = errors.Register(ModuleName, 2, "reserve not found")
	ErrInvalidAuthority = errors.Register(ModuleName, 3,
		"invalid authority; only the governance module may spend from a reserve")
	ErrInvalidReserve      = errors.Register(ModuleName, 4, "invalid reserve")
	ErrInvalidAmount       = errors.Register(ModuleName, 5, "invalid spend amount")
	ErrInsufficientReserve = errors.Register(ModuleName, 6, "reserve does not hold enough to cover this spend")
	ErrBlockedRecipient    = errors.Register(ModuleName, 7, "recipient may not receive funds")
)

// reserveNamePattern keeps reserve names usable as sub-account names and as
// store keys.
var reserveNamePattern = regexp.MustCompile(`^[a-z][a-z0-9_]{1,30}$`)

// SubAccountName returns the module sub-account name for a reserve.
//
// Each reserve gets its own account so that its balance is independently
// queryable. Merging them into one account would make the per-allocation
// figures the specification asks for unverifiable.
func SubAccountName(reserve string) string {
	return ModuleName + "_" + reserve
}

// ReserveAddress returns the deterministic address holding a reserve.
func ReserveAddress(reserve string) sdk.AccAddress {
	return authtypes.NewModuleAddress(SubAccountName(reserve))
}

// Validate checks a reserve record.
func (r Reserve) Validate() error {
	if !reserveNamePattern.MatchString(r.Name) {
		return ErrInvalidReserve.Wrapf(
			"name %q must match %s", r.Name, reserveNamePattern.String())
	}
	if !r.Initial.IsValid() || r.Initial.IsAnyNegative() {
		return ErrInvalidReserve.Wrapf("initial %q is invalid", r.Initial)
	}
	if !r.Spent.IsValid() || r.Spent.IsAnyNegative() {
		return ErrInvalidReserve.Wrapf("spent %q is invalid", r.Spent)
	}
	// Spending more than was funded would describe coins that never entered
	// the reserve.
	if !r.Initial.IsAllGTE(r.Spent) {
		return ErrInvalidReserve.Wrapf(
			"reserve %q records spent %s from an initial %s", r.Name, r.Spent, r.Initial)
	}
	if want := SubAccountName(r.Name); r.SubAccount != want {
		return ErrInvalidReserve.Wrapf(
			"reserve %q records sub_account %q, expected %q", r.Name, r.SubAccount, want)
	}
	return nil
}

// NewReserve builds a reserve record for a whole-HASH amount.
func NewReserve(name, description string, amountHash int64) Reserve {
	return Reserve{
		Name:        name,
		Description: description,
		Initial: sdk.NewCoins(sdk.NewCoin(
			hgparams.BaseCoinDenom, hgparams.HashToBase(amountHash))),
		Spent:      sdk.NewCoins(),
		SubAccount: SubAccountName(name),
	}
}

// DefaultGenesis returns an empty treasury.
//
// Empty rather than pre-funded: reserve amounts belong to the genesis
// distribution table and are written by `hashgramctl init-mainnet-genesis`,
// not defaulted here where a devnet would silently inherit Mainnet numbers.
func DefaultGenesis() *GenesisState {
	return &GenesisState{}
}

// MainnetGenesis returns the Mainnet reserve set.
//
// Amounts come from the compile-time genesis distribution table. The growth
// figure is the growth allocation minus the welcome pool, because the welcome
// pool is funded into x/welcome's own account.
func MainnetGenesis() *GenesisState {
	growthAfterWelcome := hgparams.AllocGrowthHash - hgparams.WelcomePoolHash

	return &GenesisState{
		Reserves: []Reserve{
			NewReserve(ReserveTreasury,
				"Hashgram ecosystem treasury (15% of supply), governed by x/gov",
				hgparams.AllocTreasuryHash),
			NewReserve(ReserveDevGrants,
				"Developer grants (5% of supply), governed by x/gov",
				hgparams.AllocDevGrantsHash),
			NewReserve(ReserveLiquidity,
				"Liquidity and interoperability reserve (5% of supply), governed by x/gov",
				hgparams.AllocLiquidityHash),
			NewReserve(ReserveGrowth,
				"Early user growth pool, the growth allocation less the welcome pool held by x/welcome",
				growthAfterWelcome),
		},
	}
}

// Validate checks the genesis state.
func (gs GenesisState) Validate() error {
	seen := make(map[string]bool, len(gs.Reserves))
	for i, r := range gs.Reserves {
		if err := r.Validate(); err != nil {
			return ErrInvalidReserve.Wrapf("reserves[%d]: %v", i, err)
		}
		if seen[r.Name] {
			return ErrInvalidReserve.Wrapf("reserve %q appears twice", r.Name)
		}
		seen[r.Name] = true
	}

	for i, d := range gs.Disbursements {
		if !seen[d.Reserve] {
			return ErrReserveNotFound.Wrapf(
				"disbursements[%d] references unknown reserve %q", i, d.Reserve)
		}
		if _, err := sdk.AccAddressFromBech32(d.Recipient); err != nil {
			return ErrInvalidAmount.Wrapf("disbursements[%d].recipient: %v", i, err)
		}
		if !d.Amount.IsValid() || d.Amount.IsAnyNegative() {
			return ErrInvalidAmount.Wrapf("disbursements[%d].amount %q is invalid", i, d.Amount)
		}
	}
	return nil
}

// TotalInitial sums every reserve's genesis funding, so a genesis builder can
// check the distribution table adds up.
func (gs GenesisState) TotalInitial() sdk.Coins {
	total := sdk.NewCoins()
	for _, r := range gs.Reserves {
		total = total.Add(r.Initial...)
	}
	return total
}

// SubAccountNames returns every reserve sub-account name, which the
// application needs in order to register them in maccPerms.
func (gs GenesisState) SubAccountNames() []string {
	out := make([]string, 0, len(gs.Reserves))
	for _, r := range gs.Reserves {
		out = append(out, r.SubAccount)
	}
	return out
}

// MainnetSubAccountNames returns the Mainnet reserve sub-account names.
//
// Used by app.go to register the accounts in maccPerms. It has to be a
// compile-time list because maccPerms is built before genesis is read.
func MainnetSubAccountNames() []string {
	return []string{
		SubAccountName(ReserveTreasury),
		SubAccountName(ReserveDevGrants),
		SubAccountName(ReserveLiquidity),
		SubAccountName(ReserveGrowth),
	}
}

var _ sdk.Msg = (*MsgSpend)(nil)

// ValidateBasic performs stateless validation.
func (m MsgSpend) ValidateBasic() error {
	if _, err := sdk.AccAddressFromBech32(m.Authority); err != nil {
		return ErrInvalidAuthority.Wrapf("%q: %v", m.Authority, err)
	}
	if _, err := sdk.AccAddressFromBech32(m.Recipient); err != nil {
		return ErrInvalidAmount.Wrapf("recipient %q: %v", m.Recipient, err)
	}
	if !reserveNamePattern.MatchString(m.Reserve) {
		return ErrInvalidReserve.Wrapf("reserve %q is not a valid name", m.Reserve)
	}
	if !m.Amount.IsValid() || m.Amount.IsAnyNegative() || m.Amount.IsZero() {
		return ErrInvalidAmount.Wrapf("amount %q must be a positive coin set", m.Amount)
	}
	if len(m.Memo) > 512 {
		return ErrInvalidAmount.Wrapf("memo is %d bytes, maximum is 512", len(m.Memo))
	}
	return nil
}
