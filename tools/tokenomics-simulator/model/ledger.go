// Package model simulates Hashgram token flows over time.
//
// The defining constraint of this package is that it does not reimplement the
// chain's arithmetic. It imports the same functions the state machine runs:
//
//	x/serviceproof/types.EmissionForEpoch    reward budget per epoch
//	x/serviceproof/types.ProviderRewardCap   per-provider ceiling
//	x/welcome/types.TierAmountHash           welcome reward schedule
//	x/founder/types.Params.FounderCut        the 1% protocol revenue share
//	app/params                               supply and allocation constants
//
// A simulator that models a different schedule from the chain is worse than
// no simulator, because it produces numbers that look authoritative and are
// not. Where this package does make an assumption the chain cannot supply
// (how many users join, how many transactions they send, what a transaction
// costs), that assumption is an explicit input and is printed alongside the
// results.
package model

import (
	"fmt"
	"sort"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// Bucket names. Every uhash in the simulation sits in exactly one bucket at
// all times, and the simulation only ever moves coins between buckets. There
// is no mint and no burn, matching a chain built without x/mint.
const (
	// BucketFounderLocked is the Founder's PeriodicVestingAccount balance
	// that has not yet vested.
	BucketFounderLocked = "founder_locked"
	// BucketFounderLiquid is Founder coins that are spendable: the genesis
	// unlocked portion plus everything vested so far plus revenue paid out.
	BucketFounderLiquid = "founder_liquid"
	// BucketServiceReserve is the finite useful-service reward reserve.
	BucketServiceReserve = "service_reserve"
	// BucketTreasury is the governance-controlled ecosystem treasury.
	BucketTreasury = "treasury"
	// BucketDevGrants is the governance-controlled developer grant pool.
	BucketDevGrants = "dev_grants"
	// BucketLiquidity is the liquidity / interoperability reserve.
	BucketLiquidity = "liquidity"
	// BucketGrowth is the growth allocation, which the Welcome pool is
	// carved out of.
	BucketGrowth = "growth"
	// BucketUsers is HASH held by end users.
	BucketUsers = "users"
	// BucketProviders is HASH earned by useful-service node operators.
	BucketProviders = "providers"
	// BucketValidators is HASH earned by validators and their delegators.
	BucketValidators = "validators"
)

// bucketOrder fixes the report and iteration order. Go map iteration is
// randomised, and a simulator whose output reorders between runs is a
// simulator nobody can diff.
var bucketOrder = []string{
	BucketFounderLocked,
	BucketFounderLiquid,
	BucketServiceReserve,
	BucketTreasury,
	BucketDevGrants,
	BucketLiquidity,
	BucketGrowth,
	BucketUsers,
	BucketProviders,
	BucketValidators,
}

// Ledger holds the whole simulated supply, partitioned into buckets.
type Ledger struct {
	balances map[string]math.Int

	// welcomePaid tracks how much of the Welcome pool has been spent. The
	// Welcome pool is a carve-out of BucketGrowth rather than a separate
	// allocation, so it needs its own counter to be capped correctly.
	welcomePaid math.Int

	// total is the invariant: it is set once at genesis and every operation
	// asserts against it.
	total math.Int
}

// NewGenesisLedger builds the ledger from the compile-time genesis allocation
// table, so the simulation starts from the same distribution the chain does.
func NewGenesisLedger() (*Ledger, error) {
	l := &Ledger{
		balances:    make(map[string]math.Int, len(bucketOrder)),
		welcomePaid: math.ZeroInt(),
	}
	for _, name := range bucketOrder {
		l.balances[name] = math.ZeroInt()
	}

	// Founder splits across two buckets; the rest map one-to-one onto the
	// genesis allocation table.
	l.balances[BucketFounderLocked] = hgparams.HashToBase(hgparams.FounderVestedHash)
	l.balances[BucketFounderLiquid] = hgparams.HashToBase(hgparams.FounderUnlockedAtGenesisHash)
	l.balances[BucketServiceReserve] = hgparams.HashToBase(hgparams.AllocServiceReserveHash)
	l.balances[BucketTreasury] = hgparams.HashToBase(hgparams.AllocTreasuryHash)
	l.balances[BucketDevGrants] = hgparams.HashToBase(hgparams.AllocDevGrantsHash)
	l.balances[BucketLiquidity] = hgparams.HashToBase(hgparams.AllocLiquidityHash)
	l.balances[BucketGrowth] = hgparams.HashToBase(hgparams.AllocGrowthHash)

	l.total = l.sum()

	// The genesis buckets must reconstruct the canonical supply exactly. If
	// they do not, either the allocation table or this mapping is wrong, and
	// every number produced downstream would be quietly off.
	if want := hgparams.MaxSupplyBase(); !l.total.Equal(want) {
		return nil, fmt.Errorf(
			"genesis buckets sum to %s uhash but canonical supply is %s uhash",
			l.total, want)
	}
	return l, nil
}

// Get returns a bucket balance in uhash.
func (l *Ledger) Get(bucket string) math.Int {
	if v, ok := l.balances[bucket]; ok {
		return v
	}
	return math.ZeroInt()
}

// Total returns the invariant supply.
func (l *Ledger) Total() math.Int { return l.total }

// WelcomePaid returns how much of the Welcome pool has been spent.
func (l *Ledger) WelcomePaid() math.Int { return l.welcomePaid }

// Buckets returns the bucket names in report order.
func Buckets() []string {
	out := make([]string, len(bucketOrder))
	copy(out, bucketOrder)
	return out
}

// Move transfers amount from one bucket to another.
//
// It refuses to overdraw rather than clamping. A simulation that silently
// pays out coins a bucket does not hold is modelling a chain that mints, and
// the whole point of this tool is to show that Hashgram cannot.
func (l *Ledger) Move(from, to string, amount math.Int) error {
	if amount.IsNegative() {
		return fmt.Errorf("cannot move a negative amount (%s) from %s to %s", amount, from, to)
	}
	if amount.IsZero() {
		return nil
	}

	src, ok := l.balances[from]
	if !ok {
		return fmt.Errorf("unknown source bucket %q", from)
	}
	dst, ok := l.balances[to]
	if !ok {
		return fmt.Errorf("unknown destination bucket %q", to)
	}
	if src.LT(amount) {
		return fmt.Errorf(
			"bucket %q holds %s uhash, cannot move %s uhash to %q",
			from, src, amount, to)
	}

	l.balances[from] = src.Sub(amount)
	l.balances[to] = dst.Add(amount)
	return nil
}

// MoveUpTo transfers as much as the source bucket can afford, up to amount,
// and reports what actually moved.
//
// Used where partial settlement is the honest model: if user balances cannot
// cover the assumed fee volume, the correct output is "fees were limited by
// what users held", not an error and not a fabricated payment.
func (l *Ledger) MoveUpTo(from, to string, amount math.Int) (math.Int, error) {
	if amount.IsNegative() {
		return math.ZeroInt(), fmt.Errorf("cannot move a negative amount (%s)", amount)
	}
	available := l.Get(from)
	if available.LT(amount) {
		amount = available
	}
	if err := l.Move(from, to, amount); err != nil {
		return math.ZeroInt(), err
	}
	return amount, nil
}

// PayWelcome moves a welcome reward from the growth allocation to users,
// enforcing the Welcome pool cap that x/welcome enforces on chain.
//
// Returns what was actually paid, which is zero once the pool is exhausted.
func (l *Ledger) PayWelcome(amount math.Int) (math.Int, error) {
	if !amount.IsPositive() {
		return math.ZeroInt(), nil
	}

	poolCap := hgparams.HashToBase(hgparams.WelcomePoolHash)
	remaining := poolCap.Sub(l.welcomePaid)
	if !remaining.IsPositive() {
		return math.ZeroInt(), nil
	}
	if amount.GT(remaining) {
		amount = remaining
	}

	paid, err := l.MoveUpTo(BucketGrowth, BucketUsers, amount)
	if err != nil {
		return math.ZeroInt(), err
	}
	l.welcomePaid = l.welcomePaid.Add(paid)
	return paid, nil
}

// ServiceReserveCoins expresses the reserve as sdk.Coins so it can be passed
// straight into the chain's own EmissionForEpoch.
func (l *Ledger) ServiceReserveCoins() sdk.Coins {
	amt := l.Get(BucketServiceReserve)
	if !amt.IsPositive() {
		return sdk.NewCoins()
	}
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, amt))
}

