// Package params holds the immutable protocol constants that define Hashgram
// Mainnet.
//
// Everything in this file is deliberately a compile-time constant rather than
// an on-chain parameter. A value that can be changed by a transaction is a
// value an attacker (or a careless operator) can change. The canonical supply
// ceiling, the denomination, the address prefixes and the network identity are
// all fixed here so that changing them requires shipping a new binary that the
// validator set must consciously adopt.
//
// See docs/TOKENOMICS.md and docs/DECENTRALIZATION.md.
package params

import (
	"fmt"

	"cosmossdk.io/math"
)

// ---------------------------------------------------------------------------
// Coin
// ---------------------------------------------------------------------------

const (
	// HumanCoinDenom is the display denomination shown to users.
	HumanCoinDenom = "HASH"

	// BaseCoinDenom is the smallest indivisible unit and the denomination used
	// everywhere inside the state machine.
	BaseCoinDenom = "uhash"

	// CoinDecimals is the number of decimal places between BaseCoinDenom and
	// HumanCoinDenom: 1 HASH = 10^6 uhash.
	CoinDecimals = 6
)

// MicroUnit is the number of uhash in one HASH.
const MicroUnit int64 = 1_000_000

// ---------------------------------------------------------------------------
// Supply
// ---------------------------------------------------------------------------

// MaxSupplyHash is the maximum canonical HASH supply. It can never be exceeded:
// there is no inflation, no administrative mint and no bridge that can create
// canonical HASH. Every uhash that will ever exist is created in the genesis
// block and thereafter only moves between accounts.
const MaxSupplyHash int64 = 1_000_000_000

// ---------------------------------------------------------------------------
// Genesis allocation (docs/TOKENOMICS.md)
// ---------------------------------------------------------------------------

const (
	// AllocFounderHash is the Founder allocation: 20% of max supply.
	AllocFounderHash int64 = 200_000_000

	// AllocServiceReserveHash funds Proof-of-Useful-Service rewards: 50%.
	// This reserve is finite. When it is exhausted, useful-service node
	// operators are paid from real protocol service-fee revenue only.
	AllocServiceReserveHash int64 = 500_000_000

	// AllocTreasuryHash is the ecosystem treasury, governed by x/gov: 15%.
	AllocTreasuryHash int64 = 150_000_000

	// AllocGrowthHash funds early-user growth including the Welcome pool: 5%.
	AllocGrowthHash int64 = 50_000_000

	// AllocDevGrantsHash funds developer grants, governed by x/gov: 5%.
	AllocDevGrantsHash int64 = 50_000_000

	// AllocLiquidityHash is the liquidity / interoperability reserve: 5%.
	AllocLiquidityHash int64 = 50_000_000
)

// ---------------------------------------------------------------------------
// Founder vesting (docs/TOKENOMICS.md, docs/FOUNDER_LAUNCH_RUNBOOK.md)
// ---------------------------------------------------------------------------

const (
	// FounderUnlockedAtGenesisHash is immediately spendable at genesis.
	FounderUnlockedAtGenesisHash int64 = 20_000_000

	// FounderVestedHash is released linearly in monthly periods over
	// FounderVestingYears.
	FounderVestedHash int64 = 180_000_000

	// FounderVestingYears is the vesting horizon for FounderVestedHash.
	FounderVestingYears int64 = 8

	// FounderVestingPeriods is the number of discrete vesting periods
	// (monthly over FounderVestingYears).
	FounderVestingPeriods int64 = FounderVestingYears * 12
)

// ---------------------------------------------------------------------------
// Founder protocol revenue share (docs/TOKENOMICS.md §Founder revenue)
// ---------------------------------------------------------------------------

const (
	// FounderFeeBasisPoints is the Founder share of *qualifying protocol
	// service fee revenue*, expressed in basis points. 100 bps = 1%.
	//
	// IMPORTANT: this is NOT a tax on transferred principal. If Alice sends
	// Bob 100 HASH, Bob receives 100 HASH. The Founder share applies only to
	// fees the protocol itself has already collected as revenue.
	FounderFeeBasisPoints uint32 = 100

	// MaxFounderFeeBasisPoints is a hard compile-time ceiling on
	// FounderFeeBasisPoints. Governance cannot raise the Founder share above
	// this value; doing so requires a binary upgrade adopted by the validator
	// set. This exists so that "the Founder quietly raised their cut" is not
	// representable in the state machine.
	MaxFounderFeeBasisPoints uint32 = 100

	// BasisPointDenominator is the basis-point scale (10_000 bps = 100%).
	BasisPointDenominator uint32 = 10_000
)

// ---------------------------------------------------------------------------
// Welcome rewards (docs/TOKENOMICS.md §Welcome HASH)
// ---------------------------------------------------------------------------

const (
	// WelcomeTier1Limit is the last eligible sequence number receiving
	// WelcomeTier1Hash.
	WelcomeTier1Limit uint64 = 10_000
	WelcomeTier2Limit uint64 = 100_000
	WelcomeTier3Limit uint64 = 1_000_000

	WelcomeTier1Hash int64 = 50
	WelcomeTier2Hash int64 = 5
	WelcomeTier3Hash int64 = 1

	// WelcomePoolHash is the maximum HASH the Welcome module can ever pay out:
	//   10_000 * 50 +  90_000 * 5 + 900_000 * 1
	// = 500_000    + 450_000     + 900_000
	// = 1_850_000
	// It is carved out of AllocGrowthHash.
	WelcomePoolHash int64 = 1_850_000
)

// ---------------------------------------------------------------------------
// Bech32 address prefixes
// ---------------------------------------------------------------------------

