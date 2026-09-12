//! The network link manager: one swarm for the whole app.
//!
//! Starts the SDK `Link` for the pinned network (Mainnet: compiled-in seeds
//! plus the persisted peerstore; devnet: whatever the user configured),
//! keeps a view of every peer for the "Connected nodes" panel — roles,
//! operator, latency, transport, which discovery layer found it, which
//! served the last chain reads — and never retries a wrong-network peer
//! silently: the swarm bans it, and it is listed here greyed with the
//! reason.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hashgram_sdk::link::{Link, RejectedPeer};
use hashgram_sdk::{pb, Multiaddr, NetworkIdentity, PeerId, Verification};
use serde::Serialize;
use tokio::sync::{Mutex, RwLock};

use crate::db::Db;
use crate::settings::{NetworkKind, Settings};

/// One connected peer, as the panel shows it.
#[derive(Debug, Clone, Serialize)]
pub struct PeerView {
    /// Peer id (`12D3Koo…`).
    pub peer_id: String,
    /// Operator address from the handshake, if claimed.
    pub operator: String,
    /// Roles claimed at the handshake.
    pub roles: Vec<String>,
    /// Round-trip of a small request, ms; `None` until measured.
    pub latency_ms: Option<u32>,
    /// `QUIC`, `TCP` or `relayed`.
    pub transport: String,
    /// `inbound` / `outbound`.
    pub direction: String,
    /// Seconds connected.
    pub connected_secs: u64,
    /// `built-in list`, `peerstore`, `DHT` or `configured`.
    pub discovery: String,
    /// Whether this peer served the most recent chain read.
    pub served_last_read: bool,
    /// Whether its answer agreed with the other operator's.
    pub agreed: Option<bool>,
    /// Whether the handshake completed.
    pub verified: bool,
    /// Whether this peer relays chain queries.
    pub relays_chain: bool,
}

/// A peer that failed the handshake.
#[derive(Debug, Clone, Serialize)]
pub struct RejectedView {
    /// Peer id.
    pub peer_id: String,
    /// The swarm's reason; a genesis mismatch reads "wrong network".
    pub reason: String,
    /// Human label.
    pub label: String,
    /// Unix seconds.
    pub at: u64,
}

/// The whole network view.
#[derive(Debug, Clone, Serialize)]
pub struct NetSnapshot {
    /// Whether the swarm is running.
    pub running: bool,
    /// Network name (`mainnet`/`devnet`).
    pub network: String,
    /// Chain id.
    pub chain_id: String,
    /// Pinned genesis hash.
    pub genesis_hash: String,
    /// Our peer id.
    pub own_peer_id: String,
    /// Peers.
    pub peers: Vec<PeerView>,
    /// Rejected peers.
    pub rejected: Vec<RejectedView>,
    /// autonat verdict: `public`, `private`, `unknown`.
    pub nat: String,
    /// Peers in the peerstore file.
    pub peerstore_size: usize,
    /// Compiled-in seed multiaddrs.
    pub builtin_seeds: Vec<String>,
    /// DHT routing-table size.
    pub kad_peers: usize,
    /// Verified peer count.
    pub verified: usize,
    /// Last chain-read verification, if any.
    pub last_read: Option<Verification>,
    /// Seconds since the link started.
    pub uptime_secs: u64,
    /// Listen addresses (ours).
    pub listen_addrs: Vec<String>,
    /// External addresses autonat/identify learned (ours).
    pub external_addrs: Vec<String>,
}

/// The manager.
pub struct NetManager {
    link: RwLock<Option<Arc<Link>>>,
    identity: RwLock<Option<NetworkIdentity>>,
    builtin_ids: RwLock<HashSet<String>>,
    peerstore_ids: RwLock<HashSet<String>>,
    configured_ids: RwLock<HashSet<String>>,
    latency: Mutex<HashMap<String, u32>>,
    started: Mutex<Option<Instant>>,
    last_read: Mutex<Option<Verification>>,
}

