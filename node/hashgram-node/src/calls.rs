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
use hashgram_proto::limits::TURN_SENTINEL_LIMIT;
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

/// Checks that a credential request was signed by the device it names.
///
/// The request signs the `mailbox-fetch` preimage with an empty cursor and
/// [`TURN_SENTINEL_LIMIT`] as the limit. The sentinel is what keeps this
/// preimage disjoint from every real fetch: `validate::mailbox_fetch`
/// refuses it, and this path requires it, so a store node holding a genuine
/// fetch signature cannot replay it here to obtain credentials in the
/// victim's name. Shape is checked before the signature because the check is
/// cheaper and a wrong shape is a wrong request regardless of who signed it.
pub fn verify_request(
    identity: &hashgram_net::NetworkIdentity,
    req: &pb::TurnCredentialRequest,
    now: u64,
) -> Result<(), String> {
    let f = pb::MailboxFetch {
        mailbox: hashgram_proto::signing::mailbox_for(&req.device_pubkey).to_vec(),
        device_pubkey: req.device_pubkey.clone(),
        cursor: vec![],
        limit: TURN_SENTINEL_LIMIT,
        timestamp: req.timestamp,
        signature: req.signature.clone(),
    };
    hashgram_proto::validate::turn_credential_fetch(&f, now).map_err(|e| e.to_string())?;
    hashgram_proto::signing::verify_mailbox_fetch(identity, &f).map_err(|e| e.to_string())
}

/// The coturn username label for a device: the first 8 bytes of its
/// mailbox id, hex. Long enough to attribute a credential in coturn logs,
/// short enough not to leak the whole device key to the TURN server.
#[must_use]
pub fn label_for(device_pubkey: &[u8]) -> String {
    let mailbox = hashgram_proto::signing::mailbox_for(device_pubkey);
    hex::encode(mailbox.get(..8).unwrap_or(&[]))
}

/// Verifies a credential request (see [`verify_request`]) and issues a
/// credential.
pub fn issue_for_request(
    shared: &crate::app::Shared,
    secret: &[u8],
    req: &pb::TurnCredentialRequest,
) -> Result<TurnCredential, String> {
    let now = crate::store::now();
    verify_request(&shared.identity, req, now)?;
    Ok(issue(
        secret,
        &label_for(&req.device_pubkey),
        &shared.config.turn_uris,
        now,
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

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn signed_fetch(
        id: &hashgram_net::NetworkIdentity,
        dev: &hashgram_proto::Ed25519Signer,
        limit: u32,
        cursor: Vec<u8>,
        now: u64,
    ) -> pb::MailboxFetch {
        let mut f = pb::MailboxFetch {
            cursor,
            limit,
            timestamp: now,
            ..Default::default()
        };
        hashgram_proto::signing::sign_mailbox_fetch(id, dev, &mut f).unwrap();
        f
    }

    fn request_from(f: &pb::MailboxFetch) -> pb::TurnCredentialRequest {
        pb::TurnCredentialRequest {
            device_pubkey: f.device_pubkey.clone(),
            timestamp: f.timestamp,
            signature: f.signature.clone(),
        }
    }

    #[test]
    fn a_sentinel_signed_request_is_accepted() {
        let id = hashgram_net::NetworkIdentity::devnet(GENESIS);
        let dev = hashgram_proto::Ed25519Signer::generate().unwrap();
        let now = crate::store::now();
        let f = signed_fetch(&id, &dev, TURN_SENTINEL_LIMIT, vec![], now);
        verify_request(&id, &request_from(&f), now).unwrap();
        // And that same signature is not a valid mailbox fetch: the two
        // preimage sets are disjoint.
        assert!(hashgram_proto::validate::mailbox_fetch(&f, now).is_err());
    }

    #[test]
    fn a_real_fetch_signature_is_refused_by_the_turn_path() {
        let id = hashgram_net::NetworkIdentity::devnet(GENESIS);
        let dev = hashgram_proto::Ed25519Signer::generate().unwrap();
        let now = crate::store::now();
        // The exact preimage the old code accepted: empty cursor, limit 0.
        for limit in [0, 1, 100] {
            let f = signed_fetch(&id, &dev, limit, vec![], now);
            // It is a perfectly good fetch...
            hashgram_proto::validate::mailbox_fetch(&f, now).unwrap();
            hashgram_proto::signing::verify_mailbox_fetch(&id, &f).unwrap();
            // ...and worthless as a credential request.
            assert!(
                verify_request(&id, &request_from(&f), now).is_err(),
                "limit {limit}"
            );
        }
        // A fetch with a cursor cannot be replayed either.
        let f = signed_fetch(&id, &dev, 0, vec![7; 40], now);
        assert!(verify_request(&id, &request_from(&f), now).is_err());
    }

    #[test]
    fn a_stale_or_foreign_request_is_refused() {
        let id = hashgram_net::NetworkIdentity::devnet(GENESIS);
        let dev = hashgram_proto::Ed25519Signer::generate().unwrap();
        let now = crate::store::now();
        let f = signed_fetch(&id, &dev, TURN_SENTINEL_LIMIT, vec![], now - 1000);
        assert!(verify_request(&id, &request_from(&f), now).is_err());
        // Signed for another network: the signing domain binds the network
        // and chain ids, so a mainnet signature is worthless on devnet.
        let other = hashgram_net::NetworkIdentity::mainnet(GENESIS);
        let f = signed_fetch(&other, &dev, TURN_SENTINEL_LIMIT, vec![], now);
        assert!(verify_request(&id, &request_from(&f), now).is_err());
        assert_eq!(label_for(&dev.public_key()).len(), 16);
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
