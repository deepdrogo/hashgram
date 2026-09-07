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
//! | Gossipsub | Social events and envelope announcements, partitioned by topic |
//! | Kademlia | Provider discovery for blobs, and peer discovery |
//! | Identify | Learn a peer's addresses and protocols |
//! | Autonat | Learn whether we are publicly reachable before advertising |
//! | Relay | Reach peers behind NAT that autonat says are unreachable |
//! | Ping | Liveness, and a latency signal for peer scoring |
//!
//! # Status
//!
//! Peer scoring, connection limits, the persistent peerstore and the
//! handshake are implemented and tested here. Wiring them into a running
//! swarm, and the messaging and storage protocols that ride on top, is the
//! remainder of Phase 2.

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

mod config;
mod limits;
mod score;

pub use config::{ConfigError, NodeConfig, Transport};
pub use limits::{ConnectionLimits, LimitDecision};
pub use score::{PeerScore, ScoreEvent, Scoreboard, BAN_THRESHOLD, GRAYLIST_THRESHOLD};
