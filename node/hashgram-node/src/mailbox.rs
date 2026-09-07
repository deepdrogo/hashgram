//! Store-and-forward mailboxes and key packages (the `store` role).
//!
//! What this node holds: ciphertext envelopes addressed to a mailbox (a hash
//! of a device key), and MLS key packages published by devices. What it can
//! read: the mailbox id, sizes and timestamps. What it cannot read: anything
//! else. There is no key on this machine that opens an envelope, and the
//! store never sees the sender.
//!
//! # Access control
//!
//! Anyone may deposit into a mailbox: that is what makes offline delivery
//! work. Only the holder of the device key may read or delete from it, proven
//! by a signature over a fresh timestamp. Deposits are bounded per mailbox
//! by count and bytes so a stranger cannot fill someone's mailbox; a device
//! that wants a bigger mailbox brings its own store node.
//!
//! # Retention
//!
//! Every envelope carries an expiry the node clamps to its retention bound.
//! A sweep deletes expired envelopes whether or not they were read. Nothing
//! is kept after acknowledgement.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use hashgram_p2p::{topics, MessageAcceptance, PeerId};
use hashgram_proto::pb;
use hashgram_proto::{signing, validate};
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use tracing::{debug, warn};

use crate::app::Shared;
use crate::store::now;

/// Most envelopes one mailbox may hold.
pub const MAX_ENVELOPES_PER_MAILBOX: u64 = 2000;
/// Most bytes one mailbox may hold: 64 MiB.
pub const MAX_BYTES_PER_MAILBOX: u64 = 64 << 20;

/// Envelope key: (mailbox, created_at, id).
type EnvelopeKey<'a> = (&'a [u8], u64, &'a [u8]);
// (mailbox, created_at, id) -> encoded Envelope
const ENVELOPES: TableDefinition<EnvelopeKey<'static>, &[u8]> = TableDefinition::new("envelopes");
// id -> (mailbox, created_at)
const BY_ID: TableDefinition<&[u8], (&[u8], u64)> = TableDefinition::new("envelopes_by_id");
// (expires_at, id) -> ()
const EXPIRY: TableDefinition<(u64, &[u8]), ()> = TableDefinition::new("envelope_expiry");
// mailbox -> (count, bytes)
const USAGE: TableDefinition<&[u8], (u64, u64)> = TableDefinition::new("mailbox_usage");
// One-time key packages: (device_pubkey, seq) -> encoded KeyPackagePublish.
// Handed out lowest seq first and deleted on the way out.
const KEY_PACKAGES: TableDefinition<(&[u8], u64), &[u8]> =
    TableDefinition::new("key_packages_once");
// Last-resort key package: device_pubkey -> encoded KeyPackagePublish.
const LAST_RESORT: TableDefinition<&[u8], &[u8]> = TableDefinition::new("key_packages_last_resort");

/// Most one-time key packages held per device.
pub const MAX_KEY_PACKAGES_PER_DEVICE: u64 = 64;

/// Store statistics for the operator API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MailboxStats {
    /// Envelopes held.
    pub envelopes: u64,
    /// Mailboxes with at least one envelope.
    pub mailboxes: u64,
    /// Key packages held.
    pub key_packages: u64,
    /// Bytes of ciphertext held.
    pub bytes: u64,
}

/// The service.
pub struct MailboxService {
    db: Database,
    network_id: String,
    /// Mailboxes other store nodes said have mail. A hint only.
    notified: Mutex<HashSet<Vec<u8>>>,
}

fn ok(body: pb::response::Body) -> pb::Response {
    pb::Response { body: Some(body) }
}

fn err(code: &str, msg: impl Into<String>) -> pb::Response {
    pb::Response {
        body: Some(pb::response::Body::Error(pb::Error {
            code: code.to_owned(),
            message: msg.into(),
        })),
    }
}

