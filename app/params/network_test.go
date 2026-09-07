package params_test

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"strings"
	"testing"

	"github.com/hashgram/hashgram/app/params"
)

func mainnetHash() string {
	sum := sha256.Sum256([]byte(`{"chain_id":"hashgram-1"}`))
	return hex.EncodeToString(sum[:])
}

func foreignHash() string {
	sum := sha256.Sum256([]byte(`{"chain_id":"hashgram-1","forked":true}`))
	return hex.EncodeToString(sum[:])
}

// TestMainnetIdentityConstants pins the Mainnet identifiers from §9.
func TestMainnetIdentityConstants(t *testing.T) {
	id := params.MainnetIdentity(mainnetHash())

	if id.NetworkName != "Hashgram Mainnet" {
		t.Errorf("network_name = %q", id.NetworkName)
	}
	if id.NetworkID != "hashgram-mainnet" {
		t.Errorf("network_id = %q", id.NetworkID)
	}
	if id.ChainID != "hashgram-1" {
		t.Errorf("chain_id = %q", id.ChainID)
	}
	if string(id.NetworkMagic[:]) != "HGM1" {
		t.Errorf("network_magic = %q", string(id.NetworkMagic[:]))
	}
	if id.ProtocolMajorVersion != 1 {
		t.Errorf("protocol_major_version = %d", id.ProtocolMajorVersion)
	}
	if !id.IsMainnet() {
		t.Error("IsMainnet() = false for the mainnet identity")
	}
	if id.IsDevnet() {
		t.Error("IsDevnet() = true for the mainnet identity")
	}
	if err := id.Validate(); err != nil {
		t.Errorf("Validate() = %v", err)
	}
}

// TestDevnetIsNotMainnet: a devnet node must never be mistaken for Mainnet.
func TestDevnetIsNotMainnet(t *testing.T) {
	dev := params.DevnetIdentity(mainnetHash())
	if dev.IsMainnet() {
		t.Fatal("devnet identity reports IsMainnet() = true")
	}
	if !dev.IsDevnet() {
		t.Fatal("devnet identity reports IsDevnet() = false")
	}
	if dev.NetworkMagic == params.NetworkMagicMainnet {
		t.Fatal("devnet shares the mainnet network magic")
	}
}

// TestForeignForkIsRejected is the §89 acceptance test at the identity layer:
// clone the software, generate a different genesis, try to join Mainnet.
func TestForeignForkIsRejected(t *testing.T) {
	local := params.MainnetIdentity(mainnetHash())

	tests := []struct {
		name   string
		remote params.NetworkIdentity
		reason string
	}{
		{
			name: "different genesis hash, everything else identical",
			remote: params.NetworkIdentity{
				NetworkName:          params.NetworkNameMainnet,
				NetworkID:            params.NetworkIDMainnet,
				ChainID:              params.ChainIDMainnet,
				NetworkMagic:         params.NetworkMagicMainnet,
				ProtocolMajorVersion: params.ProtocolMajorVersion,
				GenesisHash:          foreignHash(),
			},
			reason: "genesis_hash",
		},
		{
			name:   "devnet peer",
			remote: params.DevnetIdentity(mainnetHash()),
			reason: "network_id",
		},
		{
			name: "spoofed magic",
			remote: params.NetworkIdentity{
				NetworkName:          params.NetworkNameMainnet,
				NetworkID:            params.NetworkIDMainnet,
				ChainID:              params.ChainIDMainnet,
				NetworkMagic:         [4]byte{'X', 'X', 'X', 'X'},
				ProtocolMajorVersion: params.ProtocolMajorVersion,
				GenesisHash:          mainnetHash(),
			},
			reason: "network_magic",
		},
		{
			name: "future protocol generation",
			remote: params.NetworkIdentity{
				NetworkName:          params.NetworkNameMainnet,
				NetworkID:            params.NetworkIDMainnet,
				ChainID:              params.ChainIDMainnet,
				NetworkMagic:         params.NetworkMagicMainnet,
				ProtocolMajorVersion: 2,
				GenesisHash:          mainnetHash(),
			},
			reason: "protocol_major_version",
		},
		{
			name: "different chain id",
			remote: params.NetworkIdentity{
				NetworkName:          params.NetworkNameMainnet,
				NetworkID:            params.NetworkIDMainnet,
				ChainID:              "hashgram-2",
				NetworkMagic:         params.NetworkMagicMainnet,
				ProtocolMajorVersion: params.ProtocolMajorVersion,
				GenesisHash:          mainnetHash(),
			},
			reason: "chain_id",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			err := local.VerifyPeer(tc.remote)
			if err == nil {
				t.Fatal("REJECTED expected, but peer was ACCEPTED")
			}
			if !errors.Is(err, params.ErrForeignNetwork) {
				t.Fatalf("error %v is not ErrForeignNetwork", err)
			}
			if !strings.Contains(err.Error(), tc.reason) {
				t.Errorf("rejection reason %q does not mention %q", err, tc.reason)
			}
		})
	}
}

