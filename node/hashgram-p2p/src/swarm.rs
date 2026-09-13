//! The swarm event loop.
//!
//! One task owns the libp2p `Swarm` and everything that must be consistent
//! with it: the verified-peer set, the scoreboard, the connection table and
//! the peerstore. Everything else talks to it through [`NodeHandle`], which
//! sends [`Command`]s and receives [`Event`]s. This is the usual actor shape
//! and it exists for the usual reason: a swarm is not `Sync`, and a
//! handshake decision that raced with a request handler would be a security
//! bug, not a performance one.
//!
//! # The handshake gate
//!
//! No application request is served, and no gossip is accepted, from a peer
//! that has not completed the Hashgram handshake on this connection's
//! lifetime. The handshake is itself a request on `/hashgram/rpc/1`, sent by
//! the dialer as soon as the connection is established and answered by the
//! listener with its own identity. Both sides verify all five identity parts.
//! A peer that fails is disconnected, scored down and blocked for an hour; a
//! peer that never sends one is disconnected after a grace period.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use hashgram_net::{Handshake, HandshakeError, NetworkIdentity};
use hashgram_proto::pb;
use libp2p::core::ConnectedPoint;
use libp2p::gossipsub::{self, IdentTopic, MessageAcceptance, MessageId, TopicHash};
use libp2p::identity::Keypair;
use libp2p::kad;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ResponseChannel};
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::{ConnectionId, DialError, SwarmEvent};
use libp2p::{autonat, identify, noise, yamux, Multiaddr, PeerId, Swarm};
use prometheus_client::registry::Registry;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::behaviour::{Behaviour, BehaviourEvent};
use crate::config::NodeConfig;
use crate::limits::ConnectionLimits;
use crate::metrics::{request_kind, KindLabel, Metrics, ReasonCode, ReasonLabel, TopicLabel};
use crate::peerstore::Peerstore;
use crate::score::{ScoreEvent, Scoreboard};

/// How long an unverified peer may stay connected.
const HANDSHAKE_GRACE: Duration = Duration::from_secs(10);
/// How long a failed handshake keeps a peer blocked.
const BAN_DURATION: Duration = Duration::from_secs(3600);
/// How long a queued request waits for the peer to verify before failing.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(20);
/// Inbound requests per peer per second, sustained.
const RATE_PER_SEC: f64 = 40.0;
/// Inbound request burst per peer.
const RATE_BURST: f64 = 80.0;
/// How often the maintenance tick runs.
const TICK: Duration = Duration::from_secs(1);
/// How often the peerstore is flushed.
const PEERSTORE_FLUSH: Duration = Duration::from_secs(60);
/// Shortest interval between two rounds of bootstrap dialling while the
/// node is below `min_peers`. A round is cheap once peers are connected
/// (they are skipped), but a peer that is down would otherwise be dialled
/// every second.
const BOOTSTRAP_RETRY: Duration = Duration::from_secs(3);
/// Shortest interval between two Kademlia bootstrap queries started from
/// the tick (Kademlia also runs its own every five minutes).
const KAD_BOOTSTRAP: Duration = Duration::from_secs(60);

/// A request the swarm could not complete.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RequestError {
    /// The peer could not be reached.
    #[error("peer unreachable: {0}")]
    Unreachable(String),
    /// The peer did not verify in time.
    #[error("peer did not complete the Hashgram handshake")]
    NotVerified,
    /// Transport-level failure after the request was sent.
    #[error("request failed: {0}")]
    Failed(String),
    /// The swarm has shut down.
    #[error("the node is shutting down")]
    Shutdown,
}

/// A connected peer, as reported to operators.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerSummary {
    /// Peer id.
    pub peer_id: String,
    /// Remote address of the connection.
    pub address: String,
    /// Whether the Hashgram handshake completed.
    pub verified: bool,
    /// Roles the peer claimed.
    pub roles: Vec<String>,
    /// Provider operator address the peer claimed.
    pub operator: String,
    /// `inbound` or `outbound`.
    pub direction: &'static str,
    /// Local score.
    pub score: i32,
    /// Seconds connected.
    pub connected_secs: u64,
    /// Agent version from identify, if received.
    pub agent: String,
    /// Smoothed round-trip time from the liveness pings, in milliseconds;
    /// `None` until the first ping answers. The only notion of "near" the
    /// network has: no addresses are geolocated.
    pub rtt_ms: Option<u64>,
}

/// A snapshot of the swarm for the status API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Stats {
    /// This node's peer id.
    pub peer_id: String,
    /// Addresses being listened on.
    pub listen_addrs: Vec<String>,
    /// Addresses confirmed reachable from outside.
    pub external_addrs: Vec<String>,
    /// Connected peers.
    pub connected: usize,
    /// Verified peers.
    pub verified: usize,
    /// Peers in the persistent store.
    pub known: usize,
    /// Peers currently blocked.
    pub banned: usize,
    /// Kademlia routing table size.
    pub kad_peers: usize,
    /// Autonat status: `public`, `private` or `unknown`.
    pub reachability: &'static str,
    /// Subscribed topics.
    pub topics: Vec<String>,
}

/// Something the application asked the swarm to do.
pub enum Command {
    /// Dial an address.
    Dial(Multiaddr, oneshot::Sender<Result<(), String>>),
    /// Send a request to a peer, dialling `addrs` if needed, waiting for
    /// the handshake if the peer is not yet verified.
    Request {
        /// The peer.
        peer: PeerId,
        /// Addresses to try if not connected.
        addrs: Vec<Multiaddr>,
        /// The request.
        request: pb::Request,
        /// Where the response goes.
        reply: oneshot::Sender<Result<pb::Response, RequestError>>,
    },
    /// Answer an inbound request.
    Respond(ResponseChannel<pb::Response>, pb::Response),
    /// Subscribe to a gossip topic.
    Subscribe(String),
    /// Unsubscribe from a gossip topic.
    Unsubscribe(String),
    /// Publish on a topic.
    Publish(String, Vec<u8>, oneshot::Sender<Result<(), String>>),
    /// Report the application's verdict on a gossip message.
    ReportGossip(MessageId, PeerId, MessageAcceptance),
    /// Announce this node as a provider of a key.
    StartProviding(Vec<u8>, oneshot::Sender<Result<(), String>>),
    /// Stop providing a key.
    StopProviding(Vec<u8>),
    /// Find providers of a key.
    GetProviders(Vec<u8>, oneshot::Sender<Vec<PeerId>>),
    /// Addresses known for a peer.
    AddrsOf(PeerId, oneshot::Sender<Vec<Multiaddr>>),
    /// Score a peer for something the application observed.
    Score(PeerId, ScoreEvent),
    /// Block a peer for the ban duration.
    Ban(PeerId),
    /// List peers.
    Peers(oneshot::Sender<Vec<PeerSummary>>),
    /// Stats snapshot.
    Stats(oneshot::Sender<Stats>),
    /// Verified peers claiming a role, with known addresses.
    PeersWithRole(String, oneshot::Sender<Vec<(PeerId, Vec<Multiaddr>)>>),
    /// Flush the peerstore and stop.
    Shutdown,
}

/// Something the swarm tells the application.
#[derive(Debug)]
pub enum Event {
    /// A peer completed the handshake.
    PeerVerified {
        /// The peer.
        peer: PeerId,
        /// Roles it claimed.
        roles: Vec<String>,
        /// Provider operator address it claimed (empty if none). Receipts
        /// signed for it are only worth anything if the chain agrees.
        operator: String,
    },
    /// A peer was refused at the handshake.
    PeerRejected {
        /// The peer.
        peer: PeerId,
        /// Why.
        reason: String,
    },
    /// A verified peer disconnected.
    PeerDisconnected(PeerId),
    /// A verified peer sent a request. Answer with [`Command::Respond`].
    InboundRequest {
        /// The peer.
        peer: PeerId,
        /// The request.
        request: pb::Request,
        /// Reply channel.
        channel: ResponseChannel<pb::Response>,
    },
    /// A gossip message from a verified peer. Report a verdict with
    /// [`Command::ReportGossip`].
    Gossip {
        /// Topic.
        topic: String,
        /// The peer that forwarded it.
        source: PeerId,
        /// Message id, for the verdict.
        id: MessageId,
        /// Payload.
        data: Vec<u8>,
    },
    /// The node is listening on an address.
    Listening(Multiaddr),
    /// An external address was confirmed.
    ExternalAddr(Multiaddr),
}