impl MailboxService {
    /// Opens the store.
    pub fn open(db: Database, network_id: &str) -> anyhow::Result<Arc<Self>> {
        // Create tables so readers never see "table does not exist".
        let txn = db.begin_write().context("mailbox: begin")?;
        {
            txn.open_table(ENVELOPES)?;
            txn.open_table(BY_ID)?;
            txn.open_table(EXPIRY)?;
            txn.open_table(USAGE)?;
            txn.open_table(KEY_PACKAGES)?;
            txn.open_table(LAST_RESORT)?;
        }
        txn.commit().context("mailbox: init")?;
        Ok(Arc::new(Self {
            db,
            network_id: network_id.to_owned(),
            notified: Mutex::new(HashSet::new()),
        }))
    }

    /// Gossip topics this service listens on.
    #[must_use]
    pub fn topics(&self) -> Vec<String> {
        (0..16u8)
            .map(|n| topics::mailbox_shard(&self.network_id, &[n]))
            .collect()
    }

    /// Records a notify hint.
    pub fn on_notify(&self, mailbox: &[u8]) -> MessageAcceptance {
        if mailbox.len() != 32 {
            return MessageAcceptance::Reject;
        }
        let mut n = self.notified.lock().unwrap_or_else(|e| e.into_inner());
        if n.len() < 100_000 {
            n.insert(mailbox.to_vec());
        }
        MessageAcceptance::Accept
    }

    /// Whether a notify hint is pending for a mailbox; clears it.
    pub fn take_notified(&self, mailbox: &[u8]) -> bool {
        self.notified
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(mailbox)
    }

