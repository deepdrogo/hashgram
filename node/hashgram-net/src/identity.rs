//! Network identity and canonical signing preimages.
//!
//! The framing here must match `app/params/network.go` byte for byte. The
//! tests in `tests/vectors.rs` check it against vectors generated from the Go
//! implementation, so a divergence fails the build rather than surfacing as a
//! signature that verifies on one side and not the other.

use sha2::{Digest, Sha256};

use crate::purpose::SigningPurpose;

/// Length of the network magic, in bytes.
pub const MAGIC_LEN: usize = 4;

/// Length of a hex-encoded SHA-256 genesis hash, in characters.
pub const GENESIS_HASH_LEN: usize = 64;

/// The wire-compatibility generation.
///
/// Two nodes with different values here must not exchange application data:
/// the same bytes may mean different things. It appears inside every signing
/// domain, so bumping it invalidates every signature, which is the intended
/// cost of a breaking wire change.
pub const PROTOCOL_MAJOR_VERSION: u32 = 1;

/// Mainnet network magic.
pub const NETWORK_MAGIC_MAINNET: [u8; MAGIC_LEN] = *b"HGM1";

/// Devnet network magic.
pub const NETWORK_MAGIC_DEVNET: [u8; MAGIC_LEN] = *b"HGD1";

/// The five-part identity of a Hashgram network.
///
/// Four parts are fixed at compile time. The genesis hash is computed at
/// genesis and then pinned, and it is the only one that is the network's
/// actual fingerprint: the others are labels, and a fork can copy a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkIdentity {
    /// Human-readable name, for operator output only. Never hashed.
    pub network_name: String,
    /// Application-level network identity. Appears in signing domains.
    pub network_id: String,
    /// CometBFT consensus identity. Compared at the CometBFT handshake.
    pub chain_id: String,
    /// Four bytes prefixed to every signing preimage, so a foreign network's
    /// signature fails on the first four bytes rather than after a full
    /// verification.
    pub network_magic: [u8; MAGIC_LEN],
    /// Wire-compatibility generation.
    pub protocol_major_version: u32,
    /// Lowercase hex SHA-256 of the genesis file.
    pub genesis_hash: String,
}

impl NetworkIdentity {
    /// The Hashgram Mainnet identity.
    #[must_use]
    pub fn mainnet(genesis_hash: impl AsRef<str>) -> Self {
        Self {
            network_name: "Hashgram Mainnet".to_owned(),
            network_id: "hashgram-mainnet".to_owned(),
            chain_id: "hashgram-1".to_owned(),
            network_magic: NETWORK_MAGIC_MAINNET,
            protocol_major_version: PROTOCOL_MAJOR_VERSION,
            genesis_hash: genesis_hash.as_ref().to_ascii_lowercase(),
        }
    }

    /// The devnet identity.
    ///
    /// The name carries "DEVNET ONLY" so that operator output cannot be
    /// mistaken for Mainnet at a glance. `hashgramctl mainnet-preflight`
    /// refuses to pass on a host holding devnet material.
    #[must_use]
    pub fn devnet(genesis_hash: impl AsRef<str>) -> Self {
        Self {
            network_name: "Hashgram Devnet (DEVNET ONLY)".to_owned(),
            network_id: "hashgram-devnet".to_owned(),
            chain_id: "hashgram-devnet-1".to_owned(),
            network_magic: NETWORK_MAGIC_DEVNET,
            protocol_major_version: PROTOCOL_MAJOR_VERSION,
            genesis_hash: genesis_hash.as_ref().to_ascii_lowercase(),
        }
    }

    /// Whether this is Mainnet.
    #[must_use]
    pub fn is_mainnet(&self) -> bool {
        self.network_magic == NETWORK_MAGIC_MAINNET && self.network_id == "hashgram-mainnet"
    }

