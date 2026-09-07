//! The local JSON API on `127.0.0.1:26672`, and `/metrics`.
//!
//! This is the operator and same-host surface: `hashgramctl` reads status
//! here, the indexer pulls events, the safety engine publishes attestations,
//! and a client on the same machine can use it instead of speaking libp2p.
//! It binds to loopback by default and has no authentication of its own,
//! because loopback plus a dedicated Unix user is the authentication: a
//! process that can reach this socket is already on the box. Binding it
//! anywhere else is the operator's decision and is logged as a warning.
//!
//! Nothing here can do anything the P2P protocol cannot: it is the same
//! services behind a different transport.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use hashgram_proto::pb;
use hashgram_proto::signing;
use prometheus_client::registry::Registry;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::app::Shared;

/// API state.
pub struct ApiState {
    /// Node state.
    pub shared: Arc<Shared>,
    /// Metrics registry.
    pub registry: Arc<Mutex<Registry>>,
    /// Chain REST client, for pass-through.
    pub chain: Option<crate::chain::ChainClient>,
    /// TURN secret, when this node issues credentials.
    pub turn_secret: Option<Vec<u8>>,
}

type S = State<Arc<ApiState>>;

/// Builds the router.
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/metrics", get(metrics))
        .route("/v1/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/peers", get(peers))
        .route("/v1/announcements", get(announcements))
        .route("/v1/social/events", get(social_since).post(social_publish))
        .route("/v1/social/events/{id}", get(social_get))
        .route("/v1/social/author/{author}", get(social_author))
        .route("/v1/blobs", post(blob_upload))
        .route("/v1/blobs/{cid}", get(blob_download).delete(blob_delete))
        .route("/v1/blobs/{cid}/health", get(blob_health))
        .route("/v1/blobs/{cid}/fetch", post(blob_fetch))
        .route("/v1/mailbox/{mailbox}/notified", get(mailbox_notified))
        .route("/v1/safety/attestations", post(safety_publish))
        .route("/v1/safety/attestations/{subject}", get(safety_get))
        .route("/v1/calls/turn-credentials", post(turn_credentials))
        .route("/v1/chain/{*path}", get(chain_passthrough))
        // Same-host uploads may be whole media files; the P2P path is
        // chunked, but this convenience path buffers. 256 MiB is the bound.
        .layer(axum::extract::DefaultBodyLimit::max(256 << 20))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(256 << 20))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(60),
        ))
        .with_state(state)
}

/// Serves the router.
pub async fn serve(addr: &str, state: Arc<ApiState>) -> anyhow::Result<()> {
    if !(addr.starts_with("127.") || addr.starts_with("[::1]") || addr.starts_with("localhost")) {
        warn!(addr, "local API is bound to a non-loopback address; anything that can reach it can use every role this node serves");
    }
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr, "local API listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn bad(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError { error: msg.into() }),
    )
}
fn not_found() -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            error: "not found".into(),
        }),
    )
}
fn unsupported(role: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiError {
            error: format!("this node does not serve the {role} role"),
        }),
    )
}
fn internal(e: impl std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: e.to_string(),
        }),
    )
}

async fn metrics(State(s): S) -> impl IntoResponse {
    let mut out = String::new();
    let reg = s.registry.lock().await;
    if prometheus_client::encoding::text::encode(&mut out, &reg).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "encode failed".to_owned(),
        )
            .into_response();
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )],
        out,
    )
        .into_response()
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
    peers_verified: usize,
    uptime_secs: u64,
}

async fn health(State(s): S) -> impl IntoResponse {
    let stats = s.shared.handle.stats().await;
    let verified = stats.as_ref().map(|x| x.verified).unwrap_or(0);
    let status = if stats.is_none() {
        "stopped"
    } else if verified == 0 {
        "isolated"
    } else {
        "ok"
    };
    let code = if status == "stopped" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (
        code,
        Json(Health {
            status,
            peers_verified: verified,
            uptime_secs: s.shared.started.elapsed().as_secs(),
        }),
    )
}

#[derive(Serialize)]
struct Status {
    network_id: String,
    chain_id: String,
    genesis_hash: String,
    roles: Vec<String>,
    swarm: Option<hashgram_p2p::Stats>,
    announcements_known: usize,
    mailbox: Option<crate::mailbox::MailboxStats>,
    blobs: Option<crate::blob::BlobStats>,
    social: Option<crate::social::SocialStats>,
    safety: Option<crate::safety::SafetyStats>,
    uptime_secs: u64,
    version: &'static str,
}

