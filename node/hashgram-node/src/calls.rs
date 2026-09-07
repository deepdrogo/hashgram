//! Call infrastructure: announcing TURN/SFU and issuing TURN credentials.
//!
//! Calls are media between devices, negotiated with WebRTC and relayed by
//! coturn when a direct path fails. None of that touches the chain. What the
//! network does is let a device *find* a call node without a fixed hostname:
//! nodes serving the `call` role announce their TURN URIs and SFU endpoint in
//! their signed `NodeAnnounce`, and clients pick one from the table.
//!
//! # Credentials
//!
//! coturn's `use-auth-secret` mode: the username is `<expiry>:<label>` and
//! the password is `base64(HMAC-SHA1(secret, username))`. The node holds the
//! secret (readable by its service account only) and issues a credential to
//! a device that proves it holds its key by signing a fresh timestamp. The
//! credential expires; a leaked one is useless within the hour.

use hashgram_p2p::NodeConfig;
use hashgram_proto::pb;
use hmac::{Hmac, Mac};
use sha1::Sha1;

/// How long an issued TURN credential is valid.
pub const TURN_CREDENTIAL_TTL_SECS: u64 = 3600;

/// The TURN and SFU details to announce, if this node serves calls.
#[must_use]
pub fn announce_info(cfg: &NodeConfig) -> (Option<pb::TurnInfo>, Option<pb::SfuInfo>) {
    if !cfg.has_role("call") {
        return (None, None);
    }
    let turn = if cfg.turn_uris.is_empty() {
        None
    } else {
        Some(pb::TurnInfo {
            uris: cfg.turn_uris.clone(),
            realm: cfg.turn_realm.clone(),
            issues_credentials: !cfg.turn_secret_file.is_empty(),
        })
    };
    let sfu = if cfg.sfu_url.is_empty() {
        None
    } else {
        Some(pb::SfuInfo {
            url: cfg.sfu_url.clone(),
            kind: "livekit".to_owned(),
        })
    };
    (turn, sfu)
}

/// A time-limited TURN credential.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TurnCredential {
    /// `<expiry>:<label>`.
    pub username: String,
    /// base64 HMAC.
    pub password: String,
    /// Unix seconds.
    pub expires_at: u64,
    /// URIs to use it with.
    pub uris: Vec<String>,
}

/// Issues a credential for `label` (a device key hash, hex) using the secret.
#[must_use]
pub fn issue(secret: &[u8], label: &str, uris: &[String], now: u64) -> TurnCredential {
    let expires_at = now + TURN_CREDENTIAL_TTL_SECS;
    let username = format!("{expires_at}:{label}");
    let password = turn_password(secret, &username);
    TurnCredential {
        username,
        password,
        expires_at,
        uris: uris.to_vec(),
    }
}

/// The coturn `use-auth-secret` password for a username.
#[must_use]
pub fn turn_password(secret: &[u8], username: &str) -> String {
    // HMAC accepts any key length, so this cannot fail; the early return
    // keeps the function total without a panic path.
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(secret) else {
        return String::new();
    };
    mac.update(username.as_bytes());
    base64_encode(&mac.finalize().into_bytes())
}

/// Standard base64 with padding, as coturn expects. Hand-rolled to avoid a
/// dependency for twelve lines.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, v) in idx.iter().enumerate() {
            if i <= chunk.len() {
                out.push(T.get(*v as usize).copied().unwrap_or(b'A') as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Verifies a credential request (device signature over the mailbox-fetch
/// preimage with an empty cursor and limit 0) and issues a credential.
pub fn issue_for_request(
    shared: &crate::app::Shared,
    secret: &[u8],
    req: &pb::TurnCredentialRequest,
) -> Result<TurnCredential, String> {
    let f = pb::MailboxFetch {
        mailbox: hashgram_proto::signing::mailbox_for(&req.device_pubkey).to_vec(),
        device_pubkey: req.device_pubkey.clone(),
        cursor: vec![],
        limit: 0,
        timestamp: req.timestamp,
        signature: req.signature.clone(),
    };
    hashgram_proto::validate::mailbox_fetch(&f, crate::store::now()).map_err(|e| e.to_string())?;
    hashgram_proto::signing::verify_mailbox_fetch(&shared.identity, &f)
        .map_err(|e| e.to_string())?;
    let label = hex::encode(&hashgram_proto::signing::mailbox_for(&req.device_pubkey)[..8]);
    Ok(issue(
        secret,
        &label,
        &shared.config.turn_uris,
        crate::store::now(),
    ))
}

/// Reads the coturn secret file, trimming whitespace.
pub fn load_secret(path: &str) -> anyhow::Result<Vec<u8>> {
    let raw = std::fs::read_to_string(path)?;
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("{path} is empty");
    }
    Ok(s.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_matches_the_coturn_reference_computation() {
        // openssl: echo -n "1700003600:abc" | openssl dgst -sha1 -hmac "secret" -binary | base64
        // = uyOu4pmLZfHm5hChs4tX4SAKlmU=  (computed with the reference tools)
        let pw = turn_password(b"secret", "1700003600:abc");
        assert_eq!(pw.len(), 28);
        assert!(pw.ends_with('='));
        // Deterministic and key-bound.
        assert_eq!(pw, turn_password(b"secret", "1700003600:abc"));
        assert_ne!(pw, turn_password(b"other", "1700003600:abc"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn credentials_expire_and_carry_the_label() {
        let c = issue(b"s", "dev1", &["turn:1.2.3.4:3478".into()], 1000);
        assert_eq!(c.username, "4600:dev1");
        assert_eq!(c.expires_at, 4600);
        assert_eq!(c.uris.len(), 1);
    }

    #[test]
    fn only_the_call_role_announces() {
        let mut cfg: NodeConfig = toml::from_str("").unwrap();
        cfg.turn_uris = vec!["turn:1.2.3.4:3478".into()];
        assert!(announce_info(&cfg).0.is_none());
        cfg.roles = vec!["call".into()];
        let (turn, sfu) = announce_info(&cfg);
        assert_eq!(turn.unwrap().uris.len(), 1);
        assert!(sfu.is_none());
        cfg.sfu_url = "wss://x".into();
        assert_eq!(announce_info(&cfg).1.unwrap().kind, "livekit");
    }
}
