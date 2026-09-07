//! The encrypted keystore.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Why the vault could not be used.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// Filesystem.
    #[error("keystore {path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// The error.
        source: std::io::Error,
    },
    /// Not a vault file.
    #[error("keystore {path} is not a valid vault file: {reason}")]
    Format {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: String,
    },
    /// Wrong passphrase, or tampered.
    #[error("keystore could not be opened: wrong passphrase or the file was altered")]
    Locked,
    /// KDF parameters rejected.
    #[error("key derivation: {0}")]
    Kdf(String),
    /// Randomness unavailable.
    #[error("the OS random source failed")]
    NoRandomness,
    /// The vault already exists and `create` was asked not to overwrite.
    #[error("keystore {0} already exists")]
    Exists(PathBuf),
}

/// What the vault holds, in the clear only in memory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultContents {
    /// The `hash1…` address this device belongs to.
    #[serde(default)]
    pub address: String,
    /// The account's secp256k1 secret, if this device holds it.
    #[serde(default, with = "hex_opt")]
    pub wallet_secret: Option<Vec<u8>>,
    /// The account mnemonic, if the user chose to keep it here.
    #[serde(default)]
    pub mnemonic: Option<String>,
    /// The identity root ed25519 seed, if this device holds it.
    #[serde(default, with = "hex_opt")]
    pub root_seed: Option<Vec<u8>>,
    /// This device's ed25519 seed.
    #[serde(default, with = "hex_opt")]
    pub device_seed: Option<Vec<u8>>,
    /// This device's id as registered on chain.
    #[serde(default)]
    pub device_id: String,
    /// Other named secrets (MLS state, call keys), hex-encoded.
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl Drop for VaultContents {
    fn drop(&mut self) {
        if let Some(s) = &mut self.wallet_secret {
            s.zeroize();
        }
        if let Some(s) = &mut self.root_seed {
            s.zeroize();
        }
        if let Some(s) = &mut self.device_seed {
            s.zeroize();
        }
        if let Some(m) = &mut self.mnemonic {
            m.zeroize();
        }
    }
}

mod hex_opt {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        v.as_ref().map(hex::encode).serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let o: Option<String> = Option::deserialize(d)?;
        o.map(|h| hex::decode(h).map_err(serde::de::Error::custom))
            .transpose()
    }
}

/// The on-disk envelope.
#[derive(Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    kdf: String,
    salt: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    nonce: String,
    ciphertext: String,
}

/// KDF cost. The default is the OWASP interactive recommendation; tests use
/// a light setting so the suite does not spend its time hashing.
#[derive(Debug, Clone, Copy)]
pub struct KdfCost {
    /// Memory in KiB.
    pub m_cost: u32,
    /// Iterations.
    pub t_cost: u32,
    /// Parallelism.
    pub p_cost: u32,
}

impl Default for KdfCost {
    fn default() -> Self {
        Self {
            m_cost: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
        }
    }
}

impl KdfCost {
    /// A cheap setting. DEVNET ONLY: fine for tests, wrong for a real user.
    #[must_use]
    pub fn light() -> Self {
        Self {
            m_cost: 8 * 1024,
            t_cost: 1,
            p_cost: 1,
        }
    }
}

/// An encrypted keystore file.
pub struct Vault {
    path: PathBuf,
}

