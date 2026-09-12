//! The local client store: everything a Hashgram One client remembers that
//! is not in the vault and never goes to a node.
//!
//! Mail folders and flags, received capabilities, contact state, Space
//! state, Drive manifest cache, sync cursors. One redb file, one table,
//! composite keys `namespace ‖ 0x00 ‖ key`, every value sealed with
//! XChaCha20-Poly1305 under a key derived from the device seed
//! (`blake3::derive_key("hashgram local store v1", device_seed)`), with the
//! namespace and key bound as AAD so a record cannot be moved between
//! namespaces on disk. The file is therefore useless without the vault.
//!
//! A `schema_version` record guards forward-only migrations
//! ([`LocalStore::migrate`]). Values are JSON for inspectability under the
//! seal; the store is not a query engine — modules keep the indexes they
//! need (see `mail::MailStore`).

use std::path::Path;
use std::sync::Arc;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use redb::{Database, ReadableDatabase, TableDefinition};

use crate::SdkError;

const KV: TableDefinition<&[u8], &[u8]> = TableDefinition::new("kv");

/// Current schema.
pub const SCHEMA_VERSION: u32 = 1;

/// A raw (key, sealed-open bytes) pair.
pub type RawRecord = (Vec<u8>, Vec<u8>);

/// Store handle; cheap to clone.
#[derive(Clone)]
pub struct LocalStore {
    db: Arc<Database>,
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for LocalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStore").finish_non_exhaustive()
    }
}

fn err<E: std::fmt::Display>(e: E) -> SdkError {
    SdkError::Store(e.to_string())
}

fn composite(ns: &str, key: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(ns.len() + 1 + key.len());
    k.extend_from_slice(ns.as_bytes());
    k.push(0);
    k.extend_from_slice(key);
    k
}