// AssertInvariant checks that no operation created or destroyed supply.
//
// Called after every simulated epoch. This is the single most important
// assertion in the tool: it is the property the tokenomics claim rests on.
func (l *Ledger) AssertInvariant() error {
	got := l.sum()
	if !got.Equal(l.total) {
		return fmt.Errorf(
			"supply invariant broken: buckets sum to %s uhash, expected %s uhash (delta %s)",
			got, l.total, got.Sub(l.total))
	}
	if got.GT(hgparams.MaxSupplyBase()) {
		return fmt.Errorf(
			"supply %s uhash exceeds the canonical ceiling of %s uhash",
			got, hgparams.MaxSupplyBase())
	}
	for _, name := range bucketOrder {
		if l.balances[name].IsNegative() {
			return fmt.Errorf("bucket %q went negative: %s", name, l.balances[name])
		}
	}
	return nil
}

// Snapshot returns a copy of all bucket balances, for reporting.
func (l *Ledger) Snapshot() map[string]math.Int {
	out := make(map[string]math.Int, len(l.balances))
	for k, v := range l.balances {
		out[k] = v
	}
	return out
}

func (l *Ledger) sum() math.Int {
	names := make([]string, 0, len(l.balances))
	for name := range l.balances {
		names = append(names, name)
	}
	// Sorted so the addition order is fixed. math.Int addition is exact, so
	// this does not change the result, but it keeps the function reproducible
	// under any future change to a non-exact accumulator.
	sort.Strings(names)

	total := math.ZeroInt()
	for _, name := range names {
		total = total.Add(l.balances[name])
	}
	return total
}
