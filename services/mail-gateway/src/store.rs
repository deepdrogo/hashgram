//! Local state in SQLite: what was accepted, what still has to be
//! delivered, and how ids map across the bridge.
//!
//! Nothing here is secret in the HashMail sense — every row describes mail
//! that was plaintext on the Internet anyway — but the file still holds
//! envelope metadata and (in the retry queue) whole messages until they
//! are delivered, so it must live on the operator's disk with the same
//! protection as any MTA spool.
//!
//! Migrations are embedded SQL applied in order; `schema_version` records
//! the last one applied so a newer binary upgrades an older file in place
//! and an older binary refuses a newer file instead of misreading it.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::GatewayError;

/// Every migration, in order. Append only; never edit a shipped entry.
const MIGRATIONS: &[&str] = &[
    // v1: initial schema.
    r#"
    CREATE TABLE inbound (
        id INTEGER PRIMARY KEY,
        received_at INTEGER NOT NULL,
        from_header TEXT NOT NULL,
        rcpt TEXT NOT NULL,
        message_id_header TEXT NOT NULL,
        hashgram_message_id TEXT NOT NULL,
        status TEXT NOT NULL
    );
    CREATE INDEX inbound_received_at ON inbound(received_at);
    CREATE INDEX inbound_hashgram_id ON inbound(hashgram_message_id);

    CREATE TABLE outbound (
        id INTEGER PRIMARY KEY,
        created_at INTEGER NOT NULL,
        hashgram_message_id TEXT NOT NULL UNIQUE,
        sender TEXT NOT NULL,
        rcpt TEXT NOT NULL,
        message_id_header TEXT NOT NULL,
        status TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT NOT NULL DEFAULT ''
    );

    CREATE TABLE queue (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL,
        payload BLOB NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_at INTEGER NOT NULL,
        last_error TEXT NOT NULL DEFAULT '',
        created_at INTEGER NOT NULL
    );
    CREATE INDEX queue_next ON queue(next_attempt_at);

    CREATE TABLE thread_map (
        hashgram_id TEXT PRIMARY KEY,
        message_id_header TEXT NOT NULL UNIQUE,
        thread_id TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );

    CREATE TABLE cursor (
        name TEXT PRIMARY KEY,
        value INTEGER NOT NULL
    );

    CREATE TABLE rate_events (
        key TEXT NOT NULL,
        at INTEGER NOT NULL
    );
    CREATE INDEX rate_events_key ON rate_events(key, at);
    "#,
];

/// Delivery status strings kept in `inbound.status` / `outbound.status`.
pub mod status {
    /// Waiting in the queue.
    pub const QUEUED: &str = "queued";
    /// Handed to the network.
    pub const DELIVERED: &str = "delivered";
    /// Given up; a bounce was produced where possible.
    pub const FAILED: &str = "failed";
    /// Refused by policy before queueing.
    pub const REJECTED: &str = "rejected";
}

/// Kinds of queue entries.
pub mod kind {
    /// An accepted SMTP message waiting to be sent into HashMail.
    pub const INBOUND: &str = "inbound";
    /// A rendered Internet message waiting for the remote MX.
    pub const OUTBOUND: &str = "outbound";
}

/// A queue row with its decoded payload.
#[derive(Debug, Clone)]
pub struct QueueItem<T> {
    /// Row id.
    pub id: i64,
    /// How many attempts were made before this one.
    pub attempts: u32,
    /// The payload.
    pub payload: T,
}

/// A `thread_map` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRow {
    /// HashMail message id, hex.
    pub hashgram_id: String,
    /// Normalised `Message-ID` value (no angle brackets).
    pub message_id_header: String,
    /// HashMail thread id, hex.
    pub thread_id: String,
}

