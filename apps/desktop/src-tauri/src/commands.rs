//! Tauri commands: the whole surface the frontend can call.
//!
//! Every command returns `Result<T, String>`; the string is shown to the
//! user, so it says what happened in plain words. Nothing here trusts the
//! frontend: addresses are re-validated, amounts re-parsed, the vault is
//! required for anything that signs.

use std::sync::Arc;
use std::time::Instant;

use hashgram_sdk::account::{self, Account, MNEMONIC_WORDS};
use hashgram_sdk::{ChainClient, KdfCost, Verification, Wallet};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use zeroize::Zeroizing;

use crate::chain_access::{ChainRead, EndpointHealth, Source};
use crate::db::PendingRow;
use crate::net::NetSnapshot;
use crate::paths;
use crate::settings::{NetworkKind, Settings};
use crate::state::{AppState, Session};
use crate::tx::{self, MsgSpec};

type S<'a> = State<'a, Arc<AppState>>;

/// Vault key material derived key name in the vault's `extra` map.
const DB_KEY_NAME: &str = "desktop_db_key";

fn kdf() -> KdfCost {
    if std::env::var("HASHGRAM_LIGHT_KDF")
        .map(|v| v == "true")
        .unwrap_or(false)
        && std::env::var("HASHGRAM_DESKTOP_HOME").is_ok()
    {
        // Tests only: never on a real profile.
        return KdfCost::light();
    }
    KdfCost::default()
}

/// The commit this build was made from.
pub const COMMIT: &str = match option_env!("HASHGRAM_COMMIT") {
    Some(c) => c,
    None => "unknown",
};

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// What the shell needs on every start.
#[derive(Debug, Clone, Serialize)]
pub struct AppStatus {
    /// A vault exists on this PC.
    pub vault_exists: bool,
    /// The vault is unlocked.
    pub unlocked: bool,
    /// Address, when unlocked.
    pub address: Option<String>,
    /// `mainnet` or `devnet`.
    pub network: String,
    /// Chain id.
    pub chain_id: String,
    /// Onboarding finished.
    pub onboarding_done: bool,
    /// Windows Hello is available on this PC.
    pub hello_available: bool,
    /// Windows Hello unlock is enrolled.
    pub hello_enabled: bool,
    /// App version.
    pub version: String,
    /// Commit.
    pub commit: String,
    /// Data directory.
    pub data_dir: String,
    /// Whether the updater has a real public key compiled in.
    pub updater_configured: bool,
    /// minisign key id of the compiled-in updater public key (hex), when configured.
    pub updater_key_id: String,
    /// Where the updater fetches its signed manifest from.
    pub updater_endpoint: String,
    /// Milliseconds since process start (for the cold-start budget).
    pub uptime_ms: u64,
}

/// Extracts the minisign key id from a base64-encoded public key file
/// ("untrusted comment: minisign public key: <ID>\n<key>").
pub(crate) fn minisign_key_id(pubkey_b64: &str) -> String {
    let Some(raw) = crate::chain_proxy::base64_decode(pubkey_b64.trim()) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&raw);
    text.lines()
        .find_map(|l| l.split("public key:").nth(1))
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

/// Reports app status.
#[tauri::command]
pub async fn app_status(state: S<'_>, app: AppHandle) -> Result<AppStatus, String> {
    let settings = state.settings.read().await.clone();
    let session = state.session.read().await;
    let identity = crate::net::identity_for(&settings).ok();
    let hello_available = tauri::async_runtime::spawn_blocking(crate::winsec::hello_supported)
        .await
        .unwrap_or(false);
    let updater_cfg = app.config().plugins.0.get("updater").cloned();
    let updater_pubkey = updater_cfg
        .as_ref()
        .and_then(|u| u.get("pubkey"))
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_owned();
    let updater_configured =
        !updater_pubkey.is_empty() && !updater_pubkey.starts_with("REPLACE_WITH");
    let updater_key_id = if updater_configured {
        minisign_key_id(&updater_pubkey)
    } else {
        String::new()
    };
    let updater_endpoint = updater_cfg
        .as_ref()
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_owned();
    Ok(AppStatus {
        vault_exists: paths::vault_path().exists(),
        unlocked: session.is_some(),
        address: session.as_ref().map(|s| s.address.clone()),
        network: match settings.network.kind {
            NetworkKind::Mainnet => "mainnet".into(),
            NetworkKind::Devnet => "devnet".into(),
        },
        chain_id: identity.map(|i| i.chain_id).unwrap_or_default(),
        onboarding_done: settings.onboarding_done,
        hello_available,
        hello_enabled: settings.security.hello_enabled && paths::hello_blob_path().exists(),
        version: app.package_info().version.to_string(),
        commit: COMMIT.to_owned(),
        data_dir: paths::data_dir().display().to_string(),
        updater_configured,
        updater_key_id,
        updater_endpoint,
        uptime_ms: state.perf.uptime_ms(),
    })
}

// ---------------------------------------------------------------------------
// Onboarding and session
// ---------------------------------------------------------------------------

/// Account facts after unlock.
#[derive(Debug, Clone, Serialize)]
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

fn info_of(a: &Account) -> AccountInfo {
    AccountInfo {
        address: a.address().to_owned(),
        device_id: a.contents.device_id.clone(),
        has_wallet_key: a.contents.wallet_secret.is_some(),
        has_root_key: a.contents.root_seed.is_some(),
    }
}

fn device_id() -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows-pc".into());
    let mut rnd = [0u8; 4];
    let _ = getrandom::fill(&mut rnd);
    format!("{}-{}", host.to_ascii_lowercase(), hex::encode(rnd))
}

fn db_key_of(account: &mut Account) -> Result<crate::crypto::DbKey, String> {
    if let Some(h) = account.contents.extra.get(DB_KEY_NAME) {
        return crate::crypto::key_from_hex(h);
    }
    // First unlock of a vault created elsewhere: mint the key now and keep
    // it in the vault.
    let k = crate::crypto::generate_key()?;
    account
        .contents
        .extra
        .insert(DB_KEY_NAME.to_owned(), crate::crypto::key_to_hex(&k));
    account.save().map_err(|e| e.to_string())?;
    Ok(k)
}

