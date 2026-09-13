//! Identity and vault: status, onboarding, unlock/lock, Windows Hello,
//! passphrase, on-chain identity and devices, wipe.

use std::sync::Arc;

use hashgram_sdk::account::{self, Account, MNEMONIC_WORDS};
use hashgram_sdk::{KdfCost, Wallet};
use serde::Serialize;
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use crate::error::{CmdResult, UiError};
use crate::paths;
use crate::session::{self, AccountInfo};
use crate::settings::NetworkKind;
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

/// The commit this build was made from.
pub const COMMIT: &str = match option_env!("HASHGRAM_COMMIT") {
    Some(c) => c,
    None => "unknown",
};

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
    /// Genesis hash.
    pub genesis_hash: String,
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
    /// Where the updater fetches its signed manifest from.
    pub updater_endpoint: String,
    /// Whether this binary carries an Authenticode signature (release
    /// builds set `HASHGRAM_CODESIGNED=1` when signtool ran).
    pub code_signed: bool,
    /// Milliseconds since process start (for the cold-start budget).
    pub uptime_ms: u64,
    /// Network link up.
    pub link_up: bool,
    /// Why the link is down, if it is.
    pub link_error: Option<String>,
}

/// Reports app status.
#[tauri::command]
pub async fn app_status(state: S<'_>, app: AppHandle) -> CmdResult<AppStatus> {
    let settings = state.settings.read().await.clone();
    let session = state.session.read().await.clone();
    let identity = session::network_identity(&settings).ok();
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
        chain_id: identity.as_ref().map(|i| i.chain_id.clone()).unwrap_or_default(),
        genesis_hash: identity.map(|i| i.genesis_hash).unwrap_or_default(),
        onboarding_done: settings.onboarding_done,
        hello_available,
        hello_enabled: settings.security.hello_enabled && paths::hello_blob_path().exists(),
        version: app.package_info().version.to_string(),
        commit: COMMIT.to_owned(),
        data_dir: paths::data_dir().display().to_string(),
        updater_configured: !updater_pubkey.is_empty() && !updater_pubkey.starts_with("REPLACE_WITH"),
        updater_endpoint,
        code_signed: option_env!("HASHGRAM_CODESIGNED").map(|v| v == "1").unwrap_or(false),
        uptime_ms: state.perf.uptime_ms(),
        link_up: state.link.read().await.is_some(),
        link_error: state.link_error.read().await.clone(),
    })
}

// ---------------------------------------------------------------------------
// Onboarding
// ---------------------------------------------------------------------------

fn device_id() -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows-pc".into());
    let mut rnd = [0u8; 4];
    let _ = getrandom::fill(&mut rnd);
    format!("{}-{}", host.to_ascii_lowercase(), hex::encode(rnd))
}

fn passphrase_ok(p: &str) -> CmdResult<()> {
    if p.chars().count() < 10 {
        return Err(UiError::invalid("the passphrase needs at least 10 characters"));
    }
    Ok(())
}

async fn remember_label(state: &AppState, device_label: &str) -> CmdResult<()> {
    let mut s = state.settings.write().await;
    s.onboarding_done = true;
    s.device_label = if device_label.trim().is_empty() {
        std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
    } else {
        device_label.trim().to_owned()
    };
    s.save(&paths::settings_path())?;
    Ok(())
}