impl Default for NetManager {
    fn default() -> Self {
        Self {
            link: RwLock::new(None),
            identity: RwLock::new(None),
            builtin_ids: RwLock::new(HashSet::new()),
            peerstore_ids: RwLock::new(HashSet::new()),
            configured_ids: RwLock::new(HashSet::new()),
            latency: Mutex::new(HashMap::new()),
            started: Mutex::new(None),
            last_read: Mutex::new(None),
        }
    }
}

fn peer_id_of(addr: &str) -> Option<String> {
    addr.split("/p2p/")
        .nth(1)
        .map(|s| s.split('/').next().unwrap_or(s).to_owned())
}

/// The network identity for the current settings.
pub fn identity_for(settings: &Settings) -> Result<NetworkIdentity, String> {
    match settings.network.kind {
        NetworkKind::Mainnet => Ok(NetworkIdentity::mainnet(
            hashgram_sdk::net::MAINNET_GENESIS_HASH,
        )),
        NetworkKind::Devnet => {
            let g = settings
                .network
                .devnet_genesis_hash
                .trim()
                .to_ascii_lowercase();
            if !NetworkIdentity::is_well_formed_genesis_hash(&g) {
                return Err(
                    "DEVNET profile needs a 64-hex genesis hash (Settings → Network)".into(),
                );
            }
            Ok(NetworkIdentity::devnet(&g))
        }
    }
}

impl NetManager {
    /// Starts the swarm if it is not running. Returns quickly: peers verify
    /// in the background and appear in [`Self::snapshot`] as they do.
    pub async fn start(&self, settings: &Settings, db: &Db) -> Result<Arc<Link>, String> {
        if let Some(l) = self.link.read().await.clone() {
            return Ok(l);
        }
        let identity = identity_for(settings)?;
        let (bootstrap, configured): (Vec<String>, bool) = match settings.network.kind {
            NetworkKind::Mainnet => (hashgram_sdk::net::mainnet_bootstrap_peers(), false),
            NetworkKind::Devnet => (settings.network.devnet_bootstrap.clone(), true),
        };
        if bootstrap.is_empty() {
            return Err("no bootstrap peers for this network profile".into());
        }
        let addrs: Vec<Multiaddr> = bootstrap.iter().filter_map(|a| a.parse().ok()).collect();
        {
            let ids: HashSet<String> = bootstrap.iter().filter_map(|a| peer_id_of(a)).collect();
            if configured {
                *self.configured_ids.write().await = ids;
            } else {
                *self.builtin_ids.write().await = ids;
            }
        }
        // Who was in the peerstore before this run: those were found by an
        // earlier session, not by the built-in list.
        let ps_path = crate::paths::peerstore_path();
        {
            let mut ids = HashSet::new();
            if let Ok(ps) = hashgram_p2p::peerstore::Peerstore::load(&ps_path) {
                for (p, _) in ps.candidates(10_000) {
                    ids.insert(p.to_string());
                }
            }
            *self.peerstore_ids.write().await = ids;
        }
        let link = Link::connect(&identity, &addrs, Some(&ps_path), Duration::from_millis(0))
            .await
            .map_err(|e| e.to_string())?;
        let link = Arc::new(link);
        *self.link.write().await = Some(link.clone());
        *self.identity.write().await = Some(identity);
        *self.started.lock().await = Some(Instant::now());
        // Note sightings for the discovery layer as peers verify.
        let _ = db;
        Ok(link)
    }

    /// The running link, if any.
    pub async fn link(&self) -> Option<Arc<Link>> {
        self.link.read().await.clone()
    }

    /// The network identity in use.
    pub async fn identity(&self) -> Option<NetworkIdentity> {
        self.identity.read().await.clone()
    }

    /// Stops the swarm (network profile change, "forget peers").
    pub async fn stop(&self) {
        if let Some(l) = self.link.write().await.take() {
            l.shutdown().await;
        }
        *self.identity.write().await = None;
        *self.started.lock().await = None;
        self.latency.lock().await.clear();
    }

    /// Records who served the last chain read.
    pub async fn note_read(&self, v: Option<Verification>) {
        *self.last_read.lock().await = v;
    }

