//! Earn: the provider lifecycle over `one.provider()` and the node manager
//! from the previous app (`node_manager.rs`) for a node on this PC.

use std::sync::Arc;

use hashgram_sdk::provider::{Earnings, Lifecycle, ProviderStatus};
use hashgram_sdk::wallet::parse_amount;
use serde::Serialize;
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use crate::cmd_wallet::TxSubmitted;
use crate::error::{CmdResult, UiError};
use crate::node_manager::{self as nm, NodeSetup, Registration};
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

const OPERATOR_KEY_NAME: &str = "node_operator_key";

/// The lifecycle as one plain sentence.
#[must_use]
pub fn lifecycle_sentence(l: Lifecycle) -> &'static str {
    match l {
        Lifecycle::Unregistered => "Not registered as a provider. Run a node and register to earn from the storage and relay work it does.",
        Lifecycle::Registered => "Registered as a provider with a bond posted; the node earns as it serves receipts.",
        Lifecycle::WaitingForAssignment => "Registered as a storage provider; the network has not assigned data yet — this needs an assigner to be registered by governance.",
        Lifecycle::Active => "Active: earning credit in the current epoch from challenges answered and receipts served.",
        Lifecycle::Degraded => "Degraded: the fraud score is above zero. Rewards continue at a reduced weight until it decays.",
        Lifecycle::Jailed => "Jailed until the height shown. No rewards accrue; the node can be un-jailed after that height.",
        Lifecycle::Unbonding => "Unbonding: the bond is released after 21 days. The node no longer earns.",
        Lifecycle::Withdrawn => "The bond was withdrawn; the registration is closed.",
    }
}

/// Provider status with the sentence.
#[derive(Debug, Clone, Serialize)]
pub struct EarnStatus {
    /// Status.
    pub status: ProviderStatus,
    /// One-sentence explanation of the lifecycle.
    pub sentence: &'static str,
}

/// Our (or an operator's) provider status.
#[tauri::command]
pub async fn earn_status(state: S<'_>, operator: Option<String>) -> CmdResult<EarnStatus> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let status = one.provider().status(operator.as_deref()).await?;
    Ok(EarnStatus {
        sentence: lifecycle_sentence(status.lifecycle),
        status,
    })
}

/// Earnings.
#[tauri::command]
pub async fn earn_earnings(state: S<'_>, operator: Option<String>) -> CmdResult<Earnings> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.provider().earnings(operator.as_deref()).await?)
}

/// Every provider on the network.
#[tauri::command]
pub async fn earn_providers(state: S<'_>) -> CmdResult<Vec<ProviderStatus>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.provider().list().await?)
}

