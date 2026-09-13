//! Node configuration.
//!
//! Loaded from TOML at `/etc/hashgram/node.toml`. Every field is validated on
//! load rather than on first use, so a typo is a startup failure with a clear
//! message rather than a runtime surprise hours later.

use std::net::{IpAddr, Ipv4Addr};

use hashgram_net::NetworkIdentity;
use serde::{Deserialize, Serialize};

/// Which transports to listen on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// QUIC only. One round trip to connect and survives a network change,
    /// but it is UDP and some networks block or throttle UDP outright.
    Quic,
    /// TCP with Noise and Yamux only. Slower to establish, but reachable from
    /// networks that drop UDP.
    Tcp,
    /// Both. The default, because neither alone is reachable everywhere.
    #[default]
    Both,
}

/// Why a configuration was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The genesis hash is absent or malformed.
    ///
    /// Refused at load rather than at first handshake. A node with no pinned
    /// genesis cannot verify a peer, so it would join whichever network
    /// reached it first.
    #[error(
        "genesis_hash is {reason}. A node with no pinned genesis cannot verify a peer \
             and would join whichever network reached it first. Run \
             `hashgramctl join-mainnet --genesis-hash <sha256>` and copy the pin from \
             /etc/hashgram/network.json"
    )]
    GenesisHash {
        /// What is wrong with it.
        reason: String,
    },

    /// The listen port is zero.
    ///
    /// Port zero means "any free port", which is useless for a node that
    /// needs to be findable at a stable address.
    #[error(
        "listen_port is 0, which asks the kernel for any free port. \
             A node that peers cannot find again is not reachable"
    )]
    ZeroPort,

    /// No transport is enabled.
    #[error("no transport is enabled")]
    NoTransport,

    /// A bootstrap peer address could not be understood.
    #[error("bootstrap peer {index} ({value:?}) is not a valid multiaddr: {reason}")]
    BootstrapAddr {
        /// Which entry.
        index: usize,
        /// What was configured.
        value: String,
        /// Why it was refused.
        reason: String,
    },

    /// The network name is neither mainnet nor devnet.
    #[error("network {value:?} is unknown; expected \"mainnet\" or \"devnet\"")]
    UnknownNetwork {
        /// What was configured.
        value: String,
    },

    /// A role this build does not know.
    #[error("role {value:?} is unknown; valid roles: relay, store, media, bootstrap, call, indexer, safety")]
    UnknownRole {
        /// What was configured.
        value: String,
    },
}

/// Roles the P2P node understands. `validator` is a chain-node role and is
/// deliberately absent: the P2P daemon never holds a consensus key.
pub const KNOWN_ROLES: [&str; 7] = [
    "relay",
    "store",
    "media",
    "bootstrap",
    "call",
    "indexer",
    "safety",
];