impl LocalStore {
    /// Opens or creates the store at `path`, keyed by the device seed.
    pub fn open(path: &Path, device_seed: &[u8; 32]) -> Result<Self, SdkError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        let db = Database::builder()
            .set_cache_size(32 << 20)
            .create(path)
            .map_err(err)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        let key = blake3::derive_key("hashgram local store v1", device_seed);
        let s = Self {
            db: Arc::new(db),
            cipher: XChaCha20Poly1305::new((&key).into()),
        };
        s.migrate()?;
        Ok(s)
    }

    /// An in-memory store for tests and ephemeral sessions.
    pub fn ephemeral(device_seed: &[u8; 32]) -> Result<Self, SdkError> {
        let db = Database::builder()
            .create_with_backend(redb::backends::InMemoryBackend::new())
            .map_err(err)?;
        let key = blake3::derive_key("hashgram local store v1", device_seed);
        let s = Self {
            db: Arc::new(db),
            cipher: XChaCha20Poly1305::new((&key).into()),
        };
        s.migrate()?;
        Ok(s)
    }

    /// Forward-only schema migration.
    pub fn migrate(&self) -> Result<u32, SdkError> {
        let current: Option<u32> = self.get("meta", b"schema_version")?;
        match current {
            None => {
                self.put("meta", b"schema_version", &SCHEMA_VERSION)?;
                Ok(SCHEMA_VERSION)
            }
            Some(v) if v == SCHEMA_VERSION => Ok(v),
            Some(v) if v < SCHEMA_VERSION => {
                // Future migrations go here, one `if v < N { ... }` step each.
                self.put("meta", b"schema_version", &SCHEMA_VERSION)?;
                Ok(SCHEMA_VERSION)
            }
            Some(v) => Err(SdkError::Store(format!(
                "local store schema {v} is newer than this build ({SCHEMA_VERSION}); refusing to open"
            ))),
        }
    }

    fn seal(&self, ns: &str, key: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, SdkError> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|_| SdkError::Invalid("no randomness".into()))?;
        let aad = composite(ns, key);
        let ct = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| SdkError::Store("seal failed".into()))?;
        let mut out = Vec::with_capacity(24 + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    fn open_sealed(&self, ns: &str, key: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SdkError> {
        if sealed.len() < 24 {
            return Err(SdkError::Store("record too short".into()));
        }
        let (nonce, ct) = sealed.split_at(24);
        let aad = composite(ns, key);
        self.cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: &aad })
            .map_err(|_| SdkError::Store("record failed to authenticate".into()))
    }

    /// Writes a JSON-serialisable value.
    pub fn put<T: serde::Serialize>(
        &self,
        ns: &str,
        key: &[u8],
        value: &T,
    ) -> Result<(), SdkError> {
        let json = serde_json::to_vec(value).map_err(err)?;
        let sealed = self.seal(ns, key, &json)?;
        let tx = self.db.begin_write().map_err(err)?;
        {
            let mut t = tx.open_table(KV).map_err(err)?;
            t.insert(composite(ns, key).as_slice(), sealed.as_slice())
                .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(())
    }

    /// Writes raw bytes.
    pub fn put_bytes(&self, ns: &str, key: &[u8], value: &[u8]) -> Result<(), SdkError> {
        let sealed = self.seal(ns, key, value)?;
        let tx = self.db.begin_write().map_err(err)?;
        {
            let mut t = tx.open_table(KV).map_err(err)?;
            t.insert(composite(ns, key).as_slice(), sealed.as_slice())
                .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(())
    }

    /// Reads a value.
    pub fn get<T: serde::de::DeserializeOwned>(
        &self,
        ns: &str,
        key: &[u8],
    ) -> Result<Option<T>, SdkError> {
        match self.get_bytes(ns, key)? {
            Some(b) => Ok(Some(serde_json::from_slice(&b).map_err(err)?)),
            None => Ok(None),
        }
    }

    /// Reads raw bytes.
    pub fn get_bytes(&self, ns: &str, key: &[u8]) -> Result<Option<Vec<u8>>, SdkError> {
        let tx = self.db.begin_read().map_err(err)?;
        let t = match tx.open_table(KV) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(err(e)),
        };
        let k = composite(ns, key);
        match t.get(k.as_slice()).map_err(err)? {
            Some(v) => Ok(Some(self.open_sealed(ns, key, v.value())?)),
            None => Ok(None),
        }
    }

    /// Deletes a record. Returns whether it existed.
    pub fn delete(&self, ns: &str, key: &[u8]) -> Result<bool, SdkError> {
        let tx = self.db.begin_write().map_err(err)?;
        let existed;
        {
            let mut t = tx.open_table(KV).map_err(err)?;
            let removed = t.remove(composite(ns, key).as_slice()).map_err(err)?;
            existed = removed.is_some();
            drop(removed);
        }
        tx.commit().map_err(err)?;
        Ok(existed)
    }

    /// Every (key, value) in a namespace, in key order, decoded as `T`.
    /// Records that fail to decode are skipped (a newer build wrote them).
    pub fn scan<T: serde::de::DeserializeOwned>(
        &self,
        ns: &str,
    ) -> Result<Vec<(Vec<u8>, T)>, SdkError> {
        let raw = self.scan_bytes(ns)?;
        let mut out = Vec::with_capacity(raw.len());
        for (k, v) in raw {
            if let Ok(t) = serde_json::from_slice(&v) {
                out.push((k, t));
            }
        }
        Ok(out)
    }

    /// Every (key, bytes) in a namespace.
    pub fn scan_bytes(&self, ns: &str) -> Result<Vec<RawRecord>, SdkError> {
        let tx = self.db.begin_read().map_err(err)?;
        let t = match tx.open_table(KV) {
            Ok(t) => t,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(e) => return Err(err(e)),
        };
        let start = composite(ns, &[]);
        let mut end = start.clone();
        // 0x00 separator → everything below 0x01 in the same namespace.
        if let Some(last) = end.last_mut() {
            *last = 1;
        }
        let mut out = Vec::new();
        for item in t.range(start.as_slice()..end.as_slice()).map_err(err)? {
            let (k, v) = item.map_err(err)?;
            let full = k.value();
            let key = full.get(start.len()..).unwrap_or(&[]).to_vec();
            out.push((key.clone(), self.open_sealed(ns, &key, v.value())?));
        }
        Ok(out)
    }

    /// Number of records in a namespace.
    pub fn count(&self, ns: &str) -> Result<usize, SdkError> {
        Ok(self.scan_bytes(ns)?.len())
    }

    /// Removes every record in a namespace.
    pub fn clear(&self, ns: &str) -> Result<usize, SdkError> {
        let keys: Vec<Vec<u8>> = self.scan_bytes(ns)?.into_iter().map(|(k, _)| k).collect();
        let tx = self.db.begin_write().map_err(err)?;
        {
            let mut t = tx.open_table(KV).map_err(err)?;
            for k in &keys {
                t.remove(composite(ns, k).as_slice()).map_err(err)?;
            }
        }
        tx.commit().map_err(err)?;
        Ok(keys.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_scan_delete() {
        let s = LocalStore::ephemeral(&[7; 32]).unwrap();
        s.put("mail", b"b", &"second").unwrap();
        s.put("mail", b"a", &"first").unwrap();
        s.put("other", b"a", &"x").unwrap();
        assert_eq!(s.get::<String>("mail", b"a").unwrap().unwrap(), "first");
        let all: Vec<(Vec<u8>, String)> = s.scan("mail").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, b"a");
        assert!(s.delete("mail", b"a").unwrap());
        assert!(!s.delete("mail", b"a").unwrap());
        assert_eq!(s.count("mail").unwrap(), 1);
        assert_eq!(s.clear("other").unwrap(), 1);
    }

    #[test]
    fn records_are_bound_to_namespace_and_key() {
        let s = LocalStore::ephemeral(&[1; 32]).unwrap();
        s.put("a", b"k", &"v").unwrap();
        let sealed = {
            let tx = s.db.begin_read().unwrap();
            let t = tx.open_table(KV).unwrap();
            t.get(composite("a", b"k").as_slice())
                .unwrap()
                .unwrap()
                .value()
                .to_vec()
        };
        // Moving the sealed bytes to another key must fail to authenticate.
        assert!(s.open_sealed("a", b"other", &sealed).is_err());
        assert!(s.open_sealed("b", b"k", &sealed).is_err());
        assert!(s.open_sealed("a", b"k", &sealed).is_ok());
        // A different device seed cannot read it.
        let other = LocalStore::ephemeral(&[2; 32]).unwrap();
        assert!(other.open_sealed("a", b"k", &sealed).is_err());
    }

    #[test]
    fn schema_version_guard() {
        let s = LocalStore::ephemeral(&[3; 32]).unwrap();
        assert_eq!(s.migrate().unwrap(), SCHEMA_VERSION);
        s.put("meta", b"schema_version", &(SCHEMA_VERSION + 5))
            .unwrap();
        assert!(s.migrate().is_err());
    }
}
