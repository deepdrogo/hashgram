//! Opening and closing a session: the `HashgramOne` facade, the P2P link,
//! the sync loop and the auto-lock timer.
//!
//! The network link is started once at boot (retrying with backoff: a port
//! in use, an adapter not up yet at logon) and kept for the life of the
//! process. Unlock builds the facade around it; lock drops the facade, the
//! link stays. The sync loop is one task per session: `round()` → `save()`
//! → emit events → sleep `next_delay()`, woken early by `sync_now`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hashgram_sdk::account::Account;
use hashgram_sdk::sync::{SyncEvent, SyncPhase};
use hashgram_sdk::{Config, HashgramOne, KdfCost, NetworkIdentity};
use tauri::{AppHandle, Emitter};
use zeroize::Zeroizing;

use crate::error::{CmdResult, UiError};
use crate::paths;
use crate::settings::{NetworkKind, Settings};
use crate::state::{AppState, SessionInfo};

/// Vault `extra` key holding the UI-cache column key.
pub const DB_KEY_NAME: &str = "desktop_db_key";

/// The KDF cost for this profile. `KdfCost::light()` is DEVNET/test only
/// and needs both an explicit home and an explicit opt-in.
#[must_use]
pub fn kdf() -> KdfCost {
    if std::env::var("HASHGRAM_LIGHT_KDF")
        .map(|v| v == "true")
        .unwrap_or(false)
        && std::env::var("HASHGRAM_DESKTOP_HOME").is_ok()
    {
        return KdfCost::light();
    }
    KdfCost::default()
}

/// The network the settings pin.
pub fn network_identity(settings: &Settings) -> Result<NetworkIdentity, UiError> {
    match settings.network.kind {
        NetworkKind::Mainnet => Ok(NetworkIdentity::mainnet(
            hashgram_sdk::net::MAINNET_GENESIS_HASH,
        )),
        NetworkKind::Devnet => {
            let g = settings.network.devnet_genesis_hash.trim();
            if g.len() != 64 || !g.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(UiError::invalid(
                    "a devnet needs its 64-hex genesis hash in Settings → Network",
                ));
            }
            Ok(NetworkIdentity::devnet(g))
        }
    }
}

/// The SDK config for these settings. On Mainnet the bootstrap list is the
/// compiled-in one unless the user overrode it, and chain reads go through
/// the P2P relay unless a REST gateway was configured (devnet).
pub fn config(settings: &Settings, connect_wait: Duration) -> Result<Config, UiError> {
    let network = network_identity(settings)?;
    let mut bootstrap = Vec::new();
    for b in &settings.network.bootstrap {
        let b = b.trim();
        if b.is_empty() {
            continue;
        }
        bootstrap.push(
            b.parse()
                .map_err(|_| UiError::invalid(format!("bootstrap {b:?} is not a multiaddr")))?,
        );
    }
    let chain_api = settings.network.chain_api.trim();
    Ok(Config {
        paths: paths::sdk_paths(),
        network,
        bootstrap,
        chain_api: if chain_api.is_empty() {
            None
        } else {
            Some(chain_api.to_owned())
        },
        kdf: kdf(),
        connect_wait,
    })
}

/// Starts the P2P link in the background, retrying with a growing pause
/// until it is up. Idempotent: returns at once when a link exists.
pub fn spawn_link(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut pause = Duration::from_secs(2);
        loop {
            if state.link.read().await.is_some() {
                return;
            }
            let settings = state.settings.read().await.clone();
            let result = match config(&settings, Duration::from_secs(8)) {
                Ok(cfg) => HashgramOne::connect_link(&cfg).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.message),
            };
            match result {
                Ok(link) => {
                    {
                        let mut slot = state.link.write().await;
                        if slot.is_some() {
                            // An unlock started its own link meanwhile.
                            drop(slot);
                            link.shutdown().await;
                            return;
                        }
                        *slot = Some(link);
                    }
                    *state.link_error.write().await = None;
                    tracing::info!("network link started");
                    let _ = app.emit("net:changed", ());
                    // A session opened before the link was up (offline
                    // unlock) gets a facade over the new link.
                    reattach_link(&state).await;
                    state.sync_wake.notify_one();
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = %e, retry_in_secs = pause.as_secs(), "network link did not start");
                    *state.link_error.write().await = Some(e);
                    let _ = app.emit("net:changed", ());
                }
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(30));
        }
    });
}

