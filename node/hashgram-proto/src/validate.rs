//! Structural validation of wire objects.
//!
//! Signature verification says "the author said this". Validation says "this
//! is a thing the protocol admits": bounded, well-typed, and not from the
//! future. Both run before an object is stored or forwarded, validation
//! first because it is cheaper and a failure is a scoring event either way.

use prost::Message;

use crate::limits::*;
use crate::pb;

/// The social event families, as wire strings.
pub const EVENT_TYPES: [&str; 12] = [
    "PROFILE_UPDATE",
    "FOLLOW",
    "UNFOLLOW",
    "POST_CREATE",
    "POST_EDIT",
    "POST_DELETE",
    "COMMENT_CREATE",
    "REACTION",
    "REPOST",
    "CHANNEL_CREATE",
    "REEL_CREATE",
    "STORY_CREATE",
];

/// Why an object was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// A field exceeds its bound.
    #[error("{field} is {actual}, above the {max} limit")]
    TooLarge {
        /// Field name.
        field: &'static str,
        /// Observed size.
        actual: usize,
        /// Bound.
        max: usize,
    },
    /// A fixed-length field has the wrong length.
    #[error("{field} is {actual} bytes, expected {expected}")]
    WrongLength {
        /// Field name.
        field: &'static str,
        /// Observed.
        actual: usize,
        /// Expected.
        expected: usize,
    },
    /// A required field is empty.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A timestamp too far in the future.
    #[error("timestamp {ts} is more than {MAX_FUTURE_SKEW_SECS}s ahead of now ({now})")]
    FutureTimestamp {
        /// Claimed.
        ts: u64,
        /// Local clock.
        now: u64,
    },
    /// A signed request too old to accept.
    #[error("request timestamp {ts} is older than {MAX_REQUEST_AGE_SECS}s (now {now}); replay or clock skew")]
    Stale {
        /// Claimed.
        ts: u64,
        /// Local clock.
        now: u64,
    },
    /// Unknown event family.
    #[error("unknown event type {0:?}")]
    UnknownEventType(String),
    /// Wrong version.
    #[error("unsupported {0} version {1}")]
    Version(&'static str, u32),
    /// Payload did not decode as the type its event claims.
    #[error("payload does not decode as {0}: {1}")]
    Payload(&'static str, String),
    /// A semantic rule of one payload type.
    #[error("{0}")]
    Rule(&'static str),
    /// Not a Hashgram address.
    #[error("{0} is not a hash1 address")]
    Address(&'static str),
    /// Expiry not after creation, or beyond the bound.
    #[error("expiry {expires_at} is invalid against created {created_at} (max {max_secs}s)")]
    Expiry {
        /// Created.
        created_at: u64,
        /// Expires.
        expires_at: u64,
        /// Maximum lifetime.
        max_secs: u64,
    },
}

fn bound(field: &'static str, actual: usize, max: usize) -> Result<(), ValidationError> {
    if actual > max {
        return Err(ValidationError::TooLarge { field, actual, max });
    }
    Ok(())
}

fn exact(field: &'static str, actual: usize, expected: usize) -> Result<(), ValidationError> {
    if actual != expected {
        return Err(ValidationError::WrongLength {
            field,
            actual,
            expected,
        });
    }
    Ok(())
}

fn not_future(ts: u64, now: u64) -> Result<(), ValidationError> {
    if ts > now.saturating_add(MAX_FUTURE_SKEW_SECS) {
        return Err(ValidationError::FutureTimestamp { ts, now });
    }
    Ok(())
}

/// Checks a request timestamp is recent: neither ahead of the clock nor
/// older than the replay window.
pub fn fresh(ts: u64, now: u64) -> Result<(), ValidationError> {
    not_future(ts, now)?;
    if ts.saturating_add(MAX_REQUEST_AGE_SECS) < now {
        return Err(ValidationError::Stale { ts, now });
    }
    Ok(())
}

/// A loose shape check for a bech32 Hashgram address: prefix and length.
/// Full checksum verification happens where the address is used against
/// the chain; here the point is to refuse junk cheaply.
fn address(field: &'static str, s: &str) -> Result<(), ValidationError> {
    let ok = s.starts_with("hash1")
        && (38..=90).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if !ok {
        return Err(ValidationError::Address(field));
    }
    Ok(())
}

fn text(field: &'static str, s: &str) -> Result<(), ValidationError> {
    bound(field, s.len(), MAX_TEXT_BYTES)
}

fn tags(field: &'static str, items: &[String]) -> Result<(), ValidationError> {
    bound(field, items.len(), MAX_TAGS)?;
    for t in items {
        bound(field, t.len(), MAX_TAG_BYTES)?;
        if t.is_empty() {
            return Err(ValidationError::Missing(field));
        }
    }
    Ok(())
}

fn opt_hash(field: &'static str, b: &[u8]) -> Result<(), ValidationError> {
    if b.is_empty() {
        return Ok(());
    }
    exact(field, b.len(), HASH_LEN)
}

fn hash(field: &'static str, b: &[u8]) -> Result<(), ValidationError> {
    exact(field, b.len(), HASH_LEN)
}

fn decode<M: Message + Default>(name: &'static str, b: &[u8]) -> Result<M, ValidationError> {
    M::decode(b).map_err(|e| ValidationError::Payload(name, e.to_string()))
}

/// Validates a social event's structure and its typed payload.
pub fn social_event(ev: &pb::SocialEvent, now: u64) -> Result<(), ValidationError> {
    if ev.version != WIRE_VERSION {
        return Err(ValidationError::Version("event", ev.version));
    }
    if ev.network_id.is_empty() {
        return Err(ValidationError::Missing("network_id"));
    }
    hash("id", &ev.id)?;
    address("author", &ev.author)?;
    hash("device_pubkey", &ev.device_pubkey)?;
    exact("signature", ev.signature.len(), SIG_LEN)?;
    not_future(ev.timestamp, now)?;
    if ev.sequence == 0 && !ev.previous_event.is_empty() {
        return Err(ValidationError::Rule(
            "sequence 0 must have no previous event",
        ));
    }
    if ev.sequence > 0 {
        hash("previous_event", &ev.previous_event)?;
    }
    bound("payload", ev.payload.len(), MAX_EVENT_PAYLOAD)?;
    bound("media", ev.media.len(), MAX_EVENT_MEDIA)?;
    for m in &ev.media {
        hash("media.cid", &m.cid)?;
        bound("media.mime", m.mime.len(), 128)?;
        bound("media.kind", m.kind.len(), 16)?;
        opt_hash("media.thumbnail_cid", &m.thumbnail_cid)?;
        opt_hash("media.content_hash", &m.content_hash)?;
        if m.size > MAX_BLOB_SIZE {
            return Err(ValidationError::TooLarge {
                field: "media.size",
                actual: m.size as usize,
                max: MAX_BLOB_SIZE as usize,
            });
        }
    }
    if !EVENT_TYPES.contains(&ev.r#type.as_str()) {
        return Err(ValidationError::UnknownEventType(ev.r#type.clone()));
    }
    event_payload(ev)
}

fn event_payload(ev: &pb::SocialEvent) -> Result<(), ValidationError> {
    let p = &ev.payload;
    match ev.r#type.as_str() {
        "PROFILE_UPDATE" => {
            let v: pb::ProfileUpdate = decode("ProfileUpdate", p)?;
            bound("display_name", v.display_name.len(), 128)?;
            text("bio", &v.bio)?;
            opt_hash("avatar_cid", &v.avatar_cid)?;
            opt_hash("banner_cid", &v.banner_cid)?;
            bound("website", v.website.len(), 256)?;
            bound("attributes", v.attributes.len(), 32)?;
            for (k, val) in &v.attributes {
                bound("attribute key", k.len(), 64)?;
                bound("attribute value", val.len(), 512)?;
            }
        }
        "FOLLOW" => {
            let v: pb::Follow = decode("Follow", p)?;
            address("target", &v.target)?;
            if v.target == ev.author {
                return Err(ValidationError::Rule("cannot follow yourself"));
            }
        }
        "UNFOLLOW" => {
            let v: pb::Unfollow = decode("Unfollow", p)?;
            address("target", &v.target)?;
        }
        "POST_CREATE" => {
            let v: pb::PostCreate = decode("PostCreate", p)?;
            text("text", &v.text)?;
            tags("hashtags", &v.hashtags)?;
            tags("mentions", &v.mentions)?;
            opt_hash("channel", &v.channel)?;
            opt_hash("reply_to", &v.reply_to)?;
            bound("language", v.language.len(), 16)?;
            if v.text.is_empty() && ev.media.is_empty() {
                return Err(ValidationError::Rule("a post needs text or media"));
            }
        }
        "POST_EDIT" => {
            let v: pb::PostEdit = decode("PostEdit", p)?;
            hash("post", &v.post)?;
            text("text", &v.text)?;
            tags("hashtags", &v.hashtags)?;
        }
        "POST_DELETE" => {
            let v: pb::PostDelete = decode("PostDelete", p)?;
            hash("post", &v.post)?;
        }
        "COMMENT_CREATE" => {
            let v: pb::CommentCreate = decode("CommentCreate", p)?;
            hash("post", &v.post)?;
            text("text", &v.text)?;
            opt_hash("parent_comment", &v.parent_comment)?;
            if v.text.is_empty() {
                return Err(ValidationError::Missing("text"));
            }
        }
        "REACTION" => {
            let v: pb::Reaction = decode("Reaction", p)?;
            hash("target", &v.target)?;
            bound("reaction", v.reaction.len(), MAX_TAG_BYTES)?;
        }
        "REPOST" => {
            let v: pb::Repost = decode("Repost", p)?;
            hash("post", &v.post)?;
            text("comment", &v.comment)?;
        }
        "CHANNEL_CREATE" => {
            let v: pb::ChannelCreate = decode("ChannelCreate", p)?;
            bound("name", v.name.len(), 128)?;
            if v.name.is_empty() {
                return Err(ValidationError::Missing("name"));
            }
            text("description", &v.description)?;
            opt_hash("avatar_cid", &v.avatar_cid)?;
        }
        "REEL_CREATE" => {
            let v: pb::ReelCreate = decode("ReelCreate", p)?;
            text("caption", &v.caption)?;
            tags("hashtags", &v.hashtags)?;
            tags("mentions", &v.mentions)?;
            bound("audio_credit", v.audio_credit.len(), 256)?;
            bound("language", v.language.len(), 16)?;
            let Some(video) = ev.media.get(v.video_index as usize) else {
                return Err(ValidationError::Rule(
                    "reel video_index does not name a media entry",
                ));
            };
            if video.kind != "video" {
                return Err(ValidationError::Rule(
                    "reel video must be a video media entry",
                ));
            }
        }
        "STORY_CREATE" => {
            let v: pb::StoryCreate = decode("StoryCreate", p)?;
            text("caption", &v.caption)?;
            if ev.media.is_empty() {
                return Err(ValidationError::Rule("a story needs media"));
            }
            if v.expires_at <= ev.timestamp
                || v.expires_at > ev.timestamp.saturating_add(MAX_STORY_SECS)
            {
                return Err(ValidationError::Expiry {
                    created_at: ev.timestamp,
                    expires_at: v.expires_at,
                    max_secs: MAX_STORY_SECS,
                });
            }
        }
        other => return Err(ValidationError::UnknownEventType(other.to_owned())),
    }
    Ok(())
}

/// Validates an envelope's structure. Returns the expiry the node should
/// actually apply, clamped to its retention bound.
pub fn envelope(env: &pb::Envelope, now: u64) -> Result<u64, ValidationError> {
    if env.version != WIRE_VERSION {
        return Err(ValidationError::Version("envelope", env.version));
    }
    if env.network_id.is_empty() {
        return Err(ValidationError::Missing("network_id"));
    }
    hash("id", &env.id)?;
    hash("mailbox", &env.mailbox)?;
    if env.kind == pb::EnvelopeKind::Unspecified as i32
        || pb::EnvelopeKind::try_from(env.kind).is_err()
    {
        return Err(ValidationError::Missing("kind"));
    }
    if env.ciphertext.is_empty() {
        return Err(ValidationError::Missing("ciphertext"));
    }
    bound("ciphertext", env.ciphertext.len(), MAX_ENVELOPE_CIPHERTEXT)?;
    not_future(env.created_at, now)?;
    if env.expires_at <= now {
        return Err(ValidationError::Expiry {
            created_at: env.created_at,
            expires_at: env.expires_at,
            max_secs: MAX_ENVELOPE_RETENTION_SECS,
        });
    }
    Ok(env
        .expires_at
        .min(now.saturating_add(MAX_ENVELOPE_RETENTION_SECS)))
}

/// Validates a mailbox fetch request's structure.
pub fn mailbox_fetch(f: &pb::MailboxFetch, now: u64) -> Result<u32, ValidationError> {
    hash("mailbox", &f.mailbox)?;
    hash("device_pubkey", &f.device_pubkey)?;
    exact("signature", f.signature.len(), SIG_LEN)?;
    bound("cursor", f.cursor.len(), 64)?;
    fresh(f.timestamp, now)?;
    // The sentinel is reserved for TURN credential requests so that a
    // signature over one can never be presented as a fetch, and vice versa.
    if f.limit == TURN_SENTINEL_LIMIT {
        return Err(ValidationError::Rule(
            "limit is the TURN credential sentinel, not a page size",
        ));
    }
    Ok(if f.limit == 0 {
        MAX_MAILBOX_PAGE
    } else {
        f.limit.min(MAX_MAILBOX_PAGE)
    })
}

/// Validates the `MailboxFetch` preimage a TURN credential request signs.
///
/// This is the mirror image of [`mailbox_fetch`]: the cursor must be empty
/// and the limit must be exactly [`TURN_SENTINEL_LIMIT`], which
/// [`mailbox_fetch`] refuses. Together the two functions guarantee that no
/// signature is acceptable on both paths, so a store node that received a
/// legitimate fetch cannot present it for TURN credentials.
pub fn turn_credential_fetch(f: &pb::MailboxFetch, now: u64) -> Result<(), ValidationError> {
    hash("mailbox", &f.mailbox)?;
    hash("device_pubkey", &f.device_pubkey)?;
    exact("signature", f.signature.len(), SIG_LEN)?;
    if !f.cursor.is_empty() {
        return Err(ValidationError::Rule(
            "a TURN credential request carries no cursor",
        ));
    }
    if f.limit != TURN_SENTINEL_LIMIT {
        return Err(ValidationError::Rule(
            "a TURN credential request must carry the sentinel limit",
        ));
    }
    fresh(f.timestamp, now)
}

/// Validates a mailbox ack's structure.
pub fn mailbox_ack(a: &pb::MailboxAck, now: u64) -> Result<(), ValidationError> {
    hash("mailbox", &a.mailbox)?;
    hash("device_pubkey", &a.device_pubkey)?;
    exact("signature", a.signature.len(), SIG_LEN)?;
    bound(
        "envelope_ids",
        a.envelope_ids.len(),
        MAX_MAILBOX_PAGE as usize,
    )?;
    for id in &a.envelope_ids {
        hash("envelope_id", id)?;
    }
    fresh(a.timestamp, now)
}

/// Validates a key package publication's structure.
pub fn key_package(k: &pb::KeyPackagePublish, now: u64) -> Result<(), ValidationError> {
    hash("device_pubkey", &k.device_pubkey)?;
    exact("signature", k.signature.len(), SIG_LEN)?;
    if k.key_package.is_empty() {
        return Err(ValidationError::Missing("key_package"));
    }
    bound("key_package", k.key_package.len(), MAX_KEY_PACKAGE)?;
    not_future(k.created_at, now)?;
    if k.expires_at <= now {
        return Err(ValidationError::Expiry {
            created_at: k.created_at,
            expires_at: k.expires_at,
            max_secs: MAX_ENVELOPE_RETENTION_SECS,
        });
    }
    Ok(())
}

/// Validates a node announcement's structure.
pub fn node_announce(a: &pb::NodeAnnounce, now: u64) -> Result<(), ValidationError> {
    if a.version != WIRE_VERSION {
        return Err(ValidationError::Version("announce", a.version));
    }
    if a.node_pubkey.is_empty() {
        return Err(ValidationError::Missing("node_pubkey"));
    }
    bound("node_pubkey", a.node_pubkey.len(), 64)?;
    exact("signature", a.signature.len(), SIG_LEN)?;
    bound("roles", a.roles.len(), 16)?;
    for r in &a.roles {
        bound("role", r.len(), 16)?;
    }
    bound("addrs", a.addrs.len(), MAX_ANNOUNCE_ADDRS)?;
    for addr in &a.addrs {
        bound("addr", addr.len(), MAX_MULTIADDR_BYTES)?;
    }
    if !a.operator_address.is_empty() {
        address("operator_address", &a.operator_address)?;
    }
    not_future(a.timestamp, now)?;
    if a.expires_at <= now || a.expires_at > a.timestamp.saturating_add(MAX_ANNOUNCE_TTL_SECS) {
        return Err(ValidationError::Expiry {
            created_at: a.timestamp,
            expires_at: a.expires_at,
            max_secs: MAX_ANNOUNCE_TTL_SECS,
        });
    }
    if let Some(t) = &a.turn {
        bound("turn.uris", t.uris.len(), 8)?;
        for u in &t.uris {
            bound("turn.uri", u.len(), 256)?;
        }
        bound("turn.realm", t.realm.len(), 128)?;
    }
    if let Some(s) = &a.sfu {
        bound("sfu.url", s.url.len(), 256)?;
        bound("sfu.kind", s.kind.len(), 32)?;
    }
    Ok(())
}

/// Validates a bootstrap record's structure.
pub fn bootstrap_record(r: &pb::BootstrapRecord, now: u64) -> Result<(), ValidationError> {
    if r.addrs.is_empty() {
        return Err(ValidationError::Missing("addrs"));
    }
    bound("addrs", r.addrs.len(), MAX_ANNOUNCE_ADDRS)?;
    for addr in &r.addrs {
        bound("addr", addr.len(), MAX_MULTIADDR_BYTES)?;
    }
    hash("signer_pubkey", &r.signer_pubkey)?;
    exact("signature", r.signature.len(), SIG_LEN)?;
    not_future(r.issued_at, now)?;
    if r.expires_at <= now {
        return Err(ValidationError::Expiry {
            created_at: r.issued_at,
            expires_at: r.expires_at,
            max_secs: u64::MAX,
        });
    }
    Ok(())
}

/// Validates a content attestation's structure.
pub fn attestation(a: &pb::ContentAttestation, now: u64) -> Result<(), ValidationError> {
    if a.version != WIRE_VERSION {
        return Err(ValidationError::Version("attestation", a.version));
    }
    let set = [&a.cid, &a.event_id, &a.content_hash]
        .iter()
        .filter(|b| !b.is_empty())
        .count();
    if set != 1 {
        return Err(ValidationError::Rule(
            "exactly one of cid, event_id, content_hash must be set",
        ));
    }
    opt_hash("cid", &a.cid)?;
    opt_hash("event_id", &a.event_id)?;
    opt_hash("content_hash", &a.content_hash)?;
    if a.verdict == pb::Verdict::Unspecified as i32 || pb::Verdict::try_from(a.verdict).is_err() {
        return Err(ValidationError::Missing("verdict"));
    }
    bound("policy", a.policy.len(), 64)?;
    bound("reason_code", a.reason_code.len(), 64)?;
    if a.reason_code
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    {
        return Err(ValidationError::Rule(
            "reason_code must be a machine code, not free text",
        ));
    }
    hash("attestor_pubkey", &a.attestor_pubkey)?;
    exact("signature", a.signature.len(), SIG_LEN)?;
    not_future(a.timestamp, now)
}

/// Validates an upload authorisation's structure.
pub fn blob_put_manifest(p: &pb::BlobPutManifest, now: u64) -> Result<(), ValidationError> {
    hash("cid", &p.cid)?;
    hash("uploader_pubkey", &p.uploader_pubkey)?;
    exact("signature", p.signature.len(), SIG_LEN)?;
    if p.manifest.is_none() {
        return Err(ValidationError::Missing("manifest"));
    }
    fresh(p.timestamp, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;

    fn base(kind: &str, payload: Vec<u8>) -> pb::SocialEvent {
        pb::SocialEvent {
            network_id: "hashgram-devnet".into(),
            version: 1,
            id: vec![1; 32],
            r#type: kind.into(),
            author: "hash1qpzry9x8gf2tvdw0s3jn54khce6mua7lqqqqqqqq".into(),
            device_pubkey: vec![2; 32],
            timestamp: NOW,
            sequence: 0,
            payload,
            signature: vec![3; 64],
            ..Default::default()
        }
    }

    #[test]
    fn a_plain_post_passes() {
        let p = pb::PostCreate {
            text: "hi".into(),
            ..Default::default()
        };
        social_event(&base("POST_CREATE", p.encode_to_vec()), NOW).unwrap();
    }

    #[test]
    fn every_family_is_recognised_and_nothing_else() {
        for t in EVENT_TYPES {
            let ev = base(t, vec![]);
            // Structure is fine; payload rules may fail, but not on type.
            let err = social_event(&ev, NOW).err();
            assert!(
                !matches!(err, Some(ValidationError::UnknownEventType(_))),
                "{t}"
            );
        }
        assert!(matches!(
            social_event(&base("LIKE", vec![]), NOW),
            Err(ValidationError::UnknownEventType(_))
        ));
    }

    #[test]
    fn a_future_event_is_refused() {
        let mut ev = base(
            "POST_CREATE",
            pb::PostCreate {
                text: "x".into(),
                ..Default::default()
            }
            .encode_to_vec(),
        );
        ev.timestamp = NOW + 3600;
        assert!(matches!(
            social_event(&ev, NOW),
            Err(ValidationError::FutureTimestamp { .. })
        ));
    }

    #[test]
    fn following_yourself_is_refused() {
        let author = base("FOLLOW", vec![]).author;
        let p = pb::Follow { target: author };
        assert!(matches!(
            social_event(&base("FOLLOW", p.encode_to_vec()), NOW),
            Err(ValidationError::Rule(_))
        ));
    }

    #[test]
    fn a_reel_needs_a_video() {
        let p = pb::ReelCreate {
            caption: "c".into(),
            ..Default::default()
        };
        let mut ev = base("REEL_CREATE", p.encode_to_vec());
        assert!(social_event(&ev, NOW).is_err());
        ev.media.push(pb::MediaReference {
            cid: vec![4; 32],
            kind: "image".into(),
            ..Default::default()
        });
        assert!(social_event(&ev, NOW).is_err());
        ev.media[0].kind = "video".into();
        social_event(&ev, NOW).unwrap();
    }

    #[test]
    fn a_story_expiry_is_bounded() {
        let mut ev = base("STORY_CREATE", vec![]);
        ev.media.push(pb::MediaReference {
            cid: vec![4; 32],
            kind: "image".into(),
            ..Default::default()
        });
        for (exp, ok) in [
            (NOW + 3600, true),
            (NOW, false),
            (NOW + MAX_STORY_SECS + 1, false),
        ] {
            ev.payload = pb::StoryCreate {
                expires_at: exp,
                ..Default::default()
            }
            .encode_to_vec();
            assert_eq!(social_event(&ev, NOW).is_ok(), ok, "expiry {exp}");
        }
    }

    #[test]
    fn oversized_text_is_refused() {
        let p = pb::PostCreate {
            text: "x".repeat(MAX_TEXT_BYTES + 1),
            ..Default::default()
        };
        assert!(matches!(
            social_event(&base("POST_CREATE", p.encode_to_vec()), NOW),
            Err(ValidationError::TooLarge { field: "text", .. })
        ));
    }

    #[test]
    fn envelope_expiry_is_clamped_to_retention() {
        let env = pb::Envelope {
            network_id: "hashgram-devnet".into(),
            version: 1,
            id: vec![1; 32],
            mailbox: vec![2; 32],
            kind: pb::EnvelopeKind::MlsMessage as i32,
            ciphertext: vec![0; 10],
            created_at: NOW,
            expires_at: NOW + 365 * 24 * 3600,
        };
        assert_eq!(
            envelope(&env, NOW).unwrap(),
            NOW + MAX_ENVELOPE_RETENTION_SECS
        );
        let mut expired = env.clone();
        expired.expires_at = NOW - 1;
        assert!(envelope(&expired, NOW).is_err());
    }

    #[test]
    fn stale_requests_are_refused() {
        assert!(fresh(NOW - MAX_REQUEST_AGE_SECS - 1, NOW).is_err());
        assert!(fresh(NOW - 10, NOW).is_ok());
        assert!(fresh(NOW + MAX_FUTURE_SKEW_SECS + 1, NOW).is_err());
    }

    fn fetch_shape(limit: u32, cursor: Vec<u8>) -> pb::MailboxFetch {
        pb::MailboxFetch {
            mailbox: vec![1; 32],
            device_pubkey: vec![2; 32],
            cursor,
            limit,
            timestamp: NOW,
            signature: vec![3; 64],
        }
    }

    #[test]
    fn mailbox_fetch_refuses_the_turn_sentinel() {
        // A real fetch with the default limit is a page of MAX_MAILBOX_PAGE.
        assert_eq!(
            mailbox_fetch(&fetch_shape(0, vec![]), NOW).unwrap(),
            MAX_MAILBOX_PAGE
        );
        assert_eq!(mailbox_fetch(&fetch_shape(7, vec![]), NOW).unwrap(), 7);
        // The TURN sentinel is never a page size.
        assert!(matches!(
            mailbox_fetch(&fetch_shape(TURN_SENTINEL_LIMIT, vec![]), NOW),
            Err(ValidationError::Rule(_))
        ));
    }

    #[test]
    fn turn_credential_fetch_requires_the_sentinel_and_no_cursor() {
        turn_credential_fetch(&fetch_shape(TURN_SENTINEL_LIMIT, vec![]), NOW).unwrap();
        // What a legitimate mailbox fetch looks like is refused on this path.
        assert!(matches!(
            turn_credential_fetch(&fetch_shape(0, vec![]), NOW),
            Err(ValidationError::Rule(_))
        ));
        assert!(matches!(
            turn_credential_fetch(&fetch_shape(MAX_MAILBOX_PAGE, vec![]), NOW),
            Err(ValidationError::Rule(_))
        ));
        assert!(matches!(
            turn_credential_fetch(&fetch_shape(TURN_SENTINEL_LIMIT, vec![9; 40]), NOW),
            Err(ValidationError::Rule(_))
        ));
        // Freshness still applies.
        let mut old = fetch_shape(TURN_SENTINEL_LIMIT, vec![]);
        old.timestamp = NOW - MAX_REQUEST_AGE_SECS - 1;
        assert!(matches!(
            turn_credential_fetch(&old, NOW),
            Err(ValidationError::Stale { .. })
        ));
    }

    #[test]
    fn attestation_names_exactly_one_subject() {
        let mut a = pb::ContentAttestation {
            version: 1,
            verdict: pb::Verdict::ContentBlock as i32,
            reason_code: "spam".into(),
            timestamp: NOW,
            attestor_pubkey: vec![1; 32],
            signature: vec![2; 64],
            ..Default::default()
        };
        assert!(attestation(&a, NOW).is_err());
        a.cid = vec![3; 32];
        attestation(&a, NOW).unwrap();
        a.event_id = vec![4; 32];
        assert!(attestation(&a, NOW).is_err());
        a.event_id.clear();
        a.reason_code = "the content was: ...".into();
        assert!(attestation(&a, NOW).is_err());
    }
}