/// The application's handle to the swarm.
#[derive(Clone)]
pub struct NodeHandle {
    tx: mpsc::Sender<Command>,
    peer_id: PeerId,
}

impl NodeHandle {
    /// This node's peer id.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    async fn send(&self, cmd: Command) -> Result<(), RequestError> {
        self.tx.send(cmd).await.map_err(|_| RequestError::Shutdown)
    }

    /// Sends a request and waits for the response.
    pub async fn request(
        &self,
        peer: PeerId,
        addrs: Vec<Multiaddr>,
        request: pb::Request,
    ) -> Result<pb::Response, RequestError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Request {
            peer,
            addrs,
            request,
            reply,
        })
        .await?;
        rx.await.map_err(|_| RequestError::Shutdown)?
    }

    /// Answers an inbound request.
    pub async fn respond(&self, channel: ResponseChannel<pb::Response>, response: pb::Response) {
        let _ = self.tx.send(Command::Respond(channel, response)).await;
    }

    /// Dials an address.
    pub async fn dial(&self, addr: Multiaddr) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Dial(addr, reply))
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|_| "shutdown".to_owned())?
    }

    /// Subscribes to a topic.
    pub async fn subscribe(&self, topic: &str) {
        let _ = self.tx.send(Command::Subscribe(topic.to_owned())).await;
    }

    /// Publishes to a topic.
    pub async fn publish(&self, topic: &str, data: Vec<u8>) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Publish(topic.to_owned(), data, reply))
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|_| "shutdown".to_owned())?
    }

    /// Reports a gossip verdict.
    pub async fn report_gossip(
        &self,
        id: MessageId,
        source: PeerId,
        acceptance: MessageAcceptance,
    ) {
        let _ = self
            .tx
            .send(Command::ReportGossip(id, source, acceptance))
            .await;
    }

    /// Announces this node as a provider of `key`.
    pub async fn start_providing(&self, key: Vec<u8>) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::StartProviding(key, reply))
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|_| "shutdown".to_owned())?
    }

    /// Stops providing `key`.
    pub async fn stop_providing(&self, key: Vec<u8>) {
        let _ = self.tx.send(Command::StopProviding(key)).await;
    }

    /// Finds providers of `key`.
    pub async fn get_providers(&self, key: Vec<u8>) -> Vec<PeerId> {
        let (reply, rx) = oneshot::channel();
        if self.send(Command::GetProviders(key, reply)).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Addresses known for a peer.
    pub async fn addrs_of(&self, peer: PeerId) -> Vec<Multiaddr> {
        let (reply, rx) = oneshot::channel();
        if self.send(Command::AddrsOf(peer, reply)).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Scores a peer.
    pub async fn score(&self, peer: PeerId, event: ScoreEvent) {
        let _ = self.tx.send(Command::Score(peer, event)).await;
    }

    /// Bans a peer.
    pub async fn ban(&self, peer: PeerId) {
        let _ = self.tx.send(Command::Ban(peer)).await;
    }

    /// Lists connected peers.
    pub async fn peers(&self) -> Vec<PeerSummary> {
        let (reply, rx) = oneshot::channel();
        if self.send(Command::Peers(reply)).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Verified peers claiming a role.
    pub async fn peers_with_role(&self, role: &str) -> Vec<(PeerId, Vec<Multiaddr>)> {
        let (reply, rx) = oneshot::channel();
        if self
            .send(Command::PeersWithRole(role.to_owned(), reply))
            .await
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    /// Stats snapshot.
    pub async fn stats(&self) -> Option<Stats> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::Stats(reply)).await.ok()?;
        rx.await.ok()
    }

    /// Stops the swarm.
    pub async fn shutdown(&self) {
        let _ = self.tx.send(Command::Shutdown).await;
    }
}

/// Why the swarm could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// Transport construction.
    #[error("transport: {0}")]
    Transport(String),
    /// Behaviour construction.
    #[error("behaviour: {0}")]
    Behaviour(String),
    /// Listening failed.
    #[error("listen on {addr}: {reason}")]
    Listen {
        /// The address.
        addr: String,
        /// Why.
        reason: String,
    },
}

struct Verified {
    roles: Vec<String>,
    operator: String,
}

struct Connected {
    endpoint: ConnectedPoint,
    since: Instant,
}

struct Queued {
    request: pb::Request,
    reply: oneshot::Sender<Result<pb::Response, RequestError>>,
    since: Instant,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

const PING_FAILURES_BEFORE_DISCONNECT: u8 = 3;

fn ping_streak_requires_disconnect(streak: &mut u8) -> bool {
    *streak = streak.saturating_add(1);
    *streak >= PING_FAILURES_BEFORE_DISCONNECT
}

/// Exponentially weighted round-trip estimate (α = 1/4, like TCP's SRTT):
/// one slow ping does not make a near node look far.
fn smooth_rtt(previous_ms: f64, sample_ms: f64) -> f64 {
    previous_ms * 0.75 + sample_ms * 0.25
}

/// The actor.
struct Runner {
    swarm: Swarm<Behaviour>,
    identity: NetworkIdentity,
    cfg: NodeConfig,
    rx: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    metrics: Metrics,

