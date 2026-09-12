//! How the app reaches the chain, in order of precedence, each with a live
//! health dot:
//!
//! 1. A chain node on this PC (`127.0.0.1:1317`), when one is running.
//! 2. The P2P chain relay across the connected nodes, cross-checked over
//!    two operators (`hashgram_sdk::chain_relay`).
//! 3. HTTPS REST endpoints the user pasted in Settings → Network.
//!
//! Never a single hardcoded hostname as the only way in. An endpoint on a
//! different chain id is marked "wrong network" and skipped, not used.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hashgram_sdk::chain::transport::HttpTransport;
use hashgram_sdk::link::Link;
use hashgram_sdk::{ChainClient, ChainTransport, Verification};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

/// A source of chain answers.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A REST gateway on this machine.
    LocalNode {
        /// URL.
        url: String,
    },
    /// The P2P chain relay.
    P2pRelay,
    /// A user-pasted HTTPS endpoint.
    Https {
        /// URL.
        url: String,
    },
}

/// Health of one source.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointHealth {
    /// Which.
    pub source: Source,
    /// Reachable and on the right chain.
    pub ok: bool,
    /// What we found ("chain hashgram-1 at height 1234", "wrong network:
    /// hashgram-devnet-1", "unreachable").
    pub detail: String,
    /// Whether this is the source currently in use.
    pub active: bool,
    /// Milliseconds the check took.
    pub latency_ms: u32,
}

/// A read with its provenance.
#[derive(Debug, Clone, Serialize)]
pub struct ChainRead {
    /// The JSON body.
    pub value: serde_json::Value,
    /// Cross-check result (P2P only).
    pub verification: Option<Verification>,
    /// Which source answered.
    pub source: Source,
    /// Height reported, if known.
    pub height: u64,
    /// Whether this came from the short-lived cache.
    pub cached: bool,
}

struct Active {
    source: Source,
    client: ChainClient,
    chosen_at: Instant,
}

struct CacheEntry {
    at: Instant,
    read: ChainRead,
}

/// The chooser.
pub struct ChainAccess {
    chain_id: RwLock<String>,
    active: RwLock<Option<Active>>,
    health: RwLock<Vec<EndpointHealth>>,
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// Serialises re-evaluation so bursts of reads do not all probe.
    choosing: Mutex<()>,
}

/// How long a chosen source is trusted before re-checking precedence.
const RECHECK: Duration = Duration::from_secs(45);
/// How long a read stays fresh for repeated screens.
const READ_TTL: Duration = Duration::from_secs(4);

impl Default for ChainAccess {
    fn default() -> Self {
        Self {
            chain_id: RwLock::new(String::new()),
            active: RwLock::new(None),
            health: RwLock::new(Vec::new()),
            cache: Mutex::new(HashMap::new()),
            choosing: Mutex::new(()),
        }
    }
}

