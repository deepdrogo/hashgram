//! Content-addressed blob storage (the `store` and `media` roles).
//!
//! Chunks are stored under their BLAKE3 hash, manifests under their CID.
//! Every byte that arrives is hashed before it is kept, so this node never
//! stores something that does not match what it claims to be, and every byte
//! that leaves can be checked by the receiver the same way. Providers are
//! interchangeable because of this: a client trusts the hash, not the node.
//!
//! # Replication
//!
//! Target three copies. This node tracks, for each blob it holds, which
//! other verified store peers also hold it (learned by asking). When fewer
//! than the target are known and enough store peers exist, it pushes the
//! blob to another one. When only this node holds a blob, the blob's health
//! is `DEGRADED`, and the operator API says so — one copy is one copy.
//!
//! # Quota
//!
//! Uploads are accepted up to the configured storage quota, per uploader
//! device up to a per-device share, and never for manifests that do not
//! validate. A chunk that does not match its manifest entry is refused and
//! the sender scored.
//!
//! Quota is reserved when the manifest is accepted, by the declared size,
//! because that is the moment the node commits to holding the bytes. An
//! upload that never finishes would hold that reservation forever, so a
//! manifest still missing chunks after [`INCOMPLETE_UPLOAD_TTL_SECS`] is
//! dropped by the maintenance sweep and its reservation returned.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use hashgram_p2p::{Multiaddr, PeerId, ScoreEvent};
use hashgram_proto::blob::{chunk_matches, cid, validate_manifest};
use hashgram_proto::keys::blake3_hash;
use hashgram_proto::limits::{CHUNK_SIZE, MAX_RPC_FRAME};
use hashgram_proto::pb;
use hashgram_proto::{signing, validate};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;
use prost::Message;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use tracing::{debug, info, warn};

use crate::app::Shared;
use crate::store::now;

/// Desired copies of every blob.
pub const TARGET_REPLICAS: u32 = 3;

/// How long an accepted manifest may stay incomplete before the node gives
/// up on the upload and releases its quota reservation: 24 hours. Long
/// enough for a phone on a bad link to resume a large upload across a day;
/// short enough that an abandoned or hostile reservation does not hold the
/// uploader's share, or the node's, indefinitely.
pub const INCOMPLETE_UPLOAD_TTL_SECS: u64 = 24 * 3600;

/// Per-uploader share of the quota, as a fraction denominator: one device
/// may use at most 1/8 of this node's quota.
const PER_UPLOADER_DIVISOR: u64 = 8;

// cid -> encoded BlobManifest
const MANIFESTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("manifests");
// chunk hash -> chunk bytes
const CHUNKS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("chunks");
// (cid, index) -> ()  which chunks of a manifest are present
const PRESENT: TableDefinition<(&[u8], u32), ()> = TableDefinition::new("present");
// cid -> (uploader_pubkey, stored_at, bytes)
const META: TableDefinition<&[u8], (&[u8], u64, u64)> = TableDefinition::new("blob_meta");
// uploader_pubkey -> bytes used
const UPLOADER_USAGE: TableDefinition<&[u8], u64> = TableDefinition::new("uploader_usage");
// chunk hash -> reference count
const CHUNK_REFS: TableDefinition<&[u8], u64> = TableDefinition::new("chunk_refs");
// cid -> SHA-256 Merkle root over chunk contents, computed once complete
const ROOTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("merkle_roots");

/// Statistics for the operator API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlobStats {
    /// Blobs with a complete set of chunks.
    pub blobs_complete: u64,
    /// Blobs with a manifest but missing chunks (uploads in progress).
    pub blobs_partial: u64,
    /// Bytes reserved against the quota: the declared size of every
    /// accepted manifest, summed per uploader. This is the figure quota is
    /// enforced on; chunks shared between manifests are counted once per
    /// manifest, so it can exceed the bytes physically held.
    pub bytes_used: u64,
    /// Configured quota, 0 for unlimited.
    pub quota_bytes: u64,
    /// Blobs whose known replica count is below target.
    pub degraded: u64,
    /// Verified peers with a storage role.
    pub store_peers: usize,
}

/// Replication state for one blob.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Health {
    /// CID, hex.
    pub cid: String,
    /// Target.
    pub target_replicas: u32,
    /// Known copies including this node.
    pub known_replicas: u32,
    /// `HEALTHY`, `DEGRADED`.
    pub status: &'static str,
    /// Peers known to hold it.
    pub providers: Vec<String>,
}

/// Peers known to hold each blob, and when that was last refreshed.
type ReplicaMap = HashMap<Vec<u8>, (HashSet<PeerId>, Instant)>;

/// The service.
pub struct BlobService {
    db: Database,
    quota: u64,
    /// Verified peers claiming a storage role, with addresses.
    store_peers: Mutex<HashMap<PeerId, Vec<Multiaddr>>>,
    /// cid -> peers known to hold it, and when last checked.
    replicas: Mutex<ReplicaMap>,
    /// Incomplete uploads dropped after their TTL.
    incomplete_expired: Counter,
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

impl BlobService {
    /// Opens the store.
    pub fn open(db: Database, quota: u64) -> anyhow::Result<Arc<Self>> {
        let txn = db.begin_write().context("blob: begin")?;
        {
            txn.open_table(MANIFESTS)?;
            txn.open_table(CHUNKS)?;
            txn.open_table(PRESENT)?;
            txn.open_table(META)?;
            txn.open_table(UPLOADER_USAGE)?;
            txn.open_table(CHUNK_REFS)?;
            txn.open_table(ROOTS)?;
        }
        txn.commit().context("blob: init")?;
        Ok(Arc::new(Self {
            db,
            quota,
            store_peers: Mutex::new(HashMap::new()),
            replicas: Mutex::new(HashMap::new()),
            incomplete_expired: Counter::default(),
        }))
    }

