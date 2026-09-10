//! Hashgram Mainnet launch facts, compiled in.
//!
//! Mainnet (`hashgram-1`) launched on 2026-09-10. Its genesis hash and the
//! first-contact peer lists are historical facts and travel with the binary,
//! the way bitcoind carries its genesis block and fixed seeds. This module
//! is the Rust half of `app/params/mainnet.go`; both read the **same files**
//! under `app/params/mainnet/`, so the two implementations cannot drift.
//!
//! Nothing here is consulted after first contact. A node with a persisted
//! peerstore never reads these lists again, and the genesis hash pinned in
//! `/etc/hashgram/network.json` is what the handshake verifies. These are
//! defaults for a machine that knows nothing yet; each entry is a point of
//! trust chosen at build time (docs/DECENTRALIZATION.md).

/// Lowercase hex SHA-256 of the canonical Mainnet genesis file.
pub const MAINNET_GENESIS_HASH: &str =
    "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d";

/// The canonical Mainnet genesis file, byte for byte.
pub const MAINNET_GENESIS: &[u8] = include_bytes!("../../../app/params/mainnet/genesis.json");

const BOOTSTRAP_PEERS_FILE: &str = include_str!("../../../app/params/mainnet/bootstrap_peers.txt");
const DNS_SEEDS_FILE: &str = include_str!("../../../app/params/mainnet/dns_seeds.txt");

/// Non-empty, non-comment lines of a list file.
fn list(file: &'static str) -> impl Iterator<Item = &'static str> {
    file.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// Built-in libp2p bootstrap multiaddrs, each ending in `/p2p/<peer-id>`.
pub fn mainnet_bootstrap_peers() -> Vec<String> {
    list(BOOTSTRAP_PEERS_FILE).map(str::to_owned).collect()
}

/// Built-in `/dnsaddr` names. Empty until an operator publishes one.
pub fn mainnet_dns_seeds() -> Vec<String> {
    list(DNS_SEEDS_FILE).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute_genesis_hash;

    #[test]
    fn embedded_genesis_hashes_to_the_pin() {
        assert_eq!(compute_genesis_hash(MAINNET_GENESIS), MAINNET_GENESIS_HASH);
    }

    #[test]
    fn bootstrap_peers_are_present_and_carry_peer_ids() {
        let peers = mainnet_bootstrap_peers();
        assert!(
            !peers.is_empty(),
            "a fresh node would have nowhere to start"
        );
        for p in &peers {
            assert!(p.starts_with('/'), "{p}");
            assert!(p.contains("/p2p/"), "{p} lacks /p2p/<peer-id>");
        }
    }

    #[test]
    fn dns_seeds_are_bare_names() {
        for d in mainnet_dns_seeds() {
            assert!(!d.starts_with('/') && !d.starts_with('_'), "{d}");
        }
    }
}
