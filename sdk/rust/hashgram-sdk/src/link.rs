//! The network link: a light libp2p client that speaks to Hashgram nodes.
//!
//! A client is a swarm with no roles. It dials bootstrap peers, completes
//! the Hashgram handshake with them (so it knows it reached the right
//! network before trusting a byte), learns their roles, and from there uses
//! Kademlia to find providers and `AnnounceQuery` to find services. It never
//! trusts a node for content: every blob chunk is hash-checked, every social
//! event signature-checked, every message decrypted and authenticated by
//! MLS. What it trusts a node for is availability, and it spreads that trust
//! across several.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use hashgram_net::NetworkIdentity;
use hashgram_p2p::peerstore::Peerstore;
use hashgram_p2p::{Event, Keypair, Multiaddr, NodeConfig, NodeHandle, PeerId, RequestError};
use hashgram_proto::pb;
use prometheus_client::registry::Registry;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info};

/// How long a DHT provider lookup may take before the caller falls back to
/// the store nodes it is connected to. Alias of [`PROVIDER_QUERY_TIMEOUT`].
pub const PROVIDER_LOOKUP_TIMEOUT: Duration = PROVIDER_QUERY_TIMEOUT;

/// Why a link operation failed.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// Startup.
    #[error("link: {0}")]
    Start(String),
    /// No peer with the needed role is verified.
    #[error("no verified peer serves the {0} role; connect to a node that does")]
    NoPeer(&'static str),
    /// Request failure.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The node answered with an error body.
    #[error("node refused: {code}: {message}")]
    Refused {
        /// Code.
        code: String,
        /// Message.
        message: String,
    },
    /// Unexpected response body.
    #[error("unexpected response from node")]
    Unexpected,
}

/// A connected, verified peer and its roles.
#[derive(Debug, Clone)]
pub struct KnownPeer {
    /// Peer id.
    pub peer: PeerId,
    /// Roles it claimed at the handshake.
    pub roles: Vec<String>,
    /// Provider operator address it claimed; receipts are signed for it.
    pub operator: String,
}

