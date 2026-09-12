//! HashDrive: encrypted objects, the encrypted manifest tree, deterministic
//! multi-device merge, and sharing capabilities.
//!
//! # Objects
//!
//! A file becomes a *Drive object*: the plaintext is cut into segments of
//! [`SEGMENT_PLAINTEXT`] bytes and each segment is sealed independently with
//! XChaCha20-Poly1305 under one random 256-bit key. Segment `i` uses
//! `nonce_i = base_nonce ⊕ le64(i)` (in the last eight bytes) and AAD
//! `"hashgram-drive-v1" ‖ le64(i) ‖ le64(total_plaintext_size)`. Every
//! sealed segment is exactly one blob chunk (1 MiB), so:
//!
//! * encryption and decryption stream one segment at a time (a 4 GiB file
//!   never needs 4 GiB of memory),
//! * a provider cannot reorder, drop, duplicate or truncate segments without
//!   failing authentication (index and total size are in the AAD),
//! * uploads and downloads resume at chunk granularity through the existing
//!   blob protocol.
//!
//! The provider stores ciphertext under a CID that names the ciphertext. The
//! key travels only inside MLS (`DriveObjectRef` in a manifest, a capability
//! or a mail attachment).
//!
//! # Manifest
//!
//! [`Manifest`] wraps `DriveManifest`: a flat list of entries with parent
//! pointers (folders are entries). It is itself sealed as one Drive object
//! under the *manifest key* before upload. Devices that write concurrently
//! converge through [`merge`], which is commutative and associative for the
//! per-entry fields it decides on.
//!
//! # Capabilities
//!
//! A [`pb::DriveCapability`] carries the object key; holding it is the
//! permission. See `docs/HASHDRIVE.md` for revocation semantics.

use std::collections::{BTreeMap, BTreeSet};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hashgram_proto::blob::{cid, manifest_for};
use hashgram_proto::limits::CHUNK_SIZE;
use prost::Message;

use crate::ids::{
    now_ms, random_bytes, random_id, require_address, require_hash, require_id,
    require_id_or_empty, require_str, require_str_max,
};
use crate::pb;
use crate::version::{check_body, CAPABILITY_VERSION, DRIVE_VERSION};
use crate::AppError;

/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;
/// Plaintext bytes per segment: one blob chunk minus the tag.
pub const SEGMENT_PLAINTEXT: usize = CHUNK_SIZE - TAG_LEN;
/// AAD domain prefix.
pub const AAD_DOMAIN: &[u8] = b"hashgram-drive-v1";
/// Largest plaintext a Drive object may hold (fits the 4 GiB blob bound
/// after tags).
pub const MAX_OBJECT_PLAINTEXT: u64 = 4 * 1024 * 1024 * 1024 - 4096 * TAG_LEN as u64;
/// Versions kept per entry before the oldest is evicted.
pub const MAX_VERSIONS_PER_ENTRY: usize = 50;
/// Entries per manifest (a bound against a runaway device, not a product
/// limit: a manifest this size is ~100 MiB sealed).
pub const MAX_ENTRIES: usize = 500_000;
/// Entry name bytes.
pub const MAX_NAME: usize = 255;
/// MIME bytes.
pub const MAX_MIME: usize = 128;
/// Attribute key/value bytes.
pub const MAX_ATTR: usize = 1024;
/// Attributes per entry.
pub const MAX_ATTRS: usize = 32;
/// Tombstones kept.
pub const MAX_TOMBSTONES: usize = 10_000;

// ---------------------------------------------------------------------------
// Object encryption
// ---------------------------------------------------------------------------

/// Key material for one object.
#[derive(Clone, PartialEq, Eq)]
pub struct ObjectKey {
    /// 32 bytes.
    pub key: [u8; 32],
    /// 24-byte base nonce.
    pub base_nonce: [u8; 24],
}

impl std::fmt::Debug for ObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectKey").finish_non_exhaustive()
    }
}

impl ObjectKey {
    /// A fresh random key.
    pub fn generate() -> Result<Self, AppError> {
        let k = random_bytes(32)?;
        let n = random_bytes(24)?;
        let mut key = [0u8; 32];
        let mut base_nonce = [0u8; 24];
        key.copy_from_slice(&k);
        base_nonce.copy_from_slice(&n);
        Ok(Self { key, base_nonce })
    }

    /// From the wire type.
    pub fn from_pb(k: &pb::DriveKey) -> Result<Self, AppError> {
        if k.key.len() != 32 || k.base_nonce.len() != 24 {
            return Err(AppError::Invalid("DriveKey length".into()));
        }
        let mut key = [0u8; 32];
        let mut base_nonce = [0u8; 24];
        key.copy_from_slice(&k.key);
        base_nonce.copy_from_slice(&k.base_nonce);
        Ok(Self { key, base_nonce })
    }

    /// To the wire type.
    #[must_use]
    pub fn to_pb(&self) -> pb::DriveKey {
        pb::DriveKey {
            key: self.key.to_vec(),
            base_nonce: self.base_nonce.to_vec(),
        }
    }

    fn nonce(&self, index: u64) -> XNonce {
        let mut n = self.base_nonce;
        let ix = index.to_le_bytes();
        for (a, b) in n.iter_mut().skip(16).zip(ix.iter()) {
            *a ^= b;
        }
        XNonce::from(n)
    }
}

fn aad(index: u64, total: u64) -> Vec<u8> {
    let mut a = Vec::with_capacity(AAD_DOMAIN.len() + 16);
    a.extend_from_slice(AAD_DOMAIN);
    a.extend_from_slice(&index.to_le_bytes());
    a.extend_from_slice(&total.to_le_bytes());
    a
}

/// Number of segments for a plaintext size (at least one, so an empty file
/// is still an authenticated object).
#[must_use]
pub fn segment_count(size: u64) -> u64 {
    if size == 0 {
        1
    } else {
        size.div_ceil(SEGMENT_PLAINTEXT as u64)
    }
}

/// Ciphertext size for a plaintext size.
#[must_use]
pub fn ciphertext_size(size: u64) -> u64 {
    size + segment_count(size) * TAG_LEN as u64
}

/// Number of sealed segments in a ciphertext of `ct_len` bytes (one per
/// blob chunk).
#[must_use]
pub fn segment_count_for_ciphertext(ct_len: u64) -> u64 {
    ct_len.div_ceil(CHUNK_SIZE as u64).max(1)
}

/// Opens a sealed object when only the key is known (e.g. a manifest whose
/// reference another device has not sent yet): the plaintext size is
/// derived from the ciphertext length. The caller verifies content by
/// decoding it; there is no plaintext hash to compare against.
pub fn open_object_with_key(ciphertext: &[u8], key: ObjectKey) -> Result<Vec<u8>, AppError> {
    let ct_len = ciphertext.len() as u64;
    let segs = segment_count_for_ciphertext(ct_len);
    let plaintext_size = ct_len
        .checked_sub(segs * TAG_LEN as u64)
        .ok_or_else(|| AppError::Integrity("ciphertext shorter than its tags".into()))?;
    let mut dec = SegmentDecryptor::new(key, plaintext_size)?;
    let mut out = Vec::with_capacity(plaintext_size as usize);
    for chunk in ciphertext.chunks(CHUNK_SIZE) {
        out.extend_from_slice(&dec.open(chunk)?);
    }
    if !dec.finished() {
        return Err(AppError::Integrity("object is incomplete".into()));
    }
    Ok(out)
}