/// Restarts the link (reconnect / forget peers / network profile change).
pub async fn restart_link(app: &AppHandle, state: &Arc<AppState>, forget_peers: bool) {
    let old = state.link.write().await.take();
    if let Some(l) = old {
        // A transport whose sockets died during an adapter/network change
        // must not be allowed to stall recovery forever.
        let _ = tokio::time::timeout(Duration::from_secs(5), l.shutdown()).await;
    }
    if forget_peers {
        let _ = std::fs::remove_file(paths::peerstore_path());
    }
    *state.link_error.write().await = Some("reconnecting".into());
    spawn_link(app.clone(), state.clone());
}

/// When the link came up (or was restarted) after a session was opened,
/// point the facade at it. Local state is untouched.
pub async fn reattach_link(state: &Arc<AppState>) {
    let Some(link) = state.link.read().await.clone() else {
        return;
    };
    let mut g = state.one.lock().await;
    let Some(one) = g.as_mut() else {
        return;
    };
    if Arc::ptr_eq(&one.link, &link) {
        return;
    }
    let settings = state.settings.read().await.clone();
    let chain_api = settings.network.chain_api.trim().to_owned();
    match one.replace_link(link, if chain_api.is_empty() { None } else { Some(&chain_api) }) {
        Ok(()) => tracing::info!("session re-attached to the network link"),
        Err(e) => tracing::warn!(error = %e, "could not re-attach the session to the link"),
    }
}

fn link_needs_restart(unhealthy_checks: &mut u8, peers: usize, has_store: bool) -> bool {
    link_needs_restart_with(unhealthy_checks, peers, has_store, 0)
}

/// `transport_rejections`: handshakes that died at the transport in the
/// last check period. With no peers left and a node hanging up on every
/// fresh connection, waiting a second check only lengthens the outage: a
/// new link (new ephemeral identity) connects at once.
fn link_needs_restart_with(
    unhealthy_checks: &mut u8,
    peers: usize,
    has_store: bool,
    transport_rejections: usize,
) -> bool {
    if peers == 0 || !has_store {
        *unhealthy_checks = unhealthy_checks.saturating_add(1);
    } else {
        *unhealthy_checks = 0;
    }
    let fast = peers == 0 && transport_rejections >= 2;
    if *unhealthy_checks >= 2 || (fast && *unhealthy_checks >= 1) {
        *unhealthy_checks = 0;
        true
    } else {
        false
    }
}

fn db_key_of(account: &mut Account) -> CmdResult<crate::crypto::DbKey> {
    if let Some(h) = account.contents.extra.get(DB_KEY_NAME) {
        return crate::crypto::key_from_hex(h).map_err(UiError::internal);
    }
    let k = crate::crypto::generate_key().map_err(UiError::internal)?;
    account
        .contents
        .extra
        .insert(DB_KEY_NAME.to_owned(), crate::crypto::key_to_hex(&k));
    account.save()?;
    Ok(k)
}

/// Facts the UI gets back from unlock/onboarding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountInfo {
    /// `hash1…`.
    pub address: String,
    /// This device's id.
    pub device_id: String,
    /// This device holds the wallet key.
    pub has_wallet_key: bool,
    /// This device holds the identity root key.
    pub has_root_key: bool,
}

/// Builds the facade around an opened account, records the session and
/// starts the sync loop. Works offline: without a link the facade is
/// built over a link that starts now with a one-second wait (the SDK
/// renders from the local store meanwhile).
pub async fn open_session(
    app: &AppHandle,
    state: &Arc<AppState>,
    mut account: Account,
) -> CmdResult<AccountInfo> {
    let db_key = db_key_of(&mut account)?;
    let settings = state.settings.read().await.clone();
    let cfg = config(&settings, Duration::from_secs(1))?;
    let link = match state.link.read().await.clone() {
        Some(l) => l,
        None => {
            // Boot's link task is still retrying; start one now with a short
            // wait so unlock never blocks on the network. If this fails too
            // the boot task keeps retrying and re-attaches later.
            match HashgramOne::connect_link(&cfg).await {
                Ok(l) => {
                    *state.link.write().await = Some(l.clone());
                    *state.link_error.write().await = None;
                    l
                }
                Err(e) => {
                    return Err(UiError::offline(format!(
                        "the network link could not start: {e}. The app keeps trying; unlock again in a moment."
                    )))
                }
            }
        }
    };
    let info = AccountInfo {
        address: account.address().to_owned(),
        device_id: account.contents.device_id.clone(),
        has_wallet_key: account.contents.wallet_secret.is_some(),
        has_root_key: account.contents.root_seed.is_some(),
    };
    let one = HashgramOne::with_link(cfg, account, link).await?;
    let _ = std::fs::remove_dir_all(paths::tmp_dir());
    let _ = std::fs::create_dir_all(paths::tmp_dir());
    {
        let mut g = state.one.lock().await;
        *g = Some(one);
    }
    *state.session.write().await = Some(SessionInfo {
        address: info.address.clone(),
        device_id: info.device_id.clone(),
        has_wallet_key: info.has_wallet_key,
        has_root_key: info.has_root_key,
        db_key,
        unlocked_at: Instant::now(),
        last_activity: Instant::now(),
    });
    *state.sync.write().await = crate::state::SyncStatus::default();
    spawn_sync_loop(app.clone(), state.clone()).await;
    let _ = app.emit("session:unlocked", ());
    Ok(info)
}

