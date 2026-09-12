package indexer

import (
	"encoding/json"
	"errors"
	"math/big"
	"testing"

	hgparams "github.com/hashgram/hashgram/app/params"
)

func TestUhashAmount(t *testing.T) {
	cases := []struct {
		in   string
		want string
		ok   bool
	}{
		{"12345uhash", "12345", true},
		{"0uhash", "0", true},
		{"7ibc/ABC,5000uhash", "5000", true},
		{"5000uhash,7ibc/ABC", "5000", true},
		{" 42uhash , 1foo", "42", true},
		{"1000000000000000000000000000000uhash", "1000000000000000000000000000000", true}, // > uint64
		{"", "", false},
		{"uhash", "", false},
		{"12", "", false},
		{"12uhashx", "", false},    // different denom
		{"12ibc/uhash", "", false}, // denom merely ends in uhash
		{"1foo,2bar", "", false},
		{"-5uhash", "", false},
		{"12 uhash", "", false},
	}
	for _, c := range cases {
		got, ok := uhashAmount(c.in)
		if ok != c.ok {
			t.Errorf("uhashAmount(%q) ok=%v want %v", c.in, ok, c.ok)
			continue
		}
		if ok && got.String() != c.want {
			t.Errorf("uhashAmount(%q)=%s want %s", c.in, got, c.want)
		}
	}
}

func ev(typ string, kv ...string) abciEvent {
	e := abciEvent{Type: typ}
	for i := 0; i+1 < len(kv); i += 2 {
		e.Attributes = append(e.Attributes, eventAttr{Key: kv[i], Value: kv[i+1]})
	}
	return e
}

func TestApplyBalanceEvent(t *testing.T) {
	d := map[string]*big.Int{}
	if !applyBalanceEvent(d, ev("coin_spent", "spender", "hash1a", "amount", "100uhash")) {
		t.Fatal("coin_spent not applied")
	}
	if !applyBalanceEvent(d, ev("coin_received", "receiver", "hash1b", "amount", "100uhash")) {
		t.Fatal("coin_received not applied")
	}
	// Multi-coin amount: only the uhash part counts.
	if !applyBalanceEvent(d, ev("coin_received", "receiver", "hash1a", "amount", "3foo,25uhash")) {
		t.Fatal("multi-coin coin_received not applied")
	}
	// Ignored: other event types, missing address, foreign denom, zero.
	for _, e := range []abciEvent{
		ev("transfer", "sender", "hash1a", "recipient", "hash1b", "amount", "1uhash"),
		ev("coin_spent", "amount", "1uhash"),
		ev("coin_spent", "spender", "hash1a", "amount", "1foo"),
		ev("coin_spent", "spender", "hash1a", "amount", "0uhash"),
		ev("coin_spent", "spender", "hash1a"),
	} {
		if applyBalanceEvent(d, e) {
			t.Errorf("event %+v should have been ignored", e)
		}
	}
	if got := d["hash1a"].String(); got != "-75" {
		t.Errorf("hash1a delta = %s want -75", got)
	}
	if got := d["hash1b"].String(); got != "100" {
		t.Errorf("hash1b delta = %s want 100", got)
	}
	if len(d) != 2 {
		t.Errorf("unexpected addresses in deltas: %v", d)
	}
}

// TestCollectBalanceDeltas covers begin/end-block, finalize-block and tx
// events together, including a failed tx whose fee was still charged.
func TestCollectBalanceDeltas(t *testing.T) {
	raw := `{"result":{
	  "txs_results":[
	    {"code":0,"events":[
	      {"type":"coin_spent","attributes":[{"key":"spender","value":"hash1alice"},{"key":"amount","value":"1000uhash"}]},
	      {"type":"coin_received","attributes":[{"key":"receiver","value":"hash1bob"},{"key":"amount","value":"1000uhash"}]}
	    ]},
	    {"code":5,"events":[
	      {"type":"coin_spent","attributes":[{"key":"spender","value":"hash1alice"},{"key":"amount","value":"10uhash"}]},
	      {"type":"coin_received","attributes":[{"key":"receiver","value":"hash1fee"},{"key":"amount","value":"10uhash"}]}
	    ]}
	  ],
	  "begin_block_events":[{"type":"coin_spent","attributes":[{"key":"spender","value":"hash1fee"},{"key":"amount","value":"4uhash"}]}],
	  "end_block_events":[{"type":"coin_received","attributes":[{"key":"receiver","value":"hash1distr"},{"key":"amount","value":"4uhash"}]}],
	  "finalize_block_events":[
	    {"type":"coin_spent","attributes":[{"key":"spender","value":"hash1bob"},{"key":"amount","value":"1uhash"}]},
	    {"type":"burn","attributes":[{"key":"burner","value":"hash1bob"},{"key":"amount","value":"1uhash"}]}
	  ]}}`
	var r blockResults
	if err := json.Unmarshal([]byte(raw), &r); err != nil {
		t.Fatal(err)
	}
	d := collectBalanceDeltas(&r)
	want := map[string]string{"hash1alice": "-1010", "hash1bob": "999", "hash1fee": "6", "hash1distr": "4"}
	for addr, w := range want {
		if got, ok := d[addr]; !ok || got.String() != w {
			t.Errorf("%s = %v want %s", addr, got, w)
		}
	}
	if len(d) != len(want) {
		t.Errorf("deltas = %v", d)
	}
	// Net supply change is the burn only.
	sum := new(big.Int)
	for _, v := range d {
		sum.Add(sum, v)
	}
	if sum.String() != "-1" {
		t.Errorf("net delta = %s want -1 (the burn)", sum)
	}
	addrs := sortedAddrs(d)
	for i := 1; i < len(addrs); i++ {
		if addrs[i-1] >= addrs[i] {
			t.Fatalf("sortedAddrs not sorted: %v", addrs)
		}
	}
}