/// Streaming encryptor. Feed plaintext segments in order (each exactly
/// [`SEGMENT_PLAINTEXT`] bytes except the last), get one sealed chunk each.
pub struct SegmentEncryptor {
    cipher: XChaCha20Poly1305,
    key: ObjectKey,
    total: u64,
    next: u64,
    count: u64,
    hasher: blake3::Hasher,
    consumed: u64,
}

impl SegmentEncryptor {
    /// Starts an encryption of `total_size` plaintext bytes.
    pub fn new(key: ObjectKey, total_size: u64) -> Result<Self, AppError> {
        if total_size > MAX_OBJECT_PLAINTEXT {
            return Err(AppError::Invalid("object too large".into()));
        }
        Ok(Self {
            cipher: XChaCha20Poly1305::new((&key.key).into()),
            count: segment_count(total_size),
            key,
            total: total_size,
            next: 0,
            hasher: blake3::Hasher::new(),
            consumed: 0,
        })
    }

    /// Seals the next segment.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
        if self.next >= self.count {
            return Err(AppError::Invalid("all segments already sealed".into()));
        }
        let is_last = self.next + 1 == self.count;
        let expected = if is_last {
            (self.total - self.consumed) as usize
        } else {
            SEGMENT_PLAINTEXT
        };
        if plaintext.len() != expected {
            return Err(AppError::Invalid(format!(
                "segment {} must be {expected} bytes, got {}",
                self.next,
                plaintext.len()
            )));
        }
        self.hasher.update(plaintext);
        let ct = self
            .cipher
            .encrypt(
                &self.key.nonce(self.next),
                Payload {
                    msg: plaintext,
                    aad: &aad(self.next, self.total),
                },
            )
            .map_err(|_| AppError::Integrity("encryption failed".into()))?;
        self.next += 1;
        self.consumed += plaintext.len() as u64;
        Ok(ct)
    }

    /// True when every segment has been sealed.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.next == self.count
    }

    /// BLAKE3 of the plaintext fed so far (the object's `plaintext_hash`
    /// once [`Self::finished`]).
    #[must_use]
    pub fn plaintext_hash(&self) -> [u8; 32] {
        *self.hasher.finalize().as_bytes()
    }
}

/// Streaming decryptor: feed sealed chunks in order.
pub struct SegmentDecryptor {
    cipher: XChaCha20Poly1305,
    key: ObjectKey,
    total: u64,
    next: u64,
    count: u64,
    hasher: blake3::Hasher,
    produced: u64,
}

impl SegmentDecryptor {
    /// Starts a decryption for an object of `total_size` plaintext bytes.
    pub fn new(key: ObjectKey, total_size: u64) -> Result<Self, AppError> {
        if total_size > MAX_OBJECT_PLAINTEXT {
            return Err(AppError::Invalid("object too large".into()));
        }
        Ok(Self {
            cipher: XChaCha20Poly1305::new((&key.key).into()),
            count: segment_count(total_size),
            key,
            total: total_size,
            next: 0,
            hasher: blake3::Hasher::new(),
            produced: 0,
        })
    }

    /// Opens the next sealed chunk.
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, AppError> {
        if self.next >= self.count {
            return Err(AppError::Integrity(
                "more segments than the object declares".into(),
            ));
        }
        let pt = self
            .cipher
            .decrypt(
                &self.key.nonce(self.next),
                Payload {
                    msg: ciphertext,
                    aad: &aad(self.next, self.total),
                },
            )
            .map_err(|_| {
                AppError::Integrity(format!(
                    "segment {} failed to authenticate (wrong key, reordered or altered)",
                    self.next
                ))
            })?;
        let is_last = self.next + 1 == self.count;
        let expected = if is_last {
            (self.total - self.produced) as usize
        } else {
            SEGMENT_PLAINTEXT
        };
        if pt.len() != expected {
            return Err(AppError::Integrity(format!(
                "segment {} has {} plaintext bytes, expected {expected}",
                self.next,
                pt.len()
            )));
        }
        self.hasher.update(&pt);
        self.next += 1;
        self.produced += pt.len() as u64;
        Ok(pt)
    }

    /// True when every segment has been opened.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.next == self.count
    }

    /// Verifies the plaintext hash once finished.
    pub fn verify(&self, expected_plaintext_hash: &[u8]) -> Result<(), AppError> {
        if !self.finished() {
            return Err(AppError::Integrity("object is incomplete".into()));
        }
        if self.hasher.finalize().as_bytes() != expected_plaintext_hash {
            return Err(AppError::Integrity("plaintext hash mismatch".into()));
        }
        Ok(())
    }
}

/// Seals a whole in-memory plaintext. Returns the ciphertext and the
/// object reference (CID computed over the ciphertext as the blob layer
/// will see it).
pub fn seal_object(plaintext: &[u8]) -> Result<(Vec<u8>, pb::DriveObjectRef), AppError> {
    let key = ObjectKey::generate()?;
    seal_object_with(plaintext, key)
}

/// [`seal_object`] with a caller-provided key (used to re-seal the
/// manifest under the manifest key).
pub fn seal_object_with(
    plaintext: &[u8],
    key: ObjectKey,
) -> Result<(Vec<u8>, pb::DriveObjectRef), AppError> {
    let total = plaintext.len() as u64;
    let mut enc = SegmentEncryptor::new(key.clone(), total)?;
    let mut out = Vec::with_capacity(ciphertext_size(total) as usize);
    if plaintext.is_empty() {
        out.extend_from_slice(&enc.seal(&[])?);
    } else {
        for seg in plaintext.chunks(SEGMENT_PLAINTEXT) {
            out.extend_from_slice(&enc.seal(seg)?);
        }
    }
    let r = object_ref(&out, &key, total, enc.plaintext_hash())?;
    Ok((out, r))
}

/// Builds the reference for sealed bytes.
pub fn object_ref(
    ciphertext: &[u8],
    key: &ObjectKey,
    plaintext_size: u64,
    plaintext_hash: [u8; 32],
) -> Result<pb::DriveObjectRef, AppError> {
    let m = manifest_for(ciphertext, "application/octet-stream", true)
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    Ok(pb::DriveObjectRef {
        version: DRIVE_VERSION,
        cid: cid(&m).to_vec(),
        key: Some(key.to_pb()),
        size: plaintext_size,
        segment_size: SEGMENT_PLAINTEXT as u32,
        plaintext_hash: plaintext_hash.to_vec(),
    })
}

/// Opens a whole in-memory ciphertext against its reference, verifying the
/// plaintext hash.
pub fn open_object(ciphertext: &[u8], r: &pb::DriveObjectRef) -> Result<Vec<u8>, AppError> {
    validate_object_ref(r)?;
    let key = ObjectKey::from_pb(
        r.key
            .as_ref()
            .ok_or_else(|| AppError::Invalid("object ref has no key".into()))?,
    )?;
    if ciphertext.len() as u64 != ciphertext_size(r.size) {
        return Err(AppError::Integrity(
            "ciphertext length does not match the object".into(),
        ));
    }
    let mut dec = SegmentDecryptor::new(key, r.size)?;
    let mut out = Vec::with_capacity(r.size as usize);
    for chunk in ciphertext.chunks(CHUNK_SIZE) {
        out.extend_from_slice(&dec.open(chunk)?);
    }
    dec.verify(&r.plaintext_hash)?;
    Ok(out)
}