/// A verified peer with its measured network distance.
#[derive(Debug, Clone)]
pub struct RankedPeer {
    /// The peer.
    pub peer: KnownPeer,
    /// Smoothed ping round-trip in milliseconds, once measured.
    pub rtt_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct PeerInfo {
    roles: Vec<String>,
    operator: String,
}

/// A peer that failed the Hashgram handshake, and why. Kept so a UI can
/// list "wrong network" nodes greyed out with the reason instead of
/// retrying them silently.
#[derive(Debug, Clone)]
pub struct RejectedPeer {
    /// Peer id.
    pub peer: PeerId,
    /// The swarm's reason string (e.g. genesis mismatch).
    pub reason: String,
    /// Seconds since the Unix epoch when it was rejected.
    pub at: u64,
    /// How many times in a row this peer was rejected for the same reason.
    pub count: u32,
}

/// How long a client waits for DHT provider records before proceeding
/// with its connected store peers.
pub const PROVIDER_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// The link.
pub struct Link {
    handle: NodeHandle,
    peers: Arc<RwLock<HashMap<PeerId, PeerInfo>>>,
    rejected: Arc<RwLock<Vec<RejectedPeer>>>,
    peerstore_path: Option<std::path::PathBuf>,
    _task: tokio::task::JoinHandle<()>,
    _events: tokio::task::JoinHandle<()>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Link {
    /// Starts a client swarm and dials `bootstrap`. Waits up to `wait` for
    /// the first verified peer.
    pub async fn connect(
        identity: &NetworkIdentity,
        bootstrap: &[Multiaddr],
        peerstore_path: Option<&std::path::Path>,
        wait: Duration,
    ) -> Result<Self, LinkError> {
        let mut cfg: NodeConfig =
            toml::from_str("").map_err(|e| LinkError::Start(e.to_string()))?;
        cfg.network = if identity.is_mainnet() {
            "mainnet".into()
        } else {
            "devnet".into()
        };
        cfg.genesis_hash = identity.genesis_hash.clone();
        cfg.listen_addr = "0.0.0.0"
            .parse()
            .map_err(|_| LinkError::Start("listen addr".into()))?;
        // Port 0: the kernel picks a free port for each transport. A client
        // advertises nothing, so TCP and QUIC need not share a number, and
        // picking one "free" TCP port and hoping the same UDP port was free
        // (as this did before) made the swarm fail to start whenever it was
        // not, which on Windows is common enough to notice.
        cfg.listen_port = 0;
        cfg.bootstrap_peers = bootstrap.iter().map(ToString::to_string).collect();
        cfg.min_peers = 2;
        cfg.serve_relay = Some(false);
        // A client behind NAT keeps a TCP connection far longer than a QUIC
        // one (see `NodeConfig::prefer_tcp`).
        cfg.prefer_tcp = true;
        if let Some(p) = peerstore_path {
            cfg.peerstore_path = p.display().to_string();
        }
        let peerstore = match peerstore_path {
            Some(p) => Peerstore::load(p).map_err(|e| LinkError::Start(e.to_string()))?,
            None => Peerstore::in_memory(),
        };
        // A client key is ephemeral: a client has no reputation to keep and
        // a stable id would let nodes correlate sessions.
        let key = Keypair::generate_ed25519();
        let mut registry = Registry::default();
        let (handle, mut events, task) =
            hashgram_p2p::start(cfg, identity.clone(), key, peerstore, &mut registry)
                .map_err(|e| LinkError::Start(e.to_string()))?;

        let peers: Arc<RwLock<HashMap<PeerId, PeerInfo>>> = Arc::default();
        let rejected: Arc<RwLock<Vec<RejectedPeer>>> = Arc::default();
        let (first_tx, mut first_rx) = mpsc::channel::<()>(64);
        let peers_for_task = peers.clone();
        let rejected_for_task = rejected.clone();
        let inner = handle.clone();
        let events_task = tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    Event::PeerVerified {
                        peer,
                        roles,
                        operator,
                    } => {
                        debug!(%peer, ?roles, operator, "verified");
                        peers_for_task
                            .write()
                            .await
                            .insert(peer, PeerInfo { roles, operator });
                        let _ = first_tx.try_send(());
                    }
                    Event::PeerDisconnected(peer) => {
                        peers_for_task.write().await.remove(&peer);
                    }
                    Event::PeerRejected { peer, reason } => {
                        info!(%peer, reason, "peer rejected");
                        let mut r = rejected_for_task.write().await;
                        let count = r
                            .iter()
                            .find(|x| x.peer == peer && x.reason == reason)
                            .map_or(1, |x| x.count.saturating_add(1));
                        r.retain(|x| x.peer != peer);
                        r.push(RejectedPeer {
                            peer,
                            reason,
                            at: unix_now(),
                            count,
                        });
                        // Bounded: a flood of strangers cannot grow this.
                        if r.len() > 64 {
                            r.remove(0);
                        }
                    }
                    Event::InboundRequest { channel, .. } => {
                        // A client serves nothing.
                        inner
                            .respond(
                                channel,
                                pb::Response {
                                    body: Some(pb::response::Body::Error(pb::Error {
                                        code: "unsupported".into(),
                                        message: "this is a client".into(),
                                    })),
                                },
                            )
                            .await;
                    }
                    Event::Gossip { id, source, .. } => {
                        inner
                            .report_gossip(id, source, hashgram_p2p::MessageAcceptance::Ignore)
                            .await;
                    }
                    Event::Listening(_) | Event::ExternalAddr(_) => {}
                }
            }
        });

        // Wait for every bootstrap peer to verify (or the deadline), so the
        // first operation sees the whole set of roles rather than whichever
        // node answered first. Several addresses of one peer (QUIC and TCP
        // of the same node) count once, or a single-node network would
        // always wait out the full deadline.
        if !bootstrap.is_empty() {
            let deadline = tokio::time::Instant::now() + wait;
            let want = distinct_peer_ids(bootstrap);
            loop {
                if peers.read().await.len() >= want {
                    break;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    break;
                }
                if tokio::time::timeout(remaining, first_rx.recv())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
        Ok(Self {
            handle,
            peers,
            rejected,
            peerstore_path: peerstore_path.map(Path::to_path_buf),
            _task: task,
            _events: events_task,
        })
    }

    /// Peers that failed the handshake (wrong network, wrong protocol,
    /// malformed), most recent last.
    pub async fn rejected(&self) -> Vec<RejectedPeer> {
        self.rejected.read().await.clone()
    }

    /// Peers whose handshake failed at the transport (the connection was
    /// lost mid-handshake rather than refused) within the last `within`.
    /// A node that keeps hanging up on a fresh connection usually still
    /// counts this client's previous, dead connection against it; a new
    /// link (new ephemeral identity) gets through at once, so the desktop
    /// watchdog uses this to reconnect early instead of waiting.
    pub async fn transport_rejections_within(&self, within: Duration) -> usize {
        let now = unix_now();
        self.rejected
            .read()
            .await
            .iter()
            .filter(|r| {
                now.saturating_sub(r.at) <= within.as_secs()
                    && r.reason.contains("handshake transport failure")
            })
            .map(|r| r.count as usize)
            .sum()
    }

    /// The swarm handle.
    #[must_use]
    pub fn handle(&self) -> &NodeHandle {
        &self.handle
    }

    /// Verified peers.
    pub async fn peers(&self) -> Vec<KnownPeer> {
        self.peers
            .read()
            .await
            .iter()
            .map(|(p, i)| KnownPeer {
                peer: *p,
                roles: i.roles.clone(),
                operator: i.operator.clone(),
            })
            .collect()
    }

    /// Verified peers ordered nearest first: by measured ping round-trip
    /// (peers not yet measured come last), with store nodes preferred at
    /// equal distance because they hold the public log. This is the whole
    /// notion of "nodes near me" — network distance, not geography; the
    /// client never geolocates anyone.
    pub async fn peers_ranked(&self) -> Vec<RankedPeer> {
        let known = self.peers().await;
        if known.is_empty() {
            return Vec::new();
        }
        let live = self.handle.peers().await;
        let mut out: Vec<RankedPeer> = known
            .into_iter()
            .map(|k| {
                let rtt_ms = live
                    .iter()
                    .find(|s| s.peer_id == k.peer.to_string())
                    .and_then(|s| s.rtt_ms);
                RankedPeer { peer: k, rtt_ms }
            })
            .collect();
        out.sort_by(|a, b| {
            let da = a.rtt_ms.unwrap_or(u64::MAX);
            let db = b.rtt_ms.unwrap_or(u64::MAX);
            da.cmp(&db).then_with(|| {
                let sa = a.peer.roles.iter().any(|r| r == "store");
                let sb = b.peer.roles.iter().any(|r| r == "store");
                sb.cmp(&sa)
            })
        });
        out
    }

    /// The operator address a peer claimed, if any.
    pub async fn operator_of(&self, peer: PeerId) -> Option<String> {
        self.peers
            .read()
            .await
            .get(&peer)
            .map(|i| i.operator.clone())
            .filter(|o| !o.is_empty())
    }

    /// Signs and delivers a service receipt to the peer that served us.
    /// Silent on failure: a receipt is the provider's reward, not the
    /// client's problem, and a client must never be blocked by it.
    pub async fn deliver_receipt(
        &self,
        chain: &hashgram_chain::Client,
        network: &NetworkIdentity,
        device: &hashgram_proto::Ed25519Signer,
        peer: PeerId,
        role: i32,
        units: u64,
    ) {
        let Some(operator) = self.operator_of(peer).await else {
            return;
        };
        if units == 0 {
            return;
        }
        let (epoch, height) = match (
            chain.query("hashgram/serviceproof/v1/epoch/current").await,
            chain.height().await,
        ) {
            (Ok(v), Ok(h)) => (
                v.get("epoch")
                    .and_then(|e| e.get("number"))
                    .and_then(|n| n.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0),
                h,
            ),
            _ => return,
        };
        let mut nonce = [0u8; 8];
        let _ = getrandom::fill(&mut nonce);
        let mut r = pb::ServiceReceipt {
            provider: operator,
            role,
            epoch,
            nonce: u64::from_le_bytes(nonce),
            units,
            expiry_height: (height + 600) as i64,
            ..Default::default()
        };
        if hashgram_proto::signing::sign_receipt(network, device, &mut r).is_err() {
            return;
        }
        match self
            .request(
                peer,
                pb::request::Body::ReceiptDeliver(pb::ReceiptDeliver { receipt: Some(r) }),
            )
            .await
        {
            Ok(pb::response::Body::ReceiptDeliver(res)) if res.accepted => {
                debug!(%peer, units, role, "receipt delivered")
            }
            Ok(pb::response::Body::ReceiptDeliver(res)) => {
                debug!(%peer, reason = res.reason, "receipt refused")
            }
            Ok(_) => {}
            Err(e) => debug!(%peer, error = %e, "receipt delivery failed"),
        }
    }

    /// Verified peers claiming a role.
    pub async fn peers_with_role(&self, role: &str) -> Vec<PeerId> {
        self.peers
            .read()
            .await
            .iter()
            .filter(|(_, i)| i.roles.iter().any(|x| x == role))
            .map(|(p, _)| *p)
            .collect()
    }

    /// Any verified peer (for queries every node answers).
    pub async fn any_peer(&self) -> Option<PeerId> {
        self.peers.read().await.keys().next().copied()
    }

    /// Sends a request to a specific peer, unwrapping error bodies.
    pub async fn request(
        &self,
        peer: PeerId,
        body: pb::request::Body,
    ) -> Result<pb::response::Body, LinkError> {
        let resp = self
            .handle
            .request(peer, vec![], pb::Request { body: Some(body) })
            .await?;
        match resp.body {
            Some(pb::response::Body::Error(e)) => Err(LinkError::Refused {
                code: e.code,
                message: e.message,
            }),
            Some(b) => Ok(b),
            None => Err(LinkError::Unexpected),
        }
    }

    /// Sends a request to a peer we may not be connected to, dialling the
    /// given addresses.
    pub async fn request_at(
        &self,
        peer: PeerId,
        addrs: Vec<Multiaddr>,
        body: pb::request::Body,
    ) -> Result<pb::response::Body, LinkError> {
        let resp = self
            .handle
            .request(peer, addrs, pb::Request { body: Some(body) })
            .await?;
        match resp.body {
            Some(pb::response::Body::Error(e)) => Err(LinkError::Refused {
                code: e.code,
                message: e.message,
            }),
            Some(b) => Ok(b),
            None => Err(LinkError::Unexpected),
        }
    }

    /// Sends a request to the first peer with a role that answers.
    pub async fn request_role(
        &self,
        role: &'static str,
        body: pb::request::Body,
    ) -> Result<(PeerId, pb::response::Body), LinkError> {
        let peers = self.peers_with_role(role).await;
        if peers.is_empty() {
            return Err(LinkError::NoPeer(role));
        }
        let mut last = LinkError::NoPeer(role);
        for p in peers {
            match self.request(p, body.clone()).await {
                Ok(b) => return Ok((p, b)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Providers of a DHT key, from Kademlia. Unverified peers among them
    /// will be verified at connection before any request is served.
    ///
    /// Bounded: a Kademlia query waits for every routing-table peer it
    /// asked, up to the 30 s query timeout, and a send or a mailbox sync
    /// that hangs that long because one stale peer is silent is worse than
    /// falling back to the connected store nodes, which every caller does.
    pub async fn providers(&self, key: Vec<u8>) -> Vec<PeerId> {
        // A client never waits for a full Kademlia walk: with few peers the
        // walk lasts until the 30 s query timeout, and every caller falls
        // back to its connected store peers. Whatever is known within the
        // bound is returned; nothing is an acceptable answer.
        tokio::time::timeout(PROVIDER_QUERY_TIMEOUT, self.handle.get_providers(key))
            .await
            .unwrap_or_default()
    }

    /// Node announcements from any verified peer.
    pub async fn announcements(&self, roles: &[&str]) -> Result<Vec<pb::NodeAnnounce>, LinkError> {
        let Some(peer) = self.any_peer().await else {
            return Err(LinkError::NoPeer("any"));
        };
        match self
            .request(
                peer,
                pb::request::Body::AnnounceQuery(pb::AnnounceQuery {
                    roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                    limit: 50,
                }),
            )
            .await?
        {
            pb::response::Body::AnnounceQuery(r) => Ok(r.announcements),
            _ => Err(LinkError::Unexpected),
        }
    }

    /// Verified peers that relay chain queries (`relay` or `bootstrap`
    /// role), with their operator addresses.
    pub async fn chain_relays(&self) -> Vec<KnownPeer> {
        self.peers()
            .await
            .into_iter()
            .filter(|p| p.roles.iter().any(|r| r == "relay" || r == "bootstrap"))
            .collect()
    }

    /// One allow-listed chain read through `peer`. `path` has no leading
    /// slash and no query string; `query` has no `?`. Returns the gateway's
    /// answer verbatim (status, body, height).
    pub async fn chain_get(
        &self,
        peer: PeerId,
        path: &str,
        query: &str,
    ) -> Result<ChainAnswer, LinkError> {
        match self
            .request(
                peer,
                pb::request::Body::ChainQuery(pb::ChainQuery {
                    path: path.to_owned(),
                    query: query.to_owned(),
                }),
            )
            .await?
        {
            pb::response::Body::ChainQuery(r) => Ok(ChainAnswer {
                status: u16::try_from(r.status).unwrap_or(u16::MAX),
                body: r.body,
                height: r.height,
            }),
            _ => Err(LinkError::Unexpected),
        }
    }

    /// Hands a signed transaction to `peer` for `POST /cosmos/tx/v1beta1/txs`
    /// (sync mode). The peer never sees a key.
    pub async fn chain_broadcast(
        &self,
        peer: PeerId,
        tx_bytes: Vec<u8>,
    ) -> Result<ChainAnswer, LinkError> {
        match self
            .request(
                peer,
                pb::request::Body::ChainBroadcast(pb::ChainBroadcast { tx_bytes }),
            )
            .await?
        {
            pb::response::Body::ChainBroadcast(r) => Ok(ChainAnswer {
                status: u16::try_from(r.status).unwrap_or(u16::MAX),
                body: r.body,
                height: r.height,
            }),
            _ => Err(LinkError::Unexpected),
        }
    }

    /// Stops the swarm, flushing the peerstore.
    pub async fn shutdown(&self) {
        self.handle.shutdown().await;
        let _ = &self.peerstore_path;
    }
}

/// A chain gateway answer relayed by a node, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainAnswer {
    /// HTTP status the gateway returned.
    pub status: u16,
    /// Body bytes as served.
    pub body: Vec<u8>,
    /// Block height the gateway reported, 0 when absent.
    pub height: u64,
}

use std::path::Path;

/// Counts distinct `/p2p/<id>` components among bootstrap addresses;
/// addresses without one count individually.
fn distinct_peer_ids(addrs: &[Multiaddr]) -> usize {
    let mut ids = std::collections::HashSet::new();
    let mut anonymous = 0usize;
    for a in addrs {
        let id = a.iter().find_map(|p| match p {
            hashgram_p2p::Protocol::P2p(id) => Some(id),
            _ => None,
        });
        match id {
            Some(id) => {
                ids.insert(id);
            }
            None => anonymous += 1,
        }
    }
    ids.len() + anonymous
}
