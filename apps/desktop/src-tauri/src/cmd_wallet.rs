//! Wallet commands over `one.wallet()` and the generic transaction intents
//! in `tx.rs` (staking, governance, usernames, recovery).

use std::sync::Arc;

use hashgram_sdk::wallet::{format_hash, parse_amount, Balance};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::db::PendingRow;
use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::tx::MsgSpec;

type S<'a> = State<'a, Arc<AppState>>;

/// Balance with the verification note.
#[tauri::command]
pub async fn wallet_balance(state: S<'_>, address: Option<String>) -> CmdResult<Balance> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.wallet().balance(address.as_deref()).await?)
}

/// The Wallet home: balance, account facts, vesting, username.
#[derive(Debug, Clone, Serialize)]
pub struct WalletOverview {
    /// Address.
    pub address: String,
    /// Balance.
    pub balance: Balance,
    /// Account exists on chain (has ever received funds).
    pub account_exists: bool,
    /// Account number.
    pub account_number: Option<u64>,
    /// Sequence.
    pub sequence: Option<u64>,
    /// Vesting account type, if any.
    pub vesting_type: Option<String>,
    /// Original vesting in uhash.
    pub original_vesting_uhash: Option<String>,
    /// Vesting end (unix s).
    pub vesting_end: Option<i64>,
    /// Vesting start.
    pub vesting_start: Option<i64>,
    /// Our username.
    pub username: String,
    /// This device can sign.
    pub can_sign: bool,
}

fn s_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}
fn s_i64(v: &serde_json::Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Overview.
#[tauri::command]
pub async fn wallet_overview(state: S<'_>) -> CmdResult<WalletOverview> {
    let can_sign = state
        .session
        .read()
        .await
        .as_ref()
        .map(|s| s.has_wallet_key)
        .unwrap_or(false);
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let address = one.address().to_owned();
    let balance = one.wallet().balance(None).await?;
    let acct = one
        .chain
        .query(&format!("cosmos/auth/v1beta1/accounts/{address}"))
        .await;
    let (account_exists, account_number, sequence, vesting_type, original_vesting, vesting_end, vesting_start) =
        match acct {
            Ok(v) => {
                let a = v.get("account").cloned().unwrap_or_default();
                let ty = a.get("@type").and_then(|t| t.as_str()).unwrap_or("").to_owned();
                let base = a
                    .get("base_vesting_account")
                    .and_then(|v| v.get("base_account"))
                    .or_else(|| a.get("base_account"))
                    .unwrap_or(&a);
                let bva = a.get("base_vesting_account");
                let ov = bva
                    .and_then(|v| v.get("original_vesting"))
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.iter().find(|c| c.get("denom").and_then(|d| d.as_str()) == Some("uhash")))
                    .and_then(|c| c.get("amount"))
                    .and_then(|x| x.as_str())
                    .map(str::to_owned);
                (
                    true,
                    base.get("account_number").and_then(s_u64),
                    base.get("sequence").and_then(s_u64),
                    if ty.contains("Vesting") { Some(ty) } else { None },
                    ov,
                    bva.and_then(|v| v.get("end_time")).and_then(s_i64),
                    a.get("start_time").and_then(s_i64),
                )
            }
            Err(hashgram_sdk::chain::ClientError::Gateway { status: 404, .. })
            | Err(hashgram_sdk::chain::ClientError::NoAccount(_)) => (false, None, None, None, None, None, None),
            Err(e) => {
                let s = e.to_string();
                if s.contains("not found") || s.contains("404") {
                    (false, None, None, None, None, None, None)
                } else {
                    return Err(hashgram_sdk::SdkError::from(e).into());
                }
            }
        };
    let username = one.people().my_username().await.unwrap_or_default();
    Ok(WalletOverview {
        address,
        balance,
        account_exists,
        account_number,
        sequence,
        vesting_type,
        original_vesting_uhash: original_vesting,
        vesting_end,
        vesting_start,
        username,
        can_sign,
    })
}

