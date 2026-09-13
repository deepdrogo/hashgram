//! Sync commands: phase, run a round now.

use std::sync::Arc;

use tauri::State;

use crate::error::{CmdResult, UiError};
use crate::state::{AppState, SyncStatus};

type S<'a> = State<'a, Arc<AppState>>;

/// The sync loop's status.
#[tauri::command]
pub async fn sync_status(state: S<'_>) -> CmdResult<SyncStatus> {
    let mut s = state.sync.read().await.clone();
    s.peers = match state.link.read().await.as_ref() {
        Some(l) => l.peers().await.len(),
        None => 0,
    };
    Ok(s)
}

/// Runs a round now (wakes the loop). Returns at once; the round's outcome
/// arrives as `sync:event`.
#[tauri::command]
pub async fn sync_now(state: S<'_>) -> CmdResult<()> {
    if !state.is_unlocked().await {
        return Err(UiError::locked());
    }
    state.sync_wake.notify_one();
    Ok(())
}