/// Validates a reference's shape.
pub fn validate_object_ref(r: &pb::DriveObjectRef) -> Result<(), AppError> {
    check_body("DriveObjectRef", r.version, DRIVE_VERSION)?;
    require_hash("DriveObjectRef.cid", &r.cid)?;
    let k = r
        .key
        .as_ref()
        .ok_or_else(|| AppError::Invalid("DriveObjectRef.key missing".into()))?;
    if k.key.len() != 32 || k.base_nonce.len() != 24 {
        return Err(AppError::Invalid("DriveObjectRef.key length".into()));
    }
    if r.segment_size as usize != SEGMENT_PLAINTEXT {
        return Err(AppError::Unsupported(format!(
            "segment size {} (this build uses {SEGMENT_PLAINTEXT})",
            r.segment_size
        )));
    }
    if r.size > MAX_OBJECT_PLAINTEXT {
        return Err(AppError::Invalid("object too large".into()));
    }
    require_hash("DriveObjectRef.plaintext_hash", &r.plaintext_hash)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

/// The Drive tree with the invariants held.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    inner: pb::DriveManifest,
}

/// A listing row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntryView {
    /// Hex id.
    pub id: String,
    /// Hex parent id ("" for root children).
    pub parent_id: String,
    /// "file" | "folder".
    pub kind: &'static str,
    /// Name.
    pub name: String,
    /// MIME.
    pub mime: String,
    /// Plaintext size.
    pub size: u64,
    /// Created.
    pub created_at_ms: u64,
    /// Modified.
    pub modified_at_ms: u64,
    /// Versions kept (files).
    pub versions: usize,
    /// Trashed.
    pub trashed: bool,
    /// Starred.
    pub starred: bool,
    /// Path from root.
    pub path: String,
}

impl Manifest {
    /// A new empty Drive for `device`.
    pub fn new(device_pubkey: &[u8]) -> Result<Self, AppError> {
        Ok(Self {
            inner: pb::DriveManifest {
                version: DRIVE_VERSION,
                drive_id: random_id()?,
                revision: 1,
                updated_at_ms: now_ms(),
                device_pubkey: device_pubkey.to_vec(),
                entries: Vec::new(),
                shares: Vec::new(),
                tombstones: Vec::new(),
            },
        })
    }

    /// Wraps and validates a decoded manifest.
    pub fn from_pb(m: pb::DriveManifest) -> Result<Self, AppError> {
        check_body("DriveManifest", m.version, DRIVE_VERSION)?;
        require_id("drive_id", &m.drive_id)?;
        if m.entries.len() > MAX_ENTRIES {
            return Err(AppError::Invalid("too many entries".into()));
        }
        let mut ids = BTreeSet::new();
        for e in &m.entries {
            validate_entry(e)?;
            if !ids.insert(e.id.clone()) {
                return Err(AppError::Invalid("duplicate entry id".into()));
            }
        }
        for e in &m.entries {
            if !e.parent_id.is_empty() {
                let p = m.entries.iter().find(|x| x.id == e.parent_id);
                match p {
                    Some(p) if p.kind == pb::DriveEntryKind::Folder as i32 => {}
                    _ => {
                        return Err(AppError::Invalid(format!(
                            "entry {} has a parent that is not a folder",
                            hex::encode(&e.id)
                        )))
                    }
                }
            }
        }
        for s in &m.shares {
            validate_share_record(s)?;
        }
        if m.tombstones.len() > MAX_TOMBSTONES {
            return Err(AppError::Invalid("too many tombstones".into()));
        }
        for t in &m.tombstones {
            require_id("tombstone.id", &t.id)?;
        }
        let out = Self { inner: m };
        out.check_acyclic()?;
        Ok(out)
    }

    fn check_acyclic(&self) -> Result<(), AppError> {
        for e in &self.inner.entries {
            let mut seen = 0usize;
            let mut cur = e.parent_id.clone();
            while !cur.is_empty() {
                seen += 1;
                if seen > self.inner.entries.len() {
                    return Err(AppError::Invalid("folder cycle".into()));
                }
                cur = self
                    .get(&cur)
                    .map(|p| p.parent_id.clone())
                    .unwrap_or_default();
            }
        }
        Ok(())
    }

    /// Decodes and validates.
    pub fn decode(bytes: &[u8]) -> Result<Self, AppError> {
        Self::from_pb(pb::DriveManifest::decode(bytes)?)
    }