async fn open_session(state: &AppState, mut account: Account) -> Result<AccountInfo, String> {
    let db_key = db_key_of(&mut account)?;
    let info = info_of(&account);
    // Messaging and social state live in the vault; open them now so the
    // background sync can start, and so a failure is visible at unlock.
    if let Err(e) = state.chat.open(&account).await {
        tracing::warn!(error = %e, "messaging state could not be opened");
    }
    if let Err(e) = state.social.open(&account).await {
        tracing::warn!(error = %e, "social state could not be opened");
    }
    let address = account.address().to_owned();
    *state.session.write().await = Some(Session {
        account: Arc::new(tokio::sync::Mutex::new(account)),
        address,
        db_key,
        unlocked_at: Instant::now(),
        last_activity: Instant::now(),
    });
    Ok(info)
}

/// Generates the 24 words for a new account and holds them in memory until
/// [`onboarding_create`] commits them to a vault. Never written anywhere.
#[tauri::command]
pub async fn onboarding_generate(state: S<'_>) -> Result<Vec<String>, String> {
    if paths::vault_path().exists() {
        return Err(
            "a vault already exists on this PC; unlock it or restore over it from Settings".into(),
        );
    }
    let (mnemonic, _wallet) = Wallet::generate().map_err(|e| e.to_string())?;
    let words: Vec<String> = mnemonic.split_whitespace().map(str::to_owned).collect();
    if words.len() != MNEMONIC_WORDS {
        return Err("generator did not produce 24 words".into());
    }
    *state.pending_mnemonic.lock().await = Some(Zeroizing::new(mnemonic));
    Ok(words)
}

/// Checks three words the user re-typed against the pending mnemonic.
/// `positions` are 0-based indices.
#[tauri::command]
pub async fn onboarding_check_words(
    state: S<'_>,
    positions: Vec<usize>,
    words: Vec<String>,
) -> Result<bool, String> {
    let pending = state.pending_mnemonic.lock().await;
    let Some(m) = pending.as_ref() else {
        return Err("no words are pending; start again".into());
    };
    let all: Vec<&str> = m.split_whitespace().collect();
    if positions.len() != words.len() {
        return Ok(false);
    }
    Ok(positions.iter().zip(words.iter()).all(|(i, w)| {
        all.get(*i)
            .map(|x| x.eq_ignore_ascii_case(w.trim()))
            .unwrap_or(false)
    }))
}

fn passphrase_ok(p: &str) -> Result<(), String> {
    if p.chars().count() < 8 {
        return Err("the passphrase needs at least 8 characters".into());
    }
    Ok(())
}

/// Creates the vault from the pending mnemonic with a passphrase.
#[tauri::command]
pub async fn onboarding_create(
    state: S<'_>,
    passphrase: String,
    device_label: String,
) -> Result<AccountInfo, String> {
    passphrase_ok(&passphrase)?;
    let mnemonic = state
        .pending_mnemonic
        .lock()
        .await
        .take()
        .ok_or_else(|| "no words are pending; start again".to_owned())?;
    let vault = paths::vault_path();
    if vault.exists() {
        return Err("a vault already exists on this PC".into());
    }
    paths::ensure_dirs().map_err(|e| e.to_string())?;
    let did = device_id();
    let mut account = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        let m = mnemonic.clone();
        move || Account::import(&vault, &passphrase, &m, &did, kdf())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    drop(mnemonic);
    let k = crate::crypto::generate_key()?;
    account
        .contents
        .extra
        .insert(DB_KEY_NAME.to_owned(), crate::crypto::key_to_hex(&k));
    account.save().map_err(|e| e.to_string())?;
    {
        let mut s = state.settings.write().await;
        s.onboarding_done = true;
        s.device_label = if device_label.trim().is_empty() {
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
        } else {
            device_label.trim().to_owned()
        };
        s.save(&paths::settings_path()).map_err(|e| e.to_string())?;
    }
    open_session(&state, account).await
}

/// Address preview.
#[derive(Debug, Clone, Serialize)]
pub struct RestorePreview {
    /// Address the words derive.
    pub address: String,
}

fn normalise_mnemonic(m: &str) -> Result<String, String> {
    let words: Vec<String> = m
        .split_whitespace()
        .map(|w| w.trim().to_ascii_lowercase())
        .collect();
    if words.len() != MNEMONIC_WORDS {
        return Err(format!(
            "a Hashgram account is {MNEMONIC_WORDS} words; you entered {}",
            words.len()
        ));
    }
    Ok(words.join(" "))
}

/// Derives the address from 24 words so the user can confirm before a
/// vault is written. Nothing is stored.
#[tauri::command]
pub async fn onboarding_restore_preview(mnemonic: String) -> Result<RestorePreview, String> {
    let m = Zeroizing::new(normalise_mnemonic(&mnemonic)?);
    let w = Wallet::from_mnemonic(&m, "", 0).map_err(|e| match e.to_string() {
        s if s.contains("checksum") || s.contains("word") => {
            "these words are not a valid phrase (check spelling and order)".to_owned()
        }
        s => s,
    })?;
    Ok(RestorePreview {
        address: w.address().to_string(),
    })
}