/// A Hashgram node's configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// `mainnet` or `devnet`.
    ///
    /// May be left empty in the file, in which case the daemon fills it from
    /// `/etc/hashgram/network.json`, the pin `hashgramctl join-mainnet`
    /// writes. Validation refuses an empty value either way.
    #[serde(default)]
    pub network: String,

    /// The pinned genesis hash, 64 lowercase hex characters.
    ///
    /// Same source as `network`. Present here so a node can be configured
    /// from one file in tests; in production the pin file is authoritative
    /// and a disagreement between the two is a startup failure.
    #[serde(default)]
    pub genesis_hash: String,

    /// Roles this node serves: any of `relay`, `store`, `media`,
    /// `bootstrap`, `call`, `indexer`, `safety`. Filled from
    /// `/etc/hashgram/roles.json` when empty.
    #[serde(default)]
    pub roles: Vec<String>,

    /// Where the node keeps its key, stores and indexes.
    #[serde(default = "default_data_dir")]
    pub data_dir: String,

    /// Address to listen on.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: IpAddr,

    /// Port to listen on, for both QUIC and TCP.
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Which transports to enable.
    #[serde(default)]
    pub transport: Transport,

    /// Multiaddrs of bootstrap peers.
    ///
    /// The first connection has to come from somewhere. This list ships with
    /// the release, which is a centralisation point recorded in
    /// docs/DECENTRALIZATION.md; more independent bootstrap operators is the
    /// fix, and this field is how an operator uses them.
    #[serde(default)]
    pub bootstrap_peers: Vec<String>,

    /// Paths to signed bootstrap record files (`BootstrapRecord` protobuf).
    /// Records from signers not in `trusted_bootstrap_signers` are ignored.
    #[serde(default)]
    pub bootstrap_records: Vec<String>,

    /// Hex ed25519 public keys allowed to sign bootstrap records.
    #[serde(default)]
    pub trusted_bootstrap_signers: Vec<String>,

    /// Optional DNS names carrying `_dnsaddr` TXT records with multiaddrs.
    /// Convenience only: a node with an empty list and a populated
    /// peerstore never touches DNS.
    #[serde(default)]
    pub dnsaddr: Vec<String>,

    /// External multiaddrs to announce, for nodes behind a known NAT or on a
    /// host with a fixed public address. Empty means "learn from autonat".
    #[serde(default)]
    pub announce_addrs: Vec<String>,

    /// Keep dialling bootstrap candidates until this many peers are
    /// verified.
    #[serde(default = "default_min_peers")]
    pub min_peers: usize,

    /// Serve circuit relay for peers behind NAT. Costs bandwidth; the
    /// default follows the `relay` role.
    #[serde(default)]
    pub serve_relay: Option<bool>,

    /// Dial TCP before QUIC when a peer offers both. For a client behind a
    /// consumer NAT this is the difference between a connection that lasts
    /// and one that dies when the router forgets a UDP mapping: TCP
    /// mappings live for hours, UDP ones often for 30 seconds. QUIC is
    /// still tried when TCP fails (every second attempt), so a network that
    /// filters TCP is not cut off. Off by default; clients turn it on.
    #[serde(default)]
    pub prefer_tcp: bool,

    /// Maximum connections from one peer.
    #[serde(default = "default_max_per_peer")]
    pub max_connections_per_peer: usize,

    /// Maximum connections from one /24 or /64.
    #[serde(default = "default_max_per_subnet")]
    pub max_connections_per_subnet: usize,

    /// Maximum inbound connections.
    #[serde(default = "default_max_inbound")]
    pub max_inbound_connections: usize,

    /// Maximum outbound connections. Reserved, so inbound pressure cannot
    /// isolate this node.
    #[serde(default = "default_max_outbound")]
    pub max_outbound_connections: usize,

    /// Where to persist the peerstore.
    ///
    /// Persisted so a restart does not start from the bundled bootstrap list
    /// again, which would make every restart a fresh dependence on whoever
    /// built the release.
    #[serde(default = "default_peerstore_path")]
    pub peerstore_path: String,

    /// Where to bind the Prometheus metrics endpoint.
    ///
    /// Localhost by default. This endpoint describes the node's internals in
    /// detail; exposing it hands an attacker a reconnaissance feed.
    #[serde(default = "default_metrics_addr")]
    pub metrics_addr: String,

    /// Where to bind the local JSON API used by `hashgramctl`, the indexer,
    /// the safety engine and same-host clients. Localhost by default for the
    /// same reason as metrics.
    #[serde(default = "default_api_addr")]
    pub api_addr: String,

    /// CometBFT RPC of the co-located chain node, for identity and provider
    /// lookups.
    #[serde(default = "default_chain_rpc")]
    pub chain_rpc: String,

    /// Cosmos SDK REST (gRPC-gateway) of the co-located chain node.
    #[serde(default = "default_chain_api")]
    pub chain_api: String,

    /// Storage the `store`/`media` roles may use, in bytes. Zero means
    /// "unlimited", which is never what an operator wants on a shared disk,
    /// so `hashgramctl configure-role --declared-storage` always sets it.
    #[serde(default)]
    pub storage_quota_bytes: u64,

    /// Hex ed25519 public keys of content-safety attestors this node
    /// honours. A block from an unlisted attestor is recorded but not
    /// enforced.
    #[serde(default)]
    pub trusted_attestors: Vec<String>,

    /// Address that receives useful-service rewards. Distinct from any key
    /// on this machine.
    #[serde(default)]
    pub reward_address: String,

    /// TURN URIs this node announces when serving the `call` role, e.g.
    /// `turn:203.0.113.5:3478?transport=udp`.
    #[serde(default)]
    pub turn_uris: Vec<String>,

    /// The coturn realm.
    #[serde(default)]
    pub turn_realm: String,

    /// Path to a file holding coturn's `static-auth-secret`, readable by the
    /// node's service account and nobody else. When set, the node issues
    /// time-limited TURN credentials to authenticated devices.
    #[serde(default)]
    pub turn_secret_file: String,

    /// WebSocket URL of a co-located SFU (LiveKit), announced under the
    /// `call` role when set.
    #[serde(default)]
    pub sfu_url: String,

    /// Path to the provider operator key: a hex secp256k1 secret, 0600,
    /// owned by the node's service account. Hot because it signs challenge
    /// answers and receipt submissions unattended. It is not the reward
    /// address and must never be a validator or Founder key.
    #[serde(default)]
    pub operator_key_file: String,

    /// The operator address derived from that key, filled at startup.
    #[serde(default)]
    pub operator_address: String,

    /// Register as a provider automatically when not registered.
    #[serde(default)]
    pub auto_register_provider: bool,

    /// Bond to post at registration, in uhash. The chain's minimum is
    /// 1,000 HASH.
    #[serde(default = "default_bond")]
    pub provider_bond_uhash: u64,

    /// Human-readable provider label.
    #[serde(default)]
    pub moniker: String,

    /// How long to keep social events, in days. Events are also bounded by
    /// `max_events`, whichever is reached first.
    #[serde(default = "default_event_retention_days")]
    pub event_retention_days: u32,

    /// Most social events kept on this node.
    #[serde(default = "default_max_events")]
    pub max_events: u64,
}