    /// The domain-separation string for a purpose, for example
    /// `hashgram/v1/hashgram-mainnet/social-event`.
    ///
    /// It embeds the protocol version, the network id and the purpose, so a
    /// signed social event from a foreign fork cannot be replayed onto
    /// Mainnet and a device certificate cannot be replayed as a receipt.
    #[must_use]
    pub fn signing_domain(&self, purpose: SigningPurpose) -> String {
        format!(
            "hashgram/v{}/{}/{}",
            self.protocol_major_version,
            self.network_id,
            purpose.as_str()
        )
    }

    /// The exact bytes to sign for a purpose.
    ///
    /// Layout, all integers big-endian:
    ///
    /// ```text
    /// magic(4) || len(domain) as u64 || domain || len(payload) as u64 || payload
    /// ```
    ///
    /// The length prefixes are mandatory. Without them,
    /// `(domain="ab", payload="c")` and `(domain="a", payload="bc")` hash
    /// identically and a signature for one verifies for the other.
    ///
    /// They are 64-bit rather than 32-bit because this is the outer wrapper:
    /// the payload is a whole inner preimage of arbitrary length, not a
    /// bounded field. A `usize` length is non-negative and converts to `u64`
    /// exactly on every supported platform, so no length can overflow and
    /// there is nothing to check. The Go side uses `app/canonical`'s
    /// infallible builder for the same reason.
    #[must_use]
    pub fn signing_preimage(&self, purpose: SigningPurpose, payload: &[u8]) -> Vec<u8> {
        let domain = self.signing_domain(purpose);
        let domain = domain.as_bytes();

        let mut out = Vec::with_capacity(MAGIC_LEN + 8 + domain.len() + 8 + payload.len());
        out.extend_from_slice(&self.network_magic);
        out.extend_from_slice(&(domain.len() as u64).to_be_bytes());
        out.extend_from_slice(domain);
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    /// SHA-256 over [`Self::signing_preimage`], which is what signature
    /// schemes actually sign.
    #[must_use]
    pub fn signing_digest(&self, purpose: SigningPurpose, payload: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_preimage(purpose, payload));
        hasher.finalize().into()
    }

