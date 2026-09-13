//! The UI-cache database: SQLite in WAL mode, one connection behind a
//! mutex. It holds nothing the SDK's encrypted store (`local.redb`) holds:
//! submitted transactions the wallet screen watches, recent searches, and
//! sealed per-key UI values (list orderings, thumbnails metadata). A private
//! value goes through `crate::crypto::seal` with the vault-held key before
//! it is written; a test scans the file for a test secret and fails if it
//! is found.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::DbKey;

/// The database handle.
pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_tx (
    hash       TEXT PRIMARY KEY,
    submitted  INTEGER NOT NULL,
    summary    TEXT NOT NULL,
    state      TEXT NOT NULL,
    height     INTEGER NOT NULL DEFAULT 0,
    raw_log    TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS recent_search (
    query   TEXT PRIMARY KEY,
    at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ui_sealed (
    key     TEXT PRIMARY KEY,
    sealed  BLOB NOT NULL,
    updated INTEGER NOT NULL
);
"#;

impl Db {
    /// Opens (creating) the database with WAL and a busy timeout.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Runs `f` with the connection.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "database lock poisoned".to_owned())?;
        f(&conn).map_err(|e| e.to_string())
    }

    /// Stores a public meta value.
    pub fn meta_set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
        })
    }

    /// Reads a public meta value.
    pub fn meta_get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.with(|c| {
            c.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()
        })
    }

    /// Stores a sealed UI value under `key` (AAD = key).
    pub fn sealed_put(&self, db_key: &DbKey, key: &str, json: &[u8]) -> Result<(), String> {
        let sealed = crate::crypto::seal(db_key, key.as_bytes(), json)?;
        self.with(|c| {
            c.execute(
                "INSERT INTO ui_sealed(key, sealed, updated) VALUES(?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET sealed = excluded.sealed, updated = excluded.updated",
                params![key, sealed, now()],
            )
            .map(|_| ())
        })
    }

    /// Reads a sealed UI value.
    pub fn sealed_get(&self, db_key: &DbKey, key: &str) -> Result<Option<Vec<u8>>, String> {
        let sealed: Option<Vec<u8>> = self.with(|c| {
            c.query_row("SELECT sealed FROM ui_sealed WHERE key = ?1", params![key], |r| r.get(0))
                .optional()
        })?;
        match sealed {
            Some(s) => Ok(Some(crate::crypto::open(db_key, key.as_bytes(), &s)?)),
            None => Ok(None),
        }
    }

    /// Records a submitted transaction.
    pub fn pending_put(&self, hash: &str, summary: &str, state: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO pending_tx(hash, submitted, summary, state) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(hash) DO UPDATE SET state = excluded.state",
                params![hash, now(), summary, state],
            )
            .map(|_| ())
        })
    }

    /// Updates a transaction's state.
    pub fn pending_update(&self, hash: &str, state: &str, height: u64, raw_log: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "UPDATE pending_tx SET state = ?2, height = ?3, raw_log = ?4 WHERE hash = ?1",
                params![hash, state, height as i64, raw_log],
            )
            .map(|_| ())
        })
    }

    /// Recent transactions this app submitted, newest first.
    pub fn pending_list(&self, limit: usize) -> Result<Vec<PendingRow>, String> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT hash, submitted, summary, state, height, raw_log FROM pending_tx
                 ORDER BY submitted DESC LIMIT ?1",
            )?;
            let rows = st.query_map(params![limit as i64], |r| {
                Ok(PendingRow {
                    hash: r.get(0)?,
                    submitted: r.get::<_, i64>(1)? as u64,
                    summary: r.get(2)?,
                    state: r.get(3)?,
                    height: r.get::<_, i64>(4)? as u64,
                    raw_log: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Records a search.
    pub fn search_note(&self, query: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO recent_search(query, at) VALUES(?1, ?2)
                 ON CONFLICT(query) DO UPDATE SET at = excluded.at",
                params![query, now()],
            )?;
            c.execute(
                "DELETE FROM recent_search WHERE query NOT IN (SELECT query FROM recent_search ORDER BY at DESC LIMIT 20)",
                [],
            )
            .map(|_| ())
        })
    }

    /// Recent searches, newest first.
    pub fn search_recent(&self) -> Result<Vec<String>, String> {
        self.with(|c| {
            let mut st = c.prepare("SELECT query FROM recent_search ORDER BY at DESC LIMIT 20")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }
}

/// A submitted transaction row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingRow {
    /// Hex hash.
    pub hash: String,
    /// Unix seconds when submitted.
    pub submitted: u64,
    /// Human summary ("Send 10 HASH to hash1…").
    pub summary: String,
    /// `pending`, `committed`, `failed`.
    pub state: String,
    /// Height once committed.
    pub height: u64,
    /// Raw log on failure.
    pub raw_log: String,
}

/// Unix seconds.
#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hg-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("t.db")
    }

    #[test]
    fn opens_with_wal_and_stores_sealed_values() {
        let p = tmp("wal");
        let db = Db::open(&p).unwrap();
        let mode: String = db
            .with(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let key = crate::crypto::generate_key().unwrap();
        db.sealed_put(&key, "mail/order", br#"{"name":"Very Secret Friend"}"#).unwrap();
        assert_eq!(
            db.sealed_get(&key, "mail/order").unwrap().unwrap(),
            br#"{"name":"Very Secret Friend"}"#
        );
        drop(db);
        let mut bytes = std::fs::read(&p).unwrap();
        if let Ok(w) = std::fs::read(p.with_extension("db-wal")) {
            bytes.extend(w);
        }
        assert!(!bytes.windows(18).any(|w| w == b"Very Secret Friend"));
    }

    #[test]
    fn pending_tx_lifecycle() {
        let db = Db::open(&tmp("tx")).unwrap();
        db.pending_put("ABC", "Send 1 HASH", "pending").unwrap();
        db.pending_update("ABC", "committed", 42, "").unwrap();
        let l = db.pending_list(10).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].state, "committed");
        assert_eq!(l[0].height, 42);
    }
}
