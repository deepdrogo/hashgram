//! Identifiers, clocks and the small helpers every module shares.

use crate::AppError;

/// Length of every application-level random id (message, thread, share,
/// entry, event, space).
pub const ID_LEN: usize = 16;

/// A fresh random 16-byte id.
pub fn random_id() -> Result<Vec<u8>, AppError> {
    let mut id = vec![0u8; ID_LEN];
    getrandom::fill(&mut id).map_err(|_| AppError::NoRandomness)?;
    Ok(id)
}

/// Fresh random bytes of any length.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, AppError> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|_| AppError::NoRandomness)?;
    Ok(b)
}

/// Unix milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unix seconds.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Requires `id` to be exactly [`ID_LEN`] bytes.
pub fn require_id(what: &str, id: &[u8]) -> Result<(), AppError> {
    if id.len() != ID_LEN {
        return Err(AppError::Invalid(format!(
            "{what} must be {ID_LEN} bytes, got {}",
            id.len()
        )));
    }
    Ok(())
}

/// Requires `id` to be empty or exactly [`ID_LEN`] bytes.
pub fn require_id_or_empty(what: &str, id: &[u8]) -> Result<(), AppError> {
    if id.is_empty() {
        return Ok(());
    }
    require_id(what, id)
}

/// Requires a 32-byte hash.
pub fn require_hash(what: &str, h: &[u8]) -> Result<(), AppError> {
    if h.len() != 32 {
        return Err(AppError::Invalid(format!(
            "{what} must be 32 bytes, got {}",
            h.len()
        )));
    }
    Ok(())
}

/// Requires a non-empty string no longer than `max` bytes.
pub fn require_str(what: &str, s: &str, max: usize) -> Result<(), AppError> {
    if s.is_empty() {
        return Err(AppError::Invalid(format!("{what} is empty")));
    }
    require_str_max(what, s, max)
}

/// Requires a string no longer than `max` bytes (may be empty).
pub fn require_str_max(what: &str, s: &str, max: usize) -> Result<(), AppError> {
    if s.len() > max {
        return Err(AppError::Invalid(format!(
            "{what} is {} bytes, maximum {max}",
            s.len()
        )));
    }
    if s.chars().any(|c| c == '\0') {
        return Err(AppError::Invalid(format!("{what} contains NUL")));
    }
    Ok(())
}

/// Whether `s` looks like a Hashgram account address (`hash1` + bech32).
#[must_use]
pub fn is_address(s: &str) -> bool {
    s.len() >= 39
        && s.len() <= 90
        && s.starts_with("hash1")
        && s.bytes()
            .skip(5)
            .all(|b| b"qpzry9x8gf2tvdw0s3jn54khce6mua7l".contains(&b))
}

/// Requires a well-formed address.
pub fn require_address(what: &str, s: &str) -> Result<(), AppError> {
    if !is_address(s) {
        return Err(AppError::Invalid(format!("{what} is not a hash1 address")));
    }
    Ok(())
}

/// Hex helper used by views.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    hex::encode(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_random_and_sized() {
        let a = random_id().unwrap();
        let b = random_id().unwrap();
        assert_eq!(a.len(), ID_LEN);
        assert_ne!(a, b);
        assert!(require_id("x", &a).is_ok());
        assert!(require_id("x", &a[..15]).is_err());
    }

    #[test]
    fn address_shape() {
        assert!(is_address("hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5"));
        assert!(!is_address("cosmos1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5"));
        assert!(!is_address("hash1"));
        assert!(!is_address("hash1E0TL2HFF03HU4G3SAWCJQA2P9TC4UH24E4VFL5"));
    }
}
