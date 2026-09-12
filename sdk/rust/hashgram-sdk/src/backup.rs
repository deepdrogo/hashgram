//! Encrypted vault backup: a file the user can keep anywhere that the host
//! cannot decrypt, and that a fresh device imports to replace the exporting
//! one. Format and semantics: `docs/MULTI_DEVICE_SECURITY.md` §7.3.
//!
//! ```text
//! offset  size   field
//! 0       8      magic  "HGBKUP\0\x01"   (last byte = format version)
//! 8       1      kdf id                   0x01 = Argon2id
//! 9       4      m_cost (KiB, u32 LE)
//! 13      4      t_cost (u32 LE)
//! 17      1      p_cost
//! 18      16     salt
//! 34      24     nonce
//! 58      4      ciphertext length (u32 LE)
//! 62      n      ciphertext ‖ 16-byte Poly1305 tag
//! ```
//!
//! AAD = the 58-byte header, so KDF parameters and the version cannot be
//! downgraded without failing authentication. The device seed and device id
//! are deliberately NOT exported (the importer gets its own device), and MLS
//! state is omitted (a second live copy would fork the ratchet).

use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hashgram_identity::vault::KdfCost;
use hashgram_identity::VaultContents;
use hashgram_proto::keys::Ed25519Signer;
use zeroize::Zeroizing;

use crate::account::Account;
use crate::SdkError;

const MAGIC: &[u8; 8] = b"HGBKUP\0\x01";
const KDF_ARGON2ID: u8 = 1;
const HEADER_LEN: usize = 58;
/// Backup KDF: 256 MiB, t=4, p=1 — heavier than the vault because the file
/// is expected to sit on hostile storage.
pub const BACKUP_M_COST_KIB: u32 = 262_144;
/// Iterations.
pub const BACKUP_T_COST: u32 = 4;
/// Lanes.
pub const BACKUP_P_COST: u8 = 1;
/// Minimum backup passphrase length.
pub const MIN_PASSPHRASE: usize = 12;
/// Largest backup accepted on import.
pub const MAX_BACKUP_BYTES: usize = 16 * 1024 * 1024;

/// What travels inside the ciphertext.
#[derive(serde::Serialize, serde::Deserialize)]
struct Payload1 {
    address: String,
    #[serde(default)]
    wallet_secret: Option<String>,
    #[serde(default)]
    mnemonic: Option<String>,
    #[serde(default)]
    root_seed: Option<String>,
    backup: BackupMeta,
    /// Vault `extra` entries worth carrying (Drive keyring, people names);
    /// MLS/cursor/seen state is excluded.
    #[serde(default)]
    extra: std::collections::BTreeMap<String, String>,
}

/// Metadata block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackupMeta {
    /// Unix seconds.
    pub exported_at: u64,
    /// Address.
    pub address: String,
    /// True when the exporting vault was device-only (no account/root key).
    pub partial: bool,
    /// Exporting device id (informational).
    pub from_device: String,
}

/// What an import produced.
pub struct Imported {
    /// The new account (fresh device key, new vault written).
    pub account: Account,
    /// Metadata from the backup.
    pub meta: BackupMeta,
    /// Hex public key of the new device, to be certified by the root
    /// (`Devices::add_on_chain` when the root is present in the backup).
    pub new_device_pubkey_hex: String,
}