    connected: HashMap<PeerId, Connected>,
    verified: HashMap<PeerId, Verified>,
    unverified_since: HashMap<PeerId, Instant>,
    identified: HashMap<PeerId, (Vec<Multiaddr>, String)>,
    pending_handshakes: HashMap<OutboundRequestId, PeerId>,
    pending_requests:
        HashMap<OutboundRequestId, oneshot::Sender<Result<pb::Response, RequestError>>>,
    queued: HashMap<PeerId, VecDeque<Queued>>,
    pending_providers: HashMap<kad::QueryId, (HashSet<PeerId>, oneshot::Sender<Vec<PeerId>>)>,
    banned: HashMap<PeerId, Instant>,
    /// Peers whose handshake we refused and are waiting to tell so before
    /// disconnecting. Closing first would swallow the reason.
    pending_reject: HashMap<PeerId, (ReasonCode, String, Instant)>,
    buckets: HashMap<PeerId, Bucket>,
    /// Consecutive failed liveness probes for verified peers. A Windows
    /// adapter change can leave a half-open connection without a close event.
    ping_failures: HashMap<PeerId, u8>,
    /// Smoothed ping round-trip per peer (EWMA, milliseconds).
    rtt: HashMap<PeerId, f64>,
    /// Connections refused by the limits and closed before being counted.
    refused: HashSet<ConnectionId>,
    /// Dial attempts per peer since start, for TCP/QUIC alternation.
    dial_attempts: HashMap<PeerId, u32>,
    limits: ConnectionLimits,
    scores: Scoreboard,
    peerstore: Peerstore,
    topics: HashSet<String>,
    external_addrs: Vec<Multiaddr>,
    reachability: &'static str,
    bootstrap_candidates: Vec<Multiaddr>,
    last_peerstore_flush: Instant,
    last_bootstrap_dial: Instant,
    last_kad_bootstrap: Instant,
}

/// Starts the swarm. Returns the handle, the event stream and the task.
pub fn start(
    cfg: NodeConfig,
    identity: NetworkIdentity,
    keypair: Keypair,
    peerstore: Peerstore,
    registry: &mut Registry,
) -> Result<
    (
        NodeHandle,
        mpsc::Receiver<Event>,
        tokio::task::JoinHandle<()>,
    ),
    StartError,
> {
    let metrics = Metrics::register(registry);
    let serve_relay = cfg.serves_relay();
    let network_id = identity.network_id.clone();
    let cfg_for_behaviour = cfg.clone();

    // QUIC first, then TCP through the wrapper in `transport.rs` (Windows
    // TCP dials must not reuse the listening port; see that module).
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_other_transport(crate::transport::tcp_transport)
        .map_err(|e| StartError::Transport(e.to_string()))?
        .with_dns()
        .map_err(|e| StartError::Transport(e.to_string()))?
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .map_err(|e| StartError::Transport(e.to_string()))?
        .with_bandwidth_metrics(registry)
        .with_behaviour(|key, relay_client| {
            Behaviour::new(
                key,
                &cfg_for_behaviour,
                &network_id,
                relay_client,
                serve_relay,
            )
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
        })
        .map_err(|e| StartError::Behaviour(e.to_string()))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
        .build();

    let peer_id = *swarm.local_peer_id();

    let ip = cfg.listen_addr;
    let ip_proto = match ip {
        IpAddr::V4(a) => Protocol::Ip4(a),
        IpAddr::V6(a) => Protocol::Ip6(a),
    };
    let mut listen: Vec<Multiaddr> = Vec::new();
    if matches!(
        cfg.transport,
        crate::config::Transport::Quic | crate::config::Transport::Both
    ) {
        listen.push(
            Multiaddr::empty()
                .with(ip_proto.clone())
                .with(Protocol::Udp(cfg.listen_port))
                .with(Protocol::QuicV1),
        );
    }
    if matches!(
        cfg.transport,
        crate::config::Transport::Tcp | crate::config::Transport::Both
    ) {
        listen.push(
            Multiaddr::empty()
                .with(ip_proto)
                .with(Protocol::Tcp(cfg.listen_port)),
        );
    }
    // One transport failing to bind (a UDP port already taken, a firewall
    // policy refusing QUIC) must not take the other down with it: a node
    // that can still speak TCP is a node. Only when nothing listens is it
    // an error.
    let mut listening = 0usize;
    let mut last_listen_error: Option<StartError> = None;
    for addr in listen {
        match swarm.listen_on(addr.clone()) {
            Ok(_) => listening += 1,
            Err(e) => {
                warn!(%addr, error = %e, "listen failed; continuing with the other transport");
                last_listen_error = Some(StartError::Listen {
                    addr: addr.to_string(),
                    reason: e.to_string(),
                });
            }
        }
    }
    if listening == 0 {
        return Err(last_listen_error.unwrap_or(StartError::Listen {
            addr: String::new(),
            reason: "no transport enabled".into(),
        }));
    }
    for a in &cfg.announce_addrs {
        if let Ok(ma) = a.parse::<Multiaddr>() {
            swarm.add_external_address(ma);
        }
    }

    let mut bootstrap_candidates: Vec<Multiaddr> = cfg
        .bootstrap_peers
        .iter()
        .filter_map(|a| a.parse().ok())
        .collect();
    for (_, addr) in peerstore.candidates(32) {
        bootstrap_candidates.push(addr);
    }
    for name in &cfg.dnsaddr {
        if let Ok(ma) = format!("/dnsaddr/{name}").parse::<Multiaddr>() {
            bootstrap_candidates.push(ma);
        }
    }
    bootstrap_candidates.extend(load_bootstrap_records(&cfg, &identity));

    let limits = ConnectionLimits::new().with_limits(
        cfg.max_connections_per_peer,
        cfg.max_connections_per_subnet,
        cfg.max_inbound_connections,
        cfg.max_outbound_connections,
    );

    let (tx, rx) = mpsc::channel(1024);
    let (events, events_rx) = mpsc::channel(1024);

    let runner = Runner {
        swarm,
        identity,
        cfg,
        rx,
        events,
        metrics,
        connected: HashMap::new(),
        verified: HashMap::new(),
        unverified_since: HashMap::new(),
        identified: HashMap::new(),
        pending_handshakes: HashMap::new(),
        pending_requests: HashMap::new(),
        queued: HashMap::new(),
        pending_providers: HashMap::new(),
        banned: HashMap::new(),
        pending_reject: HashMap::new(),
        buckets: HashMap::new(),
        ping_failures: HashMap::new(),
        rtt: HashMap::new(),
        refused: HashSet::new(),
        dial_attempts: HashMap::new(),
        limits,
        scores: Scoreboard::new(),
        peerstore,
        topics: HashSet::new(),
        external_addrs: Vec::new(),
        reachability: "unknown",
        bootstrap_candidates,
        last_peerstore_flush: Instant::now(),
        last_bootstrap_dial: Instant::now(),
        last_kad_bootstrap: Instant::now(),
    };

    let task = tokio::spawn(runner.run());
    Ok((NodeHandle { tx, peer_id }, events_rx, task))
}

fn load_bootstrap_records(cfg: &NodeConfig, identity: &NetworkIdentity) -> Vec<Multiaddr> {
    use prost::Message;
    let trusted: Vec<[u8; 32]> = cfg
        .trusted_bootstrap_signers
        .iter()
        .filter_map(|h| hex::decode(h).ok())
        .filter_map(|b| b.try_into().ok())
        .collect();
    let now = unix_now();
    let mut out = Vec::new();
    for path in &cfg.bootstrap_records {
        let Ok(raw) = std::fs::read(path) else {
            warn!(path, "bootstrap record not readable; skipping");
            continue;
        };
        let Ok(rec) = pb::BootstrapRecord::decode(raw.as_slice()) else {
            warn!(path, "bootstrap record does not decode; skipping");
            continue;
        };
        if let Err(e) = hashgram_proto::validate::bootstrap_record(&rec, now) {
            warn!(path, error = %e, "bootstrap record invalid; skipping");
            continue;
        }
        if let Err(e) = hashgram_proto::signing::verify_bootstrap_record(identity, &rec, &trusted) {
            warn!(path, error = %e, "bootstrap record not from a trusted signer; skipping");
            continue;
        }
        for a in rec.addrs {
            if let Ok(ma) = a.parse::<Multiaddr>() {
                out.push(ma);
            }
        }
    }
    out
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn remote_ip(endpoint: &ConnectedPoint) -> Option<IpAddr> {
    let addr = match endpoint {
        ConnectedPoint::Dialer { address, .. } => address,
        ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
    };
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(a) => Some(IpAddr::V4(a)),
        Protocol::Ip6(a) => Some(IpAddr::V6(a)),
        _ => None,
    })
}

fn remote_addr(endpoint: &ConnectedPoint) -> &Multiaddr {
    match endpoint {
        ConnectedPoint::Dialer { address, .. } => address,
        ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
    }
}

fn peer_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

/// "tcp", "quic", "relay" or "other" for a multiaddr, for logs.
fn transport_of(addr: &Multiaddr) -> &'static str {
    let mut kind = "other";
    for p in addr.iter() {
        match p {
            Protocol::P2pCircuit => return "relay",
            Protocol::QuicV1 | Protocol::Quic => kind = "quic",
            Protocol::Tcp(_) => kind = "tcp",
            _ => {}
        }
    }
    kind
}

fn is_tcp(addr: &Multiaddr) -> bool {
    transport_of(addr) == "tcp"
}

/// The addresses one dial attempt should use. With `prefer_tcp`, attempts
/// alternate: the first (and every even one) uses only the TCP addresses
/// when there are any, so QUIC cannot win the race just by finishing its
/// handshake first; odd attempts use everything so a TCP-filtered network
/// still connects over QUIC.
fn addresses_for_attempt(addrs: Vec<Multiaddr>, prefer_tcp: bool, attempt: u32) -> Vec<Multiaddr> {
    if !prefer_tcp || attempt % 2 == 1 {
        return addrs;
    }
    let tcp: Vec<Multiaddr> = addrs.iter().filter(|a| is_tcp(a)).cloned().collect();
    if tcp.is_empty() {
        addrs
    } else {
        tcp
    }
}

fn reason_code(err: &HandshakeError) -> ReasonCode {
    match err {
        HandshakeError::MagicMismatch { .. } => ReasonCode::Magic,
        HandshakeError::ProtocolMismatch { .. } => ReasonCode::Protocol,
        HandshakeError::NetworkIdMismatch { .. } => ReasonCode::NetworkId,
        HandshakeError::ChainIdMismatch { .. } => ReasonCode::ChainId,
        HandshakeError::GenesisMismatch { .. } => ReasonCode::Genesis,
        HandshakeError::MalformedGenesisHash { .. } | HandshakeError::NoLocalGenesis => {
            ReasonCode::Malformed
        }
    }
}

/// Most roles a handshake may claim.
pub const MAX_HANDSHAKE_ROLES: usize = 16;
/// Longest role name a handshake may claim, in bytes.
pub const MAX_ROLE_BYTES: usize = 16;
/// Longest operator address a handshake may carry, in bytes. A bech32
/// `hash1…` address is 44 characters; 90 is the bech32 maximum.
pub const MAX_OPERATOR_ADDRESS_BYTES: usize = 90;