async fn status(State(s): S) -> impl IntoResponse {
    let sh = &s.shared;
    Json(Status {
        network_id: sh.identity.network_id.clone(),
        chain_id: sh.identity.chain_id.clone(),
        genesis_hash: sh.identity.genesis_hash.clone(),
        roles: sh.config.roles.clone(),
        swarm: sh.handle.stats().await,
        announcements_known: sh.announces.len(),
        mailbox: sh.services.mailbox.as_ref().and_then(|m| m.stats().ok()),
        blobs: sh.services.blob.as_ref().and_then(|b| b.stats().ok()),
        social: sh.services.social.as_ref().and_then(|x| x.stats().ok()),
        safety: sh.services.safety.as_ref().and_then(|x| x.stats().ok()),
        uptime_secs: sh.started.elapsed().as_secs(),
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn peers(State(s): S) -> impl IntoResponse {
    Json(s.shared.handle.peers().await)
}

#[derive(Deserialize)]
struct AnnounceQuery {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize)]
struct AnnounceView {
    peer_roles: Vec<String>,
    addrs: Vec<String>,
    operator_address: String,
    declared_storage_bytes: u64,
    timestamp: u64,
    expires_at: u64,
    turn: Option<pb::TurnInfo>,
    sfu: Option<pb::SfuInfo>,
}

async fn announcements(State(s): S, Query(q): Query<AnnounceQuery>) -> impl IntoResponse {
    let roles: Vec<String> = q.role.into_iter().collect();
    let list = s.shared.announces.query(&roles, q.limit.unwrap_or(50));
    Json(
        list.into_iter()
            .map(|a| AnnounceView {
                peer_roles: a.roles,
                addrs: a.addrs,
                operator_address: a.operator_address,
                declared_storage_bytes: a.declared_storage_bytes,
                timestamp: a.timestamp,
                expires_at: a.expires_at,
                turn: a.turn,
                sfu: a.sfu,
            })
            .collect::<Vec<_>>(),
    )
}

// -- social -------------------------------------------------------------------

/// A social event in JSON, with bytes as hex. The protobuf is the wire
/// form; this is for humans and same-host tools.
#[derive(Serialize, Deserialize)]
pub struct EventJson {
    /// Hex.
    pub id: String,
    /// Wire string.
    pub r#type: String,
    /// Address.
    pub author: String,
    /// Hex.
    pub device_pubkey: String,
    /// Unix seconds.
    pub timestamp: u64,
    /// Sequence.
    pub sequence: u64,
    /// Hex.
    pub previous_event: String,
    /// Hex protobuf payload.
    pub payload: String,
    /// Media, hex CIDs.
    pub media: Vec<MediaJson>,
    /// Hex.
    pub signature: String,
    /// Network id.
    pub network_id: String,
    /// Version.
    pub version: u32,
}

/// Media reference in JSON.
#[derive(Serialize, Deserialize)]
pub struct MediaJson {
    /// Hex.
    pub cid: String,
    /// MIME.
    pub mime: String,
    /// Bytes.
    pub size: u64,
    /// Kind.
    pub kind: String,
    /// Width.
    #[serde(default)]
    pub width: u32,
    /// Height.
    #[serde(default)]
    pub height: u32,
    /// Duration.
    #[serde(default)]
    pub duration_ms: u32,
    /// Hex.
    #[serde(default)]
    pub thumbnail_cid: String,
    /// Hex.
    #[serde(default)]
    pub content_hash: String,
}

impl From<pb::SocialEvent> for EventJson {
    fn from(e: pb::SocialEvent) -> Self {
        Self {
            id: hex::encode(&e.id),
            r#type: e.r#type,
            author: e.author,
            device_pubkey: hex::encode(&e.device_pubkey),
            timestamp: e.timestamp,
            sequence: e.sequence,
            previous_event: hex::encode(&e.previous_event),
            payload: hex::encode(&e.payload),
            media: e
                .media
                .into_iter()
                .map(|m| MediaJson {
                    cid: hex::encode(&m.cid),
                    mime: m.mime,
                    size: m.size,
                    kind: m.kind,
                    width: m.width,
                    height: m.height,
                    duration_ms: m.duration_ms,
                    thumbnail_cid: hex::encode(&m.thumbnail_cid),
                    content_hash: hex::encode(&m.content_hash),
                })
                .collect(),
            signature: hex::encode(&e.signature),
            network_id: e.network_id,
            version: e.version,
        }
    }
}

impl TryFrom<EventJson> for pb::SocialEvent {
    type Error = String;
    fn try_from(j: EventJson) -> Result<Self, String> {
        let h = |s: &str, f: &str| hex::decode(s).map_err(|e| format!("{f}: {e}"));
        Ok(pb::SocialEvent {
            network_id: j.network_id,
            version: j.version,
            id: h(&j.id, "id")?,
            r#type: j.r#type,
            author: j.author,
            device_pubkey: h(&j.device_pubkey, "device_pubkey")?,
            timestamp: j.timestamp,
            sequence: j.sequence,
            previous_event: h(&j.previous_event, "previous_event")?,
            payload: h(&j.payload, "payload")?,
            media: j
                .media
                .into_iter()
                .map(|m| {
                    Ok(pb::MediaReference {
                        cid: h(&m.cid, "media.cid")?,
                        mime: m.mime,
                        size: m.size,
                        kind: m.kind,
                        width: m.width,
                        height: m.height,
                        duration_ms: m.duration_ms,
                        thumbnail_cid: h(&m.thumbnail_cid, "media.thumbnail_cid")?,
                        content_hash: h(&m.content_hash, "media.content_hash")?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            signature: h(&j.signature, "signature")?,
        })
    }
}

#[derive(Deserialize)]
struct SinceQuery {
    #[serde(default)]
    after_ts: Option<u64>,
    #[serde(default)]
    after_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn social_since(
    State(s): S,
    Query(q): Query<SinceQuery>,
) -> Result<Json<Vec<EventJson>>, (StatusCode, Json<ApiError>)> {
    let social = s
        .shared
        .services
        .social
        .as_ref()
        .ok_or_else(|| unsupported("social"))?;
    let after = match (q.after_ts, q.after_id) {
        (Some(ts), Some(id)) => Some((ts, hex::decode(id).map_err(|e| bad(e.to_string()))?)),
        (Some(ts), None) => Some((ts, Vec::new())),
        _ => None,
    };
    let events = social
        .since(after, q.limit.unwrap_or(200).min(1000))
        .map_err(internal)?;
    Ok(Json(events.into_iter().map(EventJson::from).collect()))
}

async fn social_get(
    State(s): S,
    Path(id): Path<String>,
) -> Result<Json<EventJson>, (StatusCode, Json<ApiError>)> {
    let social = s
        .shared
        .services
        .social
        .as_ref()
        .ok_or_else(|| unsupported("social"))?;
    let id = hex::decode(id).map_err(|e| bad(e.to_string()))?;
    let mut events = social
        .fetch(
            &pb::EventFetch {
                ids: vec![id],
                ..Default::default()
            },
            s.shared.services.safety.as_deref(),
        )
        .map_err(internal)?;
    events
        .pop()
        .map(|e| Json(EventJson::from(e)))
        .ok_or_else(not_found)
}

#[derive(Deserialize)]
struct AuthorQuery {
    #[serde(default)]
    from_sequence: u64,
    #[serde(default)]
    limit: Option<u32>,
}

async fn social_author(
    State(s): S,
    Path(author): Path<String>,
    Query(q): Query<AuthorQuery>,
) -> Result<Json<Vec<EventJson>>, (StatusCode, Json<ApiError>)> {
    let social = s
        .shared
        .services
        .social
        .as_ref()
        .ok_or_else(|| unsupported("social"))?;
    let events = social
        .fetch(
            &pb::EventFetch {
                author,
                from_sequence: q.from_sequence,
                limit: q.limit.unwrap_or(0),
                ..Default::default()
            },
            s.shared.services.safety.as_deref(),
        )
        .map_err(internal)?;
    Ok(Json(events.into_iter().map(EventJson::from).collect()))
}

#[derive(Serialize)]
struct Accepted {
    accepted: bool,
    id: String,
}

async fn social_publish(
    State(s): S,
    Json(j): Json<EventJson>,
) -> Result<Json<Accepted>, (StatusCode, Json<ApiError>)> {
    let social = s
        .shared
        .services
        .social
        .as_ref()
        .ok_or_else(|| unsupported("social"))?;
    let ev: pb::SocialEvent = j.try_into().map_err(bad)?;
    let id = hex::encode(&ev.id);
    let me = s.shared.handle.peer_id();
    let resp = social
        .handle(
            &s.shared,
            me,
            pb::request::Body::EventPublish(pb::EventPublish { event: Some(ev) }),
        )
        .await;
    match resp.body {
        Some(pb::response::Body::EventPublish(r)) if r.accepted => {
            Ok(Json(Accepted { accepted: true, id }))
        }
        Some(pb::response::Body::EventPublish(r)) => Err(bad(r.reason)),
        Some(pb::response::Body::Error(e)) => Err(bad(format!("{}: {}", e.code, e.message))),
        _ => Err(internal("unexpected response")),
    }
}

// -- blobs ----------------------------------------------------------------------

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default = "default_mime")]
    mime: String,
    #[serde(default)]
    encrypted: bool,
}
fn default_mime() -> String {
    "application/octet-stream".into()
}

#[derive(Serialize)]
struct Uploaded {
    cid: String,
    size: u64,
    chunks: usize,
}

async fn blob_upload(
    State(s): S,
    Query(q): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> Result<Json<Uploaded>, (StatusCode, Json<ApiError>)> {
    let blob = s
        .shared
        .services
        .blob
        .as_ref()
        .ok_or_else(|| unsupported("store/media"))?;
    let m = hashgram_proto::blob::manifest_for(&body, &q.mime, q.encrypted)
        .map_err(|e| bad(e.to_string()))?;
    let cid = blob
        .put_local(&m, &body, &s.shared.announce_signer.public_key())
        .map_err(|e| bad(e.to_string()))?;
    let _ = s
        .shared
        .handle
        .start_providing(hashgram_proto::dht::blob(&cid))
        .await;
    Ok(Json(Uploaded {
        cid: hex::encode(cid),
        size: m.size,
        chunks: m.chunks.len(),
    }))
}

async fn blob_download(
    State(s): S,
    Path(cid): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, Json<ApiError>)> {
    let blob = s
        .shared
        .services
        .blob
        .as_ref()
        .ok_or_else(|| unsupported("store/media"))?;
    let cid = hex::decode(cid).map_err(|e| bad(e.to_string()))?;
    if s.shared
        .services
        .safety
        .as_ref()
        .is_some_and(|x| x.is_blocked_cid(&cid))
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError {
                error: "blocked by a trusted safety attestation".into(),
            }),
        ));
    }
    let me = s.shared.handle.peer_id();
    let resp = blob
        .handle(
            &s.shared,
            me,
            pb::request::Body::BlobGetManifest(pb::BlobGetManifest { cid: cid.clone() }),
        )
        .await;
    let m = match resp.body {
        Some(pb::response::Body::BlobGetManifest(r)) if r.found => {
            r.manifest.ok_or_else(not_found)?
        }
        _ => return Err(not_found()),
    };
    let mut out = Vec::with_capacity(m.size as usize);
    for i in 0..m.chunks.len() as u32 {
        let r = blob
            .handle(
                &s.shared,
                me,
                pb::request::Body::BlobGetChunk(pb::BlobGetChunk {
                    cid: cid.clone(),
                    index: i,
                }),
            )
            .await;
        match r.body {
            Some(pb::response::Body::BlobGetChunk(c)) if c.found => out.extend_from_slice(&c.data),
            _ => {
                return Err((
                    StatusCode::PARTIAL_CONTENT,
                    Json(ApiError {
                        error: format!("chunk {i} missing"),
                    }),
                ))
            }
        }
    }
    Ok(([(axum::http::header::CONTENT_TYPE, m.mime)], out))
}

#[derive(Serialize)]
struct Deleted {
    deleted: bool,
}

async fn blob_delete(
    State(s): S,
    Path(cid): Path<String>,
) -> Result<Json<Deleted>, (StatusCode, Json<ApiError>)> {
    let blob = s
        .shared
        .services
        .blob
        .as_ref()
        .ok_or_else(|| unsupported("store/media"))?;
    let cid = hex::decode(cid).map_err(|e| bad(e.to_string()))?;
    let deleted = blob.delete(&cid).map_err(internal)?;
    if deleted {
        s.shared
            .handle
            .stop_providing(hashgram_proto::dht::blob(&cid))
            .await;
    }
    Ok(Json(Deleted { deleted }))
}

async fn blob_health(
    State(s): S,
    Path(cid): Path<String>,
) -> Result<Json<crate::blob::Health>, (StatusCode, Json<ApiError>)> {
    let blob = s
        .shared
        .services
        .blob
        .as_ref()
        .ok_or_else(|| unsupported("store/media"))?;
    let cid = hex::decode(cid).map_err(|e| bad(e.to_string()))?;
    blob.health(&cid, s.shared.handle.peer_id())
        .map_err(internal)?
        .map(Json)
        .ok_or_else(not_found)
}

#[derive(Deserialize)]
struct FetchBody {
    #[serde(default)]
    peer: Option<String>,
    #[serde(default)]
    addrs: Vec<String>,
}

#[derive(Serialize)]
struct Fetched {
    cid: String,
    size: u64,
    from: String,
}

/// Pulls a blob from a peer (or from any DHT provider) into this node.
async fn blob_fetch(
    State(s): S,
    Path(cid): Path<String>,
    Json(b): Json<FetchBody>,
) -> Result<Json<Fetched>, (StatusCode, Json<ApiError>)> {
    let blob = s
        .shared
        .services
        .blob
        .as_ref()
        .ok_or_else(|| unsupported("store/media"))?;
    let cid = hex::decode(cid).map_err(|e| bad(e.to_string()))?;
    let addrs: Vec<hashgram_p2p::Multiaddr> =
        b.addrs.iter().filter_map(|a| a.parse().ok()).collect();
    let peers: Vec<hashgram_p2p::PeerId> = match b.peer {
        Some(p) => vec![p.parse().map_err(|_| bad("peer is not a peer id"))?],
        None => {
            s.shared
                .handle
                .get_providers(hashgram_proto::dht::blob(&cid))
                .await
        }
    };
    let me = s.shared.handle.peer_id();
    let mut last_err = "no providers found".to_owned();
    for p in peers.into_iter().filter(|p| *p != me) {
        match blob.pull_blob(&s.shared, p, addrs.clone(), &cid).await {
            Ok(m) => {
                return Ok(Json(Fetched {
                    cid: hex::encode(&cid),
                    size: m.size,
                    from: p.to_string(),
                }))
            }
            Err(e) => last_err = format!("{p}: {e}"),
        }
    }
    Err((StatusCode::BAD_GATEWAY, Json(ApiError { error: last_err })))
}

// -- mailbox --------------------------------------------------------------------

#[derive(Serialize)]
struct Notified {
    notified: bool,
}

async fn mailbox_notified(
    State(s): S,
    Path(mailbox): Path<String>,
) -> Result<Json<Notified>, (StatusCode, Json<ApiError>)> {
    let m = s
        .shared
        .services
        .mailbox
        .as_ref()
        .ok_or_else(|| unsupported("store"))?;
    let mb = hex::decode(mailbox).map_err(|e| bad(e.to_string()))?;
    Ok(Json(Notified {
        notified: m.take_notified(&mb),
    }))
}

// -- safety ---------------------------------------------------------------------

#[derive(Deserialize)]
struct AttestationJson {
    #[serde(default)]
    cid: String,
    #[serde(default)]
    event_id: String,
    #[serde(default)]
    content_hash: String,
    verdict: String,
    policy: String,
    reason_code: String,
    timestamp: u64,
    attestor_pubkey: String,
    signature: String,
}

#[derive(Serialize)]
struct AttestationOut {
    subject: String,
    verdict: String,
    policy: String,
    reason_code: String,
    timestamp: u64,
    attestor_pubkey: String,
    trusted: bool,
}

fn verdict_from(s: &str) -> Option<pb::Verdict> {
    Some(match s {
        "CONTENT_ALLOW" => pb::Verdict::ContentAllow,
        "CONTENT_RESTRICT" => pb::Verdict::ContentRestrict,
        "CONTENT_QUARANTINE" => pb::Verdict::ContentQuarantine,
        "CONTENT_BLOCK" => pb::Verdict::ContentBlock,
        "CONTENT_UNBLOCK" => pb::Verdict::ContentUnblock,
        _ => return None,
    })
}

fn verdict_name(v: i32) -> String {
    pb::Verdict::try_from(v)
        .map(|v| v.as_str_name().to_owned())
        .unwrap_or_else(|_| "UNKNOWN".into())
}

async fn safety_publish(
    State(s): S,
    Json(j): Json<AttestationJson>,
) -> Result<Json<Accepted>, (StatusCode, Json<ApiError>)> {
    let safety = s
        .shared
        .services
        .safety
        .as_ref()
        .ok_or_else(|| unsupported("safety"))?;
    let h = |x: &str| hex::decode(x).map_err(|e| bad(e.to_string()));
    let a = pb::ContentAttestation {
        network_id: s.shared.identity.network_id.clone(),
        version: 1,
        cid: h(&j.cid)?,
        event_id: h(&j.event_id)?,
        content_hash: h(&j.content_hash)?,
        verdict: verdict_from(&j.verdict).ok_or_else(|| bad("unknown verdict"))? as i32,
        policy: j.policy,
        reason_code: j.reason_code,
        timestamp: j.timestamp,
        attestor_pubkey: h(&j.attestor_pubkey)?,
        signature: h(&j.signature)?,
    };
    let subject = if !a.cid.is_empty() {
        hex::encode(&a.cid)
    } else if !a.event_id.is_empty() {
        hex::encode(&a.event_id)
    } else {
        hex::encode(&a.content_hash)
    };
    safety.publish(&s.shared, a).await.map_err(bad)?;
    Ok(Json(Accepted {
        accepted: true,
        id: subject,
    }))
}

async fn safety_get(
    State(s): S,
    Path(subject): Path<String>,
) -> Result<Json<Vec<AttestationOut>>, (StatusCode, Json<ApiError>)> {
    let safety = s
        .shared
        .services
        .safety
        .as_ref()
        .ok_or_else(|| unsupported("safety"))?;
    let subj = hex::decode(subject).map_err(|e| bad(e.to_string()))?;
    let resp = safety.query(&pb::AttestationQuery {
        cids: vec![subj.clone()],
        event_ids: vec![subj],
    });
    let list = match resp.body {
        Some(pb::response::Body::AttestationQuery(r)) => r.attestations,
        _ => Vec::new(),
    };
    Ok(Json(
        list.into_iter()
            .map(|a| AttestationOut {
                subject: if !a.cid.is_empty() {
                    hex::encode(&a.cid)
                } else if !a.event_id.is_empty() {
                    hex::encode(&a.event_id)
                } else {
                    hex::encode(&a.content_hash)
                },
                verdict: verdict_name(a.verdict),
                policy: a.policy,
                reason_code: a.reason_code,
                timestamp: a.timestamp,
                trusted: safety.is_trusted(&a.attestor_pubkey),
                attestor_pubkey: hex::encode(&a.attestor_pubkey),
            })
            .collect(),
    ))
}

// -- calls ----------------------------------------------------------------------

#[derive(Deserialize)]
struct TurnRequest {
    /// Hex ed25519 device public key.
    device_pubkey: String,
    /// Unix seconds, signed.
    timestamp: u64,
    /// Hex signature over the mailbox-fetch preimage with an empty cursor
    /// and limit 0: reusing the fetch purpose proves the same thing, "I hold
    /// this device key right now", without a new signing domain.
    signature: String,
}

async fn turn_credentials(
    State(s): S,
    Json(r): Json<TurnRequest>,
) -> Result<Json<crate::calls::TurnCredential>, (StatusCode, Json<ApiError>)> {
    let secret = s.turn_secret.as_ref().ok_or_else(|| unsupported("call"))?;
    let device_pubkey = hex::decode(&r.device_pubkey).map_err(|e| bad(e.to_string()))?;
    let f = pb::MailboxFetch {
        mailbox: signing::mailbox_for(&device_pubkey).to_vec(),
        device_pubkey: device_pubkey.clone(),
        cursor: vec![],
        limit: 0,
        timestamp: r.timestamp,
        signature: hex::decode(&r.signature).map_err(|e| bad(e.to_string()))?,
    };
    hashgram_proto::validate::mailbox_fetch(&f, crate::store::now())
        .map_err(|e| bad(e.to_string()))?;
    signing::verify_mailbox_fetch(&s.shared.identity, &f).map_err(|e| bad(e.to_string()))?;
    let label = hex::encode(&signing::mailbox_for(&device_pubkey)[..8]);
    Ok(Json(crate::calls::issue(
        secret,
        &label,
        &s.shared.config.turn_uris,
        crate::store::now(),
    )))
}

// -- chain pass-through -----------------------------------------------------------

async fn chain_passthrough(
    State(s): S,
    Path(path): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let chain = s.chain.as_ref().ok_or_else(|| unsupported("chain"))?;
    // Only read paths of the Hashgram and Cosmos modules are forwarded; this
    // is a convenience for same-host clients, not a proxy.
    if !(path.starts_with("hashgram/")
        || path.starts_with("cosmos/bank/")
        || path.starts_with("cosmos/auth/")
        || path.starts_with("cosmos/base/"))
    {
        return Err(bad("only read queries under hashgram/, cosmos/bank/, cosmos/auth/ and cosmos/base/ are forwarded"));
    }
    chain.get_json(&path).await.map(Json).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
    })
}