fn derive(
    passphrase: &str,
    salt: &[u8],
    m: u32,
    t: u32,
    p: u8,
) -> Result<Zeroizing<[u8; 32]>, SdkError> {
    let params = Params::new(m, t, u32::from(p), Some(32))
        .map_err(|e| SdkError::Invalid(format!("kdf params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut())
        .map_err(|e| SdkError::Invalid(format!("kdf: {e}")))?;
    Ok(out)
}

const EXTRA_KEEP: &[&str] = &[crate::drive::VAULT_DRIVE_KEYRING];

/// Exports a backup of `account` to `path` under an independent passphrase.
/// Returns the metadata written.
pub fn export_backup(
    account: &Account,
    path: &Path,
    passphrase: &str,
    cost: Option<(u32, u32, u8)>,
) -> Result<BackupMeta, SdkError> {
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(SdkError::Invalid(format!(
            "backup passphrase must be at least {MIN_PASSPHRASE} characters"
        )));
    }
    let (m, t, p) = cost.unwrap_or((BACKUP_M_COST_KIB, BACKUP_T_COST, BACKUP_P_COST));
    let c = &account.contents;
    let meta = BackupMeta {
        exported_at: hashgram_app::ids::now_secs(),
        address: c.address.clone(),
        partial: c.wallet_secret.is_none() && c.root_seed.is_none(),
        from_device: c.device_id.clone(),
    };
    let payload = Payload1 {
        address: c.address.clone(),
        wallet_secret: c.wallet_secret.as_ref().map(hex::encode),
        mnemonic: c.mnemonic.clone(),
        root_seed: c.root_seed.as_ref().map(hex::encode),
        backup: meta.clone(),
        extra: c
            .extra
            .iter()
            .filter(|(k, _)| EXTRA_KEEP.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    };
    let plaintext =
        Zeroizing::new(serde_json::to_vec(&payload).map_err(|e| SdkError::Store(e.to_string()))?);
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut salt).map_err(|_| SdkError::Invalid("no randomness".into()))?;
    getrandom::fill(&mut nonce).map_err(|_| SdkError::Invalid("no randomness".into()))?;
    let key = derive(passphrase, &salt, m, t, p)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    // Header first: it is the AAD.
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.push(KDF_ARGON2ID);
    header.extend_from_slice(&m.to_le_bytes());
    header.extend_from_slice(&t.to_le_bytes());
    header.push(p);
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &header,
            },
        )
        .map_err(|_| SdkError::Invalid("backup encryption failed".into()))?;
    let body_len = ct
        .len()
        .checked_sub(16)
        .ok_or_else(|| SdkError::Invalid("ciphertext too short".into()))? as u32;
    let mut out = header;
    out.extend_from_slice(&body_len.to_le_bytes());
    out.extend_from_slice(&ct);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &out).map_err(|e| SdkError::Store(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| SdkError::Store(e.to_string()))?;
    Ok(meta)
}

/// Reads only the header: metadata a UI can show before asking for the
/// passphrase (KDF cost, version). Nothing about the owner is in the clear.
pub fn inspect_backup(bytes: &[u8]) -> Result<(u32, u32, u8), SdkError> {
    if bytes.len() < HEADER_LEN + 4 + 16 || bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err(SdkError::Invalid(
            "not a Hashgram backup (magic/version)".into(),
        ));
    }
    if bytes.get(8) != Some(&KDF_ARGON2ID) {
        return Err(SdkError::Unsupported("backup KDF".into()));
    }
    let u32_at = |o: usize| -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(bytes.get(o..o + 4).unwrap_or(&[0, 0, 0, 0]));
        u32::from_le_bytes(b)
    };
    Ok((u32_at(9), u32_at(13), bytes.get(17).copied().unwrap_or(1)))
}