fn default_bond() -> u64 {
    1_000_000_000
}
fn default_event_retention_days() -> u32 {
    90
}
fn default_max_events() -> u64 {
    2_000_000
}

fn default_data_dir() -> String {
    "/var/lib/hashgram/node".to_owned()
}
fn default_min_peers() -> usize {
    4
}
fn default_api_addr() -> String {
    "127.0.0.1:26672".to_owned()
}
fn default_chain_rpc() -> String {
    "http://127.0.0.1:26657".to_owned()
}
fn default_chain_api() -> String {
    "http://127.0.0.1:1317".to_owned()
}

fn default_listen_addr() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}
fn default_listen_port() -> u16 {
    26670
}
fn default_max_per_peer() -> usize {
    2
}
fn default_max_per_subnet() -> usize {
    crate::limits::DEFAULT_MAX_PER_SUBNET
}
fn default_max_inbound() -> usize {
    128
}
fn default_max_outbound() -> usize {
    32
}
fn default_peerstore_path() -> String {
    "/var/lib/hashgram/node/peerstore.json".to_owned()
}
fn default_metrics_addr() -> String {
    "127.0.0.1:26671".to_owned()
}

impl NodeConfig {
    /// Validates the configuration and resolves the network identity.
    ///
    /// Everything is checked here, at load, so a typo is a startup failure
    /// with a message naming the field rather than a runtime surprise on the
    /// first connection attempt.
    pub fn validate(&self) -> Result<NetworkIdentity, ConfigError> {
        let identity = match self.network.as_str() {
            "mainnet" => NetworkIdentity::mainnet(&self.genesis_hash),
            "devnet" => NetworkIdentity::devnet(&self.genesis_hash),
            other => {
                return Err(ConfigError::UnknownNetwork {
                    value: other.to_owned(),
                })
            }
        };

        if self.genesis_hash.is_empty() {
            return Err(ConfigError::GenesisHash {
                reason: "empty".to_owned(),
            });
        }
        if !NetworkIdentity::is_well_formed_genesis_hash(&identity.genesis_hash) {
            return Err(ConfigError::GenesisHash {
                reason: format!(
                    "{:?}, which is not 64 lowercase hex characters",
                    self.genesis_hash
                ),
            });
        }

        if self.listen_port == 0 {
            return Err(ConfigError::ZeroPort);
        }

        for (index, addr) in self.bootstrap_peers.iter().enumerate() {
            if let Err(reason) = check_multiaddr(addr) {
                return Err(ConfigError::BootstrapAddr {
                    index,
                    value: addr.clone(),
                    reason,
                });
            }
        }

        for role in &self.roles {
            if !KNOWN_ROLES.contains(&role.as_str()) {
                return Err(ConfigError::UnknownRole {
                    value: role.clone(),
                });
            }
        }

        Ok(identity)
    }

