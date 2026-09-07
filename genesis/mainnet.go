// Package genesis builds the Hashgram Mainnet genesis file.
//
// Everything here is deterministic: given the same Founder address and the
// same validator gentxs, two operators produce byte-identical genesis files
// and therefore the same genesis hash. That property is what makes the
// genesis hash a usable network identifier at all.
//
// The one input the software cannot supply is the Founder's PUBLIC address.
// There is no plausible default and no way to derive it: it must be a public
// address the Founder generated on a machine that is not this server. The
// builder refuses to proceed without it rather than inventing one. See
// docs/FOUNDER_LAUNCH_RUNBOOK.md.
package genesis

import (
	"encoding/json"
	"fmt"
	"time"

	"cosmossdk.io/math"

	"github.com/cosmos/cosmos-sdk/codec"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	vestingtypes "github.com/cosmos/cosmos-sdk/x/auth/vesting/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	distrtypes "github.com/cosmos/cosmos-sdk/x/distribution/types"
	govtypes "github.com/cosmos/cosmos-sdk/x/gov/types"
	govv1 "github.com/cosmos/cosmos-sdk/x/gov/types/v1"
	slashingtypes "github.com/cosmos/cosmos-sdk/x/slashing/types"
	stakingtypes "github.com/cosmos/cosmos-sdk/x/staking/types"

	hgparams "github.com/hashgram/hashgram/app/params"
	feeroutertypes "github.com/hashgram/hashgram/x/feerouter/types"
	foundertypes "github.com/hashgram/hashgram/x/founder/types"
	identitytypes "github.com/hashgram/hashgram/x/identity/types"
	networktypes "github.com/hashgram/hashgram/x/network/types"
	serviceprooftypes "github.com/hashgram/hashgram/x/serviceproof/types"
	treasurytypes "github.com/hashgram/hashgram/x/treasury/types"
	usernametypes "github.com/hashgram/hashgram/x/username/types"
	welcometypes "github.com/hashgram/hashgram/x/welcome/types"
)

// Config is the operator input to genesis construction.
type Config struct {
	// FounderAddress is the PUBLIC Hashgram address that receives the
	// 200,000,000 HASH Founder allocation and the Founder revenue share.
	//
	// Required. The builder will not invent one, and the repository contains
	// no Founder private key.
	FounderAddress string

	// Devnet builds a development network instead of Mainnet: DEVNET
	// identifiers, a short vesting schedule so vesting can actually be
	// observed, and shorter governance periods.
	Devnet bool

	// GenesisTime is the chain start time. Zero means now, truncated to the
	// second so that the resulting file is reproducible.
	GenesisTime time.Time

	// VotingPeriod overrides the governance voting period. Zero uses the
	// default for the network type.
	VotingPeriod time.Duration

	// GenesisAccounts are plain accounts funded at genesis so that the
	// initial validators can bond and pay fees. Genesis already allocates the
	// whole supply, so these balances are not created: they are transferred
	// out of the Founder's unlocked 20,000,000 HASH, and the Founder's
	// balance is reduced by exactly their sum. The vesting schedule, the
	// treasury reserves and the service reserve are untouched, and the total
	// stays exactly the maximum supply.
	//
	// The Founder pays for the launch validators out of the Founder's own
	// spendable money. That is the honest source: the treasury is spendable
	// only by governance, and governance does not exist before block one.
	GenesisAccounts []GenesisAccount
}

// GenesisAccount is one funded launch account.
type GenesisAccount struct {
	// Address is a PUBLIC Hashgram account address. A validator operator key
	// or a fee-paying hot key; never the Founder cold address, which already
	// holds its allocation.
	Address string

	// Amount is the balance, in base units.
	Amount sdk.Coins
}

// MaxGenesisAccountFundingBase is the most the genesis accounts may take in
// total: the whole unlocked portion of the Founder allocation.
func MaxGenesisAccountFundingBase() math.Int {
	return hgparams.HashToBase(hgparams.FounderUnlockedAtGenesisHash)
}