/// The store handle. Cheap to share behind an `Arc`; SQLite serialises
/// access through the mutex, which is fine at mail-gateway volumes.
pub struct Store {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

fn sql_err(e: rusqlite::Error) -> GatewayError {
    GatewayError::Store(e.to_string())
}

impl Store {
    /// Opens (creating if needed) and migrates.
    pub fn open(path: &Path) -> Result<Self, GatewayError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| GatewayError::Store(e.to_string()))?;
            }
        }
        let conn = Connection::open(path).map_err(sql_err)?;
        Self::init(conn)
    }

    /// An in-memory store (tests).
    pub fn open_in_memory() -> Result<Self, GatewayError> {
        Self::init(Connection::open_in_memory().map_err(sql_err)?)
    }

    fn init(conn: Connection) -> Result<Self, GatewayError> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);",
        )
        .map_err(sql_err)?;
        let current: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .map_err(sql_err)?;
        let latest = MIGRATIONS.len() as i64;
        if current > latest {
            return Err(GatewayError::Store(format!(
                "database schema version {current} is newer than this binary supports ({latest})"
            )));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = i as i64 + 1;
            if v <= current {
                continue;
            }
            conn.execute_batch("BEGIN;").map_err(sql_err)?;
            let applied = conn.execute_batch(sql).and_then(|()| {
                conn.execute(
                    "INSERT INTO schema_version(version) VALUES (?1)",
                    params![v],
                )
            });
            match applied {
                Ok(_) => conn.execute_batch("COMMIT;").map_err(sql_err)?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK;");
                    return Err(GatewayError::Store(format!("migration {v}: {e}")));
                }
            }
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, GatewayError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| GatewayError::Store("store mutex poisoned".into()))?;
        f(&guard).map_err(sql_err)
    }

    /// Current schema version.
    pub fn schema_version(&self) -> Result<i64, GatewayError> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
        })
    }

    // -----------------------------------------------------------------------
    // Inbound log
    // -----------------------------------------------------------------------

    /// Records an accepted (or policy-rejected) inbound message. Returns the
    /// row id.
    pub fn log_inbound(
        &self,
        received_at: u64,
        from_header: &str,
        rcpt: &str,
        message_id_header: &str,
        hashgram_message_id: &str,
        status_: &str,
    ) -> Result<i64, GatewayError> {
        self.with(|c| {
            c.execute(
                "INSERT INTO inbound(received_at, from_header, rcpt, message_id_header, hashgram_message_id, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![received_at as i64, from_header, rcpt, message_id_header, hashgram_message_id, status_],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Updates the status of every inbound row for a HashMail id.
    pub fn set_inbound_status(
        &self,
        hashgram_message_id: &str,
        status_: &str,
    ) -> Result<(), GatewayError> {
        self.with(|c| {
            c.execute(
                "UPDATE inbound SET status = ?2 WHERE hashgram_message_id = ?1",
                params![hashgram_message_id, status_],
            )?;
            Ok(())
        })
    }

    /// Whether an inbound message with this HashMail id was already
    /// accepted (dedup across repeated SMTP deliveries).
    pub fn inbound_seen(&self, hashgram_message_id: &str) -> Result<bool, GatewayError> {
        self.with(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM inbound WHERE hashgram_message_id = ?1 AND status != ?2",
                params![hashgram_message_id, status::REJECTED],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
    }

    /// Counts inbound rows by status (metrics / runbook).
    pub fn inbound_counts(&self) -> Result<Vec<(String, i64)>, GatewayError> {
        self.with(|c| {
            let mut st = c.prepare("SELECT status, COUNT(*) FROM inbound GROUP BY status")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect()
        })
    }

    // -----------------------------------------------------------------------
    // Outbound log
    // -----------------------------------------------------------------------

    /// Records an outbound message once; returns `false` if it was already
    /// recorded (the inbox scan saw it before).
    pub fn log_outbound(
        &self,
        created_at: u64,
        hashgram_message_id: &str,
        sender: &str,
        rcpt: &str,
        message_id_header: &str,
        status_: &str,
    ) -> Result<bool, GatewayError> {
        self.with(|c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO outbound(created_at, hashgram_message_id, sender, rcpt, message_id_header, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![created_at as i64, hashgram_message_id, sender, rcpt, message_id_header, status_],
            )?;
            Ok(n == 1)
        })
    }

    /// Updates an outbound row after an attempt.
    pub fn set_outbound_status(
        &self,
        hashgram_message_id: &str,
        status_: &str,
        last_error: &str,
    ) -> Result<(), GatewayError> {
        self.with(|c| {
            c.execute(
                "UPDATE outbound SET status = ?2, attempts = attempts + 1, last_error = ?3 WHERE hashgram_message_id = ?1",
                params![hashgram_message_id, status_, last_error],
            )?;
            Ok(())
        })
    }

    /// Whether an outbound message was already seen.
    pub fn outbound_seen(&self, hashgram_message_id: &str) -> Result<bool, GatewayError> {
        self.with(|c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM outbound WHERE hashgram_message_id = ?1",
                params![hashgram_message_id],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
    }

    // -----------------------------------------------------------------------
    // Retry queue
    // -----------------------------------------------------------------------

    /// Enqueues a payload for `kind`, due at `next_attempt_at` (unix secs).
    pub fn enqueue<T: Serialize>(
        &self,
        kind_: &str,
        payload: &T,
        now: u64,
        next_attempt_at: u64,
    ) -> Result<i64, GatewayError> {
        let bytes = serde_json::to_vec(payload).map_err(|e| GatewayError::Store(e.to_string()))?;
        self.with(|c| {
            c.execute(
                "INSERT INTO queue(kind, payload, attempts, next_attempt_at, created_at) VALUES (?1, ?2, 0, ?3, ?4)",
                params![kind_, bytes, next_attempt_at as i64, now as i64],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Due items of a kind, oldest first, at most `limit`. Payloads that
    /// no longer decode (a schema change in the payload struct) are dropped
    /// with their row so they cannot wedge the queue.
    pub fn due<T: DeserializeOwned>(
        &self,
        kind_: &str,
        now: u64,
        limit: usize,
    ) -> Result<Vec<QueueItem<T>>, GatewayError> {
        let rows: Vec<(i64, u32, Vec<u8>)> = self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, attempts, payload FROM queue WHERE kind = ?1 AND next_attempt_at <= ?2
                 ORDER BY next_attempt_at, id LIMIT ?3",
            )?;
            let rows = st.query_map(params![kind_, now as i64, limit as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)? as u32,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            })?;
            rows.collect()
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, attempts, bytes) in rows {
            match serde_json::from_slice::<T>(&bytes) {
                Ok(payload) => out.push(QueueItem {
                    id,
                    attempts,
                    payload,
                }),
                Err(e) => {
                    tracing::warn!(queue_id = id, error = %e, "dropping undecodable queue row");
                    self.dequeue(id)?;
                }
            }
        }
        Ok(out)
    }

    /// Removes a queue row (delivered or given up).
    pub fn dequeue(&self, id: i64) -> Result<(), GatewayError> {
        self.with(|c| {
            c.execute("DELETE FROM queue WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Records a failed attempt and reschedules.
    pub fn reschedule(
        &self,
        id: i64,
        next_attempt_at: u64,
        error: &str,
    ) -> Result<(), GatewayError> {
        let error: String = error.chars().take(512).collect();
        self.with(|c| {
            c.execute(
                "UPDATE queue SET attempts = attempts + 1, next_attempt_at = ?2, last_error = ?3 WHERE id = ?1",
                params![id, next_attempt_at as i64, error],
            )?;
            Ok(())
        })
    }

    /// Queue depth per kind.
    pub fn queue_depth(&self, kind_: &str) -> Result<i64, GatewayError> {
        self.with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM queue WHERE kind = ?1",
                params![kind_],
                |r| r.get(0),
            )
        })
    }

    // -----------------------------------------------------------------------
    // Thread map
    // -----------------------------------------------------------------------

    /// Remembers a pairing. A repeated insert for the same id is a no-op
    /// (both directions derive ids deterministically, so collisions are
    /// re-deliveries, not conflicts).
    pub fn map_thread(&self, row: &ThreadRow, now: u64) -> Result<(), GatewayError> {
        self.with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO thread_map(hashgram_id, message_id_header, thread_id, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![row.hashgram_id, row.message_id_header, row.thread_id, now as i64],
            )?;
            Ok(())
        })
    }

    /// By HashMail id (hex).
    pub fn thread_by_hashgram_id(
        &self,
        hashgram_id: &str,
    ) -> Result<Option<ThreadRow>, GatewayError> {
        self.with(|c| {
            c.query_row(
                "SELECT hashgram_id, message_id_header, thread_id FROM thread_map WHERE hashgram_id = ?1",
                params![hashgram_id],
                row_to_thread,
            )
            .optional()
        })
    }

    /// By normalised `Message-ID` value.
    pub fn thread_by_header(
        &self,
        message_id_header: &str,
    ) -> Result<Option<ThreadRow>, GatewayError> {
        self.with(|c| {
            c.query_row(
                "SELECT hashgram_id, message_id_header, thread_id FROM thread_map WHERE message_id_header = ?1",
                params![message_id_header],
                row_to_thread,
            )
            .optional()
        })
    }

    // -----------------------------------------------------------------------
    // Cursors and rate windows
    // -----------------------------------------------------------------------

    /// A named cursor (e.g. last inbox `received_at_ms` scanned).
    pub fn cursor(&self, name: &str) -> Result<u64, GatewayError> {
        self.with(|c| {
            let v: Option<i64> = c
                .query_row(
                    "SELECT value FROM cursor WHERE name = ?1",
                    params![name],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(v.unwrap_or(0).max(0) as u64)
        })
    }

    /// Sets a cursor.
    pub fn set_cursor(&self, name: &str, value: u64) -> Result<(), GatewayError> {
        self.with(|c| {
            c.execute(
                "INSERT INTO cursor(name, value) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET value = excluded.value",
                params![name, value as i64],
            )?;
            Ok(())
        })
    }

    /// Records an event for a rate key and returns how many events the key
    /// has in the trailing `window_secs` (including this one). Old events
    /// are pruned as a side effect.
    pub fn rate_hit(&self, key: &str, now: u64, window_secs: u64) -> Result<u32, GatewayError> {
        let floor = now.saturating_sub(window_secs) as i64;
        self.with(|c| {
            c.execute(
                "DELETE FROM rate_events WHERE key = ?1 AND at < ?2",
                params![key, floor],
            )?;
            c.execute(
                "INSERT INTO rate_events(key, at) VALUES (?1, ?2)",
                params![key, now as i64],
            )?;
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM rate_events WHERE key = ?1 AND at >= ?2",
                params![key, floor],
                |r| r.get(0),
            )?;
            Ok(n.max(0) as u32)
        })
    }
}

fn row_to_thread(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        hashgram_id: r.get(0)?,
        message_id_header: r.get(1)?,
        thread_id: r.get(2)?,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_reports_version() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn reopening_a_file_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.sqlite");
        {
            let s = Store::open(&path).unwrap();
            s.set_cursor("x", 42).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.cursor("x").unwrap(), 42);
        assert_eq!(s.schema_version().unwrap(), 1);
    }

    #[test]
    fn thread_map_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let row = ThreadRow {
            hashgram_id: "aa".repeat(16),
            message_id_header: "x@example.com".into(),
            thread_id: "bb".repeat(16),
        };
        s.map_thread(&row, 1).unwrap();
        s.map_thread(&row, 2).unwrap(); // no-op
        assert_eq!(
            s.thread_by_hashgram_id(&row.hashgram_id).unwrap().unwrap(),
            row
        );
        assert_eq!(s.thread_by_header("x@example.com").unwrap().unwrap(), row);
        assert!(s.thread_by_header("nope").unwrap().is_none());
    }

    #[test]
    fn queue_due_reschedule_dequeue() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .enqueue(kind::INBOUND, &vec![1u8, 2, 3], 100, 100)
            .unwrap();
        s.enqueue(kind::INBOUND, &vec![9u8], 100, 500).unwrap();
        let due: Vec<QueueItem<Vec<u8>>> = s.due(kind::INBOUND, 100, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].payload, vec![1, 2, 3]);
        assert_eq!(due[0].attempts, 0);
        s.reschedule(id, 300, "boom").unwrap();
        let due: Vec<QueueItem<Vec<u8>>> = s.due(kind::INBOUND, 200, 10).unwrap();
        assert!(due.is_empty());
        let due: Vec<QueueItem<Vec<u8>>> = s.due(kind::INBOUND, 600, 10).unwrap();
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].attempts, 1);
        s.dequeue(id).unwrap();
        assert_eq!(s.queue_depth(kind::INBOUND).unwrap(), 1);
        // Undecodable payload rows are dropped rather than wedging the queue.
        let bad: Vec<QueueItem<u32>> = s.due(kind::INBOUND, 600, 10).unwrap();
        assert!(bad.is_empty());
        assert_eq!(s.queue_depth(kind::INBOUND).unwrap(), 0);
    }

    #[test]
    fn inbound_and_outbound_logs() {
        let s = Store::open_in_memory().unwrap();
        s.log_inbound(
            1,
            "a@example.com",
            "alice@hashgram.io",
            "m@example.com",
            "id1",
            status::QUEUED,
        )
        .unwrap();
        assert!(s.inbound_seen("id1").unwrap());
        assert!(!s.inbound_seen("id2").unwrap());
        s.set_inbound_status("id1", status::DELIVERED).unwrap();
        let counts = s.inbound_counts().unwrap();
        assert_eq!(counts, vec![(status::DELIVERED.to_owned(), 1)]);
        assert!(s
            .log_outbound(
                1,
                "o1",
                "hash1x",
                "b@example.com",
                "o1@hashgram.io",
                status::QUEUED
            )
            .unwrap());
        assert!(!s
            .log_outbound(
                1,
                "o1",
                "hash1x",
                "b@example.com",
                "o1@hashgram.io",
                status::QUEUED
            )
            .unwrap());
        assert!(s.outbound_seen("o1").unwrap());
        s.set_outbound_status("o1", status::FAILED, "550").unwrap();
    }

    #[test]
    fn rate_window_counts_and_prunes() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.rate_hit("u", 1000, 60).unwrap(), 1);
        assert_eq!(s.rate_hit("u", 1010, 60).unwrap(), 2);
        assert_eq!(s.rate_hit("v", 1010, 60).unwrap(), 1);
        assert_eq!(s.rate_hit("u", 1100, 60).unwrap(), 1); // first two pruned
    }
}