const (
	// Bech32Prefix is the human-readable part of every Hashgram address.
	Bech32Prefix = "hash"

	Bech32PrefixAccAddr  = Bech32Prefix
	Bech32PrefixAccPub   = Bech32Prefix + "pub"
	Bech32PrefixValAddr  = Bech32Prefix + "valoper"
	Bech32PrefixValPub   = Bech32Prefix + "valoperpub"
	Bech32PrefixConsAddr = Bech32Prefix + "valcons"
	Bech32PrefixConsPub  = Bech32Prefix + "valconspub"
)

// ---------------------------------------------------------------------------
// Chain / binary identity
// ---------------------------------------------------------------------------

const (
	// AppName is the ABCI application name.
	AppName = "hashgram"

	// DefaultNodeHomeDirName is the default node home directory name under $HOME.
	DefaultNodeHomeDirName = ".hashgram"

	// BIP44CoinType is the SLIP-0044 coin type used for HD key derivation.
	// 118 is the Cosmos coin type; Hashgram uses it so that standard Cosmos
	// hardware-wallet and keyring tooling works without modification, which
	// matters for keeping the Founder key on a hardware wallet.
	BIP44CoinType uint32 = 118

	// DefaultBondDenom is the denomination used for staking.
	DefaultBondDenom = BaseCoinDenom
)

// ---------------------------------------------------------------------------
// Derived helpers
// ---------------------------------------------------------------------------

// HashToBase converts a whole-HASH amount into uhash.
func HashToBase(hash int64) math.Int {
	return math.NewInt(hash).MulRaw(MicroUnit)
}

// MaxSupplyBase is the maximum canonical supply expressed in uhash.
func MaxSupplyBase() math.Int { return HashToBase(MaxSupplyHash) }

// GenesisAllocation is one line of the genesis distribution table.
type GenesisAllocation struct {
	// Name is the stable identifier used in genesis output and docs.
	Name string
	// AmountHash is the whole-HASH size of the allocation.
	AmountHash int64
	// Description is shown by `hashgramctl init-mainnet-genesis`.
	Description string
}

// GenesisAllocations returns the canonical genesis distribution table in a
// fixed order. The order is part of the deterministic genesis output.
func GenesisAllocations() []GenesisAllocation {
	return []GenesisAllocation{
		{"founder", AllocFounderHash, "Founder allocation (20%): 20M unlocked at genesis, 180M vested over 8 years"},
		{"service_reserve", AllocServiceReserveHash, "Community / Useful-Service node rewards (50%), finite reserve"},
		{"treasury", AllocTreasuryHash, "Hashgram ecosystem treasury (15%), governed by x/gov"},
		{"growth", AllocGrowthHash, "Early user / growth pool (5%), includes the 1,850,000 HASH Welcome pool"},
		{"dev_grants", AllocDevGrantsHash, "Developer grants (5%), governed by x/gov"},
		{"liquidity", AllocLiquidityHash, "Liquidity / interoperability reserve (5%)"},
	}
}

// TotalAllocatedHash sums the genesis distribution table.
func TotalAllocatedHash() int64 {
	var total int64
	for _, a := range GenesisAllocations() {
		total += a.AmountHash
	}
	return total
}

// ValidateSupplyInvariants asserts, at process start, that the compile-time
// tokenomics constants are internally consistent. A build whose constants do
// not add up must not be able to produce a genesis file.
//
// This is called from an init() below so that no code path can skip it.
func ValidateSupplyInvariants() error {
	if got, want := TotalAllocatedHash(), MaxSupplyHash; got != want {
		return fmt.Errorf(
			"genesis allocations sum to %d HASH but max supply is %d HASH",
			got, want,
		)
	}

	if got, want := FounderUnlockedAtGenesisHash+FounderVestedHash, AllocFounderHash; got != want {
		return fmt.Errorf(
			"founder unlocked (%d) + vested (%d) = %d HASH but founder allocation is %d HASH",
			FounderUnlockedAtGenesisHash, FounderVestedHash, got, want,
		)
	}

	if FounderFeeBasisPoints > MaxFounderFeeBasisPoints {
		return fmt.Errorf(
			"founder fee %d bps exceeds hard ceiling of %d bps",
			FounderFeeBasisPoints, MaxFounderFeeBasisPoints,
		)
	}

	if MaxFounderFeeBasisPoints > BasisPointDenominator {
		return fmt.Errorf(
			"founder fee ceiling %d bps exceeds 100%% (%d bps)",
			MaxFounderFeeBasisPoints, BasisPointDenominator,
		)
	}

	// The Welcome pool must be exactly the worst-case payout of the tier
	// schedule, and must fit inside the growth allocation.
	worstCase := int64(WelcomeTier1Limit)*WelcomeTier1Hash +
		int64(WelcomeTier2Limit-WelcomeTier1Limit)*WelcomeTier2Hash +
		int64(WelcomeTier3Limit-WelcomeTier2Limit)*WelcomeTier3Hash
	if worstCase != WelcomePoolHash {
		return fmt.Errorf(
			"welcome tier schedule worst case is %d HASH but pool is %d HASH",
			worstCase, WelcomePoolHash,
		)
	}
	if WelcomePoolHash > AllocGrowthHash {
		return fmt.Errorf(
			"welcome pool %d HASH exceeds growth allocation %d HASH",
			WelcomePoolHash, AllocGrowthHash,
		)
	}

	if !(WelcomeTier1Limit < WelcomeTier2Limit && WelcomeTier2Limit < WelcomeTier3Limit) {
		return fmt.Errorf("welcome tier limits must be strictly increasing")
	}

	return nil
}

func init() {
	if err := ValidateSupplyInvariants(); err != nil {
		panic("hashgram: tokenomics constants are inconsistent: " + err.Error())
	}
}