func TestSumUhash(t *testing.T) {
	if n, ok := sumUhash(nil); !ok || n.Sign() != 0 {
		t.Fatalf("empty list should be zero, got %v %v", n, ok)
	}
	n, ok := sumUhash([]restCoin{{"uhash", "5"}, {"foo", "9"}, {"uhash", "7"}})
	if !ok || n.String() != "12" {
		t.Fatalf("sum = %v %v want 12", n, ok)
	}
	if _, ok := sumUhash([]restCoin{{"uhash", "x"}}); ok {
		t.Fatal("malformed amount accepted")
	}
	if _, ok := sumUhash([]restCoin{{"uhash", "-1"}}); ok {
		t.Fatal("negative amount accepted")
	}
}

// TestParseGenesisBalancesMainnet reads the embedded mainnet genesis both
// bare and wrapped in the RPC envelope; the sum must equal the bank supply.
func TestParseGenesisBalancesMainnet(t *testing.T) {
	bals, err := parseGenesisBalances(hgparams.MainnetGenesis)
	if err != nil {
		t.Fatal(err)
	}
	if len(bals) != 8 {
		t.Fatalf("got %d genesis balances, want 8", len(bals))
	}
	total := new(big.Int)
	for _, b := range bals {
		if b.Address == "" || b.Amount.Sign() <= 0 {
			t.Errorf("bad genesis balance %+v", b)
		}
		total.Add(total, b.Amount)
	}
	if want := "1000000000000000"; total.String() != want {
		t.Fatalf("genesis total %s want %s (bank supply)", total, want)
	}

	wrapped, _ := json.Marshal(map[string]any{"jsonrpc": "2.0", "id": -1, "result": map[string]any{"genesis": json.RawMessage(hgparams.MainnetGenesis)}})
	again, err := parseGenesisBalances(wrapped)
	if err != nil {
		t.Fatal(err)
	}
	if len(again) != len(bals) {
		t.Fatalf("envelope parse got %d balances, want %d", len(again), len(bals))
	}
	if _, err := parseGenesisBalances([]byte(`{"jsonrpc":"2.0","result":{}}`)); err == nil {
		t.Fatal("empty envelope accepted")
	}
	if _, err := parseGenesisBalances([]byte(`not json`)); err == nil {
		t.Fatal("garbage accepted")
	}
	if _, err := parseGenesisBalances([]byte(`{"app_state":{"bank":{"balances":[{"address":"hash1x","coins":[{"denom":"uhash","amount":"abc"}]}]}}}`)); err == nil {
		t.Fatal("malformed amount accepted")
	}
}

func TestValidatorFromREST(t *testing.T) {
	var m map[string]any
	if err := json.Unmarshal([]byte(`{
	  "operator_address":"hashvaloper1abc","jailed":true,"status":"BOND_STATUS_BONDED","tokens":"123456789012345678901234",
	  "description":{"moniker":"node-1"},
	  "commission":{"commission_rates":{"rate":"0.100000000000000000"}}}`), &m); err != nil {
		t.Fatal(err)
	}
	v, ok := validatorFromREST(m)
	if !ok {
		t.Fatal("valid validator rejected")
	}
	if v.Operator != "hashvaloper1abc" || v.Moniker != "node-1" || v.Tokens.String() != "123456789012345678901234" ||
		v.CommissionRate != "0.100000000000000000" || v.Status != "bonded" || !v.Jailed {
		t.Fatalf("unexpected row %+v", v)
	}
	if _, ok := validatorFromREST(map[string]any{"tokens": "1"}); ok {
		t.Fatal("validator without operator accepted")
	}
	if _, ok := validatorFromREST(map[string]any{"operator_address": "x", "tokens": "many"}); ok {
		t.Fatal("validator with unparsable tokens accepted")
	}
	for in, want := range map[string]string{"BOND_STATUS_UNBONDING": "unbonding", "BOND_STATUS_UNBONDED": "unbonded", "bonded": "bonded", "": ""} {
		if got := normalizeBondStatus(in); got != want {
			t.Errorf("normalizeBondStatus(%q)=%q want %q", in, got, want)
		}
	}
}

func TestValidatorsDue(t *testing.T) {
	if !validatorsDue(0, 1, 50) {
		t.Error("first refresh should be due")
	}
	if validatorsDue(100, 149, 50) {
		t.Error("refresh due too early")
	}
	if !validatorsDue(100, 150, 50) {
		t.Error("refresh not due at the interval")
	}
	if !validatorsDue(100, 150, 0) {
		t.Error("zero interval should fall back to the default of 50")
	}
}

func TestConfigDefaults(t *testing.T) {
	var c Config
	c.Defaults()
	if c.ValidatorRefreshBlocks != 50 {
		t.Errorf("validator_refresh_blocks default = %d want 50", c.ValidatorRefreshBlocks)
	}
	if c.GenesisPath != "" {
		t.Errorf("genesis_path default should be empty (fetch from RPC), got %q", c.GenesisPath)
	}
	c = Config{ValidatorRefreshBlocks: 7}
	c.Defaults()
	if c.ValidatorRefreshBlocks != 7 {
		t.Errorf("explicit validator_refresh_blocks overridden: %d", c.ValidatorRefreshBlocks)
	}
}

func TestPaginationIncompleteError(t *testing.T) {
	err := error(&errPaginationIncomplete{path: "/x", pages: 1000, rows: 200000})
	if err.Error() == "" {
		t.Fatal("empty message")
	}
	var e *errPaginationIncomplete
	if !errors.As(err, &e) || e.rows != 200000 {
		t.Fatal("error type lost")
	}
}