impl Vault {
    /// Points at a file, creating nothing.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file exists.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    fn io(&self, source: std::io::Error) -> VaultError {
        VaultError::Io {
            path: self.path.clone(),
            source,
        }
    }

    fn derive(
        &self,
        passphrase: &[u8],
        salt: &[u8],
        cost: KdfCost,
    ) -> Result<[u8; 32], VaultError> {
        let params = Params::new(cost.m_cost, cost.t_cost, cost.p_cost, Some(32))
            .map_err(|e| VaultError::Kdf(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut key = [0u8; 32];
        argon
            .hash_password_into(passphrase, salt, &mut key)
            .map_err(|e| VaultError::Kdf(e.to_string()))?;
        Ok(key)
    }

    /// Creates the vault with the given contents.
    pub fn create(
        &self,
        passphrase: &str,
        contents: &VaultContents,
        cost: KdfCost,
    ) -> Result<(), VaultError> {
        if self.exists() {
            return Err(VaultError::Exists(self.path.clone()));
        }
        self.write(passphrase, contents, cost)
    }

    /// Rewrites the vault. The salt and nonce are fresh on every write.
    pub fn write(
        &self,
        passphrase: &str,
        contents: &VaultContents,
        cost: KdfCost,
    ) -> Result<(), VaultError> {
        let mut salt = [0u8; 16];
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut salt).map_err(|_| VaultError::NoRandomness)?;
        getrandom::fill(&mut nonce).map_err(|_| VaultError::NoRandomness)?;
        let mut key = self.derive(passphrase.as_bytes(), &salt, cost)?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        let mut plaintext = serde_json::to_vec(contents).map_err(|e| VaultError::Format {
            path: self.path.clone(),
            reason: e.to_string(),
        })?;
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| VaultError::Locked)?;
        plaintext.zeroize();
        key.zeroize();
        let file = FileFormat {
            version: 1,
            kdf: "argon2id".into(),
            salt: hex::encode(salt),
            m_cost: cost.m_cost,
            t_cost: cost.t_cost,
            p_cost: cost.p_cost,
            nonce: hex::encode(nonce),
            ciphertext: hex::encode(ciphertext),
        };
        let raw = serde_json::to_vec_pretty(&file).map_err(|e| VaultError::Format {
            path: self.path.clone(),
            reason: e.to_string(),
        })?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| self.io(e))?;
        }
        let tmp = self.path.with_extension("tmp");
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)
                .map_err(|e| self.io(e))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                f.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| self.io(e))?;
            }
            f.write_all(&raw).map_err(|e| self.io(e))?;
            f.sync_all().map_err(|e| self.io(e))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| self.io(e))?;
        Ok(())
    }

    /// Opens the vault.
    pub fn open(&self, passphrase: &str) -> Result<VaultContents, VaultError> {
        let raw = std::fs::read(&self.path).map_err(|e| self.io(e))?;
        let file: FileFormat = serde_json::from_slice(&raw).map_err(|e| VaultError::Format {
            path: self.path.clone(),
            reason: e.to_string(),
        })?;
        if file.version != 1 || file.kdf != "argon2id" {
            return Err(VaultError::Format {
                path: self.path.clone(),
                reason: format!("unsupported version {} / kdf {}", file.version, file.kdf),
            });
        }
        let bad = |reason: &str| VaultError::Format {
            path: self.path.clone(),
            reason: reason.to_owned(),
        };
        let salt = hex::decode(&file.salt).map_err(|_| bad("salt"))?;
        let nonce = hex::decode(&file.nonce).map_err(|_| bad("nonce"))?;
        if nonce.len() != 24 {
            return Err(bad("nonce length"));
        }
        let ciphertext = hex::decode(&file.ciphertext).map_err(|_| bad("ciphertext"))?;
        let cost = KdfCost {
            m_cost: file.m_cost,
            t_cost: file.t_cost,
            p_cost: file.p_cost,
        };
        let mut key = self.derive(passphrase.as_bytes(), &salt, cost)?;
        let cipher = XChaCha20Poly1305::new((&key).into());
        let mut plaintext = cipher
            .decrypt(XNonce::from_slice(&nonce), ciphertext.as_slice())
            .map_err(|_| VaultError::Locked)?;
        key.zeroize();
        let contents: VaultContents =
            serde_json::from_slice(&plaintext).map_err(|e| VaultError::Format {
                path: self.path.clone(),
                reason: e.to_string(),
            })?;
        plaintext.zeroize();
        Ok(contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hg-vault-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("keystore.json")
    }

    fn contents() -> VaultContents {
        VaultContents {
            address: "hash1abc".into(),
            wallet_secret: Some(vec![1; 32]),
            mnemonic: Some("word ".repeat(24).trim().into()),
            root_seed: Some(vec![2; 32]),
            device_seed: Some(vec![3; 32]),
            device_id: "laptop".into(),
            extra: BTreeMap::from([("mls".to_owned(), "00ff".to_owned())]),
        }
    }

    #[test]
    fn round_trips_and_refuses_the_wrong_passphrase() {
        let v = Vault::at(tmp("rt"));
        v.create("correct horse", &contents(), KdfCost::light())
            .unwrap();
        assert_eq!(v.open("correct horse").unwrap(), contents());
        assert!(matches!(v.open("wrong"), Err(VaultError::Locked)));
        assert!(matches!(
            v.create("x", &contents(), KdfCost::light()),
            Err(VaultError::Exists(_))
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(v.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn the_file_never_contains_a_secret_in_the_clear() {
        let v = Vault::at(tmp("clear"));
        v.create("pw", &contents(), KdfCost::light()).unwrap();
        let raw = std::fs::read_to_string(v.path()).unwrap();
        assert!(!raw.contains("hash1abc"));
        assert!(!raw.contains("word word"));
        assert!(!raw.contains(&hex::encode([1u8; 32])));
        assert!(!raw.contains("laptop"));
    }

    #[test]
    fn a_tampered_file_is_refused() {
        let v = Vault::at(tmp("tamper"));
        v.create("pw", &contents(), KdfCost::light()).unwrap();
        let mut file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(v.path()).unwrap()).unwrap();
        let ct = file["ciphertext"].as_str().unwrap().to_owned();
        let rest = ct.get(1..).unwrap_or("");
        let flipped = if ct.starts_with('0') {
            format!("1{rest}")
        } else {
            format!("0{rest}")
        };
        file["ciphertext"] = serde_json::Value::String(flipped);
        std::fs::write(v.path(), serde_json::to_vec(&file).unwrap()).unwrap();
        assert!(matches!(v.open("pw"), Err(VaultError::Locked)));
    }

    #[test]
    fn rewriting_changes_salt_and_nonce() {
        let v = Vault::at(tmp("rewrite"));
        v.create("pw", &contents(), KdfCost::light()).unwrap();
        let a: serde_json::Value =
            serde_json::from_slice(&std::fs::read(v.path()).unwrap()).unwrap();
        v.write("pw", &contents(), KdfCost::light()).unwrap();
        let b: serde_json::Value =
            serde_json::from_slice(&std::fs::read(v.path()).unwrap()).unwrap();
        assert_ne!(a["salt"], b["salt"]);
        assert_ne!(a["nonce"], b["nonce"]);
        assert_eq!(v.open("pw").unwrap(), contents());
    }
}
