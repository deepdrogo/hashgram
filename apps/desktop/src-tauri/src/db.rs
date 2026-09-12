//! The local database: SQLite in WAL mode, one connection behind a mutex.
//!
//! Plain columns hold only public data (addresses, heights, peer ids).
//! Anything private — a decrypted message, a display name we typed, a
//! contact note — goes through `crate::crypto::seal` with the vault-held
//! database key before it is written. A test scans the file for a test
//! message and a test mnemonic and fails if either is found.

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
CREATE TABLE IF NOT EXISTS tx_cache (
    hash      TEXT PRIMARY KEY,
    address   TEXT NOT NULL,
    height    INTEGER NOT NULL,
    timestamp TEXT NOT NULL,
    kind      TEXT NOT NULL,
    body      BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS tx_cache_addr ON tx_cache(address, height DESC);
CREATE TABLE IF NOT EXISTS pending_tx (
    hash       TEXT PRIMARY KEY,
    submitted  INTEGER NOT NULL,
    summary    TEXT NOT NULL,
    state      TEXT NOT NULL,
    height     INTEGER NOT NULL DEFAULT 0,
    raw_log    TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS peer_meta (
    peer_id    TEXT PRIMARY KEY,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    discovery  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS contacts (
    address   TEXT PRIMARY KEY,
    sealed    BLOB NOT NULL,
    updated   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS recent_search (
    query   TEXT PRIMARY KEY,
    at      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id        TEXT PRIMARY KEY,
    group_id  TEXT NOT NULL,
    sender    TEXT NOT NULL,
    device    TEXT NOT NULL,
    ts        INTEGER NOT NULL,
    kind      TEXT NOT NULL,
    sealed    BLOB NOT NULL,
    state     TEXT NOT NULL DEFAULT 'sent',
    expires   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_group ON messages(group_id, ts DESC);
CREATE TABLE IF NOT EXISTS conversations (
    group_id  TEXT PRIMARY KEY,
    sealed    BLOB NOT NULL,
    updated   INTEGER NOT NULL,
    unread    INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS social_events (
    id        TEXT PRIMARY KEY,
    author    TEXT NOT NULL,
    sequence  INTEGER NOT NULL,
    kind      TEXT NOT NULL,
    ts        INTEGER NOT NULL,
    verified  INTEGER NOT NULL,
    body      BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS social_author ON social_events(author, sequence DESC);
CREATE INDEX IF NOT EXISTS social_ts ON social_events(ts DESC);
CREATE TABLE IF NOT EXISTS follows (
    address  TEXT PRIMARY KEY,
    since    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blocks (
    address  TEXT PRIMARY KEY,
    mode     TEXT NOT NULL,
    since    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS blob_cache (
    cid       TEXT PRIMARY KEY,
    mime      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    path      TEXT NOT NULL,
    last_used INTEGER NOT NULL
);
"#;

impl Db {
    /// Opens (creating) the database with WAL and a busy timeout, so a
    /// second process or a slow disk yields a wait, not a lock error.
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
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;",
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

    /// Stores a sealed contact record (display name, note) for an address.
    pub fn contact_put(&self, key: &DbKey, address: &str, json: &[u8]) -> Result<(), String> {
        let sealed = crate::crypto::seal(key, address.as_bytes(), json)?;
        self.with(|c| {
            c.execute(
                "INSERT INTO contacts(address, sealed, updated) VALUES(?1, ?2, ?3)
                 ON CONFLICT(address) DO UPDATE SET sealed = excluded.sealed, updated = excluded.updated",
                params![address, sealed, now()],
            )
            .map(|_| ())
        })
    }

    /// Reads a contact record.
    pub fn contact_get(&self, key: &DbKey, address: &str) -> Result<Option<Vec<u8>>, String> {
        let sealed: Option<Vec<u8>> = self.with(|c| {
            c.query_row(
                "SELECT sealed FROM contacts WHERE address = ?1",
                params![address],
                |r| r.get(0),
            )
            .optional()
        })?;
        match sealed {
            Some(s) => Ok(Some(crate::crypto::open(key, address.as_bytes(), &s)?)),
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
    pub fn pending_update(
        &self,
        hash: &str,
        state: &str,
        height: u64,
        raw_log: &str,
    ) -> Result<(), String> {
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

    /// Notes a peer sighting and how it was discovered (first sighting wins
    /// for the discovery layer).
    pub fn peer_seen(&self, peer_id: &str, discovery: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO peer_meta(peer_id, first_seen, last_seen, discovery) VALUES(?1, ?2, ?2, ?3)
                 ON CONFLICT(peer_id) DO UPDATE SET last_seen = excluded.last_seen",
                params![peer_id, now(), discovery],
            )
            .map(|_| ())
        })
    }

    /// Discovery layer recorded for a peer.
    pub fn peer_discovery(&self, peer_id: &str) -> Result<Option<String>, String> {
        self.with(|c| {
            c.query_row(
                "SELECT discovery FROM peer_meta WHERE peer_id = ?1",
                params![peer_id],
                |r| r.get(0),
            )
            .optional()
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

    /// Forgets everything peer-related (the "forget peers" button).
    pub fn forget_peers(&self) -> Result<(), String> {
        self.with(|c| c.execute("DELETE FROM peer_meta", []).map(|_| ()))
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
    fn opens_with_wal_and_stores_sealed_contacts() {
        let p = tmp("wal");
        let db = Db::open(&p).unwrap();
        let mode: String = db
            .with(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let key = crate::crypto::generate_key().unwrap();
        db.contact_put(&key, "hash1abc", br#"{"name":"Very Secret Friend"}"#)
            .unwrap();
        assert_eq!(
            db.contact_get(&key, "hash1abc").unwrap().unwrap(),
            br#"{"name":"Very Secret Friend"}"#
        );
        // The plaintext is nowhere in the file (WAL included).
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
