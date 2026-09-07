//! Prometheus metrics for the P2P layer.
//!
//! Counts and gauges about protocol behaviour. Nothing here carries a peer
//! id, a mailbox, a CID or any content as a label: labels are bounded enums,
//! because an unbounded label set is a memory leak an attacker can drive.

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

/// Why a handshake failed, as a bounded label.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ReasonLabel {
    /// One of a fixed set of reason codes.
    pub reason: ReasonCode,
}

/// The fixed set of handshake failure reasons.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(missing_docs)]
pub enum ReasonCode {
    Magic,
    Protocol,
    NetworkId,
    ChainId,
    Genesis,
    Malformed,
    Timeout,
    Transport,
}

/// A request family, as a bounded label.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct KindLabel {
    /// Request family.
    pub kind: RequestKind,
}

/// Request families on `/hashgram/rpc/1`.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(missing_docs)]
pub enum RequestKind {
    Handshake,
    Mailbox,
    KeyPackage,
    Event,
    Blob,
    Announce,
    PeerExchange,
    Attestation,
    Receipt,
    Calls,
    Unknown,
}

/// Gossip topic families.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct TopicLabel {
    /// Topic family.
    pub family: TopicFamily,
}

/// The fixed set of topic families.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
#[allow(missing_docs)]
pub enum TopicFamily {
    Social,
    Channel,
    Tag,
    Mailbox,
    Announce,
    Safety,
    Other,
}

/// The metric set.
#[derive(Debug, Clone)]
pub struct Metrics {
    /// Connected peers, verified or not.
    pub peers_connected: Gauge,
    /// Peers that completed the handshake.
    pub peers_verified: Gauge,
    /// Peers in the persistent store.
    pub peers_known: Gauge,
    /// Banned peers.
    pub peers_banned: Gauge,
    /// Handshakes completed.
    pub handshakes_ok: Counter,
    /// Handshakes refused, by reason.
    pub handshakes_failed: Family<ReasonLabel, Counter>,
    /// Inbound requests, by kind.
    pub requests_in: Family<KindLabel, Counter>,
    /// Outbound requests, by kind.
    pub requests_out: Family<KindLabel, Counter>,
    /// Requests refused before the handler ran (unauthenticated, rate limit).
    pub requests_refused: Counter,
    /// Outbound requests that failed at the transport.
    pub requests_failed: Counter,
    /// Gossip messages received, by topic family.
    pub gossip_in: Family<TopicLabel, Counter>,
    /// Gossip messages this node rejected as invalid.
    pub gossip_rejected: Counter,
    /// Gossip messages published by this node.
    pub gossip_out: Family<TopicLabel, Counter>,
    /// Connections refused by our own limits.
    pub connections_refused: Counter,
    /// Whether autonat believes this node is publicly reachable (1/0/-1).
    pub reachable: Gauge,
    /// DHT routing table size.
    pub kad_peers: Gauge,
}

impl Metrics {
    /// Registers every metric under the `hashgram_node_` prefix.
    pub fn register(registry: &mut Registry) -> Self {
        let r = registry.sub_registry_with_prefix("hashgram_node");
        let m = Self {
            peers_connected: Gauge::default(),
            peers_verified: Gauge::default(),
            peers_known: Gauge::default(),
            peers_banned: Gauge::default(),
            handshakes_ok: Counter::default(),
            handshakes_failed: Family::default(),
            requests_in: Family::default(),
            requests_out: Family::default(),
            requests_refused: Counter::default(),
            requests_failed: Counter::default(),
            gossip_in: Family::default(),
            gossip_rejected: Counter::default(),
            gossip_out: Family::default(),
            connections_refused: Counter::default(),
            reachable: Gauge::default(),
            kad_peers: Gauge::default(),
        };
        r.register(
            "peers_connected",
            "Connected peers",
            m.peers_connected.clone(),
        );
        r.register(
            "peers_verified",
            "Peers past the Hashgram handshake",
            m.peers_verified.clone(),
        );
        r.register(
            "peers_known",
            "Peers in the persistent peerstore",
            m.peers_known.clone(),
        );
        r.register(
            "peers_banned",
            "Peers banned by score",
            m.peers_banned.clone(),
        );
        r.register(
            "handshakes_ok",
            "Completed handshakes",
            m.handshakes_ok.clone(),
        );
        r.register(
            "handshakes_failed",
            "Refused handshakes by reason",
            m.handshakes_failed.clone(),
        );
        r.register(
            "requests_in",
            "Inbound RPC requests by kind",
            m.requests_in.clone(),
        );
        r.register(
            "requests_out",
            "Outbound RPC requests by kind",
            m.requests_out.clone(),
        );
        r.register(
            "requests_refused",
            "Inbound requests refused before handling",
            m.requests_refused.clone(),
        );
        r.register(
            "requests_failed",
            "Outbound requests that failed",
            m.requests_failed.clone(),
        );
        r.register(
            "gossip_in",
            "Gossip messages received by topic family",
            m.gossip_in.clone(),
        );
        r.register(
            "gossip_rejected",
            "Gossip messages rejected as invalid",
            m.gossip_rejected.clone(),
        );
        r.register(
            "gossip_out",
            "Gossip messages published by topic family",
            m.gossip_out.clone(),
        );
        r.register(
            "connections_refused",
            "Connections refused by local limits",
            m.connections_refused.clone(),
        );
        r.register(
            "reachable",
            "Autonat reachability: 1 public, 0 private, -1 unknown",
            m.reachable.clone(),
        );
        r.register(
            "kad_peers",
            "Peers in the Kademlia routing table",
            m.kad_peers.clone(),
        );
        m.reachable.set(-1);
        m
    }

    /// Classifies a topic name into its family.
    #[must_use]
    pub fn topic_family(topic: &str) -> TopicFamily {
        // Topics are "hashgram/<network>/<family>/...".
        let family = topic.split('/').nth(2).unwrap_or("");
        match family {
            "social" => TopicFamily::Social,
            "channel" => TopicFamily::Channel,
            "tag" => TopicFamily::Tag,
            "mailbox" => TopicFamily::Mailbox,
            "announce" => TopicFamily::Announce,
            "safety" => TopicFamily::Safety,
            _ => TopicFamily::Other,
        }
    }
}

/// Classifies a request body.
#[must_use]
pub fn request_kind(req: &hashgram_proto::pb::Request) -> RequestKind {
    use hashgram_proto::pb::request::Body as B;
    match &req.body {
        Some(B::Handshake(_)) => RequestKind::Handshake,
        Some(B::MailboxPut(_) | B::MailboxFetch(_) | B::MailboxAck(_)) => RequestKind::Mailbox,
        Some(B::KeyPackagePublish(_) | B::KeyPackageFetch(_)) => RequestKind::KeyPackage,
        Some(B::EventFetch(_) | B::EventPublish(_)) => RequestKind::Event,
        Some(
            B::BlobGetManifest(_)
            | B::BlobGetChunk(_)
            | B::BlobHas(_)
            | B::BlobPutManifest(_)
            | B::BlobPutChunk(_),
        ) => RequestKind::Blob,
        Some(B::AnnounceQuery(_)) => RequestKind::Announce,
        Some(B::PeerExchange(_)) => RequestKind::PeerExchange,
        Some(B::AttestationQuery(_)) => RequestKind::Attestation,
        Some(B::ReceiptDeliver(_)) => RequestKind::Receipt,
        Some(B::TurnCredentials(_)) => RequestKind::Calls,
        None => RequestKind::Unknown,
    }
}