// Result reports what was built.
type Result struct {
	// ChainID and NetworkID identify the network.
	ChainID   string
	NetworkID string

	// AppState is the assembled application genesis.
	AppState map[string]json.RawMessage

	// Allocations is the distribution table as built, for operator display.
	Allocations []Allocation

	// TotalSupply is the sum of every genesis balance. It must equal the
	// canonical maximum supply exactly.
	TotalSupply sdk.Coins
}

// Allocation is one line of the built distribution, with the address that
// actually holds it.
type Allocation struct {
	Name        string
	Description string
	Address     string
	Amount      sdk.Coins

	// Vesting is the locked portion, for the Founder line.
	Vesting sdk.Coins
}

// Build assembles the Hashgram genesis application state.
//
// It starts from the application's own default genesis, so every module gets
// its own defaults and nothing is silently omitted, and then replaces the
// modules whose genesis is a Mainnet decision rather than a default.
func Build(cdc codec.Codec, defaults map[string]json.RawMessage, cfg Config) (*Result, error) {
	if cfg.FounderAddress == "" {
		return nil, fmt.Errorf(
			"the Founder PUBLIC address is required and cannot be generated by this software; " +
				"create it on a machine that is not this server and supply it as " +
				"GENESIS_FOUNDER_ADDRESS (see docs/FOUNDER_LAUNCH_RUNBOOK.md)")
	}
	founderAddr, err := sdk.AccAddressFromBech32(cfg.FounderAddress)
	if err != nil {
		return nil, fmt.Errorf("founder address %q is not a valid Hashgram address: %w",
			cfg.FounderAddress, err)
	}

	genesisTime := cfg.GenesisTime
	if genesisTime.IsZero() {
		genesisTime = time.Now().UTC()
	}
	// Truncated so the file is reproducible: a sub-second component would
	// make two runs differ and produce different genesis hashes.
	genesisTime = genesisTime.UTC().Truncate(time.Second)

	appState := make(map[string]json.RawMessage, len(defaults))
	for k, v := range defaults {
		appState[k] = v
	}

	netIdentity := hgparams.MainnetIdentity("")
	networkGenesis := networktypes.MainnetGenesis()
	if cfg.Devnet {
		netIdentity = hgparams.DevnetIdentity("")
		networkGenesis = networktypes.DefaultGenesis()
	}

	// --- x/network -------------------------------------------------------
	appState[networktypes.ModuleName] = cdc.MustMarshalJSON(networkGenesis)

	// --- x/founder -------------------------------------------------------
	appState[foundertypes.ModuleName] = cdc.MustMarshalJSON(
		foundertypes.NewGenesisState(cfg.FounderAddress))

	// --- x/feerouter -----------------------------------------------------
	appState[feeroutertypes.ModuleName] = cdc.MustMarshalJSON(feeroutertypes.DefaultGenesis())

	// --- x/treasury ------------------------------------------------------
	treasuryGenesis := treasurytypes.MainnetGenesis()
	appState[treasurytypes.ModuleName] = cdc.MustMarshalJSON(treasuryGenesis)

	// --- x/welcome, x/serviceproof, x/username, x/identity ---------------
	appState[welcometypes.ModuleName] = cdc.MustMarshalJSON(welcometypes.DefaultGenesis())
	appState[serviceprooftypes.ModuleName] = cdc.MustMarshalJSON(serviceprooftypes.DefaultGenesis())
	appState[usernametypes.ModuleName] = cdc.MustMarshalJSON(usernametypes.DefaultGenesis())
	appState[identitytypes.ModuleName] = cdc.MustMarshalJSON(identitytypes.DefaultGenesis())

	// --- accounts and balances -------------------------------------------

	allocations, accounts, balances, err := buildAllocations(
		founderAddr, treasuryGenesis, genesisTime, cfg.Devnet, cfg.GenesisAccounts)
	if err != nil {
		return nil, err
	}

	packedAccounts, err := authtypes.PackAccounts(accounts)
	if err != nil {
		return nil, fmt.Errorf("packing genesis accounts: %w", err)
	}

	var authGenesis authtypes.GenesisState
	cdc.MustUnmarshalJSON(appState[authtypes.ModuleName], &authGenesis)
	authGenesis.Accounts = packedAccounts
	appState[authtypes.ModuleName] = cdc.MustMarshalJSON(&authGenesis)

	total := sdk.NewCoins()
	for _, b := range balances {
		total = total.Add(b.Coins...)
	}

	// The supply ceiling. Checked here as well as in the invariant tests,
	// because a genesis file that does not add up must never reach disk.
	want := sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.MaxSupplyBase()))
	if !total.Equal(want) {
		return nil, fmt.Errorf(
			"genesis balances total %s but the canonical maximum supply is %s; "+
				"refusing to write a genesis file that does not add up",
			total, want)
	}

	var bankGenesis banktypes.GenesisState
	cdc.MustUnmarshalJSON(appState[banktypes.ModuleName], &bankGenesis)
	bankGenesis.Balances = balances
	bankGenesis.Supply = total
	bankGenesis.DenomMetadata = []banktypes.Metadata{denomMetadata()}
	appState[banktypes.ModuleName] = cdc.MustMarshalJSON(&bankGenesis)

	// --- staking, distribution, slashing, gov ----------------------------

	var stakingGenesis stakingtypes.GenesisState
	cdc.MustUnmarshalJSON(appState[stakingtypes.ModuleName], &stakingGenesis)
	stakingGenesis.Params = stakingParams(cfg.Devnet)
	appState[stakingtypes.ModuleName] = cdc.MustMarshalJSON(&stakingGenesis)

	var distrGenesis distrtypes.GenesisState
	cdc.MustUnmarshalJSON(appState[distrtypes.ModuleName], &distrGenesis)
	distrGenesis.Params = distributionParams()
	appState[distrtypes.ModuleName] = cdc.MustMarshalJSON(&distrGenesis)

	var slashingGenesis slashingtypes.GenesisState
	cdc.MustUnmarshalJSON(appState[slashingtypes.ModuleName], &slashingGenesis)
	slashingGenesis.Params = slashingParams()
	appState[slashingtypes.ModuleName] = cdc.MustMarshalJSON(&slashingGenesis)

	var govGenesis govv1.GenesisState
	cdc.MustUnmarshalJSON(appState[govtypes.ModuleName], &govGenesis)
	govParams := governanceParams(cfg)
	govGenesis.Params = &govParams
	appState[govtypes.ModuleName] = cdc.MustMarshalJSON(&govGenesis)

	return &Result{
		ChainID:     netIdentity.ChainID,
		NetworkID:   netIdentity.NetworkID,
		AppState:    appState,
		Allocations: allocations,
		TotalSupply: total,
	}, nil
}