    /// Handles a mailbox-family request from a verified peer.
    pub async fn handle(
        &self,
        shared: &Arc<Shared>,
        peer: PeerId,
        body: pb::request::Body,
    ) -> pb::Response {
        use pb::request::Body as B;
        let t = now();
        match body {
            B::MailboxPut(p) => {
                let Some(env) = p.envelope else {
                    return err("invalid", "no envelope");
                };
                match self.put(&env, t) {
                    Ok(expires_at) => {
                        // Tell other store nodes, so a device polling any
                        // of them learns there is mail. Carries only the
                        // mailbox id.
                        let g = pb::Gossip {
                            body: Some(pb::gossip::Body::MailboxNotify(env.mailbox.clone())),
                        };
                        let topic = topics::mailbox_shard(&self.network_id, &env.mailbox);
                        let _ = shared.handle.publish(&topic, g.encode_to_vec()).await;
                        ok(pb::response::Body::MailboxPut(pb::MailboxPutResult {
                            stored: true,
                            reason: String::new(),
                            expires_at,
                        }))
                    }
                    Err(PutError::Duplicate) => {
                        ok(pb::response::Body::MailboxPut(pb::MailboxPutResult {
                            stored: true,
                            reason: "duplicate".into(),
                            expires_at: 0,
                        }))
                    }
                    Err(PutError::Invalid(e)) => {
                        shared
                            .handle
                            .score(peer, hashgram_p2p::ScoreEvent::MalformedFrame)
                            .await;
                        err("invalid", e.to_string())
                    }
                    Err(PutError::Quota(why)) => err("quota", why),
                    Err(PutError::WrongNetwork) => {
                        err("invalid", "envelope is for another network")
                    }
                    Err(PutError::Db(e)) => {
                        warn!(error = %e, "mailbox put failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::MailboxFetch(f) => {
                let limit = match validate::mailbox_fetch(&f, t) {
                    Ok(l) => l,
                    Err(e) => return err("invalid", e.to_string()),
                };
                if let Err(e) = signing::verify_mailbox_fetch(&shared.identity, &f) {
                    shared
                        .handle
                        .score(peer, hashgram_p2p::ScoreEvent::InvalidSignature)
                        .await;
                    return err("invalid", e.to_string());
                }
                match self.fetch(&f.mailbox, &f.cursor, limit as usize) {
                    Ok((envelopes, cursor, remaining)) => {
                        ok(pb::response::Body::MailboxFetch(pb::MailboxFetchResult {
                            envelopes,
                            cursor,
                            remaining,
                        }))
                    }
                    Err(e) => {
                        warn!(error = %e, "mailbox fetch failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::MailboxAck(a) => {
                if let Err(e) = validate::mailbox_ack(&a, t) {
                    return err("invalid", e.to_string());
                }
                if let Err(e) = signing::verify_mailbox_ack(&shared.identity, &a) {
                    shared
                        .handle
                        .score(peer, hashgram_p2p::ScoreEvent::InvalidSignature)
                        .await;
                    return err("invalid", e.to_string());
                }
                match self.ack(&a.mailbox, &a.envelope_ids) {
                    Ok(deleted) => ok(pb::response::Body::MailboxAck(pb::MailboxAckResult {
                        deleted,
                    })),
                    Err(e) => {
                        warn!(error = %e, "mailbox ack failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::KeyPackagePublish(k) => {
                if let Err(e) = validate::key_package(&k, t) {
                    return err("invalid", e.to_string());
                }
                if let Err(e) = signing::verify_key_package(&shared.identity, &k) {
                    shared
                        .handle
                        .score(peer, hashgram_p2p::ScoreEvent::InvalidSignature)
                        .await;
                    return err("invalid", e.to_string());
                }
                match self.put_key_package(&k) {
                    Ok(remaining) => {
                        // Advertise that this node holds the device's mailbox
                        // and key package, so senders can find us.
                        let _ = shared
                            .handle
                            .start_providing(hashgram_proto::dht::mailbox_for_device(
                                &k.device_pubkey,
                            ))
                            .await;
                        ok(pb::response::Body::KeyPackagePublish(
                            pb::KeyPackagePublishResult {
                                stored: true,
                                reason: String::new(),
                                remaining,
                            },
                        ))
                    }
                    Err(e) => {
                        warn!(error = %e, "key package store failed");
                        err("internal", "storage error")
                    }
                }
            }
            B::KeyPackageFetch(f) => match self.take_key_package(&f.device_pubkey) {
                Ok(Some(k)) => ok(pb::response::Body::KeyPackageFetch(
                    pb::KeyPackageFetchResult {
                        found: true,
                        key_package: Some(k),
                    },
                )),
                Ok(None) => ok(pb::response::Body::KeyPackageFetch(
                    pb::KeyPackageFetchResult::default(),
                )),
                Err(e) => {
                    warn!(error = %e, "key package fetch failed");
                    err("internal", "storage error")
                }
            },
            _ => err("invalid", "not a mailbox request"),
        }
    }

    // -- storage --------------------------------------------------------------

    fn put(&self, env: &pb::Envelope, t: u64) -> Result<u64, PutError> {
        let expires_at = validate::envelope(env, t).map_err(PutError::Invalid)?;
        if env.network_id != self.network_id {
            return Err(PutError::WrongNetwork);
        }
        if signing::envelope_id(env).as_slice() != env.id.as_slice() {
            return Err(PutError::Invalid(validate::ValidationError::Rule(
                "envelope id does not match content",
            )));
        }
        let mut stored = env.clone();
        stored.expires_at = expires_at;
        let bytes = stored.encode_to_vec();
        let size = bytes.len() as u64;

        let txn = self.db.begin_write()?;
        {
            let mut by_id = txn.open_table(BY_ID)?;
            if by_id.get(env.id.as_slice())?.is_some() {
                return Err(PutError::Duplicate);
            }
            let mut usage = txn.open_table(USAGE)?;
            let (count, used) = usage
                .get(env.mailbox.as_slice())?
                .map(|g| g.value())
                .unwrap_or((0, 0));
            if count >= MAX_ENVELOPES_PER_MAILBOX {
                return Err(PutError::Quota(format!(
                    "mailbox holds {count} envelopes, the maximum"
                )));
            }
            if used + size > MAX_BYTES_PER_MAILBOX {
                return Err(PutError::Quota(format!(
                    "mailbox holds {used} bytes; {size} more exceeds the maximum"
                )));
            }
            usage.insert(env.mailbox.as_slice(), (count + 1, used + size))?;
            by_id.insert(env.id.as_slice(), (env.mailbox.as_slice(), env.created_at))?;
            let mut envelopes = txn.open_table(ENVELOPES)?;
            envelopes.insert(
                (env.mailbox.as_slice(), env.created_at, env.id.as_slice()),
                bytes.as_slice(),
            )?;
            let mut expiry = txn.open_table(EXPIRY)?;
            expiry.insert((expires_at, env.id.as_slice()), ())?;
        }
        txn.commit()?;
        Ok(expires_at)
    }

    fn fetch(
        &self,
        mailbox: &[u8],
        cursor: &[u8],
        limit: usize,
    ) -> anyhow::Result<(Vec<pb::Envelope>, Vec<u8>, u64)> {
        let (from_ts, from_id): (u64, Vec<u8>) = if cursor.len() == 40 {
            let ts = u64::from_be_bytes(
                cursor
                    .get(..8)
                    .and_then(|s| s.try_into().ok())
                    .unwrap_or([0; 8]),
            );
            (ts, cursor.get(8..).unwrap_or(&[]).to_vec())
        } else {
            (0, Vec::new())
        };
        let txn = self.db.begin_read()?;
        let table = txn.open_table(ENVELOPES)?;
        let start = (mailbox, from_ts, from_id.as_slice());
        let end = (mailbox, u64::MAX, [0xffu8; 32].as_slice());
        let mut out = Vec::new();
        let mut next_cursor = Vec::new();
        let mut remaining = 0u64;
        for item in table.range(start..=end)? {
            let (k, v) = item?;
            let (_, ts, id) = k.value();
            if ts == from_ts && id == from_id.as_slice() {
                continue; // the cursor names the last delivered envelope
            }
            if out.len() >= limit {
                remaining += 1;
                continue;
            }
            match pb::Envelope::decode(v.value()) {
                Ok(env) => {
                    next_cursor.clear();
                    next_cursor.extend_from_slice(&ts.to_be_bytes());
                    next_cursor.extend_from_slice(id);
                    out.push(env);
                }
                Err(e) => warn!(error = %e, "stored envelope did not decode; skipping"),
            }
        }
        if remaining == 0 {
            next_cursor.clear();
        }
        Ok((out, next_cursor, remaining))
    }

    fn ack(&self, mailbox: &[u8], ids: &[Vec<u8>]) -> anyhow::Result<u32> {
        let txn = self.db.begin_write()?;
        let mut deleted = 0u32;
        {
            let mut by_id = txn.open_table(BY_ID)?;
            let mut envelopes = txn.open_table(ENVELOPES)?;
            let mut expiry = txn.open_table(EXPIRY)?;
            let mut usage = txn.open_table(USAGE)?;
            for id in ids {
                let Some((mb, ts)) = by_id.get(id.as_slice())?.map(|g| {
                    let (m, t) = g.value();
                    (m.to_vec(), t)
                }) else {
                    continue;
                };
                if mb.as_slice() != mailbox {
                    // Acking somebody else's envelope. Signed by the wrong
                    // key for that mailbox, so it is refused silently.
                    continue;
                }
                let removed = envelopes.remove((mailbox, ts, id.as_slice()))?;
                let size = removed.map(|g| g.value().len() as u64).unwrap_or(0);
                by_id.remove(id.as_slice())?;
                // Expiry entry: we do not know expires_at from BY_ID, so scan
                // is avoided by leaving it; the sweep tolerates a missing
                // envelope.
                let _ = &mut expiry;
                let current = usage.get(mailbox)?.map(|g| g.value());
                if let Some((count, used)) = current {
                    let count = count.saturating_sub(1);
                    let used = used.saturating_sub(size);
                    if count == 0 {
                        usage.remove(mailbox)?;
                    } else {
                        usage.insert(mailbox, (count, used))?;
                    }
                }
                deleted += 1;
            }
        }
        txn.commit()?;
        Ok(deleted)
    }

    /// Deletes expired envelopes and key packages. Returns how many.
    pub fn sweep(&self) -> anyhow::Result<u64> {
        let t = now();
        let txn = self.db.begin_write()?;
        let mut removed = 0u64;
        {
            let mut expiry = txn.open_table(EXPIRY)?;
            let mut by_id = txn.open_table(BY_ID)?;
            let mut envelopes = txn.open_table(ENVELOPES)?;
            let mut usage = txn.open_table(USAGE)?;
            let due: Vec<(u64, Vec<u8>)> = expiry
                .range((0u64, [].as_slice())..=(t, [0xffu8; 32].as_slice()))?
                .filter_map(|r| r.ok())
                .map(|(k, _)| {
                    let (e, id) = k.value();
                    (e, id.to_vec())
                })
                .collect();
            for (e, id) in due {
                expiry.remove((e, id.as_slice()))?;
                if let Some((mb, ts)) = by_id.remove(id.as_slice())?.map(|g| {
                    let (m, t) = g.value();
                    (m.to_vec(), t)
                }) {
                    let size = envelopes
                        .remove((mb.as_slice(), ts, id.as_slice()))?
                        .map(|g| g.value().len() as u64)
                        .unwrap_or(0);
                    let current = usage.get(mb.as_slice())?.map(|g| g.value());
                    if let Some((count, used)) = current {
                        let count = count.saturating_sub(1);
                        if count == 0 {
                            usage.remove(mb.as_slice())?;
                        } else {
                            usage.insert(mb.as_slice(), (count, used.saturating_sub(size)))?;
                        }
                    }
                    removed += 1;
                }
            }
            let mut kps = txn.open_table(KEY_PACKAGES)?;
            kps.retain(|_, v| {
                pb::KeyPackagePublish::decode(v)
                    .map(|k| k.expires_at > t)
                    .unwrap_or(false)
            })?;
        }
        txn.commit()?;
        if removed > 0 {
            debug!(removed, "mailbox sweep");
        }
        Ok(removed)
    }

    /// Stores a key package. Returns how many one-time packages the device
    /// now has here.
    fn put_key_package(&self, k: &pb::KeyPackagePublish) -> anyhow::Result<u32> {
        let txn = self.db.begin_write()?;
        let remaining;
        {
            if k.last_resort {
                let mut t = txn.open_table(LAST_RESORT)?;
                let keep_old = t
                    .get(k.device_pubkey.as_slice())?
                    .and_then(|g| pb::KeyPackagePublish::decode(g.value()).ok())
                    .is_some_and(|old| old.created_at > k.created_at);
                if !keep_old {
                    t.insert(k.device_pubkey.as_slice(), k.encode_to_vec().as_slice())?;
                }
            }
            let mut once = txn.open_table(KEY_PACKAGES)?;
            let existing: Vec<u64> = once
                .range((k.device_pubkey.as_slice(), 0u64)..=(k.device_pubkey.as_slice(), u64::MAX))?
                .filter_map(|r| r.ok())
                .map(|(key, _)| key.value().1)
                .collect();
            if !k.last_resort {
                if existing.len() as u64 >= MAX_KEY_PACKAGES_PER_DEVICE {
                    remaining = existing.len() as u32;
                } else {
                    let next = existing.last().map(|s| s + 1).unwrap_or(0);
                    once.insert(
                        (k.device_pubkey.as_slice(), next),
                        k.encode_to_vec().as_slice(),
                    )?;
                    remaining = existing.len() as u32 + 1;
                }
            } else {
                remaining = existing.len() as u32;
            }
        }
        txn.commit()?;
        Ok(remaining)
    }

    /// Hands out a key package: the oldest one-time package (deleting it),
    /// or the last-resort one.
    fn take_key_package(
        &self,
        device_pubkey: &[u8],
    ) -> anyhow::Result<Option<pb::KeyPackagePublish>> {
        let t = now();
        let txn = self.db.begin_write()?;
        let mut out = None;
        {
            let mut once = txn.open_table(KEY_PACKAGES)?;
            let first: Option<(u64, Vec<u8>)> = once
                .range((device_pubkey, 0u64)..=(device_pubkey, u64::MAX))?
                .filter_map(|r| r.ok())
                .map(|(k, v)| (k.value().1, v.value().to_vec()))
                .find(|(_, v)| {
                    pb::KeyPackagePublish::decode(v.as_slice()).is_ok_and(|kp| kp.expires_at > t)
                });
            if let Some((seq, raw)) = first {
                once.remove((device_pubkey, seq))?;
                out = pb::KeyPackagePublish::decode(raw.as_slice()).ok();
            }
            if out.is_none() {
                let lr = txn.open_table(LAST_RESORT)?;
                out = lr
                    .get(device_pubkey)?
                    .and_then(|g| pb::KeyPackagePublish::decode(g.value()).ok())
                    .filter(|k| k.expires_at > t);
            }
        }
        txn.commit()?;
        Ok(out)
    }

    /// Statistics.
    pub fn stats(&self) -> anyhow::Result<MailboxStats> {
        let txn = self.db.begin_read()?;
        let usage = txn.open_table(USAGE)?;
        let mut envelopes = 0;
        let mut bytes = 0;
        let mut mailboxes = 0;
        for r in usage.iter()? {
            let (_, v) = r?;
            let (c, b) = v.value();
            envelopes += c;
            bytes += b;
            mailboxes += 1;
        }
        let key_packages =
            txn.open_table(KEY_PACKAGES)?.len()? + txn.open_table(LAST_RESORT)?.len()?;
        Ok(MailboxStats {
            envelopes,
            mailboxes,
            key_packages,
            bytes,
        })
    }
}

#[derive(Debug, thiserror::Error)]
enum PutError {
    #[error("duplicate")]
    Duplicate,
    #[error(transparent)]
    Invalid(validate::ValidationError),
    #[error("{0}")]
    Quota(String),
    #[error("wrong network")]
    WrongNetwork,
    #[error(transparent)]
    Db(#[from] anyhow::Error),
}

impl From<redb::TransactionError> for PutError {
    fn from(e: redb::TransactionError) -> Self {
        Self::Db(e.into())
    }
}
impl From<redb::TableError> for PutError {
    fn from(e: redb::TableError) -> Self {
        Self::Db(e.into())
    }
}
impl From<redb::StorageError> for PutError {
    fn from(e: redb::StorageError) -> Self {
        Self::Db(e.into())
    }
}
impl From<redb::CommitError> for PutError {
    fn from(e: redb::CommitError) -> Self {
        Self::Db(e.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashgram_proto::Ed25519Signer;

    fn service() -> Arc<MailboxService> {
        let dir = std::env::temp_dir().join(format!(
            "hg-mailbox-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let db = crate::store::open(&dir, "mailbox").unwrap();
        MailboxService::open(db, "hashgram-devnet").unwrap()
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn envelope(mailbox: &[u8], n: u8, ttl: u64) -> pb::Envelope {
        let t = now();
        let mut e = pb::Envelope {
            network_id: "hashgram-devnet".into(),
            version: 1,
            mailbox: mailbox.to_vec(),
            kind: pb::EnvelopeKind::MlsMessage as i32,
            ciphertext: vec![n; 64],
            created_at: t,
            expires_at: t + ttl,
            ..Default::default()
        };
        e.id = signing::envelope_id(&e).to_vec();
        e
    }

    #[test]
    fn put_fetch_ack_cycle() {
        let s = service();
        let dev = Ed25519Signer::generate().unwrap();
        let mb = signing::mailbox_for(&dev.public_key());
        for n in 0..5u8 {
            s.put(&envelope(&mb, n, 3600), now()).unwrap();
        }
        // Duplicate is reported, not stored twice.
        assert!(matches!(
            s.put(&envelope(&mb, 0, 3600), now()),
            Err(PutError::Duplicate)
        ));
        let stats = s.stats().unwrap();
        assert_eq!(stats.envelopes, 5);
        assert_eq!(stats.mailboxes, 1);

        let (page, cursor, remaining) = s.fetch(&mb, &[], 2).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(remaining, 3);
        assert!(!cursor.is_empty());
        let (page2, cursor2, remaining2) = s.fetch(&mb, &cursor, 10).unwrap();
        assert_eq!(page2.len(), 3);
        assert_eq!(remaining2, 0);
        assert!(cursor2.is_empty());

        let ids: Vec<Vec<u8>> = page
            .iter()
            .chain(page2.iter())
            .map(|e| e.id.clone())
            .collect();
        assert_eq!(s.ack(&mb, &ids).unwrap(), 5);
        assert_eq!(s.stats().unwrap().envelopes, 0);
    }

    #[test]
    fn a_stranger_cannot_ack_another_mailbox() {
        let s = service();
        let a = signing::mailbox_for(&Ed25519Signer::generate().unwrap().public_key());
        let b = signing::mailbox_for(&Ed25519Signer::generate().unwrap().public_key());
        let e = envelope(&a, 1, 3600);
        s.put(&e, now()).unwrap();
        assert_eq!(s.ack(&b, std::slice::from_ref(&e.id)).unwrap(), 0);
        assert_eq!(s.stats().unwrap().envelopes, 1);
    }

    #[test]
    fn quota_bounds_a_mailbox() {
        let s = service();
        let mb = [7u8; 32];
        for n in 0..=255u8 {
            let mut e = envelope(&mb, n, 3600);
            e.ciphertext = vec![n; 200_000];
            e.id = signing::envelope_id(&e).to_vec();
            match s.put(&e, now()) {
                Ok(_) => {}
                Err(PutError::Quota(_)) => return,
                Err(other) => panic!("{other}"),
            }
        }
        // 256 * 200 KB = 51 MB < 64 MiB, so the count cap must not have
        // fired either; keep pushing until bytes do.
        for n in 0..200u16 {
            let mut e = envelope(&mb, (n % 256) as u8, 3600);
            e.ciphertext = vec![(n >> 8) as u8; 200_000];
            e.created_at += u64::from(n) + 1;
            e.id = signing::envelope_id(&e).to_vec();
            if let Err(PutError::Quota(_)) = s.put(&e, now()) {
                return;
            }
        }
        panic!("quota never fired");
    }

    #[test]
    fn expired_envelopes_are_swept() {
        let s = service();
        let mb = [9u8; 32];
        let mut e = envelope(&mb, 1, 3600);
        // Already expired at storage time is refused outright.
        e.expires_at = now() - 1;
        e.id = signing::envelope_id(&e).to_vec();
        assert!(matches!(s.put(&e, now()), Err(PutError::Invalid(_))));
        // Store one that expires in 1s, then sweep after.
        let mut e2 = envelope(&mb, 2, 1);
        e2.id = signing::envelope_id(&e2).to_vec();
        s.put(&e2, now()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert_eq!(s.sweep().unwrap(), 1);
        assert_eq!(s.stats().unwrap().envelopes, 0);
    }

    #[test]
    fn a_fork_envelope_is_refused() {
        let s = service();
        let mut e = envelope(&[1u8; 32], 1, 3600);
        e.network_id = "hashgram-mainnet".into();
        e.id = signing::envelope_id(&e).to_vec();
        assert!(matches!(s.put(&e, now()), Err(PutError::WrongNetwork)));
    }

    #[test]
    fn one_time_key_packages_are_consumed_then_last_resort_serves() {
        let s = service();
        let dev = Ed25519Signer::generate().unwrap();
        let t = now();
        let id = hashgram_net::NetworkIdentity::devnet(
            "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287",
        );
        let mk = |n: u8, last_resort: bool| {
            let mut k = pb::KeyPackagePublish {
                network_id: "hashgram-devnet".into(),
                key_package: vec![n; 100],
                created_at: t,
                expires_at: t + 3600,
                last_resort,
                ..Default::default()
            };
            signing::sign_key_package(&id, &dev, &mut k).unwrap();
            k
        };
        assert_eq!(s.put_key_package(&mk(1, false)).unwrap(), 1);
        assert_eq!(s.put_key_package(&mk(2, false)).unwrap(), 2);
        assert_eq!(s.put_key_package(&mk(9, true)).unwrap(), 2);
        assert_eq!(
            s.take_key_package(&dev.public_key())
                .unwrap()
                .unwrap()
                .key_package,
            vec![1; 100]
        );
        assert_eq!(
            s.take_key_package(&dev.public_key())
                .unwrap()
                .unwrap()
                .key_package,
            vec![2; 100]
        );
        // One-time supply exhausted: the last-resort package serves, repeatedly.
        assert_eq!(
            s.take_key_package(&dev.public_key())
                .unwrap()
                .unwrap()
                .key_package,
            vec![9; 100]
        );
        assert_eq!(
            s.take_key_package(&dev.public_key())
                .unwrap()
                .unwrap()
                .key_package,
            vec![9; 100]
        );
        // A device with nothing published gets nothing.
        assert!(s.take_key_package(&[0u8; 32]).unwrap().is_none());
    }
}
