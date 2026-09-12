//! HashDrive: the user's encrypted file tree on the network.
//!
//! * Object bytes: sealed by `hashgram_app::drive` (segmented
//!   XChaCha20-Poly1305), uploaded as `encrypted` blobs with
//!   [`crate::blob::upload_sealed`], downloaded and verified with
//!   [`crate::blob::download`] + `open_object`.
//! * The manifest: a `hashgram_app::drive::Manifest`, sealed under the
//!   manifest key and uploaded as a private blob on every
//!   [`Drive::commit`]. The keyring `{drive_id, manifest_key, manifest_cid,
//!   revision}` lives in the vault (`extra["drive_keyring"]`) and is sent to
//!   the user's other devices as `DeviceSync.DriveKeyring` in the self
//!   group; a device that receives a keyring with a higher revision fetches
//!   that manifest and merges it with its own.
//! * Sharing: `Drive::share` grants a capability and sends it in the
//!   conversation group with the grantee(s); live shares receive
//!   `DriveShareUpdate` on every new version until `revoke`.
//!
//! Received capabilities are kept in the local store ("Shared with me").

use hashgram_app::drive as d;
use hashgram_app::pb as app;
use hashgram_app::AppError;
use tracing::{debug, warn};

use crate::account::Account;
use crate::app::HashgramOne;
use crate::messaging::Received;
use crate::store::LocalStore;
use crate::SdkError;

/// Vault key holding the keyring.
pub const VAULT_DRIVE_KEYRING: &str = "drive_keyring";
const NS_MANIFEST: &str = "drive/manifest";
const NS_SHARED: &str = "drive/shared_with_me";
const NS_UPLOADS: &str = "drive/uploads";
/// Replicas requested for objects and manifests.
pub const REPLICAS: usize = 3;

/// The keyring: what a device needs to find and open the manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Keyring {
    /// Hex drive id.
    pub drive_id: String,
    /// Hex 32-byte key.
    pub key: String,
    /// Hex 24-byte base nonce.
    pub base_nonce: String,
    /// Hex CID of the latest committed manifest (empty before first commit).
    pub manifest_cid: String,
    /// Object ref of the latest manifest (for open_object).
    pub manifest_ref: Option<app::DriveObjectRef>,
    /// Revision of that manifest.
    pub revision: u64,
}

impl Keyring {
    fn object_key(&self) -> Result<d::ObjectKey, SdkError> {
        let k = hex::decode(&self.key).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let n = hex::decode(&self.base_nonce).map_err(|e| SdkError::Invalid(e.to_string()))?;
        d::ObjectKey::from_pb(&app::DriveKey {
            key: k,
            base_nonce: n,
        })
        .map_err(Into::into)
    }
}

/// A capability someone shared with us.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SharedWithMe {
    /// The capability (latest known version).
    pub capability: app::DriveCapability,
    /// Who shared it (authenticated MLS sender).
    pub from: String,
    /// Group it arrived in (hex).
    pub group_id: String,
    /// Note.
    pub note: String,
    /// When received.
    pub received_at_ms: u64,
    /// Updates received (version numbers).
    pub updates: u32,
    /// Revoked by the owner.
    pub revoked: bool,
}

/// A resumable upload in progress (so a crash mid-upload can resume).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PendingUpload {
    /// Hex parent id.
    pub parent_id: String,
    /// Name.
    pub name: String,
    /// MIME.
    pub mime: String,
    /// The object ref (key + cid) computed before upload.
    pub object: app::DriveObjectRef,
    /// Path of the sealed ciphertext in the cache dir.
    pub sealed_path: String,
    /// Started.
    pub started_at_ms: u64,
}

/// Drive state held by the facade.
#[derive(Debug)]
pub struct DriveState {
    /// The manifest (always present; created on first open).
    pub manifest: d::Manifest,
    /// Keyring.
    pub keyring: Keyring,
    /// Whether the manifest has uncommitted changes.
    pub dirty: bool,
}