    /// Measures round-trip time to every verified peer with the smallest
    /// request there is (an announce query with limit 1).
    pub async fn measure_latency(&self) {
        let Some(link) = self.link().await else {
            return;
        };
        let peers = link.peers().await;
        for p in peers {
            let t = Instant::now();
            let ok = tokio::time::timeout(
                Duration::from_secs(4),
                link.request(
                    p.peer,
                    pb::request::Body::AnnounceQuery(pb::AnnounceQuery {
                        roles: vec![],
                        limit: 1,
                    }),
                ),
            )
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false);
            if ok {
                self.latency
                    .lock()
                    .await
                    .insert(p.peer.to_string(), t.elapsed().as_millis() as u32);
            }
        }
    }

    fn transport_of(addr: &str) -> String {
        if addr.contains("/p2p-circuit") {
            "relayed".into()
        } else if addr.contains("/quic") {
            "QUIC".into()
        } else if addr.contains("/tcp/") {
            "TCP".into()
        } else {
            "-".into()
        }
    }

    async fn discovery_of(&self, db: &Db, peer_id: &str) -> String {
        if self.builtin_ids.read().await.contains(peer_id) {
            return "built-in list".into();
        }
        if self.configured_ids.read().await.contains(peer_id) {
            return "configured".into();
        }
        if self.peerstore_ids.read().await.contains(peer_id) {
            return "peerstore".into();
        }
        match db.peer_discovery(peer_id) {
            Ok(Some(d)) => d,
            _ => "DHT".into(),
        }
    }

    /// The full view.
    pub async fn snapshot(&self, db: &Db) -> NetSnapshot {
        let identity = self.identity().await;
        let (network, chain_id, genesis_hash) = match &identity {
            Some(i) => (
                if i.is_mainnet() { "mainnet" } else { "devnet" }.to_owned(),
                i.chain_id.clone(),
                i.genesis_hash.clone(),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        let builtin_seeds = hashgram_sdk::net::mainnet_bootstrap_peers();
        let Some(link) = self.link().await else {
            return NetSnapshot {
                running: false,
                network,
                chain_id,
                genesis_hash,
                own_peer_id: String::new(),
                peers: vec![],
                rejected: vec![],
                nat: "unknown".into(),
                peerstore_size: 0,
                builtin_seeds,
                kad_peers: 0,
                verified: 0,
                last_read: None,
                uptime_secs: 0,
                listen_addrs: vec![],
                external_addrs: vec![],
            };
        };
        let last_read = self.last_read.lock().await.clone();
        let latency = self.latency.lock().await.clone();
        let summaries = link.handle().peers().await;
        let stats = link.handle().stats().await;
        let mut peers = Vec::with_capacity(summaries.len());
        for s in summaries {
            let discovery = self.discovery_of(db, &s.peer_id).await;
            if s.verified {
                let _ = db.peer_seen(&s.peer_id, &discovery);
            }
            let served = last_read
                .as_ref()
                .map(|v| v.peers.contains(&s.peer_id))
                .unwrap_or(false);
            let disputed = last_read
                .as_ref()
                .map(|v| v.disputed.contains(&s.peer_id))
                .unwrap_or(false);
            peers.push(PeerView {
                relays_chain: s.roles.iter().any(|r| r == "relay" || r == "bootstrap"),
                latency_ms: latency.get(&s.peer_id).copied(),
                transport: Self::transport_of(&s.address),
                direction: s.direction.to_owned(),
                connected_secs: s.connected_secs,
                discovery,
                served_last_read: served,
                agreed: if served {
                    last_read.as_ref().map(|v| v.agreed)
                } else if disputed {
                    Some(false)
                } else {
                    None
                },
                verified: s.verified,
                peer_id: s.peer_id,
                operator: s.operator,
                roles: s.roles,
            });
        }
        peers.sort_by(|a, b| b.verified.cmp(&a.verified).then(a.peer_id.cmp(&b.peer_id)));
        let rejected = link
            .rejected()
            .await
            .into_iter()
            .map(|r: RejectedPeer| RejectedView {
                peer_id: r.peer.to_string(),
                label: label_for_reason(&r.reason),
                reason: r.reason,
                at: r.at,
            })
            .collect();
        let peerstore_size =
            hashgram_p2p::peerstore::Peerstore::load(&crate::paths::peerstore_path())
                .map(|p| p.len())
                .unwrap_or(0);
        let uptime_secs = self
            .started
            .lock()
            .await
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        NetSnapshot {
            running: true,
            network,
            chain_id,
            genesis_hash,
            own_peer_id: link.handle().peer_id().to_string(),
            verified: peers.iter().filter(|p| p.verified).count(),
            peers,
            rejected,
            nat: stats
                .as_ref()
                .map(|s| s.reachability.to_owned())
                .unwrap_or_else(|| "unknown".into()),
            peerstore_size,
            builtin_seeds,
            kad_peers: stats.as_ref().map(|s| s.kad_peers).unwrap_or(0),
            last_read,
            uptime_secs,
            listen_addrs: stats
                .as_ref()
                .map(|s| s.listen_addrs.clone())
                .unwrap_or_default(),
            external_addrs: stats
                .as_ref()
                .map(|s| s.external_addrs.clone())
                .unwrap_or_default(),
        }
    }

    /// Relay peers grouped by operator, for the status bar.
    pub async fn relay_operator_count(&self) -> usize {
        let Some(link) = self.link().await else {
            return 0;
        };
        link.chain_relays()
            .await
            .into_iter()
            .map(|p| p.operator)
            .filter(|o| !o.is_empty())
            .collect::<HashSet<_>>()
            .len()
    }
}

/// Turns the swarm's reason string into the panel's label.
#[must_use]
pub fn label_for_reason(reason: &str) -> String {
    let r = reason.to_ascii_lowercase();
    if r.contains("genesis") || r.contains("chain") || r.contains("network") || r.contains("magic")
    {
        "wrong network".into()
    } else if r.contains("protocol") {
        "incompatible protocol version".into()
    } else if r.contains("timeout") {
        "handshake timed out".into()
    } else if r.contains("malformed") {
        "malformed handshake".into()
    } else {
        "rejected".into()
    }
}

/// A peer id string from a multiaddr, for tests and the panel.
#[must_use]
pub fn peer_id_from_multiaddr(addr: &str) -> Option<String> {
    peer_id_of(addr)
}

#[allow(dead_code)]
fn _assert_peer_id_type(_: PeerId) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_mismatch_is_labelled_wrong_network() {
        assert_eq!(label_for_reason("genesis hash mismatch"), "wrong network");
        assert_eq!(label_for_reason("chain id mismatch"), "wrong network");
        assert_eq!(
            label_for_reason("protocol major version 2"),
            "incompatible protocol version"
        );
        assert_eq!(label_for_reason("handshake timeout"), "handshake timed out");
    }

    #[test]
    fn mainnet_identity_is_pinned_to_the_compiled_in_genesis() {
        let s = Settings::default();
        let id = identity_for(&s).unwrap();
        assert!(id.is_mainnet());
        assert_eq!(
            id.genesis_hash,
            "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d"
        );
        assert_eq!(id.chain_id, "hashgram-1");
    }

    #[test]
    fn devnet_needs_a_well_formed_hash() {
        let mut s = Settings::default();
        s.network.kind = NetworkKind::Devnet;
        s.network.devnet_genesis_hash = "nope".into();
        assert!(identity_for(&s).is_err());
        s.network.devnet_genesis_hash = "a".repeat(64);
        assert!(!identity_for(&s).unwrap().is_mainnet());
    }

    #[test]
    fn transport_and_peer_id_parsing() {
        assert_eq!(
            peer_id_from_multiaddr("/ip4/1.2.3.4/udp/26670/quic-v1/p2p/12D3KooWabc").unwrap(),
            "12D3KooWabc"
        );
        assert_eq!(
            NetManager::transport_of("/ip4/1.2.3.4/udp/26670/quic-v1/p2p/x"),
            "QUIC"
        );
        assert_eq!(
            NetManager::transport_of("/ip4/1.2.3.4/tcp/26670/p2p/x"),
            "TCP"
        );
        assert_eq!(
            NetManager::transport_of("/ip4/1.2.3.4/tcp/26670/p2p/r/p2p-circuit/p2p/x"),
            "relayed"
        );
    }
}