// buildAllocations constructs every genesis account and balance.
//
// The Founder allocation is a single PeriodicVestingAccount holding
// 200,000,000 HASH of which 180,000,000 is locked. One account rather than
// two, because that is what "20M unlocked at genesis, 180M vested" actually
// means: a vesting account's original_vesting is the locked part and the rest
// of its balance is immediately spendable.
func buildAllocations(
	founder sdk.AccAddress,
	treasuryGenesis *treasurytypes.GenesisState,
	genesisTime time.Time,
	devnet bool,
	genesisAccounts []GenesisAccount,
) ([]Allocation, []authtypes.GenesisAccount, []banktypes.Balance, error) {
	founderTotal := hashCoins(hgparams.AllocFounderHash)
	founderVesting := hashCoins(hgparams.FounderVestedHash)

	// Launch accounts come out of the Founder's unlocked portion. Validate
	// them completely before touching any balance.
	launchAccounts, launchTotal, err := validateGenesisAccounts(founder, genesisAccounts)
	if err != nil {
		return nil, nil, nil, err
	}
	founderBalance := founderTotal.Sub(launchTotal...)

	periods, endTime := founderVestingSchedule(genesisTime, devnet)

	base := authtypes.NewBaseAccountWithAddress(founder)
	baseVesting, err := vestingtypes.NewBaseVestingAccount(base, founderVesting, endTime.Unix())
	if err != nil {
		return nil, nil, nil, fmt.Errorf("building the Founder vesting account: %w", err)
	}
	founderAccount := vestingtypes.NewPeriodicVestingAccountRaw(
		baseVesting, genesisTime.Unix(), periods)

	accounts := []authtypes.GenesisAccount{founderAccount}
	balances := []banktypes.Balance{{
		Address: founder.String(),
		Coins:   founderBalance,
	}}

	founderDescription := fmt.Sprintf(
		"Founder allocation (20%%): %s HASH unlocked at genesis, %s HASH vesting over %d monthly periods",
		commas(hgparams.FounderUnlockedAtGenesisHash),
		commas(hgparams.FounderVestedHash),
		len(periods))
	if !launchTotal.IsZero() {
		founderDescription += fmt.Sprintf(
			"; %s HASH of the unlocked portion transferred at genesis to %d launch account(s) below",
			commasStr(launchTotal.AmountOf(hgparams.BaseCoinDenom).QuoRaw(hgparams.MicroUnit).String()),
			len(launchAccounts))
	}
	allocations := []Allocation{{
		Name:        "founder",
		Description: founderDescription,
		Address:     founder.String(),
		Amount:      founderBalance,
		Vesting:     founderVesting,
	}}

	for i, la := range launchAccounts {
		addr := sdk.MustAccAddressFromBech32(la.Address)
		accounts = append(accounts, authtypes.NewBaseAccountWithAddress(addr))
		balances = append(balances, banktypes.Balance{Address: la.Address, Coins: la.Amount})
		allocations = append(allocations, Allocation{
			Name:        fmt.Sprintf("launch-%d", i+1),
			Description: "Launch account funded from the Founder's unlocked portion (validator stake and fees)",
			Address:     la.Address,
			Amount:      la.Amount,
		})
	}

	// Module-held allocations. Each is a module account declared explicitly
	// in auth genesis: the account keeper panics if it finds a plain
	// BaseAccount where a module account should be, so declaring them is not
	// optional.
	moduleAllocations := []struct {
		module      string
		permissions []string
		amountHash  int64
		description string
	}{
		{
			module:      serviceprooftypes.ModuleName,
			amountHash:  hgparams.AllocServiceReserveHash,
			description: "Community / useful-service node rewards (50%), a finite reserve held by x/serviceproof",
		},
		{
			module:      welcometypes.ModuleName,
			amountHash:  hgparams.WelcomePoolHash,
			description: "Welcome reward pool, carved out of the growth allocation and held by x/welcome",
		},
	}

	for _, m := range moduleAllocations {
		addr := authtypes.NewModuleAddress(m.module)
		accounts = append(accounts,
			authtypes.NewEmptyModuleAccount(m.module, m.permissions...))
		coins := hashCoins(m.amountHash)
		balances = append(balances, banktypes.Balance{Address: addr.String(), Coins: coins})
		allocations = append(allocations, Allocation{
			Name:        m.module,
			Description: m.description,
			Address:     addr.String(),
			Amount:      coins,
		})
	}

	// Treasury reserves, one account each so every balance is independently
	// queryable.
	for _, r := range treasuryGenesis.Reserves {
		accounts = append(accounts, authtypes.NewEmptyModuleAccount(r.SubAccount))
		balances = append(balances, banktypes.Balance{
			Address: treasurytypes.ReserveAddress(r.Name).String(),
			Coins:   r.Initial,
		})
		allocations = append(allocations, Allocation{
			Name:        r.Name,
			Description: r.Description,
			Address:     treasurytypes.ReserveAddress(r.Name).String(),
			Amount:      r.Initial,
		})
	}

	return allocations, accounts, balances, nil
}