/// A fee preview.
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
    /// Fee formatted.
    pub fee_display: String,
    /// Fee came from simulation (true) or estimate (false).
    pub simulated: bool,
}

/// A submitted transaction.
#[derive(Debug, Clone, Serialize)]
pub struct TxSubmitted {
    /// Hex hash.
    pub hash: String,
    /// Summary.
    pub summary: String,
}

/// Records a submitted transaction and starts watching it.
pub async fn track_tx(state: &Arc<AppState>, app: &AppHandle, hash: &str, summary: &str, height: u64) {
    let st = if height > 0 { "committed" } else { "pending" };
    let _ = state.db.pending_put(hash, summary, st);
    if st == "pending" {
        state.pending_tx.lock().await.push(hash.to_owned());
    } else {
        let _ = state.db.pending_update(hash, st, height, "");
    }
    let _ = app.emit("tx:update", serde_json::json!({ "hash": hash, "state": st, "height": height }));
}

/// Polls pending transactions once; called by the sync loop.
pub async fn poll_pending(state: &Arc<AppState>, app: &AppHandle) {
    let hashes: Vec<String> = state.pending_tx.lock().await.clone();
    if hashes.is_empty() {
        return;
    }
    let mut g = state.one.lock().await;
    let Some(one) = g.as_mut() else { return };
    for h in hashes {
        match one.chain.tx(&h).await {
            Ok(Some(t)) => {
                let st = if t.code == 0 { "committed" } else { "failed" };
                let _ = state.db.pending_update(&h, st, t.height, &t.raw_log);
                state.pending_tx.lock().await.retain(|x| x != &h);
                let _ = app.emit(
                    "tx:update",
                    serde_json::json!({ "hash": h, "state": st, "height": t.height, "raw_log": t.raw_log }),
                );
            }
            Ok(None) => {}
            Err(e) => tracing::debug!(error = %e, "tx poll"),
        }
    }
}

/// Previews any transaction intent.
#[tauri::command]
pub async fn tx_preview(state: S<'_>, spec: MsgSpec) -> CmdResult<TxPreview> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let wallet = one.account.wallet()?;
    let built = spec.build(one.address()).map_err(UiError::invalid)?;
    let fee = one.chain.fee_preview(&wallet, built.msgs, "").await.map_err(hashgram_sdk::SdkError::from)?;
    Ok(TxPreview {
        summary: built.summary,
        warnings: built.warnings,
        gas_limit: fee.gas_limit,
        fee_uhash: fee.fee_uhash.to_string(),
        fee_display: format_hash(fee.fee_uhash),
        simulated: fee.simulated,
    })
}

/// Signs and broadcasts any transaction intent; inclusion is tracked in the
/// background (`tx:update` events).
#[tauri::command]
pub async fn tx_submit(state: S<'_>, app: AppHandle, spec: MsgSpec, memo: String) -> CmdResult<TxSubmitted> {
    if memo.len() > 256 {
        return Err(UiError::invalid("memo is too long"));
    }
    let (hash, summary) = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let wallet = one.account.wallet()?;
        let built = spec.build(one.address()).map_err(UiError::invalid)?;
        let r = one
            .chain
            .sign_and_broadcast_nowait(&wallet, built.msgs, &memo)
            .await
            .map_err(hashgram_sdk::SdkError::from)?;
        (r.txhash, built.summary)
    };
    track_tx(&state, &app, &hash, &summary, 0).await;
    Ok(TxSubmitted { hash, summary })
}

/// Recently submitted transactions and their state.
#[tauri::command]
pub async fn tx_recent(state: S<'_>) -> CmdResult<Vec<PendingRow>> {
    state.db.pending_list(50).map_err(UiError::internal)
}

/// Whether a transaction is pending (the updater must not run then).
#[tauri::command]
pub async fn tx_has_pending(state: S<'_>) -> CmdResult<bool> {
    Ok(!state.pending_tx.lock().await.is_empty())
}

