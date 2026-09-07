//! The node's embedded database.
//!
//! One redb file per service under `<data_dir>/`, opened once and shared.
//! redb is ACID with a single writer and many readers, which is the right
//! shape for a node: requests are mostly reads, writes are small and must
//! not be lost on a crash.

use std::path::Path;

use anyhow::Context;
use redb::Database;

/// Opens (or creates) a database file, with a modest cache so a small VPS
/// running several roles does not have each one claim a gigabyte.
pub fn open(data_dir: &Path, name: &str) -> anyhow::Result<Database> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let path = data_dir.join(format!("{name}.redb"));
    let db = Database::builder()
        .set_cache_size(64 << 20)
        .create(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(db)
}

/// Unix seconds.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
