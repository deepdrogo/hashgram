//! Network commands: peers, chain height, supply, validators, indexer
//! leaderboards, reconnect, diagnostics.

use std::sync::Arc;

use hashgram_sdk::network::{Overview, PeerView};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::error::{CmdResult, UiError};
use crate::session;
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

/// The Network page. Works locked too (peers from the link), with chain
/// facts only when unlocked.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkOverview {
    /// Network id.
    pub network_id: String,
    /// Chain id.
    pub chain_id: String,
    /// Genesis hash.
    pub genesis_hash: String,
    /// Verified peers.
    pub peers: Vec<PeerView>,
    /// Rejected peers (peer, reason).
    pub rejected: Vec<(String, String)>,
    /// Chain height, when readable.
    pub height: Option<u64>,
    /// Verification summary of chain reads.
    pub verification: Option<String>,
    /// Distinct operators among verified peers.
    pub operators: usize,
    /// Store peers.
    pub store_peers: usize,
    /// Relay peers.
    pub relay_peers: usize,
    /// Link up.
    pub link_up: bool,
    /// Why the link is down.
    pub link_error: Option<String>,
    /// Indexer configured.
    pub indexer_configured: bool,
}

fn label_reason(reason: &str) -> String {
    let r = reason.to_ascii_lowercase();
    if r.contains("genesis") {
        "wrong network (different genesis)".into()
    } else if r.contains("chain") {
        "wrong chain id".into()
    } else if r.contains("protocol") || r.contains("version") {
        "incompatible protocol version".into()
    } else if r.contains("timeout") || r.contains("timed out") {
        "handshake timed out".into()
    } else {
        reason.to_owned()
    }
}