/// Checks the free-form claims in a peer's handshake: its roles and its
/// operator address.
///
/// `verify_handshake` covers the identity parts. These two fields are not
/// identity, but they are stored per peer (in `verified`, the peerstore, and
/// every `PeerVerified` event) for as long as the peer is around, so an
/// unbounded value is a memory grab a single connection can perform once per
/// ban period. Role names are also used as map keys and log fields, hence
/// the strict alphabet: lowercase ASCII, digits, `_` and `-`, which is what
/// every known role already satisfies.
pub fn validate_handshake_claims(roles: &[String], operator: &str) -> Result<(), &'static str> {
    if roles.len() > MAX_HANDSHAKE_ROLES {
        return Err("too many or malformed roles");
    }
    for r in roles {
        if r.is_empty()
            || r.len() > MAX_ROLE_BYTES
            || !r
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err("too many or malformed roles");
        }
    }
    if operator.len() > MAX_OPERATOR_ADDRESS_BYTES {
        return Err("operator address too long");
    }
    Ok(())
}

fn to_wire(hs: &Handshake, roles: &[String], operator: &str) -> pb::Handshake {
    pb::Handshake {
        network_magic: hs.network_magic.to_vec(),
        network_id: hs.network_id.clone(),
        chain_id: hs.chain_id.clone(),
        genesis_hash: hs.genesis_hash.clone(),
        protocol_major_version: hs.protocol_major_version,
        roles: roles.to_vec(),
        operator_address: operator.to_owned(),
    }
}

fn from_wire(w: &pb::Handshake) -> Handshake {
    let mut magic = [0u8; hashgram_net::MAGIC_LEN];
    if w.network_magic.len() == hashgram_net::MAGIC_LEN {
        magic.copy_from_slice(&w.network_magic);
    }
    Handshake {
        network_magic: magic,
        network_id: w.network_id.clone(),
        chain_id: w.chain_id.clone(),
        genesis_hash: w.genesis_hash.clone(),
        protocol_major_version: w.protocol_major_version,
    }
}

fn error_response(code: &str, message: &str) -> pb::Response {
    pb::Response {
        body: Some(pb::response::Body::Error(pb::Error {
            code: code.to_owned(),
            message: message.to_owned(),
        })),
    }
}

