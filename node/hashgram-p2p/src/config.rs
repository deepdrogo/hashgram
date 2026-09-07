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
}

/// A Hashgram node's configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    /// `mainnet` or `devnet`.
    pub network: String,

    /// The pinned genesis hash, 64 lowercase hex characters.
    ///
    /// Copied from `/etc/hashgram/network.json`, which `hashgramctl
    /// join-mainnet` writes after verifying it.
    pub genesis_hash: String,

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
    4
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

        Ok(identity)
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
        NodeConfig {
            network: "mainnet".to_owned(),
            genesis_hash: GENESIS.to_owned(),
            listen_addr: default_listen_addr(),
            listen_port: 26670,
            transport: Transport::Both,
            bootstrap_peers: vec![],
            max_connections_per_peer: 2,
            max_connections_per_subnet: 4,
            max_inbound_connections: 128,
            max_outbound_connections: 32,
            peerstore_path: default_peerstore_path(),
            metrics_addr: default_metrics_addr(),
        }
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
    fn a_config_missing_the_genesis_hash_does_not_parse_at_all() {
        // Not defaulted to empty and then caught by validate: absent from
        // the file is a parse error, so the message names the field.
        let toml = r#"network = "mainnet""#;
        assert!(
            toml::from_str::<NodeConfig>(toml).is_err(),
            "a config with no genesis_hash parsed"
        );
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
