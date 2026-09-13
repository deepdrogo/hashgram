//! Settings, backup, help, performance, misc.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use zeroize::Zeroizing;

use crate::error::{CmdResult, UiError};
use crate::paths;
use crate::session;
use crate::settings::Settings;
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

/// Reads settings.
#[tauri::command]
pub async fn settings_get(state: S<'_>) -> CmdResult<Settings> {
    Ok(state.settings.read().await.clone())
}

/// Writes settings; restarts the network when the profile changed.
#[tauri::command]
pub async fn settings_set(state: S<'_>, app: AppHandle, settings: Settings) -> CmdResult<()> {
    settings.validate().map_err(UiError::invalid)?;
    let network_changed = {
        let cur = state.settings.read().await;
        cur.network.kind != settings.network.kind
            || cur.network.devnet_genesis_hash != settings.network.devnet_genesis_hash
            || cur.network.bootstrap != settings.network.bootstrap
            || cur.network.chain_api != settings.network.chain_api
    };
    settings.save(&paths::settings_path())?;
    *state.settings.write().await = settings.clone();
    if network_changed {
        if state.is_unlocked().await {
            // The facade is bound to a network; a profile change means a
            // fresh unlock.
            session::lock(&app, &state).await;
        }
        session::restart_link(&app, &state, false).await;
    }
    let _ = app.emit("settings:changed", ());
    Ok(())
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

/// Exports an encrypted backup of the vault to `path`.
#[tauri::command]
pub async fn backup_export(state: S<'_>, path: String, passphrase: String) -> CmdResult<hashgram_sdk::backup::BackupMeta> {
    if passphrase.chars().count() < hashgram_sdk::backup::MIN_PASSPHRASE {
        return Err(UiError::invalid(format!(
            "a backup passphrase is at least {} characters",
            hashgram_sdk::backup::MIN_PASSPHRASE
        )));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.save()?;
    let p = std::path::PathBuf::from(path);
    // Argon2id at 256 MiB: off the async runtime.
    let passphrase = Zeroizing::new(passphrase);
    let contents = one.account.contents.clone();
    drop(g);
    let meta = tauri::async_runtime::spawn_blocking(move || {
        hashgram_sdk::backup::export_backup_contents(&contents, &p, &passphrase, None)
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    Ok(meta)
}

// ---------------------------------------------------------------------------
// Help, perf, misc
// ---------------------------------------------------------------------------

/// Help pages.
#[tauri::command]
pub fn help_list() -> Vec<crate::help::HelpPage> {
    crate::help::PAGES.to_vec()
}

/// One help page as HTML.
#[tauri::command]
pub fn help_page(slug: String) -> CmdResult<String> {
    crate::help::render(&slug, crate::cmd_identity::COMMIT).ok_or_else(|| UiError::not_found("no such help page"))
}

/// Performance spans.
#[tauri::command]
pub fn perf_snapshot(state: S<'_>) -> Vec<crate::perf::SpanRecord> {
    state.perf.snapshot()
}

/// Records a frontend timing.
#[tauri::command]
pub fn perf_mark(state: S<'_>, name: String, micros: u64) {
    if name.len() <= 64 {
        state.perf.push(name, micros, "ui");
    }
}

/// Working set in bytes.
#[tauri::command]
pub fn perf_memory() -> u64 {
    crate::winsec::working_set_bytes()
}

/// Opens the data folder in Explorer.
#[tauri::command]
pub fn open_data_dir(app: AppHandle) -> CmdResult<()> {
    crate::util::open_path(&app, &paths::data_dir())
}

/// Frontend log line (never content; the UI passes short event names).
#[tauri::command]
pub fn ui_log(level: String, message: String) {
    let m: String = message.chars().take(200).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "ui", "{m}"),
        "warn" => tracing::warn!(target: "ui", "{m}"),
        _ => tracing::info!(target: "ui", "{m}"),
    }
}

/// Writes a text file the user chose (diagnostics export).
#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> CmdResult<()> {
    if contents.len() > 8 * 1024 * 1024 {
        return Err(UiError::invalid("too large"));
    }
    std::fs::write(&path, contents)?;
    Ok(())
}

/// Recent global searches (typed queries only, kept locally).
#[tauri::command]
pub async fn search_recent(state: S<'_>) -> CmdResult<Vec<String>> {
    state.db.search_recent().map_err(UiError::internal)
}

/// Notes a global search.
#[tauri::command]
pub async fn search_note(state: S<'_>, query: String) -> CmdResult<()> {
    let q = query.trim();
    if q.is_empty() || q.len() > 128 {
        return Ok(());
    }
    state.db.search_note(q).map_err(UiError::internal)
}

/// About facts.
#[derive(Debug, Clone, Serialize)]
pub struct AboutInfo {
    /// Version.
    pub version: String,
    /// Commit.
    pub commit: String,
    /// Genesis hash.
    pub genesis_hash: String,
    /// Chain id.
    pub chain_id: String,
    /// KDF label.
    pub kdf: String,
    /// Code signed.
    pub code_signed: bool,
    /// Updater endpoint.
    pub updater_endpoint: String,
    /// Data dir.
    pub data_dir: String,
    /// Log dir.
    pub logs_dir: String,
    /// Licenses (crate → license), abbreviated.
    pub licenses: Vec<(String, String)>,
}

/// About.
#[tauri::command]
pub async fn about_info(state: S<'_>, app: AppHandle) -> CmdResult<AboutInfo> {
    let settings = state.settings.read().await.clone();
    let identity = session::network_identity(&settings)?;
    let updater_endpoint = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_owned();
    Ok(AboutInfo {
        version: app.package_info().version.to_string(),
        commit: crate::cmd_identity::COMMIT.to_owned(),
        genesis_hash: identity.genesis_hash,
        chain_id: identity.chain_id,
        kdf: crate::cmd_identity::kdf_label(),
        code_signed: option_env!("HASHGRAM_CODESIGNED").map(|v| v == "1").unwrap_or(false),
        updater_endpoint,
        data_dir: paths::data_dir().display().to_string(),
        logs_dir: paths::logs_dir().display().to_string(),
        licenses: vec![
            ("Hashgram".into(), "Apache-2.0".into()),
            ("Tauri".into(), "MIT / Apache-2.0".into()),
            ("SolidJS".into(), "MIT".into()),
            ("Inter (font)".into(), "SIL OFL 1.1".into()),
            ("libp2p".into(), "MIT".into()),
            ("OpenMLS".into(), "MIT".into()),
            ("RustCrypto".into(), "MIT / Apache-2.0".into()),
            ("redb".into(), "MIT / Apache-2.0".into()),
            ("rusqlite / SQLite".into(), "MIT / Public domain".into()),
            ("lucide".into(), "ISC".into()),
        ],
    })
}

/// Storage leases recorded locally (Advanced page). `offer()` is
/// protocol v1.1 work; this lists and verifies what exists.
#[tauri::command]
pub async fn leases_list(state: S<'_>) -> CmdResult<Vec<hashgram_sdk::storage_lease::LeaseRecord>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.leases().list()?)
}

/// Verifies an epoch of a lease.
#[tauri::command]
pub async fn leases_verify(state: S<'_>, lease_id: String, epoch: u64) -> CmdResult<hashgram_sdk::storage_lease::Verification> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.leases().verify_epoch(&lease_id, epoch).await?)
}

/// Hides the main window to the tray.
#[tauri::command]
pub fn window_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}
