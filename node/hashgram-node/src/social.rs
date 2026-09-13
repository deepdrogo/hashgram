//! The public social event log.
//!
//! Every node that relays social events keeps the ones it has verified, so a
//! client can fetch history from any of them and an indexer can rebuild from
//! any of them. Acceptance is strict and cheap-first:
//!
//! 1. Shape and bounds ([`validate::social_event`]).
//! 2. Signature and id ([`signing::verify_social_event`]).
//! 3. The signing device is an authorised, unrevoked device of the author
//!    **on chain**. This is the step that stops a relay forging Alice's
//!    posts: it can sign with any key it likes, and none of them is Alice's
//!    device.
//! 4. Not already held. Two events with one `(author, sequence)` and
//!    different ids are both kept: that is evidence of a misbehaving device.
//! 5. Not blocked by a trusted safety attestation.
//!
//! When the chain is unreachable the node neither accepts nor punishes: the
//! event is ignored (not propagated) and the peer is not scored, because the
//! failure is local.
//!
//! 6. Within the author's rate allowance ([`RateLimiter`]). A registered
//!    identity is cheap enough that a script could still flood the log;
//!    per-author allowances keep one author from filling everybody's
//!    Explore. A limited event is ignored, not punished: the relaying peer
//!    did nothing wrong.
//!
//! Besides the id/author/time indexes the node keeps three derived ones so
//! that clients can page a wall, a hashtag or the engagement under a post
//! without pulling the whole log: by target, by channel and by hashtag. All
//! three are rebuilt from `EVENTS` when a node with an older store starts.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use hashgram_p2p::{topics, MessageAcceptance, PeerId, ScoreEvent};
use hashgram_proto::limits::{
    DEFAULT_DIGEST_WINDOW_SECS, MAX_DIGEST_ENTRIES, MAX_DIGEST_SCAN, MAX_DIGEST_WINDOW_SECS,
    MAX_EVENT_PAGE, MAX_TIMELINE_SCAN,
};
use hashgram_proto::pb;
use hashgram_proto::{signing, validate};
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use tracing::{debug, info, warn};

use crate::app::Shared;
use crate::store::now;

// id -> encoded SocialEvent
const EVENTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("events");
// (author, sequence, id) -> ()
const BY_AUTHOR: TableDefinition<(&str, u64, &[u8]), ()> = TableDefinition::new("events_by_author");
// (timestamp, id) -> ()
const BY_TIME: TableDefinition<(u64, &[u8]), ()> = TableDefinition::new("events_by_time");
// (referenced post/comment id, timestamp, id) -> ()
const BY_TARGET: TableDefinition<(&[u8], u64, &[u8]), ()> =
    TableDefinition::new("events_by_target");
// (channel id, timestamp, id) -> (); the CHANNEL_CREATE itself is filed
// under its own id so an empty wall is still listed.
const BY_CHANNEL: TableDefinition<(&[u8], u64, &[u8]), ()> =
    TableDefinition::new("events_by_channel");
// (hashtag, timestamp, id) -> ()
const BY_TAG: TableDefinition<(&str, u64, &[u8]), ()> = TableDefinition::new("events_by_tag");
// Small key/values about the store itself.
const META: TableDefinition<&str, u64> = TableDefinition::new("social_meta");
/// Bump when a derived index is added or its keys change; older stores are
/// re-indexed on open.
const INDEX_VERSION: u64 = 2;

/// The largest id, for inclusive range ends.
const MAX_ID: [u8; 32] = [0xff; 32];

/// What an event points at, derived from its typed payload.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refs {
    /// Posts or comments this event references (reply, comment, reaction,
    /// repost, edit, delete).
    pub targets: Vec<Vec<u8>>,
    /// Channel (wall) a post belongs to.
    pub channel: Option<Vec<u8>>,
    /// Normalised hashtags (lower-case, no `#`).
    pub tags: Vec<String>,
}

/// Lower-cases a hashtag and drops a leading `#`; empty tags are dropped
/// by the caller.
#[must_use]
pub fn normalize_tag(t: &str) -> String {
    t.trim().trim_start_matches('#').to_lowercase()
}

fn norm_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = tags
        .iter()
        .map(|t| normalize_tag(t))
        .filter(|t| !t.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn opt(b: Vec<u8>) -> Option<Vec<u8>> {
    (!b.is_empty()).then_some(b)
}

/// Decodes the references an event carries. Tolerant: an undecodable
/// payload yields no references rather than an error, because the event
/// already passed validation when it was stored.
#[must_use]
pub fn refs_of(ev: &pb::SocialEvent) -> Refs {
    let p = ev.payload.as_slice();
    let mut r = Refs::default();
    match ev.r#type.as_str() {
        "POST_CREATE" => {
            if let Ok(v) = pb::PostCreate::decode(p) {
                r.channel = opt(v.channel);
                r.targets.extend(opt(v.reply_to));
                r.tags = norm_tags(&v.hashtags);
            }
        }
        "POST_EDIT" => {
            if let Ok(v) = pb::PostEdit::decode(p) {
                r.targets.extend(opt(v.post));
                r.tags = norm_tags(&v.hashtags);
            }
        }
        "POST_DELETE" => {
            if let Ok(v) = pb::PostDelete::decode(p) {
                r.targets.extend(opt(v.post));
            }
        }
        "COMMENT_CREATE" => {
            if let Ok(v) = pb::CommentCreate::decode(p) {
                r.targets.extend(opt(v.post));
                r.targets.extend(opt(v.parent_comment));
            }
        }
        "REACTION" => {
            if let Ok(v) = pb::Reaction::decode(p) {
                r.targets.extend(opt(v.target));
            }
        }
        "REPOST" => {
            if let Ok(v) = pb::Repost::decode(p) {
                r.targets.extend(opt(v.post));
            }
        }
        "REEL_CREATE" => {
            if let Ok(v) = pb::ReelCreate::decode(p) {
                r.tags = norm_tags(&v.hashtags);
            }
        }
        "CHANNEL_CREATE" => {
            // A wall is filed under itself so it can be listed before its
            // first post.
            r.channel = Some(ev.id.clone());
        }
        _ => {}
    }
    r.targets.sort();
    r.targets.dedup();
    r
}

// -- rate limiting -----------------------------------------------------------

/// A token bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: u64,
}

