package params

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"

	"github.com/hashgram/hashgram/app/canonical"
)

// ---------------------------------------------------------------------------
// Network identity (docs/DECENTRALIZATION.md §Fork isolation)
// ---------------------------------------------------------------------------
//
// Hashgram Mainnet is defined by cryptographic identity, not by a domain name,
// a git remote, or an IP address. Five values together form that identity:
//
//	network_name           human-readable label
//	network_id             stable machine identifier
//	chain_id               CometBFT chain identifier
//	network_magic          4-byte P2P wire discriminator
//	protocol_major_version wire-compatibility generation
//
// plus the genesis_hash, which cannot exist as a compile-time constant because
// it is the hash of the genesis file produced at launch. It is pinned into the
// node configuration and verified on every peer handshake.
//
// Anybody may copy this software. Copying it does not produce Hashgram
// Mainnet: a different genesis file yields a different genesis hash, and the
// handshake in x/network (and in the Rust P2P layer) rejects the peer.

const (
	// NetworkNameMainnet is the human-readable network name.
	NetworkNameMainnet = "Hashgram Mainnet"

	// NetworkIDMainnet is the stable machine identifier for Mainnet.
	NetworkIDMainnet = "hashgram-mainnet"

	// ChainIDMainnet is the CometBFT chain-id for Mainnet.
	ChainIDMainnet = "hashgram-1"

	// NetworkNameDevnet / NetworkIDDevnet / ChainIDDevnet identify the
	// throwaway local development network. Nodes carrying these identifiers
	// can never handshake with Mainnet peers, and
	// `hashgramctl mainnet-preflight` fails if it finds them.
	NetworkNameDevnet = "Hashgram Devnet (DEVNET ONLY)"
	NetworkIDDevnet   = "hashgram-devnet"
	ChainIDDevnet     = "hashgram-devnet-1"

	// ProtocolMajorVersion is the wire-compatibility generation. Peers must
	// agree on this exactly. Bumping it is a hard network split and is only
	// done through a coordinated x/upgrade migration.
	ProtocolMajorVersion uint32 = 1
)

// NetworkMagicMainnet is the 4-byte discriminator prefixed to Hashgram P2P
// frames and mixed into signature domains. It is the ASCII bytes "HGM1".
//
// It is intentionally *not* derived from the network id at runtime: a constant
// makes it trivial for an operator to eyeball whether two binaries belong to
// the same network.
var NetworkMagicMainnet = [4]byte{'H', 'G', 'M', '1'}

// NetworkMagicDevnet is the devnet discriminator, ASCII "HGD1".
var NetworkMagicDevnet = [4]byte{'H', 'G', 'D', '1'}

// ---------------------------------------------------------------------------
// NetworkIdentity
// ---------------------------------------------------------------------------

// NetworkIdentity is the full identity of a Hashgram network. Two nodes may
// exchange application data only if their identities are Equal.
type NetworkIdentity struct {
	NetworkName          string
	NetworkID            string
	ChainID              string
	NetworkMagic         [4]byte
	ProtocolMajorVersion uint32

	// GenesisHash is the lowercase hex SHA-256 of the canonical genesis file.
	// Empty means "not yet pinned", which is only legitimate before genesis
	// has been created.
	GenesisHash string
}

// MainnetIdentity returns the Mainnet identity with the supplied genesis hash
// pinned. Pass the empty string before genesis exists.
func MainnetIdentity(genesisHash string) NetworkIdentity {
	return NetworkIdentity{
		NetworkName:          NetworkNameMainnet,
		NetworkID:            NetworkIDMainnet,
		ChainID:              ChainIDMainnet,
		NetworkMagic:         NetworkMagicMainnet,
		ProtocolMajorVersion: ProtocolMajorVersion,
		GenesisHash:          strings.ToLower(genesisHash),
	}
}

// DevnetIdentity returns the DEVNET ONLY identity.
func DevnetIdentity(genesisHash string) NetworkIdentity {
	return NetworkIdentity{
		NetworkName:          NetworkNameDevnet,
		NetworkID:            NetworkIDDevnet,
		ChainID:              ChainIDDevnet,
		NetworkMagic:         NetworkMagicDevnet,
		ProtocolMajorVersion: ProtocolMajorVersion,
		GenesisHash:          strings.ToLower(genesisHash),
	}
}