    /// Whether a genesis hash is well-formed: 64 lowercase hex characters.
    ///
    /// Checked before comparison so that a malformed value is a clear error
    /// rather than a mismatch that looks like a fork.
    #[must_use]
    pub fn is_well_formed_genesis_hash(hash: &str) -> bool {
        hash.len() == GENESIS_HASH_LEN
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

/// Computes the genesis hash of raw genesis file bytes.
///
/// # A trap worth knowing
///
/// This hashes the **file**. CometBFT's RPC serves a re-serialised genesis:
/// it drops the SDK's `app_name` and `app_version` fields, renders
/// `initial_height` as a string, and emits compact rather than indented JSON.
/// So the hash of an RPC response never equals the hash of the file, and a
/// diagnostic comparing them warns on every healthy node. `hashgramctl
/// network-info` made exactly that mistake and now compares chain ids
/// instead.
#[must_use]
pub fn compute_genesis_hash(raw: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    #[test]
    fn length_prefixes_prevent_domain_payload_confusion() {
        // The property the prefixes exist for. Two identities whose domain and
        // payload concatenate to the same bytes must still produce different
        // digests.
        let id = NetworkIdentity::mainnet(GENESIS);

        let a = id.signing_digest(SigningPurpose::SocialEvent, b"ab");
        let b = id.signing_digest(SigningPurpose::SocialEvent, b"abc");
        assert_ne!(a, b, "payload length is not committed to");
    }

    #[test]
    fn purposes_produce_distinct_digests() {
        let id = NetworkIdentity::mainnet(GENESIS);
        let payload = b"same payload everywhere";

        let mut seen = std::collections::HashSet::new();
        for purpose in crate::ALL_PURPOSES {
            let digest = id.signing_digest(purpose, payload);
            assert!(
                seen.insert(digest),
                "{purpose} produced a digest another purpose already produced"
            );
        }
    }

    #[test]
    fn networks_produce_distinct_digests() {
        // The property that stops a devnet signature being replayed on
        // mainnet. Same purpose, same payload, same genesis hash.
        let mainnet = NetworkIdentity::mainnet(GENESIS);
        let devnet = NetworkIdentity::devnet(GENESIS);

        assert_ne!(
            mainnet.signing_digest(SigningPurpose::EligibilityAttestation, b"x"),
            devnet.signing_digest(SigningPurpose::EligibilityAttestation, b"x"),
            "a devnet signature would verify on mainnet"
        );
    }

    #[test]
    fn preimage_begins_with_the_magic() {
        let id = NetworkIdentity::mainnet(GENESIS);
        let pre = id.signing_preimage(SigningPurpose::PeerHandshake, b"x");
        assert!(pre.starts_with(&NETWORK_MAGIC_MAINNET));
    }

    #[test]
    fn preimage_layout_is_exact() {
        let id = NetworkIdentity::mainnet(GENESIS);
        let payload = b"payload";
        let pre = id.signing_preimage(SigningPurpose::SocialEvent, payload);
        let domain = id.signing_domain(SigningPurpose::SocialEvent);

        assert_eq!(pre.len(), MAGIC_LEN + 8 + domain.len() + 8 + payload.len());

        // Domain length, big-endian, immediately after the magic.
        let domain_len = u64::from_be_bytes(
            pre[MAGIC_LEN..MAGIC_LEN + 8]
                .try_into()
                .expect("slice is exactly 8 bytes"),
        );
        assert_eq!(domain_len as usize, domain.len());

        let start = MAGIC_LEN + 8;
        assert_eq!(&pre[start..start + domain.len()], domain.as_bytes());

        // And the payload length is committed to as well.
        let at = start + domain.len();
        let payload_len = u64::from_be_bytes(
            pre[at..at + 8]
                .try_into()
                .expect("slice is exactly 8 bytes"),
        );
        assert_eq!(payload_len as usize, payload.len());
    }

    #[test]
    fn an_empty_payload_is_distinct_from_a_one_byte_one() {
        let id = NetworkIdentity::mainnet(GENESIS);
        assert_ne!(
            id.signing_digest(SigningPurpose::SocialEvent, b""),
            id.signing_digest(SigningPurpose::SocialEvent, b"\0"),
        );
    }

    #[test]
    fn genesis_hash_is_lowercased_on_construction() {
        let upper = GENESIS.to_ascii_uppercase();
        let id = NetworkIdentity::mainnet(&upper);
        assert_eq!(id.genesis_hash, GENESIS);
    }

    #[test]
    fn malformed_genesis_hashes_are_recognised() {
        assert!(NetworkIdentity::is_well_formed_genesis_hash(GENESIS));
        assert!(!NetworkIdentity::is_well_formed_genesis_hash(""));
        assert!(!NetworkIdentity::is_well_formed_genesis_hash("abc"));
        assert!(!NetworkIdentity::is_well_formed_genesis_hash(
            &GENESIS.to_ascii_uppercase()
        ));
        assert!(!NetworkIdentity::is_well_formed_genesis_hash(&format!(
            "{GENESIS}0"
        )));
        // 64 characters but not hex.
        assert!(!NetworkIdentity::is_well_formed_genesis_hash(
            &"z".repeat(64)
        ));
    }

    #[test]
    fn mainnet_and_devnet_are_distinguishable() {
        assert!(NetworkIdentity::mainnet(GENESIS).is_mainnet());
        assert!(!NetworkIdentity::devnet(GENESIS).is_mainnet());
        assert!(NetworkIdentity::devnet(GENESIS)
            .network_name
            .contains("DEVNET ONLY"));
    }

    #[test]
    fn genesis_hash_of_known_bytes() {
        // SHA-256 of the empty string, so this fails if the hash function or
        // the encoding ever changes.
        assert_eq!(
            compute_genesis_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