/// Restores an account from 24 words into a new vault.
#[tauri::command]
pub async fn onboarding_restore(
    state: S<'_>,
    mnemonic: String,
    passphrase: String,
    device_label: String,
) -> Result<AccountInfo, String> {
    passphrase_ok(&passphrase)?;
    let m = Zeroizing::new(normalise_mnemonic(&mnemonic)?);
    let vault = paths::vault_path();
    if vault.exists() {
        return Err(
            "a vault already exists on this PC; remove it from Settings → Security first".into(),
        );
    }
    paths::ensure_dirs().map_err(|e| e.to_string())?;
    let did = device_id();
    let mut account = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || Account::import(&vault, &passphrase, &m, &did, kdf())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let k = crate::crypto::generate_key()?;
    account
        .contents
        .extra
        .insert(DB_KEY_NAME.to_owned(), crate::crypto::key_to_hex(&k));
    account.save().map_err(|e| e.to_string())?;
    {
        let mut s = state.settings.write().await;
        s.onboarding_done = true;
        s.device_label = if device_label.trim().is_empty() {
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
        } else {
            device_label.trim().to_owned()
        };
        s.save(&paths::settings_path()).map_err(|e| e.to_string())?;
    }
    open_session(&state, account).await
}

/// Unlocks with the passphrase.
#[tauri::command]
pub async fn unlock(state: S<'_>, passphrase: String) -> Result<AccountInfo, String> {
    let vault = paths::vault_path();
    if !vault.exists() {
        return Err("no vault on this PC".into());
    }
    let account =
        tauri::async_runtime::spawn_blocking(move || Account::open(&vault, &passphrase, kdf()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| {
                let s = e.to_string();
                if s.contains("decrypt") || s.contains("passphrase") || s.contains("aead") {
                    "wrong passphrase".to_owned()
                } else {
                    s
                }
            })?;
    open_session(&state, account).await
}

/// Locks the vault.
#[tauri::command]
pub async fn lock(state: S<'_>, app: AppHandle) -> Result<(), String> {
    state.lock().await;
    let _ = app.emit("session:locked", ());
    Ok(())
}

/// Touches the activity timer (auto-lock).
#[tauri::command]
pub async fn touch(state: S<'_>) -> Result<(), String> {
    state.touch().await;
    Ok(())
}

/// Changes the passphrase (re-encrypts the vault).
#[tauri::command]
pub async fn change_passphrase(state: S<'_>, current: String, new: String) -> Result<(), String> {
    passphrase_ok(&new)?;
    let vault = paths::vault_path();
    let mut account = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || Account::open(&vault, &current, kdf())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|_| "wrong passphrase".to_owned())?;
    let contents = std::mem::take(&mut account.contents);
    let v = hashgram_sdk::Vault::at(&vault);
    v.write(&new, &contents, kdf()).map_err(|e| e.to_string())?;
    // Hello wraps the old passphrase; it must be re-enrolled.
    let _ = std::fs::remove_file(paths::hello_blob_path());
    {
        let mut s = state.settings.write().await;
        s.security.hello_enabled = false;
        let _ = s.save(&paths::settings_path());
    }
    state.lock().await;
    Ok(())
}

