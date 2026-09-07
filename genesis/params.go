package genesis

import (
	"time"

	"cosmossdk.io/math"

	sdk "github.com/cosmos/cosmos-sdk/types"
	distrtypes "github.com/cosmos/cosmos-sdk/x/distribution/types"
	govv1 "github.com/cosmos/cosmos-sdk/x/gov/types/v1"
	slashingtypes "github.com/cosmos/cosmos-sdk/x/slashing/types"
	stakingtypes "github.com/cosmos/cosmos-sdk/x/staking/types"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// Consensus and governance parameters for Hashgram Mainnet.
//
// Each departure from the Cosmos SDK default is justified in place. A bare
// number tells a future maintainer nothing about which direction is safe to
// move it.

const (
	// MaxValidators is the active set size.
	//
	// 100 is the ecosystem norm and is a real trade-off: a larger set is more
	// decentralised and costs more bandwidth per block, since every validator
	// gossips with every other. Hashgram expects most community
	// participation to be relay, storage and media nodes rather than
	// validators, so the validator set does not need to be enormous to be
	// broadly held.
	MaxValidators uint32 = 100

	// UnbondingTime is 21 days.
	//
	// It must exceed the window in which a double-sign could still be
	// discovered and slashed, otherwise a validator could equivocate and
	// unbond before the evidence lands. 21 days is the ecosystem norm and
	// comfortably exceeds any plausible evidence delay.
	UnbondingTime = 21 * 24 * time.Hour

	// MaxEntries is how many simultaneous unbonding or redelegation entries
	// one delegator-validator pair may have.
	MaxEntries uint32 = 7

	// HistoricalEntries is how many historical staking snapshots to keep.
	HistoricalEntries uint32 = 10_000

	// SignedBlocksWindow and MinSignedPerWindow define liveness.
	//
	// A 30,000 block window at roughly 4 second blocks is about 33 hours,
	// during which a validator must sign at least 5%. That is a deliberately
	// forgiving liveness requirement: Hashgram wants community operators on
	// ordinary hardware, and jailing somebody for a few hours of downtime
	// pushes validation toward professional operators only.
	SignedBlocksWindow int64 = 30_000

	// DowntimeJailDuration is how long a jailed validator waits.
	DowntimeJailDuration = 10 * time.Minute
)

// stakingParams returns the staking configuration.
func stakingParams(devnet bool) stakingtypes.Params {
	p := stakingtypes.DefaultParams()
	p.BondDenom = hgparams.DefaultBondDenom
	p.MaxValidators = MaxValidators
	p.MaxEntries = MaxEntries
	p.HistoricalEntries = HistoricalEntries
	p.UnbondingTime = UnbondingTime

	// A minimum commission floor prevents a race to zero commission, which
	// ends with validators funded by nothing and dependent on subsidy.
	p.MinCommissionRate = math.LegacyNewDecWithPrec(5, 2) // 5%

	if devnet {
		// Short enough that unbonding can be observed during a test run.
		p.UnbondingTime = 10 * time.Minute
		p.MinCommissionRate = math.LegacyZeroDec()
	}
	return p
}

// distributionParams returns the fee distribution configuration.
func distributionParams() distrtypes.Params {
	p := distrtypes.DefaultParams()

	// The community tax is zero. Hashgram routes protocol revenue through
	// x/feerouter, which has an explicit, queryable split; adding a second
	// implicit tax here would mean two mechanisms taking a cut with no single
	// place to read the total.
	p.CommunityTax = math.LegacyZeroDec()

	// Rewards go to the validators and delegators who earned them, with no
	// proposer bonus. A proposer bonus rewards being selected rather than
	// doing more work, and its main effect is to advantage large validators
	// who are selected more often.
	p.WithdrawAddrEnabled = true

	return p
}

// slashingParams returns the liveness and equivocation configuration.
func slashingParams() slashingtypes.Params {
	p := slashingtypes.DefaultParams()
	p.SignedBlocksWindow = SignedBlocksWindow
	p.MinSignedPerWindow = math.LegacyNewDecWithPrec(5, 2) // 5%
	p.DowntimeJailDuration = DowntimeJailDuration

	// Downtime is slashed at 0.01%: a nudge rather than a punishment, because
	// downtime is usually a failure of somebody's hosting rather than an
	// attack.
	p.SlashFractionDowntime = math.LegacyNewDecWithPrec(1, 4)

	// Double-signing is slashed at 5%. Equivocation is the one thing a
	// validator can do that actually threatens consensus, and unlike downtime
	// it cannot happen by accident on correctly configured infrastructure.
	// See docs/SECURITY.md on remote signers and sentry topology.
	p.SlashFractionDoubleSign = math.LegacyNewDecWithPrec(5, 2)

	return p
}

// governanceParams returns the governance configuration.
func governanceParams(cfg Config) govv1.Params {
	p := govv1.DefaultParams()

	minDeposit := sdk.NewCoins(sdk.NewCoin(
		hgparams.BaseCoinDenom, hgparams.HashToBase(10_000)))
	p.MinDeposit = minDeposit
	p.ExpeditedMinDeposit = sdk.NewCoins(sdk.NewCoin(
		hgparams.BaseCoinDenom, hgparams.HashToBase(50_000)))

	votingPeriod := 7 * 24 * time.Hour
	depositPeriod := 14 * 24 * time.Hour
	expedited := 24 * time.Hour

	if cfg.Devnet {
		// Short enough to actually run a proposal during a test.
		votingPeriod = 2 * time.Minute
		depositPeriod = 2 * time.Minute
		expedited = 1 * time.Minute
		p.MinDeposit = sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(10)))
		p.ExpeditedMinDeposit = sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(20)))
	}
	if cfg.VotingPeriod > 0 {
		votingPeriod = cfg.VotingPeriod
	}

	p.VotingPeriod = &votingPeriod
	p.MaxDepositPeriod = &depositPeriod
	p.ExpeditedVotingPeriod = &expedited

	// A 40% quorum, above the SDK's 33.4%. Hashgram governance can change
	// module parameters and spend the treasury, so a proposal passing on the
	// participation of a third of stake is too thin a mandate for that.
	p.Quorum = "0.400000000000000000"
	p.Threshold = "0.500000000000000000"

	// A 33.4% veto threshold, the ecosystem norm: a determined minority can
	// block a proposal that a majority wants, which is the point of a veto.
	p.VetoThreshold = "0.334000000000000000"

	// Rejected proposal deposits are burned, so that spamming governance with
	// proposals nobody supports has a cost.
	p.BurnVoteVeto = true
	p.BurnProposalDepositPrevote = false

	return p
}
