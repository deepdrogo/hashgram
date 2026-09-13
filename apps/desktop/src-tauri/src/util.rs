//! Small helpers shared by the command modules.

use std::path::Path;

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::{CmdResult, UiError};

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
#[must_use]
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = ((n >> (18 - 6 * i)) & 63) as usize;
                out.push(B64.get(idx).copied().unwrap_or(b'A') as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard or URL-safe base64 decode; whitespace and a `data:` prefix are
/// tolerated.
#[must_use]
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = match s.find(";base64,") {
        Some(i) => &s[i + 8..],
        None => s,
    };
    let mut out = Vec::with_capacity((s.len() * 3).div_euclid(4));
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let c = match c {
            b'-' => b'+',
            b'_' => b'/',
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => other,
        };
        let v = B64.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Opens a file with the default application.
pub fn open_path(app: &AppHandle, p: &Path) -> CmdResult<()> {
    app.opener()
        .open_path(p.display().to_string(), None::<&str>)
        .map_err(|e| UiError::new("open", format!("could not open: {e}"), false))
}

/// Middle-truncates a hash/address for display: `hash1abc…wxyz`.
#[must_use]
pub fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    if s.chars().count() <= head + tail + 1 {
        return s.to_owned();
    }
    let h: String = s.chars().take(head).collect();
    let t: String = s
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{h}…{t}")
}

/// Unix milliseconds.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..40usize {
            let v: Vec<u8> = (0..n as u8).collect();
            let e = base64_encode(&v);
            assert_eq!(base64_decode(&e).unwrap(), v, "len {n}");
        }
        assert_eq!(base64_decode("data:image/png;base64,AQID").unwrap(), vec![1, 2, 3]);
        assert_eq!(base64_decode("AQ ID\n").unwrap(), vec![1, 2, 3]);
        assert!(base64_decode("!!").is_none());
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate_middle("hash1abcdefghijklmnop", 8, 4), "hash1abc…mnop");
        assert_eq!(truncate_middle("short", 8, 4), "short");
    }
}