// founderVestingSchedule builds the monthly vesting periods.
//
// Equal monthly periods with the rounding remainder placed in the final
// period, so the sum is exactly FounderVestedHash rather than
// FounderVestedHash minus accumulated truncation.
//
// On a devnet the whole schedule is compressed into minutes, so that vesting
// can actually be observed during a test run rather than in eight years.
func founderVestingSchedule(genesisTime time.Time, devnet bool) (vestingtypes.Periods, time.Time) {
	count := hgparams.FounderVestingPeriods
	periodLength := int64(30 * 24 * 60 * 60) // ~1 month in seconds
	if devnet {
		periodLength = 60 // one minute per period on a devnet
	}

	totalBase := hgparams.HashToBase(hgparams.FounderVestedHash)
	per := totalBase.Quo(math.NewInt(count))
	remainder := totalBase.Sub(per.MulRaw(count))

	periods := make(vestingtypes.Periods, 0, count)
	for i := int64(0); i < count; i++ {
		amount := per
		if i == count-1 {
			amount = amount.Add(remainder)
		}
		periods = append(periods, vestingtypes.Period{
			Length: periodLength,
			Amount: sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, amount)),
		})
	}

	endTime := genesisTime.Add(time.Duration(periodLength*count) * time.Second)
	return periods, endTime
}