// IsMainnet reports whether this identity claims to be Hashgram Mainnet.
func (n NetworkIdentity) IsMainnet() bool {
	return n.NetworkID == NetworkIDMainnet &&
		n.ChainID == ChainIDMainnet &&
		n.NetworkMagic == NetworkMagicMainnet
}

// IsDevnet reports whether this identity is the development network.
func (n NetworkIdentity) IsDevnet() bool {
	return n.NetworkID == NetworkIDDevnet || n.ChainID == ChainIDDevnet
}

// Validate checks internal consistency of an identity.
func (n NetworkIdentity) Validate() error {
	if n.NetworkName == "" {
		return errors.New("network_name must not be empty")
	}
	if n.NetworkID == "" {
		return errors.New("network_id must not be empty")
	}
	if n.ChainID == "" {
		return errors.New("chain_id must not be empty")
	}
	if n.NetworkMagic == ([4]byte{}) {
		return errors.New("network_magic must not be zero")
	}
	if n.ProtocolMajorVersion == 0 {
		return errors.New("protocol_major_version must be >= 1")
	}
	if n.GenesisHash != "" {
		if err := ValidateGenesisHash(n.GenesisHash); err != nil {
			return err
		}
	}
	// A node claiming the Mainnet network_id must carry every other Mainnet
	// identifier too. This blocks the "same chain-id, different magic"
	// class of impersonation.
	if n.NetworkID == NetworkIDMainnet {
		if n.ChainID != ChainIDMainnet {
			return fmt.Errorf("network_id %q requires chain_id %q, got %q",
				NetworkIDMainnet, ChainIDMainnet, n.ChainID)
		}
		if n.NetworkMagic != NetworkMagicMainnet {
			return fmt.Errorf("network_id %q requires network_magic %q, got %q",
				NetworkIDMainnet, string(NetworkMagicMainnet[:]), string(n.NetworkMagic[:]))
		}
	}
	return nil
}

// ErrForeignNetwork is returned when a peer belongs to a different network.
var ErrForeignNetwork = errors.New("peer belongs to a foreign network")

// VerifyPeer decides whether a remote identity may join this network.
//
// Every field must match. In particular the genesis hash must match: this is
// what makes a fork of the software with a fresh genesis a *different network*
// rather than a participant in Hashgram Mainnet.
//
// An empty local GenesisHash means the local node has not yet pinned its
// genesis; in that state we refuse to accept any peer rather than accept all
// of them, because "unknown" must never be more permissive than "known".
func (n NetworkIdentity) VerifyPeer(remote NetworkIdentity) error {
	if n.GenesisHash == "" {
		return fmt.Errorf("%w: local genesis hash is not pinned; refusing all peers", ErrForeignNetwork)
	}
	if remote.NetworkID != n.NetworkID {
		return fmt.Errorf("%w: network_id %q != %q", ErrForeignNetwork, remote.NetworkID, n.NetworkID)
	}
	if remote.ChainID != n.ChainID {
		return fmt.Errorf("%w: chain_id %q != %q", ErrForeignNetwork, remote.ChainID, n.ChainID)
	}
	if remote.NetworkMagic != n.NetworkMagic {
		return fmt.Errorf("%w: network_magic %q != %q", ErrForeignNetwork,
			string(remote.NetworkMagic[:]), string(n.NetworkMagic[:]))
	}
	if remote.ProtocolMajorVersion != n.ProtocolMajorVersion {
		return fmt.Errorf("%w: protocol_major_version %d != %d", ErrForeignNetwork,
			remote.ProtocolMajorVersion, n.ProtocolMajorVersion)
	}
	if !strings.EqualFold(remote.GenesisHash, n.GenesisHash) {
		return fmt.Errorf("%w: genesis_hash %s != %s", ErrForeignNetwork,
			shortHash(remote.GenesisHash), shortHash(n.GenesisHash))
	}
	return nil
}