async fn probe_http(url: &str, chain_id: &str) -> (bool, String, u32) {
    let t = Instant::now();
    let transport = match HttpTransport::new(url) {
        Ok(t) => t,
        Err(e) => return (false, e.to_string(), 0),
    };
    let probe = tokio::time::timeout(
        Duration::from_millis(2500),
        transport.get("cosmos/base/tendermint/v1beta1/node_info", ""),
    )
    .await;
    let ms = t.elapsed().as_millis() as u32;
    match probe {
        Ok(Ok(r)) if r.ok() => {
            let network = serde_json::from_slice::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| {
                    v.get("default_node_info")
                        .and_then(|d| d.get("network"))
                        .and_then(|n| n.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            if network == chain_id {
                (true, format!("chain {network}"), ms)
            } else {
                (false, format!("wrong network: {network}"), ms)
            }
        }
        Ok(Ok(r)) => (false, format!("HTTP {}", r.status), ms),
        Ok(Err(e)) => (false, e.to_string(), ms),
        Err(_) => (false, "unreachable (timeout)".into(), ms),
    }
}

impl ChainAccess {
    /// Sets the chain id every source must be on.
    pub async fn set_chain_id(&self, chain_id: &str) {
        let mut c = self.chain_id.write().await;
        if *c != chain_id {
            *c = chain_id.to_owned();
            *self.active.write().await = None;
            self.cache.lock().await.clear();
        }
    }

    /// Drops the chosen source so the next read re-evaluates precedence.
    pub async fn invalidate(&self) {
        *self.active.write().await = None;
        self.cache.lock().await.clear();
    }

    /// Current health of every source.
    pub async fn health(&self) -> Vec<EndpointHealth> {
        self.health.read().await.clone()
    }

    /// The active source, if any.
    pub async fn active_source(&self) -> Option<Source> {
        self.active.read().await.as_ref().map(|a| a.source.clone())
    }

    /// Chooses (or re-uses) the best source. `link` is the P2P link if it is
    /// running; `local` and `https` come from settings.
    pub async fn client(
        &self,
        link: Option<Arc<Link>>,
        local: &str,
        https: &[String],
    ) -> Result<(Source, ChainClient), String> {
        if let Some(a) = self.active.read().await.as_ref() {
            if a.chosen_at.elapsed() < RECHECK {
                return Ok((a.source.clone(), a.client.clone()));
            }
        }
        let _guard = self.choosing.lock().await;
        // Someone else may have chosen while we waited.
        if let Some(a) = self.active.read().await.as_ref() {
            if a.chosen_at.elapsed() < RECHECK {
                return Ok((a.source.clone(), a.client.clone()));
            }
        }
        let chain_id = self.chain_id.read().await.clone();
        if chain_id.is_empty() {
            return Err("network not initialised".into());
        }
        let mut health = Vec::new();
        let mut chosen: Option<(Source, ChainClient)> = None;

        // 1. Local node.
        if !local.trim().is_empty() {
            let (ok, detail, ms) = probe_http(local, &chain_id).await;
            let source = Source::LocalNode {
                url: local.to_owned(),
            };
            if ok && chosen.is_none() {
                if let Ok(c) = ChainClient::new(local, &chain_id) {
                    chosen = Some((source.clone(), c));
                }
            }
            health.push(EndpointHealth {
                source,
                ok,
                detail,
                active: false,
                latency_ms: ms,
            });
        }

        // 2. P2P relay.
        {
            let t = Instant::now();
            let (ok, detail) = match &link {
                Some(l) => {
                    let relays = l.chain_relays().await;
                    if relays.is_empty() {
                        (
                            false,
                            "no connected node relays chain queries yet".to_owned(),
                        )
                    } else {
                        // One probe read proves the relay answers on this network.
                        let client = hashgram_sdk::chain_client_over_link(l.clone(), &chain_id);
                        match tokio::time::timeout(
                            Duration::from_secs(6),
                            client.query("cosmos/base/tendermint/v1beta1/node_info"),
                        )
                        .await
                        {
                            Ok(Ok(v)) => {
                                let network = v
                                    .get("default_node_info")
                                    .and_then(|d| d.get("network"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("");
                                if network == chain_id {
                                    let ops = relays
                                        .iter()
                                        .map(|p| p.operator.as_str())
                                        .filter(|o| !o.is_empty())
                                        .collect::<std::collections::HashSet<_>>()
                                        .len();
                                    if chosen.is_none() {
                                        chosen = Some((Source::P2pRelay, client));
                                    }
                                    (
                                        true,
                                        format!(
                                            "{} relay node(s), {} operator(s)",
                                            relays.len(),
                                            ops
                                        ),
                                    )
                                } else {
                                    (false, format!("wrong network: {network}"))
                                }
                            }
                            Ok(Err(e)) => (false, e.to_string()),
                            Err(_) => (false, "relay timed out".into()),
                        }
                    }
                }
                None => (false, "not connected".to_owned()),
            };
            health.push(EndpointHealth {
                source: Source::P2pRelay,
                ok,
                detail,
                active: false,
                latency_ms: t.elapsed().as_millis() as u32,
            });
        }

        // 3. User HTTPS endpoints, in order.
        for url in https {
            if url.trim().is_empty() {
                continue;
            }
            let (ok, detail, ms) = probe_http(url, &chain_id).await;
            let source = Source::Https { url: url.clone() };
            if ok && chosen.is_none() {
                if let Ok(c) = ChainClient::new(url, &chain_id) {
                    chosen = Some((source.clone(), c));
                }
            }
            health.push(EndpointHealth {
                source,
                ok,
                detail,
                active: false,
                latency_ms: ms,
            });
        }

        if let Some((src, _)) = &chosen {
            for h in &mut health {
                h.active = h.source == *src;
            }
        }
        *self.health.write().await = health;
        match chosen {
            Some((source, client)) => {
                *self.active.write().await = Some(Active {
                    source: source.clone(),
                    client: client.clone(),
                    chosen_at: Instant::now(),
                });
                Ok((source, client))
            }
            None => {
                *self.active.write().await = None;
                Err("no chain source is reachable: no local node, no relay node answered, no HTTPS endpoint configured".into())
            }
        }
    }

    /// A cached, cross-checked read.
    pub async fn get(
        &self,
        link: Option<Arc<Link>>,
        local: &str,
        https: &[String],
        path: &str,
    ) -> Result<ChainRead, String> {
        let key = path.trim_start_matches('/').to_owned();
        if let Some(e) = self.cache.lock().await.get(&key) {
            if e.at.elapsed() < READ_TTL {
                let mut r = e.read.clone();
                r.cached = true;
                return Ok(r);
            }
        }
        let (source, client) = self.client(link, local, https).await?;
        let (p, q) = hashgram_sdk::chain::transport::split_path(&key);
        let resp = client
            .transport()
            .get(p, q)
            .await
            .map_err(|e| e.to_string())?;
        if !resp.ok() {
            // A failed read may mean the source died; re-evaluate next time
            // for transport-level failures, not for 404s (which are answers).
            if resp.status >= 500 {
                self.invalidate().await;
            }
            return Err(format!(
                "HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            ));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&resp.body).map_err(|e| format!("not JSON: {e}"))?;
        let read = ChainRead {
            value,
            verification: client.verification(),
            source,
            height: resp.height,
            cached: false,
        };
        self.cache.lock().await.insert(
            key,
            CacheEntry {
                at: Instant::now(),
                read: read.clone(),
            },
        );
        Ok(read)
    }

    /// Drops cached reads (after a transaction, so balances refresh).
    pub async fn clear_cache(&self) {
        self.cache.lock().await.clear();
    }
}