/// Parses a typed amount (`"1.5"`, `"1.5 HASH"`, `"1500000uhash"`).
#[tauri::command]
pub fn wallet_parse_amount(amount: String) -> CmdResult<String> {
    Ok(parse_amount(&amount)?.to_string())
}

/// Fee preview for a send.
#[tauri::command]
pub async fn wallet_preview_send(state: S<'_>, to: String, amount: String) -> CmdResult<TxPreview> {
    let uhash = parse_amount(&amount)?;
    if uhash == 0 {
        return Err(UiError::invalid("amount must be more than 0"));
    }
    tx_preview(
        state,
        MsgSpec::Send {
            to: to.trim().to_owned(),
            amount_uhash: uhash.to_string(),
        },
    )
    .await
}

/// Sends HASH.
#[tauri::command]
pub async fn wallet_send(state: S<'_>, app: AppHandle, to: String, amount: String, memo: String) -> CmdResult<TxSubmitted> {
    let uhash = parse_amount(&amount)?;
    if uhash == 0 {
        return Err(UiError::invalid("amount must be more than 0"));
    }
    tx_submit(
        state,
        app,
        MsgSpec::Send {
            to: to.trim().to_owned(),
            amount_uhash: uhash.to_string(),
        },
        memo,
    )
    .await
}

/// Delegates.
#[tauri::command]
pub async fn wallet_stake(state: S<'_>, app: AppHandle, validator: String, amount: String) -> CmdResult<TxSubmitted> {
    let uhash = parse_amount(&amount)?;
    tx_submit(
        state,
        app,
        MsgSpec::Delegate {
            validator: validator.trim().to_owned(),
            amount_uhash: uhash.to_string(),
        },
        "hashgram one: stake".into(),
    )
    .await
}

/// Undelegates.
#[tauri::command]
pub async fn wallet_unstake(state: S<'_>, app: AppHandle, validator: String, amount: String) -> CmdResult<TxSubmitted> {
    let uhash = parse_amount(&amount)?;
    tx_submit(
        state,
        app,
        MsgSpec::Undelegate {
            validator: validator.trim().to_owned(),
            amount_uhash: uhash.to_string(),
        },
        "hashgram one: unstake".into(),
    )
    .await
}

/// Withdraws rewards.
#[tauri::command]
pub async fn wallet_withdraw_rewards(state: S<'_>, app: AppHandle, validator: String) -> CmdResult<TxSubmitted> {
    tx_submit(
        state,
        app,
        MsgSpec::WithdrawRewards {
            validator: validator.trim().to_owned(),
        },
        "hashgram one: rewards".into(),
    )
    .await
}

/// Username availability with the chain's reason code
/// (`taken`, `reserved`, `confusable_with`, `invalid`, `too_short`,
/// `too_long`, `mixed_script`, `non_ascii_not_allowed`).
#[derive(Debug, Clone, Serialize)]
pub struct UsernameAvailability {
    /// Available.
    pub available: bool,
    /// Normalised form.
    pub normalized: String,
    /// Reason when not available.
    pub reason: String,
    /// Conflicting name for `confusable_with`.
    pub conflicting_name: String,
}

/// Live availability check.
#[tauri::command]
pub async fn wallet_username_availability(state: S<'_>, name: String) -> CmdResult<UsernameAvailability> {
    let n = name.trim().trim_start_matches('@').to_lowercase();
    if n.is_empty() {
        return Ok(UsernameAvailability {
            available: false,
            normalized: String::new(),
            reason: "too_short".into(),
            conflicting_name: String::new(),
        });
    }
    if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Ok(UsernameAvailability {
            available: false,
            normalized: n,
            reason: "invalid".into(),
            conflicting_name: String::new(),
        });
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let v = one.wallet().username_availability(&n).await?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_owned();
    Ok(UsernameAvailability {
        available: v.get("available").and_then(|a| a.as_bool()).unwrap_or(false),
        normalized: s("normalized"),
        reason: s("reason"),
        conflicting_name: s("conflicting_name"),
    })
}