func shortHash(h string) string {
	if h == "" {
		return "<unset>"
	}
	if len(h) <= 16 {
		return h
	}
	return h[:8] + ".." + h[len(h)-8:]
}

// ValidateGenesisHash checks that s is a lowercase hex SHA-256 digest.
func ValidateGenesisHash(s string) error {
	if len(s) != sha256.Size*2 {
		return fmt.Errorf("genesis_hash must be %d hex characters, got %d", sha256.Size*2, len(s))
	}
	if s != strings.ToLower(s) {
		return errors.New("genesis_hash must be lowercase hex")
	}
	if _, err := hex.DecodeString(s); err != nil {
		return fmt.Errorf("genesis_hash is not valid hex: %w", err)
	}
	return nil
}

// ComputeGenesisHash returns the lowercase hex SHA-256 of the raw genesis file
// bytes.
//
// The hash is taken over the exact bytes on disk, not over a re-serialisation
// of the parsed JSON. Two operators who ran `sha256sum genesis.json` and got
// the same answer have the same network; that property is worth more than
// canonicalisation cleverness.
func ComputeGenesisHash(genesisFileBytes []byte) string {
	sum := sha256.Sum256(genesisFileBytes)
	return hex.EncodeToString(sum[:])
}

// ---------------------------------------------------------------------------
// Signature domain separation (docs/PROTOCOL.md §Signing)
// ---------------------------------------------------------------------------

// SigningPurpose enumerates the independent signature domains in Hashgram.
// A signature produced for one purpose must never verify for another, and a
// signature produced on one network must never verify on another.
type SigningPurpose string

const (
	PurposeSocialEvent      SigningPurpose = "social-event"
	PurposeDeviceCert       SigningPurpose = "device-cert"
	PurposeServiceReceipt   SigningPurpose = "service-receipt"
	PurposeStorageChallenge SigningPurpose = "storage-challenge"
	PurposeEligibility      SigningPurpose = "eligibility-attestation"
	PurposeContentAttest    SigningPurpose = "content-attestation"
	PurposeBootstrapRecord  SigningPurpose = "bootstrap-record"
	PurposePeerHandshake    SigningPurpose = "peer-handshake"
	PurposeNodeAnnounce     SigningPurpose = "node-announce"
)

// SigningDomain returns the domain-separation prefix string for a purpose on
// this network, for example:
//
//	hashgram/v1/hashgram-mainnet/social-event
//
// It embeds the protocol major version and the network id, so a signed social
// event from a foreign fork cannot be replayed onto Mainnet, and a device
// certificate cannot be replayed as a service receipt.
func (n NetworkIdentity) SigningDomain(p SigningPurpose) string {
	return fmt.Sprintf("hashgram/v%d/%s/%s", n.ProtocolMajorVersion, n.NetworkID, p)
}

// SigningPreimage builds the exact byte string that must be signed for a
// purpose. The layout is:
//
//	magic(4) || len(domain) as uint64 BE || domain || len(payload) as uint64 BE || payload
//
// Length prefixes are mandatory: without them, (domain="ab", payload="c") and
// (domain="a", payload="bc") would hash identically and a signature for one
// would verify for the other.
// The length prefixes here are 64-bit rather than 32-bit, because this is the
// outer wrapper: the payload is a whole inner preimage of arbitrary length,
// not a bounded field. A non-negative int converts to uint64 exactly on every
// platform, so no length can overflow and there is nothing to check. See
// app/canonical for the distinction between the bounded and unbounded
// builders.
func (n NetworkIdentity) SigningPreimage(p SigningPurpose, payload []byte) []byte {
	domain := n.SigningDomain(p)

	return canonical.NewFixed(len(n.NetworkMagic) + 8 + len(domain) + 8 + len(payload)).
		Raw(n.NetworkMagic[:]).
		String(domain).
		Bytes(payload).
		Preimage()
}

// SigningDigest returns SHA-256 over SigningPreimage, which is what signature
// schemes actually sign.
func (n NetworkIdentity) SigningDigest(p SigningPurpose, payload []byte) [32]byte {
	return sha256.Sum256(n.SigningPreimage(p, payload))
}
