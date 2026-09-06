package app_test

import (
	"encoding/json"
	"testing"

	dbm "github.com/cosmos/cosmos-db"

	"cosmossdk.io/log"

	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	simtestutil "github.com/cosmos/cosmos-sdk/testutil/sims"

	"github.com/hashgram/hashgram/app"
	hgparams "github.com/hashgram/hashgram/app/params"
)

func newTestApp(t *testing.T) *app.HashgramApp {
	t.Helper()
	hgparams.SetSDKConfig()
	return app.NewHashgramApp(
		log.NewNopLogger(),
		dbm.NewMemDB(),
		nil,
		true,
		simtestutil.NewAppOptionsWithFlagHome(t.TempDir()),
	)
}

// TestNoModuleCanMint is the wiring-level guarantee behind §116.6 ("no hidden
// mint"). On Hashgram no module account holds authtypes.Minter, so
// BankKeeper.MintCoins cannot succeed for anyone.
//
// If a future change adds a minting module, this test fails and forces an
// explicit decision rather than letting inflation appear quietly.
func TestNoModuleCanMint(t *testing.T) {
	if n := app.MintAuthorityCount(); n != 0 {
		t.Fatalf("%d module account(s) hold the Minter permission; Hashgram must have zero", n)
	}

	for name, perms := range app.GetMaccPerms() {
		for _, p := range perms {
			if p == authtypes.Minter {
				t.Errorf("module account %q holds Minter", name)
			}
		}
	}
}

// TestMintModuleIsNotWiredIn asserts the mint module is absent from the module
// manager, not merely configured with zero inflation.
func TestMintModuleIsNotWiredIn(t *testing.T) {
	a := newTestApp(t)

	forbidden := map[string]string{
		"mint":    "Hashgram has a fixed supply; the inflation module must not be compiled in",
		"circuit": "a chain-wide message pause switch contradicts the no-kill-switch requirement",
	}

	for name := range a.ModuleManager.Modules {
		if reason, bad := forbidden[name]; bad {
			t.Errorf("module %q is wired into the app: %s", name, reason)
		}
	}
}

// TestRequiredModulesArePresent guards against accidentally dropping a module
// the protocol depends on.
func TestRequiredModulesArePresent(t *testing.T) {
	a := newTestApp(t)

	required := []string{
		"auth", "bank", "staking", "slashing", "distribution",
		"gov", "evidence", "vesting", "upgrade", "consensus",
		"genutil", "feegrant", "authz",
	}
	for _, name := range required {
		if _, ok := a.ModuleManager.Modules[name]; !ok {
			t.Errorf("required module %q is missing", name)
		}
	}
}

// TestDefaultGenesisIsValidJSON makes sure every module produces parseable
// default genesis, which is what `hashgramd init` writes.
func TestDefaultGenesisIsValidJSON(t *testing.T) {
	a := newTestApp(t)

	gen := a.DefaultGenesis()
	if len(gen) == 0 {
		t.Fatal("default genesis is empty")
	}
	for module, raw := range gen {
		// x/consensus intentionally contributes no app_state: consensus
		// params live in the CometBFT section of the genesis file, not in
		// app_state. An empty entry here is correct, not a bug.
		if len(raw) == 0 {
			continue
		}
		var v any
		if err := json.Unmarshal(raw, &v); err != nil {
			t.Errorf("module %q default genesis is not valid JSON: %v", module, err)
		}
	}

	if _, ok := gen[banktypes.ModuleName]; !ok {
		t.Error("bank genesis is missing")
	}
	if _, ok := gen["mint"]; ok {
		t.Error("mint genesis is present; the module should not be wired in")
	}
}

// TestModuleAccountsAreBlockedFromReceivingFunds prevents users losing HASH by
// sending it into a module escrow.
func TestModuleAccountsAreBlockedFromReceivingFunds(t *testing.T) {
	blocked := app.BlockedAddresses()

	feeCollector := authtypes.NewModuleAddress(authtypes.FeeCollectorName).String()
	if !blocked[feeCollector] {
		t.Error("the fee collector module account is not blocked from receiving funds")
	}

	// Governance must stay unblocked: proposals fund it legitimately.
	govAddr := authtypes.NewModuleAddress("gov").String()
	if blocked[govAddr] {
		t.Error("the gov module account is blocked; governance proposals could not be funded")
	}
}

// TestAppNameAndVersion pins the identifiers that appear in the P2P handshake
// and in operator output.
func TestAppNameAndVersion(t *testing.T) {
	a := newTestApp(t)
	if a.Name() != "hashgram" {
		t.Errorf("app name = %q, want hashgram", a.Name())
	}

	info := app.BuildInfo()
	if info.ProtocolMajorVersion != hgparams.ProtocolMajorVersion {
		t.Errorf("build info protocol version = %d, want %d",
			info.ProtocolMajorVersion, hgparams.ProtocolMajorVersion)
	}
	if info.GoVersion == "" {
		t.Error("build info is missing the Go version")
	}
}
