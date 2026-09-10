package params

import (
	"bufio"
	_ "embed"
	"fmt"
	"net"
	"regexp"
	"strconv"
	"strings"
)

// ---------------------------------------------------------------------------
// Mainnet launch facts (docs/DECENTRALIZATION.md §Bootstrap peers)
// ---------------------------------------------------------------------------
//
// Hashgram Mainnet (chain-id hashgram-1) was launched on 2026-09-10. From that
// moment its genesis file is a historical fact, and this package carries it
// the same way bitcoind carries its genesis block and its fixed seed list:
// compiled in, so that a fresh machine running
//
//	hashgramctl join-mainnet
//
// with no arguments obtains the genesis, verifies it against the hash below,
// and knows at least one address to dial. The operator no longer has to be
// told the hash out of band; the binary they chose to run is the out-of-band
// channel, exactly as it is for every other chain parameter in this package.
//
// Nothing here is consulted by consensus. A node that has ever been online
// keeps its own address book and never reads these lists again. They are for
// first contact, and every entry is a point of trust chosen at build time.
// The fix, as with Bitcoin's seeds, is more entries from more independent
// operators, which is why the data lives in plain text files that a pull
// request can extend.

// MainnetGenesisHash is the lowercase hex SHA-256 of the canonical Mainnet
// genesis file, exactly as it was written by `hashgramctl finalize-genesis`
// at launch. `MainnetGenesis` hashes to this value; TestMainnetGenesisMatchesPin
// enforces it.
const MainnetGenesisHash = "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d"

// MainnetGenesisTime is the genesis_time recorded in the file, for display.
const MainnetGenesisTime = "2026-09-10T13:01:34.695033422Z"

// MainnetGenesis is the canonical Mainnet genesis file, byte for byte.
//
//go:embed mainnet/genesis.json
var MainnetGenesis []byte

//go:embed mainnet/seeds.txt
var mainnetSeedsFile string

//go:embed mainnet/bootstrap_peers.txt
var mainnetBootstrapPeersFile string

//go:embed mainnet/dns_seeds.txt
var mainnetDNSSeedsFile string

// MainnetSeeds returns the built-in CometBFT seed list for hashgramd, in the
// `<node-id>@<host>:<port>` form config.toml expects.
func MainnetSeeds() []string { return parseListFile(mainnetSeedsFile) }

// MainnetBootstrapPeers returns the built-in libp2p bootstrap multiaddrs for
// hashgram-node. Each ends in /p2p/<peer-id>.
func MainnetBootstrapPeers() []string { return parseListFile(mainnetBootstrapPeersFile) }

// MainnetDNSSeeds returns the built-in /dnsaddr names for hashgram-node. It is
// empty until an operator publishes one.
func MainnetDNSSeeds() []string { return parseListFile(mainnetDNSSeedsFile) }

// parseListFile returns the non-empty, non-comment lines of a list file.
func parseListFile(s string) []string {
	var out []string
	sc := bufio.NewScanner(strings.NewReader(s))
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		out = append(out, line)
	}
	return out
}

var cometNodeIDRe = regexp.MustCompile(`^[0-9a-f]{40}$`)

// ValidateCometSeed checks a `<node-id>@<host>:<port>` entry: 40 lowercase
// hex characters, a non-empty host, and a port in range.
func ValidateCometSeed(s string) error {
	id, hostport, ok := strings.Cut(s, "@")
	if !ok {
		return fmt.Errorf("seed %q: expected <node-id>@<host>:<port>", s)
	}
	if !cometNodeIDRe.MatchString(id) {
		return fmt.Errorf("seed %q: node id must be 40 lowercase hex characters", s)
	}
	host, port, err := net.SplitHostPort(hostport)
	if err != nil {
		return fmt.Errorf("seed %q: %w", s, err)
	}
	if host == "" {
		return fmt.Errorf("seed %q: empty host", s)
	}
	p, err := strconv.Atoi(port)
	if err != nil || p < 1 || p > 65535 {
		return fmt.Errorf("seed %q: port out of range", s)
	}
	return nil
}

// ValidateBootstrapMultiaddr checks the shape hashgram-node requires: a
// multiaddr ending in /p2p/<peer-id>. Full multiaddr parsing happens in the
// Rust node at startup; this catches the mistakes a text edit can make.
func ValidateBootstrapMultiaddr(s string) error {
	if !strings.HasPrefix(s, "/") {
		return fmt.Errorf("bootstrap peer %q: multiaddr must start with '/'", s)
	}
	i := strings.LastIndex(s, "/p2p/")
	if i < 0 {
		return fmt.Errorf("bootstrap peer %q: missing /p2p/<peer-id>", s)
	}
	peer := s[i+len("/p2p/"):]
	if len(peer) < 46 || strings.ContainsAny(peer, "/ \t") {
		return fmt.Errorf("bootstrap peer %q: malformed peer id", s)
	}
	return nil
}