impl DriveState {
    pub(crate) fn load(account: &Account, store: &LocalStore) -> Result<Self, SdkError> {
        let device = account.device()?.public_key().to_vec();
        let keyring: Option<Keyring> = account
            .contents
            .extra
            .get(VAULT_DRIVE_KEYRING)
            .and_then(|h| hex::decode(h).ok())
            .and_then(|b| serde_json::from_slice(&b).ok());
        let cached: Option<Vec<u8>> = store.get_bytes(NS_MANIFEST, b"current")?;
        match (keyring, cached) {
            (Some(k), Some(bytes)) => {
                let manifest = d::Manifest::decode(&bytes)?;
                Ok(Self {
                    manifest,
                    keyring: k,
                    dirty: false,
                })
            }
            (Some(k), None) => {
                // Keyring without a local manifest: another device created
                // the Drive; sync will fetch it. Start empty under the same id.
                let mut m = d::Manifest::new(&device)?;
                let mut pb = m.as_pb().clone();
                pb.drive_id = hex::decode(&k.drive_id).unwrap_or(pb.drive_id);
                pb.revision = 0;
                m = d::Manifest::from_pb(pb)?;
                Ok(Self {
                    manifest: m,
                    keyring: k,
                    dirty: false,
                })
            }
            (None, _) => {
                let manifest = d::Manifest::new(&device)?;
                let key = d::ObjectKey::generate()?;
                let keyring = Keyring {
                    drive_id: hex::encode(manifest.drive_id()),
                    key: hex::encode(key.key),
                    base_nonce: hex::encode(key.base_nonce),
                    manifest_cid: String::new(),
                    manifest_ref: None,
                    revision: 0,
                };
                Ok(Self {
                    manifest,
                    keyring,
                    dirty: true,
                })
            }
        }
    }

    pub(crate) fn persist(&self, account: &mut Account) -> Result<(), SdkError> {
        let raw = serde_json::to_vec(&self.keyring).map_err(|e| SdkError::Store(e.to_string()))?;
        account
            .contents
            .extra
            .insert(VAULT_DRIVE_KEYRING.into(), hex::encode(raw));
        Ok(())
    }
}

