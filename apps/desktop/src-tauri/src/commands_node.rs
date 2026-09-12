//! Stage 3 commands: Earn → Run a node on this PC.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use zeroize::Zeroizing;

use crate::node_manager::{self as nm, NodeSetup, Registration};
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

const OPERATOR_KEY_NAME: &str = "node_operator_key";

/// Everything the Earn screen needs about the local node.
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
    /// Provider record on chain, raw.
    pub provider: Option<serde_json::Value>,
    /// Assignments on chain.
    pub assignments: Option<serde_json::Value>,
    /// Challenges on chain.
    pub challenges: Option<serde_json::Value>,
    /// Fraud score on chain.
    pub fraud: Option<serde_json::Value>,
    /// Rewards on chain for the operator.
    pub chain_rewards: Option<serde_json::Value>,
    /// autonat verdict from the node (`public`/`private`/`unknown`).
    pub reachability: String,
    /// The app is elevated (a real service can be created).
    pub elevated: bool,
    /// Loopback gateway URL the node uses for the chain.
    pub chain_gateway: String,
    /// Node binary path, when found.
    pub binary: Option<String>,
}

async fn operator_secret(state: &AppState) -> Result<Option<Zeroizing<String>>, String> {
    let (account, _, _) = state.session_handles().await?;
    let a = account.lock().await;
    Ok(a.contents
        .extra
        .get(OPERATOR_KEY_NAME)
        .map(|s| Zeroizing::new(s.clone())))
}

async fn ensure_operator_secret(state: &AppState) -> Result<Zeroizing<String>, String> {
    if let Some(s) = operator_secret(state).await? {
        return Ok(s);
    }
    let (account, _, _) = state.session_handles().await?;
    let (_, w) = hashgram_sdk::Wallet::generate().map_err(|e| e.to_string())?;
    let hex = Zeroizing::new(hex::encode(w.secret_bytes()));
    let mut a = account.lock().await;
    a.contents
        .extra
        .insert(OPERATOR_KEY_NAME.to_owned(), hex.to_string());
    a.save().map_err(|e| e.to_string())?;
    Ok(hex)
}

fn operator_address(secret_hex: &str) -> Result<String, String> {
    let bytes = hex::decode(secret_hex.trim()).map_err(|e| e.to_string())?;
    Ok(hashgram_sdk::Wallet::from_secret(&bytes)
        .map_err(|e| e.to_string())?
        .address()
        .to_string())
}

async fn chain_read_opt(state: &AppState, path: &str) -> Option<serde_json::Value> {
    let s = state.settings.read().await.clone();
    let link = state.net.link().await;
    state
        .chain
        .get(
            link,
            &s.network.local_node_api,
            &s.network.https_endpoints,
            path,
        )
        .await
        .ok()
        .map(|r| r.value)
}

/// The node overview.
#[tauri::command]
pub async fn node_overview(state: S<'_>) -> Result<NodeOverview, String> {
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
    let (provider, assignments, challenges, fraud, chain_rewards, balance) = match &operator {
        Some(op) => (
            chain_read_opt(&state, &format!("hashgram/serviceproof/v1/provider/{op}")).await,
            chain_read_opt(
                &state,
                &format!("hashgram/serviceproof/v1/assignments/{op}"),
            )
            .await,
            chain_read_opt(&state, &format!("hashgram/serviceproof/v1/challenges/{op}")).await,
            chain_read_opt(&state, &format!("hashgram/serviceproof/v1/fraud/{op}")).await,
            chain_read_opt(&state, &format!("hashgram/serviceproof/v1/rewards/{op}")).await,
            chain_read_opt(
                &state,
                &format!("cosmos/bank/v1beta1/balances/{op}/by_denom?denom=uhash"),
            )
            .await
            .and_then(|v| {
                v.get("balance")
                    .and_then(|b| b.get("amount"))
                    .and_then(|a| a.as_str())
                    .map(str::to_owned)
            }),
        ),
        None => (None, None, None, None, None, None),
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
        assignments,
        challenges,
        fraud,
        chain_rewards,
        reachability,
        elevated,
        chain_gateway: crate::chain_proxy::url(),
        binary: bin.map(|b| b.display().to_string()),
    })
}

/// Writes the node configuration (creating the operator key in the vault).
#[tauri::command]
pub async fn node_configure(state: S<'_>, setup: NodeSetup) -> Result<String, String> {
    let identity = state
        .net
        .identity()
        .await
        .ok_or_else(|| "network not started".to_owned())?;
    let secret = ensure_operator_secret(&state).await?;
    nm::write_config(&setup, &identity, &secret)?;
    operator_address(&secret)
}

/// Registers the node to run at logon (or as a service when elevated).
#[tauri::command]
pub async fn node_install() -> Result<Registration, String> {
    tauri::async_runtime::spawn_blocking(nm::install)
        .await
        .map_err(|e| e.to_string())?
}

/// Starts the node.
#[tauri::command]
pub async fn node_start() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(nm::start)
        .await
        .map_err(|e| e.to_string())?
}

/// Stops the node.
#[tauri::command]
pub async fn node_stop() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(nm::stop)
        .await
        .map_err(|e| e.to_string())?
}

/// Removes the registration (keeps data and keys).
#[tauri::command]
pub async fn node_uninstall() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(nm::uninstall)
        .await
        .map_err(|e| e.to_string())?
}

/// A cold reward address, generated on this PC and shown once. Nothing is
/// stored: the user writes the 24 words down; the app keeps only the
/// address in the node configuration.
#[derive(Debug, Serialize)]
pub struct ColdAddress {
    /// The words (shown once).
    pub words: Vec<String>,
    /// The address they derive.
    pub address: String,
}

/// Generates a cold reward address.
#[tauri::command]
pub fn node_generate_cold_address() -> Result<ColdAddress, String> {
    let (mnemonic, w) = hashgram_sdk::Wallet::generate().map_err(|e| e.to_string())?;
    Ok(ColdAddress {
        words: mnemonic.split_whitespace().map(str::to_owned).collect(),
        address: w.address().to_string(),
    })
}

/// Node log tail (its stderr goes to the task's console; we read the
/// data dir log if present).
#[tauri::command]
pub fn node_log_tail(lines: Option<usize>) -> Result<String, String> {
    let p = nm::node_home().join("node.log");
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let n = lines.unwrap_or(200);
    let tail: Vec<&str> = text.lines().rev().take(n).collect();
    Ok(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
}