impl Runner {
    async fn run(mut self) {
        let mut tick = tokio::time::interval(TICK);
        self.dial_bootstrap(8);
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => self.on_swarm_event(event).await,
                cmd = self.rx.recv() => match cmd {
                    Some(Command::Shutdown) | None => break,
                    Some(cmd) => self.on_command(cmd),
                },
                _ = tick.tick() => self.on_tick(),
            }
        }
        if let Err(e) = self.peerstore.save() {
            warn!(error = %e, "peerstore not saved at shutdown");
        }
        info!("swarm stopped");
    }

    // -- commands -----------------------------------------------------------

    fn on_command(&mut self, cmd: Command) {
        match cmd {
            Command::Dial(addr, reply) => {
                let _ = reply.send(self.dial_grouped(vec![addr]).map(|_| ()));
            }
            Command::Request {
                peer,
                addrs,
                request,
                reply,
            } => self.send_request(peer, addrs, request, reply),
            Command::Respond(channel, response) => {
                let _ = self
                    .swarm
                    .behaviour_mut()
                    .rpc
                    .send_response(channel, response);
            }
            Command::Subscribe(topic) => {
                match self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .subscribe(&IdentTopic::new(&topic))
                {
                    Ok(_) => {
                        self.topics.insert(topic);
                    }
                    Err(e) => warn!(topic, error = ?e, "subscribe failed"),
                }
            }
            Command::Unsubscribe(topic) => {
                let _ = self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .unsubscribe(&IdentTopic::new(&topic));
                self.topics.remove(&topic);
            }
            Command::Publish(topic, data, reply) => {
                let family = Metrics::topic_family(&topic);
                let res = self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(IdentTopic::new(&topic), data)
                    .map(|_| ())
                    .map_err(|e| format!("{e:?}"));
                if res.is_ok() {
                    self.metrics
                        .gossip_out
                        .get_or_create(&TopicLabel { family })
                        .inc();
                }
                let _ = reply.send(res);
            }
            Command::ReportGossip(id, source, acceptance) => {
                if matches!(acceptance, MessageAcceptance::Reject) {
                    self.metrics.gossip_rejected.inc();
                    self.score(source, ScoreEvent::InvalidSignature);
                }
                let _ = self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .report_message_validation_result(&id, &source, acceptance);
            }
            Command::StartProviding(key, reply) => {
                let res = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .start_providing(kad::RecordKey::new(&key))
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = reply.send(res);
            }
            Command::StopProviding(key) => {
                self.swarm
                    .behaviour_mut()
                    .kad
                    .stop_providing(&kad::RecordKey::new(&key));
            }
            Command::GetProviders(key, reply) => {
                let id = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .get_providers(kad::RecordKey::new(&key));
                self.pending_providers.insert(id, (HashSet::new(), reply));
            }
            Command::AddrsOf(peer, reply) => {
                let _ = reply.send(self.addrs_of(&peer));
            }
            Command::Score(peer, event) => self.score(peer, event),
            Command::Ban(peer) => self.ban(peer, "application request"),
            Command::Peers(reply) => {
                let _ = reply.send(self.peer_summaries());
            }
            Command::Stats(reply) => {
                let _ = reply.send(self.stats());
            }
            Command::PeersWithRole(role, reply) => {
                let peers: Vec<PeerId> = self
                    .verified
                    .iter()
                    .filter(|(_, v)| v.roles.iter().any(|r| r == &role))
                    .map(|(p, _)| *p)
                    .collect();
                let out = peers.into_iter().map(|p| (p, self.addrs_of(&p))).collect();
                let _ = reply.send(out);
            }
            Command::Shutdown => {}
        }
    }

    fn send_request(
        &mut self,
        peer: PeerId,
        addrs: Vec<Multiaddr>,
        request: pb::Request,
        reply: oneshot::Sender<Result<pb::Response, RequestError>>,
    ) {
        if self.verified.contains_key(&peer) {
            debug!(%peer, kind = ?request_kind(&request), "sending request");
            self.metrics
                .requests_out
                .get_or_create(&KindLabel {
                    kind: request_kind(&request),
                })
                .inc();
            let id = self.swarm.behaviour_mut().rpc.send_request(&peer, request);
            self.pending_requests.insert(id, reply);
            return;
        }
        if self.banned.contains_key(&peer) {
            let _ = reply.send(Err(RequestError::Unreachable("peer is banned".into())));
            return;
        }
        if !self.connected.contains_key(&peer) {
            // One dial per peer with every address we have: libp2p races
            // them and keeps the first that succeeds (QUIC or TCP), so a
            // blocked transport costs nothing and the peer sees one
            // connection, not one per address.
            let mut all: Vec<Multiaddr> = addrs
                .into_iter()
                .map(|a| {
                    if peer_of(&a).is_some() {
                        a
                    } else {
                        a.with(Protocol::P2p(peer))
                    }
                })
                .collect();
            for a in self.addrs_of(&peer) {
                if !all.contains(&a) {
                    all.push(a);
                }
            }
            let opts = DialOpts::peer_id(peer)
                .addresses(all)
                .condition(PeerCondition::DisconnectedAndNotDialing)
                .build();
            match self.swarm.dial(opts) {
                Ok(()) => {}
                // Already dialling: the queued request rides that attempt.
                Err(DialError::DialPeerConditionFalse(_)) => {}
                Err(e) => {
                    let _ = reply.send(Err(RequestError::Unreachable(format!("dial failed: {e}"))));
                    return;
                }
            }
        }
        self.queued.entry(peer).or_default().push_back(Queued {
            request,
            reply,
            since: Instant::now(),
        });
    }

    fn flush_queued(&mut self, peer: PeerId) {
        let Some(queue) = self.queued.remove(&peer) else {
            return;
        };
        for q in queue {
            self.send_request(peer, Vec::new(), q.request, q.reply);
        }
    }

    fn fail_queued(&mut self, peer: &PeerId, err: RequestError) {
        if let Some(queue) = self.queued.remove(peer) {
            for q in queue {
                let _ = q.reply.send(Err(err.clone()));
            }
        }
    }

    // -- swarm events ---------------------------------------------------------

    async fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!(%address, "listening");
                let _ = self.events.try_send(Event::Listening(address));
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                info!(%address, "external address confirmed");
                if !self.external_addrs.contains(&address) {
                    self.external_addrs.push(address.clone());
                }
                let _ = self.events.try_send(Event::ExternalAddr(address));
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                ..
            } => self.on_connected(peer_id, connection_id, endpoint, num_established.get()),
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                cause,
            } => {
                if self.verified.contains_key(&peer_id) {
                    // Info, not debug: when a client keeps losing its node
                    // this line is what tells the operator whether it was
                    // QUIC or TCP and who hung up.
                    info!(
                        peer = %peer_id,
                        transport = transport_of(remote_addr(&endpoint)),
                        cause = %cause.as_ref().map(ToString::to_string).unwrap_or_else(|| "closed by us or by the peer".into()),
                        remaining = num_established,
                        "connection closed"
                    );
                }
                self.on_closed(peer_id, connection_id, &endpoint, num_established);
            }
            SwarmEvent::OutgoingConnectionError {
                peer_id: Some(peer),
                error,
                ..
            } => {
                debug!(%peer, %error, "outgoing connection failed");
                self.peerstore.record_failure(&peer);
                self.fail_queued(&peer, RequestError::Unreachable(error.to_string()));
            }
            SwarmEvent::Behaviour(b) => self.on_behaviour(b),
            _ => {}
        }
    }

    fn on_connected(
        &mut self,
        peer: PeerId,
        connection: ConnectionId,
        endpoint: ConnectedPoint,
        established: u32,
    ) {
        let inbound = endpoint.is_listener();
        let ip = remote_ip(&endpoint).unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        let banned = self.banned.contains_key(&peer);
        let decision = if inbound {
            self.limits.allow_inbound(&peer.to_string(), ip, banned)
        } else {
            self.limits.allow_outbound(&peer.to_string(), banned)
        };
        if !decision.is_allowed() {
            // Info, not debug: an operator whose users cannot connect needs
            // to see this in the journal under the default filter.
            info!(%peer, %ip, %decision, "connection refused by local limits");
            self.metrics.connections_refused.inc();
            // Remember it so its close does not debit a connection that was
            // never credited (that drift loosened the limits over time).
            self.refused.insert(connection);
            self.swarm.close_connection(connection);
            return;
        }
        self.limits.established(&peer.to_string(), ip, inbound);
        if established == 1 {
            self.connected.insert(
                peer,
                Connected {
                    endpoint: endpoint.clone(),
                    since: Instant::now(),
                },
            );
        }
        self.metrics
            .peers_connected
            .set(self.connected.len() as i64);

        if self.verified.contains_key(&peer) {
            return;
        }
        self.unverified_since
            .entry(peer)
            .or_insert_with(Instant::now);
        if !inbound {
            let hs = Handshake::for_identity(&self.identity);
            let req = pb::Request {
                body: Some(pb::request::Body::Handshake(to_wire(
                    &hs,
                    &self.cfg.roles,
                    &self.cfg.operator_address,
                ))),
            };
            let id = self.swarm.behaviour_mut().rpc.send_request(&peer, req);
            self.pending_handshakes.insert(id, peer);
        }
    }

    fn on_closed(
        &mut self,
        peer: PeerId,
        connection: ConnectionId,
        endpoint: &ConnectedPoint,
        remaining: u32,
    ) {
        let ip = remote_ip(endpoint).unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
        if !self.refused.remove(&connection) {
            self.limits
                .closed(&peer.to_string(), ip, endpoint.is_listener());
        }
        if remaining == 0 {
            self.connected.remove(&peer);
            self.unverified_since.remove(&peer);
            self.identified.remove(&peer);
            self.buckets.remove(&peer);
            self.ping_failures.remove(&peer);
            self.rtt.remove(&peer);
            if self.verified.remove(&peer).is_some() {
                let _ = self.events.try_send(Event::PeerDisconnected(peer));
            }
            self.fail_queued(&peer, RequestError::Unreachable("disconnected".into()));
        }
        self.metrics
            .peers_connected
            .set(self.connected.len() as i64);
        self.metrics.peers_verified.set(self.verified.len() as i64);
    }

    fn on_behaviour(&mut self, event: BehaviourEvent) {
        match event {
            BehaviourEvent::Rpc(e) => self.on_rpc(e),
            BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            }) => self.on_gossip(propagation_source, message_id, message),
            BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                let addrs: Vec<Multiaddr> = info
                    .listen_addrs
                    .into_iter()
                    .filter(is_dialable)
                    .take(8)
                    .collect();
                if self.verified.contains_key(&peer_id) {
                    for a in &addrs {
                        self.swarm
                            .behaviour_mut()
                            .kad
                            .add_address(&peer_id, a.clone());
                    }
                }
                self.identified.insert(peer_id, (addrs, info.agent_version));
            }
            BehaviourEvent::Kad(kad::Event::OutboundQueryProgressed {
                id,
                result: kad::QueryResult::GetProviders(res),
                step,
                ..
            }) => {
                // Answer the requester as soon as any providers are known
                // rather than when the whole Kademlia walk finishes: on a
                // small network a full walk runs to the 30 s query timeout,
                // and every SDK caller unions the answer with its connected
                // store peers anyway. The query keeps running to completion
                // in the background so the routing table still benefits.
                let mut done = step.last;
                if let Some((found, _)) = self.pending_providers.get_mut(&id) {
                    if let Ok(kad::GetProvidersOk::FoundProviders { providers, .. }) = res {
                        found.extend(providers);
                        if !found.is_empty() {
                            done = true;
                        }
                    }
                }
                if done {
                    if let Some((found, reply)) = self.pending_providers.remove(&id) {
                        let _ = reply.send(found.into_iter().collect());
                    }
                }
            }
            BehaviourEvent::Kad(kad::Event::RoutingUpdated { .. }) => {
                let size = self.kad_size() as i64;
                self.metrics.kad_peers.set(size);
            }
            BehaviourEvent::Autonat(autonat::Event::StatusChanged { new, .. }) => {
                self.reachability = match new {
                    autonat::NatStatus::Public(_) => "public",
                    autonat::NatStatus::Private => "private",
                    autonat::NatStatus::Unknown => "unknown",
                };
                self.metrics.reachable.set(match self.reachability {
                    "public" => 1,
                    "private" => 0,
                    _ => -1,
                });
                info!(status = self.reachability, "reachability changed");
            }
            BehaviourEvent::Ping(libp2p::ping::Event { peer, result, .. }) => {
                if let Ok(rtt) = result {
                    self.ping_failures.remove(&peer);
                    let sample = rtt.as_secs_f64() * 1000.0;
                    self.rtt
                        .entry(peer)
                        .and_modify(|v| *v = smooth_rtt(*v, sample))
                        .or_insert(sample);
                } else {
                    self.score(peer, ScoreEvent::RequestTimedOut);
                    if self.verified.contains_key(&peer) {
                        let disconnect = {
                            let streak = self.ping_failures.entry(peer).or_default();
                            ping_streak_requires_disconnect(streak)
                        };
                        if disconnect {
                            self.ping_failures.remove(&peer);
                            warn!(%peer, "peer failed three consecutive pings; disconnecting");
                            // ConnectionClosed performs the authoritative map
                            // cleanup and emits PeerDisconnected. Removing the
                            // socket locally also unblocks bootstrap redial.
                            let _ = self.swarm.disconnect_peer_id(peer);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn on_rpc(&mut self, event: request_response::Event<pb::Request, pb::Response>) {
        match event {
            request_response::Event::Message { peer, message, .. } => match message {
                request_response::Message::Request {
                    request, channel, ..
                } => self.on_inbound_request(peer, request, channel),
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(peer) = self.pending_handshakes.remove(&request_id) {
                        self.on_handshake_response(peer, response);
                    } else if let Some(reply) = self.pending_requests.remove(&request_id) {
                        self.score(peer, ScoreEvent::RequestAnswered);
                        let _ = reply.send(Ok(response));
                    }
                }
            },
            request_response::Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                if self.pending_handshakes.remove(&request_id).is_some() {
                    self.reject(
                        peer,
                        ReasonCode::Transport,
                        &format!("handshake transport failure: {error}"),
                    );
                } else if let Some(reply) = self.pending_requests.remove(&request_id) {
                    self.metrics.requests_failed.inc();
                    self.score(peer, ScoreEvent::RequestTimedOut);
                    let _ = reply.send(Err(RequestError::Failed(error.to_string())));
                }
            }
            request_response::Event::InboundFailure { peer, error, .. } => {
                debug!(%peer, %error, "inbound request failed");
            }
            request_response::Event::ResponseSent { peer, .. } => {
                // The ack is on its way. Give the transport a moment to flush
                // it before the tick disconnects; closing in the same poll
                // races the final frame and the fork sees "connection closed"
                // instead of the reason.
                if let Some(entry) = self.pending_reject.get_mut(&peer) {
                    let now = Instant::now();
                    entry.2 = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
                }
            }
        }
    }

    fn on_inbound_request(
        &mut self,
        peer: PeerId,
        request: pb::Request,
        channel: ResponseChannel<pb::Response>,
    ) {
        let kind = request_kind(&request);
        debug!(%peer, ?kind, "inbound request");
        self.metrics
            .requests_in
            .get_or_create(&KindLabel { kind })
            .inc();

        if let Some(pb::request::Body::Handshake(hs)) = &request.body {
            self.on_handshake_request(peer, hs, channel);
            return;
        }
        if !self.verified.contains_key(&peer) || self.pending_reject.contains_key(&peer) {
            self.metrics.requests_refused.inc();
            let _ = self.swarm.behaviour_mut().rpc.send_response(
                channel,
                error_response("unauthenticated", "complete the Hashgram handshake first"),
            );
            return;
        }
        if !self.take_token(peer) {
            self.metrics.requests_refused.inc();
            self.score(peer, ScoreEvent::RateLimitExceeded);
            let _ = self
                .swarm
                .behaviour_mut()
                .rpc
                .send_response(channel, error_response("rate_limited", "too many requests"));
            return;
        }
        if let Some(pb::request::Body::PeerExchange(px)) = &request.body {
            let limit = (px.limit as usize).clamp(1, 32);
            let addrs = self.exchange_addrs(&peer, limit);
            let _ = self.swarm.behaviour_mut().rpc.send_response(
                channel,
                pb::Response {
                    body: Some(pb::response::Body::PeerExchange(pb::PeerExchangeResult {
                        addrs,
                    })),
                },
            );
            return;
        }
        if request.body.is_none() {
            self.score(peer, ScoreEvent::MalformedFrame);
            let _ = self
                .swarm
                .behaviour_mut()
                .rpc
                .send_response(channel, error_response("invalid", "empty request"));
            return;
        }
        if self
            .events
            .try_send(Event::InboundRequest {
                peer,
                request,
                channel,
            })
            .is_err()
        {
            // The application is behind. Dropping the channel makes the
            // requester see a failure, which is the honest signal.
            warn!("application event queue full; dropping inbound request");
        }
    }

    fn on_handshake_request(
        &mut self,
        peer: PeerId,
        hs: &pb::Handshake,
        channel: ResponseChannel<pb::Response>,
    ) {
        let ours = to_wire(
            &Handshake::for_identity(&self.identity),
            &self.cfg.roles,
            &self.cfg.operator_address,
        );
        match self.identity.verify_handshake(&from_wire(hs)) {
            Ok(()) => {
                // Identity agreed; now the claims we would store about it.
                // Oversized claims are a malformed frame, not a fork, and
                // are refused the same way a fork is: an ack that says why,
                // then a disconnect.
                if let Err(why) = validate_handshake_claims(&hs.roles, &hs.operator_address) {
                    self.score(peer, ScoreEvent::MalformedFrame);
                    let ack = pb::Response {
                        body: Some(pb::response::Body::Handshake(pb::HandshakeAck {
                            accepted: false,
                            reason: why.to_owned(),
                            identity: Some(ours),
                        })),
                    };
                    let _ = self.swarm.behaviour_mut().rpc.send_response(channel, ack);
                    self.pending_reject.insert(
                        peer,
                        (ReasonCode::Malformed, why.to_owned(), Instant::now()),
                    );
                    return;
                }
                let ack = pb::Response {
                    body: Some(pb::response::Body::Handshake(pb::HandshakeAck {
                        accepted: true,
                        reason: String::new(),
                        identity: Some(ours),
                    })),
                };
                let _ = self.swarm.behaviour_mut().rpc.send_response(channel, ack);
                self.verify(peer, hs.roles.clone(), hs.operator_address.clone());
            }
            Err(e) => {
                let ack = pb::Response {
                    body: Some(pb::response::Body::Handshake(pb::HandshakeAck {
                        accepted: false,
                        reason: e.to_string(),
                        identity: Some(ours),
                    })),
                };
                let _ = self.swarm.behaviour_mut().rpc.send_response(channel, ack);
                // Disconnect once the ack has left, so the fork's operator
                // reads "genesis hash mismatch" rather than "connection
                // closed". See ResponseSent and the tick.
                self.pending_reject
                    .insert(peer, (reason_code(&e), e.to_string(), Instant::now()));
            }
        }
    }

    fn on_handshake_response(&mut self, peer: PeerId, response: pb::Response) {
        let Some(pb::response::Body::Handshake(ack)) = response.body else {
            self.reject(
                peer,
                ReasonCode::Malformed,
                "handshake answered with a non-handshake body",
            );
            return;
        };
        let Some(remote) = ack.identity else {
            self.reject(
                peer,
                ReasonCode::Malformed,
                "handshake ack carried no identity",
            );
            return;
        };
        match self.identity.verify_handshake(&from_wire(&remote)) {
            Ok(()) if ack.accepted => {
                if let Err(why) = validate_handshake_claims(&remote.roles, &remote.operator_address)
                {
                    self.score(peer, ScoreEvent::MalformedFrame);
                    self.reject(peer, ReasonCode::Malformed, why);
                    return;
                }
                self.verify(peer, remote.roles, remote.operator_address);
            }
            Ok(()) => self.reject(
                peer,
                ReasonCode::Genesis,
                &format!("peer refused us: {}", ack.reason),
            ),
            Err(e) => self.reject(peer, reason_code(&e), &e.to_string()),
        }
    }

    fn verify(&mut self, peer: PeerId, roles: Vec<String>, operator: String) {
        self.unverified_since.remove(&peer);
        let fresh = self
            .verified
            .insert(
                peer,
                Verified {
                    roles: roles.clone(),
                    operator: operator.clone(),
                },
            )
            .is_none();
        if !fresh {
            return;
        }
        self.metrics.handshakes_ok.inc();
        self.metrics.peers_verified.set(self.verified.len() as i64);
        self.score(peer, ScoreEvent::HandshakeSucceeded);

        let addr = self
            .connected
            .get(&peer)
            .map(|c| remote_addr(&c.endpoint).clone());
        let dialable_addr = addr.filter(is_dialable);
        self.peerstore
            .record_success(&peer, dialable_addr.as_ref(), &roles);
        self.metrics.peers_known.set(self.peerstore.len() as i64);

        let addrs: Vec<Multiaddr> = self
            .identified
            .get(&peer)
            .map(|(a, _)| a.clone())
            .unwrap_or_default()
            .into_iter()
            .chain(dialable_addr)
            .collect();
        for a in addrs {
            self.swarm.behaviour_mut().kad.add_address(&peer, a);
        }
        info!(%peer, roles = ?roles, "peer verified");
        let _ = self.events.try_send(Event::PeerVerified {
            peer,
            roles,
            operator,
        });
        self.flush_queued(peer);
    }

    fn reject(&mut self, peer: PeerId, code: ReasonCode, reason: &str) {
        warn!(%peer, reason, "peer rejected at handshake");
        self.metrics
            .handshakes_failed
            .get_or_create(&ReasonLabel { reason: code })
            .inc();
        self.score(peer, ScoreEvent::HandshakeFailed);
        self.fail_queued(&peer, RequestError::NotVerified);
        let _ = self.events.try_send(Event::PeerRejected {
            peer,
            reason: reason.to_owned(),
        });
        // A transport failure is not a fork; give it another chance later.
        if matches!(code, ReasonCode::Transport | ReasonCode::Timeout) {
            let _ = self.swarm.disconnect_peer_id(peer);
        } else {
            self.ban(peer, reason);
        }
    }

    fn ban(&mut self, peer: PeerId, reason: &str) {
        debug!(%peer, reason, "banning peer");
        self.banned.insert(peer, Instant::now() + BAN_DURATION);
        self.swarm.behaviour_mut().blocked.block_peer(peer);
        self.swarm.behaviour_mut().gossipsub.blacklist_peer(&peer);
        self.swarm.behaviour_mut().kad.remove_peer(&peer);
        self.peerstore.forget(&peer);
        self.verified.remove(&peer);
        let _ = self.swarm.disconnect_peer_id(peer);
        self.metrics.peers_banned.set(self.banned.len() as i64);
        self.metrics.peers_verified.set(self.verified.len() as i64);
    }

    fn on_gossip(&mut self, source: PeerId, id: MessageId, message: gossipsub::Message) {
        let topic = topic_name(&message.topic);
        self.metrics
            .gossip_in
            .get_or_create(&TopicLabel {
                family: Metrics::topic_family(&topic),
            })
            .inc();
        if !self.verified.contains_key(&source) {
            let _ = self
                .swarm
                .behaviour_mut()
                .gossipsub
                .report_message_validation_result(&id, &source, MessageAcceptance::Ignore);
            return;
        }
        if !self.take_token(source) {
            self.score(source, ScoreEvent::RateLimitExceeded);
            let _ = self
                .swarm
                .behaviour_mut()
                .gossipsub
                .report_message_validation_result(&id, &source, MessageAcceptance::Ignore);
            return;
        }
        if self
            .events
            .try_send(Event::Gossip {
                topic,
                source,
                id: id.clone(),
                data: message.data,
            })
            .is_err()
        {
            let _ = self
                .swarm
                .behaviour_mut()
                .gossipsub
                .report_message_validation_result(&id, &source, MessageAcceptance::Ignore);
        }
    }

    // -- maintenance ----------------------------------------------------------

    fn on_tick(&mut self) {
        let now = Instant::now();

        // Unverified peers past the grace period.
        let stale: Vec<PeerId> = self
            .unverified_since
            .iter()
            .filter(|(_, since)| now.duration_since(**since) > HANDSHAKE_GRACE)
            .map(|(p, _)| *p)
            .collect();
        for peer in stale {
            self.unverified_since.remove(&peer);
            self.reject(
                peer,
                ReasonCode::Timeout,
                "no handshake within the grace period",
            );
        }

        // Rejections whose ack never reported as sent.
        let overdue: Vec<PeerId> = self
            .pending_reject
            .iter()
            .filter(|(_, (_, _, since))| now.duration_since(*since) > Duration::from_millis(2500))
            .map(|(p, _)| *p)
            .collect();
        for peer in overdue {
            if let Some((code, reason, _)) = self.pending_reject.remove(&peer) {
                self.reject(peer, code, &reason);
            }
        }

        // Queued requests past their timeout.
        let expired: Vec<PeerId> = self
            .queued
            .iter()
            .filter(|(_, q)| {
                q.front()
                    .is_some_and(|f| now.duration_since(f.since) > QUEUE_TIMEOUT)
            })
            .map(|(p, _)| *p)
            .collect();
        for peer in expired {
            self.fail_queued(&peer, RequestError::NotVerified);
        }

        // Bans that have expired.
        let unban: Vec<PeerId> = self
            .banned
            .iter()
            .filter(|(_, until)| **until <= now)
            .map(|(p, _)| *p)
            .collect();
        for peer in unban {
            self.banned.remove(&peer);
            self.swarm.behaviour_mut().blocked.unblock_peer(peer);
            self.swarm
                .behaviour_mut()
                .gossipsub
                .remove_blacklisted_peer(&peer);
        }
        self.metrics.peers_banned.set(self.banned.len() as i64);

        self.scores.decay_all(now);
        self.scores.prune(now, Duration::from_secs(6 * 3600));
        self.buckets.retain(|p, _| self.connected.contains_key(p));

        if self.verified.len() < self.cfg.min_peers
            && now.duration_since(self.last_bootstrap_dial) >= BOOTSTRAP_RETRY
        {
            self.last_bootstrap_dial = now;
            self.dial_bootstrap(4);
        }
        if self.kad_size() > 0 && now.duration_since(self.last_kad_bootstrap) >= KAD_BOOTSTRAP {
            self.last_kad_bootstrap = now;
            let _ = self.swarm.behaviour_mut().kad.bootstrap();
        }

        if now.duration_since(self.last_peerstore_flush) > PEERSTORE_FLUSH {
            self.last_peerstore_flush = now;
            if let Err(e) = self.peerstore.save() {
                warn!(error = %e, "peerstore not saved");
            }
        }
    }

    fn dial_bootstrap(&mut self, max: usize) {
        let candidates = std::mem::take(&mut self.bootstrap_candidates);
        let local = *self.swarm.local_peer_id();
        // Every address of one peer goes into one dial (QUIC and TCP of the
        // same node race; the first to connect wins), and a peer already
        // connected, banned, or being dialled is not dialled again. Before
        // this, each address was dialled on its own with no peer condition,
        // which opened two or three connections to the same node and used
        // up its per-subnet inbound slots two or three times as fast.
        let mut ordered: Vec<(Option<PeerId>, Vec<Multiaddr>)> = Vec::new();
        for addr in &candidates {
            let pid = peer_of(addr);
            if pid == Some(local) {
                continue;
            }
            match pid.and_then(|p| ordered.iter_mut().find(|(q, _)| *q == Some(p))) {
                Some((_, list)) => list.push(addr.clone()),
                None => ordered.push((pid, vec![addr.clone()])),
            }
        }
        let mut dialled = 0usize;
        for (pid, addrs) in ordered {
            if dialled >= max {
                break;
            }
            let skip = pid
                .is_some_and(|p| self.connected.contains_key(&p) || self.banned.contains_key(&p));
            if skip {
                continue;
            }
            match self.dial_grouped(addrs) {
                Ok(n) => dialled += n,
                Err(e) => debug!(error = %e, "bootstrap dial not started"),
            }
        }
        // Rotate so the next round tries different candidates first.
        let mut keep = candidates;
        if !keep.is_empty() {
            let by = dialled.min(keep.len());
            keep.rotate_left(by);
        }
        self.bootstrap_candidates = keep;
        // Refresh from the peerstore too: peers learned since start.
        for (_, addr) in self.peerstore.candidates(16) {
            if !self.bootstrap_candidates.contains(&addr) {
                self.bootstrap_candidates.push(addr);
            }
        }
    }

    /// Dials a set of addresses as one attempt per peer id. Addresses that
    /// carry no `/p2p/` component are dialled individually. Returns how many
    /// dial attempts were started; a peer already connected or already
    /// being dialled counts as zero and is not an error.
    fn dial_grouped(&mut self, addrs: Vec<Multiaddr>) -> Result<usize, String> {
        let mut by_peer: Vec<(PeerId, Vec<Multiaddr>)> = Vec::new();
        let mut anonymous = Vec::new();
        for a in addrs {
            match peer_of(&a) {
                Some(p) => match by_peer.iter_mut().find(|(q, _)| *q == p) {
                    Some((_, list)) => list.push(a),
                    None => by_peer.push((p, vec![a])),
                },
                None => anonymous.push(a),
            }
        }
        let mut started = 0usize;
        let mut last_err: Option<String> = None;
        for (peer, list) in by_peer {
            if self.connected.contains_key(&peer) || self.banned.contains_key(&peer) {
                continue;
            }
            let attempt = {
                let n = self.dial_attempts.entry(peer).or_insert(0);
                let cur = *n;
                *n = n.wrapping_add(1);
                cur
            };
            if self.dial_attempts.len() > 4_096 {
                self.dial_attempts.clear();
            }
            let list = addresses_for_attempt(list, self.cfg.prefer_tcp, attempt);
            let opts = DialOpts::peer_id(peer)
                .addresses(list)
                .condition(PeerCondition::DisconnectedAndNotDialing)
                .build();
            match self.swarm.dial(opts) {
                Ok(()) => started += 1,
                Err(DialError::DialPeerConditionFalse(_)) => {}
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        for a in anonymous {
            match self.swarm.dial(a) {
                Ok(()) => started += 1,
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        match (started, last_err) {
            (0, Some(e)) => Err(e),
            _ => Ok(started),
        }
    }

    /// Addresses of verified peers other than the asker, for peer exchange.
    /// Only peers that passed the handshake are offered: handing out
    /// unverified addresses would make this node a vector for a fork's
    /// peers to spread.
    fn exchange_addrs(&mut self, asker: &PeerId, limit: usize) -> Vec<String> {
        let peers: Vec<PeerId> = self
            .verified
            .keys()
            .filter(|p| *p != asker)
            .copied()
            .collect();
        let mut out = Vec::new();
        for p in peers {
            for a in self.addrs_of(&p) {
                if !is_dialable(&a) {
                    continue;
                }
                let full = if peer_of(&a).is_some() {
                    a
                } else {
                    a.with(Protocol::P2p(p))
                };
                out.push(full.to_string());
                if out.len() >= limit {
                    return out;
                }
                break;
            }
        }
        if out.len() < limit {
            for a in self.peerstore.exchange_addrs(limit - out.len()) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
        out
    }

    fn take_token(&mut self, peer: PeerId) -> bool {
        let now = Instant::now();
        let b = self.buckets.entry(peer).or_insert(Bucket {
            tokens: RATE_BURST,
            last: now,
        });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.last = now;
        b.tokens = (b.tokens + elapsed * RATE_PER_SEC).min(RATE_BURST);
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn score(&mut self, peer: PeerId, event: ScoreEvent) {
        let now = Instant::now();
        self.scores.record(&peer.to_string(), event, now);
        if self.scores.is_banned(&peer.to_string()) && !self.banned.contains_key(&peer) {
            self.ban(peer, "score below ban threshold");
        }
    }

    fn addrs_of(&mut self, peer: &PeerId) -> Vec<Multiaddr> {
        let mut out: Vec<Multiaddr> = Vec::new();
        if let Some(c) = self.connected.get(peer) {
            out.push(remote_addr(&c.endpoint).clone());
        }
        if let Some((addrs, _)) = self.identified.get(peer) {
            for a in addrs {
                if !out.contains(a) {
                    out.push(a.clone());
                }
            }
        }
        for bucket in self.swarm.behaviour_mut().kad.kbuckets() {
            for entry in bucket.iter() {
                if entry.node.key.preimage() == peer {
                    for a in entry.node.value.iter() {
                        if !out.contains(a) {
                            out.push(a.clone());
                        }
                    }
                }
            }
        }
        out
    }

    fn kad_size(&mut self) -> usize {
        self.swarm
            .behaviour_mut()
            .kad
            .kbuckets()
            .map(|b| b.num_entries())
            .sum()
    }

    fn peer_summaries(&self) -> Vec<PeerSummary> {
        let now = Instant::now();
        self.connected
            .iter()
            .map(|(peer, c)| PeerSummary {
                peer_id: peer.to_string(),
                address: remote_addr(&c.endpoint).to_string(),
                verified: self.verified.contains_key(peer),
                roles: self
                    .verified
                    .get(peer)
                    .map(|v| v.roles.clone())
                    .unwrap_or_default(),
                operator: self
                    .verified
                    .get(peer)
                    .map(|v| v.operator.clone())
                    .unwrap_or_default(),
                direction: if c.endpoint.is_listener() {
                    "inbound"
                } else {
                    "outbound"
                },
                score: self.scores.score(&peer.to_string()),
                connected_secs: now.duration_since(c.since).as_secs(),
                agent: self
                    .identified
                    .get(peer)
                    .map(|(_, a)| a.clone())
                    .unwrap_or_default(),
                rtt_ms: self.rtt.get(peer).map(|v| v.round() as u64),
            })
            .collect()
    }

    fn stats(&mut self) -> Stats {
        let listen_addrs = self.swarm.listeners().map(ToString::to_string).collect();
        let kad_peers = self.kad_size();
        Stats {
            peer_id: self.swarm.local_peer_id().to_string(),
            listen_addrs,
            external_addrs: self
                .external_addrs
                .iter()
                .map(ToString::to_string)
                .collect(),
            connected: self.connected.len(),
            verified: self.verified.len(),
            known: self.peerstore.len(),
            banned: self.banned.len(),
            kad_peers,
            reachability: self.reachability,
            topics: self.topics.iter().cloned().collect(),
        }
    }
}

fn topic_name(hash: &TopicHash) -> String {
    hash.to_string()
}

/// Whether an address is worth dialling: not loopback unless we are
/// ourselves on loopback, not a relay circuit we cannot use, not empty.
fn is_dialable(addr: &Multiaddr) -> bool {
    let mut has_transport = false;
    for p in addr.iter() {
        match p {
            Protocol::Ip4(_)
            | Protocol::Ip6(_)
            | Protocol::Dns(_)
            | Protocol::Dns4(_)
            | Protocol::Dns6(_)
            | Protocol::Dnsaddr(_) => has_transport = true,
            Protocol::P2pCircuit => return false,
            _ => {}
        }
    }
    has_transport
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn prefer_tcp_alternates_tcp_only_and_everything() {
        let quic: Multiaddr = "/ip4/203.0.113.1/udp/26670/quic-v1".parse().unwrap();
        let tcp: Multiaddr = "/ip4/203.0.113.1/tcp/26670".parse().unwrap();
        let both = vec![quic.clone(), tcp.clone()];
        assert_eq!(transport_of(&quic), "quic");
        assert_eq!(transport_of(&tcp), "tcp");
        // Even attempts: TCP only.
        assert_eq!(addresses_for_attempt(both.clone(), true, 0), vec![tcp.clone()]);
        assert_eq!(addresses_for_attempt(both.clone(), true, 2), vec![tcp.clone()]);
        // Odd attempts: everything, so a TCP-filtered network still connects.
        assert_eq!(addresses_for_attempt(both.clone(), true, 1), both);
        // No TCP address: nothing to prefer.
        assert_eq!(addresses_for_attempt(vec![quic.clone()], true, 0), vec![quic.clone()]);
        // Preference off: untouched.
        assert_eq!(addresses_for_attempt(both.clone(), false, 0), both);
    }

    #[test]
    fn three_consecutive_ping_failures_require_disconnect() {
        let mut streak = 0;
        assert!(!ping_streak_requires_disconnect(&mut streak));
        assert!(!ping_streak_requires_disconnect(&mut streak));
        assert!(ping_streak_requires_disconnect(&mut streak));
    }

    #[test]
    fn known_role_sets_pass() {
        validate_handshake_claims(&roles(&["store", "relay", "call", "media_2", "x-y"]), "")
            .unwrap();
        validate_handshake_claims(&[], "hash1qpzry9x8gf2tvdw0s3jn54khce6mua7lqqqqqqqq").unwrap();
        let sixteen: Vec<String> = (0..MAX_HANDSHAKE_ROLES).map(|i| format!("r{i}")).collect();
        validate_handshake_claims(&sixteen, "").unwrap();
        let longest = "a".repeat(MAX_ROLE_BYTES);
        validate_handshake_claims(&[longest], "").unwrap();
    }

    #[test]
    fn too_many_roles_are_refused() {
        let many: Vec<String> = (0..=MAX_HANDSHAKE_ROLES).map(|i| format!("r{i}")).collect();
        assert_eq!(
            validate_handshake_claims(&many, ""),
            Err("too many or malformed roles")
        );
    }

    #[test]
    fn oversized_or_malformed_roles_are_refused() {
        let long = "a".repeat(MAX_ROLE_BYTES + 1);
        assert!(validate_handshake_claims(&[long], "").is_err());
        // The 1 MiB role the audit described.
        let huge = "z".repeat(1 << 20);
        assert!(validate_handshake_claims(&[huge], "").is_err());
        for bad in ["", "Store", "st ore", "st.ore", "stör", "rel/ay", "a\n"] {
            assert!(
                validate_handshake_claims(&roles(&[bad]), "").is_err(),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn operator_address_is_bounded() {
        let at_limit = "h".repeat(MAX_OPERATOR_ADDRESS_BYTES);
        validate_handshake_claims(&[], &at_limit).unwrap();
        let over = "h".repeat(MAX_OPERATOR_ADDRESS_BYTES + 1);
        assert_eq!(
            validate_handshake_claims(&[], &over),
            Err("operator address too long")
        );
    }
}