/// The Drive API.
pub struct Drive<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Drive<'a> {
    fn device_key(&self) -> Result<Vec<u8>, SdkError> {
        Ok(self.one.account.device()?.public_key().to_vec())
    }

    fn save_manifest_locally(&mut self) -> Result<(), SdkError> {
        let bytes = self.one.drive_state.manifest.encode();
        self.one.store.put_bytes(NS_MANIFEST, b"current", &bytes)?;
        self.one.drive_state.dirty = true;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Listing
    // -----------------------------------------------------------------------

    /// Children of a folder (`""` = root).
    pub fn list(&self, parent_hex: &str) -> Result<Vec<d::EntryView>, SdkError> {
        let parent = hex::decode(parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        Ok(self.one.drive_state.manifest.list(&parent, false))
    }
    /// Trash roots.
    pub fn trash_list(&self) -> Vec<d::EntryView> {
        self.one.drive_state.manifest.list(&[], true)
    }
    /// Starred.
    pub fn starred(&self) -> Vec<d::EntryView> {
        self.one.drive_state.manifest.starred()
    }
    /// Name search.
    pub fn search(&self, q: &str, limit: usize) -> Vec<d::EntryView> {
        self.one.drive_state.manifest.search(q, limit)
    }
    /// One entry.
    pub fn entry(&self, id_hex: &str) -> Result<Option<d::EntryView>, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        Ok(self
            .one
            .drive_state
            .manifest
            .get(&id)
            .map(|e| self.one.drive_state.manifest.view(e)))
    }
    /// Versions of a file.
    pub fn versions(&self, id_hex: &str) -> Result<Vec<app::DriveVersion>, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let e = self
            .one
            .drive_state
            .manifest
            .get(&id)
            .ok_or_else(|| SdkError::NotFound("entry".into()))?;
        let mut v = e.versions.clone();
        if let Some(c) = &e.current {
            v.push(app::DriveVersion {
                version_no: self.one.drive_state.manifest.version_no(&id),
                object: Some(c.clone()),
                created_at_ms: e.modified_at_ms,
                device_pubkey: Vec::new(),
                note: "current".into(),
            });
        }
        Ok(v)
    }
    /// Resolve a path.
    pub fn resolve_path(&self, path: &str) -> Option<String> {
        self.one
            .drive_state
            .manifest
            .resolve_path(path)
            .map(hex::encode)
    }
    /// Usage.
    pub fn usage(&self) -> DriveUsage {
        let m = &self.one.drive_state.manifest;
        DriveUsage {
            files: m
                .entries()
                .iter()
                .filter(|e| !e.trashed && e.kind == app::DriveEntryKind::File as i32)
                .count(),
            folders: m
                .entries()
                .iter()
                .filter(|e| !e.trashed && e.kind == app::DriveEntryKind::Folder as i32)
                .count(),
            trashed: m.entries().iter().filter(|e| e.trashed).count(),
            bytes: m.used_bytes(),
            revision: m.revision(),
            committed_revision: self.one.drive_state.keyring.revision,
            dirty: self.one.drive_state.dirty,
        }
    }

    // -----------------------------------------------------------------------
    // Mutations (local; `commit` publishes)
    // -----------------------------------------------------------------------

    /// Creates a folder.
    pub fn mkdir(&mut self, parent_hex: &str, name: &str) -> Result<String, SdkError> {
        let parent = hex::decode(parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        let id = self.one.drive_state.manifest.mkdir(&parent, name, &dev)?;
        self.save_manifest_locally()?;
        Ok(hex::encode(id))
    }

    /// Uploads a file: seals, pushes to store nodes, adds to the manifest.
    /// Returns the entry id (hex).
    pub async fn upload(
        &mut self,
        parent_hex: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<String, SdkError> {
        let parent = hex::decode(parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let (ct, r) = d::seal_object(bytes)?;
        let device = self.one.account.device()?;
        crate::blob::upload_sealed(&self.one.link, &self.one.network, &device, &ct, REPLICAS)
            .await?;
        let dev = device.public_key().to_vec();
        let id = self
            .one
            .drive_state
            .manifest
            .add_file(&parent, name, mime, r, &dev)?;
        self.save_manifest_locally()?;
        Ok(hex::encode(id))
    }

    /// Replaces a file's content with a new version and pushes live-share
    /// updates.
    pub async fn update(
        &mut self,
        id_hex: &str,
        bytes: &[u8],
        note: &str,
    ) -> Result<u64, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let (ct, r) = d::seal_object(bytes)?;
        let device = self.one.account.device()?;
        crate::blob::upload_sealed(&self.one.link, &self.one.network, &device, &ct, REPLICAS)
            .await?;
        let dev = device.public_key().to_vec();
        let no = self
            .one
            .drive_state
            .manifest
            .update_file(&id, r.clone(), &dev, note)?;
        self.save_manifest_locally()?;
        // Live shares.
        let shares: Vec<app::DriveShareRecord> = self
            .one
            .drive_state
            .manifest
            .live_shares_of(&id)
            .into_iter()
            .cloned()
            .collect();
        let (name, size) = self
            .one
            .drive_state
            .manifest
            .get(&id)
            .map(|e| (e.name.clone(), e.size))
            .unwrap_or_default();
        for s in shares {
            let Ok(gid) = hex::decode(&s.group_id) else {
                continue;
            };
            let upd = app::DriveShareUpdate {
                version: hashgram_app::version::CAPABILITY_VERSION,
                share_id: s.share_id.clone(),
                object: Some(r.clone()),
                version_no: no,
                name: name.clone(),
                size,
                at_ms: hashgram_app::ids::now_ms(),
            };
            if let Err(e) = self
                .one
                .send_app(&gid, app::app_message::Body::DriveShareUpdate(upd))
                .await
            {
                warn!(error = %e, "live share update not delivered");
            }
        }
        Ok(no)
    }

    /// Re-encrypts the current content of a file under a fresh key as a new
    /// version and revokes every live share of it. Used after a device is
    /// stolen or a share must be cut off: grantees who still hold the old
    /// capability can fetch the OLD ciphertext by CID (unavoidable), but
    /// never this or later versions. Returns the new version number.
    pub async fn rekey(&mut self, id_hex: &str) -> Result<u64, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let bytes = self.download(id_hex).await?;
        let shares: Vec<Vec<u8>> = self
            .one
            .drive_state
            .manifest
            .shares()
            .iter()
            .filter(|s| s.entry_id == id && !s.revoked)
            .map(|s| s.share_id.clone())
            .collect();
        for sid in shares {
            self.revoke(&hex::encode(sid)).await?;
        }
        self.update(id_hex, &bytes, "rekeyed").await
    }

    /// Re-keys every file in the Drive (see [`Self::rekey`]). Slow: every
    /// object is downloaded, re-sealed and uploaded. Returns the count.
    pub async fn rekey_all(&mut self) -> Result<usize, SdkError> {
        let ids: Vec<String> = self
            .one
            .drive_state
            .manifest
            .entries()
            .iter()
            .filter(|e| {
                !e.trashed && e.kind == app::DriveEntryKind::File as i32 && e.current.is_some()
            })
            .map(|e| hex::encode(&e.id))
            .collect();
        let mut n = 0;
        for id in ids {
            self.rekey(&id).await?;
            n += 1;
        }
        Ok(n)
    }

    /// Restores an older version.
    pub async fn restore_version(&mut self, id_hex: &str, version_no: u64) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one
            .drive_state
            .manifest
            .restore_version(&id, version_no, &dev)?;
        self.save_manifest_locally()
    }
    /// Rename.
    pub fn rename(&mut self, id_hex: &str, name: &str) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one.drive_state.manifest.rename(&id, name, &dev)?;
        self.save_manifest_locally()
    }
    /// Move.
    pub fn mv(&mut self, id_hex: &str, new_parent_hex: &str) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let p = hex::decode(new_parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one.drive_state.manifest.mv(&id, &p, &dev)?;
        self.save_manifest_locally()
    }
    /// Copy a file (no re-upload).
    pub fn copy(
        &mut self,
        id_hex: &str,
        new_parent_hex: &str,
        name: &str,
    ) -> Result<String, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let p = hex::decode(new_parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        let n = self
            .one
            .drive_state
            .manifest
            .copy_file(&id, &p, name, &dev)?;
        self.save_manifest_locally()?;
        Ok(hex::encode(n))
    }
    /// Trash.
    pub fn trash(&mut self, id_hex: &str) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one.drive_state.manifest.trash(&id, &dev)?;
        self.save_manifest_locally()
    }
    /// Restore from trash.
    pub fn restore(&mut self, id_hex: &str) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one.drive_state.manifest.restore(&id, &dev)?;
        self.save_manifest_locally()
    }
    /// Permanently delete (must be trashed).
    pub fn delete(&mut self, id_hex: &str) -> Result<usize, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        let n = self.one.drive_state.manifest.delete(&id, &dev)?;
        self.save_manifest_locally()?;
        Ok(n)
    }
    /// Empty trash.
    pub fn empty_trash(&mut self) -> Result<usize, SdkError> {
        let dev = self.device_key()?;
        let n = self.one.drive_state.manifest.empty_trash(&dev);
        self.save_manifest_locally()?;
        Ok(n)
    }
    /// Star.
    pub fn star(&mut self, id_hex: &str, on: bool) -> Result<(), SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        self.one.drive_state.manifest.set_starred(&id, on, &dev)?;
        self.save_manifest_locally()
    }

    // -----------------------------------------------------------------------
    // Download
    // -----------------------------------------------------------------------

    /// Downloads and decrypts the current content of a file.
    pub async fn download(&mut self, id_hex: &str) -> Result<Vec<u8>, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let r = self
            .one
            .drive_state
            .manifest
            .get(&id)
            .and_then(|e| e.current.clone())
            .ok_or_else(|| SdkError::NotFound("file content".into()))?;
        self.download_object(&r).await
    }

    /// Downloads a specific version.
    pub async fn download_version(
        &mut self,
        id_hex: &str,
        version_no: u64,
    ) -> Result<Vec<u8>, SdkError> {
        let v = self.versions(id_hex)?;
        let r = v
            .into_iter()
            .find(|v| v.version_no == version_no)
            .and_then(|v| v.object)
            .ok_or_else(|| SdkError::NotFound(format!("version {version_no}")))?;
        self.download_object(&r).await
    }

    /// Downloads and opens any object reference (verifies chunk hashes,
    /// AEAD tags and the plaintext hash).
    pub async fn download_object(&mut self, r: &app::DriveObjectRef) -> Result<Vec<u8>, SdkError> {
        d::validate_object_ref(r)?;
        let device = self.one.account.device()?;
        let (ct, _m, _peer) = crate::blob::download(
            &self.one.link,
            &r.cid,
            Some((&self.one.chain, &self.one.network, &device)),
        )
        .await?;
        Ok(d::open_object(&ct, r)?)
    }

    /// Downloads the object behind a capability (file bytes, or the sealed
    /// folder manifest for a folder capability).
    pub async fn download_capability(
        &mut self,
        cap: &app::DriveCapability,
    ) -> Result<Vec<u8>, SdkError> {
        d::validate_capability(cap)?;
        let r = cap
            .object
            .as_ref()
            .ok_or_else(|| SdkError::Invalid("capability has no object".into()))?;
        self.download_object(r).await
    }

    /// Lists a shared folder capability's entries.
    pub async fn shared_folder_entries(
        &mut self,
        cap: &app::DriveCapability,
    ) -> Result<app::DriveFolderManifest, SdkError> {
        if !cap.folder {
            return Err(SdkError::Invalid("not a folder capability".into()));
        }
        let bytes = self.download_capability(cap).await?;
        use prost::Message;
        app::DriveFolderManifest::decode(bytes.as_slice())
            .map_err(|e| SdkError::Corrupt(e.to_string()))
    }

    /// Saves a received capability's current content into our own Drive
    /// (no re-upload: the object ref is copied; the recipient now holds
    /// the key like any other entry).
    pub fn save_capability(
        &mut self,
        cap: &app::DriveCapability,
        parent_hex: &str,
    ) -> Result<String, SdkError> {
        d::validate_capability(cap)?;
        if cap.folder {
            return Err(SdkError::Invalid(
                "save individual files from a folder share".into(),
            ));
        }
        let parent = hex::decode(parent_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        let obj = cap.object.clone().unwrap_or_default();
        let id = self
            .one
            .drive_state
            .manifest
            .add_file(&parent, &cap.name, &cap.mime, obj, &dev)?;
        let _ = self.one.drive_state.manifest.set_attr(
            &id,
            "origin",
            &format!("share:{}", hex::encode(&cap.share_id)),
            &dev,
        );
        let _ = self
            .one
            .drive_state
            .manifest
            .set_attr(&id, "owner", &cap.owner, &dev);
        self.save_manifest_locally()?;
        Ok(hex::encode(id))
    }

    // -----------------------------------------------------------------------
    // Sharing
    // -----------------------------------------------------------------------

    /// Shares an entry with one or more addresses (comma-separated
    /// `grantees`) in their conversation group. Returns the capability.
    pub async fn share(
        &mut self,
        id_hex: &str,
        grantees: &str,
        mode: app::DriveShareMode,
        permission: app::DrivePermission,
        note: &str,
    ) -> Result<app::DriveCapability, SdkError> {
        let id = hex::decode(id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let addrs: Vec<String> = grantees
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        if addrs.is_empty() {
            return Err(SdkError::Invalid("no grantee".into()));
        }
        let gid = self.one.conversation_group(&addrs).await?;
        let cap = self
            .grant_in_group(&id, grantees, &gid, mode, permission)
            .await?;
        self.one
            .send_app(
                &gid,
                app::app_message::Body::DriveShare(app::DriveShare {
                    capability: Some(cap.clone()),
                    note: note.to_owned(),
                }),
            )
            .await?;
        self.save_manifest_locally()?;
        Ok(cap)
    }

    /// Grants a capability inside an existing group (used by Spaces and by
    /// mail attachments). Does not send anything.
    pub(crate) async fn grant_in_group(
        &mut self,
        id: &[u8],
        grantee_label: &str,
        gid: &[u8],
        mode: app::DriveShareMode,
        permission: app::DrivePermission,
    ) -> Result<app::DriveCapability, SdkError> {
        let is_folder = self
            .one
            .drive_state
            .manifest
            .get(id)
            .map(|e| e.kind == app::DriveEntryKind::Folder as i32)
            .unwrap_or(false);
        let folder_object = if is_folder {
            use prost::Message;
            let fm = self.one.drive_state.manifest.folder_manifest(id)?;
            let (ct, r) = d::seal_object(&fm.encode_to_vec())?;
            let device = self.one.account.device()?;
            crate::blob::upload_sealed(&self.one.link, &self.one.network, &device, &ct, REPLICAS)
                .await?;
            Some(r)
        } else {
            None
        };
        let owner = self.one.account.address().to_owned();
        let dev = self.device_key()?;
        let cap = self.one.drive_state.manifest.grant(
            id,
            &owner,
            grantee_label,
            &hex::encode(gid),
            mode,
            permission,
            folder_object,
            &dev,
        )?;
        Ok(cap)
    }

    /// Revokes a share: notifies the group and marks the record. The next
    /// version of the entry is sealed under a fresh key automatically.
    pub async fn revoke(&mut self, share_id_hex: &str) -> Result<(), SdkError> {
        let sid = hex::decode(share_id_hex).map_err(|e| SdkError::Invalid(e.to_string()))?;
        let dev = self.device_key()?;
        let rec = self.one.drive_state.manifest.revoke(&sid, &dev)?;
        if let Ok(gid) = hex::decode(&rec.group_id) {
            let _ = self
                .one
                .send_app(
                    &gid,
                    app::app_message::Body::DriveShareRevoke(app::DriveShareRevoke {
                        version: hashgram_app::version::CAPABILITY_VERSION,
                        share_id: sid,
                        at_ms: hashgram_app::ids::now_ms(),
                    }),
                )
                .await;
        }
        self.save_manifest_locally()
    }

    /// Shares we granted.
    pub fn shares(&self) -> Vec<app::DriveShareRecord> {
        self.one.drive_state.manifest.shares().to_vec()
    }

    /// Capabilities shared with us.
    pub fn shared_with_me(&self) -> Result<Vec<SharedWithMe>, SdkError> {
        let mut v: Vec<SharedWithMe> = self
            .one
            .store
            .scan::<SharedWithMe>(NS_SHARED)?
            .into_iter()
            .map(|(_, s)| s)
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.received_at_ms));
        Ok(v)
    }

    /// Handles Drive application messages.
    pub(crate) fn handle_incoming(
        &mut self,
        r: &Received,
        appmsg: &app::AppMessage,
    ) -> Result<bool, SdkError> {
        use app::app_message::Body as B;
        match &appmsg.body {
            Some(B::DriveShare(s)) => {
                let cap = s
                    .capability
                    .clone()
                    .ok_or_else(|| SdkError::Invalid("share without capability".into()))?;
                d::validate_capability(&cap)?;
                if cap.owner != r.sender {
                    warn!("drive share whose owner is not the MLS sender; ignored");
                    return Ok(false);
                }
                let rec = SharedWithMe {
                    capability: cap.clone(),
                    from: r.sender.clone(),
                    group_id: r.group_id.clone(),
                    note: s.note.clone(),
                    received_at_ms: hashgram_app::ids::now_ms(),
                    updates: 0,
                    revoked: false,
                };
                self.one.store.put(NS_SHARED, &cap.share_id, &rec)?;
                Ok(true)
            }
            Some(B::DriveShareUpdate(u)) => {
                let Some(mut rec) = self.one.store.get::<SharedWithMe>(NS_SHARED, &u.share_id)?
                else {
                    return Ok(false);
                };
                if rec.from != r.sender || rec.revoked {
                    return Ok(false);
                }
                if rec.capability.mode != app::DriveShareMode::Live as i32 {
                    debug!("update for a snapshot share ignored");
                    return Ok(false);
                }
                if let Some(o) = &u.object {
                    d::validate_object_ref(o)?;
                    rec.capability.object = Some(o.clone());
                }
                if u.version_no > rec.capability.version_no {
                    rec.capability.version_no = u.version_no;
                }
                if !u.name.is_empty() {
                    rec.capability.name = u.name.clone();
                }
                rec.capability.size = u.size;
                rec.updates += 1;
                self.one.store.put(NS_SHARED, &u.share_id, &rec)?;
                Ok(true)
            }
            Some(B::DriveShareRevoke(v)) => {
                if let Some(mut rec) = self.one.store.get::<SharedWithMe>(NS_SHARED, &v.share_id)? {
                    if rec.from == r.sender {
                        rec.revoked = true;
                        self.one.store.put(NS_SHARED, &v.share_id, &rec)?;
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    // -----------------------------------------------------------------------
    // Commit / sync
    // -----------------------------------------------------------------------

    /// Seals and uploads the manifest, updates the keyring and tells our
    /// other devices. Idempotent when nothing changed.
    pub async fn commit(&mut self) -> Result<u64, SdkError> {
        if !self.one.drive_state.dirty && !self.one.drive_state.keyring.manifest_cid.is_empty() {
            return Ok(self.one.drive_state.keyring.revision);
        }
        let key = self.one.drive_state.keyring.object_key()?;
        let (ct, r) = self.one.drive_state.manifest.seal(&key)?;
        let device = self.one.account.device()?;
        crate::blob::upload_sealed(&self.one.link, &self.one.network, &device, &ct, REPLICAS)
            .await?;
        let rev = self.one.drive_state.manifest.revision();
        self.one.drive_state.keyring.manifest_cid = hex::encode(&r.cid);
        self.one.drive_state.keyring.manifest_ref = Some(r);
        self.one.drive_state.keyring.revision = rev;
        self.one.drive_state.dirty = false;
        self.one.drive_state.persist(&mut self.one.account)?;
        self.announce_keyring().await;
        Ok(rev)
    }

    /// Sends the keyring to the self group (no-op on single-device accounts).
    pub(crate) async fn announce_keyring(&mut self) {
        let Ok(Some(gid)) = self.one.self_group().await else {
            return;
        };
        let k = &self.one.drive_state.keyring;
        let body = app::DeviceSync {
            version: hashgram_app::version::DEVICE_SYNC_VERSION,
            body: Some(app::device_sync::Body::DriveKeyring(app::DriveKeyring {
                drive_id: hex::decode(&k.drive_id).unwrap_or_default(),
                manifest_key: Some(app::DriveKey {
                    key: hex::decode(&k.key).unwrap_or_default(),
                    base_nonce: hex::decode(&k.base_nonce).unwrap_or_default(),
                }),
                manifest_cid: hex::decode(&k.manifest_cid).unwrap_or_default(),
                revision: k.revision,
            })),
        };
        if let Err(e) = self
            .one
            .send_app(&gid, app::app_message::Body::DeviceSync(body))
            .await
        {
            debug!(error = %e, "drive keyring announce skipped");
        }
    }

    /// Applies a keyring from another of our devices: adopts the key if we
    /// had none, and fetches + merges a newer manifest.
    pub(crate) async fn apply_keyring(&mut self, k: &app::DriveKeyring) -> Result<bool, SdkError> {
        let Some(mk) = &k.manifest_key else {
            return Ok(false);
        };
        let incoming_id = hex::encode(&k.drive_id);
        let mine = &self.one.drive_state.keyring;
        let have_content =
            !mine.manifest_cid.is_empty() || !self.one.drive_state.manifest.entries().is_empty();
        if mine.drive_id != incoming_id {
            if have_content {
                // Two devices created separate Drives before ever syncing.
                // Keep ours; the user merges manually (documented limitation).
                warn!("another device announced a different drive id; keeping local drive");
                return Ok(false);
            }
            self.one.drive_state.keyring = Keyring {
                drive_id: incoming_id,
                key: hex::encode(&mk.key),
                base_nonce: hex::encode(&mk.base_nonce),
                manifest_cid: String::new(),
                manifest_ref: None,
                revision: 0,
            };
        }
        if k.revision <= self.one.drive_state.keyring.revision || k.manifest_cid.is_empty() {
            return Ok(false);
        }
        // Fetch the newer manifest.
        let key = self.one.drive_state.keyring.object_key()?;
        let (ct, _m, _p) = crate::blob::download(&self.one.link, &k.manifest_cid, None).await?;
        let pt = d::open_object_with_key(&ct, key)?;
        let theirs = d::Manifest::decode(&pt)?;
        let merged = if self.one.drive_state.manifest.entries().is_empty()
            && self.one.drive_state.manifest.revision() <= 1
        {
            theirs
        } else {
            d::merge(&self.one.drive_state.manifest, &theirs)?
        };
        self.one.drive_state.manifest = merged;
        self.one.drive_state.keyring.manifest_cid = hex::encode(&k.manifest_cid);
        self.one.drive_state.keyring.revision = k.revision;
        self.save_manifest_locally()?;
        self.one.drive_state.dirty = self.one.drive_state.manifest.revision() > k.revision;
        self.one.drive_state.persist(&mut self.one.account)?;
        Ok(true)
    }

    /// Pending uploads (crash recovery bookkeeping).
    pub fn pending_uploads(&self) -> Result<Vec<PendingUpload>, SdkError> {
        Ok(self
            .one
            .store
            .scan::<PendingUpload>(NS_UPLOADS)?
            .into_iter()
            .map(|(_, v)| v)
            .collect())
    }
}

/// Drive usage summary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DriveUsage {
    /// Files.
    pub files: usize,
    /// Folders.
    pub folders: usize,
    /// Trashed entries.
    pub trashed: usize,
    /// Plaintext bytes of current content.
    pub bytes: u64,
    /// Local manifest revision.
    pub revision: u64,
    /// Last committed revision.
    pub committed_revision: u64,
    /// Uncommitted changes.
    pub dirty: bool,
}

#[allow(dead_code)]
fn _assert_app_error_conv(e: AppError) -> SdkError {
    e.into()
}
