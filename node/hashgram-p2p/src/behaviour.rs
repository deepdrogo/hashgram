//! The composed libp2p behaviour.
//!
//! | Behaviour | Why |
//! | --- | --- |
//! | `rpc` | Handshake, mailbox, blob and query traffic on `/hashgram/rpc/1` |
//! | `gossipsub` | Social events, announcements, attestations, mailbox notifies |
//! | `kad` | Provider records for blobs and mailboxes; peer discovery |
//! | `identify` | Learn peers' addresses and protocols |
//! | `ping` | Liveness and a latency signal |
//! | `autonat` | Learn whether we are publicly reachable before advertising |
//! | `relay` | Serve circuit relay for peers behind NAT (relay role) |
//! | `relay_client`, `dcutr` | Reach and be reached through relays, then hole-punch |
//! | `blocked` | Banned peers, refused at connection |
//! | `limits` | Hard connection caps the swarm enforces before our own accounting |

use std::time::Duration;

use hashgram_proto::limits::MAX_GOSSIP_FRAME;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{
    allow_block_list, autonat, connection_limits, dcutr, gossipsub, identify, identity, kad, ping,
    relay, request_response, PeerId, StreamProtocol,
};

use crate::codec::{RpcCodec, RPC_PROTOCOL};
use crate::config::NodeConfig;

/// The Kademlia protocol id. Network-specific so a fork's DHT and ours do
/// not merge even if a peer gets through everything else.
fn kad_protocol(network_id: &str) -> StreamProtocol {
    // StreamProtocol::try_from_owned refuses strings without a leading '/'.
    // network_id is validated at config load to be one of two constants, so
    // this cannot fail, but the type is total anyway.
    StreamProtocol::try_from_owned(format!("/hashgram/{network_id}/kad/1"))
        .unwrap_or_else(|_| StreamProtocol::new("/hashgram/kad/1"))
}

/// The identify protocol id.
pub(crate) const IDENTIFY_PROTOCOL: &str = "/hashgram/id/1";

/// Everything the swarm runs.
#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    pub(crate) blocked: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    pub(crate) limits: connection_limits::Behaviour,
    pub(crate) rpc: request_response::Behaviour<RpcCodec>,
    pub(crate) gossipsub: gossipsub::Behaviour,
    pub(crate) kad: kad::Behaviour<kad::store::MemoryStore>,
    pub(crate) identify: identify::Behaviour,
    pub(crate) ping: ping::Behaviour,
    pub(crate) autonat: autonat::Behaviour,
    pub(crate) relay: libp2p::swarm::behaviour::toggle::Toggle<relay::Behaviour>,
    pub(crate) relay_client: relay::client::Behaviour,
    pub(crate) dcutr: dcutr::Behaviour,
}

/// Gossipsub message id: BLAKE3 of the payload, so the same event arriving
/// from two peers is one message, and a peer cannot make a duplicate look
/// new by re-signing it (gossipsub's own signature is over the envelope,
/// not part of the id).
fn message_id(msg: &gossipsub::Message) -> gossipsub::MessageId {
    gossipsub::MessageId::from(blake3::hash(&msg.data).as_bytes().to_vec())
}

impl Behaviour {
    /// Builds the behaviour set for a node.
    pub(crate) fn new(
        key: &identity::Keypair,
        cfg: &NodeConfig,
        network_id: &str,
        relay_client: relay::client::Behaviour,
        serve_relay: bool,
    ) -> Result<Self, String> {
        let peer_id = PeerId::from(key.public());

        let rpc = request_response::Behaviour::with_codec(
            RpcCodec,
            [(RPC_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(30))
                .with_max_concurrent_streams(64),
        );

        let gossip_cfg = gossipsub::ConfigBuilder::default()
            .max_transmit_size(MAX_GOSSIP_FRAME)
            .validation_mode(gossipsub::ValidationMode::Strict)
            .validate_messages()
            .message_id_fn(message_id)
            .duplicate_cache_time(Duration::from_secs(120))
            .heartbeat_interval(Duration::from_secs(1))
            .mesh_n(6)
            .mesh_n_low(4)
            .mesh_n_high(12)
            .flood_publish(false)
            .build()
            .map_err(|e| format!("gossipsub config: {e}"))?;
        let gossipsub = gossipsub::Behaviour::new(
            gossipsub::MessageAuthenticity::Signed(key.clone()),
            gossip_cfg,
        )
        .map_err(|e| format!("gossipsub: {e}"))?;

        let mut kad_cfg = kad::Config::new(kad_protocol(network_id));
        kad_cfg
            .set_query_timeout(Duration::from_secs(30))
            .set_provider_record_ttl(Some(Duration::from_secs(24 * 3600)))
            .set_provider_publication_interval(Some(Duration::from_secs(6 * 3600)))
            .set_periodic_bootstrap_interval(Some(Duration::from_secs(300)));
        let mut kad =
            kad::Behaviour::with_config(peer_id, kad::store::MemoryStore::new(peer_id), kad_cfg);
        // Server mode: answer queries. A node that only reads the DHT is a
        // free rider; every Hashgram node that can be reached serves it.
        kad.set_mode(Some(kad::Mode::Server));

        let identify = identify::Behaviour::new(
            identify::Config::new(IDENTIFY_PROTOCOL.to_owned(), key.public())
                .with_agent_version(format!("hashgram-node/{}", env!("CARGO_PKG_VERSION")))
                .with_interval(Duration::from_secs(300)),
        );

        let ping = ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(30)));

        let autonat = autonat::Behaviour::new(
            peer_id,
            autonat::Config {
                boot_delay: Duration::from_secs(15),
                refresh_interval: Duration::from_secs(300),
                ..Default::default()
            },
        );

        let relay = if serve_relay {
            Some(relay::Behaviour::new(
                peer_id,
                relay::Config {
                    max_reservations: 128,
                    max_reservations_per_peer: 4,
                    max_circuits: 64,
                    max_circuits_per_peer: 4,
                    max_circuit_duration: Duration::from_secs(2 * 3600),
                    max_circuit_bytes: 64 << 20,
                    ..Default::default()
                },
            ))
        } else {
            None
        }
        .into();

        let limits = connection_limits::Behaviour::new(
            connection_limits::ConnectionLimits::default()
                .with_max_established_incoming(Some(cfg.max_inbound_connections as u32))
                .with_max_established_outgoing(Some(cfg.max_outbound_connections as u32))
                .with_max_established_per_peer(Some(cfg.max_connections_per_peer as u32))
                .with_max_pending_incoming(Some(64))
                .with_max_pending_outgoing(Some(64)),
        );

        Ok(Self {
            blocked: allow_block_list::Behaviour::default(),
            limits,
            rpc,
            gossipsub,
            kad,
            identify,
            ping,
            autonat,
            relay,
            relay_client,
            dcutr: dcutr::Behaviour::new(peer_id),
        })
    }
}
