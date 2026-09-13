//! Process-wide state behind Tauri's managed state.
//!
//! One `HashgramOne` value is the whole unlocked session. It lives in a
//! `tokio::sync::Mutex<Option<_>>`: every command locks it, calls the SDK,
//! saves, and lets go. Locking the vault drops the value, which zeroises
//! the keys it holds. The P2P link outlives the session so the app stays
//! connected (and a local node keeps its chain proxy) while locked.

use std::sync::Arc;
use std::time::Instant;

use hashgram_sdk::link::Link;
use hashgram_sdk::sync::SyncPhase;
use hashgram_sdk::HashgramOne;
use tokio::sync::{Mutex, RwLock};
use zeroize::Zeroizing;

use crate::crypto::DbKey;
use crate::db::Db;
use crate::error::{CmdResult, UiError};
use crate::perf::PerfStore;
use crate::settings::Settings;

/// A mnemonic shown to the user during onboarding and not yet committed to
/// a vault. Zeroised on drop.
pub type PendingMnemonic = Zeroizing<String>;

/// Cheap facts about the unlocked session (read without the SDK lock).
#[derive(Clone)]
pub struct SessionInfo {
    /// Address.
    pub address: String,
    /// This device's id.
    pub device_id: String,
    /// This device holds the wallet key.
    pub has_wallet_key: bool,
    /// This device holds the root key.
    pub has_root_key: bool,
    /// UI cache column key from the vault.
    pub db_key: DbKey,
    /// When it was unlocked.
    pub unlocked_at: Instant,
    /// Last user activity, for auto-lock.
    pub last_activity: Instant,
}

/// What the sync loop last reported.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SyncStatus {
    /// Engine phase.
    pub phase: SyncPhase,
    /// Unix ms of the last successful round.
    pub last_ok_ms: Option<u64>,
    /// Rounds run this session.
    pub rounds: u64,
    /// Last error text, if the last round failed.
    pub last_error: Option<String>,
    /// Verified peers right now.
    pub peers: usize,
    /// Latest balance seen (uhash string).
    pub balance_uhash: Option<String>,
}

impl Default for SyncStatus {
    fn default() -> Self {
        Self {
            phase: SyncPhase::Offline,
            last_ok_ms: None,
            rounds: 0,
            last_error: None,
            peers: 0,
            balance_uhash: None,
        }
    }
}

/// Everything.
pub struct AppState {
    /// Settings.
    pub settings: RwLock<Settings>,
    /// The unlocked session's facade, if any.
    pub one: Mutex<Option<HashgramOne>>,
    /// Facts about the session that need no SDK lock.
    pub session: RwLock<Option<SessionInfo>>,
    /// The P2P link, started at boot and kept across lock/unlock.
    pub link: RwLock<Option<Arc<Link>>>,
    /// Why the link is not up, when it is not.
    pub link_error: RwLock<Option<String>>,
    /// Mnemonic generated during onboarding, before the vault exists.
    pub pending_mnemonic: Mutex<Option<PendingMnemonic>>,
    /// UI cache database.
    pub db: Arc<Db>,
    /// Performance store.
    pub perf: Arc<PerfStore>,
    /// Sync loop status.
    pub sync: RwLock<SyncStatus>,
    /// The sync loop task (aborted at lock).
    pub sync_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Transactions still pending (hashes), watched by a background task.
    pub pending_tx: Mutex<Vec<String>>,
    /// Ask the sync loop to run a round now.
    pub sync_wake: tokio::sync::Notify,
}

impl AppState {
    /// The unlocked facade or `locked`.
    pub fn unlocked(g: &mut Option<HashgramOne>) -> CmdResult<&mut HashgramOne> {
        g.as_mut().ok_or_else(UiError::locked)
    }

    /// Whether a session is open.
    pub async fn is_unlocked(&self) -> bool {
        self.session.read().await.is_some()
    }

    /// The session's address or `locked`.
    pub async fn address(&self) -> CmdResult<String> {
        self.session
            .read()
            .await
            .as_ref()
            .map(|s| s.address.clone())
            .ok_or_else(UiError::locked)
    }

    /// The session's UI-cache key or `locked`.
    pub async fn db_key(&self) -> CmdResult<DbKey> {
        self.session
            .read()
            .await
            .as_ref()
            .map(|s| s.db_key.clone())
            .ok_or_else(UiError::locked)
    }

    /// Milliseconds since unlock, or `None` when locked.
    pub async fn unlocked_for_ms(&self) -> Option<u64> {
        self.session
            .read()
            .await
            .as_ref()
            .map(|s| s.unlocked_at.elapsed().as_millis() as u64)
    }

    /// Touches the activity timer.
    pub async fn touch(&self) {
        if let Some(s) = self.session.write().await.as_mut() {
            s.last_activity = Instant::now();
        }
    }

    /// Whether the auto-lock timer elapsed.
    pub async fn auto_lock_due(&self) -> bool {
        let minutes = self.settings.read().await.security.auto_lock_minutes;
        if minutes == 0 {
            return false;
        }
        self.session
            .read()
            .await
            .as_ref()
            .map(|s| s.last_activity.elapsed().as_secs() >= u64::from(minutes) * 60)
            .unwrap_or(false)
    }

    /// The link, or an `offline` error explaining why there is none.
    pub async fn link(&self) -> CmdResult<Arc<Link>> {
        if let Some(l) = self.link.read().await.clone() {
            return Ok(l);
        }
        let why = self
            .link_error
            .read()
            .await
            .clone()
            .unwrap_or_else(|| "the network link has not started yet".to_owned());
        Err(UiError::offline(why))
    }
}
