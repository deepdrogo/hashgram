//! Feed, reels, stories, channels: signed social events.
//!
//! Every event shown has (1) a valid signature by the device key it names
//! (checked by the SDK) and (2) that device key resolving on chain to the
//! event's author (checked here, cached). Anything failing either check is
//! counted and hidden. Events are cached in the database as they were
//! served (public data, not sealed); follows, mutes and blocks are local.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::state::SharedAccount;
use hashgram_sdk::account::Account;
use hashgram_sdk::link::Link;
use hashgram_sdk::social::{payload_json, Social};
use hashgram_sdk::{blob, pb, ChainClient, NetworkIdentity};
use prost::Message;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::Db;

/// An event as the UI shows it.
#[derive(Debug, Clone, Serialize)]
pub struct EventView {
    /// Event id hex.
    pub id: String,
    /// Type.
    pub kind: String,
    /// Author address.
    pub author: String,
    /// Author's verified @username, if the caller resolved one.
    pub username: Option<String>,
    /// Author's display name from their latest PROFILE_UPDATE, if cached.
    pub display_name: Option<String>,
    /// Sequence.
    pub sequence: u64,
    /// Unix seconds.
    pub timestamp: u64,
    /// Typed payload as JSON.
    pub payload: serde_json::Value,
    /// Media references.
    pub media: Vec<MediaView>,
    /// Device key hex.
    pub device: String,
    /// Reactions folded: reaction → count.
    #[serde(default)]
    pub reactions: HashMap<String, u32>,
    /// Comment count (from cached events).
    #[serde(default)]
    pub comments: u32,
    /// Repost count.
    #[serde(default)]
    pub reposts: u32,
    /// Whether this account reacted (any reaction).
    #[serde(default)]
    pub my_reaction: Option<String>,
}