    /// Registers this service's metrics: `hashgram_blob_incomplete_expired_total`.
    pub fn register_metrics(&self, registry: &mut Registry) {
        registry.register(
            "hashgram_blob_incomplete_expired",
            "Blob manifests dropped because their upload was still incomplete after the TTL",
            self.incomplete_expired.clone(),
        );
    }

    /// A verified peer appeared; remember it if it stores.
    pub fn on_peer_verified(&self, peer: PeerId, roles: &[String]) {
        if roles.iter().any(|r| r == "store" || r == "media") {
            self.store_peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(peer)
                .or_default();
        }
    }

    /// A peer left.
    pub fn on_peer_lost(&self, peer: PeerId) {
        self.store_peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&peer);
        for (set, _) in self
            .replicas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            set.remove(&peer);
        }
    }

    /// Handles a blob-family request from a verified peer.
    pub async fn handle(
        &self,
        shared: &Arc<Shared>,
        peer: PeerId,
        body: pb::request::Body,
    ) -> pb::Response {
        use pb::request::Body as B;
        match body {
            B::BlobGetManifest(q) => match self.get_manifest(&q.cid) {
                Ok(Some(m)) => ok(pb::response::Body::BlobGetManifest(
                    pb::BlobManifestResult {
                        found: true,
                        manifest: Some(m),
                    },
                )),
                Ok(None) => ok(pb::response::Body::BlobGetManifest(
                    pb::BlobManifestResult::default(),
                )),
                Err(e) => internal(e),
            },
            B::BlobGetChunk(q) => match self.get_chunk(&q.cid, q.index) {
                Ok(Some(data)) => ok(pb::response::Body::BlobGetChunk(pb::BlobChunk {
                    found: true,
                    data,
                })),
                Ok(None) => ok(pb::response::Body::BlobGetChunk(pb::BlobChunk::default())),
                Err(e) => internal(e),
            },
            B::BlobHas(q) => match self.has(&q.cid) {
                Ok((has_manifest, present, total)) => {
                    ok(pb::response::Body::BlobHas(pb::BlobHasResult {
                        has_manifest,
                        chunks_present: present,
                        chunks_total: total,
                    }))
                }
                Err(e) => internal(e),
            },
            B::BlobPutManifest(p) => {
                let t = now();
                if let Err(e) = validate::blob_put_manifest(&p, t) {
                    return err("invalid", e.to_string());
                }
                if let Err(e) = signing::verify_blob_upload(&shared.identity, &p) {
                    shared
                        .handle
                        .score(peer, ScoreEvent::InvalidSignature)
                        .await;
                    return err("invalid", e.to_string());
                }
                let Some(manifest) = p.manifest else {
                    return err("invalid", "no manifest");
                };
                match self.put_manifest(&p.cid, &manifest, &p.uploader_pubkey) {
                    Ok(missing) => {
                        if missing.is_empty() {
                            let _ = shared
                                .handle
                                .start_providing(hashgram_proto::dht::blob(&p.cid))
                                .await;
                        }
                        ok(pb::response::Body::BlobPutManifest(
                            pb::BlobPutManifestResult {
                                accepted: true,
                                reason: String::new(),
                                missing_chunks: missing,
                            },
                        ))
                    }
                    Err(PutError::Invalid(why)) => {
                        shared.handle.score(peer, ScoreEvent::MalformedFrame).await;
                        err("invalid", why)
                    }
                    Err(PutError::Quota(why)) => err("quota", why),
                    Err(PutError::Db(e)) => internal(e),
                }
            }
            B::BlobPutChunk(c) => match self.put_chunk(&c.cid, c.index, &c.data) {
                Ok(complete) => {
                    if complete {
                        let _ = shared
                            .handle
                            .start_providing(hashgram_proto::dht::blob(&c.cid))
                            .await;
                        self.note_replica(&c.cid, shared.handle.peer_id());
                    }
                    ok(pb::response::Body::BlobPutChunk(pb::BlobPutChunkResult {
                        accepted: true,
                        reason: String::new(),
                    }))
                }
                Err(PutError::Invalid(why)) => {
                    shared
                        .handle
                        .score(peer, ScoreEvent::ServedCorruptData)
                        .await;
                    err("invalid", why)
                }
                Err(PutError::Quota(why)) => err("quota", why),
                Err(PutError::Db(e)) => internal(e),
            },
            _ => err("invalid", "not a blob request"),
        }
    }

    // -- storage --------------------------------------------------------------

    fn get_manifest(&self, cid_bytes: &[u8]) -> anyhow::Result<Option<pb::BlobManifest>> {
        let txn = self.db.begin_read()?;
        let t = txn.open_table(MANIFESTS)?;
        Ok(t.get(cid_bytes)?
            .and_then(|g| pb::BlobManifest::decode(g.value()).ok()))
    }

    fn get_chunk(&self, cid_bytes: &[u8], index: u32) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(m) = self.get_manifest(cid_bytes)? else {
            return Ok(None);
        };
        let Some(hash) = m.chunks.get(index as usize) else {
            return Ok(None);
        };
        let txn = self.db.begin_read()?;
        let t = txn.open_table(CHUNKS)?;
        Ok(t.get(hash.as_slice())?.map(|g| g.value().to_vec()))
    }

    fn has(&self, cid_bytes: &[u8]) -> anyhow::Result<(bool, u32, u32)> {
        let Some(m) = self.get_manifest(cid_bytes)? else {
            return Ok((false, 0, 0));
        };
        let total = m.chunks.len() as u32;
        let txn = self.db.begin_read()?;
        let present = txn.open_table(PRESENT)?;
        let count = present
            .range((cid_bytes, 0u32)..=(cid_bytes, u32::MAX))?
            .count() as u32;
        Ok((true, count, total))
    }

    fn missing_chunks(&self, cid_bytes: &[u8], total: usize) -> anyhow::Result<Vec<u32>> {
        let txn = self.db.begin_read()?;
        let present = txn.open_table(PRESENT)?;
        let have: HashSet<u32> = present
            .range((cid_bytes, 0u32)..=(cid_bytes, u32::MAX))?
            .filter_map(|r| r.ok())
            .map(|(k, _)| k.value().1)
            .collect();
        Ok((0..total as u32).filter(|i| !have.contains(i)).collect())
    }

    /// Bytes reserved by accepted manifests, as the sum of every uploader's
    /// usage. Reservation is by declared size at manifest time, and a chunk
    /// shared by two manifests counts for both: that is the number quota is
    /// enforced against, so it is the number reported. Walking the uploader
    /// table is O(uploaders); the earlier chunk walk was O(bytes held).
    fn bytes_used(&self) -> anyhow::Result<u64> {
        let txn = self.db.begin_read()?;
        let per = txn.open_table(UPLOADER_USAGE)?;
        let mut total = 0u64;
        for r in per.iter()? {
            let (_, v) = r?;
            total = total.saturating_add(v.value());
        }
        Ok(total)
    }

    fn put_manifest(
        &self,
        cid_bytes: &[u8],
        m: &pb::BlobManifest,
        uploader: &[u8],
    ) -> Result<Vec<u32>, PutError> {
        validate_manifest(m).map_err(|e| PutError::Invalid(e.to_string()))?;
        if cid(m).as_slice() != cid_bytes {
            return Err(PutError::Invalid("cid does not match manifest".into()));
        }
        if let Some(existing) = self.get_manifest(cid_bytes)? {
            // Already known: report what is still missing so the upload
            // resumes. The uploader on record stays whoever was first.
            let _ = existing;
            return Ok(self.missing_chunks(cid_bytes, m.chunks.len())?);
        }
        // Quota checks happen against the declared size, so a manifest for
        // a blob that cannot fit is refused before any chunk arrives.
        if self.quota > 0 {
            let used = self.bytes_used()?;
            if used.saturating_add(m.size) > self.quota {
                return Err(PutError::Quota(format!(
                    "node has {used} of {} bytes in use; a {}-byte blob does not fit",
                    self.quota, m.size
                )));
            }
            let txn = self.db.begin_read()?;
            let per = txn.open_table(UPLOADER_USAGE)?;
            let mine = per.get(uploader)?.map(|g| g.value()).unwrap_or(0);
            #[allow(clippy::integer_division)] // a share, truncation intended
            let share = self.quota / PER_UPLOADER_DIVISOR;
            if mine.saturating_add(m.size) > share {
                return Err(PutError::Quota(format!(
                    "uploader has {mine} of a {share}-byte share in use on this node"
                )));
            }
        }
        let txn = self.db.begin_write()?;
        {
            let mut manifests = txn.open_table(MANIFESTS)?;
            manifests.insert(cid_bytes, m.encode_to_vec().as_slice())?;
            let mut meta = txn.open_table(META)?;
            meta.insert(cid_bytes, (uploader, now(), m.size))?;
            let mut per = txn.open_table(UPLOADER_USAGE)?;
            let mine = per.get(uploader)?.map(|g| g.value()).unwrap_or(0);
            per.insert(uploader, mine + m.size)?;
        }
        txn.commit()?;
        // Chunks already held from another blob count as present.
        let mut missing = Vec::new();
        {
            let txn = self.db.begin_write()?;
            {
                let chunks = txn.open_table(CHUNKS)?;
                let mut present = txn.open_table(PRESENT)?;
                let mut refs = txn.open_table(CHUNK_REFS)?;
                for (i, h) in m.chunks.iter().enumerate() {
                    if chunks.get(h.as_slice())?.is_some() {
                        present.insert((cid_bytes, i as u32), ())?;
                        let n = refs.get(h.as_slice())?.map(|g| g.value()).unwrap_or(0);
                        refs.insert(h.as_slice(), n + 1)?;
                    } else {
                        missing.push(i as u32);
                    }
                }
            }
            txn.commit()?;
        }
        Ok(missing)
    }

    fn put_chunk(&self, cid_bytes: &[u8], index: u32, data: &[u8]) -> Result<bool, PutError> {
        let Some(m) = self.get_manifest(cid_bytes)? else {
            return Err(PutError::Invalid(
                "no manifest for this cid; send the manifest first".into(),
            ));
        };
        if !chunk_matches(&m, index, data) {
            return Err(PutError::Invalid(format!(
                "chunk {index} does not match its manifest hash"
            )));
        }
        let hash = blake3_hash(data);
        let txn = self.db.begin_write()?;
        let complete;
        {
            let mut chunks = txn.open_table(CHUNKS)?;
            let mut present = txn.open_table(PRESENT)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            if present.get((cid_bytes, index))?.is_none() {
                if chunks.get(hash.as_slice())?.is_none() {
                    chunks.insert(hash.as_slice(), data)?;
                }
                present.insert((cid_bytes, index), ())?;
                let n = refs.get(hash.as_slice())?.map(|g| g.value()).unwrap_or(0);
                refs.insert(hash.as_slice(), n + 1)?;
            }
            let have = present
                .range((cid_bytes, 0u32)..=(cid_bytes, u32::MAX))?
                .count();
            complete = have == m.chunks.len();
        }
        txn.commit()?;
        Ok(complete)
    }

    /// The manifest of a blob, if held.
    #[must_use]
    pub fn manifest(&self, cid_bytes: &[u8]) -> Option<pb::BlobManifest> {
        self.get_manifest(cid_bytes).ok().flatten()
    }

    /// Peers known to hold a blob besides this node.
    #[must_use]
    pub fn known_holders(&self, cid_bytes: &[u8]) -> Vec<PeerId> {
        self.replicas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(cid_bytes)
            .map(|(s, _)| s.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Leaf hashes of every chunk (SHA-256 with the storage-challenge
    /// prefix) and the bytes of chunk `index`, if the blob is complete.
    #[must_use]
    pub fn chunk_leaves_and_chunk(
        &self,
        cid_bytes: &[u8],
        index: u32,
    ) -> Option<(Vec<[u8; 32]>, Vec<u8>)> {
        let m = self.get_manifest(cid_bytes).ok().flatten()?;
        let txn = self.db.begin_read().ok()?;
        let chunks = txn.open_table(CHUNKS).ok()?;
        let mut leaves = Vec::with_capacity(m.chunks.len());
        let mut wanted = None;
        for (i, h) in m.chunks.iter().enumerate() {
            let data = chunks.get(h.as_slice()).ok().flatten()?.value().to_vec();
            leaves.push(hashgram_proto::merkle::leaf_hash(&data));
            if i as u32 == index {
                wanted = Some(data);
            }
        }
        Some((leaves, wanted?))
    }

    /// The Merkle root the chain expects in a storage assignment, computed
    /// once and cached. `None` unless the blob is complete.
    #[must_use]
    pub fn merkle_root(&self, cid_bytes: &[u8]) -> Option<[u8; 32]> {
        if let Ok(txn) = self.db.begin_read() {
            if let Ok(t) = txn.open_table(ROOTS) {
                if let Ok(Some(v)) = t.get(cid_bytes) {
                    if let Ok(r) = <[u8; 32]>::try_from(v.value()) {
                        return Some(r);
                    }
                }
            }
        }
        let (leaves, _) = self.chunk_leaves_and_chunk(cid_bytes, 0)?;
        let root = hashgram_proto::merkle::root_from_leaves(&leaves);
        if let Ok(txn) = self.db.begin_write() {
            if let Ok(mut t) = txn.open_table(ROOTS) {
                let _ = t.insert(cid_bytes, root.as_slice());
            }
            let _ = txn.commit();
        }
        Some(root)
    }

    /// Stores a whole blob held in memory (used by repair and by the local
    /// API for same-host uploads). Returns the CID.
    pub fn put_local(
        &self,
        m: &pb::BlobManifest,
        data: &[u8],
        uploader: &[u8],
    ) -> Result<Vec<u8>, PutError> {
        let c = cid(m).to_vec();
        self.put_manifest(&c, m, uploader)?;
        for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
            self.put_chunk(&c, i as u32, chunk)?;
        }
        Ok(c)
    }

    /// Deletes a blob this node holds, releasing chunks no other blob uses.
    pub fn delete(&self, cid_bytes: &[u8]) -> anyhow::Result<bool> {
        let Some(m) = self.get_manifest(cid_bytes)? else {
            return Ok(false);
        };
        let txn = self.db.begin_write()?;
        {
            let mut roots = txn.open_table(ROOTS)?;
            roots.remove(cid_bytes)?;
            let mut manifests = txn.open_table(MANIFESTS)?;
            manifests.remove(cid_bytes)?;
            let mut meta = txn.open_table(META)?;
            if let Some((uploader, _, size)) = meta.remove(cid_bytes)?.map(|g| {
                let (u, t, s) = g.value();
                (u.to_vec(), t, s)
            }) {
                let mut per = txn.open_table(UPLOADER_USAGE)?;
                let mine = per
                    .get(uploader.as_slice())?
                    .map(|g| g.value())
                    .unwrap_or(0);
                let left = mine.saturating_sub(size);
                if left == 0 {
                    per.remove(uploader.as_slice())?;
                } else {
                    per.insert(uploader.as_slice(), left)?;
                }
            }
            let mut present = txn.open_table(PRESENT)?;
            let mut chunks = txn.open_table(CHUNKS)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            for (i, h) in m.chunks.iter().enumerate() {
                if present.remove((cid_bytes, i as u32))?.is_some() {
                    let n = refs.get(h.as_slice())?.map(|g| g.value()).unwrap_or(1);
                    if n <= 1 {
                        refs.remove(h.as_slice())?;
                        chunks.remove(h.as_slice())?;
                    } else {
                        refs.insert(h.as_slice(), n - 1)?;
                    }
                }
            }
        }
        txn.commit()?;
        self.replicas
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(cid_bytes);
        Ok(true)
    }

    /// All complete CIDs this node holds.
    pub fn complete_cids(&self) -> anyhow::Result<Vec<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let manifests = txn.open_table(MANIFESTS)?;
        let present = txn.open_table(PRESENT)?;
        let mut out = Vec::new();
        for r in manifests.iter()? {
            let (k, v) = r?;
            let Ok(m) = pb::BlobManifest::decode(v.value()) else {
                continue;
            };
            let have = present
                .range((k.value(), 0u32)..=(k.value(), u32::MAX))?
                .count();
            if have == m.chunks.len() {
                out.push(k.value().to_vec());
            }
        }
        Ok(out)
    }

    /// Drops manifests whose upload is still incomplete `INCOMPLETE_UPLOAD_TTL_SECS`
    /// after they were accepted, releasing the uploader's reservation and
    /// any chunks no other blob references. Returns how many were dropped.
    ///
    /// `now` is a parameter rather than read here so the sweep is testable
    /// without waiting a day; the maintenance tick passes the clock.
    pub fn expire_incomplete(&self, now: u64) -> anyhow::Result<u64> {
        let due: Vec<Vec<u8>> = {
            let txn = self.db.begin_read()?;
            let meta = txn.open_table(META)?;
            let manifests = txn.open_table(MANIFESTS)?;
            let present = txn.open_table(PRESENT)?;
            let mut out = Vec::new();
            for r in meta.iter()? {
                let (k, v) = r?;
                let (_, stored_at, _) = v.value();
                if stored_at.saturating_add(INCOMPLETE_UPLOAD_TTL_SECS) >= now {
                    continue;
                }
                let c = k.value();
                let total = manifests
                    .get(c)?
                    .and_then(|g| pb::BlobManifest::decode(g.value()).ok())
                    .map(|m| m.chunks.len());
                let have = present.range((c, 0u32)..=(c, u32::MAX))?.count();
                // A manifest row that no longer decodes is also junk worth
                // dropping; a complete blob is kept whatever its age.
                if total.is_none_or(|t| have < t) {
                    out.push(c.to_vec());
                }
            }
            out
        };
        let mut removed = 0u64;
        for c in due {
            if self.delete(&c)? {
                removed += 1;
                self.incomplete_expired.inc();
            }
        }
        if removed > 0 {
            info!(removed, "expired incomplete blob uploads");
        }
        Ok(removed)
    }

    /// Statistics.
    pub fn stats(&self) -> anyhow::Result<BlobStats> {
        let txn = self.db.begin_read()?;
        let manifests = txn.open_table(MANIFESTS)?;
        let present = txn.open_table(PRESENT)?;
        let mut complete = 0;
        let mut partial = 0;
        let mut degraded = 0;
        let replicas = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
        for r in manifests.iter()? {
            let (k, v) = r?;
            let Ok(m) = pb::BlobManifest::decode(v.value()) else {
                continue;
            };
            let have = present
                .range((k.value(), 0u32)..=(k.value(), u32::MAX))?
                .count();
            if have == m.chunks.len() {
                complete += 1;
                let known = replicas.get(k.value()).map(|(s, _)| s.len()).unwrap_or(0) as u32 + 1;
                if known < TARGET_REPLICAS {
                    degraded += 1;
                }
            } else {
                partial += 1;
            }
        }
        drop(replicas);
        // O(uploaders), not O(bytes): the per-uploader table is maintained on
        // every manifest accept and delete, so its total is the reservation.
        let per = txn.open_table(UPLOADER_USAGE)?;
        let mut bytes_used = 0u64;
        for r in per.iter()? {
            let (_, v) = r?;
            bytes_used = bytes_used.saturating_add(v.value());
        }
        Ok(BlobStats {
            blobs_complete: complete,
            blobs_partial: partial,
            bytes_used,
            quota_bytes: self.quota,
            degraded,
            store_peers: self
                .store_peers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len(),
        })
    }

    /// Replication health of one blob.
    pub fn health(&self, cid_bytes: &[u8], me: PeerId) -> anyhow::Result<Option<Health>> {
        let (has, present, total) = self.has(cid_bytes)?;
        if !has || present != total {
            return Ok(None);
        }
        let replicas = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
        let others: Vec<PeerId> = replicas
            .get(cid_bytes)
            .map(|(s, _)| s.iter().copied().collect())
            .unwrap_or_default();
        let known = others.len() as u32 + 1;
        let mut providers: Vec<String> = others.iter().map(ToString::to_string).collect();
        providers.insert(0, me.to_string());
        Ok(Some(Health {
            cid: hex::encode(cid_bytes),
            target_replicas: TARGET_REPLICAS,
            known_replicas: known,
            status: if known >= TARGET_REPLICAS {
                "HEALTHY"
            } else {
                "DEGRADED"
            },
            providers,
        }))
    }

    fn note_replica(&self, cid_bytes: &[u8], peer: PeerId) {
        let mut r = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
        let entry = r
            .entry(cid_bytes.to_vec())
            .or_insert_with(|| (HashSet::new(), Instant::now()));
        entry.0.insert(peer);
        entry.1 = Instant::now();
    }

    // -- replication ----------------------------------------------------------

    /// One repair pass: for every complete blob, refresh which store peers
    /// hold it and push to one more if below target. Bounded work per pass
    /// so a node with many blobs spreads the effort.
    pub async fn repair_pass(&self, shared: &Arc<Shared>, max_pushes: usize) {
        let me = shared.handle.peer_id();
        let peers: Vec<PeerId> = self
            .store_peers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|p| **p != me)
            .copied()
            .collect();
        if peers.is_empty() {
            return;
        }
        let Ok(cids) = self.complete_cids() else {
            return;
        };
        let mut pushes = 0;
        for c in cids {
            // Refresh knowledge of who holds it, at most every 10 minutes.
            let stale = {
                let r = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
                r.get(&c)
                    .map(|(_, at)| at.elapsed() > Duration::from_secs(600))
                    .unwrap_or(true)
            };
            if stale {
                let mut holders = HashSet::new();
                for p in &peers {
                    if self.peer_has(shared, *p, &c).await {
                        holders.insert(*p);
                    }
                }
                let mut r = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
                r.insert(c.clone(), (holders, Instant::now()));
            }
            let (known, holders) = {
                let r = self.replicas.lock().unwrap_or_else(|e| e.into_inner());
                let h = r.get(&c).map(|(s, _)| s.clone()).unwrap_or_default();
                (h.len() as u32 + 1, h)
            };
            if known >= TARGET_REPLICAS || pushes >= max_pushes {
                continue;
            }
            // Push to a peer that does not hold it.
            if let Some(target) = peers.iter().find(|p| !holders.contains(p)) {
                match self.push_blob(shared, *target, &c).await {
                    Ok(()) => {
                        info!(cid = hex::encode(&c), peer = %target, "replicated blob");
                        self.note_replica(&c, *target);
                        pushes += 1;
                        if let (Some(agent), Some(op)) =
                            (&shared.rewards, shared.announces.operator_of(target))
                        {
                            agent.assign(self, &c, &op, known).await;
                        }
                    }
                    Err(e) => {
                        debug!(cid = hex::encode(&c), peer = %target, error = e, "replication push failed")
                    }
                }
            }
        }
    }

    async fn peer_has(&self, shared: &Arc<Shared>, peer: PeerId, c: &[u8]) -> bool {
        let req = pb::Request {
            body: Some(pb::request::Body::BlobHas(pb::BlobHas { cid: c.to_vec() })),
        };
        match shared.handle.request(peer, vec![], req).await {
            Ok(pb::Response {
                body: Some(pb::response::Body::BlobHas(r)),
            }) => r.has_manifest && r.chunks_present == r.chunks_total && r.chunks_total > 0,
            _ => false,
        }
    }

    /// Uploads a blob this node holds to another store peer, signed with
    /// this node's own key as the uploader.
    pub async fn push_blob(
        &self,
        shared: &Arc<Shared>,
        peer: PeerId,
        c: &[u8],
    ) -> Result<(), String> {
        let m = self
            .get_manifest(c)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no manifest".to_owned())?;
        let mut put = pb::BlobPutManifest {
            cid: c.to_vec(),
            manifest: Some(m.clone()),
            timestamp: now(),
            ..Default::default()
        };
        signing::sign_blob_upload(&shared.identity, &shared.announce_signer, &mut put)
            .map_err(|e| e.to_string())?;
        let resp = shared
            .handle
            .request(
                peer,
                vec![],
                pb::Request {
                    body: Some(pb::request::Body::BlobPutManifest(put)),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        let missing = match resp.body {
            Some(pb::response::Body::BlobPutManifest(r)) if r.accepted => r.missing_chunks,
            Some(pb::response::Body::BlobPutManifest(r)) => return Err(r.reason),
            Some(pb::response::Body::Error(e)) => return Err(format!("{}: {}", e.code, e.message)),
            _ => return Err("unexpected response".into()),
        };
        for index in missing {
            let Some(data) = self.get_chunk(c, index).map_err(|e| e.to_string())? else {
                return Err(format!("chunk {index} missing locally"));
            };
            let resp = shared
                .handle
                .request(
                    peer,
                    vec![],
                    pb::Request {
                        body: Some(pb::request::Body::BlobPutChunk(pb::BlobPutChunk {
                            cid: c.to_vec(),
                            index,
                            data,
                        })),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            match resp.body {
                Some(pb::response::Body::BlobPutChunk(r)) if r.accepted => {}
                Some(pb::response::Body::BlobPutChunk(r)) => return Err(r.reason),
                Some(pb::response::Body::Error(e)) => {
                    return Err(format!("{}: {}", e.code, e.message))
                }
                _ => return Err("unexpected response".into()),
            }
        }
        Ok(())
    }

    /// Fetches a blob from a peer and stores it locally, verifying every
    /// chunk. Used by the local API's `fetch` and by clients through the SDK.
    pub async fn pull_blob(
        &self,
        shared: &Arc<Shared>,
        peer: PeerId,
        addrs: Vec<Multiaddr>,
        c: &[u8],
    ) -> Result<pb::BlobManifest, String> {
        let resp = shared
            .handle
            .request(
                peer,
                addrs.clone(),
                pb::Request {
                    body: Some(pb::request::Body::BlobGetManifest(pb::BlobGetManifest {
                        cid: c.to_vec(),
                    })),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        let m = match resp.body {
            Some(pb::response::Body::BlobGetManifest(r)) if r.found => {
                r.manifest.ok_or("empty manifest")?
            }
            Some(pb::response::Body::BlobGetManifest(_)) => {
                return Err("peer does not have it".into())
            }
            Some(pb::response::Body::Error(e)) => return Err(format!("{}: {}", e.code, e.message)),
            _ => return Err("unexpected response".into()),
        };
        if cid(&m).as_slice() != c {
            shared
                .handle
                .score(peer, ScoreEvent::ServedCorruptData)
                .await;
            return Err("peer served a manifest that does not hash to the cid".into());
        }
        let missing = self
            .put_manifest(c, &m, &shared.announce_signer.public_key())
            .map_err(|e| e.to_string())?;
        for index in missing {
            let resp = shared
                .handle
                .request(
                    peer,
                    addrs.clone(),
                    pb::Request {
                        body: Some(pb::request::Body::BlobGetChunk(pb::BlobGetChunk {
                            cid: c.to_vec(),
                            index,
                        })),
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            let data = match resp.body {
                Some(pb::response::Body::BlobGetChunk(r)) if r.found => r.data,
                _ => return Err(format!("peer did not serve chunk {index}")),
            };
            if !chunk_matches(&m, index, &data) {
                shared
                    .handle
                    .score(peer, ScoreEvent::ServedCorruptData)
                    .await;
                return Err(format!("chunk {index} from peer does not match its hash"));
            }
            self.put_chunk(c, index, &data).map_err(|e| e.to_string())?;
        }
        let _ = shared
            .handle
            .start_providing(hashgram_proto::dht::blob(c))
            .await;
        self.note_replica(c, peer);
        Ok(m)
    }
}

/// Sanity: a chunk plus its frame fits the RPC bound.
const _: () = assert!(CHUNK_SIZE + 1024 < MAX_RPC_FRAME);

fn internal(e: anyhow::Error) -> pb::Response {
    warn!(error = %e, "blob storage error");
    err("internal", "storage error")
}

#[derive(Debug, thiserror::Error)]
pub enum PutError {
    /// Shape, hash or manifest mismatch.
    #[error("{0}")]
    Invalid(String),
    /// Quota.
    #[error("{0}")]
    Quota(String),
    /// Storage.
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
    use hashgram_proto::blob::manifest_for;

    fn service(quota: u64) -> Arc<BlobService> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "hg-blob-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let db = crate::store::open(&dir, "blobs").unwrap();
        BlobService::open(db, quota).unwrap()
    }

    #[test]
    fn stores_and_serves_a_multi_chunk_blob() {
        let s = service(0);
        let data: Vec<u8> = (0..(CHUNK_SIZE * 2 + 123))
            .map(|i| (i % 251) as u8)
            .collect();
        let m = manifest_for(&data, "video/mp4", false).unwrap();
        let c = s.put_local(&m, &data, &[1; 32]).unwrap();
        let (has, present, total) = s.has(&c).unwrap();
        assert!(has);
        assert_eq!((present, total), (3, 3));
        for i in 0..3u32 {
            let chunk = s.get_chunk(&c, i).unwrap().unwrap();
            assert!(chunk_matches(&m, i, &chunk));
        }
        assert!(s.get_chunk(&c, 3).unwrap().is_none());
        assert_eq!(s.complete_cids().unwrap(), vec![c.clone()]);
        assert!(s.delete(&c).unwrap());
        assert_eq!(s.stats().unwrap().bytes_used, 0);
    }

    #[test]
    fn a_corrupt_chunk_is_refused() {
        let s = service(0);
        let data = vec![5u8; 1000];
        let m = manifest_for(&data, "application/octet-stream", true).unwrap();
        let c = cid(&m).to_vec();
        assert_eq!(s.put_manifest(&c, &m, &[1; 32]).unwrap(), vec![0]);
        let mut bad = data.clone();
        bad[0] = 6;
        assert!(matches!(
            s.put_chunk(&c, 0, &bad),
            Err(PutError::Invalid(_))
        ));
        assert!(s.put_chunk(&c, 0, &data).unwrap());
        // A manifest whose cid does not match is refused.
        assert!(matches!(
            s.put_manifest(&[0; 32], &m, &[1; 32]),
            Err(PutError::Invalid(_))
        ));
    }

    #[test]
    fn resume_reports_only_missing_chunks() {
        let s = service(0);
        let data = vec![9u8; CHUNK_SIZE * 3];
        let m = manifest_for(&data, "x", false).unwrap();
        let c = cid(&m).to_vec();
        assert_eq!(s.put_manifest(&c, &m, &[1; 32]).unwrap(), vec![0, 1, 2]);
        // All three chunks are identical bytes, so storing index 0 stores
        // the one chunk hash all three share: the resume list is empty of
        // hashes we hold. Re-sending the manifest reports nothing missing
        // for indices whose chunk hash is present.
        assert!(!s.put_chunk(&c, 0, &data[..CHUNK_SIZE]).unwrap());
        assert_eq!(s.missing_chunks(&c, 3).unwrap(), vec![1, 2]);
        assert!(!s.put_chunk(&c, 1, &data[..CHUNK_SIZE]).unwrap());
        assert!(s.put_chunk(&c, 2, &data[..CHUNK_SIZE]).unwrap());
        assert_eq!(s.put_manifest(&c, &m, &[1; 32]).unwrap(), Vec::<u32>::new());
        // Quota accounting is by declared size, not physical chunks: the
        // three identical chunks are one on disk but the reservation is the
        // manifest's size, which is what the uploader is charged.
        assert_eq!(s.stats().unwrap().bytes_used, m.size);
        assert_eq!(s.stats().unwrap().bytes_used, (CHUNK_SIZE * 3) as u64);
    }

    #[test]
    fn an_incomplete_upload_expires_and_releases_quota() {
        let s = service(100_000);
        let uploader = [1u8; 32];
        // Two uploads by the same device: one finished, one abandoned after
        // the manifest and a single chunk.
        let done = vec![7u8; 3000];
        let md = manifest_for(&done, "x", false).unwrap();
        let cd = s.put_local(&md, &done, &uploader).unwrap();
        let abandoned = vec![8u8; 5000];
        let ma = manifest_for(&abandoned, "x", false).unwrap();
        let ca = cid(&ma).to_vec();
        assert_eq!(s.put_manifest(&ca, &ma, &uploader).unwrap(), vec![0]);
        let t = now();
        let st = s.stats().unwrap();
        assert_eq!((st.blobs_complete, st.blobs_partial), (1, 1));
        assert_eq!(st.bytes_used, 8000);

        // Inside the TTL nothing happens.
        assert_eq!(
            s.expire_incomplete(t + INCOMPLETE_UPLOAD_TTL_SECS).unwrap(),
            0
        );
        assert_eq!(s.stats().unwrap().blobs_partial, 1);
        // Past it, the incomplete one goes and its reservation returns; the
        // complete one is untouched however old it is.
        assert_eq!(
            s.expire_incomplete(t + INCOMPLETE_UPLOAD_TTL_SECS + 1)
                .unwrap(),
            1
        );
        assert_eq!(s.incomplete_expired.get(), 1);
        let st = s.stats().unwrap();
        assert_eq!((st.blobs_complete, st.blobs_partial), (1, 0));
        assert_eq!(st.bytes_used, 3000);
        assert!(s.get_manifest(&ca).unwrap().is_none());
        assert_eq!(s.has(&cd).unwrap(), (true, 1, 1));
        // The reservation is really free: the uploader may upload again.
        assert_eq!(s.put_manifest(&ca, &ma, &uploader).unwrap(), vec![0]);
        // Running again with nothing due is a no-op.
        assert_eq!(s.expire_incomplete(t).unwrap(), 0);
    }

    #[test]
    fn expiring_an_incomplete_upload_keeps_chunks_other_blobs_use() {
        let s = service(0);
        // Blob A is complete. Blob B shares A's first chunk and never gets
        // its second, so it expires; A's chunk must survive.
        let a = vec![1u8; CHUNK_SIZE];
        let ma = manifest_for(&a, "x", false).unwrap();
        let ca = s.put_local(&ma, &a, &[1; 32]).unwrap();
        let mut b = vec![1u8; CHUNK_SIZE];
        b.extend(vec![2u8; 10]);
        let mb = manifest_for(&b, "x", false).unwrap();
        let cb = cid(&mb).to_vec();
        // Chunk 0 is already held, so only chunk 1 is reported missing.
        assert_eq!(s.put_manifest(&cb, &mb, &[2; 32]).unwrap(), vec![1]);
        let t = now();
        assert_eq!(
            s.expire_incomplete(t + INCOMPLETE_UPLOAD_TTL_SECS + 1)
                .unwrap(),
            1
        );
        assert!(s.get_manifest(&cb).unwrap().is_none());
        assert!(s.get_chunk(&ca, 0).unwrap().is_some());
        assert_eq!(s.has(&ca).unwrap(), (true, 1, 1));
        assert_eq!(s.stats().unwrap().bytes_used, CHUNK_SIZE as u64);
    }

    #[test]
    fn quota_is_enforced_before_any_chunk() {
        let s = service(10_000);
        let data = vec![1u8; 20_000];
        let m = manifest_for(&data, "x", false).unwrap();
        assert!(matches!(
            s.put_manifest(&cid(&m), &m, &[1; 32]),
            Err(PutError::Quota(_))
        ));
        // Per-uploader share is quota/8 = 1250 bytes.
        let small = vec![2u8; 2000];
        let m2 = manifest_for(&small, "x", false).unwrap();
        assert!(matches!(
            s.put_manifest(&cid(&m2), &m2, &[1; 32]),
            Err(PutError::Quota(_))
        ));
        let tiny = vec![3u8; 1000];
        let m3 = manifest_for(&tiny, "x", false).unwrap();
        s.put_manifest(&cid(&m3), &m3, &[1; 32]).unwrap();
    }

    #[test]
    fn one_copy_is_reported_degraded() {
        let s = service(0);
        let data = vec![4u8; 10];
        let m = manifest_for(&data, "x", false).unwrap();
        let c = s.put_local(&m, &data, &[1; 32]).unwrap();
        let me = PeerId::random();
        let h = s.health(&c, me).unwrap().unwrap();
        assert_eq!(h.status, "DEGRADED");
        assert_eq!(h.known_replicas, 1);
        s.note_replica(&c, PeerId::random());
        s.note_replica(&c, PeerId::random());
        let h = s.health(&c, me).unwrap().unwrap();
        assert_eq!(h.status, "HEALTHY");
        assert_eq!(h.known_replicas, 3);
    }
}