/// Registers this account as a provider. The reward address must differ
/// from the operator: the operator key lives on the node (hot) and signs
/// receipts, the reward address should be a cold key that only receives.
#[tauri::command]
pub async fn earn_register(
    state: S<'_>,
    app: AppHandle,
    reward_address: String,
    node_pubkey_hex: String,
    roles: Vec<String>,
    bond: String,
    storage_gib: u32,
    moniker: String,
) -> CmdResult<TxSubmitted> {
    let bond_uhash = parse_amount(&bond)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    if reward_address.trim() == one.address() {
        return Err(UiError::invalid(
            "the reward address must differ from the operator address: the operator key is hot (it signs receipts on the node); rewards should go to a key that only receives",
        ));
    }
    let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
    let hash = one
        .provider()
        .register(
            reward_address.trim(),
            node_pubkey_hex.trim(),
            &roles,
            bond_uhash,
            u64::from(storage_gib) * 1024 * 1024 * 1024,
            moniker.trim(),
        )
        .await?;
    drop(g);
    let summary = format!("Register provider \"{}\"", moniker.trim());
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Updates mutable provider fields.
#[tauri::command]
pub async fn earn_update(
    state: S<'_>,
    app: AppHandle,
    reward_address: String,
    roles: Vec<String>,
    storage_gib: u32,
    moniker: String,
    additional_bond: String,
) -> CmdResult<TxSubmitted> {
    let add = if additional_bond.trim().is_empty() {
        0
    } else {
        parse_amount(&additional_bond)?
    };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let roles: Vec<&str> = roles.iter().map(String::as_str).collect();
    let hash = one
        .provider()
        .update(
            reward_address.trim(),
            &roles,
            u64::from(storage_gib) * 1024 * 1024 * 1024,
            moniker.trim(),
            add,
        )
        .await?;
    drop(g);
    let summary = "Update provider".to_owned();
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Begins unbonding.
#[tauri::command]
pub async fn earn_unbond(state: S<'_>, app: AppHandle) -> CmdResult<TxSubmitted> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let hash = one.provider().unbond().await?;
    drop(g);
    let summary = "Begin unbonding provider bond".to_owned();
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Withdraws the bond.
#[tauri::command]
pub async fn earn_withdraw(state: S<'_>, app: AppHandle) -> CmdResult<TxSubmitted> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let hash = one.provider().withdraw().await?;
    drop(g);
    let summary = "Withdraw provider bond".to_owned();
    crate::cmd_wallet::track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

// ---------------------------------------------------------------------------
// Node on this PC
// ---------------------------------------------------------------------------

/// Everything the node card needs.
#[derive(Debug, Clone, Serialize)]
pub struct NodeOverview {
    /// The node binary is bundled.
    pub bundled: bool,
    /// Configuration written.
    pub configured: bool,
    /// Saved setup.
    pub setup: Option<NodeSetup>,
    /// How it is registered to run.
    pub registration: Registration,
    /// Running (local API answers).
    pub running: bool,
    /// Operator address (from the vault key), if created.
    pub operator: Option<String>,
    /// Operator balance in uhash (needs bond + fees).
    pub operator_balance_uhash: Option<String>,
    /// `/v1/status` of the node, raw.
    pub status: Option<serde_json::Value>,
    /// `/v1/rewards` of the node, raw.
    pub rewards: Option<serde_json::Value>,
    /// Provider status of the operator, from chain.
    pub provider: Option<EarnStatus>,
    /// autonat verdict from the node (`public`/`private`/`unknown`).
    pub reachability: String,
    /// The app is elevated (a real service can be created).
    pub elevated: bool,
    /// Loopback gateway URL the node uses for the chain.
    pub chain_gateway: String,
    /// Node binary path, when found.
    pub binary: Option<String>,
    /// Node peer id / public key, when the binary answers.
    pub node_id: Option<String>,
}

async fn operator_secret(state: &AppState) -> CmdResult<Option<Zeroizing<String>>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .account
        .contents
        .extra
        .get(OPERATOR_KEY_NAME)
        .map(|s| Zeroizing::new(s.clone())))
}

async fn ensure_operator_secret(state: &AppState) -> CmdResult<Zeroizing<String>> {
    if let Some(s) = operator_secret(state).await? {
        return Ok(s);
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let (_, w) = hashgram_sdk::Wallet::generate().map_err(|e| UiError::internal(e.to_string()))?;
    let hex = Zeroizing::new(hex::encode(w.secret_bytes()));
    one.account
        .contents
        .extra
        .insert(OPERATOR_KEY_NAME.to_owned(), hex.to_string());
    one.save()?;
    Ok(hex)
}

fn operator_address(secret_hex: &str) -> CmdResult<String> {
    let bytes = hex::decode(secret_hex.trim()).map_err(|e| UiError::internal(e.to_string()))?;
    Ok(hashgram_sdk::Wallet::from_secret(&bytes)
        .map_err(|e| UiError::internal(e.to_string()))?
        .address()
        .to_string())
}

/// The node overview.
#[tauri::command]
pub async fn node_overview(state: S<'_>) -> CmdResult<NodeOverview> {
    let bin = nm::node_binary();
    let setup = nm::read_setup();
    let operator = match operator_secret(&state).await? {
        Some(s) => operator_address(&s).ok(),
        None => None,
    };
    let registration = tauri::async_runtime::spawn_blocking(nm::registration)
        .await
        .unwrap_or(Registration::None);
    let elevated = tauri::async_runtime::spawn_blocking(nm::is_elevated)
        .await
        .unwrap_or(false);
    let status = nm::api_get("v1/status").await.ok();
    let rewards = if status.is_some() {
        nm::api_get("v1/rewards").await.ok()
    } else {
        None
    };
    let reachability = status
        .as_ref()
        .and_then(|s| s.get("swarm"))
        .and_then(|s| s.get("reachability"))
        .and_then(|r| r.as_str())
        .unwrap_or("unknown")
        .to_owned();
    let node_id = if nm::config_path().exists() {
        tauri::async_runtime::spawn_blocking(nm::node_id).await.ok().and_then(Result::ok)
    } else {
        None
    };
    let (provider, balance) = match &operator {
        Some(op) => {
            let mut g = state.one.lock().await;
            match g.as_mut() {
                Some(one) => {
                    let p = one.provider().status(Some(op)).await.ok().map(|status| EarnStatus {
                        sentence: lifecycle_sentence(status.lifecycle),
                        status,
                    });
                    let b = one.wallet().balance(Some(op)).await.ok().map(|b| b.uhash);
                    (p, b)
                }
                None => (None, None),
            }
        }
        None => (None, None),
    };
    Ok(NodeOverview {
        bundled: bin.is_some(),
        configured: nm::config_path().exists(),
        setup,
        registration,
        running: status.is_some(),
        operator,
        operator_balance_uhash: balance,
        status,
        rewards,
        provider,
        reachability,
        elevated,
        chain_gateway: crate::chain_proxy::url(),
        binary: bin.map(|b| b.display().to_string()),
        node_id,
    })
}

/// Writes the node configuration (creating the operator key in the vault).
#[tauri::command]
pub async fn node_configure(state: S<'_>, setup: NodeSetup) -> CmdResult<String> {
    let settings = state.settings.read().await.clone();
    let identity = crate::session::network_identity(&settings)?;
    let secret = ensure_operator_secret(&state).await?;
    nm::write_config(&setup, &identity, &secret).map_err(UiError::invalid)?;
    operator_address(&secret)
}

/// Registers the node to run at logon (or as a service when elevated).
#[tauri::command]
pub async fn node_install() -> CmdResult<Registration> {
    tauri::async_runtime::spawn_blocking(nm::install)
        .await
        .map_err(|e| UiError::internal(e.to_string()))?
        .map_err(UiError::internal)
}

/// Starts the node.
#[tauri::command]
pub async fn node_start() -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(nm::start)
        .await
        .map_err(|e| UiError::internal(e.to_string()))?
        .map_err(UiError::internal)
}

/// Stops the node.
#[tauri::command]
pub async fn node_stop() -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(nm::stop)
        .await
        .map_err(|e| UiError::internal(e.to_string()))?
        .map_err(UiError::internal)
}

/// Removes the registration (keeps data and keys).
#[tauri::command]
pub async fn node_uninstall() -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(nm::uninstall)
        .await
        .map_err(|e| UiError::internal(e.to_string()))?
        .map_err(UiError::internal)
}