    /// Whether this node serves a role.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role)
    }

    /// Whether to serve circuit relay: the explicit setting, else the role.
    #[must_use]
    pub fn serves_relay(&self) -> bool {
        self.serve_relay
            .unwrap_or_else(|| self.has_role("relay") || self.has_role("bootstrap"))
    }

    /// Whether this node stores envelopes, key packages and blobs.
    #[must_use]
    pub fn stores(&self) -> bool {
        self.has_role("store") || self.has_role("media")
    }

    /// Whether the metrics endpoint is bound to a loopback address.
    ///
    /// Reported rather than enforced, because an operator with a private
    /// management network has a legitimate reason to bind elsewhere. The node
    /// logs a warning at startup when this is false, so the decision is
    /// visible rather than silent.
    #[must_use]
    pub fn metrics_is_loopback(&self) -> bool {
        self.metrics_addr.starts_with("127.")
            || self.metrics_addr.starts_with("[::1]")
            || self.metrics_addr.starts_with("localhost:")
    }
}

/// A conservative multiaddr shape check.
///
/// Deliberately not a full parse. The purpose is to catch the configuration
/// mistakes people actually make — pasting a bare `host:port`, or a URL —
/// with a message that says what was expected. libp2p performs the real parse
/// when the address is dialled, and duplicating its grammar here would mean
/// two parsers that can disagree.
fn check_multiaddr(addr: &str) -> Result<(), String> {
    if addr.is_empty() {
        return Err("empty".to_owned());
    }
    if !addr.starts_with('/') {
        return Err(format!(
            "a multiaddr starts with '/', for example \
             /ip4/203.0.113.1/udp/26670/quic-v1/p2p/12D3Koo…, not {addr:?}"
        ));
    }
    if !addr.contains("/p2p/") {
        return Err(
            "no /p2p/<peer-id> component. Without a peer id this node cannot verify \
             it reached the peer it intended, which is what makes a bootstrap \
             address trustworthy"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";

    fn valid() -> NodeConfig {
        let mut cfg: NodeConfig = toml::from_str("").expect("all fields default");
        cfg.network = "mainnet".to_owned();
        cfg.genesis_hash = GENESIS.to_owned();
        cfg
    }

    #[test]
    fn a_valid_config_resolves_its_identity() {
        let id = valid().validate().expect("a valid config was refused");
        assert!(id.is_mainnet());
        assert_eq!(id.genesis_hash, GENESIS);
    }

    #[test]
    fn a_missing_genesis_hash_is_refused_at_load() {
        // Refused here rather than at the first handshake, because a node
        // with no pin would join whichever network reached it first.
        let mut cfg = valid();
        cfg.genesis_hash = String::new();

        let err = cfg.validate().expect_err("an unpinned config was accepted");
        assert!(matches!(err, ConfigError::GenesisHash { .. }));
        assert!(
            err.to_string().contains("hashgramctl join-mainnet"),
            "the error does not say how to fix it: {err}"
        );
    }

    #[test]
    fn a_malformed_genesis_hash_is_refused() {
        for bad in ["abc", "zz", &"f".repeat(63), &"f".repeat(65)] {
            let mut cfg = valid();
            cfg.genesis_hash = bad.to_owned();
            assert!(cfg.validate().is_err(), "genesis hash {bad:?} was accepted");
        }
    }

    #[test]
    fn an_uppercase_genesis_hash_is_accepted_and_normalised() {
        // Operators copy hashes from terminals and documents. Rejecting an
        // uppercase one would be pedantry; silently comparing it wrongly
        // would be a bug. It is lowercased on construction.
        let mut cfg = valid();
        cfg.genesis_hash = GENESIS.to_ascii_uppercase();

        let id = cfg.validate().expect("an uppercase hash was refused");
        assert_eq!(id.genesis_hash, GENESIS);
    }

    #[test]
    fn an_unknown_network_is_refused() {
        let mut cfg = valid();
        cfg.network = "testnet".to_owned();

        let err = cfg.validate().expect_err("an unknown network was accepted");
        assert!(matches!(err, ConfigError::UnknownNetwork { .. }));
    }

    #[test]
    fn port_zero_is_refused() {
        let mut cfg = valid();
        cfg.listen_port = 0;
        assert_eq!(
            cfg.validate().expect_err("port 0 accepted"),
            ConfigError::ZeroPort
        );
    }

    #[test]
    fn a_bootstrap_address_without_a_peer_id_is_refused() {
        // Without a peer id, this node cannot verify it reached the peer it
        // intended, so the address provides no security at all.
        let mut cfg = valid();
        cfg.bootstrap_peers = vec!["/ip4/203.0.113.1/udp/26670/quic-v1".to_owned()];

        let err = cfg
            .validate()
            .expect_err("a peer-id-less address was accepted");
        assert!(
            err.to_string().contains("/p2p/"),
            "the error does not explain what is missing: {err}"
        );
    }

    #[test]
    fn a_host_port_pasted_instead_of_a_multiaddr_is_refused_helpfully() {
        // The mistake people actually make. The message has to show the
        // expected shape, not just say "invalid".
        let mut cfg = valid();
        cfg.bootstrap_peers = vec!["203.0.113.1:26670".to_owned()];

        let err = cfg.validate().expect_err("a host:port was accepted");
        let text = err.to_string();
        assert!(text.contains("starts with '/'"), "unhelpful: {text}");
        assert!(text.contains("/quic-v1/"), "no example given: {text}");
    }

    #[test]
    fn the_offending_bootstrap_entry_is_identified() {
        // With a list of ten, "one of them is wrong" is not actionable.
        let mut cfg = valid();
        cfg.bootstrap_peers = vec![
            "/ip4/203.0.113.1/udp/26670/quic-v1/p2p/12D3KooWtest".to_owned(),
            "/ip4/203.0.113.2/udp/26670/quic-v1/p2p/12D3KooWtest2".to_owned(),
            "broken".to_owned(),
        ];

        match cfg.validate().expect_err("a broken entry was accepted") {
            ConfigError::BootstrapAddr { index, value, .. } => {
                assert_eq!(index, 2);
                assert_eq!(value, "broken");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn valid_bootstrap_addresses_are_accepted() {
        let mut cfg = valid();
        cfg.bootstrap_peers = vec![
            "/ip4/203.0.113.1/udp/26670/quic-v1/p2p/12D3KooWtest".to_owned(),
            "/ip4/203.0.113.2/tcp/26670/p2p/12D3KooWtest2".to_owned(),
            "/dns4/boot.example/udp/26670/quic-v1/p2p/12D3KooWtest3".to_owned(),
        ];
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn metrics_default_to_loopback() {
        assert!(valid().metrics_is_loopback());

        let mut cfg = valid();
        cfg.metrics_addr = "0.0.0.0:26671".to_owned();
        assert!(
            !cfg.metrics_is_loopback(),
            "a wildcard bind was reported as loopback"
        );
    }

    #[test]
    fn defaults_fill_in_from_a_minimal_toml() {
        // An operator should only have to write the two fields that cannot
        // have a default: which network, and its genesis hash.
        let toml = format!(
            r#"
            network = "devnet"
            genesis_hash = "{GENESIS}"
            "#
        );

        let cfg: NodeConfig = toml::from_str(&toml).expect("minimal config did not parse");
        let id = cfg.validate().expect("minimal config was refused");

        assert!(!id.is_mainnet());
        assert_eq!(cfg.listen_port, 26670);
        assert_eq!(cfg.transport, Transport::Both);
        assert!(cfg.metrics_is_loopback());
        assert_eq!(cfg.max_inbound_connections, 128);
    }

    #[test]
    fn a_config_missing_the_genesis_hash_fails_validation() {
        // Absent from the file is permitted, because the daemon fills the
        // pin from network.json; but validation must still refuse an empty
        // one, otherwise a node with no pin file at all would start.
        let toml = r#"network = "mainnet""#;
        let cfg: NodeConfig = toml::from_str(toml).expect("parses with defaults");
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::GenesisHash { .. })
        ));
    }

    #[test]
    fn an_unknown_role_is_refused() {
        let mut cfg = valid();
        cfg.roles = vec!["validator".to_owned()];
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::UnknownRole { .. })
        ));
    }

    #[test]
    fn relay_service_follows_the_role_unless_overridden() {
        let mut cfg = valid();
        assert!(!cfg.serves_relay());
        cfg.roles = vec!["relay".to_owned()];
        assert!(cfg.serves_relay());
        cfg.serve_relay = Some(false);
        assert!(!cfg.serves_relay());
    }

    #[test]
    fn config_round_trips_through_toml() {
        let cfg = valid();
        let text = toml::to_string(&cfg).expect("serialising");
        let back: NodeConfig = toml::from_str(&text).expect("deserialising");
        assert_eq!(back.network, cfg.network);
        assert_eq!(back.genesis_hash, cfg.genesis_hash);
        assert_eq!(back.listen_port, cfg.listen_port);
        assert_eq!(back.transport, cfg.transport);
    }
}