/// Overview.
#[tauri::command]
pub async fn network_overview(state: S<'_>) -> CmdResult<NetworkOverview> {
    let settings = state.settings.read().await.clone();
    let identity = session::network_identity(&settings)?;
    let indexer_configured = settings.indexer().is_some();
    let link = state.link.read().await.clone();
    let link_error = state.link_error.read().await.clone();
    let (peers, rejected) = match &link {
        Some(l) => (
            l.peers_ranked()
                .await
                .into_iter()
                .map(|p| PeerView {
                    peer: p.peer.peer.to_string(),
                    roles: p.peer.roles,
                    operator: p.peer.operator,
                    rtt_ms: p.rtt_ms,
                })
                .collect::<Vec<_>>(),
            l.rejected()
                .await
                .into_iter()
                .map(|r| (r.peer.to_string(), label_reason(&r.reason)))
                .collect::<Vec<_>>(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let (height, verification) = {
        let mut g = state.one.lock().await;
        match g.as_mut() {
            Some(one) => {
                let o: Option<Overview> = one.network_api().overview().await.ok();
                (o.as_ref().and_then(|o| o.height), o.and_then(|o| o.verification))
            }
            None => (None, None),
        }
    };
    let operators: std::collections::BTreeSet<&str> = peers
        .iter()
        .map(|p| p.operator.as_str())
        .filter(|o| !o.is_empty())
        .collect();
    Ok(NetworkOverview {
        network_id: identity.network_id.clone(),
        chain_id: identity.chain_id.clone(),
        genesis_hash: identity.genesis_hash.clone(),
        store_peers: peers.iter().filter(|p| p.roles.iter().any(|r| r == "store")).count(),
        relay_peers: peers
            .iter()
            .filter(|p| p.roles.iter().any(|r| r == "relay" || r == "bootstrap"))
            .count(),
        operators: operators.len(),
        peers,
        rejected,
        height,
        verification,
        link_up: link.is_some(),
        link_error,
        indexer_configured,
    })
}

/// Validators from the chain, sorted by tokens.
#[tauri::command]
pub async fn network_validators(state: S<'_>) -> CmdResult<Vec<serde_json::Value>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.network_api().validators().await?)
}

/// Supply, service reserve and founder revenue.
#[tauri::command]
pub async fn network_supply(state: S<'_>) -> CmdResult<serde_json::Value> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.network_api().supply().await?)
}

/// A leaderboard from the configured indexer: `holders` | `validators` |
/// `providers` | `earners`. `None` when no indexer is configured.
#[tauri::command]
pub async fn network_top(state: S<'_>, what: String, limit: Option<u32>) -> CmdResult<Option<serde_json::Value>> {
    let indexer = state.settings.read().await.indexer().map(str::to_owned);
    let Some(base) = indexer else { return Ok(None) };
    let limit = limit.unwrap_or(25).clamp(1, 100);
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut n = one.network_api();
    Ok(match what.as_str() {
        "holders" => n.top_holders(Some(&base), limit).await?,
        "validators" => n.top_validators(Some(&base), limit).await?,
        "providers" => n.top_providers(Some(&base), limit).await?,
        "earners" => {
            n.indexer(Some(&base), &format!("/v1/leaderboards/earners?limit={limit}"))
                .await?
        }
        other => return Err(UiError::invalid(format!("unknown leaderboard {other}"))),
    })
}

/// Network stats from the indexer.
#[tauri::command]
pub async fn network_stats(state: S<'_>) -> CmdResult<Option<serde_json::Value>> {
    let indexer = state.settings.read().await.indexer().map(str::to_owned);
    let Some(base) = indexer else { return Ok(None) };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.network_api().stats(Some(&base)).await?)
}

/// Reconnects without forgetting peers.
#[tauri::command]
pub async fn net_reconnect(state: S<'_>, app: AppHandle) -> CmdResult<()> {
    session::restart_link(&app, &state, false).await;
    Ok(())
}

/// Forgets every remembered peer and reconnects from the built-in list.
#[tauri::command]
pub async fn net_forget_peers(state: S<'_>, app: AppHandle) -> CmdResult<()> {
    session::restart_link(&app, &state, true).await;
    Ok(())
}

/// A diagnostics report: no IP addresses, no addresses of contacts, no
/// subjects. Peer ids and roles only.
#[tauri::command]
pub async fn diagnostics_export(state: S<'_>, app: AppHandle) -> CmdResult<String> {
    let settings = state.settings.read().await.clone();
    let identity = session::network_identity(&settings)?;
    let sync = state.sync.read().await.clone();
    let mut out = String::new();
    out.push_str(&format!(
        "Hashgram One for Windows {} ({})\n",
        app.package_info().version,
        crate::cmd_identity::COMMIT
    ));
    out.push_str(&format!(
        "network {} chain {} genesis {}\n",
        identity.network_id, identity.chain_id, identity.genesis_hash
    ));
    out.push_str(&format!(
        "unlocked {} sync phase {:?} rounds {} last_ok_ms {:?} last_error {:?}\n",
        state.is_unlocked().await,
        sync.phase,
        sync.rounds,
        sync.last_ok_ms,
        sync.last_error
    ));
    out.push_str(&format!(
        "uptime {} s, working set {} MiB\n",
        state.perf.uptime_ms() / 1000,
        crate::winsec::working_set_bytes() / (1024 * 1024)
    ));
    match state.link.read().await.clone() {
        Some(l) => {
            out.push_str("\npeers (no addresses):\n");
            for p in l.peers().await {
                out.push_str(&format!(
                    "  {} roles={} operator={}\n",
                    p.peer,
                    p.roles.join(","),
                    if p.operator.is_empty() { "-" } else { &p.operator }
                ));
            }
            out.push_str("\nrejected:\n");
            for r in l.rejected().await {
                out.push_str(&format!("  {} ({})\n", r.peer, label_reason(&r.reason)));
            }
        }
        None => out.push_str(&format!(
            "\nlink: down ({})\n",
            state.link_error.read().await.clone().unwrap_or_default()
        )),
    }
    {
        let mut g = state.one.lock().await;
        if let Some(one) = g.as_mut() {
            let c = one.mail().counts();
            out.push_str("\nmail (counts only):\n");
            for (f, n) in c {
                out.push_str(&format!("  {f}: {} ({} unread)\n", n.total, n.unread));
            }
            let u = one.drive().usage();
            out.push_str(&format!(
                "drive: {} files, {} folders, {} bytes, revision {} (committed {}), dirty {}\n",
                u.files, u.folders, u.bytes, u.revision, u.committed_revision, u.dirty
            ));
            out.push_str(&format!("spaces: {}\n", one.spaces().list().map(|s| s.len()).unwrap_or(0)));
            out.push_str(&format!("circles: {}\n", one.circles().list().map(|s| s.len()).unwrap_or(0)));
        }
    }
    out.push_str(&format!(
        "\nsettings: network={:?} bootstrap_override={} chain_api={} indexer={} gateway_set={}\n",
        settings.network.kind,
        !settings.network.bootstrap.is_empty(),
        !settings.network.chain_api.trim().is_empty(),
        settings.indexer().is_some(),
        !settings.network.gateway_address.trim().is_empty(),
    ));
    Ok(out)
}
