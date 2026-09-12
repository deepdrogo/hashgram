//! `/healthz` and `/metrics` on a loopback HTTP port.
//!
//! Metric labels are bounded enumerations only (direction, outcome) —
//! never addresses, usernames or domains, per `docs/LOGGING_POLICY.md`
//! ("never metric labels"). Counts of what happened, not to whom.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use tracing::info;

/// Label set for message outcomes.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct OutcomeLabels {
    /// `inbound` or `outbound`.
    pub direction: &'static str,
    /// `accepted`, `rejected`, `delivered`, `deferred`, `failed`, `bounced`.
    pub outcome: &'static str,
}

/// All gauges and counters.
pub struct Metrics {
    registry: Mutex<Registry>,
    /// Messages by direction and outcome.
    pub messages: Family<OutcomeLabels, Counter>,
    /// Messages offered through SMTP `DATA` (before parsing and policy).
    pub smtp_messages_offered: Counter,
    /// Sync rounds run against the Hashgram network.
    pub sync_rounds: Counter,
    /// Sync rounds that failed.
    pub sync_failures: Counter,
    /// Queue depth per kind.
    pub queue_depth: Family<QueueLabels, Gauge>,
    /// Whether the bridge identity currently sees a verified peer.
    pub connected: Gauge,
    /// Unix seconds of the last successful sync round.
    pub last_sync_ok: Gauge,
    /// Health flag consulted by `/healthz`.
    healthy: AtomicBool,
    started_at: AtomicU64,
}

/// Label set for queue depth.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct QueueLabels {
    /// `inbound` or `outbound`.
    pub kind: &'static str,
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics").finish_non_exhaustive()
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Registers everything.
    #[must_use]
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("hashgram_mail_gateway");
        let messages = Family::<OutcomeLabels, Counter>::default();
        registry.register(
            "messages",
            "Messages by direction and outcome",
            messages.clone(),
        );
        let smtp_messages_offered = Counter::default();
        registry.register(
            "smtp_messages_offered",
            "Messages offered through SMTP DATA, before parsing and policy",
            smtp_messages_offered.clone(),
        );
        let sync_rounds = Counter::default();
        registry.register("sync_rounds", "Hashgram sync rounds", sync_rounds.clone());
        let sync_failures = Counter::default();
        registry.register(
            "sync_failures",
            "Hashgram sync rounds that failed",
            sync_failures.clone(),
        );
        let queue_depth = Family::<QueueLabels, Gauge>::default();
        registry.register("queue_depth", "Retry queue depth", queue_depth.clone());
        let connected = Gauge::default();
        registry.register(
            "connected",
            "1 when a verified Hashgram peer is reachable",
            connected.clone(),
        );
        let last_sync_ok = Gauge::default();
        registry.register(
            "last_sync_ok_seconds",
            "Unix time of the last successful sync round",
            last_sync_ok.clone(),
        );
        Self {
            registry: Mutex::new(registry),
            messages,
            smtp_messages_offered,
            sync_rounds,
            sync_failures,
            queue_depth,
            connected,
            last_sync_ok,
            healthy: AtomicBool::new(false),
            started_at: AtomicU64::new(hashgram_app::ids::now_secs()),
        }
    }

    /// Counts a message outcome.
    pub fn message(&self, direction: &'static str, outcome: &'static str) {
        self.messages
            .get_or_create(&OutcomeLabels { direction, outcome })
            .inc();
    }

    /// Sets the health flag.
    pub fn set_healthy(&self, ok: bool) {
        self.healthy.store(ok, Ordering::Relaxed);
    }

    /// Current health.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Prometheus text exposition.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Ok(r) = self.registry.lock() {
            let _ = encode(&mut out, &r);
        }
        out
    }

    /// Seconds since start.
    #[must_use]
    pub fn uptime_secs(&self) -> u64 {
        hashgram_app::ids::now_secs().saturating_sub(self.started_at.load(Ordering::Relaxed))
    }
}

async fn healthz(State(m): State<Arc<Metrics>>) -> impl IntoResponse {
    let body = serde_json::json!({
        "ok": m.is_healthy(),
        "uptime_secs": m.uptime_secs(),
    })
    .to_string();
    let status = if m.is_healthy() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, [(header::CONTENT_TYPE, "application/json")], body)
}

async fn metrics(State(m): State<Arc<Metrics>>) -> impl IntoResponse {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/openmetrics-text; version=1.0.0; charset=utf-8",
        )],
        m.render(),
    )
}

/// The router (exposed for tests).
pub fn router(m: Arc<Metrics>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .with_state(m)
}

/// Serves until `shutdown` resolves.
pub async fn serve(
    listen: SocketAddr,
    m: Arc<Metrics>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen).await?;
    info!(%listen, "http (healthz, metrics) listening");
    axum::serve(listener, router(m))
        .with_graceful_shutdown(shutdown)
        .await
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn renders_without_identifying_labels() {
        let m = Metrics::new();
        m.message("inbound", "accepted");
        m.message("inbound", "accepted");
        m.message("outbound", "failed");
        m.queue_depth
            .get_or_create(&QueueLabels { kind: "inbound" })
            .set(3);
        let text = m.render();
        assert!(text.contains(
            "hashgram_mail_gateway_messages_total{direction=\"inbound\",outcome=\"accepted\"} 2"
        ));
        assert!(text.contains("hashgram_mail_gateway_queue_depth{kind=\"inbound\"} 3"));
        assert!(!m.is_healthy());
        m.set_healthy(true);
        assert!(m.is_healthy());
    }
}
