//! A loopback REST gateway backed by the app's P2P chain relay.
//!
//! A node running on this PC (Earn → Run a node) needs a chain REST API for
//! device authorisation and for submitting its receipts. A home PC has no
//! `hashgramd`; what it has is this app, which reads the chain through the
//! peer-to-peer relay with cross-checking and can broadcast. This server
//! exposes exactly that on `127.0.0.1:26680`: allow-listed GET reads and
//! `POST /cosmos/tx/v1beta1/txs`. Loopback is the boundary. It works while
//! the vault is locked: the link outlives the session.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

/// Port the proxy listens on (loopback only).
pub const PORT: u16 = 26680;

/// The URL a local node should use as `chain_api`.
#[must_use]
pub fn url() -> String {
    format!("http://127.0.0.1:{PORT}")
}

async fn client(st: &AppState) -> Result<hashgram_sdk::ChainClient, String> {
    let settings = st.settings.read().await.clone();
    let identity = crate::session::network_identity(&settings).map_err(|e| e.message)?;
    let chain_api = settings.network.chain_api.trim().to_owned();
    if !chain_api.is_empty() {
        return hashgram_sdk::ChainClient::new(&chain_api, &identity.chain_id).map_err(|e| e.to_string());
    }
    let link = st.link.read().await.clone().ok_or_else(|| "network link not up".to_owned())?;
    Ok(hashgram_sdk::chain_client_over_link(link, &identity.chain_id))
}

async fn read(State(st): State<Arc<AppState>>, Path(path): Path<String>, RawQuery(q): RawQuery) -> Response {
    let full = match q {
        Some(q) if !q.is_empty() => format!("{path}?{q}"),
        _ => path,
    };
    let client = match client(&st).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };
    let (p, qq) = hashgram_sdk::chain::transport::split_path(&full);
    match client.transport().get(p, qq).await {
        Ok(r) => {
            let mut resp = Response::new(axum::body::Body::from(r.body));
            *resp.status_mut() = StatusCode::from_u16(r.status).unwrap_or(StatusCode::OK);
            resp.headers_mut()
                .insert("content-type", HeaderValue::from_static("application/json"));
            if r.height > 0 {
                if let Ok(v) = HeaderValue::from_str(&r.height.to_string()) {
                    resp.headers_mut().insert("grpc-metadata-x-cosmos-block-height", v);
                }
            }
            if let Some(v) = client.verification() {
                if let Ok(h) = HeaderValue::from_str(&format!(
                    "{}:{}",
                    v.peers.len(),
                    if v.agreed {
                        "agreed"
                    } else if v.single_operator {
                        "single-operator"
                    } else {
                        "unverified"
                    }
                )) {
                    resp.headers_mut().insert("x-hashgram-verified", h);
                }
            }
            resp
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

async fn broadcast(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    #[derive(serde::Deserialize)]
    struct B {
        tx_bytes: String,
    }
    let b: B = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("bad body: {e}")).into_response(),
    };
    let Some(tx) = crate::util::base64_decode(&b.tx_bytes) else {
        return (StatusCode::BAD_REQUEST, "tx_bytes is not base64").into_response();
    };
    let client = match client(&st).await {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_GATEWAY, e).into_response(),
    };
    match client.transport().broadcast(&tx).await {
        Ok(r) => {
            let mut resp = Response::new(axum::body::Body::from(r.body));
            *resp.status_mut() = StatusCode::from_u16(r.status).unwrap_or(StatusCode::OK);
            resp.headers_mut()
                .insert("content-type", HeaderValue::from_static("application/json"));
            resp
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e.to_string()).into_response(),
    }
}

async fn unsupported() -> Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        "simulate is not available through the peer-to-peer relay; the client estimates gas",
    )
        .into_response()
}

/// Serves until the process ends. Binding failure (port taken) is logged
/// and the app carries on: the proxy is a convenience for a local node.
pub async fn serve(state: Arc<AppState>) {
    let router = Router::new()
        .route("/cosmos/tx/v1beta1/simulate", post(unsupported))
        .route(
            "/cosmos/tx/v1beta1/txs",
            post(broadcast).get(|s, q| async move { read(s, Path("cosmos/tx/v1beta1/txs".to_owned()), q).await }),
        )
        .route("/{*path}", get(read))
        .with_state(state);
    let addr = format!("127.0.0.1:{PORT}");
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => {
            tracing::info!(addr, "loopback chain gateway listening (for a node on this PC)");
            if let Err(e) = axum::serve(l, router).await {
                tracing::warn!(error = %e, "loopback chain gateway stopped");
            }
        }
        Err(e) => tracing::warn!(addr, error = %e, "loopback chain gateway not started"),
    }
}