    /// Encodes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.inner.encode_to_vec()
    }

    /// The wire type.
    #[must_use]
    pub fn as_pb(&self) -> &pb::DriveManifest {
        &self.inner
    }

    /// Drive id.
    #[must_use]
    pub fn drive_id(&self) -> &[u8] {
        &self.inner.drive_id
    }

    /// Revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.inner.revision
    }

    /// Seals for upload under the manifest key.
    pub fn seal(
        &self,
        manifest_key: &ObjectKey,
    ) -> Result<(Vec<u8>, pb::DriveObjectRef), AppError> {
        seal_object_with(&self.encode(), manifest_key.clone())
    }

    /// Opens a sealed manifest.
    pub fn open_sealed(ciphertext: &[u8], r: &pb::DriveObjectRef) -> Result<Self, AppError> {
        Self::decode(&open_object(ciphertext, r)?)
    }

    fn bump(&mut self, device: &[u8]) {
        self.inner.revision += 1;
        self.inner.updated_at_ms = now_ms();
        self.inner.device_pubkey = device.to_vec();
    }

    /// Entry by id.
    #[must_use]
    pub fn get(&self, id: &[u8]) -> Option<&pb::DriveEntry> {
        self.inner.entries.iter().find(|e| e.id == id)
    }

    fn get_mut(&mut self, id: &[u8]) -> Result<&mut pb::DriveEntry, AppError> {
        self.inner
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| AppError::NotFound(format!("entry {}", hex::encode(id))))
    }

    fn require_folder(&self, id: &[u8]) -> Result<(), AppError> {
        if id.is_empty() {
            return Ok(());
        }
        match self.get(id) {
            Some(e) if e.kind == pb::DriveEntryKind::Folder as i32 && !e.trashed => Ok(()),
            Some(_) => Err(AppError::Invalid("parent is not an active folder".into())),
            None => Err(AppError::NotFound(format!("folder {}", hex::encode(id)))),
        }
    }

    fn require_free_name(&self, parent: &[u8], name: &str, except: &[u8]) -> Result<(), AppError> {
        require_str("name", name, MAX_NAME)?;
        if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
            return Err(AppError::Invalid("name contains a path separator".into()));
        }
        if self.inner.entries.iter().any(|e| {
            e.parent_id == parent
                && !e.trashed
                && e.id != except
                && e.name.eq_ignore_ascii_case(name)
        }) {
            return Err(AppError::Invalid(format!(
                "an entry named {name:?} already exists here"
            )));
        }
        Ok(())
    }

    /// Creates a folder. Returns its id.
    pub fn mkdir(&mut self, parent: &[u8], name: &str, device: &[u8]) -> Result<Vec<u8>, AppError> {
        self.require_folder(parent)?;
        self.require_free_name(parent, name, &[])?;
        if self.inner.entries.len() >= MAX_ENTRIES {
            return Err(AppError::Invalid("drive is full (entry limit)".into()));
        }
        let id = random_id()?;
        let t = now_ms();
        self.inner.entries.push(pb::DriveEntry {
            id: id.clone(),
            parent_id: parent.to_vec(),
            kind: pb::DriveEntryKind::Folder as i32,
            name: name.to_owned(),
            created_at_ms: t,
            modified_at_ms: t,
            ..Default::default()
        });
        self.bump(device);
        Ok(id)
    }

    /// Adds a file whose bytes are already sealed and uploaded.
    pub fn add_file(
        &mut self,
        parent: &[u8],
        name: &str,
        mime: &str,
        object: pb::DriveObjectRef,
        device: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        self.require_folder(parent)?;
        self.require_free_name(parent, name, &[])?;
        require_str_max("mime", mime, MAX_MIME)?;
        validate_object_ref(&object)?;
        if self.inner.entries.len() >= MAX_ENTRIES {
            return Err(AppError::Invalid("drive is full (entry limit)".into()));
        }
        let id = random_id()?;
        let t = now_ms();
        self.inner.entries.push(pb::DriveEntry {
            id: id.clone(),
            parent_id: parent.to_vec(),
            kind: pb::DriveEntryKind::File as i32,
            name: name.to_owned(),
            mime: mime.to_owned(),
            size: object.size,
            created_at_ms: t,
            modified_at_ms: t,
            current: Some(object),
            ..Default::default()
        });
        self.bump(device);
        Ok(id)
    }

    /// Replaces a file's content; the previous content becomes a version.
    pub fn update_file(
        &mut self,
        id: &[u8],
        object: pb::DriveObjectRef,
        device: &[u8],
        note: &str,
    ) -> Result<u64, AppError> {
        validate_object_ref(&object)?;
        require_str_max("note", note, 256)?;
        let e = self.get_mut(id)?;
        if e.kind != pb::DriveEntryKind::File as i32 {
            return Err(AppError::Invalid("not a file".into()));
        }
        let prev_no = e.versions.last().map(|v| v.version_no).unwrap_or(0);
        if let Some(prev) = e.current.take() {
            e.versions.push(pb::DriveVersion {
                version_no: prev_no + 1,
                object: Some(prev),
                created_at_ms: e.modified_at_ms,
                device_pubkey: device.to_vec(),
                note: note.to_owned(),
            });
            while e.versions.len() > MAX_VERSIONS_PER_ENTRY {
                e.versions.remove(0);
            }
        }
        e.size = object.size;
        e.current = Some(object);
        e.modified_at_ms = now_ms();
        let current_no = prev_no + 2;
        self.bump(device);
        Ok(current_no)
    }

    /// Current version number of a file (1 for a never-updated file).
    #[must_use]
    pub fn version_no(&self, id: &[u8]) -> u64 {
        self.get(id)
            .map(|e| e.versions.last().map(|v| v.version_no).unwrap_or(0) + 1)
            .unwrap_or(0)
    }

    /// Restores an older version as the current content (the current one is
    /// kept as a version, nothing is lost).
    pub fn restore_version(
        &mut self,
        id: &[u8],
        version_no: u64,
        device: &[u8],
    ) -> Result<(), AppError> {
        let obj = {
            let e = self
                .get(id)
                .ok_or_else(|| AppError::NotFound("entry".into()))?;
            e.versions
                .iter()
                .find(|v| v.version_no == version_no)
                .and_then(|v| v.object.clone())
                .ok_or_else(|| AppError::NotFound(format!("version {version_no}")))?
        };
        self.update_file(id, obj, device, &format!("restored v{version_no}"))?;
        Ok(())
    }

    /// Renames.
    pub fn rename(&mut self, id: &[u8], name: &str, device: &[u8]) -> Result<(), AppError> {
        let parent = self
            .get(id)
            .ok_or_else(|| AppError::NotFound("entry".into()))?
            .parent_id
            .clone();
        self.require_free_name(&parent, name, id)?;
        let e = self.get_mut(id)?;
        e.name = name.to_owned();
        e.modified_at_ms = now_ms();
        self.bump(device);
        Ok(())
    }

    /// Moves into another folder.
    pub fn mv(&mut self, id: &[u8], new_parent: &[u8], device: &[u8]) -> Result<(), AppError> {
        self.require_folder(new_parent)?;
        // Cannot move a folder into itself or its descendants.
        let mut cur = new_parent.to_vec();
        while !cur.is_empty() {
            if cur == id {
                return Err(AppError::Invalid("cannot move a folder into itself".into()));
            }
            cur = self
                .get(&cur)
                .map(|p| p.parent_id.clone())
                .unwrap_or_default();
        }
        let name = self
            .get(id)
            .ok_or_else(|| AppError::NotFound("entry".into()))?
            .name
            .clone();
        self.require_free_name(new_parent, &name, id)?;
        let e = self.get_mut(id)?;
        e.parent_id = new_parent.to_vec();
        e.modified_at_ms = now_ms();
        self.bump(device);
        Ok(())
    }

    /// Copies a file (same object, no re-upload) into a folder.
    pub fn copy_file(
        &mut self,
        id: &[u8],
        new_parent: &[u8],
        name: &str,
        device: &[u8],
    ) -> Result<Vec<u8>, AppError> {
        let src = self
            .get(id)
            .ok_or_else(|| AppError::NotFound("entry".into()))?
            .clone();
        if src.kind != pb::DriveEntryKind::File as i32 {
            return Err(AppError::Invalid("only files can be copied".into()));
        }
        let obj = src
            .current
            .ok_or_else(|| AppError::Invalid("file has no content".into()))?;
        let new_id = self.add_file(new_parent, name, &src.mime, obj, device)?;
        if let Ok(e) = self.get_mut(&new_id) {
            e.attrs = src.attrs;
        }
        Ok(new_id)
    }

    /// Trashes (recursively for folders).
    pub fn trash(&mut self, id: &[u8], device: &[u8]) -> Result<(), AppError> {
        let ids = self.subtree(id);
        if ids.is_empty() {
            return Err(AppError::NotFound("entry".into()));
        }
        let t = now_ms();
        for i in ids {
            if let Ok(e) = self.get_mut(&i) {
                e.trashed = true;
                e.trashed_at_ms = t;
            }
        }
        self.bump(device);
        Ok(())
    }

    /// Restores from trash (recursively).
    pub fn restore(&mut self, id: &[u8], device: &[u8]) -> Result<(), AppError> {
        let entry = self
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::NotFound("entry".into()))?;
        // The parent must be active; otherwise restore to root.
        let parent_ok = entry.parent_id.is_empty()
            || self
                .get(&entry.parent_id)
                .map(|p| !p.trashed)
                .unwrap_or(false);
        let ids = self.subtree(id);
        for i in ids {
            if let Ok(e) = self.get_mut(&i) {
                e.trashed = false;
                e.trashed_at_ms = 0;
            }
        }
        if !parent_ok {
            let e = self.get_mut(id)?;
            e.parent_id = Vec::new();
        }
        let name = self.get(id).map(|e| e.name.clone()).unwrap_or_default();
        let parent = self
            .get(id)
            .map(|e| e.parent_id.clone())
            .unwrap_or_default();
        if self.require_free_name(&parent, &name, id).is_err() {
            let e = self.get_mut(id)?;
            e.name = format!("{name} (restored {})", now_ms());
        }
        self.bump(device);
        Ok(())
    }

    /// Permanently deletes (recursively). Only trashed entries may be
    /// deleted, so a deletion is always a two-step act.
    pub fn delete(&mut self, id: &[u8], device: &[u8]) -> Result<usize, AppError> {
        let e = self
            .get(id)
            .ok_or_else(|| AppError::NotFound("entry".into()))?;
        if !e.trashed {
            return Err(AppError::Invalid(
                "entry must be trashed before it is deleted".into(),
            ));
        }
        let ids: BTreeSet<Vec<u8>> = self.subtree(id).into_iter().collect();
        let before = self.inner.entries.len();
        self.inner.entries.retain(|e| !ids.contains(&e.id));
        let t = now_ms();
        for i in &ids {
            self.inner.tombstones.push(pb::DriveTombstone {
                id: i.clone(),
                at_ms: t,
            });
        }
        while self.inner.tombstones.len() > MAX_TOMBSTONES {
            self.inner.tombstones.remove(0);
        }
        for s in &mut self.inner.shares {
            if ids.contains(&s.entry_id) && !s.revoked {
                s.revoked = true;
                s.revoked_at_ms = now_ms();
            }
        }
        self.bump(device);
        Ok(before - self.inner.entries.len())
    }

    /// Empties the trash. Returns removed entry count.
    pub fn empty_trash(&mut self, device: &[u8]) -> usize {
        let roots: Vec<Vec<u8>> = self
            .inner
            .entries
            .iter()
            .filter(|e| e.trashed)
            .map(|e| e.id.clone())
            .collect();
        let mut n = 0;
        for r in roots {
            if self.get(&r).is_some() {
                n += self.delete(&r, device).unwrap_or(0);
            }
        }
        n
    }

    /// Star / unstar.
    pub fn set_starred(&mut self, id: &[u8], starred: bool, device: &[u8]) -> Result<(), AppError> {
        let e = self.get_mut(id)?;
        e.starred = starred;
        self.bump(device);
        Ok(())
    }

    /// Sets an attribute.
    pub fn set_attr(
        &mut self,
        id: &[u8],
        key: &str,
        value: &str,
        device: &[u8],
    ) -> Result<(), AppError> {
        require_str("attr key", key, 64)?;
        require_str_max("attr value", value, MAX_ATTR)?;
        let e = self.get_mut(id)?;
        if e.attrs.len() >= MAX_ATTRS && !e.attrs.contains_key(key) {
            return Err(AppError::Invalid("too many attributes".into()));
        }
        e.attrs.insert(key.to_owned(), value.to_owned());
        self.bump(device);
        Ok(())
    }

    /// Ids of an entry and everything under it.
    #[must_use]
    pub fn subtree(&self, id: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if self.get(id).is_none() {
            return out;
        }
        let mut stack = vec![id.to_vec()];
        while let Some(cur) = stack.pop() {
            out.push(cur.clone());
            for e in &self.inner.entries {
                if e.parent_id == cur {
                    stack.push(e.id.clone());
                }
            }
            if out.len() > MAX_ENTRIES {
                break;
            }
        }
        out
    }

    /// Children of a folder (root when `parent` is empty), sorted folders
    /// first then by name, excluding trashed unless `trashed` is set (then
    /// only trashed roots are returned).
    #[must_use]
    pub fn list(&self, parent: &[u8], trashed: bool) -> Vec<EntryView> {
        let mut v: Vec<EntryView> = self
            .inner
            .entries
            .iter()
            .filter(|e| {
                if trashed {
                    // Trash view: entries whose parent is not trashed (roots
                    // of trashed subtrees).
                    e.trashed
                        && (e.parent_id.is_empty()
                            || self.get(&e.parent_id).map(|p| !p.trashed).unwrap_or(true))
                } else {
                    e.parent_id == parent && !e.trashed
                }
            })
            .map(|e| self.view(e))
            .collect();
        v.sort_by(|a, b| {
            (a.kind != "folder")
                .cmp(&(b.kind != "folder"))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        v
    }

    /// Starred entries.
    #[must_use]
    pub fn starred(&self) -> Vec<EntryView> {
        self.inner
            .entries
            .iter()
            .filter(|e| e.starred && !e.trashed)
            .map(|e| self.view(e))
            .collect()
    }

    /// Simple name search (case-insensitive substring), bounded.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<EntryView> {
        let q = query.to_lowercase();
        self.inner
            .entries
            .iter()
            .filter(|e| !e.trashed && e.name.to_lowercase().contains(&q))
            .take(limit)
            .map(|e| self.view(e))
            .collect()
    }

    /// Full path of an entry.
    #[must_use]
    pub fn path(&self, id: &[u8]) -> String {
        let mut parts = Vec::new();
        let mut cur = id.to_vec();
        let mut guard = 0;
        while let Some(e) = self.get(&cur) {
            parts.push(e.name.clone());
            cur = e.parent_id.clone();
            guard += 1;
            if cur.is_empty() || guard > 4096 {
                break;
            }
        }
        parts.reverse();
        format!("/{}", parts.join("/"))
    }

    /// Resolves a `/a/b/c` path to an entry id.
    #[must_use]
    pub fn resolve_path(&self, path: &str) -> Option<Vec<u8>> {
        let mut parent: Vec<u8> = Vec::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let e = self.inner.entries.iter().find(|e| {
                e.parent_id == parent && !e.trashed && e.name.eq_ignore_ascii_case(part)
            })?;
            parent = e.id.clone();
        }
        if parent.is_empty() {
            None
        } else {
            Some(parent)
        }
    }

    /// Entry view.
    #[must_use]
    pub fn view(&self, e: &pb::DriveEntry) -> EntryView {
        EntryView {
            id: hex::encode(&e.id),
            parent_id: hex::encode(&e.parent_id),
            kind: if e.kind == pb::DriveEntryKind::Folder as i32 {
                "folder"
            } else {
                "file"
            },
            name: e.name.clone(),
            mime: e.mime.clone(),
            size: e.size,
            created_at_ms: e.created_at_ms,
            modified_at_ms: e.modified_at_ms,
            versions: e.versions.len() + usize::from(e.current.is_some()),
            trashed: e.trashed,
            starred: e.starred,
            path: self.path(&e.id),
        }
    }

    /// Total plaintext bytes of current file contents (not versions).
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.inner
            .entries
            .iter()
            .filter(|e| !e.trashed && e.kind == pb::DriveEntryKind::File as i32)
            .map(|e| e.size)
            .sum()
    }

    /// Every entry (for sync/backup).
    #[must_use]
    pub fn entries(&self) -> &[pb::DriveEntry] {
        &self.inner.entries
    }

    /// Every share record.
    #[must_use]
    pub fn shares(&self) -> &[pb::DriveShareRecord] {
        &self.inner.shares
    }

    // --- Sharing -----------------------------------------------------------

    /// Grants a capability to an entry and records the share. Returns the
    /// capability to send (inside MLS) and the share id.
    #[allow(clippy::too_many_arguments)]
    pub fn grant(
        &mut self,
        entry_id: &[u8],
        owner: &str,
        grantee: &str,
        group_id_hex: &str,
        mode: pb::DriveShareMode,
        permission: pb::DrivePermission,
        folder_object: Option<pb::DriveObjectRef>,
        device: &[u8],
    ) -> Result<pb::DriveCapability, AppError> {
        require_address("owner", owner)?;
        require_str("grantee", grantee, 128)?;
        let e = self
            .get(entry_id)
            .ok_or_else(|| AppError::NotFound("entry".into()))?
            .clone();
        if e.trashed {
            return Err(AppError::Invalid("cannot share a trashed entry".into()));
        }
        let is_folder = e.kind == pb::DriveEntryKind::Folder as i32;
        let object = if is_folder {
            folder_object.ok_or_else(|| {
                AppError::Invalid("a folder share needs its sealed folder manifest".into())
            })?
        } else {
            e.current
                .clone()
                .ok_or_else(|| AppError::Invalid("file has no content".into()))?
        };
        let share_id = random_id()?;
        let cap = pb::DriveCapability {
            version: CAPABILITY_VERSION,
            share_id: share_id.clone(),
            owner: owner.to_owned(),
            entry_id: e.id.clone(),
            name: e.name.clone(),
            mime: e.mime.clone(),
            size: e.size,
            object: Some(object),
            mode: mode as i32,
            permission: permission as i32,
            version_no: self.version_no(&e.id),
            granted_at_ms: now_ms(),
            folder: is_folder,
        };
        validate_capability(&cap)?;
        self.inner.shares.push(pb::DriveShareRecord {
            share_id,
            entry_id: e.id,
            grantee: grantee.to_owned(),
            mode: mode as i32,
            permission: permission as i32,
            granted_at_ms: cap.granted_at_ms,
            revoked: false,
            revoked_at_ms: 0,
            group_id: group_id_hex.to_owned(),
        });
        self.bump(device);
        Ok(cap)
    }

    /// Marks a share revoked. Returns the record (so the SDK can send the
    /// revoke message to its group).
    pub fn revoke(
        &mut self,
        share_id: &[u8],
        device: &[u8],
    ) -> Result<pb::DriveShareRecord, AppError> {
        let s = self
            .inner
            .shares
            .iter_mut()
            .find(|s| s.share_id == share_id)
            .ok_or_else(|| AppError::NotFound("share".into()))?;
        if s.revoked {
            return Err(AppError::Invalid("share already revoked".into()));
        }
        s.revoked = true;
        s.revoked_at_ms = now_ms();
        let out = s.clone();
        self.bump(device);
        Ok(out)
    }

    /// Live shares of an entry that must receive an update after a new
    /// version.
    #[must_use]
    pub fn live_shares_of(&self, entry_id: &[u8]) -> Vec<&pb::DriveShareRecord> {
        self.inner
            .shares
            .iter()
            .filter(|s| {
                s.entry_id == entry_id && !s.revoked && s.mode == pb::DriveShareMode::Live as i32
            })
            .collect()
    }

    /// Whether an entry has any non-revoked share (so the next version must
    /// still be readable by grantees) or has had one revoked since the
    /// current version (so the next version needs a fresh key — which
    /// [`seal_object`] always gives anyway; this is for the UI).
    #[must_use]
    pub fn has_active_shares(&self, entry_id: &[u8]) -> bool {
        self.inner
            .shares
            .iter()
            .any(|s| s.entry_id == entry_id && !s.revoked)
    }

    /// Builds the folder manifest for a folder share: the subtree with ids
    /// preserved and the folder itself as the root (its children get an
    /// empty parent id).
    pub fn folder_manifest(&self, folder_id: &[u8]) -> Result<pb::DriveFolderManifest, AppError> {
        let f = self
            .get(folder_id)
            .ok_or_else(|| AppError::NotFound("folder".into()))?;
        if f.kind != pb::DriveEntryKind::Folder as i32 {
            return Err(AppError::Invalid("not a folder".into()));
        }
        let ids: BTreeSet<Vec<u8>> = self.subtree(folder_id).into_iter().collect();
        let entries = self
            .inner
            .entries
            .iter()
            .filter(|e| ids.contains(&e.id) && e.id != folder_id && !e.trashed)
            .map(|e| {
                let mut e = e.clone();
                if e.parent_id == folder_id {
                    e.parent_id = Vec::new();
                }
                e.versions.clear();
                e
            })
            .collect();
        Ok(pb::DriveFolderManifest {
            version: DRIVE_VERSION,
            folder_id: folder_id.to_vec(),
            name: f.name.clone(),
            revision: self.inner.revision,
            updated_at_ms: now_ms(),
            entries,
        })
    }
}

