//! Where the app keeps its files.
//!
//! Everything lives under `%LOCALAPPDATA%\Hashgram\data` — local, not
//! roaming: the vault and the database are bound to this machine (DPAPI,
//! Windows Hello), and roaming an encrypted vault between PCs is exactly what
//! the 24 words are for. The per-user installer puts the program itself in
//! `%LOCALAPPDATA%\Hashgram`; keeping the data in a subfolder means an
//! update or an uninstall that removes installed files never touches it.
//! `HASHGRAM_DESKTOP_HOME` overrides the root for tests and portable use.

use std::path::{Path, PathBuf};

/// Files the 0.1.0 preview wrote next to the binaries; moved into `data\` once.
const LEGACY_ENTRIES: &[&str] = &[
    "vault.json",
    "settings.json",
    "hashgram.db",
    "hashgram.db-wal",
    "hashgram.db-shm",
    "peers.json",
    "hello.bin",
    "logs",
    "media-cache",
    "node",
];

/// The data root.
#[must_use]
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("HASHGRAM_DESKTOP_HOME") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        if !p.trim().is_empty() {
            return PathBuf::from(p).join("Hashgram").join("data");
        }
    }
    std::env::temp_dir().join("Hashgram").join("data")
}

/// Ensures the data root exists, moving a 0.1.0 layout into place first and
/// renaming a 0.1.x vault to the SDK's file name (same format).
pub fn ensure_dirs() -> std::io::Result<PathBuf> {
    let d = data_dir();
    if std::env::var("HASHGRAM_DESKTOP_HOME")
        .map(|p| p.trim().is_empty())
        .unwrap_or(true)
    {
        if let Some(parent) = d.parent() {
            migrate_legacy_layout(parent, &d);
        }
    }
    std::fs::create_dir_all(&d)?;
    std::fs::create_dir_all(d.join("logs"))?;
    std::fs::create_dir_all(d.join("cache"))?;
    std::fs::create_dir_all(d.join("tmp"))?;
    migrate_vault_name(&d);
    Ok(d)
}

/// 0.1.x wrote the vault as `vault.json`; the SDK's `Paths` names it
/// `keystore.json`. Same format, so a rename is the whole migration. The
/// peerstore is renamed too (`peers.json` → `peerstore.json`).
fn migrate_vault_name(d: &Path) {
    let old = d.join("vault.json");
    let new = d.join("keystore.json");
    if old.exists() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
    let old = d.join("peers.json");
    let new = d.join("peerstore.json");
    if old.exists() && !new.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

/// Moves the known data files from `parent` into `data` when `data` does not
/// exist yet and a vault or database is found beside the binaries. Best
/// effort: a file that cannot be moved is left where it is.
fn migrate_legacy_layout(parent: &Path, data: &Path) {
    if data.exists() {
        return;
    }
    let has_legacy = parent.join("vault.json").exists() || parent.join("hashgram.db").exists();
    if !has_legacy {
        return;
    }
    if std::fs::create_dir_all(data).is_err() {
        return;
    }
    for name in LEGACY_ENTRIES {
        let from = parent.join(name);
        if from.exists() {
            let _ = std::fs::rename(&from, data.join(name));
        }
    }
}

/// The SDK's file layout under the data root (`keystore.json`,
/// `local.redb`, `peerstore.json`, `cache/`).
#[must_use]
pub fn sdk_paths() -> hashgram_sdk::Paths {
    hashgram_sdk::Paths::new(&data_dir())
}

/// The encrypted vault (keys). Never plaintext.
#[must_use]
pub fn vault_path() -> PathBuf {
    sdk_paths().vault()
}

/// Settings (no secrets).
#[must_use]
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// The SQLite database for UI caches (sensitive columns sealed with the
/// vault-held key). The SDK's `local.redb` is the source of truth.
#[must_use]
pub fn db_path() -> PathBuf {
    data_dir().join("ui-cache.db")
}

/// The SDK's encrypted local store.
#[must_use]
pub fn store_path() -> PathBuf {
    sdk_paths().store()
}

/// The persisted peerstore (peer ids and addresses only).
#[must_use]
pub fn peerstore_path() -> PathBuf {
    sdk_paths().peerstore()
}

/// Scratch files for "Open" (decrypted attachments / Drive files). Wiped
/// at start and at lock.
#[must_use]
pub fn tmp_dir() -> PathBuf {
    data_dir().join("tmp")
}

/// DPAPI-wrapped passphrase blob for Windows Hello unlock.
#[must_use]
pub fn hello_blob_path() -> PathBuf {
    data_dir().join("hello.bin")
}

/// Log directory.
#[must_use]
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

#[cfg(test)]
mod tests {
    use super::migrate_legacy_layout;

    #[test]
    fn a_preview_layout_moves_into_data_once() {
        let root = std::env::temp_dir().join(format!("hg-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(root.join("vault.json"), b"{}").unwrap();
        std::fs::write(root.join("hashgram-desktop.exe"), b"MZ").unwrap();
        let data = root.join("data");

        migrate_legacy_layout(&root, &data);
        assert!(data.join("vault.json").exists());
        assert!(data.join("logs").is_dir());
        assert!(!root.join("vault.json").exists());
        assert!(
            root.join("hashgram-desktop.exe").exists(),
            "binaries stay where the installer put them"
        );

        // Second run: data exists, nothing else moves.
        std::fs::write(root.join("peers.json"), b"[]").unwrap();
        migrate_legacy_layout(&root, &data);
        assert!(root.join("peers.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_0_1_vault_is_renamed_to_the_sdk_name_once() {
        let root = std::env::temp_dir().join(format!("hg-paths-vault-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("vault.json"), b"{\"v\":1}").unwrap();
        super::migrate_vault_name(&root);
        assert!(root.join("keystore.json").exists());
        assert!(!root.join("vault.json").exists());
        // A later vault.json (say, restored by hand) never overwrites.
        std::fs::write(root.join("vault.json"), b"{\"v\":2}").unwrap();
        super::migrate_vault_name(&root);
        assert_eq!(
            std::fs::read(root.join("keystore.json")).unwrap(),
            b"{\"v\":1}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_clean_install_creates_nothing_to_migrate() {
        let root = std::env::temp_dir().join(format!("hg-paths-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        migrate_legacy_layout(&root, &root.join("data"));
        assert!(!root.join("data").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
