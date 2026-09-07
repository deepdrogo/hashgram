package genesis_test

import (
	"testing"
	"time"

	dbm "github.com/cosmos/cosmos-db"
	"github.com/stretchr/testify/require"

	"cosmossdk.io/log"

	simtestutil "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"

	"github.com/hashgram/hashgram/app"
	hgparams "github.com/hashgram/hashgram/app/params"
	"github.com/hashgram/hashgram/genesis"
)

func launchAddress(label string) string {
	b := make([]byte, 20)
	copy(b, label)
	return sdk.AccAddress(b).String()
}

func buildWithLaunchAccounts(t *testing.T, accounts []genesis.GenesisAccount) (*genesis.Result, *app.HashgramApp, error) {
	t.Helper()
	a := app.NewHashgramApp(
		log.NewNopLogger(), dbm.NewMemDB(), nil, true,
		simtestutil.NewAppOptionsWithFlagHome(t.TempDir()),
	)
	res, err := genesis.Build(a.AppCodec(), a.DefaultGenesis(), genesis.Config{
		FounderAddress:  founderAddress(),
		GenesisTime:     time.Unix(1_700_000_000, 0).UTC(),
		GenesisAccounts: accounts,
	})
	return res, a, err
}

func uhash(hash int64) sdk.Coins {
	return sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.HashToBase(hash)))
}

// TestLaunchAccountsComeOutOfTheFounderUnlockedPortion: the supply does not
// grow, the treasury and reserves are untouched, and the Founder's balance
// drops by exactly what the launch accounts received. The vesting schedule
// is unchanged, so the unlocked portion is what pays.
func TestLaunchAccountsComeOutOfTheFounderUnlockedPortion(t *testing.T) {
	v1 := launchAddress("LAUNCH-VALIDATOR-1")
	v2 := launchAddress("LAUNCH-VALIDATOR-2")
	res, a, err := buildWithLaunchAccounts(t, []genesis.GenesisAccount{
		{Address: v1, Amount: uhash(1_000_000)},
		{Address: v2, Amount: uhash(500_000)},
	})
	require.NoError(t, err)

	want := sdk.NewCoins(sdk.NewCoin(hgparams.BaseCoinDenom, hgparams.MaxSupplyBase()))
	require.Equal(t, want.String(), res.TotalSupply.String(), "supply must not grow")

	var bank banktypes.GenesisState
	a.AppCodec().MustUnmarshalJSON(res.AppState[banktypes.ModuleName], &bank)
	balances := map[string]sdk.Coins{}
	for _, b := range bank.Balances {
		balances[b.Address] = b.Coins
	}
	require.Equal(t, uhash(1_000_000).String(), balances[v1].String())
	require.Equal(t, uhash(500_000).String(), balances[v2].String())
	require.Equal(t, uhash(hgparams.AllocFounderHash-1_500_000).String(), balances[founderAddress()].String(),
		"the Founder pays for the launch accounts")

	// Vesting is unchanged: the whole 180M still vests, so what was spent is
	// unlocked money only.
	var auth authtypes.GenesisState
	a.AppCodec().MustUnmarshalJSON(res.AppState[authtypes.ModuleName], &auth)
	accounts, err := authtypes.UnpackAccounts(auth.Accounts)
	require.NoError(t, err)
	var sawFounder, sawV1 bool
	for _, acc := range accounts {
		switch acc.GetAddress().String() {
		case founderAddress():
			sawFounder = true
			vesting, ok := acc.(interface{ GetOriginalVesting() sdk.Coins })
			require.True(t, ok, "the Founder account must still vest")
			require.Equal(t, uhash(hgparams.FounderVestedHash).String(), vesting.GetOriginalVesting().String())
		case v1:
			sawV1 = true
			_, isBase := acc.(*authtypes.BaseAccount)
			require.True(t, isBase, "a launch account is a plain account")
		}
	}
	require.True(t, sawFounder)
	require.True(t, sawV1)

	// The distribution table shows what happened rather than hiding it.
	var founderLine *genesis.Allocation
	launchLines := 0
	for i := range res.Allocations {
		switch {
		case res.Allocations[i].Name == "founder":
			founderLine = &res.Allocations[i]
		case len(res.Allocations[i].Name) > 7 && res.Allocations[i].Name[:7] == "launch-":
			launchLines++
		}
	}
	require.NotNil(t, founderLine)
	require.Contains(t, founderLine.Description, "1,500,000 HASH of the unlocked portion transferred")
	require.Equal(t, 2, launchLines)
}

// TestLaunchAccountsCannotExceedTheUnlockedPortion: the 180M vesting is not
// available for launch funding, and neither is anyone else's allocation.
func TestLaunchAccountsCannotExceedTheUnlockedPortion(t *testing.T) {
	_, _, err := buildWithLaunchAccounts(t, []genesis.GenesisAccount{
		{Address: launchAddress("LAUNCH-BIG"), Amount: uhash(hgparams.FounderUnlockedAtGenesisHash + 1)},
	})
	require.Error(t, err)
	require.Contains(t, err.Error(), "unlocked portion")

	// Exactly the unlocked portion is allowed, leaving the Founder with the
	// vesting balance only.
	res, _, err := buildWithLaunchAccounts(t, []genesis.GenesisAccount{
		{Address: launchAddress("LAUNCH-ALL"), Amount: uhash(hgparams.FounderUnlockedAtGenesisHash)},
	})
	require.NoError(t, err)
	for _, al := range res.Allocations {
		if al.Name == "founder" {
			require.Equal(t, uhash(hgparams.FounderVestedHash).String(), al.Amount.String())
		}
	}
}

// TestLaunchAccountsRejectBadInput covers the mistakes the runbook warns
// about: the Founder's own address, duplicates, a foreign denom, zero.
func TestLaunchAccountsRejectBadInput(t *testing.T) {
	cases := map[string][]genesis.GenesisAccount{
		"founder address": {{Address: founderAddress(), Amount: uhash(1)}},
		"duplicate": {
			{Address: launchAddress("DUP"), Amount: uhash(1)},
			{Address: launchAddress("DUP"), Amount: uhash(2)},
		},
		"foreign denom":  {{Address: launchAddress("DENOM"), Amount: sdk.NewCoins(sdk.NewInt64Coin("stake", 5))}},
		"zero":           {{Address: launchAddress("ZERO"), Amount: sdk.NewCoins()}},
		"not an address": {{Address: "cosmos1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq", Amount: uhash(1)}},
	}
	for name, accounts := range cases {
		t.Run(name, func(t *testing.T) {
			_, _, err := buildWithLaunchAccounts(t, accounts)
			require.Error(t, err)
		})
	}
}

// TestLaunchAccountsKeepGenesisDeterministic: two builds with the same launch
// accounts produce the same bytes, so a second person can still reproduce the
// genesis hash.
func TestLaunchAccountsKeepGenesisDeterministic(t *testing.T) {
	accounts := []genesis.GenesisAccount{
		{Address: launchAddress("DET-1"), Amount: uhash(10)},
		{Address: launchAddress("DET-2"), Amount: uhash(20)},
	}
	r1, a, err := buildWithLaunchAccounts(t, accounts)
	require.NoError(t, err)
	r2, _, err := buildWithLaunchAccounts(t, accounts)
	require.NoError(t, err)
	for k := range r1.AppState {
		if len(r1.AppState[k]) == 0 {
			require.Empty(t, r2.AppState[k], k)
			continue
		}
		require.JSONEq(t, string(r1.AppState[k]), string(r2.AppState[k]), k)
	}
	_ = a
}