fn validate_entry(e: &pb::DriveEntry) -> Result<(), AppError> {
    require_id("entry.id", &e.id)?;
    require_id_or_empty("entry.parent_id", &e.parent_id)?;
    require_str("entry.name", &e.name, MAX_NAME)?;
    require_str_max("entry.mime", &e.mime, MAX_MIME)?;
    if e.versions.len() > MAX_VERSIONS_PER_ENTRY {
        return Err(AppError::Invalid("too many versions".into()));
    }
    if e.attrs.len() > MAX_ATTRS {
        return Err(AppError::Invalid("too many attrs".into()));
    }
    if e.kind == pb::DriveEntryKind::File as i32 {
        if let Some(c) = &e.current {
            validate_object_ref(c)?;
        }
    } else if e.current.is_some() || !e.versions.is_empty() {
        return Err(AppError::Invalid("folder with content".into()));
    }
    for v in &e.versions {
        if let Some(o) = &v.object {
            validate_object_ref(o)?;
        }
    }
    Ok(())
}

fn validate_share_record(s: &pb::DriveShareRecord) -> Result<(), AppError> {
    require_id("share.share_id", &s.share_id)?;
    require_id("share.entry_id", &s.entry_id)?;
    require_str("share.grantee", &s.grantee, 128)?;
    Ok(())
}