func hashCoins(amountHash int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(amountHash)))
}

// denomMetadata registers HASH so that wallets render amounts correctly
// rather than showing raw uhash to users.
func denomMetadata() banktypes.Metadata {
	return banktypes.Metadata{
		Description: "The native coin of Hashgram Mainnet.",
		DenomUnits: []*banktypes.DenomUnit{
			{Denom: hgparams.BaseCoinDenom, Exponent: 0, Aliases: []string{"microhash"}},
			{Denom: hgparams.HumanCoinDenom, Exponent: hgparams.CoinDecimals},
		},
		Base:    hgparams.BaseCoinDenom,
		Display: hgparams.HumanCoinDenom,
		Name:    "Hashgram HASH",
		Symbol:  hgparams.HumanCoinDenom,
	}
}

func commas(n int64) string { return commasStr(fmt.Sprintf("%d", n)) }

func commasStr(s string) string {
	if len(s) <= 3 {
		return s
	}
	out := make([]byte, 0, len(s)+len(s)/3)
	lead := len(s) % 3
	if lead > 0 {
		out = append(out, s[:lead]...)
	}
	for i := lead; i < len(s); i += 3 {
		if len(out) > 0 {
			out = append(out, ',')
		}
		out = append(out, s[i:i+3]...)
	}
	return string(out)
}

// validateGenesisAccounts checks the launch accounts and returns them with
// their total.
//
// The rules are the ones that keep the distribution table honest: the money
// comes only from the Founder's unlocked portion and can never exceed it,
// the Founder's own address cannot be a launch account (it already holds its
// allocation), module addresses cannot receive plain balances, and an
// address appears once.
func validateGenesisAccounts(founder sdk.AccAddress, in []GenesisAccount) ([]GenesisAccount, sdk.Coins, error) {
	total := sdk.NewCoins()
	seen := make(map[string]bool, len(in))
	out := make([]GenesisAccount, 0, len(in))
	for i, ga := range in {
		addr, err := sdk.AccAddressFromBech32(ga.Address)
		if err != nil {
			return nil, nil, fmt.Errorf("genesis account %d: %q is not a valid Hashgram address: %w", i+1, ga.Address, err)
		}
		if addr.Equals(founder) {
			return nil, nil, fmt.Errorf("genesis account %d is the Founder address; it already holds the Founder allocation", i+1)
		}
		if seen[addr.String()] {
			return nil, nil, fmt.Errorf("genesis account %s is listed twice", addr.String())
		}
		seen[addr.String()] = true
		if !ga.Amount.IsValid() || ga.Amount.IsZero() {
			return nil, nil, fmt.Errorf("genesis account %s: amount %q must be a positive coin amount", addr.String(), ga.Amount.String())
		}
		for _, c := range ga.Amount {
			if c.Denom != hgparams.BaseCoinDenom {
				return nil, nil, fmt.Errorf("genesis account %s: only %s can be allocated, not %s",
					addr.String(), hgparams.BaseCoinDenom, c.Denom)
			}
		}
		total = total.Add(ga.Amount...)
		out = append(out, GenesisAccount{Address: addr.String(), Amount: ga.Amount})
	}
	if total.AmountOf(hgparams.BaseCoinDenom).GT(MaxGenesisAccountFundingBase()) {
		return nil, nil, fmt.Errorf(
			"genesis accounts total %s %s but the Founder's unlocked portion is %s %s; "+
				"launch accounts are funded only from what the Founder can spend at genesis",
			total.AmountOf(hgparams.BaseCoinDenom), hgparams.BaseCoinDenom,
			MaxGenesisAccountFundingBase(), hgparams.BaseCoinDenom)
	}
	return out, total, nil
}
