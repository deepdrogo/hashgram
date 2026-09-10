package params

import (
	"encoding/json"
	"testing"
)

// The embedded genesis is the launch artefact. If anyone reformats, re-saves
// or "fixes" the file, the hash changes and every join-mainnet built from
// that tree would refuse the real network. This test is the guard.
func TestMainnetGenesisMatchesPin(t *testing.T) {
	if err := ValidateGenesisHash(MainnetGenesisHash); err != nil {
		t.Fatalf("MainnetGenesisHash constant is malformed: %v", err)
	}
	got := ComputeGenesisHash(MainnetGenesis)
	if got != MainnetGenesisHash {
		t.Fatalf("embedded mainnet/genesis.json hashes to %s, pin is %s", got, MainnetGenesisHash)
	}

	var doc struct {
		ChainID     string `json:"chain_id"`
		GenesisTime string `json:"genesis_time"`
	}
	if err := json.Unmarshal(MainnetGenesis, &doc); err != nil {
		t.Fatalf("embedded genesis does not parse: %v", err)
	}
	if doc.ChainID != ChainIDMainnet {
		t.Fatalf("embedded genesis chain_id %q != %q", doc.ChainID, ChainIDMainnet)
	}
	if doc.GenesisTime != MainnetGenesisTime {
		t.Fatalf("embedded genesis_time %q != constant %q", doc.GenesisTime, MainnetGenesisTime)
	}
}

func TestMainnetSeedsAreWellFormed(t *testing.T) {
	seeds := MainnetSeeds()
	if len(seeds) == 0 {
		t.Fatal("no built-in CometBFT seeds; a fresh node could not find the network")
	}
	for _, s := range seeds {
		if err := ValidateCometSeed(s); err != nil {
			t.Error(err)
		}
	}
}

func TestMainnetBootstrapPeersAreWellFormed(t *testing.T) {
	peers := MainnetBootstrapPeers()
	if len(peers) == 0 {
		t.Fatal("no built-in bootstrap peers; a fresh hashgram-node could not find the network")
	}
	for _, p := range peers {
		if err := ValidateBootstrapMultiaddr(p); err != nil {
			t.Error(err)
		}
	}
	for _, d := range MainnetDNSSeeds() {
		if len(d) == 0 || d[0] == '/' || d[0] == '_' {
			t.Errorf("dns seed %q must be a bare host name", d)
		}
	}
}

func TestValidateCometSeedRejectsGarbage(t *testing.T) {
	bad := []string{
		"",
		"186.241.19.230:26656",
		"2f6629254d6568e48a09aff20ba01a61a26c47f0@186.241.19.230",
		"2F6629254D6568E48A09AFF20BA01A61A26C47F0@186.241.19.230:26656",
		"2f6629254d6568e48a09aff20ba01a61a26c47f0@:26656",
		"2f6629254d6568e48a09aff20ba01a61a26c47f0@186.241.19.230:70000",
	}
	for _, s := range bad {
		if ValidateCometSeed(s) == nil {
			t.Errorf("accepted %q", s)
		}
	}
	if err := ValidateBootstrapMultiaddr("/ip4/203.0.113.1/udp/26670/quic-v1"); err == nil {
		t.Error("accepted a multiaddr without a peer id")
	}
}

func TestParseListFileSkipsCommentsAndBlanks(t *testing.T) {
	got := parseListFile("# c\n\n  a  \n#b\nb\n")
	if len(got) != 2 || got[0] != "a" || got[1] != "b" {
		t.Fatalf("got %v", got)
	}
}