/// Enrols Windows Hello: wraps the passphrase with DPAPI under entropy that
/// exists only after a Hello prompt. The mnemonic is not involved.
#[tauri::command]
pub async fn hello_enable(state: S<'_>, passphrase: String) -> Result<(), String> {
    // Prove the passphrase first.
    let vault = paths::vault_path();
    tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        let p = passphrase.clone();
        move || Account::open(&vault, &p, kdf())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|_| "wrong passphrase".to_owned())?;
    let blob = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let sig1 = crate::winsec::hello_create_and_sign(
            crate::winsec::HELLO_CREDENTIAL,
            crate::winsec::HELLO_CHALLENGE,
        )?;
        // The entropy must be reproducible: sign again and compare. If the
        // key produces non-deterministic signatures, Hello cannot be used
        // this way and we say so instead of enrolling something that will
        // never unlock.
        let sig2 = crate::winsec::hello_sign(
            crate::winsec::HELLO_CREDENTIAL,
            crate::winsec::HELLO_CHALLENGE,
        )?;
        if sig1 != sig2 {
            let _ = crate::winsec::hello_delete(crate::winsec::HELLO_CREDENTIAL);
            return Err("this PC's Windows Hello key does not produce reproducible signatures; Hello unlock is not available here".into());
        }
        let entropy = crate::winsec::entropy_from_signature(&sig1);
        crate::winsec::dpapi_protect(passphrase.as_bytes(), &entropy)
    })
    .await
    .map_err(|e| e.to_string())??;
    std::fs::write(paths::hello_blob_path(), blob).map_err(|e| e.to_string())?;
    let mut s = state.settings.write().await;
    s.security.hello_enabled = true;
    s.save(&paths::settings_path()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Unlocks with Windows Hello.
#[tauri::command]
pub async fn hello_unlock(state: S<'_>) -> Result<AccountInfo, String> {
    let blob = std::fs::read(paths::hello_blob_path())
        .map_err(|_| "Windows Hello is not enrolled on this PC".to_owned())?;
    let vault = paths::vault_path();
    let account = tauri::async_runtime::spawn_blocking(move || -> Result<Account, String> {
        let sig = crate::winsec::hello_sign(
            crate::winsec::HELLO_CREDENTIAL,
            crate::winsec::HELLO_CHALLENGE,
        )?;
        let entropy = crate::winsec::entropy_from_signature(&sig);
        let pass = Zeroizing::new(crate::winsec::dpapi_unprotect(&blob, &entropy)?);
        let pass = std::str::from_utf8(&pass).map_err(|_| "corrupt Hello blob".to_owned())?;
        Account::open(&vault, pass, kdf()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    open_session(&state, account).await
}

/// Disables Windows Hello unlock.
#[tauri::command]
pub async fn hello_disable(state: S<'_>) -> Result<(), String> {
    let _ = std::fs::remove_file(paths::hello_blob_path());
    let _ = tauri::async_runtime::spawn_blocking(|| {
        crate::winsec::hello_delete(crate::winsec::HELLO_CREDENTIAL)
    })
    .await;
    let mut s = state.settings.write().await;
    s.security.hello_enabled = false;
    s.save(&paths::settings_path()).map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes the vault and all local data (the user confirmed and knows the
/// 24 words are the only way back). Refused while unlocked, so it cannot be
/// triggered by a stray click: lock first.
#[tauri::command]
pub async fn wipe_local_data(state: S<'_>, confirm: String) -> Result<(), String> {
    if confirm != "DELETE" {
        return Err("type DELETE to confirm".into());
    }
    if state.session.read().await.is_some() {
        return Err("lock the app first".into());
    }
    let _ = std::fs::remove_file(paths::vault_path());
    let _ = std::fs::remove_file(paths::hello_blob_path());
    let _ = std::fs::remove_file(paths::db_path());
    let _ = std::fs::remove_file(paths::db_path().with_extension("db-wal"));
    let _ = std::fs::remove_file(paths::db_path().with_extension("db-shm"));
    let mut s = state.settings.write().await;
    s.onboarding_done = false;
    s.security.hello_enabled = false;
    let _ = s.save(&paths::settings_path());
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Reads settings.
#[tauri::command]
pub async fn settings_get(state: S<'_>) -> Result<Settings, String> {
    Ok(state.settings.read().await.clone())
}

/// Writes settings; restarts the network when the profile changed.
#[tauri::command]
pub async fn settings_set(state: S<'_>, app: AppHandle, settings: Settings) -> Result<(), String> {
    let network_changed = {
        let cur = state.settings.read().await;
        cur.network != settings.network
    };
    for url in &settings.network.https_endpoints {
        if !(url.starts_with("https://")
            || url.starts_with("http://127.0.0.1")
            || url.starts_with("http://localhost"))
        {
            return Err(format!(
                "{url:?}: endpoints must be https:// (plain http only on this PC)"
            ));
        }
    }
    settings
        .save(&paths::settings_path())
        .map_err(|e| e.to_string())?;
    *state.settings.write().await = settings.clone();
    if network_changed {
        state.net.stop().await;
        state.chain.invalidate().await;
        if let Ok(identity) = crate::net::identity_for(&settings) {
            state.chain.set_chain_id(&identity.chain_id).await;
        }
        let st = state.inner().clone();
        tauri::async_runtime::spawn(async move {
            let s = st.settings.read().await.clone();
            if let Err(e) = st.net.start(&s, &st.db).await {
                tracing::warn!(error = %e, "network restart failed");
            }
        });
    } else {
        state.chain.invalidate().await;
    }
    let _ = app.emit("settings:changed", ());
    Ok(())
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// The Connected-nodes panel data.
#[tauri::command]
pub async fn net_snapshot(state: S<'_>) -> Result<NetSnapshot, String> {
    Ok(state.net.snapshot(&state.db).await)
}

/// Measures latency to every peer now.
#[tauri::command]
pub async fn net_measure_latency(state: S<'_>) -> Result<(), String> {
    state.net.measure_latency().await;
    Ok(())
}

/// Forgets every remembered peer and reconnects from the built-in list.
#[tauri::command]
pub async fn net_forget_peers(state: S<'_>) -> Result<(), String> {
    state.net.stop().await;
    let _ = std::fs::remove_file(paths::peerstore_path());
    state.db.forget_peers()?;
    state.chain.invalidate().await;
    let s = state.settings.read().await.clone();
    state.net.start(&s, &state.db).await.map(|_| ())
}

/// Reconnects (after a failure) without forgetting peers.
#[tauri::command]
pub async fn net_reconnect(state: S<'_>) -> Result<(), String> {
    state.net.stop().await;
    state.chain.invalidate().await;
    let s = state.settings.read().await.clone();
    state.net.start(&s, &state.db).await.map(|_| ())
}

/// Chain source health (the dots).
#[derive(Debug, Clone, Serialize)]
pub struct ChainHealth {
    /// Each source.
    pub sources: Vec<EndpointHealth>,
    /// The one in use.
    pub active: Option<Source>,
    /// Distinct relay operators reachable.
    pub relay_operators: usize,
}

/// Health of every chain source, re-probing.
#[tauri::command]
pub async fn chain_health(state: S<'_>, probe: bool) -> Result<ChainHealth, String> {
    if probe {
        state.chain.invalidate().await;
        let s = state.settings.read().await.clone();
        let link = state.net.link().await;
        let _ = state
            .chain
            .client(link, &s.network.local_node_api, &s.network.https_endpoints)
            .await;
    }
    Ok(ChainHealth {
        sources: state.chain.health().await,
        active: state.chain.active_source().await,
        relay_operators: state.net.relay_operator_count().await,
    })
}

/// A diagnostics report with no IP addresses of other peers.
#[tauri::command]
pub async fn diagnostics_export(state: S<'_>, app: AppHandle) -> Result<String, String> {
    let snap = state.net.snapshot(&state.db).await;
    let settings = state.settings.read().await.clone();
    let health = state.chain.health().await;
    let mut out = String::new();
    out.push_str(&format!(
        "Hashgram for Windows {} ({})\n",
        app.package_info().version,
        COMMIT
    ));
    out.push_str(&format!(
        "network {} chain {} genesis {}\n",
        snap.network, snap.chain_id, snap.genesis_hash
    ));
    out.push_str(&format!(
        "own peer id {}\nnat {}\nverified peers {}\nkad {}\npeerstore {}\nuptime {}s\n",
        snap.own_peer_id,
        snap.nat,
        snap.verified,
        snap.kad_peers,
        snap.peerstore_size,
        snap.uptime_secs
    ));
    out.push_str("\npeers (no addresses):\n");
    for p in &snap.peers {
        out.push_str(&format!(
            "  {} roles={} operator={} transport={} latency={} discovery={} verified={}\n",
            p.peer_id,
            p.roles.join(","),
            if p.operator.is_empty() {
                "-"
            } else {
                &p.operator
            },
            p.transport,
            p.latency_ms
                .map(|l| l.to_string())
                .unwrap_or_else(|| "-".into()),
            p.discovery,
            p.verified
        ));
    }
    out.push_str("\nrejected:\n");
    for r in &snap.rejected {
        out.push_str(&format!("  {} {} ({})\n", r.peer_id, r.label, r.reason));
    }
    out.push_str("\nchain sources:\n");
    for h in &health {
        out.push_str(&format!(
            "  {:?} ok={} {} {}ms{}\n",
            h.source,
            h.ok,
            h.detail,
            h.latency_ms,
            if h.active { " [active]" } else { "" }
        ));
    }
    out.push_str(&format!(
        "\nsettings.network: https_endpoints={} local_node_api={}\n",
        settings.network.https_endpoints.len(),
        settings.network.local_node_api
    ));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Chain reads
// ---------------------------------------------------------------------------

async fn chain_read(state: &AppState, path: &str) -> Result<ChainRead, String> {
    let s = state.settings.read().await.clone();
    let link = state.net.link().await;
    let r = state
        .chain
        .get(
            link,
            &s.network.local_node_api,
            &s.network.https_endpoints,
            path,
        )
        .await?;
    if !r.cached {
        state.net.note_read(r.verification.clone()).await;
    }
    Ok(r)
}

/// One allow-listed chain read, with verification.
#[tauri::command]
pub async fn chain_get(state: S<'_>, path: String) -> Result<ChainRead, String> {
    chain_read(&state, &path).await
}

/// Several reads at once (one screen, one round of dots).
#[tauri::command]
pub async fn chain_get_many(
    state: S<'_>,
    paths: Vec<String>,
) -> Result<Vec<Result<ChainRead, String>>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(chain_read(&state, &p).await);
    }
    Ok(out)
}

async fn chain_client(state: &AppState) -> Result<(Source, ChainClient), String> {
    let s = state.settings.read().await.clone();
    let link = state.net.link().await;
    state
        .chain
        .client(link, &s.network.local_node_api, &s.network.https_endpoints)
        .await
}

/// Wallet overview.
#[derive(Debug, Clone, Serialize)]
pub struct WalletOverview {
    /// Address.
    pub address: String,
    /// Spendable balance in uhash (decimal string).
    pub balance_uhash: String,
    /// Account exists on chain.
    pub account_exists: bool,
    /// Account number.
    pub account_number: Option<u64>,
    /// Sequence.
    pub sequence: Option<u64>,
    /// Vesting account type, if any.
    pub vesting_type: Option<String>,
    /// Original vesting in uhash, if vesting.
    pub original_vesting_uhash: Option<String>,
    /// Vesting end time (unix seconds), if any.
    pub vesting_end: Option<i64>,
    /// Vesting start time.
    pub vesting_start: Option<i64>,
    /// Verified `@username`, if owned.
    pub username: Option<String>,
    /// Verification of the balance read.
    pub verification: Option<Verification>,
    /// Source used.
    pub source: Source,
    /// Height of the balance read.
    pub height: u64,
}

fn s_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
fn s_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Reads balance, account and username for the unlocked address.
#[tauri::command]
pub async fn wallet_overview(state: S<'_>) -> Result<WalletOverview, String> {
    let address = state
        .session
        .read()
        .await
        .as_ref()
        .map(|s| s.address.clone())
        .ok_or_else(|| "locked".to_owned())?;
    let bal = chain_read(
        &state,
        &format!("cosmos/bank/v1beta1/balances/{address}/by_denom?denom=uhash"),
    )
    .await?;
    let balance_uhash = bal
        .value
        .get("balance")
        .and_then(|b| b.get("amount"))
        .and_then(|a| a.as_str())
        .unwrap_or("0")
        .to_owned();
    let acct = chain_read(&state, &format!("cosmos/auth/v1beta1/accounts/{address}")).await;
    let (
        account_exists,
        account_number,
        sequence,
        vesting_type,
        original_vesting,
        vesting_end,
        vesting_start,
    ) = match acct {
        Ok(r) => {
            let a = r.value.get("account").cloned().unwrap_or_default();
            let ty = a
                .get("@type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_owned();
            let base = a
                .get("base_vesting_account")
                .and_then(|v| v.get("base_account"))
                .or_else(|| a.get("base_account"))
                .unwrap_or(&a);
            let num = base.get("account_number").and_then(s_u64);
            let seq = base.get("sequence").and_then(s_u64);
            let bva = a.get("base_vesting_account");
            let ov = bva
                .and_then(|v| v.get("original_vesting"))
                .and_then(|v| v.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|c| c.get("denom").and_then(|d| d.as_str()) == Some("uhash"))
                })
                .and_then(|c| c.get("amount"))
                .and_then(|x| x.as_str())
                .map(str::to_owned);
            let end = bva.and_then(|v| v.get("end_time")).and_then(s_i64);
            let start = a.get("start_time").and_then(s_i64);
            let vt = if ty.contains("Vesting") {
                Some(ty)
            } else {
                None
            };
            (true, num, seq, vt, ov, end, start)
        }
        Err(e) if e.contains("404") || e.contains("not found") => {
            (false, None, None, None, None, None, None)
        }
        Err(e) => return Err(e),
    };
    let username = chain_read(&state, &format!("hashgram/username/v1/reverse/{address}"))
        .await
        .ok()
        .and_then(|r| {
            r.value
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        });
    Ok(WalletOverview {
        address,
        balance_uhash,
        account_exists,
        account_number,
        sequence,
        vesting_type,
        original_vesting_uhash: original_vesting,
        vesting_end,
        vesting_start,
        username,
        verification: bal.verification,
        source: bal.source,
        height: bal.height,
    })
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

/// What the confirm screen shows.
#[derive(Debug, Clone, Serialize)]
pub struct TxPreview {
    /// Summary.
    pub summary: String,
    /// Warnings shown before confirm.
    pub warnings: Vec<String>,
    /// Gas limit.
    pub gas_limit: u64,
    /// Fee in uhash.
    pub fee_uhash: String,
    /// Fee came from simulation (true) or estimate (false).
    pub simulated: bool,
    /// Founder share of the fee in uhash (1 %).
    pub founder_share_uhash: String,
    /// Source that will broadcast.
    pub source: Source,
}

async fn wallet_of(session: &Session) -> Result<Wallet, String> {
    session.account.lock().await.wallet().map_err(|_| {
        "this device does not hold the wallet key (restore from the 24 words to send)".to_owned()
    })
}

/// Previews a transaction: summary, warnings, gas and fee.
#[tauri::command]
pub async fn tx_preview(state: S<'_>, spec: MsgSpec) -> Result<TxPreview, String> {
    let (wallet, address) = {
        let s = state.session.read().await;
        let s = s.as_ref().ok_or_else(|| "locked".to_owned())?;
        (wallet_of(s).await?, s.address.clone())
    };
    let built = spec.build(&address)?;
    let (source, client) = chain_client(&state).await?;
    let fee = client
        .fee_preview(&wallet, built.msgs.clone(), "")
        .await
        .map_err(|e| e.to_string())?;
    Ok(TxPreview {
        summary: built.summary,
        warnings: built.warnings,
        gas_limit: fee.gas_limit,
        fee_uhash: fee.fee_uhash.to_string(),
        simulated: fee.simulated,
        founder_share_uhash: fee.fee_uhash.div_euclid(100).to_string(),
        source,
    })
}

/// A submitted transaction.
#[derive(Debug, Clone, Serialize)]
pub struct TxSubmitted {
    /// Hex hash.
    pub hash: String,
    /// Summary.
    pub summary: String,
}

/// Signs and broadcasts; returns as soon as a node accepted it. Inclusion
/// is tracked in the background and reported as `tx:update` events.
#[tauri::command]
pub async fn tx_submit(
    state: S<'_>,
    app: AppHandle,
    spec: MsgSpec,
    memo: String,
) -> Result<TxSubmitted, String> {
    let (wallet, address) = {
        let s = state.session.read().await;
        let s = s.as_ref().ok_or_else(|| "locked".to_owned())?;
        (wallet_of(s).await?, s.address.clone())
    };
    if memo.len() > 256 {
        return Err("memo is too long".into());
    }
    let built = spec.build(&address)?;
    let (_, client) = chain_client(&state).await?;
    let r = client
        .sign_and_broadcast_nowait(&wallet, built.msgs, &memo)
        .await
        .map_err(|e| e.to_string())?;
    state.db.pending_put(&r.txhash, &built.summary, "pending")?;
    state.pending.lock().await.push(r.txhash.clone());
    state.chain.clear_cache().await;
    let _ = app.emit(
        "tx:update",
        serde_json::json!({ "hash": r.txhash, "state": "pending" }),
    );
    Ok(TxSubmitted {
        hash: r.txhash,
        summary: built.summary,
    })
}

/// Recently submitted transactions and their state.
#[tauri::command]
pub async fn tx_recent(state: S<'_>) -> Result<Vec<PendingRow>, String> {
    state.db.pending_list(50)
}

/// Looks a transaction up by hash.
#[tauri::command]
pub async fn tx_status(state: S<'_>, hash: String) -> Result<serde_json::Value, String> {
    let h = hash.trim().to_ascii_uppercase();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("a transaction hash is 64 hex characters".into());
    }
    let r = chain_read(&state, &format!("cosmos/tx/v1beta1/txs/{h}")).await?;
    Ok(r.value)
}

/// Polls pending transactions once; called by the background task.
pub async fn poll_pending(state: &AppState, app: &AppHandle) {
    let hashes: Vec<String> = state.pending.lock().await.clone();
    if hashes.is_empty() {
        return;
    }
    let Ok((_, client)) = chain_client(state).await else {
        return;
    };
    for h in hashes {
        match client.tx(&h).await {
            Ok(Some(t)) => {
                let st = if t.code == 0 { "committed" } else { "failed" };
                let _ = state.db.pending_update(&h, st, t.height, &t.raw_log);
                state.pending.lock().await.retain(|x| x != &h);
                state.chain.clear_cache().await;
                let _ = app.emit(
                    "tx:update",
                    serde_json::json!({ "hash": h, "state": st, "height": t.height, "raw_log": t.raw_log }),
                );
            }
            Ok(None) => {}
            Err(e) => tracing::debug!(hash = %h, error = %e, "tx poll"),
        }
    }
}

/// Whether a transaction is pending (the updater must not run then).
#[tauri::command]
pub async fn tx_has_pending(state: S<'_>) -> Result<bool, String> {
    Ok(!state.pending.lock().await.is_empty())
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Identity state on chain for this account.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityStatus {
    /// An identity exists on chain.
    pub registered: bool,
    /// Devices on chain.
    pub devices: Vec<DeviceInfo>,
    /// This PC's device id.
    pub this_device_id: String,
    /// This PC's device public key (hex).
    pub this_device_pubkey: String,
    /// Whether this PC's device is registered.
    pub this_device_registered: bool,
    /// Root rotation count.
    pub rotation_count: Option<u32>,
    /// Recovery config, raw.
    pub recovery: Option<serde_json::Value>,
}

/// One device.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    /// Id.
    pub device_id: String,
    /// Label.
    pub label: String,
    /// Platform.
    pub platform: String,
    /// Revoked.
    pub revoked: bool,
    /// Public key hex.
    pub pubkey_hex: String,
    /// This PC.
    pub is_this_device: bool,
}

/// Reads identity and devices from chain.
#[tauri::command]
pub async fn identity_status(state: S<'_>) -> Result<IdentityStatus, String> {
    let (address, did, dpk) = {
        let (account, _, address) = state.session_handles().await?;
        let a = account.lock().await;
        let dev = a.device().map_err(|e| e.to_string())?;
        (
            address,
            a.contents.device_id.clone(),
            hex::encode(dev.public_key()),
        )
    };
    let (_, client) = chain_client(&state).await?;
    let rotation = account::rotation_count_on_chain(&client, &address)
        .await
        .map_err(|e| e.to_string())?;
    let devices = if rotation.is_some() {
        account::devices_on_chain(&client, &address)
            .await
            .map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    let devices: Vec<DeviceInfo> = devices
        .into_iter()
        .map(|d| {
            let pk = hex::encode(&d.device_pubkey);
            DeviceInfo {
                is_this_device: pk == dpk,
                device_id: d.device_id,
                label: d.label,
                platform: d.platform,
                revoked: d.revoked,
                pubkey_hex: pk,
            }
        })
        .collect();
    let recovery = chain_read(&state, &format!("hashgram/identity/v1/recovery/{address}"))
        .await
        .ok()
        .map(|r| r.value);
    Ok(IdentityStatus {
        registered: rotation.is_some(),
        this_device_registered: devices.iter().any(|d| d.is_this_device && !d.revoked),
        devices,
        this_device_id: did,
        this_device_pubkey: dpk,
        rotation_count: rotation,
        recovery,
    })
}

/// Registers this account's identity (`MsgCreateIdentity`) or, when it
/// already exists, this PC as a device (`MsgAddDevice`). Only public keys
/// go on chain.
#[tauri::command]
pub async fn identity_register(
    state: S<'_>,
    app: AppHandle,
    label: String,
) -> Result<TxSubmitted, String> {
    let label = if label.trim().is_empty() {
        state.settings.read().await.device_label.clone()
    } else {
        label.trim().to_owned()
    };
    let (_, client) = chain_client(&state).await?;
    let identity = state
        .net
        .identity()
        .await
        .ok_or_else(|| "network not started".to_owned())?;
    let (account, _, address) = state.session_handles().await?;
    let existing = account::rotation_count_on_chain(&client, &address)
        .await
        .map_err(|e| e.to_string())?;
    let a = account.lock().await;
    let (r, summary) = if existing.is_none() {
        let r = account::create_identity_on_chain(&a, &identity, &client, &label, "windows")
            .await
            .map_err(|e| e.to_string())?;
        (r, format!("Create identity with device \"{label}\""))
    } else {
        let rotation = existing.unwrap_or(0);
        let device = a.device().map_err(|e| e.to_string())?;
        let r = account::add_device_on_chain(
            &a,
            &identity,
            &client,
            &a.contents.device_id,
            &device.public_key(),
            rotation,
            &label,
            "windows",
        )
        .await
        .map_err(|e| e.to_string())?;
        (r, format!("Add device \"{label}\""))
    };
    drop(a);
    let st = if r.height > 0 { "committed" } else { "pending" };
    state.db.pending_put(&r.txhash, &summary, st)?;
    if st == "pending" {
        state.pending.lock().await.push(r.txhash.clone());
    }
    state.chain.clear_cache().await;
    let _ = app.emit(
        "tx:update",
        serde_json::json!({ "hash": r.txhash, "state": st }),
    );
    Ok(TxSubmitted {
        hash: r.txhash,
        summary,
    })
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// What a search query resolved to.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchResult {
    /// An address (optionally with its @username).
    Address {
        /// Address.
        address: String,
        /// Reverse-resolved name.
        username: Option<String>,
    },
    /// A username that resolved.
    Username {
        /// Name.
        name: String,
        /// Owner address.
        address: String,
        /// Expiry height.
        expiry_height: Option<i64>,
    },
    /// A username that is free.
    UsernameAvailable {
        /// Name.
        name: String,
        /// Confusable with an existing name.
        confusable_with: Vec<String>,
    },
    /// A transaction.
    Tx {
        /// Hash.
        hash: String,
        /// Height, if found.
        height: Option<u64>,
        /// Found.
        found: bool,
    },
    /// A hashtag.
    Hashtag {
        /// Tag.
        tag: String,
    },
    /// A channel.
    Channel {
        /// Channel id or name.
        id: String,
    },
    /// Nothing matched.
    Nothing {
        /// Why.
        reason: String,
    },
}

/// Resolves a search: address → @username → tx hash → #hashtag → channel.
/// Never a fuzzy "people you may know" from a server; there is none.
#[tauri::command]
pub async fn search_resolve(state: S<'_>, query: String) -> Result<SearchResult, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(SearchResult::Nothing {
            reason: "type an address, @username, transaction hash, #hashtag or channel".into(),
        });
    }
    let _ = state.db.search_note(q);
    let q = q.strip_prefix("hashgram://").unwrap_or(q);
    if tx::validate_address(q, tx::ADDRESS_PREFIX).is_ok() {
        let username = chain_read(&state, &format!("hashgram/username/v1/reverse/{q}"))
            .await
            .ok()
            .and_then(|r| {
                r.value
                    .get("name")
                    .and_then(|n| n.as_str())
                    .filter(|n| !n.is_empty())
                    .map(str::to_owned)
            });
        return Ok(SearchResult::Address {
            address: q.to_owned(),
            username,
        });
    }
    if let Some(name) = q.strip_prefix('@') {
        let name = name.to_ascii_lowercase();
        if let Ok(r) = chain_read(&state, &format!("hashgram/username/v1/lookup/{name}")).await {
            {
                let owner = r
                    .value
                    .get("owner")
                    .or_else(|| r.value.get("registration").and_then(|x| x.get("owner")))
                    .and_then(|o| o.as_str())
                    .unwrap_or("")
                    .to_owned();
                if !owner.is_empty() {
                    let expiry = r
                        .value
                        .get("expiry_height")
                        .or_else(|| {
                            r.value
                                .get("registration")
                                .and_then(|x| x.get("expiry_height"))
                        })
                        .and_then(s_i64);
                    return Ok(SearchResult::Username {
                        name,
                        address: owner,
                        expiry_height: expiry,
                    });
                }
            }
        }
        let confusable = chain_read(&state, &format!("hashgram/username/v1/availability/{name}"))
            .await
            .ok()
            .and_then(|r| {
                r.value
                    .get("confusable_with")
                    .and_then(|c| c.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
            })
            .unwrap_or_default();
        return Ok(SearchResult::UsernameAvailable {
            name,
            confusable_with: confusable,
        });
    }
    if q.len() == 64 && q.chars().all(|c| c.is_ascii_hexdigit()) {
        let h = q.to_ascii_uppercase();
        return match chain_read(&state, &format!("cosmos/tx/v1beta1/txs/{h}")).await {
            Ok(r) => Ok(SearchResult::Tx {
                hash: h,
                height: r
                    .value
                    .get("tx_response")
                    .and_then(|t| t.get("height"))
                    .and_then(s_u64),
                found: true,
            }),
            Err(_) => Ok(SearchResult::Tx {
                hash: h,
                height: None,
                found: false,
            }),
        };
    }
    if let Some(tag) = q.strip_prefix('#') {
        return Ok(SearchResult::Hashtag {
            tag: tag.to_ascii_lowercase(),
        });
    }
    if let Some(ch) = q.strip_prefix("channel:") {
        return Ok(SearchResult::Channel { id: ch.to_owned() });
    }
    // A bare word is tried as a username too, since people forget the @.
    if q.len() >= 3 && q.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        let name = q.to_ascii_lowercase();
        if let Ok(r) = chain_read(&state, &format!("hashgram/username/v1/lookup/{name}")).await {
            if let Some(owner) = r
                .value
                .get("owner")
                .and_then(|o| o.as_str())
                .filter(|o| !o.is_empty())
            {
                return Ok(SearchResult::Username {
                    name,
                    address: owner.to_owned(),
                    expiry_height: r.value.get("expiry_height").and_then(s_i64),
                });
            }
        }
    }
    Ok(SearchResult::Nothing {
        reason: "not an address, @username, transaction hash, #hashtag or channel".into(),
    })
}

/// Recent searches.
#[tauri::command]
pub async fn search_recent(state: S<'_>) -> Result<Vec<String>, String> {
    state.db.search_recent()
}

// ---------------------------------------------------------------------------
// QR, help, perf
// ---------------------------------------------------------------------------

/// A monochrome QR code as an SVG string (white modules on transparent; the
/// page paints the black behind it).
#[tauri::command]
pub fn qr_svg(text: String) -> Result<String, String> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| e.to_string())?;
    let w = code.width();
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {w}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"QR code\">"
    );
    svg.push_str("<path fill=\"#ffffff\" d=\"");
    for y in 0..w {
        for x in 0..w {
            if code[(x, y)] == qrcode::Color::Dark {
                svg.push_str(&format!("M{x} {y}h1v1h-1z"));
            }
        }
    }
    svg.push_str("\"/></svg>");
    Ok(svg)
}

/// Help page list.
#[tauri::command]
pub fn help_list() -> Vec<crate::help::HelpPage> {
    crate::help::PAGES.to_vec()
}

/// Help page HTML.
#[tauri::command]
pub fn help_page(slug: String) -> Result<String, String> {
    crate::help::render(&slug, COMMIT).ok_or_else(|| format!("no help page {slug:?}"))
}

/// Performance records.
#[tauri::command]
pub fn perf_snapshot(state: S<'_>) -> Vec<crate::perf::SpanRecord> {
    state.perf.snapshot()
}

/// A UI-side mark (e.g. "interactive" at cold start).
#[tauri::command]
pub fn perf_mark(state: S<'_>, name: String, micros: u64) {
    state.perf.push(name, micros, "ui");
}

/// Memory use of this process in bytes (private working set), for the
/// panel. WebView2 renderer processes are separate; the panel says so.
#[tauri::command]
pub fn perf_memory() -> u64 {
    crate::winsec::working_set_bytes()
}

/// Opens the data folder in Explorer.
#[tauri::command]
pub fn open_data_dir(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(paths::data_dir().display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// A frontend log line (uncaught errors, boot failures) into the Rust log.
/// Truncated and never containing secrets: the UI only reports messages it
/// composes itself.
#[tauri::command]
pub fn ui_log(level: String, message: String) {
    let msg: String = message.chars().take(2000).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "ui", "{msg}"),
        "warn" => tracing::warn!(target: "ui", "{msg}"),
        _ => tracing::info!(target: "ui", "{msg}"),
    }
}

/// Writes text to a path the user chose in a save dialog (CSV export,
/// diagnostics). Refuses paths inside the app's own data directory so an
/// export can never overwrite the vault or database.
#[tauri::command]
pub fn save_text_file(path: String, contents: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    let data = paths::data_dir();
    if p.starts_with(&data) {
        return Err("choose a location outside the Hashgram data folder".into());
    }
    if contents.len() > 64 * 1024 * 1024 {
        return Err("export too large".into());
    }
    std::fs::write(&p, contents.as_bytes()).map_err(|e| e.to_string())
}

/// Deep-link payload delivered to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepLink {
    /// Raw URL.
    pub url: String,
}

#[cfg(test)]
mod updater_tests {
    use super::minisign_key_id;

    #[test]
    fn key_id_comes_out_of_the_minisign_comment() {
        // "untrusted comment: minisign public key: 32E94D6E87BA4BE5\nRW..." base64-encoded.
        let b64 = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMyRTk0RDZFODdCQTRCRTUKUldUbFM3cUhiazNwTW55NExKb0U5OWh3N0NLTU9panBZaU41TGVEa3pjeFBaNEtTUnlJcTZDcDIK";
        assert_eq!(minisign_key_id(b64), "32E94D6E87BA4BE5");
    }

    #[test]
    fn garbage_yields_an_empty_id() {
        assert_eq!(
            minisign_key_id("REPLACE_WITH_MINISIGN_PUBLIC_KEY_KEPT_OFFLINE"),
            ""
        );
        assert_eq!(minisign_key_id("!!!"), "");
    }
}