impl Bucket {
    fn take(&mut self, now: u64, burst: f64, per_sec: f64) -> bool {
        let dt = now.saturating_sub(self.last) as f64;
        self.tokens = (self.tokens + dt * per_sec).min(burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AuthorBuckets {
    /// Every event: 40 in a burst, 240 an hour sustained.
    any: Bucket,
    /// Posts, reels, stories, channels: 6 in a burst, one every 15 s
    /// sustained. Nobody writes faster; a script does.
    posts: Bucket,
    /// Everything over a day: 600.
    daily: Bucket,
}

const ANY_BURST: f64 = 40.0;
const ANY_PER_SEC: f64 = 240.0 / 3600.0;
const POST_BURST: f64 = 6.0;
const POST_PER_SEC: f64 = 1.0 / 15.0;
const DAILY_BURST: f64 = 600.0;
const DAILY_PER_SEC: f64 = 600.0 / 86_400.0;
const RATE_TABLE_CAP: usize = 100_000;

/// Per-author allowances. Applies to events that already proved their
/// author (signature + chain authority), so nobody can spend another
/// author's allowance.
#[derive(Debug, Default)]
pub struct RateLimiter {
    authors: Mutex<HashMap<String, AuthorBuckets>>,
}

/// Whether an event type creates top-level content (posts allowance) or is
/// engagement/metadata (general allowance only).
fn is_content(ty: &str) -> bool {
    matches!(
        ty,
        "POST_CREATE" | "REEL_CREATE" | "STORY_CREATE" | "CHANNEL_CREATE" | "REPOST"
    )
}

impl RateLimiter {
    /// Charges one event to `author` at `now`. `false` means over allowance.
    pub fn allow(&self, author: &str, ty: &str, now: u64) -> bool {
        let mut m = self.authors.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= RATE_TABLE_CAP && !m.contains_key(author) {
            // Drop authors idle for a day; if that frees nothing, start over.
            m.retain(|_, b| now.saturating_sub(b.any.last) < 86_400);
            if m.len() >= RATE_TABLE_CAP {
                m.clear();
            }
        }
        let b = m.entry(author.to_owned()).or_insert(AuthorBuckets {
            any: Bucket { tokens: ANY_BURST, last: now },
            posts: Bucket { tokens: POST_BURST, last: now },
            daily: Bucket { tokens: DAILY_BURST, last: now },
        });
        // Check the strictest applicable bucket first so a refusal does not
        // consume from the others.
        if is_content(ty) {
            let mut probe = b.posts;
            if !probe.take(now, POST_BURST, POST_PER_SEC) {
                return false;
            }
        }
        let mut probe_any = b.any;
        if !probe_any.take(now, ANY_BURST, ANY_PER_SEC) {
            return false;
        }
        let mut probe_daily = b.daily;
        if !probe_daily.take(now, DAILY_BURST, DAILY_PER_SEC) {
            return false;
        }
        b.any = probe_any;
        b.daily = probe_daily;
        if is_content(ty) {
            b.posts.take(now, POST_BURST, POST_PER_SEC);
        }
        true
    }
}

/// What the chain says about a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authz {
    /// An active device of the claimed author.
    Authorised,
    /// Not a device of the author, or revoked.
    Refused,
    /// The chain could not be asked.
    Unavailable,
}

/// The chain lookup a node uses to check a device against its author.
#[async_trait::async_trait]
pub trait DeviceAuthority: Send + Sync {
    /// Is `device_pubkey` an active device of `author`?
    async fn check(&self, author: &str, device_pubkey: &[u8]) -> Authz;
}

/// (author, device key) -> (answer, when).
type AuthzCache = HashMap<(String, Vec<u8>), (Authz, Instant)>;

/// A cache in front of a [`DeviceAuthority`].
pub struct CachedAuthority {
    inner: Arc<dyn DeviceAuthority>,
    cache: Mutex<AuthzCache>,
    ttl: Duration,
}

impl CachedAuthority {
    /// Wraps an authority with a positive/negative cache.
    pub fn new(inner: Arc<dyn DeviceAuthority>, ttl: Duration) -> Self {
        Self {
            inner,
            cache: Mutex::new(HashMap::new()),
            ttl,
        }
    }
}

#[async_trait::async_trait]
impl DeviceAuthority for CachedAuthority {
    async fn check(&self, author: &str, device_pubkey: &[u8]) -> Authz {
        let key = (author.to_owned(), device_pubkey.to_vec());
        {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((a, at)) = cache.get(&key) {
                if at.elapsed() < self.ttl {
                    return *a;
                }
            }
        }
        let a = self.inner.check(author, device_pubkey).await;
        if a != Authz::Unavailable {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.len() > 100_000 {
                cache.clear();
            }
            cache.insert(key, (a, Instant::now()));
        }
        a
    }
}

/// Statistics for the operator API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SocialStats {
    /// Events held.
    pub events: u64,
    /// Distinct authors.
    pub authors: u64,
}

/// A cached digest.
struct DigestCache {
    at: Instant,
    window_secs: u64,
    result: pb::SocialDigestResult,
}

/// How long a computed digest is served before being recomputed.
const DIGEST_TTL: Duration = Duration::from_secs(60);

/// The service.
pub struct SocialService {
    db: Database,
    network_id: String,
    authority: Arc<dyn DeviceAuthority>,
    retention_secs: u64,
    max_events: u64,
    rate: RateLimiter,
    digest_cache: Mutex<Option<DigestCache>>,
}

/// One page of a fetch.
#[derive(Debug, Default)]
pub struct FetchPage {
    /// The events.
    pub events: Vec<pb::SocialEvent>,
    /// Timeline mode: `before_timestamp` for the next page; 0 when done.
    pub next_before: u64,
}

fn err(code: &str, msg: impl Into<String>) -> pb::Response {
    pb::Response {
        body: Some(pb::response::Body::Error(pb::Error {
            code: code.to_owned(),
            message: msg.into(),
        })),
    }
}

/// Why an event was not accepted.
#[derive(Debug)]
pub enum Refusal {
    /// Structure, signature or authority: the sender is at fault.
    Invalid(String),
    /// Chain unavailable: nobody is at fault.
    Unavailable,
    /// Blocked by a trusted attestation.
    Blocked,
    /// Already held.
    Duplicate,
    /// The author is over its allowance.
    RateLimited,
    /// Storage.
    Db(anyhow::Error),
}

impl SocialService {
    /// Opens the log.
    pub fn open(
        db: Database,
        network_id: &str,
        authority: Arc<dyn DeviceAuthority>,
        retention_days: u32,
        max_events: u64,
    ) -> anyhow::Result<Arc<Self>> {
        let txn = db.begin_write().context("social: begin")?;
        let index_version = {
            txn.open_table(EVENTS)?;
            txn.open_table(BY_AUTHOR)?;
            txn.open_table(BY_TIME)?;
            txn.open_table(BY_TARGET)?;
            txn.open_table(BY_CHANNEL)?;
            txn.open_table(BY_TAG)?;
            let meta = txn.open_table(META)?;
            let v = meta.get("index_version")?.map(|v| v.value()).unwrap_or(0);
            v
        };
        txn.commit().context("social: init")?;
        let svc = Self {
            db,
            network_id: network_id.to_owned(),
            authority,
            retention_secs: u64::from(retention_days) * 86_400,
            max_events,
            rate: RateLimiter::default(),
            digest_cache: Mutex::new(None),
        };
        if index_version < INDEX_VERSION {
            let n = svc.reindex().context("social: reindex")?;
            info!(events = n, "social: derived indexes rebuilt");
        }
        Ok(Arc::new(svc))
    }

