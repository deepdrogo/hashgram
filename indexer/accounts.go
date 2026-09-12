package indexer

import (
	"crypto/sha256"
	"sort"
	"strings"
)

// Module accounts.
//
// The chain derives a module account address as sha256(name)[:20] encoded
// with the account bech32 prefix (authtypes.NewModuleAddress). Re-deriving
// the list here, rather than importing the application, keeps the indexer
// binary free of the whole Cosmos SDK app graph while still classifying
// holders the way the chain does. TestModuleAccountsMatchGenesis pins the
// derivation against the embedded mainnet genesis.
//
// The names are the keys of app.maccPerms plus the x/treasury reserve
// sub-accounts (docs/TOKENOMICS.md). A new module account added to the
// application must be added here too, or it will rank as a plain account.

// accountBech32Prefix is params.Bech32PrefixAccAddr for Hashgram.
const accountBech32Prefix = "hash"

// moduleAccountNames is the compile-time list of module account names.
var moduleAccountNames = []string{
	// Cosmos SDK.
	"fee_collector",
	"distribution",
	"bonded_tokens_pool",
	"not_bonded_tokens_pool",
	"gov",
	// Hashgram modules.
	"founder",
	"feerouter",
	"welcome",
	"serviceproof",
	"serviceproof_bond",
	// x/treasury reserve sub-accounts, one per genesis allocation.
	"treasury_treasury",
	"treasury_dev_grants",
	"treasury_liquidity",
	"treasury_growth",
}

// moduleAccounts maps address -> module name for every known module account.
var moduleAccounts = func() map[string]string {
	m := make(map[string]string, len(moduleAccountNames))
	for _, n := range moduleAccountNames {
		m[moduleAddress(n)] = n
	}
	return m
}()

// moduleAddress derives the bech32 account address of a module account.
func moduleAddress(name string) string {
	sum := sha256.Sum256([]byte(name))
	return bech32Encode(accountBech32Prefix, sum[:20])
}

// Account kinds reported by the holders leaderboard.
const (
	kindModule  = "module"
	kindAccount = "account"
)

// accountKind classifies an address for the holders leaderboard: "module"
// for a known module account, "account" for everything else. The second
// value is the module name when kind is "module".
func accountKind(addr string) (kind, label string) {
	if n, ok := moduleAccounts[addr]; ok {
		return kindModule, n
	}
	return kindAccount, ""
}

// moduleAccountList returns the known module accounts sorted by name, for
// documentation and tests.
func moduleAccountList() []struct{ Name, Address string } {
	out := make([]struct{ Name, Address string }, 0, len(moduleAccounts))
	for addr, name := range moduleAccounts {
		out = append(out, struct{ Name, Address string }{name, addr})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// ---------------------------------------------------------------------------
// bech32 (BIP-173), encode only. Cosmos addresses use the original bech32
// checksum constant (1), not bech32m.
// ---------------------------------------------------------------------------

const bech32Charset = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"

var bech32Generator = [5]uint32{0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3}

func bech32Polymod(values []byte) uint32 {
	chk := uint32(1)
	for _, v := range values {
		top := chk >> 25
		chk = (chk&0x1ffffff)<<5 ^ uint32(v)
		for i := 0; i < 5; i++ {
			if (top>>uint(i))&1 == 1 {
				chk ^= bech32Generator[i]
			}
		}
	}
	return chk
}

func bech32HrpExpand(hrp string) []byte {
	out := make([]byte, 0, len(hrp)*2+1)
	for i := 0; i < len(hrp); i++ {
		out = append(out, hrp[i]>>5)
	}
	out = append(out, 0)
	for i := 0; i < len(hrp); i++ {
		out = append(out, hrp[i]&31)
	}
	return out
}

func bech32Checksum(hrp string, data []byte) []byte {
	values := append(bech32HrpExpand(hrp), data...)
	values = append(values, 0, 0, 0, 0, 0, 0)
	polymod := bech32Polymod(values) ^ 1
	out := make([]byte, 6)
	for i := range out {
		out[i] = byte((polymod >> uint(5*(5-i))) & 31)
	}
	return out
}

// convertBits8to5 regroups bytes into 5-bit symbols with zero padding.
func convertBits8to5(data []byte) []byte {
	out := make([]byte, 0, (len(data)*8+4)/5)
	var acc uint32
	bits := uint(0)
	for _, b := range data {
		acc = acc<<8 | uint32(b)
		bits += 8
		for bits >= 5 {
			bits -= 5
			out = append(out, byte((acc>>bits)&31))
		}
	}
	if bits > 0 {
		out = append(out, byte((acc<<(5-bits))&31))
	}
	return out
}

// bech32Encode renders hrp + "1" + data + checksum.
func bech32Encode(hrp string, data []byte) string {
	hrp = strings.ToLower(hrp)
	d := convertBits8to5(data)
	var sb strings.Builder
	sb.Grow(len(hrp) + 1 + len(d) + 6)
	sb.WriteString(hrp)
	sb.WriteByte('1')
	for _, v := range d {
		sb.WriteByte(bech32Charset[v])
	}
	for _, v := range bech32Checksum(hrp, d) {
		sb.WriteByte(bech32Charset[v])
	}
	return sb.String()
}
