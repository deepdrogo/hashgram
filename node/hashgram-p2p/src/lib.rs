//! The Hashgram libp2p layer: transport, discovery, peer scoring and the
//! network handshake that CometBFT does not perform.
//!
//! # What this is for
//!
//! The chain carries what must be globally agreed. This layer carries what
//! must not: encrypted message envelopes, social events, blob chunks. Putting
//! those on chain would make them permanent, public and replicated forever,
//! and would make throughput a consensus problem.
//!
//! # Transport
//!
//! QUIC first, TCP as a fallback. QUIC is preferred because it does its own
//! encryption and multiplexing, so a connection needs one round trip rather
//! than three, and because it survives a client changing network without
//! re-establishing. TCP with Noise and Yamux stays because QUIC is UDP and
//! some networks block or throttle UDP outright.
//!
//! # Behaviours
//!
//! | Behaviour | Why |
//! | --- | --- |
//! | Request-response (`/hashgram/rpc/1`) | Handshake, mailbox, blob, queries |
//! | Gossipsub | Social events, announcements, attestations, mailbox notifies |
//! | Kademlia | Provider discovery for blobs and mailboxes, peer discovery |
//! | Identify | Learn a peer's addresses and protocols |
//! | Autonat | Learn whether we are publicly reachable before advertising |
//! | Relay, DCUtR | Reach peers behind NAT, then hole-punch |
//! | Ping | Liveness, and a latency signal for peer scoring |
//!
//! # The handshake gate
//!
//! Every connection begins with the Hashgram handshake, which verifies all
//! five parts of the network identity including the genesis hash. Nothing
//! else is served to a peer that has not passed it. See [`swarm`].

#![forbid(unsafe_code)]
// The workspace denies panicking constructs because this daemon parses
// attacker-controlled bytes: an unwrap on a malformed frame is a remote
// denial of service. Test code is a different situation. A test that asserts
// a value is present, or indexes a fixture it just built, is clearer with
// expect than with a match arm that cannot be taken, and a test panicking is
// the test failing rather than a node crashing.
#![cfg_attr(
    test,
    allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::assertions_on_constants,
    )
)]

mod behaviour;
mod codec;
mod config;
mod limits;
pub mod metrics;
pub mod peerstore;
mod score;
pub mod swarm;
pub mod transport;

pub use codec::RPC_PROTOCOL;
pub use config::{ConfigError, NodeConfig, Transport, KNOWN_ROLES};
pub use limits::{ConnectionLimits, LimitDecision, DEFAULT_MAX_PER_SUBNET};
pub use score::{PeerScore, ScoreEvent, Scoreboard, BAN_THRESHOLD, GRAYLIST_THRESHOLD};
pub use swarm::{start, Command, Event, NodeHandle, PeerSummary, RequestError, StartError, Stats};

/// Re-exported so callers do not need a direct libp2p dependency for the
/// handful of types that cross the handle boundary.
pub use libp2p::gossipsub::{MessageAcceptance, MessageId};
pub use libp2p::identity as libp2p_identity;
pub use libp2p::identity::Keypair;
pub use libp2p::multiaddr::Protocol;
pub use libp2p::request_response::ResponseChannel;
pub use libp2p::{Multiaddr, PeerId};

/// Gossip topic names. One place, so the publisher and every subscriber
/// agree, and so a topic can be recognised by its family for metrics.
pub mod topics {
    /// Number of social shards. Authors are spread across them by address
    /// hash so no single topic carries every post on the network.
    pub const SOCIAL_SHARDS: u32 = 64;

    fn prefix(network_id: &str) -> String {
        format!("hashgram/{network_id}")
    }

    /// The shard an author's events are published on.
    #[must_use]
    pub fn social_shard(network_id: &str, author: &str) -> String {
        let h = blake3::hash(author.as_bytes());
        let n = u32::from_be_bytes([
            h.as_bytes()[0],
            h.as_bytes()[1],
            h.as_bytes()[2],
            h.as_bytes()[3],
        ]) % SOCIAL_SHARDS;
        format!("{}/social/shard/{n}", prefix(network_id))
    }

    /// All social shard topics, for a node that indexes everything.
    #[must_use]
    pub fn all_social_shards(network_id: &str) -> Vec<String> {
        (0..SOCIAL_SHARDS)
            .map(|n| format!("{}/social/shard/{n}", prefix(network_id)))
            .collect()
    }

    /// A channel's topic.
    #[must_use]
    pub fn channel(network_id: &str, channel_id_hex: &str) -> String {
        format!("{}/channel/{channel_id_hex}", prefix(network_id))
    }

    /// A hashtag's topic, by hash so the tag itself is not in the topic.
    #[must_use]
    pub fn tag(network_id: &str, tag: &str) -> String {
        let h = blake3::hash(tag.to_lowercase().as_bytes());
        format!(
            "{}/tag/{}",
            prefix(network_id),
            hex::encode(&h.as_bytes()[..8])
        )
    }

    /// Mailbox notification shard for a mailbox id.
    #[must_use]
    pub fn mailbox_shard(network_id: &str, mailbox: &[u8]) -> String {
        let n = mailbox.first().copied().unwrap_or(0) as u32 % 16;
        format!("{}/mailbox/{n}", prefix(network_id))
    }

    /// Node announcements.
    #[must_use]
    pub fn announce(network_id: &str) -> String {
        format!("{}/announce", prefix(network_id))
    }

    /// Content safety attestations.
    #[must_use]
    pub fn safety(network_id: &str) -> String {
        format!("{}/safety", prefix(network_id))
    }
}