/// Validates a capability.
pub fn validate_capability(c: &pb::DriveCapability) -> Result<(), AppError> {
    check_body("DriveCapability", c.version, CAPABILITY_VERSION)?;
    require_id("capability.share_id", &c.share_id)?;
    require_address("capability.owner", &c.owner)?;
    require_id("capability.entry_id", &c.entry_id)?;
    require_str("capability.name", &c.name, MAX_NAME)?;
    if c.name.contains('/') || c.name.contains('\\') || c.name == ".." {
        return Err(AppError::Invalid(
            "capability name contains a path separator".into(),
        ));
    }
    require_str_max("capability.mime", &c.mime, MAX_MIME)?;
    let o = c
        .object
        .as_ref()
        .ok_or_else(|| AppError::Invalid("capability has no object".into()))?;
    validate_object_ref(o)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Deterministically merges two revisions of the same Drive written by
/// different devices.
///
/// Per entry id: the entry with the later `modified_at_ms` wins all mutable
/// fields (name, parent, trashed, starred, attrs, current); ties break on
/// the writing device's key (the manifest-level `device_pubkey` of the side
/// it came from) so both devices choose the same winner. Versions are the
/// union by `version_no` (plus the loser's `current` if it differs from the
/// winner's, so no content is lost). Entries present on one side only are
/// kept. Shares are unioned by `share_id`, `revoked` is sticky.
///
/// Revision = max(a, b) + 1.
pub fn merge(a: &Manifest, b: &Manifest) -> Result<Manifest, AppError> {
    if a.inner.drive_id != b.inner.drive_id {
        return Err(AppError::Invalid("cannot merge different drives".into()));
    }
    let mut entries: BTreeMap<Vec<u8>, pb::DriveEntry> = BTreeMap::new();
    for e in &a.inner.entries {
        entries.insert(e.id.clone(), e.clone());
    }
    for eb in &b.inner.entries {
        match entries.get_mut(&eb.id) {
            None => {
                entries.insert(eb.id.clone(), eb.clone());
            }
            Some(ea) => {
                let a_wins = match ea.modified_at_ms.cmp(&eb.modified_at_ms) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    std::cmp::Ordering::Equal => a.inner.device_pubkey >= b.inner.device_pubkey,
                };
                let (ea_c, eb_c) = (ea.clone(), eb.clone());
                let (winner, loser) = if a_wins { (ea_c, eb_c) } else { (eb_c, ea_c) };
                let mut merged = winner.clone();
                // Union of versions.
                let mut versions: BTreeMap<u64, pb::DriveVersion> = BTreeMap::new();
                for v in winner.versions.iter().chain(loser.versions.iter()) {
                    versions.entry(v.version_no).or_insert_with(|| v.clone());
                }
                if let (Some(lc), Some(wc)) = (&loser.current, &winner.current) {
                    if lc.cid != wc.cid
                        && !versions
                            .values()
                            .any(|v| v.object.as_ref().map(|o| o.cid == lc.cid).unwrap_or(false))
                    {
                        let no = versions.keys().next_back().copied().unwrap_or(0) + 1;
                        versions.insert(
                            no,
                            pb::DriveVersion {
                                version_no: no,
                                object: Some(lc.clone()),
                                created_at_ms: loser.modified_at_ms,
                                device_pubkey: Vec::new(),
                                note: "concurrent edit".into(),
                            },
                        );
                    }
                }
                merged.versions = versions.into_values().collect();
                while merged.versions.len() > MAX_VERSIONS_PER_ENTRY {
                    merged.versions.remove(0);
                }
                // Trash is sticky against an older untrash.
                if loser.trashed && !winner.trashed && loser.trashed_at_ms > winner.modified_at_ms {
                    merged.trashed = true;
                    merged.trashed_at_ms = loser.trashed_at_ms;
                }
                merged.created_at_ms = winner.created_at_ms.min(loser.created_at_ms);
                *ea = merged;
            }
        }
    }
    let mut shares: BTreeMap<Vec<u8>, pb::DriveShareRecord> = BTreeMap::new();
    for s in a.inner.shares.iter().chain(b.inner.shares.iter()) {
        match shares.get_mut(&s.share_id) {
            None => {
                shares.insert(s.share_id.clone(), s.clone());
            }
            Some(existing) => {
                if s.revoked && !existing.revoked {
                    existing.revoked = true;
                    existing.revoked_at_ms = s.revoked_at_ms;
                }
            }
        }
    }
    // Tombstones: union; an entry deleted on one side stays deleted unless
    // the other side modified it after the deletion (then the edit wins and
    // the user sees the file again rather than losing work).
    let mut tombstones: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
    for t in a.inner.tombstones.iter().chain(b.inner.tombstones.iter()) {
        let e = tombstones.entry(t.id.clone()).or_insert(0);
        *e = (*e).max(t.at_ms);
    }
    entries.retain(|id, e| match tombstones.get(id) {
        Some(t) => e.modified_at_ms > *t,
        None => true,
    });
    let mut tomb: Vec<pb::DriveTombstone> = tombstones
        .into_iter()
        .map(|(id, at_ms)| pb::DriveTombstone { id, at_ms })
        .collect();
    tomb.sort_by_key(|t| t.at_ms);
    while tomb.len() > MAX_TOMBSTONES {
        tomb.remove(0);
    }
    let mut out = pb::DriveManifest {
        version: DRIVE_VERSION,
        drive_id: a.inner.drive_id.clone(),
        revision: a.inner.revision.max(b.inner.revision) + 1,
        updated_at_ms: a.inner.updated_at_ms.max(b.inner.updated_at_ms),
        device_pubkey: if a.inner.updated_at_ms >= b.inner.updated_at_ms {
            a.inner.device_pubkey.clone()
        } else {
            b.inner.device_pubkey.clone()
        },
        entries: entries.into_values().collect(),
        shares: shares.into_values().collect(),
        tombstones: tomb,
    };
    // Orphans (parent deleted on the other side) go to root.
    let ids: BTreeSet<Vec<u8>> = out.entries.iter().map(|e| e.id.clone()).collect();
    for e in &mut out.entries {
        if !e.parent_id.is_empty() && !ids.contains(&e.parent_id) {
            e.parent_id = Vec::new();
        }
    }
    // Sibling name collisions after merge: suffix the later one.
    let mut seen: BTreeSet<(Vec<u8>, String)> = BTreeSet::new();
    let mut fixes = Vec::new();
    for e in &out.entries {
        if e.trashed {
            continue;
        }
        let key = (e.parent_id.clone(), e.name.to_lowercase());
        if !seen.insert(key) {
            fixes.push(e.id.clone());
        }
    }
    for id in fixes {
        if let Some(e) = out.entries.iter_mut().find(|e| e.id == id) {
            let tag = hex::encode(e.id.get(..4).unwrap_or(&e.id));
            e.name = format!("{} ({tag})", e.name);
        }
    }
    Manifest::from_pb(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "hash1e0tl2hff03hu4g3sawcjqa2p9tc4uh24e4vfl5";

    #[test]
    fn seal_open_round_trip_various_sizes() {
        for size in [
            0usize,
            1,
            SEGMENT_PLAINTEXT - 1,
            SEGMENT_PLAINTEXT,
            SEGMENT_PLAINTEXT + 1,
            3 * SEGMENT_PLAINTEXT + 7,
        ] {
            let pt: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let (ct, r) = seal_object(&pt).unwrap();
            assert_eq!(ct.len() as u64, ciphertext_size(size as u64));
            assert_eq!(r.size, size as u64);
            assert_eq!(open_object(&ct, &r).unwrap(), pt, "size {size}");
            // Every sealed segment is exactly one chunk except the last.
            assert!(ct.len().div_ceil(CHUNK_SIZE) as u64 == segment_count(size as u64));
        }
    }

    #[test]
    fn tamper_reorder_truncate_detected() {
        let pt: Vec<u8> = (0..(2 * SEGMENT_PLAINTEXT + 10))
            .map(|i| (i % 7) as u8)
            .collect();
        let (ct, r) = seal_object(&pt).unwrap();
        let mut bad = ct.clone();
        bad[5] ^= 1;
        assert!(matches!(open_object(&bad, &r), Err(AppError::Integrity(_))));
        // Swap segments 0 and 1.
        let mut swapped = ct.clone();
        let (s0, rest) = swapped.split_at_mut(CHUNK_SIZE);
        let (s1, _) = rest.split_at_mut(CHUNK_SIZE);
        s0.swap_with_slice(s1);
        assert!(matches!(
            open_object(&swapped, &r),
            Err(AppError::Integrity(_))
        ));
        // Truncate.
        let trunc = &ct[..2 * CHUNK_SIZE];
        assert!(open_object(trunc, &r).is_err());
        // Wrong key.
        let mut r2 = r.clone();
        r2.key.as_mut().unwrap().key[0] ^= 1;
        assert!(matches!(open_object(&ct, &r2), Err(AppError::Integrity(_))));
        // Wrong plaintext hash.
        let mut r3 = r.clone();
        r3.plaintext_hash[0] ^= 1;
        assert!(matches!(open_object(&ct, &r3), Err(AppError::Integrity(_))));
    }

    #[test]
    fn streaming_matches_one_shot() {
        let pt: Vec<u8> = (0..(SEGMENT_PLAINTEXT + 100))
            .map(|i| (i % 13) as u8)
            .collect();
        let key = ObjectKey::generate().unwrap();
        let (ct, r) = seal_object_with(&pt, key.clone()).unwrap();
        let mut dec = SegmentDecryptor::new(key, r.size).unwrap();
        let mut out = Vec::new();
        for c in ct.chunks(CHUNK_SIZE) {
            out.extend_from_slice(&dec.open(c).unwrap());
        }
        dec.verify(&r.plaintext_hash).unwrap();
        assert_eq!(out, pt);
    }

    fn dev(n: u8) -> Vec<u8> {
        vec![n; 32]
    }

    fn file(m: &mut Manifest, parent: &[u8], name: &str, body: &[u8]) -> Vec<u8> {
        let (_, r) = seal_object(body).unwrap();
        m.add_file(parent, name, "text/plain", r, &dev(1)).unwrap()
    }

    #[test]
    fn tree_operations() {
        let mut m = Manifest::new(&dev(1)).unwrap();
        let docs = m.mkdir(&[], "Docs", &dev(1)).unwrap();
        let f = file(&mut m, &docs, "a.txt", b"hello");
        assert!(
            m.mkdir(&[], "docs", &dev(1)).is_err(),
            "case-insensitive sibling uniqueness"
        );
        assert!(m
            .add_file(&docs, "a.txt", "", seal_object(b"x").unwrap().1, &dev(1))
            .is_err());
        assert_eq!(m.path(&f), "/Docs/a.txt");
        assert_eq!(m.resolve_path("/docs/A.TXT").unwrap(), f);
        m.rename(&f, "b.txt", &dev(1)).unwrap();
        let sub = m.mkdir(&docs, "Sub", &dev(1)).unwrap();
        m.mv(&f, &sub, &dev(1)).unwrap();
        assert_eq!(m.path(&f), "/Docs/Sub/b.txt");
        assert!(
            m.mv(&docs, &sub, &dev(1)).is_err(),
            "no folder into descendant"
        );
        // Versions.
        let (_, r2) = seal_object(b"hello v2").unwrap();
        let no = m.update_file(&f, r2.clone(), &dev(1), "edit").unwrap();
        assert_eq!(no, 2);
        assert_eq!(m.version_no(&f), 2);
        assert_eq!(m.get(&f).unwrap().versions.len(), 1);
        m.restore_version(&f, 1, &dev(1)).unwrap();
        assert_eq!(m.get(&f).unwrap().size, 5);
        assert_eq!(m.version_no(&f), 3);
        // Trash / restore / delete.
        m.trash(&docs, &dev(1)).unwrap();
        assert!(m.get(&f).unwrap().trashed);
        assert_eq!(m.list(&[], false).len(), 0);
        assert_eq!(m.list(&[], true).len(), 1);
        assert!(m.delete(&f, &dev(1)).is_ok());
        assert!(m.get(&f).is_none());
        m.restore(&docs, &dev(1)).unwrap();
        assert!(!m.get(&docs).unwrap().trashed);
        assert!(m.delete(&docs, &dev(1)).is_err(), "must trash first");
        // Round trip through encode/decode and seal/open.
        let key = ObjectKey::generate().unwrap();
        let (ct, r) = m.seal(&key).unwrap();
        let back = Manifest::open_sealed(&ct, &r).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn sharing_records() {
        let mut m = Manifest::new(&dev(1)).unwrap();
        let f = file(&mut m, &[], "c.txt", b"contract");
        let cap = m
            .grant(
                &f,
                OWNER,
                "hash1bob",
                "aa",
                pb::DriveShareMode::Live,
                pb::DrivePermission::Read,
                None,
                &dev(1),
            )
            .unwrap();
        assert_eq!(cap.version_no, 1);
        assert!(!cap.folder);
        assert_eq!(m.live_shares_of(&f).len(), 1);
        let rec = m.revoke(&cap.share_id, &dev(1)).unwrap();
        assert!(rec.revoked);
        assert!(m.live_shares_of(&f).is_empty());
        assert!(m.revoke(&cap.share_id, &dev(1)).is_err());
        // Folder share needs a folder object.
        let d = m.mkdir(&[], "D", &dev(1)).unwrap();
        file(&mut m, &d, "in.txt", b"x");
        assert!(m
            .grant(
                &d,
                OWNER,
                "hash1bob",
                "aa",
                pb::DriveShareMode::Snapshot,
                pb::DrivePermission::Read,
                None,
                &dev(1)
            )
            .is_err());
        let fm = m.folder_manifest(&d).unwrap();
        assert_eq!(fm.entries.len(), 1);
        assert!(fm.entries[0].parent_id.is_empty());
        let (_, fobj) = seal_object(&fm.encode_to_vec()).unwrap();
        let cap = m
            .grant(
                &d,
                OWNER,
                "hash1bob",
                "aa",
                pb::DriveShareMode::Snapshot,
                pb::DrivePermission::Read,
                Some(fobj),
                &dev(1),
            )
            .unwrap();
        assert!(cap.folder);
        validate_capability(&cap).unwrap();
    }

    #[test]
    fn merge_is_deterministic_and_lossless() {
        let mut base = Manifest::new(&dev(1)).unwrap();
        let f = file(&mut base, &[], "shared.txt", b"v1");
        let g = file(&mut base, &[], "other.txt", b"o");
        let mut a = base.clone();
        let mut b = base.clone();
        // Device A edits f, device B renames f and trashes g.
        let (_, ra) = seal_object(b"v2 from A").unwrap();
        a.update_file(&f, ra.clone(), &dev(1), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        b.rename(&f, "renamed.txt", &dev(2)).unwrap();
        b.trash(&g, &dev(2)).unwrap();
        b.inner.device_pubkey = dev(2);
        let m1 = merge(&a, &b).unwrap();
        let m2 = merge(&b, &a).unwrap();
        assert_eq!(m1.inner.entries, m2.inner.entries, "commutative");
        let e = m1.get(&f).unwrap();
        assert_eq!(e.name, "renamed.txt", "later modification wins fields");
        // A's newer content is not lost: it is in versions or current.
        let has_a = e.current.as_ref().map(|c| c.cid == ra.cid).unwrap_or(false)
            || e.versions
                .iter()
                .any(|v| v.object.as_ref().map(|o| o.cid == ra.cid).unwrap_or(false));
        assert!(has_a);
        assert!(m1.get(&g).unwrap().trashed);
        assert_eq!(m1.revision(), a.revision().max(b.revision()) + 1);
    }

    #[test]
    fn merge_name_collision_and_orphans() {
        let base = Manifest::new(&dev(1)).unwrap();
        let mut a = base.clone();
        let mut b = base.clone();
        file(&mut a, &[], "same.txt", b"a");
        file(&mut b, &[], "same.txt", b"b");
        let m = merge(&a, &b).unwrap();
        assert_eq!(m.list(&[], false).len(), 2);
        let names: Vec<_> = m.list(&[], false).into_iter().map(|e| e.name).collect();
        assert!(names.iter().any(|n| n == "same.txt"));
        assert!(names.iter().any(|n| n.starts_with("same.txt (")));
        // Orphan: A deletes a folder B put a file into.
        let mut a = base.clone();
        let d = a.mkdir(&[], "D", &dev(1)).unwrap();
        let mut b = a.clone();
        let inner = file(&mut b, &d, "in.txt", b"x");
        a.trash(&d, &dev(1)).unwrap();
        a.delete(&d, &dev(1)).unwrap();
        let m = merge(&a, &b).unwrap();
        assert!(
            m.get(&d).is_none(),
            "deleted folder stays deleted (tombstone)"
        );
        assert!(
            m.get(&inner).unwrap().parent_id.is_empty(),
            "orphan goes to root"
        );
        let m2 = merge(&b, &a).unwrap();
        assert_eq!(m.entries(), m2.entries());
    }

    #[test]
    fn manifest_validation_rejects_cycles_and_bad_parents() {
        let mut m = Manifest::new(&dev(1)).unwrap();
        let a = m.mkdir(&[], "A", &dev(1)).unwrap();
        let b = m.mkdir(&a, "B", &dev(1)).unwrap();
        let mut pb = m.inner.clone();
        pb.entries.iter_mut().find(|e| e.id == a).unwrap().parent_id = b.clone();
        assert!(Manifest::from_pb(pb).is_err());
        let mut pb = m.inner.clone();
        let f = file(&mut m, &[], "f", b"x");
        pb.entries.push(m.get(&f).unwrap().clone());
        pb.entries.last_mut().unwrap().parent_id = f.clone(); // file as parent
        assert!(Manifest::from_pb(pb).is_err());
    }
}
