//! Application-level signing domains.
//!
//! The protocol layer (`hashgram-net`) has a closed set of 13 signing
//! purposes shared byte-for-byte with the Go chain; adding to it is a
//! cross-language protocol change. Application objects that need
//! signatures (Space events) therefore use their own domain family with the
//! same framing rules, so a signature over a Space event can never be
//! mistaken for any protocol object and vice versa:
//!
//! ```text
//! magic(4) || u64be(len(domain)) || domain || u64be(len(payload)) || payload
//! domain = "hashgram-app/v1/<network_id>/<purpose>"
//! digest = SHA-256(preimage)
//! ```
//!
//! The `hashgram-app/` prefix differs from the protocol's `hashgram/` in
//! its first bytes, so the two families are disjoint even if a purpose name
//! were reused.

use hashgram_net::NetworkIdentity;
use hashgram_proto::keys::{verify_ed25519, Ed25519Signer, KeyError};
use sha2::{Digest, Sha256};

/// Application signing purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppPurpose {
    /// A Space governance/content event.
    SpaceEvent,
    /// A Drive capability grant (reserved; capabilities are currently
    /// authenticated by the MLS channel they travel in).
    DriveCapability,
    /// A paid storage lease between a client and a provider
    /// (`docs/ADR_HASH_STORAGE_MARKET.md` §5).
    StorageLease,
}

impl AppPurpose {
    /// Wire string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpaceEvent => "space-event",
            Self::DriveCapability => "drive-capability",
            Self::StorageLease => "storage-lease",
        }
    }
}

/// Application domain generation.
pub const APP_DOMAIN_VERSION: u32 = 1;

/// The domain string.
#[must_use]
pub fn domain(network: &NetworkIdentity, purpose: AppPurpose) -> String {
    format!(
        "hashgram-app/v{APP_DOMAIN_VERSION}/{}/{}",
        network.network_id,
        purpose.as_str()
    )
}

/// The exact bytes hashed.
#[must_use]
pub fn preimage(network: &NetworkIdentity, purpose: AppPurpose, payload: &[u8]) -> Vec<u8> {
    let d = domain(network, purpose);
    let d = d.as_bytes();
    let mut out = Vec::with_capacity(4 + 8 + d.len() + 8 + payload.len());
    out.extend_from_slice(&network.network_magic);
    out.extend_from_slice(&(d.len() as u64).to_be_bytes());
    out.extend_from_slice(d);
    out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// SHA-256 of the preimage; what ed25519 signs.
#[must_use]
pub fn digest(network: &NetworkIdentity, purpose: AppPurpose, payload: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(preimage(network, purpose, payload));
    h.finalize().into()
}

/// Signs a canonical payload.
#[must_use]
pub fn sign(
    network: &NetworkIdentity,
    purpose: AppPurpose,
    signer: &Ed25519Signer,
    payload: &[u8],
) -> [u8; 64] {
    signer.sign_digest(&digest(network, purpose, payload))
}

/// Verifies a signature over a canonical payload.
pub fn verify(
    network: &NetworkIdentity,
    purpose: AppPurpose,
    pubkey: &[u8],
    payload: &[u8],
    signature: &[u8],
) -> Result<(), KeyError> {
    verify_ed25519(pubkey, &digest(network, purpose, payload), signature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashgram_net::SigningPurpose;

    fn net() -> NetworkIdentity {
        NetworkIdentity::devnet("0".repeat(64))
    }

    #[test]
    fn app_and_protocol_domains_are_disjoint() {
        let n = net();
        let payload = b"same bytes";
        let app = digest(&n, AppPurpose::SpaceEvent, payload);
        for p in hashgram_net::ALL_PURPOSES {
            assert_ne!(app, n.signing_digest(p, payload), "collides with {p:?}");
        }
        let _ = SigningPurpose::SocialEvent;
    }

    #[test]
    fn networks_differ() {
        let a = digest(
            &NetworkIdentity::devnet("0".repeat(64)),
            AppPurpose::SpaceEvent,
            b"x",
        );
        let b = digest(
            &NetworkIdentity::mainnet("0".repeat(64)),
            AppPurpose::SpaceEvent,
            b"x",
        );
        assert_ne!(a, b);
    }

    #[test]
    fn sign_verify_round_trip() {
        let n = net();
        let s = Ed25519Signer::from_secret([9; 32]);
        let sig = sign(&n, AppPurpose::SpaceEvent, &s, b"payload");
        assert!(verify(
            &n,
            AppPurpose::SpaceEvent,
            &s.public_key(),
            b"payload",
            &sig
        )
        .is_ok());
        assert!(verify(
            &n,
            AppPurpose::SpaceEvent,
            &s.public_key(),
            b"payloae",
            &sig
        )
        .is_err());
        assert!(verify(
            &n,
            AppPurpose::DriveCapability,
            &s.public_key(),
            b"payload",
            &sig
        )
        .is_err());
    }
}