    /// Rebuilds the derived indexes from `EVENTS`. Idempotent.
    fn reindex(&self) -> anyhow::Result<u64> {
        let txn = self.db.begin_write()?;
        let mut n = 0u64;
        {
            let events = txn.open_table(EVENTS)?;
            let mut by_target = txn.open_table(BY_TARGET)?;
            let mut by_channel = txn.open_table(BY_CHANNEL)?;
            let mut by_tag = txn.open_table(BY_TAG)?;
            for r in events.iter()? {
                let (_, v) = r?;
                let Ok(ev) = pb::SocialEvent::decode(v.value()) else {
                    continue;
                };
                index_refs(&ev, &mut by_target, &mut by_channel, &mut by_tag)?;
                n += 1;
            }
            let mut meta = txn.open_table(META)?;
            meta.insert("index_version", INDEX_VERSION)?;
        }
        txn.commit()?;
        Ok(n)
    }

    /// Whether an event id is already held.
    fn held(&self, id: &[u8]) -> anyhow::Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(EVENTS)?.get(id)?.is_some())
    }

    /// Topics to subscribe: every shard. Sharding exists for clients that
    /// want a slice; a node relays all of them.
    #[must_use]
    pub fn topics(&self) -> Vec<String> {
        topics::all_social_shards(&self.network_id)
    }

    /// Full acceptance pipeline. `shared` supplies identity and safety.
    pub async fn accept(&self, shared: &Shared, ev: &pb::SocialEvent) -> Result<(), Refusal> {
        validate::social_event(ev, now()).map_err(|e| Refusal::Invalid(e.to_string()))?;
        signing::verify_social_event(&shared.identity, ev)
            .map_err(|e| Refusal::Invalid(e.to_string()))?;
        match self.authority.check(&ev.author, &ev.device_pubkey).await {
            Authz::Authorised => {}
            Authz::Refused => {
                return Err(Refusal::Invalid(format!(
                    "device {} is not an active device of {}",
                    hex::encode(&ev.device_pubkey),
                    ev.author
                )))
            }
            Authz::Unavailable => return Err(Refusal::Unavailable),
        }
        // Dedup before charging the allowance: a re-publish of something we
        // hold is not activity.
        if self.held(&ev.id).map_err(Refusal::Db)? {
            return Err(Refusal::Duplicate);
        }
        if !self.rate.allow(&ev.author, &ev.r#type, now()) {
            return Err(Refusal::RateLimited);
        }
        if let Some(s) = &shared.services.safety {
            if s.is_blocked_event(&ev.id) {
                return Err(Refusal::Blocked);
            }
            for m in &ev.media {
                if s.is_blocked_cid(&m.cid) {
                    return Err(Refusal::Blocked);
                }
            }
        }
        self.store(ev)
            .map_err(Refusal::Db)?
            .then_some(())
            .ok_or(Refusal::Duplicate)
    }

    /// Gossip entry point.
    pub async fn accept_gossip(
        &self,
        shared: &Arc<Shared>,
        topic: &str,
        source: PeerId,
        ev: pb::SocialEvent,
    ) -> MessageAcceptance {
        // An event must travel on its author's shard, or a peer could
        // flood a quiet shard with a busy author's traffic.
        if topic != topics::social_shard(&self.network_id, &ev.author) {
            shared
                .handle
                .score(source, ScoreEvent::MalformedFrame)
                .await;
            return MessageAcceptance::Reject;
        }
        match self.accept(shared, &ev).await {
            Ok(()) => MessageAcceptance::Accept,
            Err(Refusal::Duplicate) => MessageAcceptance::Ignore,
            Err(Refusal::Unavailable) => MessageAcceptance::Ignore,
            Err(Refusal::Blocked) => MessageAcceptance::Ignore,
            // The author is flooding, not the peer that relayed it: keep
            // the event out of the log and out of the mesh without scoring.
            Err(Refusal::RateLimited) => MessageAcceptance::Ignore,
            Err(Refusal::Invalid(why)) => {
                debug!(%source, why, "social event refused");
                MessageAcceptance::Reject
            }
            Err(Refusal::Db(e)) => {
                warn!(error = %e, "social store failed");
                MessageAcceptance::Ignore
            }
        }
    }

    /// Handles a request from a verified peer or client.
    pub async fn handle(
        &self,
        shared: &Arc<Shared>,
        peer: PeerId,
        body: pb::request::Body,
    ) -> pb::Response {
        use pb::request::Body as B;
        match body {
            B::EventPublish(p) => {
                let Some(ev) = p.event else {
                    return err("invalid", "no event");
                };
                match self.accept(shared, &ev).await {
                    Ok(()) | Err(Refusal::Duplicate) => {
                        let topic = topics::social_shard(&self.network_id, &ev.author);
                        let g = pb::Gossip {
                            body: Some(pb::gossip::Body::SocialEvent(ev)),
                        };
                        if let Err(e) = shared.handle.publish(&topic, g.encode_to_vec()).await {
                            debug!(error = e, "event stored but not gossiped yet");
                        }
                        pb::Response {
                            body: Some(pb::response::Body::EventPublish(pb::EventPublishResult {
                                accepted: true,
                                reason: String::new(),
                            })),
                        }
                    }
                    Err(Refusal::Invalid(why)) => {
                        shared
                            .handle
                            .score(peer, ScoreEvent::InvalidSignature)
                            .await;
                        err("invalid", why)
                    }
                    Err(Refusal::Unavailable) => err("internal", "chain unavailable; try again"),
                    Err(Refusal::Blocked) => err(
                        "invalid",
                        "content is blocked by a trusted safety attestation",
                    ),
                    Err(Refusal::RateLimited) => err(
                        "rate_limited",
                        "this identity is posting faster than the network allows; wait a little",
                    ),
                    Err(Refusal::Db(e)) => {
                        warn!(error = %e, "social store failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::EventFetch(f) => match self.fetch_page(&f, shared.services.safety.as_deref()) {
                Ok(page) => pb::Response {
                    body: Some(pb::response::Body::EventFetch(pb::EventFetchResult {
                        events: page.events,
                        next_before: page.next_before,
                    })),
                },
                Err(e) => {
                    warn!(error = %e, "event fetch failed");
                    err("internal", "storage error")
                }
            },
            B::SocialDigest(d) => match self.digest(&d, shared.services.safety.as_deref()) {
                Ok(result) => pb::Response {
                    body: Some(pb::response::Body::SocialDigest(result)),
                },
                Err(e) => {
                    warn!(error = %e, "social digest failed");
                    err("internal", "storage error")
                }
            },
            _ => err("invalid", "not a social request"),
        }
    }

    // -- storage --------------------------------------------------------------

    /// Stores an already-verified event. Returns false if already held.
    pub fn store(&self, ev: &pb::SocialEvent) -> anyhow::Result<bool> {
        let txn = self.db.begin_write()?;
        {
            let mut events = txn.open_table(EVENTS)?;
            if events.get(ev.id.as_slice())?.is_some() {
                return Ok(false);
            }
            events.insert(ev.id.as_slice(), ev.encode_to_vec().as_slice())?;
            let mut by_author = txn.open_table(BY_AUTHOR)?;
            by_author.insert((ev.author.as_str(), ev.sequence, ev.id.as_slice()), ())?;
            let mut by_time = txn.open_table(BY_TIME)?;
            by_time.insert((ev.timestamp, ev.id.as_slice()), ())?;
            let mut by_target = txn.open_table(BY_TARGET)?;
            let mut by_channel = txn.open_table(BY_CHANNEL)?;
            let mut by_tag = txn.open_table(BY_TAG)?;
            index_refs(ev, &mut by_target, &mut by_channel, &mut by_tag)?;
        }
        txn.commit()?;
        Ok(true)
    }

    /// Fetches by ids, target, author or timeline; see [`pb::EventFetch`].
    /// The events only; use [`Self::fetch_page`] for the paging cursor.
    pub fn fetch(
        &self,
        f: &pb::EventFetch,
        safety: Option<&crate::safety::SafetyService>,
    ) -> anyhow::Result<Vec<pb::SocialEvent>> {
        Ok(self.fetch_page(f, safety)?.events)
    }

    /// Fetches one page. See [`pb::EventFetch`] for the modes.
    pub fn fetch_page(
        &self,
        f: &pb::EventFetch,
        safety: Option<&crate::safety::SafetyService>,
    ) -> anyhow::Result<FetchPage> {
        let limit = if f.limit == 0 {
            MAX_EVENT_PAGE
        } else {
            f.limit.min(MAX_EVENT_PAGE)
        } as usize;
        let txn = self.db.begin_read()?;
        let events = txn.open_table(EVENTS)?;
        let filter = Filter::from(f);
        let blocked = |id: &[u8]| safety.is_some_and(|s| s.is_blocked_event(id));
        let mut page = FetchPage::default();

        // Loads, filters and appends one event; returns whether the page is
        // full. Shared by every index walk below.
        let mut scanned = 0usize;
        let mut last_ts = 0u64;
        let mut push = |id: &[u8], ts: u64, out: &mut Vec<pb::SocialEvent>| -> anyhow::Result<bool> {
            scanned += 1;
            last_ts = ts;
            if blocked(id) {
                return Ok(out.len() >= limit);
            }
            if let Some(v) = events.get(id)? {
                if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                    if filter.passes(&ev) {
                        out.push(ev);
                    }
                }
            }
            Ok(out.len() >= limit || scanned >= MAX_TIMELINE_SCAN)
        };

        if !f.ids.is_empty() {
            for id in f.ids.iter().take(limit) {
                if push(id, 0, &mut page.events)? {
                    break;
                }
            }
            return Ok(page);
        }
        if !f.target.is_empty() {
            let by_target = txn.open_table(BY_TARGET)?;
            let start = (f.target.as_slice(), 0u64, [].as_slice());
            let end = (f.target.as_slice(), u64::MAX, MAX_ID.as_slice());
            for r in by_target.range(start..=end)? {
                let (k, _) = r?;
                if push(k.value().2, k.value().1, &mut page.events)? {
                    break;
                }
            }
            return Ok(page);
        }
        if !f.author.is_empty() {
            let by_author = txn.open_table(BY_AUTHOR)?;
            let start = (f.author.as_str(), f.from_sequence, [].as_slice());
            let end = (f.author.as_str(), u64::MAX, MAX_ID.as_slice());
            let range = by_author.range(start..=end)?;
            if f.latest {
                for r in range.rev() {
                    let (k, _) = r?;
                    if push(k.value().2, 0, &mut page.events)? {
                        break;
                    }
                }
            } else {
                for r in range {
                    let (k, _) = r?;
                    if push(k.value().2, 0, &mut page.events)? {
                        break;
                    }
                }
            }
            return Ok(page);
        }

        // Timeline: newest first, strictly before `before`. Walk the most
        // selective index available.
        let before = if f.before_timestamp == 0 {
            u64::MAX
        } else {
            f.before_timestamp
        };
        if let Some(ch) = filter.channel.as_deref() {
            let by_channel = txn.open_table(BY_CHANNEL)?;
            let start = (ch, 0u64, [].as_slice());
            let end = (ch, before.saturating_sub(1), MAX_ID.as_slice());
            for r in by_channel.range(start..=end)?.rev() {
                let (k, _) = r?;
                if push(k.value().2, k.value().1, &mut page.events)? {
                    break;
                }
            }
        } else if let Some(tag) = filter.hashtag.as_deref() {
            let by_tag = txn.open_table(BY_TAG)?;
            let start = (tag, 0u64, [].as_slice());
            let end = (tag, before.saturating_sub(1), MAX_ID.as_slice());
            for r in by_tag.range(start..=end)?.rev() {
                let (k, _) = r?;
                if push(k.value().2, k.value().1, &mut page.events)? {
                    break;
                }
            }
        } else {
            let by_time = txn.open_table(BY_TIME)?;
            for r in by_time.range(..(before, [].as_slice()))?.rev() {
                let (k, _) = r?;
                if push(k.value().1, k.value().0, &mut page.events)? {
                    break;
                }
            }
        }
        page.next_before = next_before(&page.events, limit, scanned, last_ts);
        Ok(page)
    }

    // -- digest ---------------------------------------------------------------

    /// What is active in the log over a window. Cached for [`DIGEST_TTL`].
    pub fn digest(
        &self,
        d: &pb::SocialDigest,
        safety: Option<&crate::safety::SafetyService>,
    ) -> anyhow::Result<pb::SocialDigestResult> {
        let window = if d.window_secs == 0 {
            DEFAULT_DIGEST_WINDOW_SECS
        } else {
            d.window_secs.clamp(3600, MAX_DIGEST_WINDOW_SECS)
        };
        let limit = if d.limit == 0 {
            20
        } else {
            d.limit.min(MAX_DIGEST_ENTRIES)
        } as usize;
        {
            let cache = self.digest_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(c) = cache.as_ref() {
                if c.window_secs == window && c.at.elapsed() < DIGEST_TTL {
                    return Ok(truncate_digest(c.result.clone(), limit));
                }
            }
        }
        let full = self.compute_digest(window, safety)?;
        let mut cache = self.digest_cache.lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some(DigestCache {
            at: Instant::now(),
            window_secs: window,
            result: full.clone(),
        });
        Ok(truncate_digest(full, limit))
    }

    fn compute_digest(
        &self,
        window: u64,
        safety: Option<&crate::safety::SafetyService>,
    ) -> anyhow::Result<pb::SocialDigestResult> {
        #[derive(Default)]
        struct A {
            posts: u32,
            comments: u32,
            reactions_received: u32,
            comments_received: u32,
            last_active: u64,
        }
        #[derive(Default)]
        struct T {
            posts: u32,
            authors: HashSet<String>,
            last_used: u64,
        }
        let now_ts = now();
        let since = now_ts.saturating_sub(window);
        let blocked = |id: &[u8]| safety.is_some_and(|s| s.is_blocked_event(id));
        let txn = self.db.begin_read()?;
        let events = txn.open_table(EVENTS)?;
        let by_time = txn.open_table(BY_TIME)?;

        let mut authors: HashMap<String, A> = HashMap::new();
        let mut tags: HashMap<String, T> = HashMap::new();
        let mut window_events = 0u64;
        // Author of a target, memoised: reactions and comments credit the
        // author of what they point at.
        let mut target_author: HashMap<Vec<u8>, Option<String>> = HashMap::new();
        let mut author_of = |id: &[u8]| -> anyhow::Result<Option<String>> {
            if let Some(a) = target_author.get(id) {
                return Ok(a.clone());
            }
            let a = match events.get(id)? {
                Some(v) => pb::SocialEvent::decode(v.value()).ok().map(|e| e.author),
                None => None,
            };
            if target_author.len() < MAX_DIGEST_SCAN {
                target_author.insert(id.to_vec(), a.clone());
            }
            Ok(a)
        };

        for r in by_time
            .range((since, [].as_slice())..)?
            .rev()
            .take(MAX_DIGEST_SCAN)
        {
            let (k, _) = r?;
            let id = k.value().1;
            if blocked(id) {
                continue;
            }
            let Some(v) = events.get(id)? else { continue };
            let Ok(ev) = pb::SocialEvent::decode(v.value()) else {
                continue;
            };
            window_events += 1;
            let a = authors.entry(ev.author.clone()).or_default();
            a.last_active = a.last_active.max(ev.timestamp);
            let refs = refs_of(&ev);
            match ev.r#type.as_str() {
                "POST_CREATE" | "REEL_CREATE" => {
                    a.posts += 1;
                    for t in &refs.tags {
                        let e = tags.entry(t.clone()).or_default();
                        e.posts += 1;
                        e.authors.insert(ev.author.clone());
                        e.last_used = e.last_used.max(ev.timestamp);
                    }
                }
                "COMMENT_CREATE" => {
                    a.comments += 1;
                    if let Some(target) = refs.targets.first() {
                        if let Some(ta) = author_of(target)? {
                            if ta != ev.author {
                                authors.entry(ta).or_default().comments_received += 1;
                            }
                        }
                    }
                }
                "REACTION" => {
                    if let Some(target) = refs.targets.first() {
                        if let Some(ta) = author_of(target)? {
                            if ta != ev.author {
                                authors.entry(ta).or_default().reactions_received += 1;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Channels: every wall the node knows, with its activity.
        struct C {
            meta: Option<pb::SocialEvent>,
            posts: u32,
            authors: HashSet<String>,
            last_post: u64,
        }
        let by_channel = txn.open_table(BY_CHANNEL)?;
        let mut channels: BTreeMap<Vec<u8>, C> = BTreeMap::new();
        for r in by_channel.iter()?.take(MAX_DIGEST_SCAN * 5) {
            let (k, _) = r?;
            let (ch, ts, id) = k.value();
            if blocked(id) {
                continue;
            }
            let c = channels.entry(ch.to_vec()).or_insert_with(|| C {
                meta: None,
                posts: 0,
                authors: HashSet::new(),
                last_post: 0,
            });
            if ch == id {
                if let Some(v) = events.get(id)? {
                    c.meta = pb::SocialEvent::decode(v.value()).ok();
                }
                continue;
            }
            // Only POST_CREATE is filed under a channel besides its
            // creation, so a row is a post; the author needs a decode.
            if let Some(v) = events.get(id)? {
                if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                    c.posts += 1;
                    c.authors.insert(ev.author);
                    c.last_post = c.last_post.max(ts);
                }
            }
        }
        let mut channel_list: Vec<pb::ChannelActivity> = channels
            .into_iter()
            .filter_map(|(id, c)| {
                let meta = c.meta?;
                let cc = pb::ChannelCreate::decode(meta.payload.as_slice()).ok()?;
                Some(pb::ChannelActivity {
                    id,
                    name: cc.name,
                    description: cc.description,
                    creator: meta.author,
                    open_posting: cc.open_posting,
                    created_at: meta.timestamp,
                    posts: c.posts,
                    authors: c.authors.len() as u32,
                    last_post: c.last_post,
                })
            })
            .collect();
        channel_list.sort_by(|a, b| {
            b.last_post
                .max(b.created_at)
                .cmp(&a.last_post.max(a.created_at))
                .then_with(|| b.posts.cmp(&a.posts))
        });

        let mut top_authors: Vec<pb::AuthorActivity> = authors
            .into_iter()
            .filter(|(_, a)| a.posts + a.comments + a.reactions_received + a.comments_received > 0)
            .map(|(author, a)| pb::AuthorActivity {
                author,
                posts: a.posts,
                comments: a.comments,
                reactions_received: a.reactions_received,
                comments_received: a.comments_received,
                last_active: a.last_active,
            })
            .collect();
        let window_authors = top_authors.len() as u64;
        top_authors.sort_by(|a, b| {
            (b.posts + b.comments)
                .cmp(&(a.posts + a.comments))
                .then_with(|| {
                    (b.reactions_received + b.comments_received)
                        .cmp(&(a.reactions_received + a.comments_received))
                })
                .then_with(|| b.last_active.cmp(&a.last_active))
        });
        let mut top_hashtags: Vec<pb::HashtagActivity> = tags
            .into_iter()
            .map(|(tag, t)| pb::HashtagActivity {
                tag,
                posts: t.posts,
                authors: t.authors.len() as u32,
                last_used: t.last_used,
            })
            .collect();
        top_hashtags.sort_by(|a, b| {
            b.posts
                .cmp(&a.posts)
                .then_with(|| b.authors.cmp(&a.authors))
                .then_with(|| b.last_used.cmp(&a.last_used))
        });
        let totals = self.stats()?;
        Ok(pb::SocialDigestResult {
            window_secs: window,
            events: window_events,
            authors: window_authors,
            total_events: totals.events,
            total_authors: totals.authors,
            top_authors: top_authors.into_iter().take(MAX_DIGEST_ENTRIES as usize).collect(),
            top_hashtags: top_hashtags
                .into_iter()
                .take(MAX_DIGEST_ENTRIES as usize)
                .collect(),
            channels: channel_list
                .into_iter()
                .take(MAX_DIGEST_ENTRIES as usize)
                .collect(),
            computed_at: now_ts,
        })
    }

    /// Events since a timestamp, oldest first, for indexers. `after` is
    /// `(timestamp, id)` of the last one seen.
    pub fn since(
        &self,
        after: Option<(u64, Vec<u8>)>,
        limit: usize,
    ) -> anyhow::Result<Vec<pb::SocialEvent>> {
        let txn = self.db.begin_read()?;
        let events = txn.open_table(EVENTS)?;
        let by_time = txn.open_table(BY_TIME)?;
        let (ts, id) = after.unwrap_or((0, Vec::new()));
        let mut out = Vec::new();
        for r in by_time.range((ts, id.as_slice())..)? {
            let (k, _) = r?;
            let (kts, kid) = k.value();
            if kts == ts && kid == id.as_slice() {
                continue;
            }
            if let Some(v) = events.get(kid)? {
                if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                    out.push(ev);
                }
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Drops events past retention or beyond the cap, oldest first.
    pub fn sweep(&self) -> anyhow::Result<u64> {
        let cutoff = now().saturating_sub(self.retention_secs);
        let txn = self.db.begin_write()?;
        let mut removed = 0u64;
        {
            let mut events = txn.open_table(EVENTS)?;
            let mut by_time = txn.open_table(BY_TIME)?;
            let mut by_author = txn.open_table(BY_AUTHOR)?;
            let mut by_target = txn.open_table(BY_TARGET)?;
            let mut by_channel = txn.open_table(BY_CHANNEL)?;
            let mut by_tag = txn.open_table(BY_TAG)?;
            let total = events.len()?;
            let excess = total.saturating_sub(self.max_events);
            let victims: Vec<(u64, Vec<u8>)> = by_time
                .iter()?
                .filter_map(|r| r.ok())
                .map(|(k, _)| {
                    let (t, id) = k.value();
                    (t, id.to_vec())
                })
                .enumerate()
                .take_while(|(i, (t, _))| *t < cutoff || (*i as u64) < excess)
                .map(|(_, v)| v)
                .collect();
            for (t, id) in victims {
                by_time.remove((t, id.as_slice()))?;
                if let Some(v) = events.remove(id.as_slice())? {
                    if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                        by_author.remove((ev.author.as_str(), ev.sequence, id.as_slice()))?;
                        let refs = refs_of(&ev);
                        for tg in &refs.targets {
                            by_target.remove((tg.as_slice(), ev.timestamp, id.as_slice()))?;
                        }
                        if let Some(ch) = &refs.channel {
                            by_channel.remove((ch.as_slice(), ev.timestamp, id.as_slice()))?;
                        }
                        for tag in &refs.tags {
                            by_tag.remove((tag.as_str(), ev.timestamp, id.as_slice()))?;
                        }
                    }
                }
                removed += 1;
            }
        }
        txn.commit()?;
        Ok(removed)
    }

    /// Statistics.
    pub fn stats(&self) -> anyhow::Result<SocialStats> {
        let txn = self.db.begin_read()?;
        let events = txn.open_table(EVENTS)?.len()?;
        let by_author = txn.open_table(BY_AUTHOR)?;
        let mut authors = 0u64;
        let mut last: Option<String> = None;
        for r in by_author.iter()? {
            let (k, _) = r?;
            let a = k.value().0;
            if last.as_deref() != Some(a) {
                authors += 1;
                last = Some(a.to_owned());
            }
        }
        Ok(SocialStats { events, authors })
    }
}

/// The narrowing part of a fetch.
struct Filter {
    types: Vec<String>,
    hashtag: Option<String>,
    channel: Option<Vec<u8>>,
}

impl From<&pb::EventFetch> for Filter {
    fn from(f: &pb::EventFetch) -> Self {
        let hashtag = Some(normalize_tag(&f.hashtag)).filter(|t| !t.is_empty());
        Self {
            types: f.types.clone(),
            hashtag,
            channel: opt(f.channel.clone()),
        }
    }
}

impl Filter {
    fn passes(&self, ev: &pb::SocialEvent) -> bool {
        if !self.types.is_empty() && !self.types.iter().any(|t| t == &ev.r#type) {
            return false;
        }
        if self.hashtag.is_none() && self.channel.is_none() {
            return true;
        }
        let refs = refs_of(ev);
        if let Some(t) = &self.hashtag {
            if !refs.tags.iter().any(|x| x == t) {
                return false;
            }
        }
        if let Some(c) = &self.channel {
            // A wall's own creation event is filed under it; it is not a post.
            if refs.channel.as_deref() != Some(c.as_slice()) || ev.id == *c {
                return false;
            }
        }
        true
    }
}

/// Files an event's references into the derived indexes.
fn index_refs(
    ev: &pb::SocialEvent,
    by_target: &mut redb::Table<'_, (&[u8], u64, &[u8]), ()>,
    by_channel: &mut redb::Table<'_, (&[u8], u64, &[u8]), ()>,
    by_tag: &mut redb::Table<'_, (&str, u64, &[u8]), ()>,
) -> anyhow::Result<()> {
    let refs = refs_of(ev);
    for t in &refs.targets {
        by_target.insert((t.as_slice(), ev.timestamp, ev.id.as_slice()), ())?;
    }
    if let Some(ch) = &refs.channel {
        by_channel.insert((ch.as_slice(), ev.timestamp, ev.id.as_slice()), ())?;
    }
    for tag in &refs.tags {
        by_tag.insert((tag.as_str(), ev.timestamp, ev.id.as_slice()), ())?;
    }
    Ok(())
}

/// The cursor for the page after `events` (newest first). 0 when the walk
/// ended because the index ran out. When a page is full and its events do
/// not all share one second, the cursor is `last + 1` so events sharing
/// that second are not skipped (the client drops the ones it already has);
/// when they all share it, the cursor steps past it so paging always makes
/// progress.
fn next_before(events: &[pb::SocialEvent], limit: usize, scanned: usize, last_ts: u64) -> u64 {
    if events.len() < limit {
        // The walk ended early: either the index ran out (done) or the scan
        // cap stopped it (continue from the last row looked at).
        return if scanned >= MAX_TIMELINE_SCAN {
            last_ts
        } else {
            0
        };
    }
    let (Some(first), Some(last)) = (events.first(), events.last()) else {
        return 0;
    };
    if first.timestamp == last.timestamp {
        last.timestamp
    } else {
        last.timestamp.saturating_add(1)
    }
}

fn truncate_digest(mut d: pb::SocialDigestResult, limit: usize) -> pb::SocialDigestResult {
    d.top_authors.truncate(limit);
    d.top_hashtags.truncate(limit);
    d.channels.truncate(limit);
    d
}

/// An authority that accepts everything. DEVNET ONLY: used by tests and by
/// a node started with `--insecure-no-chain`, which refuses to run on
/// mainnet.
pub struct PermissiveAuthority;

#[async_trait::async_trait]
impl DeviceAuthority for PermissiveAuthority {
    async fn check(&self, _: &str, _: &[u8]) -> Authz {
        Authz::Authorised
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(authority: Arc<dyn DeviceAuthority>) -> Arc<SocialService> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hg-social-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        SocialService::open(
            crate::store::open(&dir, "social").unwrap(),
            "hashgram-devnet",
            authority,
            90,
            1_000_000,
        )
        .unwrap()
    }

    fn ev(author: &str, seq: u64, ts: u64) -> pb::SocialEvent {
        pb::SocialEvent {
            network_id: "hashgram-devnet".into(),
            version: 1,
            id: blake3::hash(format!("{author}{seq}{ts}").as_bytes())
                .as_bytes()
                .to_vec(),
            r#type: "POST_CREATE".into(),
            author: author.into(),
            device_pubkey: vec![1; 32],
            timestamp: ts,
            sequence: seq,
            payload: vec![],
            signature: vec![0; 64],
            ..Default::default()
        }
    }

    #[test]
    fn stores_dedups_and_fetches_by_author_and_id() {
        let s = open(Arc::new(PermissiveAuthority));
        let a = "hash1alice";
        assert!(s.store(&ev(a, 0, 100)).unwrap());
        assert!(s.store(&ev(a, 1, 101)).unwrap());
        assert!(!s.store(&ev(a, 1, 101)).unwrap());
        assert!(s.store(&ev("hash1bob", 0, 102)).unwrap());
        let got = s
            .fetch(
                &pb::EventFetch {
                    author: a.into(),
                    from_sequence: 1,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].sequence, 1);
        let by_id = s
            .fetch(
                &pb::EventFetch {
                    ids: vec![ev("hash1bob", 0, 102).id],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(by_id.len(), 1);
        let stats = s.stats().unwrap();
        assert_eq!((stats.events, stats.authors), (3, 2));
        let since = s.since(Some((100, ev(a, 0, 100).id)), 10).unwrap();
        assert_eq!(since.len(), 2);
    }

    #[test]
    fn sweep_enforces_the_cap_oldest_first() {
        let s = open(Arc::new(PermissiveAuthority));
        let svc = SocialService {
            db: crate::store::open(
                &std::env::temp_dir().join(format!("hg-social-cap-{}", std::process::id())),
                "social-cap",
            )
            .unwrap(),
            network_id: "hashgram-devnet".into(),
            authority: Arc::new(PermissiveAuthority),
            retention_secs: u64::MAX >> 1,
            max_events: 2,
            rate: RateLimiter::default(),
            digest_cache: Mutex::new(None),
        };
        drop(s);
        let txn = svc.db.begin_write().unwrap();
        {
            txn.open_table(EVENTS).unwrap();
            txn.open_table(BY_AUTHOR).unwrap();
            txn.open_table(BY_TIME).unwrap();
            txn.open_table(BY_TARGET).unwrap();
            txn.open_table(BY_CHANNEL).unwrap();
            txn.open_table(BY_TAG).unwrap();
        }
        txn.commit().unwrap();
        let t = now();
        for i in 0..5u64 {
            svc.store(&ev("hash1a", i, t - 100 + i)).unwrap();
        }
        assert_eq!(svc.sweep().unwrap(), 3);
        let left = svc.since(None, 10).unwrap();
        assert_eq!(left.len(), 2);
        assert_eq!(left[0].sequence, 3);
    }

    fn typed(author: &str, seq: u64, ts: u64, ty: &str, payload: Vec<u8>) -> pb::SocialEvent {
        let mut e = ev(author, seq, ts);
        e.r#type = ty.into();
        e.payload = payload;
        e.id = blake3::hash(format!("{author}{seq}{ts}{ty}").as_bytes())
            .as_bytes()
            .to_vec();
        e
    }

    fn post(author: &str, seq: u64, ts: u64, text: &str, tags: &[&str], channel: &[u8]) -> pb::SocialEvent {
        typed(
            author,
            seq,
            ts,
            "POST_CREATE",
            pb::PostCreate {
                text: text.into(),
                hashtags: tags.iter().map(|t| (*t).to_owned()).collect(),
                channel: channel.to_vec(),
                ..Default::default()
            }
            .encode_to_vec(),
        )
    }

    #[test]
    fn timeline_pages_newest_first_with_filters() {
        let s = open(Arc::new(PermissiveAuthority));
        let wall = typed(
            "hash1carol",
            0,
            1_000,
            "CHANNEL_CREATE",
            pb::ChannelCreate {
                name: "Protests".into(),
                description: "Open wall".into(),
                open_posting: true,
                ..Default::default()
            }
            .encode_to_vec(),
        );
        assert!(s.store(&wall).unwrap());
        for i in 0..30u64 {
            let tags: &[&str] = if i % 3 == 0 { &["#Georgia", "news"] } else { &["news"] };
            let ch: &[u8] = if i % 5 == 0 { &wall.id } else { &[] };
            s.store(&post("hash1alice", i, 2_000 + i, &format!("post {i}"), tags, ch))
                .unwrap();
        }
        // Follows are not posts and must be filtered out by `types`.
        s.store(&typed(
            "hash1bob",
            0,
            2_500,
            "FOLLOW",
            pb::Follow { target: "hash1alice".into() }.encode_to_vec(),
        ))
        .unwrap();

        let p1 = s
            .fetch_page(
                &pb::EventFetch {
                    limit: 10,
                    types: vec!["POST_CREATE".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(p1.events.len(), 10);
        assert_eq!(p1.events[0].timestamp, 2_029, "newest first");
        assert!(p1.events.iter().all(|e| e.r#type == "POST_CREATE"));
        assert_eq!(p1.next_before, 2_020 + 1);
        let p2 = s
            .fetch_page(
                &pb::EventFetch {
                    limit: 10,
                    before_timestamp: p1.next_before,
                    types: vec!["POST_CREATE".into()],
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        // The +1 cursor re-serves the boundary second; the client dedups.
        assert_eq!(p2.events[0].timestamp, 2_020);
        let mut all: Vec<u64> = p1.events.iter().chain(&p2.events).map(|e| e.timestamp).collect();
        all.dedup();
        assert_eq!(all.len(), 19);

        let tagged = s
            .fetch_page(
                &pb::EventFetch {
                    hashtag: "#GEORGIA".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(tagged.events.len(), 10);
        assert_eq!(tagged.next_before, 0, "exhausted");

        let on_wall = s
            .fetch_page(
                &pb::EventFetch {
                    channel: wall.id.clone(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(on_wall.events.len(), 6);
        assert!(on_wall.events.iter().all(|e| e.r#type == "POST_CREATE"), "the wall's own creation is not a post");

        let latest = s
            .fetch_page(
                &pb::EventFetch {
                    author: "hash1alice".into(),
                    latest: true,
                    limit: 3,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        assert_eq!(latest.events.iter().map(|e| e.sequence).collect::<Vec<_>>(), vec![29, 28, 27]);
    }

    #[test]
    fn target_index_and_digest() {
        let s = open(Arc::new(PermissiveAuthority));
        let t = now();
        let p = post("hash1alice", 0, t - 500, "hello", &["Hello"], &[]);
        s.store(&p).unwrap();
        for i in 0..3u64 {
            s.store(&typed(
                "hash1bob",
                i,
                t - 400 + i,
                "COMMENT_CREATE",
                pb::CommentCreate { post: p.id.clone(), text: "hi".into(), ..Default::default() }.encode_to_vec(),
            ))
            .unwrap();
        }
        s.store(&typed(
            "hash1carol",
            0,
            t - 300,
            "REACTION",
            pb::Reaction { target: p.id.clone(), reaction: "❤".into() }.encode_to_vec(),
        ))
        .unwrap();
        // Old: outside a one-day window.
        s.store(&post("hash1old", 0, t - 10 * 86_400, "ancient", &[], &[])).unwrap();

        let under = s
            .fetch_page(&pb::EventFetch { target: p.id.clone(), ..Default::default() }, None)
            .unwrap();
        assert_eq!(under.events.len(), 4);
        assert_eq!(under.events[0].r#type, "COMMENT_CREATE", "oldest first");
        assert_eq!(under.events[3].r#type, "REACTION");

        let d = s
            .digest(&pb::SocialDigest { window_secs: 86_400, limit: 10 }, None)
            .unwrap();
        assert_eq!(d.events, 5);
        assert_eq!(d.total_events, 6);
        assert_eq!(d.total_authors, 4);
        let alice = d.top_authors.iter().find(|a| a.author == "hash1alice").unwrap();
        assert_eq!((alice.posts, alice.comments_received, alice.reactions_received), (1, 3, 1));
        let bob = d.top_authors.iter().find(|a| a.author == "hash1bob").unwrap();
        assert_eq!(bob.comments, 3);
        assert_eq!(d.top_authors[0].author, "hash1bob", "most active first");
        assert_eq!(d.top_hashtags[0].tag, "hello");
        assert!(d.top_authors.iter().all(|a| a.author != "hash1old"));
    }

    #[test]
    fn reopen_backfills_derived_indexes() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hg-social-reindex-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let db = crate::store::open(&dir, "social").unwrap();
        // An "old" store: only the original three tables, one tagged post.
        let p = post("hash1alice", 0, 10, "x", &["tag"], &[]);
        {
            let txn = db.begin_write().unwrap();
            {
                let mut events = txn.open_table(EVENTS).unwrap();
                events.insert(p.id.as_slice(), p.encode_to_vec().as_slice()).unwrap();
                txn.open_table(BY_AUTHOR).unwrap().insert(("hash1alice", 0u64, p.id.as_slice()), ()).unwrap();
                txn.open_table(BY_TIME).unwrap().insert((10u64, p.id.as_slice()), ()).unwrap();
            }
            txn.commit().unwrap();
        }
        let s = SocialService::open(db, "hashgram-devnet", Arc::new(PermissiveAuthority), 90, 1_000).unwrap();
        let by_tag = s
            .fetch_page(&pb::EventFetch { hashtag: "tag".into(), ..Default::default() }, None)
            .unwrap();
        assert_eq!(by_tag.events.len(), 1);
    }

    #[test]
    fn rate_limiter_stops_floods_but_not_people() {
        let r = RateLimiter::default();
        let t = 1_000_000u64;
        // A person: a post every few minutes all day is fine.
        for i in 0..100u64 {
            assert!(r.allow("hash1alice", "POST_CREATE", t + i * 300), "post {i}");
        }
        // A script: six posts in one second are allowed, the seventh is not.
        for _ in 0..6 {
            assert!(r.allow("hash1bot", "POST_CREATE", t));
        }
        assert!(!r.allow("hash1bot", "POST_CREATE", t));
        // Reactions do not draw on the posts allowance...
        assert!(r.allow("hash1bot", "REACTION", t));
        // ...but the general one runs out too.
        let mut ok = 0;
        for _ in 0..100 {
            if r.allow("hash1bot", "REACTION", t) {
                ok += 1;
            }
        }
        assert!(ok < 40, "{ok}");
        // Time heals: fifteen seconds later one more post is fine.
        assert!(r.allow("hash1bot", "POST_CREATE", t + 15));
        assert!(!r.allow("hash1bot", "POST_CREATE", t + 15));
        // Other authors are unaffected.
        assert!(r.allow("hash1carol", "POST_CREATE", t));
    }

    #[test]
    fn refs_are_derived_from_payloads() {
        let p = post("hash1a", 0, 1, "t", &["#Big", "big", "small"], b"cc");
        let r = refs_of(&p);
        assert_eq!(r.tags, vec!["big", "small"]);
        assert_eq!(r.channel.as_deref(), Some(&b"cc"[..]));
        let c = typed(
            "hash1b",
            0,
            2,
            "COMMENT_CREATE",
            pb::CommentCreate { post: vec![7; 32], text: "x".into(), parent_comment: vec![8; 32] }.encode_to_vec(),
        );
        assert_eq!(refs_of(&c).targets, vec![vec![7; 32], vec![8; 32]]);
    }

    #[tokio::test]
    async fn cached_authority_caches_answers_but_not_outages() {
        struct Counting(std::sync::atomic::AtomicUsize, Authz);
        #[async_trait::async_trait]
        impl DeviceAuthority for Counting {
            async fn check(&self, _: &str, _: &[u8]) -> Authz {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.1
            }
        }
        let inner = Arc::new(Counting(Default::default(), Authz::Authorised));
        let c = CachedAuthority::new(inner.clone(), Duration::from_secs(60));
        c.check("a", &[1]).await;
        c.check("a", &[1]).await;
        assert_eq!(inner.0.load(std::sync::atomic::Ordering::Relaxed), 1);
        let down = Arc::new(Counting(Default::default(), Authz::Unavailable));
        let c2 = CachedAuthority::new(down.clone(), Duration::from_secs(60));
        c2.check("a", &[1]).await;
        c2.check("a", &[1]).await;
        assert_eq!(down.0.load(std::sync::atomic::Ordering::Relaxed), 2);
    }
}
