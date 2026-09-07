//! Key material helpers: ed25519 signing and the libp2p public key envelope.
//!
//! Nothing here is novel cryptography. ed25519 comes from `ed25519-dalek`;
//! the libp2p key envelope is the protobuf `PublicKey { Type, Data }` that
//! libp2p-identity defines, reproduced here so this crate does not depend on
//! libp2p (mobile SDKs verify announcements without a swarm).

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::limits::{HASH_LEN, SIG_LEN};

/// Why a key or signature was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// Wrong length or not a valid curve point.
    #[error("invalid ed25519 public key ({0} bytes)")]
    InvalidPublicKey(usize),
    /// Wrong signature length.
    #[error("invalid signature length {0}, expected {SIG_LEN}")]
    InvalidSignatureLength(usize),
    /// The signature did not verify.
    #[error("signature did not verify")]
    BadSignature,
    /// A libp2p key envelope that is not ed25519.
    #[error("libp2p key type {0} is not ed25519; Hashgram node keys are ed25519")]
    UnsupportedLibp2pKeyType(i32),
    /// A libp2p key envelope that did not decode.
    #[error("libp2p key envelope did not decode")]
    MalformedLibp2pKey,
    /// The OS random source failed.
    #[error("the OS random source failed; refusing to generate a key")]
    NoRandomness,
}

/// An ed25519 signing key with the Hashgram signing convention: the
/// signature is over the 32-byte domain-separated digest, never over the
/// raw object.
#[derive(Clone)]
pub struct Ed25519Signer {
    key: SigningKey,
}

impl std::fmt::Debug for Ed25519Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material, not even in debug output.
        write!(f, "Ed25519Signer({})", hex::encode(self.public_key()))
    }
}

impl Ed25519Signer {
    /// Wraps a 32-byte secret.
    #[must_use]
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(&secret),
        }
    }

    /// Generates a fresh key from OS randomness.
    ///
    /// Fails only if the OS random source does, which on Linux means the
    /// system is in a state where no key should be generated.
    pub fn generate() -> Result<Self, KeyError> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|_| KeyError::NoRandomness)?;
        Ok(Self::from_secret(secret))
    }

    /// The 32-byte secret. Callers encrypt it at rest; this crate never
    /// writes it anywhere.
    #[must_use]
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    /// The raw 32-byte public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; HASH_LEN] {
        self.key.verifying_key().to_bytes()
    }

    /// Signs a digest.
    #[must_use]
    pub fn sign_digest(&self, digest: &[u8; 32]) -> [u8; SIG_LEN] {
        self.key.sign(digest).to_bytes()
    }
}

/// Fills a buffer from the OS random source.
pub fn random_bytes(buf: &mut [u8]) -> Result<(), KeyError> {
    getrandom::fill(buf).map_err(|_| KeyError::NoRandomness)
}

/// Verifies an ed25519 signature over a digest.
pub fn verify_ed25519(pubkey: &[u8], digest: &[u8; 32], signature: &[u8]) -> Result<(), KeyError> {
    let key: [u8; 32] = pubkey
        .try_into()
        .map_err(|_| KeyError::InvalidPublicKey(pubkey.len()))?;
    let key =
        VerifyingKey::from_bytes(&key).map_err(|_| KeyError::InvalidPublicKey(pubkey.len()))?;
    let sig: [u8; SIG_LEN] = signature
        .try_into()
        .map_err(|_| KeyError::InvalidSignatureLength(signature.len()))?;
    key.verify(digest, &Signature::from_bytes(&sig))
        .map_err(|_| KeyError::BadSignature)
}

/// Extracts the raw ed25519 key from a libp2p `PublicKey` protobuf envelope.
///
/// libp2p encodes `message PublicKey { KeyType Type = 1; bytes Data = 2; }`
/// with `Ed25519 = 1`. A node's announcement carries the envelope so the
/// peer id can be derived from it; verification needs the raw key.
pub fn ed25519_from_libp2p(envelope: &[u8]) -> Result<[u8; 32], KeyError> {
    use prost::Message;

    #[derive(Clone, PartialEq, prost::Message)]
    struct Libp2pPublicKey {
        #[prost(int32, tag = "1")]
        r#type: i32,
        #[prost(bytes = "vec", tag = "2")]
        data: Vec<u8>,
    }

    let pk = Libp2pPublicKey::decode(envelope).map_err(|_| KeyError::MalformedLibp2pKey)?;
    if pk.r#type != 1 {
        return Err(KeyError::UnsupportedLibp2pKeyType(pk.r#type));
    }
    pk.data
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::InvalidPublicKey(pk.data.len()))
}

/// Wraps a raw ed25519 key in the libp2p `PublicKey` envelope.
#[must_use]
pub fn ed25519_to_libp2p(raw: &[u8; 32]) -> Vec<u8> {
    // Hand-encoded: field 1 varint 1, field 2 bytes(32).
    let mut out = Vec::with_capacity(36);
    out.extend_from_slice(&[0x08, 0x01, 0x12, 0x20]);
    out.extend_from_slice(raw);
    out
}

/// BLAKE3 of a byte string, the hash used for every off-chain identifier.
#[must_use]
pub fn blake3_hash(data: &[u8]) -> [u8; HASH_LEN] {
    *blake3::hash(data).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let s = Ed25519Signer::generate().unwrap();
        let digest = blake3_hash(b"x");
        let sig = s.sign_digest(&digest);
        verify_ed25519(&s.public_key(), &digest, &sig).unwrap();
        assert!(verify_ed25519(&s.public_key(), &blake3_hash(b"y"), &sig).is_err());
    }

    #[test]
    fn libp2p_envelope_round_trips() {
        let s = Ed25519Signer::generate().unwrap();
        let env = ed25519_to_libp2p(&s.public_key());
        assert_eq!(ed25519_from_libp2p(&env).unwrap(), s.public_key());
    }

    #[test]
    fn a_non_ed25519_libp2p_key_is_refused() {
        let env = [0x08, 0x02, 0x12, 0x01, 0x00];
        assert!(matches!(
            ed25519_from_libp2p(&env),
            Err(KeyError::UnsupportedLibp2pKeyType(2))
        ));
    }

    #[test]
    fn debug_output_does_not_leak_the_secret() {
        let s = Ed25519Signer::generate().unwrap();
        let text = format!("{s:?}");
        assert!(!text.contains(&hex::encode(s.secret_bytes())));
    }
}
