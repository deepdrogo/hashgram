//! Symmetric sealing for database columns.
//!
//! The database key is a 32-byte secret generated at account creation and
//! stored only inside the vault (`extra["desktop_db_key"]`), so everything
//! sealed here is encrypted at rest under the vault. The cipher is
//! XChaCha20-Poly1305 with a random 24-byte nonce prefixed to the ciphertext. Column-level
//! sealing keeps the SQLite build plain (no SQLCipher toolchain on Windows)
//! while nothing sensitive ever hits disk in the clear.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

/// The database key.
pub type DbKey = Zeroizing<[u8; 32]>;

/// Generates a fresh key.
pub fn generate_key() -> Result<DbKey, String> {
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| e.to_string())?;
    Ok(Zeroizing::new(k))
}

/// Seals `plaintext` under `key`, binding `aad` (e.g. the row's id).
pub fn seal(key: &DbKey, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = XChaCha20Poly1305::new(key.as_slice().into());
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| "seal failed".to_owned())?;
    let mut out = Vec::with_capacity(24 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Opens a sealed blob.
pub fn open(key: &DbKey, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, String> {
    if sealed.len() < 24 + 16 {
        return Err("sealed blob too short".into());
    }
    let (nonce, ct) = sealed.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.as_slice().into());
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
        .map_err(|_| "open failed (wrong key or tampered)".to_owned())
}

/// Hex helpers for storing the key in the vault's `extra` map.
pub fn key_to_hex(k: &DbKey) -> String {
    hex::encode(k.as_slice())
}

/// Parses a hex key.
pub fn key_from_hex(s: &str) -> Result<DbKey, String> {
    let v = hex::decode(s.trim()).map_err(|e| e.to_string())?;
    let arr: [u8; 32] = v
        .try_into()
        .map_err(|_| "database key is not 32 bytes".to_owned())?;
    Ok(Zeroizing::new(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_with_aad() {
        let k = generate_key().unwrap();
        let s = seal(&k, b"row-1", b"hello").unwrap();
        assert_eq!(open(&k, b"row-1", &s).unwrap(), b"hello");
        assert!(open(&k, b"row-2", &s).is_err(), "aad is bound");
        let other = generate_key().unwrap();
        assert!(open(&other, b"row-1", &s).is_err());
        assert!(
            !s.windows(5).any(|w| w == b"hello"),
            "no plaintext in the blob"
        );
    }

    #[test]
    fn hex_round_trip() {
        let k = generate_key().unwrap();
        let h = key_to_hex(&k);
        assert_eq!(key_from_hex(&h).unwrap().as_slice(), k.as_slice());
        assert!(key_from_hex("abcd").is_err());
    }
}