/// Media on an event.
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct MediaView {
    /// CID hex.
    pub cid: String,
    /// MIME.
    pub mime: String,
    /// Size.
    pub size: u64,
    /// image/video/audio.
    pub kind: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Duration ms.
    pub duration_ms: u32,
    /// Content hash hex.
    pub content_hash: String,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The hub.
pub struct SocialHub {
    inner: Mutex<Option<Social>>,
    /// device pubkey hex → (author or "" when unauthorised, checked at).
    authz: Mutex<HashMap<String, (String, Instant)>>,
    /// Hidden (unverifiable) event count since start.
    hidden: std::sync::atomic::AtomicU32,
}

impl Default for SocialHub {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            authz: Mutex::new(HashMap::new()),
            hidden: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

const AUTHZ_TTL: Duration = Duration::from_secs(600);

impl SocialHub {
    /// Opens the per-device chain state (at unlock).
    pub async fn open(&self, account: &Account) -> Result<(), String> {
        *self.inner.lock().await = Some(Social::open(account).map_err(|e| e.to_string())?);
        Ok(())
    }

    /// Drops it (at lock).
    pub async fn close(&self) {
        *self.inner.lock().await = None;
    }

    /// How many events were hidden as unverifiable.
    pub fn hidden_count(&self) -> u32 {
        self.hidden.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Builds, signs and publishes an event; persists the sequence chain.
    pub async fn publish<M: Message>(
        &self,
        account: &SharedAccount,
        link: &Link,
        network: &NetworkIdentity,
        kind: &str,
        payload: &M,
        media: Vec<pb::MediaReference>,
    ) -> Result<pb::SocialEvent, String> {
        let mut guard = self.inner.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| "social is not open (locked?)".to_owned())?;
        let ev = s
            .build(network, kind, payload, media)
            .map_err(|e| e.to_string())?;
        s.publish(link, ev.clone())
            .await
            .map_err(|e| e.to_string())?;
        let mut a = account.lock().await;
        s.persist(&mut a);
        a.save().map_err(|e| e.to_string())?;
        Ok(ev)
    }

    /// Checks that `device` is an active device of `author` on chain, with
    /// a cache. `None` chain means "cannot check": the event is hidden.
    pub async fn device_authorised(
        &self,
        chain: Option<&ChainClient>,
        author: &str,
        device_hex: &str,
    ) -> bool {
        {
            let a = self.authz.lock().await;
            if let Some((who, at)) = a.get(device_hex) {
                if at.elapsed() < AUTHZ_TTL {
                    return who == author;
                }
            }
        }
        let Some(chain) = chain else {
            return false;
        };
        let Ok(bytes) = hex::decode(device_hex) else {
            return false;
        };
        let b64 = base64_std(&bytes);
        let who = match chain
            .query(&format!(
                "hashgram/identity/v1/resolve_device_key?device_pubkey={}",
                urlenc(&b64)
            ))
            .await
        {
            Ok(v) => {
                let found = v.get("found").and_then(|f| f.as_bool()).unwrap_or(false);
                let revoked = v
                    .get("device")
                    .and_then(|d| d.get("revoked"))
                    .and_then(|r| r.as_bool())
                    .unwrap_or(false);
                if found && !revoked {
                    v.get("root_address")
                        .and_then(|r| r.as_str())
                        .unwrap_or("")
                        .to_owned()
                } else {
                    String::new()
                }
            }
            Err(_) => return false, // unavailable: do not cache, do not show
        };
        self.authz
            .lock()
            .await
            .insert(device_hex.to_owned(), (who.clone(), Instant::now()));
        who == author
    }

    /// Fetches an author's recent events, verifies, caches. Returns how
    /// many new events were stored.
    pub async fn refresh_author(
        &self,
        db: &Db,
        link: &Link,
        network: &NetworkIdentity,
        chain: Option<&ChainClient>,
        author: &str,
        limit: u32,
    ) -> Result<usize, String> {
        let from = db
            .with(|c| {
                c.query_row(
                    "SELECT COALESCE(MAX(sequence), -1) FROM social_events WHERE author = ?1 AND verified = 1",
                    params![author],
                    |r| r.get::<_, i64>(0),
                )
            })
            .unwrap_or(-1);
        let events = {
            let guard = self.inner.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| "social is not open".to_owned())?;
            s.fetch_author(link, network, author, (from + 1).max(0) as u64, limit)
                .await
                .map_err(|e| e.to_string())?
        };
        let mut stored = 0;
        for ev in events {
            let dev = hex::encode(&ev.device_pubkey);
            let verified = self.device_authorised(chain, &ev.author, &dev).await;
            if !verified {
                self.hidden
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            if store_event(db, &ev, true)? {
                stored += 1;
            }
        }
        Ok(stored)
    }

    /// Fetches specific events by id (thread loading).
    pub async fn fetch_ids(
        &self,
        db: &Db,
        link: &Link,
        network: &NetworkIdentity,
        chain: Option<&ChainClient>,
        ids: &[String],
    ) -> Result<usize, String> {
        let raw: Vec<Vec<u8>> = ids.iter().filter_map(|i| hex::decode(i).ok()).collect();
        if raw.is_empty() {
            return Ok(0);
        }
        let events = {
            let guard = self.inner.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| "social is not open".to_owned())?;
            s.fetch_ids(link, network, raw)
                .await
                .map_err(|e| e.to_string())?
        };
        let mut stored = 0;
        for ev in events {
            let dev = hex::encode(&ev.device_pubkey);
            if self.device_authorised(chain, &ev.author, &dev).await && store_event(db, &ev, true)?
            {
                stored += 1;
            }
        }
        Ok(stored)
    }
}

fn base64_std(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk.first().copied().unwrap_or(0),
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(
                    T.get(((n >> (18 - 6 * i)) & 63) as usize)
                        .copied()
                        .unwrap_or(b'A') as char,
                );
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// -- database ---------------------------------------------------------------

/// Stores an event (verbatim protobuf). Returns `true` when new.
pub fn store_event(db: &Db, ev: &pb::SocialEvent, verified: bool) -> Result<bool, String> {
    let id = hex::encode(&ev.id);
    let body = ev.encode_to_vec();
    let n = db.with(|c| {
        c.execute(
            "INSERT INTO social_events(id, author, sequence, kind, ts, verified, body) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO NOTHING",
            params![id, ev.author, ev.sequence as i64, ev.r#type, ev.timestamp as i64, verified as i64, body],
        )
    })?;
    Ok(n > 0)
}

fn decode_row(body: &[u8]) -> Option<pb::SocialEvent> {
    pb::SocialEvent::decode(body).ok()
}

fn view_of(ev: &pb::SocialEvent) -> EventView {
    EventView {
        id: hex::encode(&ev.id),
        kind: ev.r#type.clone(),
        author: ev.author.clone(),
        username: None,
        display_name: None,
        sequence: ev.sequence,
        timestamp: ev.timestamp,
        payload: payload_json(ev),
        media: ev
            .media
            .iter()
            .map(|m| MediaView {
                cid: hex::encode(&m.cid),
                mime: m.mime.clone(),
                size: m.size,
                kind: m.kind.clone(),
                width: m.width,
                height: m.height,
                duration_ms: m.duration_ms,
                content_hash: hex::encode(&m.content_hash),
            })
            .collect(),
        device: hex::encode(&ev.device_pubkey),
        reactions: HashMap::new(),
        comments: 0,
        reposts: 0,
        my_reaction: None,
    }
}

/// Folds reactions, comments and reposts (cached events) onto `views`.
fn fold_interactions(db: &Db, views: &mut [EventView], me: &str) -> Result<(), String> {
    if views.is_empty() {
        return Ok(());
    }
    let rows: Vec<(String, String, Vec<u8>)> = db.with(|c| {
        let mut st = c.prepare("SELECT kind, author, body FROM social_events WHERE verified = 1 AND kind IN ('REACTION','COMMENT_CREATE','REPOST')")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    })?;
    let mut idx: HashMap<String, usize> = views
        .iter()
        .enumerate()
        .map(|(i, v)| (v.id.clone(), i))
        .collect();
    for (kind, author, body) in rows {
        let Some(ev) = decode_row(&body) else {
            continue;
        };
        let target = match kind.as_str() {
            "REACTION" => pb::Reaction::decode(ev.payload.as_slice())
                .ok()
                .map(|r| (hex::encode(r.target), r.reaction)),
            "COMMENT_CREATE" => pb::CommentCreate::decode(ev.payload.as_slice())
                .ok()
                .map(|c| (hex::encode(c.post), String::new())),
            "REPOST" => pb::Repost::decode(ev.payload.as_slice())
                .ok()
                .map(|r| (hex::encode(r.post), String::new())),
            _ => None,
        };
        let Some((tid, reaction)) = target else {
            continue;
        };
        let Some(i) = idx.get_mut(&tid).copied() else {
            continue;
        };
        let Some(v) = views.get_mut(i) else { continue };
        match kind.as_str() {
            "REACTION" => {
                if !reaction.is_empty() {
                    *v.reactions.entry(reaction.clone()).or_default() += 1;
                    if author == me {
                        v.my_reaction = Some(reaction);
                    }
                }
            }
            "COMMENT_CREATE" => v.comments += 1,
            "REPOST" => v.reposts += 1,
            _ => {}
        }
    }
    Ok(())
}

/// Feed: newest first among `authors` (followed + self), optional kind
/// filter and hashtag.
pub fn feed(
    db: &Db,
    authors: &[String],
    kinds: &[&str],
    tag: Option<&str>,
    before_ts: Option<u64>,
    limit: usize,
    me: &str,
) -> Result<Vec<EventView>, String> {
    let mut sql = String::from("SELECT body FROM social_events WHERE verified = 1 AND ts < ?1");
    let mut args: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Integer(
        before_ts.map(|t| t as i64).unwrap_or(i64::MAX),
    )];
    if !authors.is_empty() {
        sql.push_str(" AND author IN (");
        sql.push_str(
            &std::iter::repeat_n("?", authors.len())
                .collect::<Vec<_>>()
                .join(","),
        );
        sql.push(')');
        for a in authors {
            args.push(rusqlite::types::Value::Text(a.clone()));
        }
    }
    if !kinds.is_empty() {
        sql.push_str(" AND kind IN (");
        sql.push_str(
            &std::iter::repeat_n("?", kinds.len())
                .collect::<Vec<_>>()
                .join(","),
        );
        sql.push(')');
        for k in kinds {
            args.push(rusqlite::types::Value::Text((*k).to_owned()));
        }
    }
    sql.push_str(" ORDER BY ts DESC LIMIT ?");
    args.push(rusqlite::types::Value::Integer(
        (limit * if tag.is_some() { 8 } else { 1 }) as i64,
    ));
    let bodies: Vec<Vec<u8>> = db.with(|c| {
        let mut st = c.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            r.get::<_, Vec<u8>>(0)
        })?;
        rows.collect()
    })?;
    let mut views: Vec<EventView> = bodies
        .iter()
        .filter_map(|b| decode_row(b))
        .map(|e| view_of(&e))
        .collect();
    if let Some(t) = tag {
        let t = t.trim_start_matches('#').to_lowercase();
        views.retain(|v| {
            v.payload
                .get("hashtags")
                .and_then(|h| h.as_array())
                .map(|a| {
                    a.iter().any(|x| {
                        x.as_str()
                            .map(|s| s.trim_start_matches('#').to_lowercase() == t)
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        views.truncate(limit);
    }
    fold_interactions(db, &mut views, me)?;
    Ok(views)
}

/// One event and the comments on it.
pub fn thread(db: &Db, id: &str, me: &str) -> Result<(Option<EventView>, Vec<EventView>), String> {
    let body: Option<Vec<u8>> = db.with(|c| {
        c.query_row(
            "SELECT body FROM social_events WHERE id = ?1 AND verified = 1",
            params![id],
            |r| r.get(0),
        )
        .optional()
    })?;
    let mut post = body.and_then(|b| decode_row(&b)).map(|e| view_of(&e));
    let rows: Vec<Vec<u8>> = db.with(|c| {
        let mut st = c.prepare("SELECT body FROM social_events WHERE verified = 1 AND kind = 'COMMENT_CREATE' ORDER BY ts ASC")?;
        let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        rows.collect()
    })?;
    let mut comments: Vec<EventView> = rows
        .iter()
        .filter_map(|b| decode_row(b))
        .filter(|e| {
            pb::CommentCreate::decode(e.payload.as_slice())
                .map(|c| hex::encode(c.post) == id)
                .unwrap_or(false)
        })
        .map(|e| view_of(&e))
        .collect();
    if let Some(p) = post.as_mut() {
        fold_interactions(db, std::slice::from_mut(p), me)?;
    }
    fold_interactions(db, &mut comments, me)?;
    Ok((post, comments))
}

/// Latest profile of an author from cached PROFILE_UPDATE events.
pub fn profile(db: &Db, author: &str) -> Result<Option<serde_json::Value>, String> {
    let body: Option<Vec<u8>> = db.with(|c| {
        c.query_row(
            "SELECT body FROM social_events WHERE verified = 1 AND author = ?1 AND kind = 'PROFILE_UPDATE' ORDER BY sequence DESC LIMIT 1",
            params![author],
            |r| r.get(0),
        )
        .optional()
    })?;
    Ok(body.and_then(|b| decode_row(&b)).map(|e| payload_json(&e)))
}

/// Channels created by `authors` (or all cached when empty).
pub fn channels(db: &Db, authors: &[String], me: &str) -> Result<Vec<EventView>, String> {
    feed(db, authors, &["CHANNEL_CREATE"], None, None, 200, me)
}

/// Posts in a channel (by channel event id).
pub fn channel_posts(
    db: &Db,
    channel_id: &str,
    limit: usize,
    me: &str,
) -> Result<Vec<EventView>, String> {
    let rows: Vec<Vec<u8>> = db.with(|c| {
        let mut st = c.prepare("SELECT body FROM social_events WHERE verified = 1 AND kind = 'POST_CREATE' ORDER BY ts DESC LIMIT 2000")?;
        let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
        rows.collect()
    })?;
    let mut views: Vec<EventView> = rows
        .iter()
        .filter_map(|b| decode_row(b))
        .filter(|e| {
            pb::PostCreate::decode(e.payload.as_slice())
                .map(|p| hex::encode(p.channel) == channel_id)
                .unwrap_or(false)
        })
        .map(|e| view_of(&e))
        .take(limit)
        .collect();
    fold_interactions(db, &mut views, me)?;
    Ok(views)
}

/// Follow list.
pub fn follows(db: &Db) -> Result<Vec<String>, String> {
    db.with(|c| {
        let mut st = c.prepare("SELECT address FROM follows ORDER BY since DESC")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect()
    })
}

/// Records a follow/unfollow locally.
pub fn set_follow(db: &Db, address: &str, follow: bool) -> Result<(), String> {
    db.with(|c| {
        if follow {
            c.execute("INSERT INTO follows(address, since) VALUES(?1, ?2) ON CONFLICT(address) DO NOTHING", params![address, now() as i64]).map(|_| ())
        } else {
            c.execute("DELETE FROM follows WHERE address = ?1", params![address]).map(|_| ())
        }
    })
}

/// Mute/block list (`mode` = mute | block).
pub fn blocks(db: &Db) -> Result<Vec<(String, String)>, String> {
    db.with(|c| {
        let mut st = c.prepare("SELECT address, mode FROM blocks")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })
}

/// Sets or clears a mute/block.
pub fn set_block(db: &Db, address: &str, mode: Option<&str>) -> Result<(), String> {
    db.with(|c| match mode {
        Some(m) => c
            .execute(
                "INSERT INTO blocks(address, mode, since) VALUES(?1, ?2, ?3) ON CONFLICT(address) DO UPDATE SET mode = excluded.mode",
                params![address, m, now() as i64],
            )
            .map(|_| ()),
        None => c.execute("DELETE FROM blocks WHERE address = ?1", params![address]).map(|_| ()),
    })
}

/// Downloads public media (reel, image) into the cache, verifying the
/// content hash; returns the path.
pub async fn fetch_media(
    link: &Link,
    m: &MediaView,
    receipt: Option<(
        &ChainClient,
        &NetworkIdentity,
        &hashgram_sdk::proto::Ed25519Signer,
    )>,
) -> Result<std::path::PathBuf, String> {
    let dir = crate::paths::data_dir().join("media-cache");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}{}", m.cid, crate::chat::ext_for(&m.mime)));
    if path.exists() {
        return Ok(path);
    }
    let cid = hex::decode(&m.cid).map_err(|e| e.to_string())?;
    let (bytes, _manifest, _from) = blob::download(link, &cid, receipt)
        .await
        .map_err(|e| e.to_string())?;
    if !m.content_hash.is_empty() && hex::encode(blake3::hash(&bytes).as_bytes()) != m.content_hash
    {
        return Err("media hash mismatch: the provider served corrupt data".into());
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Uploads public media and returns its reference.
pub async fn upload_media(
    link: &Link,
    network: &NetworkIdentity,
    device: &hashgram_sdk::proto::Ed25519Signer,
    data: &[u8],
    mime: &str,
    kind: &str,
    duration_ms: u32,
) -> Result<pb::MediaReference, String> {
    let up = blob::upload(link, network, device, data, mime, false, 2)
        .await
        .map_err(|e| e.to_string())?;
    Ok(pb::MediaReference {
        cid: hex::decode(&up.cid).map_err(|e| e.to_string())?,
        mime: mime.to_owned(),
        size: up.size,
        kind: kind.to_owned(),
        duration_ms,
        content_hash: hex::decode(&up.plaintext_hash).map_err(|e| e.to_string())?,
        ..Default::default()
    })
}

/// Largest reel accepted (100 MB, pre-encoded MP4).
pub const MAX_REEL_BYTES: usize = 100 * 1024 * 1024;

/// Shared hub handle type.
pub type SharedSocial = Arc<SocialHub>;

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hg-social-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("t.db")
    }

    fn ev(id: u8, author: &str, kind: &str, ts: u64, payload: Vec<u8>) -> pb::SocialEvent {
        pb::SocialEvent {
            id: vec![id; 32],
            r#type: kind.into(),
            author: author.into(),
            timestamp: ts,
            sequence: ts,
            payload,
            device_pubkey: vec![9; 32],
            ..Default::default()
        }
    }

    #[test]
    fn feed_folds_reactions_comments_and_filters_tags() {
        let db = Db::open(&tmp("feed")).unwrap();
        let post = ev(
            1,
            "hash1a",
            "POST_CREATE",
            100,
            pb::PostCreate {
                text: "gm #hashgram".into(),
                hashtags: vec!["hashgram".into()],
                ..Default::default()
            }
            .encode_to_vec(),
        );
        let other = ev(
            2,
            "hash1b",
            "POST_CREATE",
            90,
            pb::PostCreate {
                text: "hello".into(),
                ..Default::default()
            }
            .encode_to_vec(),
        );
        let react = ev(
            3,
            "hash1me",
            "REACTION",
            101,
            pb::Reaction {
                target: vec![1; 32],
                reaction: "🔥".into(),
            }
            .encode_to_vec(),
        );
        let comment = ev(
            4,
            "hash1b",
            "COMMENT_CREATE",
            102,
            pb::CommentCreate {
                post: vec![1; 32],
                text: "nice".into(),
                ..Default::default()
            }
            .encode_to_vec(),
        );
        let unverified = ev(
            5,
            "hash1evil",
            "POST_CREATE",
            103,
            pb::PostCreate {
                text: "spam".into(),
                ..Default::default()
            }
            .encode_to_vec(),
        );
        for (e, v) in [
            (&post, true),
            (&other, true),
            (&react, true),
            (&comment, true),
            (&unverified, false),
        ] {
            store_event(&db, e, v).unwrap();
        }
        let f = feed(&db, &[], &["POST_CREATE"], None, None, 50, "hash1me").unwrap();
        assert_eq!(f.len(), 2, "unverified events are hidden");
        assert_eq!(f[0].id, hex::encode([1u8; 32]));
        assert_eq!(f[0].reactions.get("🔥"), Some(&1));
        assert_eq!(f[0].my_reaction.as_deref(), Some("🔥"));
        assert_eq!(f[0].comments, 1);
        let tagged = feed(
            &db,
            &[],
            &["POST_CREATE"],
            Some("#Hashgram"),
            None,
            50,
            "hash1me",
        )
        .unwrap();
        assert_eq!(tagged.len(), 1);
        let only_b = feed(
            &db,
            &["hash1b".into()],
            &["POST_CREATE"],
            None,
            None,
            50,
            "hash1me",
        )
        .unwrap();
        assert_eq!(only_b.len(), 1);
        let (p, comments) = thread(&db, &hex::encode([1u8; 32]), "hash1me").unwrap();
        assert!(p.is_some());
        assert_eq!(comments.len(), 1);
        set_follow(&db, "hash1b", true).unwrap();
        assert_eq!(follows(&db).unwrap(), vec!["hash1b".to_owned()]);
        set_block(&db, "hash1evil", Some("block")).unwrap();
        assert_eq!(blocks(&db).unwrap().len(), 1);
    }
}