/// Locks: stops the sync loop, drops the facade (zeroising keys), clears
/// the session and the scratch files.
pub async fn lock(app: &AppHandle, state: &Arc<AppState>) {
    if let Some(t) = state.sync_task.lock().await.take() {
        t.abort();
    }
    {
        let mut g = state.one.lock().await;
        if let Some(one) = g.as_mut() {
            if let Err(e) = one.save() {
                tracing::warn!(error = %e, "save at lock failed");
            }
        }
        *g = None;
    }
    *state.session.write().await = None;
    state.pending_mnemonic.lock().await.take();
    *state.sync.write().await = crate::state::SyncStatus::default();
    let _ = std::fs::remove_dir_all(paths::tmp_dir());
    let _ = std::fs::create_dir_all(paths::tmp_dir());
    let _ = app.emit("session:locked", ());
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One event as the webview sees it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiSyncEvent {
    /// Phase change.
    Phase {
        /// Phase.
        phase: SyncPhase,
    },
    /// New mail filed.
    NewMail {
        /// Id.
        id: String,
        /// Folder.
        folder: String,
    },
    /// Shared-with-me changed.
    DriveShareChanged,
    /// Contacts changed.
    ContactsChanged,
    /// Circle activity.
    CircleActivity {
        /// Circle hex.
        id: String,
    },
    /// Space activity.
    SpaceActivity {
        /// Space hex.
        id: String,
    },
    /// Balance.
    Balance {
        /// uhash.
        uhash: String,
    },
    /// A message this build cannot read.
    UnsupportedMessage,
    /// Non-fatal.
    Warning {
        /// Text.
        message: String,
    },
    /// A round finished (the UI refreshes lists).
    RoundDone {
        /// Mail filed.
        mail: usize,
        /// Drive changes.
        drive: usize,
        /// People changes.
        people: usize,
        /// Circle events.
        circles: usize,
        /// Space events.
        spaces: usize,
        /// Feed events.
        feed: usize,
        /// Drive commit revision, if any.
        drive_committed: Option<u64>,
        /// ms.
        elapsed_ms: u64,
    },
}

impl From<SyncEvent> for UiSyncEvent {
    fn from(e: SyncEvent) -> Self {
        match e {
            SyncEvent::Phase(p) => Self::Phase { phase: p },
            SyncEvent::NewMail { id, folder } => Self::NewMail { id, folder },
            SyncEvent::DriveShareChanged => Self::DriveShareChanged,
            SyncEvent::ContactsChanged => Self::ContactsChanged,
            SyncEvent::CircleActivity(id) => Self::CircleActivity { id },
            SyncEvent::SpaceActivity(id) => Self::SpaceActivity { id },
            SyncEvent::Balance(u) => Self::Balance { uhash: u },
            SyncEvent::UnsupportedMessage => Self::UnsupportedMessage,
            SyncEvent::Warning(m) => Self::Warning { message: m },
        }
    }
}

async fn spawn_sync_loop(app: AppHandle, state: Arc<AppState>) {
    let rx = {
        let mut g = state.one.lock().await;
        match g.as_mut() {
            Some(one) => one.sync().subscribe(),
            None => return,
        }
    };
    // Forward engine events to the webview and to OS notifications.
    {
        let app = app.clone();
        let state = state.clone();
        tauri::async_runtime::spawn(async move {
            let mut rx = rx;
            while let Some(ev) = rx.recv().await {
                if let SyncEvent::Phase(p) = &ev {
                    state.sync.write().await.phase = p.clone();
                    let _ = app.emit("sync:phase", p.clone());
                }
                if let SyncEvent::Balance(b) = &ev {
                    state.sync.write().await.balance_uhash = Some(b.clone());
                }
                crate::notify::on_sync_event(&app, &state, &ev).await;
                let _ = app.emit("sync:event", UiSyncEvent::from(ev));
            }
        });
    }
    let loop_state = state.clone();
    let task = tauri::async_runtime::spawn(async move {
        let state = loop_state;
        loop {
            let (delay, done) = {
                let mut g = state.one.lock().await;
                let Some(one) = g.as_mut() else { break };
                let r = one.sync().round().await;
                if let Err(e) = one.save() {
                    tracing::warn!(error = %e, "save after sync round failed");
                }
                let delay = one.sync().next_delay();
                let phase = one.sync().phase();
                let peers = one.link.peers().await.len();
                let mut st = state.sync.write().await;
                st.phase = phase;
                st.peers = peers;
                match &r {
                    Ok(rep) => {
                        st.rounds += 1;
                        st.last_ok_ms = Some(now_ms());
                        st.last_error = None;
                        if let Some(b) = rep.balance_uhash {
                            st.balance_uhash = Some(b.to_string());
                        }
                    }
                    Err(e) => {
                        st.last_error = Some(e.to_string());
                    }
                }
                let done = r.ok().map(|rep| UiSyncEvent::RoundDone {
                    mail: rep.mail,
                    drive: rep.drive,
                    people: rep.people,
                    circles: rep.circles,
                    spaces: rep.spaces,
                    feed: rep.feed,
                    drive_committed: rep.drive_committed,
                    elapsed_ms: rep.elapsed_ms,
                });
                (delay, done)
            };
            if let Some(d) = done {
                let _ = app.emit("sync:event", d);
            }
            let _ = app.emit("sync:tick", ());
            crate::cmd_wallet::poll_pending(&state, &app).await;
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = state.sync_wake.notified() => {}
            }
        }
    });
    *state.sync_task.lock().await = Some(task);
}