/// Registers a username (1 HASH).
#[tauri::command]
pub async fn wallet_register_username(state: S<'_>, app: AppHandle, name: String) -> CmdResult<TxSubmitted> {
    tx_submit(state, app, MsgSpec::RegisterUsername { name }, "hashgram one: username".into()).await
}

/// Renews a username.
#[tauri::command]
pub async fn wallet_renew_username(state: S<'_>, app: AppHandle, name: String) -> CmdResult<TxSubmitted> {
    tx_submit(state, app, MsgSpec::RenewUsername { name }, "hashgram one: renew".into()).await
}

/// Our delegations (raw staking module answer).
#[tauri::command]
pub async fn wallet_delegations(state: S<'_>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.wallet().delegations().await?)
}

/// Our staking rewards (distribution module).
#[tauri::command]
pub async fn wallet_rewards(state: S<'_>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let me = one.address().to_owned();
    Ok(one
        .chain
        .query(&format!("cosmos/distribution/v1beta1/delegators/{me}/rewards"))
        .await
        .map_err(hashgram_sdk::SdkError::from)?)
}

/// Unbonding delegations.
#[tauri::command]
pub async fn wallet_unbonding(state: S<'_>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let me = one.address().to_owned();
    Ok(one
        .chain
        .query(&format!("cosmos/staking/v1beta1/delegators/{me}/unbonding_delegations"))
        .await
        .map_err(hashgram_sdk::SdkError::from)?)
}

/// Transaction history (recipient side, through the gateway's tx search).
#[tauri::command]
pub async fn wallet_history(state: S<'_>, limit: Option<u32>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.wallet().history(limit.unwrap_or(50).clamp(1, 200)).await?)
}

/// Transaction by hash.
#[tauri::command]
pub async fn wallet_tx(state: S<'_>, hash: String) -> CmdResult<Option<serde_json::Value>> {
    let h = hash.trim().to_ascii_uppercase();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UiError::invalid("a transaction hash is 64 hex characters"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.wallet().tx(&h).await?.map(|t| {
        serde_json::json!({
            "txhash": t.txhash, "code": t.code, "height": t.height,
            "gas_used": t.gas_used, "raw_log": t.raw_log,
        })
    }))
}

/// Our usernames with expiry (reverse lookup, raw registrations).
#[tauri::command]
pub async fn wallet_usernames(state: S<'_>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let me = one.address().to_owned();
    Ok(one
        .chain
        .query(&format!("hashgram/username/v1/reverse/{me}"))
        .await
        .map_err(hashgram_sdk::SdkError::from)?)
}

/// Any allow-listed chain read (governance proposals, params, …) for
/// screens that render raw module answers. The relay allow-list is the
/// boundary; this is a read, never a write.
#[tauri::command]
pub async fn chain_query(state: S<'_>, path: String) -> CmdResult<serde_json::Value> {
    if path.contains("..") || path.starts_with('/') || path.len() > 512 {
        return Err(UiError::invalid("path"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.chain.query(&path).await.map_err(hashgram_sdk::SdkError::from)?)
}

/// A QR code as an SVG string (receive screen).
#[tauri::command]
pub fn qr_svg(text: String) -> CmdResult<String> {
    if text.len() > 512 {
        return Err(UiError::invalid("too long for a QR code"));
    }
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| UiError::invalid(e.to_string()))?;
    let w = code.width();
    let mut path = String::new();
    for y in 0..w {
        for x in 0..w {
            if code[(x, y)] == qrcode::Color::Dark {
                path.push_str(&format!("M{x} {y}h1v1h-1z"));
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {w}\" shape-rendering=\"crispEdges\"><rect width=\"{w}\" height=\"{w}\" fill=\"#ffffff\"/><path d=\"{path}\" fill=\"#000000\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_renders_paths_only() {
        let svg = qr_svg("hash1abc".into()).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("<path"));
        assert!(!svg.contains("<script"));
    }
}
