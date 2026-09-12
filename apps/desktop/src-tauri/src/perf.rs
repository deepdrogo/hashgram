//! Performance spans for the hidden Performance panel (Ctrl+Shift+P).
//!
//! A `tracing` layer records every closed span with its duration into a
//! bounded ring buffer; the frontend adds its own marks (cold start to
//! interactive, list frame times). Nothing here leaves the machine.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use tracing::span;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// One recorded span or mark.
#[derive(Debug, Clone, Serialize)]
pub struct SpanRecord {
    /// Name.
    pub name: String,
    /// Duration in microseconds.
    pub micros: u64,
    /// Milliseconds since app start when it ended.
    pub at_ms: u64,
    /// "rust" or "ui".
    pub origin: &'static str,
}

/// The store.
pub struct PerfStore {
    started: Instant,
    records: Mutex<VecDeque<SpanRecord>>,
}

const CAPACITY: usize = 600;

impl Default for PerfStore {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            records: Mutex::new(VecDeque::with_capacity(CAPACITY)),
        }
    }
}

impl PerfStore {
    /// Milliseconds since the process started tracking.
    #[must_use]
    pub fn uptime_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Records a span or mark.
    pub fn push(&self, name: impl Into<String>, micros: u64, origin: &'static str) {
        if let Ok(mut r) = self.records.lock() {
            if r.len() >= CAPACITY {
                r.pop_front();
            }
            r.push_back(SpanRecord {
                name: name.into(),
                micros,
                at_ms: self.uptime_ms(),
                origin,
            });
        }
    }

    /// A snapshot, oldest first.
    #[must_use]
    pub fn snapshot(&self) -> Vec<SpanRecord> {
        self.records
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// The tracing layer.
pub struct PerfLayer {
    store: std::sync::Arc<PerfStore>,
}

impl PerfLayer {
    /// Wraps a store.
    #[must_use]
    pub fn new(store: std::sync::Arc<PerfStore>) -> Self {
        Self { store }
    }
}

struct Opened(Instant);

impl<S> Layer<S> for PerfLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_new_span(&self, _attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(Opened(Instant::now()));
        }
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let name = span.name();
            if let Some(Opened(t)) = span.extensions().get::<Opened>() {
                self.store
                    .push(name, t.elapsed().as_micros() as u64, "rust");
            }
        }
    }
}