// TestGenuineMainnetPeerIsAccepted ensures the check is not vacuously strict.
func TestGenuineMainnetPeerIsAccepted(t *testing.T) {
	h := mainnetHash()
	local := params.MainnetIdentity(h)
	remote := params.MainnetIdentity(strings.ToUpper(h)) // case must not matter

	if err := local.VerifyPeer(remote); err != nil {
		t.Fatalf("genuine mainnet peer rejected: %v", err)
	}
}

// TestUnpinnedGenesisRefusesAllPeers: "unknown" must never be more permissive
// than "known".
func TestUnpinnedGenesisRefusesAllPeers(t *testing.T) {
	local := params.MainnetIdentity("")
	remote := params.MainnetIdentity(mainnetHash())

	err := local.VerifyPeer(remote)
	if err == nil {
		t.Fatal("a node with no pinned genesis accepted a peer")
	}
	if !errors.Is(err, params.ErrForeignNetwork) {
		t.Fatalf("error %v is not ErrForeignNetwork", err)
	}
}

// TestIdentityValidateRejectsPartialMainnetClaims blocks the
// "claim the mainnet id but keep my own magic" impersonation.
func TestIdentityValidateRejectsPartialMainnetClaims(t *testing.T) {
	bad := params.NetworkIdentity{
		NetworkName:          "Totally Hashgram",
		NetworkID:            params.NetworkIDMainnet,
		ChainID:              "not-hashgram-1",
		NetworkMagic:         params.NetworkMagicMainnet,
		ProtocolMajorVersion: 1,
	}
	if err := bad.Validate(); err == nil {
		t.Fatal("identity claiming mainnet network_id with a foreign chain_id validated")
	}

	bad2 := params.NetworkIdentity{
		NetworkName:          "Totally Hashgram",
		NetworkID:            params.NetworkIDMainnet,
		ChainID:              params.ChainIDMainnet,
		NetworkMagic:         [4]byte{'N', 'O', 'P', 'E'},
		ProtocolMajorVersion: 1,
	}
	if err := bad2.Validate(); err == nil {
		t.Fatal("identity claiming mainnet network_id with a foreign magic validated")
	}
}

// TestValidateRejectsEmptyFields
func TestValidateRejectsEmptyFields(t *testing.T) {
	var zero params.NetworkIdentity
	if err := zero.Validate(); err == nil {
		t.Fatal("zero identity validated")
	}

	id := params.MainnetIdentity(mainnetHash())
	id.ProtocolMajorVersion = 0
	if err := id.Validate(); err == nil {
		t.Fatal("protocol_major_version 0 validated")
	}
}

// TestValidateGenesisHash
func TestValidateGenesisHash(t *testing.T) {
	good := mainnetHash()
	if err := params.ValidateGenesisHash(good); err != nil {
		t.Fatalf("valid hash rejected: %v", err)
	}
	for _, bad := range []string{
		"",
		"abc",
		strings.ToUpper(good),
		good[:63] + "z",
		good + "00",
	} {
		if err := params.ValidateGenesisHash(bad); err == nil {
			t.Errorf("invalid hash %q accepted", bad)
		}
	}
}

// TestComputeGenesisHashIsSha256OfRawBytes documents the exact definition, so
// that an operator running `sha256sum genesis.json` gets the same answer.
func TestComputeGenesisHashIsSha256OfRawBytes(t *testing.T) {
	raw := []byte("{\n  \"chain_id\": \"hashgram-1\"\n}\n")
	sum := sha256.Sum256(raw)
	want := hex.EncodeToString(sum[:])

	if got := params.ComputeGenesisHash(raw); got != want {
		t.Fatalf("ComputeGenesisHash = %s, want sha256sum = %s", got, want)
	}
	// Whitespace matters: a reformatted file is a different file.
	if params.ComputeGenesisHash([]byte(`{"chain_id":"hashgram-1"}`)) == want {
		t.Fatal("reformatted genesis produced the same hash")
	}
}

// ---------------------------------------------------------------------------
// Signature domain separation
// ---------------------------------------------------------------------------

