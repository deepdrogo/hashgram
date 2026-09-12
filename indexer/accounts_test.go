package indexer

import (
	"crypto/sha256"
	"encoding/json"
	"testing"

	hgparams "github.com/hashgram/hashgram/app/params"
)

// TestBech32EncodeVectors checks the encoder against BIP-173 test vectors
// and a known Cosmos module address.
func TestBech32EncodeVectors(t *testing.T) {
	// BIP-173: empty data.
	if got := bech32Encode("a", nil); got != "a12uel5l" {
		t.Errorf("bech32Encode(a, nil) = %q want a12uel5l", got)
	}
	// Cosmos Hub fee_collector: sha256("fee_collector")[:20] with hrp cosmos.
	if got := moduleAddressWithPrefix("cosmos", "fee_collector"); got != "cosmos17xpfvakm2amg962yls6f84z3kell8c5lserqta" {
		t.Errorf("cosmos fee_collector = %q", got)
	}
	// Cosmos Hub bonded_tokens_pool.
	if got := moduleAddressWithPrefix("cosmos", "bonded_tokens_pool"); got != "cosmos1fl48vsnmsdzcv85q5d2q4z5ajdha8yu34mf0eh" {
		t.Errorf("cosmos bonded_tokens_pool = %q", got)
	}
}

func moduleAddressWithPrefix(hrp, name string) string {
	sum := sha256.Sum256([]byte(name))
	return bech32Encode(hrp, sum[:20])
}

// TestModuleAccountsMatchGenesis: every ModuleAccount in the embedded
// mainnet genesis must be derived by the indexer, with the same name, and
// every genesis balance held by a module account must classify as "module".
func TestModuleAccountsMatchGenesis(t *testing.T) {
	var g struct {
		AppState struct {
			Auth struct {
				Accounts []struct {
					Type        string `json:"@type"`
					Name        string `json:"name"`
					BaseAccount struct {
						Address string `json:"address"`
					} `json:"base_account"`
				} `json:"accounts"`
			} `json:"auth"`
		} `json:"app_state"`
	}
	if err := json.Unmarshal(hgparams.MainnetGenesis, &g); err != nil {
		t.Fatal(err)
	}
	seen := 0
	for _, acc := range g.AppState.Auth.Accounts {
		if acc.Type != "/cosmos.auth.v1beta1.ModuleAccount" {
			continue
		}
		seen++
		if got := moduleAddress(acc.Name); got != acc.BaseAccount.Address {
			t.Errorf("moduleAddress(%q) = %s, genesis has %s", acc.Name, got, acc.BaseAccount.Address)
		}
		if name, ok := moduleAccounts[acc.BaseAccount.Address]; !ok || name != acc.Name {
			t.Errorf("genesis module account %s (%s) not in moduleAccounts (got %q)", acc.Name, acc.BaseAccount.Address, name)
		}
		if kind, label := accountKind(acc.BaseAccount.Address); kind != kindModule || label != acc.Name {
			t.Errorf("accountKind(%s) = %s/%s want module/%s", acc.BaseAccount.Address, kind, label, acc.Name)
		}
	}
	if seen == 0 {
		t.Fatal("no module accounts found in the embedded genesis")
	}
	// Pinned addresses from app/params/mainnet/genesis.json.
	for name, addr := range map[string]string{
		"serviceproof":        "hash1znsxwrqcg7svmw5zzmeswqpc0v5ddjdsf8djn4",
		"welcome":             "hash19qx5f2c7naumtn8zm4843a07j8c0htx6xzkrt3",
		"treasury_treasury":   "hash1j8nrdutkgj2cdctuj7zlcf0n754juc0jfluut2",
		"treasury_dev_grants": "hash1p3sevw2enxjankvzuhmt7ysqy7re09txvvyefr",
		"treasury_liquidity":  "hash1vup3q25ce68v7nm2se970lcmdwkar46cq555el",
		"treasury_growth":     "hash1vualfelayplprjlpx60vn69lgkgy8ft72qvapt",
	} {
		if got := moduleAddress(name); got != addr {
			t.Errorf("moduleAddress(%q) = %s want %s", name, got, addr)
		}
	}
	// The founder's vesting account and the launch operator are ordinary
	// accounts even though they hold genesis balances.
	for _, addr := range []string{"hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy", "hash127zemcfnxd3jrldpjzzgcckek4dswyw044sdzj", ""} {
		if kind, label := accountKind(addr); kind != kindAccount || label != "" {
			t.Errorf("accountKind(%q) = %s/%s want account", addr, kind, label)
		}
	}
	// The list is deterministic and covers every configured name.
	if got := moduleAccountList(); len(got) != len(moduleAccountNames) {
		t.Errorf("moduleAccountList has %d entries, want %d", len(got), len(moduleAccountNames))
	}
}
