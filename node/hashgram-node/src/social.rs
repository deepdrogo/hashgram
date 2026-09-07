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

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use hashgram_p2p::{topics, MessageAcceptance, PeerId, ScoreEvent};
use hashgram_proto::limits::MAX_EVENT_PAGE;
use hashgram_proto::pb;
use hashgram_proto::{signing, validate};
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use tracing::{debug, warn};

use crate::app::Shared;
use crate::store::now;

// id -> encoded SocialEvent
const EVENTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("events");
// (author, sequence, id) -> ()
const BY_AUTHOR: TableDefinition<(&str, u64, &[u8]), ()> = TableDefinition::new("events_by_author");
// (timestamp, id) -> ()
const BY_TIME: TableDefinition<(u64, &[u8]), ()> = TableDefinition::new("events_by_time");

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

/// The service.
pub struct SocialService {
    db: Database,
    network_id: String,
    authority: Arc<dyn DeviceAuthority>,
    retention_secs: u64,
    max_events: u64,
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
        {
            txn.open_table(EVENTS)?;
            txn.open_table(BY_AUTHOR)?;
            txn.open_table(BY_TIME)?;
        }
        txn.commit().context("social: init")?;
        Ok(Arc::new(Self {
            db,
            network_id: network_id.to_owned(),
            authority,
            retention_secs: u64::from(retention_days) * 86_400,
            max_events,
        }))
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
                    Err(Refusal::Db(e)) => {
                        warn!(error = %e, "social store failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::EventFetch(f) => match self.fetch(&f, shared.services.safety.as_deref()) {
                Ok(events) => pb::Response {
                    body: Some(pb::response::Body::EventFetch(pb::EventFetchResult {
                        events,
                    })),
                },
                Err(e) => {
                    warn!(error = %e, "event fetch failed");
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
        }
        txn.commit()?;
        Ok(true)
    }

    /// Fetches by ids, or by author from a sequence.
    pub fn fetch(
        &self,
        f: &pb::EventFetch,
        safety: Option<&crate::safety::SafetyService>,
    ) -> anyhow::Result<Vec<pb::SocialEvent>> {
        let limit = if f.limit == 0 {
            MAX_EVENT_PAGE
        } else {
            f.limit.min(MAX_EVENT_PAGE)
        } as usize;
        let txn = self.db.begin_read()?;
        let events = txn.open_table(EVENTS)?;
        let mut out = Vec::new();
        let blocked = |id: &[u8]| safety.is_some_and(|s| s.is_blocked_event(id));
        if !f.ids.is_empty() {
            for id in f.ids.iter().take(limit) {
                if blocked(id) {
                    continue;
                }
                if let Some(v) = events.get(id.as_slice())? {
                    if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                        out.push(ev);
                    }
                }
            }
            return Ok(out);
        }
        if !f.author.is_empty() {
            let by_author = txn.open_table(BY_AUTHOR)?;
            let start = (f.author.as_str(), f.from_sequence, [].as_slice());
            let end = (f.author.as_str(), u64::MAX, [0xffu8; 32].as_slice());
            for r in by_author.range(start..=end)? {
                let (k, _) = r?;
                let id = k.value().2;
                if blocked(id) {
                    continue;
                }
                if let Some(v) = events.get(id)? {
                    if let Ok(ev) = pb::SocialEvent::decode(v.value()) {
                        out.push(ev);
                    }
                }
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
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
        };
        drop(s);
        let txn = svc.db.begin_write().unwrap();
        {
            txn.open_table(EVENTS).unwrap();
            txn.open_table(BY_AUTHOR).unwrap();
            txn.open_table(BY_TIME).unwrap();
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