/// Generates the 24 words for a new account and holds them in memory until
/// [`onboarding_create`] commits them to a vault. Never written anywhere.
#[tauri::command]
pub async fn onboarding_generate(state: S<'_>) -> CmdResult<Vec<String>> {
    if paths::vault_path().exists() {
        return Err(UiError::invalid(
            "a vault already exists on this PC; unlock it or wipe it from Settings → Advanced",
        ));
    }
    let (mnemonic, _wallet) = Wallet::generate().map_err(|e| UiError::internal(e.to_string()))?;
    let words: Vec<String> = mnemonic.split_whitespace().map(str::to_owned).collect();
    if words.len() != MNEMONIC_WORDS {
        return Err(UiError::internal("generator did not produce 24 words"));
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
) -> CmdResult<bool> {
    let pending = state.pending_mnemonic.lock().await;
    let Some(m) = pending.as_ref() else {
        return Err(UiError::invalid("no words are pending; start again"));
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

/// Creates the vault from the pending mnemonic with a passphrase.
#[tauri::command]
pub async fn onboarding_create(
    state: S<'_>,
    app: AppHandle,
    passphrase: String,
    device_label: String,
) -> CmdResult<AccountInfo> {
    passphrase_ok(&passphrase)?;
    let mnemonic = state
        .pending_mnemonic
        .lock()
        .await
        .take()
        .ok_or_else(|| UiError::invalid("no words are pending; start again"))?;
    let vault = paths::vault_path();
    if vault.exists() {
        return Err(UiError::invalid("a vault already exists on this PC"));
    }
    paths::ensure_dirs()?;
    let did = device_id();
    let passphrase = Zeroizing::new(passphrase);
    let account = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || Account::import(&vault, &passphrase, &mnemonic, &did, session::kdf())
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    remember_label(&state, &device_label).await?;
    session::open_session(&app, &state, account).await
}

/// Address preview.
#[derive(Debug, Clone, Serialize)]
pub struct RestorePreview {
    /// Address the words derive.
    pub address: String,
}

fn normalise_mnemonic(m: &str) -> CmdResult<Zeroizing<String>> {
    let words: Vec<String> = m
        .split_whitespace()
        .map(|w| w.trim().to_ascii_lowercase())
        .collect();
    if words.len() != MNEMONIC_WORDS {
        return Err(UiError::invalid(format!(
            "a Hashgram account is {MNEMONIC_WORDS} words; you entered {}",
            words.len()
        )));
    }
    Ok(Zeroizing::new(words.join(" ")))
}

/// Derives the address from 24 words so the user can confirm before a
/// vault is written. Nothing is stored.
#[tauri::command]
pub async fn onboarding_restore_preview(mnemonic: String) -> CmdResult<RestorePreview> {
    let m = normalise_mnemonic(&mnemonic)?;
    let w = Wallet::from_mnemonic(&m, "", 0).map_err(|e| {
        let s = e.to_string();
        if s.contains("checksum") || s.contains("word") {
            UiError::invalid("these words are not a valid phrase (check spelling and order)")
        } else {
            UiError::invalid(s)
        }
    })?;
    Ok(RestorePreview {
        address: w.address().to_string(),
    })
}

/// Restores an account from 24 words into a new vault.
#[tauri::command]
pub async fn onboarding_restore(
    state: S<'_>,
    app: AppHandle,
    mnemonic: String,
    passphrase: String,
    device_label: String,
) -> CmdResult<AccountInfo> {
    passphrase_ok(&passphrase)?;
    let m = normalise_mnemonic(&mnemonic)?;
    let vault = paths::vault_path();
    if vault.exists() {
        return Err(UiError::invalid(
            "a vault already exists on this PC; wipe it from Settings → Advanced first",
        ));
    }
    paths::ensure_dirs()?;
    let did = device_id();
    let passphrase = Zeroizing::new(passphrase);
    let account = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || Account::import(&vault, &passphrase, &m, &did, session::kdf())
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    remember_label(&state, &device_label).await?;
    session::open_session(&app, &state, account).await
}

/// What a backup file says about itself before decryption.
#[derive(Debug, Clone, Serialize)]
pub struct BackupInfo {
    /// Argon2id memory (KiB).
    pub m_cost_kib: u32,
    /// Iterations.
    pub t_cost: u32,
    /// Lanes.
    pub p_cost: u8,
    /// File size.
    pub bytes: u64,
}

/// Inspects a backup file (header only, no passphrase).
#[tauri::command]
pub async fn backup_inspect(path: String) -> CmdResult<BackupInfo> {
    let bytes = std::fs::read(&path)?;
    if bytes.len() > hashgram_sdk::backup::MAX_BACKUP_BYTES {
        return Err(UiError::invalid("this file is too large to be a Hashgram backup"));
    }
    let (m, t, p) = hashgram_sdk::backup::inspect_backup(&bytes)?;
    Ok(BackupInfo {
        m_cost_kib: m,
        t_cost: t,
        p_cost: p,
        bytes: bytes.len() as u64,
    })
}

/// Restores from an encrypted backup file into a new vault. The importer
/// gets a fresh device key; the root holder (this account, if the backup
/// carried the root) must add it on chain from Wallet → Devices.
#[tauri::command]
pub async fn restore_from_backup(
    state: S<'_>,
    app: AppHandle,
    path: String,
    backup_passphrase: String,
    vault_passphrase: String,
    device_label: String,
) -> CmdResult<AccountInfo> {
    passphrase_ok(&vault_passphrase)?;
    let vault = paths::vault_path();
    if vault.exists() {
        return Err(UiError::invalid(
            "a vault already exists on this PC; wipe it from Settings → Advanced first",
        ));
    }
    paths::ensure_dirs()?;
    let bytes = std::fs::read(&path)?;
    let did = device_id();
    let bp = Zeroizing::new(backup_passphrase);
    let vp = Zeroizing::new(vault_passphrase);
    let imported = tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || hashgram_sdk::backup::import_backup(&bytes, &bp, &vault, &vp, &did, session::kdf())
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    remember_label(&state, &device_label).await?;
    session::open_session(&app, &state, imported.account).await
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Unlocks with the passphrase.
#[tauri::command]
pub async fn unlock(state: S<'_>, app: AppHandle, passphrase: String) -> CmdResult<AccountInfo> {
    if state.is_unlocked().await {
        return Err(UiError::invalid("already unlocked"));
    }
    let account = session::open_account(Zeroizing::new(passphrase)).await?;
    session::open_session(&app, &state, account).await
}

/// Locks the vault.
#[tauri::command]
pub async fn lock(state: S<'_>, app: AppHandle) -> CmdResult<()> {
    session::lock(&app, &state).await;
    Ok(())
}

/// Touches the activity timer (auto-lock).
#[tauri::command]
pub async fn touch(state: S<'_>) -> CmdResult<()> {
    state.touch().await;
    Ok(())
}

/// Changes the passphrase (re-encrypts the vault) and locks.
#[tauri::command]
pub async fn change_passphrase(
    state: S<'_>,
    app: AppHandle,
    current: String,
    new: String,
) -> CmdResult<()> {
    passphrase_ok(&new)?;
    let vault = paths::vault_path();
    // Persist the live session first so the re-encryption sees its state.
    {
        let mut g = state.one.lock().await;
        if let Some(one) = g.as_mut() {
            one.save()?;
        }
    }
    let current = Zeroizing::new(current);
    let new = Zeroizing::new(new);
    tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        move || -> Result<(), hashgram_sdk::SdkError> {
            let mut account = Account::open(&vault, &current, session::kdf())?;
            let contents = std::mem::take(&mut account.contents);
            let v = hashgram_sdk::Vault::at(&vault);
            v.write(&new, &contents, session::kdf())?;
            Ok(())
        }
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    // Hello wraps the old passphrase; it must be re-enrolled.
    let _ = std::fs::remove_file(paths::hello_blob_path());
    {
        let mut s = state.settings.write().await;
        s.security.hello_enabled = false;
        let _ = s.save(&paths::settings_path());
    }
    session::lock(&app, &state).await;
    Ok(())
}

/// Enrols Windows Hello: wraps the passphrase with DPAPI under entropy that
/// exists only after a Hello prompt. The mnemonic is not involved.
#[tauri::command]
pub async fn hello_enable(state: S<'_>, passphrase: String) -> CmdResult<()> {
    let vault = paths::vault_path();
    let passphrase = Zeroizing::new(passphrase);
    tauri::async_runtime::spawn_blocking({
        let vault = vault.clone();
        let p = passphrase.clone();
        move || Account::open(&vault, &p, session::kdf())
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    let blob = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let sig1 = crate::winsec::hello_create_and_sign(
            crate::winsec::HELLO_CREDENTIAL,
            crate::winsec::HELLO_CHALLENGE,
        )?;
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
    .map_err(|e| UiError::internal(e.to_string()))?
    .map_err(UiError::internal)?;
    std::fs::write(paths::hello_blob_path(), blob)?;
    let mut s = state.settings.write().await;
    s.security.hello_enabled = true;
    s.save(&paths::settings_path())?;
    Ok(())
}

/// Unlocks with Windows Hello.
#[tauri::command]
pub async fn hello_unlock(state: S<'_>, app: AppHandle) -> CmdResult<AccountInfo> {
    if state.is_unlocked().await {
        return Err(UiError::invalid("already unlocked"));
    }
    let blob = std::fs::read(paths::hello_blob_path())
        .map_err(|_| UiError::not_found("Windows Hello is not enrolled on this PC"))?;
    let vault = paths::vault_path();
    let account = tauri::async_runtime::spawn_blocking(move || -> Result<Account, UiError> {
        let sig = crate::winsec::hello_sign(
            crate::winsec::HELLO_CREDENTIAL,
            crate::winsec::HELLO_CHALLENGE,
        )
        .map_err(|e| UiError::new("hello", e, true))?;
        let entropy = crate::winsec::entropy_from_signature(&sig);
        let pass = Zeroizing::new(
            crate::winsec::dpapi_unprotect(&blob, &entropy).map_err(|e| UiError::new("hello", e, false))?,
        );
        let pass = std::str::from_utf8(&pass).map_err(|_| UiError::internal("corrupt Hello blob"))?;
        Ok(Account::open(&vault, pass, session::kdf())?)
    })
    .await
    .map_err(|e| UiError::internal(e.to_string()))??;
    session::open_session(&app, &state, account).await
}

/// Disables Windows Hello unlock.
#[tauri::command]
pub async fn hello_disable(state: S<'_>) -> CmdResult<()> {
    let _ = std::fs::remove_file(paths::hello_blob_path());
    let _ = tauri::async_runtime::spawn_blocking(|| {
        crate::winsec::hello_delete(crate::winsec::HELLO_CREDENTIAL)
    })
    .await;
    let mut s = state.settings.write().await;
    s.security.hello_enabled = false;
    s.save(&paths::settings_path())?;
    Ok(())
}

/// Removes the vault and all local data (the user confirmed and knows the
/// 24 words are the only way back). Refused while unlocked: lock first.
#[tauri::command]
pub async fn wipe_local_data(state: S<'_>, confirm: String) -> CmdResult<()> {
    if confirm != "DELETE" {
        return Err(UiError::invalid("type DELETE to confirm"));
    }
    if state.is_unlocked().await {
        return Err(UiError::invalid("lock the app first"));
    }
    for p in [
        paths::vault_path(),
        paths::hello_blob_path(),
        paths::db_path(),
        paths::db_path().with_extension("db-wal"),
        paths::db_path().with_extension("db-shm"),
        paths::store_path(),
    ] {
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_dir_all(paths::sdk_paths().cache());
    let _ = std::fs::remove_dir_all(paths::tmp_dir());
    let mut s = state.settings.write().await;
    s.onboarding_done = false;
    s.security.hello_enabled = false;
    let _ = s.save(&paths::settings_path());
    Ok(())
}

// ---------------------------------------------------------------------------
// On-chain identity and devices
// ---------------------------------------------------------------------------

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

/// Identity state on chain for this account, with what gates messaging.
#[derive(Debug, Clone, Serialize)]
pub struct IdentityStatus {
    /// Address.
    pub address: String,
    /// An identity exists on chain.
    pub registered: bool,
    /// Devices on chain.
    pub devices: Vec<DeviceInfo>,
    /// This PC's device id.
    pub this_device_id: String,
    /// This PC's device public key (hex).
    pub this_device_pubkey: String,
    /// Whether this PC's device is registered and active.
    pub this_device_registered: bool,
    /// Root rotation count.
    pub rotation_count: Option<u32>,
    /// Our `@username`, if any.
    pub username: String,
    /// Balance in uhash (string), `None` when the chain was unreachable.
    pub balance_uhash: Option<String>,
    /// This device holds the wallet key (can pay).
    pub has_wallet_key: bool,
    /// This device holds the root key (can add devices).
    pub has_root_key: bool,
    /// Chain reachable for this answer.
    pub online: bool,
}

/// Reads identity and devices from chain.
#[tauri::command]
pub async fn identity_status(state: S<'_>) -> CmdResult<IdentityStatus> {
    let info = state
        .session
        .read()
        .await
        .clone()
        .ok_or_else(UiError::locked)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let (did, dpk) = one.devices().this_device()?;
    let address = one.address().to_owned();
    let mut online = true;
    let rotation = match account::rotation_count_on_chain(&one.chain, &address).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "identity read failed");
            online = false;
            None
        }
    };
    let devices = if rotation.is_some() {
        one.devices().list().await.unwrap_or_default()
    } else {
        Vec::new()
    };
    let devices: Vec<DeviceInfo> = devices
        .into_iter()
        .map(|d| DeviceInfo {
            is_this_device: d.device_pubkey.eq_ignore_ascii_case(&dpk),
            device_id: d.device_id,
            label: d.label,
            platform: d.platform,
            revoked: d.revoked,
            pubkey_hex: d.device_pubkey,
        })
        .collect();
    let username = one.people().my_username().await.unwrap_or_default();
    let balance_uhash = if online {
        one.wallet().balance(None).await.ok().map(|b| b.uhash)
    } else {
        None
    };
    Ok(IdentityStatus {
        address,
        registered: rotation.is_some(),
        this_device_registered: devices.iter().any(|d| d.is_this_device && !d.revoked),
        devices,
        this_device_id: did,
        this_device_pubkey: dpk,
        rotation_count: rotation,
        username,
        balance_uhash,
        has_wallet_key: info.has_wallet_key,
        has_root_key: info.has_root_key,
        online,
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

/// Registers this account's identity (`MsgCreateIdentity`) or, when it
/// already exists, this PC as a device (`MsgAddDevice`). Only public keys
/// go on chain. Needs HASH for the fee.
#[tauri::command]
pub async fn identity_register(state: S<'_>, app: AppHandle, label: String) -> CmdResult<TxSubmitted> {
    let label = if label.trim().is_empty() {
        let s = state.settings.read().await.device_label.clone();
        if s.trim().is_empty() {
            std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into())
        } else {
            s
        }
    } else {
        label.trim().to_owned()
    };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let address = one.address().to_owned();
    let existing = account::rotation_count_on_chain(&one.chain, &address).await?;
    let (r, summary) = match existing {
        None => {
            let r = account::create_identity_on_chain(&one.account, &one.network, &one.chain, &label, "windows")
                .await?;
            (r, format!("Create identity with device \"{label}\""))
        }
        Some(_) => {
            let (did, pk) = one.devices().this_device()?;
            let hash = one.devices().add_on_chain(&did, &pk, &label, "windows").await?;
            (
                hashgram_sdk::chain::TxResult {
                    txhash: hash,
                    code: 0,
                    height: 0,
                    gas_used: 0,
                    raw_log: String::new(),
                },
                format!("Add device \"{label}\""),
            )
        }
    };
    drop(g);
    crate::cmd_wallet::track_tx(&state, &app, &r.txhash, &summary, r.height).await;
    Ok(TxSubmitted {
        hash: r.txhash,
        summary,
    })
}

/// Our devices on chain.
#[tauri::command]
pub async fn devices_list(state: S<'_>) -> CmdResult<Vec<DeviceInfo>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let (_, dpk) = one.devices().this_device()?;
    Ok(one
        .devices()
        .list()
        .await?
        .into_iter()
        .map(|d| DeviceInfo {
            is_this_device: d.device_pubkey.eq_ignore_ascii_case(&dpk),
            device_id: d.device_id,
            label: d.label,
            platform: d.platform,
            revoked: d.revoked,
            pubkey_hex: d.device_pubkey,
        })
        .collect())
}

/// This device's id and public key (to paste into the root device).
#[tauri::command]
pub async fn this_device(state: S<'_>) -> CmdResult<(String, String)> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.devices().this_device()?)
}

/// Adds another device to our identity (needs the root key here).
#[tauri::command]
pub async fn device_add(
    state: S<'_>,
    app: AppHandle,
    device_id: String,
    pubkey_hex: String,
    label: String,
) -> CmdResult<TxSubmitted> {
    let did = device_id.trim();
    if did.is_empty() || did.len() > 64 {
        return Err(UiError::invalid("device id length"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let hash = one
        .devices()
        .add_on_chain(did, pubkey_hex.trim(), label.trim(), "windows")
        .await?;
    // Give the new device what it cannot derive as soon as it can decrypt.
    let _ = one.devices().bootstrap_new_device().await;
    one.save()?;
    drop(g);
    let summary = format!("Add device \"{}\"", label.trim());
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Revokes a device on chain (wallet key) and removes it from every group.
#[tauri::command]
pub async fn device_revoke(state: S<'_>, app: AppHandle, device_id: String) -> CmdResult<TxSubmitted> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    if one.account.contents.device_id == device_id {
        return Err(UiError::invalid("revoke this PC from another device, not from itself"));
    }
    let hash = one.devices().revoke_on_chain(&device_id).await?;
    one.save()?;
    drop(g);
    let summary = format!("Revoke device \"{device_id}\"");
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Reconciles MLS groups with the chain's device registry now.
#[tauri::command]
pub async fn devices_reconcile(state: S<'_>) -> CmdResult<hashgram_sdk::devices::ReconcileReport> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let r = one.devices().reconcile().await?;
    one.save()?;
    Ok(r)
}

/// Sends the Drive keyring and contacts to our other devices now.
#[tauri::command]
pub async fn devices_bootstrap(state: S<'_>) -> CmdResult<bool> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let r = one.devices().bootstrap_new_device().await?;
    one.save()?;
    Ok(r)
}

/// KDF used by this profile, for the About page.
#[must_use]
pub fn kdf_label() -> String {
    let k = session::kdf();
    if k.m_cost == KdfCost::light().m_cost {
        "Argon2id (light, DEVNET/test profile)".into()
    } else {
        format!("Argon2id, {} MiB", k.m_cost / 1024)
    }
}