/// Imports a backup into a fresh vault at `vault_path` (vault passphrase
/// `vault_passphrase`), creating a new device key.
pub fn import_backup(
    bytes: &[u8],
    passphrase: &str,
    vault_path: &Path,
    vault_passphrase: &str,
    device_id: &str,
    kdf: KdfCost,
) -> Result<Imported, SdkError> {
    if bytes.len() > MAX_BACKUP_BYTES {
        return Err(SdkError::Invalid("backup too large".into()));
    }
    let (m, t, p) = inspect_backup(bytes)?;
    if m > 4 * 1024 * 1024 || t > 64 {
        return Err(SdkError::Invalid("backup KDF cost out of range".into()));
    }
    let header = bytes
        .get(..HEADER_LEN)
        .ok_or_else(|| SdkError::Invalid("short".into()))?;
    let salt = header
        .get(18..34)
        .ok_or_else(|| SdkError::Invalid("short".into()))?;
    let nonce = header
        .get(34..58)
        .ok_or_else(|| SdkError::Invalid("short".into()))?;
    let mut lb = [0u8; 4];
    lb.copy_from_slice(
        bytes
            .get(58..62)
            .ok_or_else(|| SdkError::Invalid("short".into()))?,
    );
    let n = u32::from_le_bytes(lb) as usize;
    let ct = bytes
        .get(62..62 + n + 16)
        .ok_or_else(|| SdkError::Invalid("backup truncated".into()))?;
    let key = derive(passphrase, salt, m, t, p)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let pt = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: ct,
                    aad: header,
                },
            )
            .map_err(|_| SdkError::Invalid("wrong backup passphrase or corrupted file".into()))?,
    );
    let payload: Payload1 =
        serde_json::from_slice(&pt).map_err(|e| SdkError::Corrupt(e.to_string()))?;
    let device = Ed25519Signer::generate()?;
    let contents = VaultContents {
        address: payload.address.clone(),
        wallet_secret: payload
            .wallet_secret
            .as_deref()
            .map(hex::decode)
            .transpose()
            .map_err(|e| SdkError::Corrupt(e.to_string()))?,
        mnemonic: payload.mnemonic.clone(),
        root_seed: payload
            .root_seed
            .as_deref()
            .map(hex::decode)
            .transpose()
            .map_err(|e| SdkError::Corrupt(e.to_string()))?,
        device_seed: Some(device.secret_bytes().to_vec()),
        device_id: device_id.to_owned(),
        extra: payload.extra.clone(),
    };
    let account = Account::from_contents(vault_path, vault_passphrase, contents, kdf)?;
    Ok(Imported {
        new_device_pubkey_hex: hex::encode(device.public_key()),
        account,
        meta: payload.backup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_round_trip_without_device_seed() {
        let dir = std::env::temp_dir().join(format!("hg-backup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (acct, _mnemonic) =
            Account::create(&dir.join("v.json"), "vault-pass", "dev-1", KdfCost::light()).unwrap();
        let mut acct = acct;
        acct.contents
            .extra
            .insert(crate::drive::VAULT_DRIVE_KEYRING.into(), "abcd".into());
        acct.contents.extra.insert(
            crate::messaging::VAULT_MLS_KEY.into(),
            "must-not-export".into(),
        );
        let out = dir.join("backup.hgbkup");
        let meta = export_backup(
            &acct,
            &out,
            "a long backup passphrase",
            Some((8 * 1024, 1, 1)),
        )
        .unwrap();
        assert!(!meta.partial);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        // The device seed must not be in the file even encrypted-then-leaked:
        // decrypt and check the payload.
        let imp = import_backup(
            &bytes,
            "a long backup passphrase",
            &dir.join("v2.json"),
            "vault2",
            "dev-2",
            KdfCost::light(),
        )
        .unwrap();
        assert_eq!(imp.account.address(), acct.address());
        assert_eq!(
            imp.account.contents.wallet_secret,
            acct.contents.wallet_secret
        );
        assert_eq!(imp.account.contents.root_seed, acct.contents.root_seed);
        assert_ne!(
            imp.account.contents.device_seed, acct.contents.device_seed,
            "fresh device key"
        );
        assert_eq!(imp.account.contents.device_id, "dev-2");
        assert_eq!(
            imp.account
                .contents
                .extra
                .get(crate::drive::VAULT_DRIVE_KEYRING)
                .map(String::as_str),
            Some("abcd")
        );
        assert!(!imp
            .account
            .contents
            .extra
            .contains_key(crate::messaging::VAULT_MLS_KEY));
        // Wrong passphrase and tampered header fail.
        assert!(import_backup(
            &bytes,
            "a wrong backup passphrase",
            &dir.join("v3.json"),
            "x",
            "d",
            KdfCost::light()
        )
        .is_err());
        let mut bad = bytes.clone();
        bad[9] ^= 1; // m_cost downgrade
        assert!(import_backup(
            &bad,
            "a long backup passphrase",
            &dir.join("v4.json"),
            "x",
            "d",
            KdfCost::light()
        )
        .is_err());
        assert!(export_backup(&acct, &out, "short", None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