/// A cold reward address, generated on this PC and shown once. Nothing is
/// stored: the user writes the 24 words down.
#[derive(Debug, Serialize)]
pub struct ColdAddress {
    /// The words (shown once).
    pub words: Vec<String>,
    /// The address they derive.
    pub address: String,
}

/// Generates a cold reward address.
#[tauri::command]
pub fn node_generate_cold_address() -> CmdResult<ColdAddress> {
    let (mnemonic, w) = hashgram_sdk::Wallet::generate().map_err(|e| UiError::internal(e.to_string()))?;
    Ok(ColdAddress {
        words: mnemonic.split_whitespace().map(str::to_owned).collect(),
        address: w.address().to_string(),
    })
}

/// Node log tail.
#[tauri::command]
pub fn node_log_tail(lines: Option<usize>) -> CmdResult<String> {
    let p = nm::node_home().join("node.log");
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let n = lines.unwrap_or(200).min(2000);
    let tail: Vec<&str> = text.lines().rev().take(n).collect();
    Ok(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lifecycle_has_a_sentence_without_the_forbidden_word() {
        for l in [
            Lifecycle::Unregistered,
            Lifecycle::Registered,
            Lifecycle::WaitingForAssignment,
            Lifecycle::Active,
            Lifecycle::Degraded,
            Lifecycle::Jailed,
            Lifecycle::Unbonding,
            Lifecycle::Withdrawn,
        ] {
            let s = lifecycle_sentence(l);
            assert!(s.len() > 20);
            assert!(!s.to_ascii_lowercase().contains("mining"));
        }
        assert!(lifecycle_sentence(Lifecycle::WaitingForAssignment).contains("assigner"));
    }
}