// TestSigningDomainSeparatesPurposes: a signature made for one purpose must
// not be valid for another.
func TestSigningDomainSeparatesPurposes(t *testing.T) {
	id := params.MainnetIdentity(mainnetHash())
	payload := []byte("same bytes for every purpose")

	seen := map[[32]byte]params.SigningPurpose{}
	purposes := []params.SigningPurpose{
		params.PurposeSocialEvent,
		params.PurposeDeviceCert,
		params.PurposeServiceReceipt,
		params.PurposeStorageChallenge,
		params.PurposeEligibility,
		params.PurposeContentAttest,
		params.PurposeBootstrapRecord,
		params.PurposePeerHandshake,
		params.PurposeNodeAnnounce,
		params.PurposeMailboxFetch,
		params.PurposeMailboxAck,
		params.PurposeKeyPackage,
		params.PurposeBlobUpload,
	}
	for _, p := range purposes {
		d := id.SigningDigest(p, payload)
		if other, dup := seen[d]; dup {
			t.Fatalf("purposes %q and %q produce the same digest", p, other)
		}
		seen[d] = p
	}
	if len(seen) != len(purposes) {
		t.Fatalf("got %d distinct digests for %d purposes", len(seen), len(purposes))
	}
}

// TestSigningDomainSeparatesNetworks: §10 replay protection. A social event
// signed on a fork must not verify on Mainnet.
func TestSigningDomainSeparatesNetworks(t *testing.T) {
	mainnet := params.MainnetIdentity(mainnetHash())
	devnet := params.DevnetIdentity(mainnetHash())
	payload := []byte("post: hello world")

	if mainnet.SigningDigest(params.PurposeSocialEvent, payload) ==
		devnet.SigningDigest(params.PurposeSocialEvent, payload) {
		t.Fatal("mainnet and devnet produce identical signing digests; " +
			"events would be replayable across networks")
	}

	// Also across protocol generations.
	future := mainnet
	future.ProtocolMajorVersion = 2
	if mainnet.SigningDigest(params.PurposeSocialEvent, payload) ==
		future.SigningDigest(params.PurposeSocialEvent, payload) {
		t.Fatal("protocol major version is not part of the signing domain")
	}
}

// TestSigningPreimageIsUnambiguous: without length prefixes,
// (domain "ab", payload "c") and (domain "a", payload "bc") would collide.
// We cannot vary the domain directly, so we verify the framing explicitly.
func TestSigningPreimageIsUnambiguous(t *testing.T) {
	id := params.MainnetIdentity(mainnetHash())

	pre := id.SigningPreimage(params.PurposeSocialEvent, []byte("payload"))
	domain := []byte(id.SigningDomain(params.PurposeSocialEvent))

	if !bytes.HasPrefix(pre, id.NetworkMagic[:]) {
		t.Fatal("preimage does not begin with the network magic")
	}

	// magic(4) + uint64 len + domain + uint64 len + payload.
	//
	// The length prefixes are 64-bit because this is the outer wrapper and it
	// frames a whole inner preimage of arbitrary length rather than a bounded
	// field. A 32-bit prefix would need an overflow check that has no
	// meaningful handling at this layer; a 64-bit prefix cannot overflow,
	// because a Go len() is a non-negative int. See app/canonical.
	const prefixLen = 8
	wantLen := len(id.NetworkMagic) + prefixLen + len(domain) + prefixLen + len("payload")
	if len(pre) != wantLen {
		t.Fatalf("preimage length = %d, want %d", len(pre), wantLen)
	}

	magicLen := len(id.NetworkMagic)
	gotLen := binary.BigEndian.Uint64(pre[magicLen : magicLen+prefixLen])
	if gotLen != uint64(len(domain)) {
		t.Fatalf("encoded domain length = %d, want %d", gotLen, len(domain))
	}

	domainStart := magicLen + prefixLen
	if !bytes.Equal(pre[domainStart:domainStart+len(domain)], domain) {
		t.Fatal("domain bytes are not where the framing says they are")
	}

	// And the payload length must be committed to as well, which is what
	// stops a payload being extended without changing the digest.
	payloadLenAt := domainStart + len(domain)
	gotPayloadLen := binary.BigEndian.Uint64(pre[payloadLenAt : payloadLenAt+prefixLen])
	if gotPayloadLen != uint64(len("payload")) {
		t.Fatalf("encoded payload length = %d, want %d", gotPayloadLen, len("payload"))
	}

	// Distinct payloads must give distinct digests even when one is a prefix
	// of the other.
	a := id.SigningDigest(params.PurposeSocialEvent, []byte("ab"))
	b := id.SigningDigest(params.PurposeSocialEvent, []byte("abc"))
	if a == b {
		t.Fatal("payload length is not committed to")
	}
}

// TestSigningDomainFormat documents the exact string, because the Rust P2P
// layer and every client SDK must reproduce it byte for byte.
func TestSigningDomainFormat(t *testing.T) {
	id := params.MainnetIdentity(mainnetHash())
	got := id.SigningDomain(params.PurposeSocialEvent)
	want := "hashgram/v1/hashgram-mainnet/social-event"
	if got != want {
		t.Fatalf("signing domain = %q, want %q", got, want)
	}
}