/// Periodic housekeeping while the app runs: auto-lock, net snapshots.
pub fn spawn_housekeeping(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        let mut n: u64 = 0;
        let mut unhealthy_link_checks: u8 = 0;
        loop {
            tick.tick().await;
            n += 1;
            if n.is_multiple_of(3) && state.auto_lock_due().await {
                lock(&app, &state).await;
            }
            if n.is_multiple_of(3) {
                let (peers, has_store, transport_rejections) =
                    match state.link.read().await.as_ref() {
                        Some(l) => {
                            let peers = l.peers().await.len();
                            let has_store = !l.peers_with_role("store").await.is_empty();
                            let tr = l
                                .transport_rejections_within(Duration::from_secs(15))
                                .await;
                            (peers, has_store, tr)
                        }
                        None => (0, false, 0),
                    };
                state.sync.write().await.peers = peers;
                let _ = app.emit("net:changed", ());

                // A Link object can survive while all of its sockets are
                // dead. Previously `spawn_link` saw `Some(link)` and returned,
                // so only restarting the process recovered after an internet
                // outage. Two unhealthy 15-second checks trigger a clean
                // redial while keeping local data and the unlocked session;
                // one check is enough when nodes are hanging up on fresh
                // connections (a new identity gets through at once).
                if link_needs_restart_with(
                    &mut unhealthy_link_checks,
                    peers,
                    has_store,
                    transport_rejections,
                ) {
                    tracing::warn!(
                        peers,
                        has_store,
                        transport_rejections,
                        "network link unhealthy; reconnecting automatically"
                    );
                    *state.link_error.write().await =
                        Some("connection lost; reconnecting automatically".into());
                    let _ = app.emit("net:changed", ());
                    restart_link(&app, &state, false).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::link_needs_restart;

    #[test]
    fn a_dead_link_restarts_after_two_checks_and_health_resets_the_streak() {
        let mut checks = 0;
        assert!(!link_needs_restart(&mut checks, 0, false));
        assert!(!link_needs_restart(&mut checks, 1, true));
        assert_eq!(checks, 0);
        assert!(!link_needs_restart(&mut checks, 1, false));
        assert!(link_needs_restart(&mut checks, 0, false));
        assert_eq!(checks, 0);
    }

    #[test]
    fn nodes_hanging_up_on_fresh_connections_restart_after_one_check() {
        use super::link_needs_restart_with;
        let mut checks = 0;
        // One transport rejection is not a pattern.
        assert!(!link_needs_restart_with(&mut checks, 0, false, 1));
        assert_eq!(checks, 1);
        checks = 0;
        // Two within the period, with no peers left: reconnect now.
        assert!(link_needs_restart_with(&mut checks, 0, false, 2));
        assert_eq!(checks, 0);
        // With a peer still connected the fast path does not apply.
        assert!(!link_needs_restart_with(&mut checks, 1, false, 5));
    }
}

/// Opens a vault by passphrase on a blocking thread (Argon2id).
pub async fn open_account(passphrase: Zeroizing<String>) -> CmdResult<Account> {
    let vault = paths::vault_path();
    if !vault.exists() {
        return Err(UiError::not_found("no vault on this PC"));
    }
    tauri::async_runtime::spawn_blocking(move || Account::open(&vault, &passphrase, kdf()))
        .await
        .map_err(|e| UiError::internal(e.to_string()))?
        .map_err(UiError::from)
}
