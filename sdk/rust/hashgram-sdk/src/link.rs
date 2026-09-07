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
}

/// The link.
pub struct Link {
    handle: NodeHandle,
    peers: Arc<RwLock<HashMap<PeerId, Vec<String>>>>,
    peerstore_path: Option<std::path::PathBuf>,
    _task: tokio::task::JoinHandle<()>,
    _events: tokio::task::JoinHandle<()>,
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
        cfg.listen_port = free_port();
        cfg.bootstrap_peers = bootstrap.iter().map(ToString::to_string).collect();
        cfg.min_peers = 2;
        cfg.serve_relay = Some(false);
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

        let peers: Arc<RwLock<HashMap<PeerId, Vec<String>>>> = Arc::default();
        let (first_tx, mut first_rx) = mpsc::channel::<()>(64);
        let peers_for_task = peers.clone();
        let inner = handle.clone();
        let events_task = tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    Event::PeerVerified { peer, roles } => {
                        debug!(%peer, ?roles, "verified");
                        peers_for_task.write().await.insert(peer, roles);
                        let _ = first_tx.try_send(());
                    }
                    Event::PeerDisconnected(peer) => {
                        peers_for_task.write().await.remove(&peer);
                    }
                    Event::PeerRejected { peer, reason } => {
                        info!(%peer, reason, "peer rejected");
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
        // node answered first.
        if !bootstrap.is_empty() {
            let deadline = tokio::time::Instant::now() + wait;
            let want = bootstrap.len();
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
            peerstore_path: peerstore_path.map(Path::to_path_buf),
            _task: task,
            _events: events_task,
        })
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
            .map(|(p, r)| KnownPeer {
                peer: *p,
                roles: r.clone(),
            })
            .collect()
    }

    /// Verified peers claiming a role.
    pub async fn peers_with_role(&self, role: &str) -> Vec<PeerId> {
        self.peers
            .read()
            .await
            .iter()
            .filter(|(_, r)| r.iter().any(|x| x == role))
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
    pub async fn providers(&self, key: Vec<u8>) -> Vec<PeerId> {
        self.handle.get_providers(key).await
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

    /// Stops the swarm, flushing the peerstore.
    pub async fn shutdown(self) {
        self.handle.shutdown().await;
        let _ = self.peerstore_path;
    }
}

use std::path::Path;

fn free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(0)
        .max(1)
}
