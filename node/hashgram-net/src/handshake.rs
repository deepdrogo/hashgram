//! The Hashgram P2P handshake.
//!
//! # Why this exists
//!
//! CometBFT's peer handshake compares chain ids and nothing else. It does not
//! hash the genesis file. That was verified rather than assumed: a fork
//! keeping the real chain id and changing only the genesis was pointed at a
//! four-validator testnet, and the transport connection opened. Consensus
//! refused to accept its blocks, and the real chain was unaffected, but the
//! connection happened.
//!
//! Three layers turn a fork away today: the CometBFT handshake for a
//! different chain id, consensus for a same-chain-id fork, and
//! `hashgramctl join-mainnet` for an operator being handed the wrong file.
//! None of them stops a same-chain-id fork from opening a socket.
//!
//! This handshake closes that gap for the Hashgram P2P layer by verifying all
//! five identity parts. It runs before any application data is exchanged.
//!
//! # Design
//!
//! Checks are ordered cheapest first, and each returns a distinct error so an
//! operator sees *which* part disagreed. "Handshake failed" tells nobody
//! anything; "genesis hash mismatch, expected abc… received def…" tells them
//! they are on a fork.
//!
//! The remote is never trusted to describe itself accurately. Every field is
//! compared against the local pinned identity, and a field that is absent or
//! malformed is a rejection rather than a default.

use serde::{Deserialize, Serialize};

use crate::identity::{NetworkIdentity, GENESIS_HASH_LEN, MAGIC_LEN};

/// The handshake message each peer sends.
///
/// Deliberately contains no addresses, no service list and no version string
/// beyond the protocol major version. Everything here is compared against a
/// local constant, so there is nothing an attacker can put in it that changes
/// behaviour beyond causing a rejection. Service announcements are a separate,
/// signed message exchanged only after this succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// The four-byte network magic.
    pub network_magic: [u8; MAGIC_LEN],
    /// Application-level network id.
    pub network_id: String,
    /// CometBFT consensus chain id.
    pub chain_id: String,
    /// Lowercase hex SHA-256 of the genesis file.
    pub genesis_hash: String,
    /// Wire-compatibility generation.
    pub protocol_major_version: u32,
}

impl Handshake {
    /// Builds the handshake this node sends.
    #[must_use]
    pub fn for_identity(id: &NetworkIdentity) -> Self {
        Self {
            network_magic: id.network_magic,
            network_id: id.network_id.clone(),
            chain_id: id.chain_id.clone(),
            genesis_hash: id.genesis_hash.clone(),
            protocol_major_version: id.protocol_major_version,
        }
    }
}

/// Why a peer was rejected.
///
/// One variant per identity part, so the rejection reason is actionable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandshakeError {
    /// The magic did not match. A foreign network, or garbage.
    #[error(
        "network magic mismatch: expected {expected}, received {received}. \
             This peer is on a different Hashgram network, or is not a Hashgram node"
    )]
    MagicMismatch {
        /// The local magic, as a printable string.
        expected: String,
        /// The remote magic, as a printable string.
        received: String,
    },

    /// The protocol generation differs, so the same bytes may mean different
    /// things.
    #[error("protocol version mismatch: this node speaks v{expected}, the peer speaks v{received}. \
             Exchanging application data across a major version is not safe; one side needs an upgrade")]
    ProtocolMismatch {
        /// The local protocol major version.
        expected: u32,
        /// The remote protocol major version.
        received: u32,
    },

    /// A different application-level network.
    #[error("network id mismatch: expected {expected:?}, received {received:?}")]
    NetworkIdMismatch {
        /// The local network id.
        expected: String,
        /// The remote network id.
        received: String,
    },

    /// A different consensus chain.
    #[error("chain id mismatch: expected {expected:?}, received {received:?}")]
    ChainIdMismatch {
        /// The local chain id.
        expected: String,
        /// The remote chain id.
        received: String,
    },

    /// The genesis hash differs. This is the check CometBFT does not perform.
    #[error(
        "genesis hash mismatch: expected {expected}, received {received}. \
             This peer is running a fork: same software, different genesis. \
             CometBFT's own handshake does not check this, which is why this layer does"
    )]
    GenesisMismatch {
        /// The local pinned genesis hash.
        expected: String,
        /// The remote genesis hash.
        received: String,
    },

    /// The remote sent a genesis hash that is not a hex SHA-256.
    ///
    /// Distinct from a mismatch: a malformed value means a broken or hostile
    /// implementation, not a fork, and an operator should read it differently.
    #[error("malformed genesis hash {received:?}: expected {GENESIS_HASH_LEN} lowercase hex characters. \
             This is a broken or hostile peer rather than a fork")]
    MalformedGenesisHash {
        /// What the remote sent.
        received: String,
    },

    /// The local identity has no usable genesis hash.
    ///
    /// A node that has not pinned a genesis cannot verify anyone, so it
    /// refuses to complete a handshake at all rather than accepting whatever
    /// it is told. Accepting would make an unconfigured node join the first
    /// network that spoke to it.
    #[error(
        "this node has no pinned genesis hash, so it cannot verify a peer. \
             Run `hashgramctl join-mainnet --genesis-hash <sha256>` first"
    )]
    NoLocalGenesis,
}

/// The result of verifying a peer.
pub type HandshakeResult = Result<(), HandshakeError>;

impl NetworkIdentity {
    /// Verifies a peer's handshake against this node's pinned identity.
    ///
    /// Checks are ordered cheapest first: four bytes of magic reject a
    /// wrong-network peer before any string comparison. The genesis hash is
    /// checked last because it is the most expensive comparison and the least
    /// likely to differ when the earlier fields agree.
    pub fn verify_handshake(&self, remote: &Handshake) -> HandshakeResult {
        // Refuse to verify anything if we have nothing to verify against.
        // An unconfigured node that accepted peers would join whichever
        // network reached it first.
        if !Self::is_well_formed_genesis_hash(&self.genesis_hash) {
            return Err(HandshakeError::NoLocalGenesis);
        }

        if remote.network_magic != self.network_magic {
            return Err(HandshakeError::MagicMismatch {
                expected: printable_magic(&self.network_magic),
                received: printable_magic(&remote.network_magic),
            });
        }

        if remote.protocol_major_version != self.protocol_major_version {
            return Err(HandshakeError::ProtocolMismatch {
                expected: self.protocol_major_version,
                received: remote.protocol_major_version,
            });
        }

        if remote.network_id != self.network_id {
            return Err(HandshakeError::NetworkIdMismatch {
                expected: self.network_id.clone(),
                received: remote.network_id.clone(),
            });
        }

        if remote.chain_id != self.chain_id {
            return Err(HandshakeError::ChainIdMismatch {
                expected: self.chain_id.clone(),
                received: remote.chain_id.clone(),
            });
        }

        // Malformed before mismatched, so an operator can tell a broken peer
        // from a fork.
        if !Self::is_well_formed_genesis_hash(&remote.genesis_hash) {
            return Err(HandshakeError::MalformedGenesisHash {
                received: remote.genesis_hash.clone(),
            });
        }

        if remote.genesis_hash != self.genesis_hash {
            return Err(HandshakeError::GenesisMismatch {
                expected: self.genesis_hash.clone(),
                received: remote.genesis_hash.clone(),
            });
        }

        Ok(())
    }
}

/// Renders network magic for an error message.
///
/// The magic is chosen to be printable ASCII, but a hostile peer can send any
/// four bytes, and an error message is not the place to emit control
/// characters into an operator's terminal.
fn printable_magic(magic: &[u8; MAGIC_LEN]) -> String {
    if magic.iter().all(|b| b.is_ascii_graphic()) {
        // Safe: every byte was just checked to be printable ASCII.
        String::from_utf8_lossy(magic).into_owned()
    } else {
        format!("0x{}", hex::encode(magic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{NETWORK_MAGIC_DEVNET, NETWORK_MAGIC_MAINNET};

    const GENESIS: &str = "9348af00681eecefb8d6329d5ba101c13bc3c8943f2c610295026f6503654287";
    const OTHER_GENESIS: &str = "eed34034d774c8a2f0b1e5c6d7a8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6";

    fn mainnet() -> NetworkIdentity {
        NetworkIdentity::mainnet(GENESIS)
    }

    #[test]
    fn a_matching_peer_is_accepted() {
        let id = mainnet();
        let peer = Handshake::for_identity(&id);
        assert!(id.verify_handshake(&peer).is_ok());
    }

    #[test]
    fn the_same_chain_id_with_a_different_genesis_is_refused() {
        // The case CometBFT lets through. This is the whole reason the crate
        // exists, so it gets its own test with the reason written down.
        let id = mainnet();
        let mut fork = Handshake::for_identity(&id);
        fork.genesis_hash = OTHER_GENESIS.to_owned();

        match id.verify_handshake(&fork) {
            Err(HandshakeError::GenesisMismatch { expected, received }) => {
                assert_eq!(expected, GENESIS);
                assert_eq!(received, OTHER_GENESIS);
            }
            other => panic!("a same-chain-id fork was not refused on genesis: {other:?}"),
        }
    }

    #[test]
    fn a_different_network_is_refused_on_the_magic_first() {
        // Cheapest check first: four bytes, before any string comparison.
        let id = mainnet();
        let mut peer = Handshake::for_identity(&NetworkIdentity::devnet(GENESIS));
        peer.genesis_hash = GENESIS.to_owned();

        match id.verify_handshake(&peer) {
            Err(HandshakeError::MagicMismatch { expected, received }) => {
                assert_eq!(expected, "HGM1");
                assert_eq!(received, "HGD1");
            }
            other => panic!("a devnet peer was not refused on the magic: {other:?}"),
        }
    }

    #[test]
    fn a_different_protocol_version_is_refused() {
        let id = mainnet();
        let mut peer = Handshake::for_identity(&id);
        peer.protocol_major_version = 2;

        match id.verify_handshake(&peer) {
            Err(HandshakeError::ProtocolMismatch { expected, received }) => {
                assert_eq!(expected, 1);
                assert_eq!(received, 2);
            }
            other => panic!("a future protocol version was accepted: {other:?}"),
        }
    }

    #[test]
    fn a_different_network_id_is_refused() {
        let id = mainnet();
        let mut peer = Handshake::for_identity(&id);
        peer.network_id = "hashgram-somethingelse".to_owned();

        assert!(matches!(
            id.verify_handshake(&peer),
            Err(HandshakeError::NetworkIdMismatch { .. })
        ));
    }

    #[test]
    fn a_different_chain_id_is_refused() {
        let id = mainnet();
        let mut peer = Handshake::for_identity(&id);
        peer.chain_id = "hashgram-2".to_owned();

        assert!(matches!(
            id.verify_handshake(&peer),
            Err(HandshakeError::ChainIdMismatch { .. })
        ));
    }

    #[test]
    fn a_malformed_genesis_hash_is_distinguished_from_a_fork() {
        // An operator should be able to tell "this peer is broken" from
        // "this peer is on a fork", so these are separate errors.
        let id = mainnet();

        for bad in ["", "abc", "zzzz", &GENESIS.to_ascii_uppercase()] {
            let mut peer = Handshake::for_identity(&id);
            peer.genesis_hash = bad.to_owned();
            assert!(
                matches!(
                    id.verify_handshake(&peer),
                    Err(HandshakeError::MalformedGenesisHash { .. })
                ),
                "genesis hash {bad:?} was not reported as malformed"
            );
        }
    }

    #[test]
    fn an_unconfigured_node_refuses_to_verify_anyone() {
        // Accepting here would make a node with no pinned genesis join the
        // first network that spoke to it.
        let mut id = mainnet();
        id.genesis_hash = String::new();

        let peer = Handshake::for_identity(&mainnet());
        assert!(matches!(
            id.verify_handshake(&peer),
            Err(HandshakeError::NoLocalGenesis)
        ));
    }

    #[test]
    fn a_hostile_magic_does_not_reach_the_terminal_raw() {
        // A peer can send any four bytes. An error message is not the place
        // to emit control characters or a terminal escape sequence.
        let id = mainnet();
        let mut peer = Handshake::for_identity(&id);
        peer.network_magic = [0x1b, b'[', b'2', b'J'];

        let err = id
            .verify_handshake(&peer)
            .expect_err("a hostile magic was accepted");
        let text = err.to_string();
        assert!(
            text.contains("0x1b5b324a"),
            "hostile magic was not hex-escaped: {text}"
        );
        assert!(
            !text.contains('\x1b'),
            "an escape character reached the error message"
        );
    }

    #[test]
    fn every_rejection_names_the_field_that_disagreed() {
        // "Handshake failed" is useless to an operator. Each error must say
        // which part mismatched and, where relevant, both values.
        let id = mainnet();

        let cases: Vec<(&str, Handshake)> = vec![
            ("magic", {
                let mut h = Handshake::for_identity(&id);
                h.network_magic = NETWORK_MAGIC_DEVNET;
                h
            }),
            ("protocol", {
                let mut h = Handshake::for_identity(&id);
                h.protocol_major_version = 99;
                h
            }),
            ("network id", {
                let mut h = Handshake::for_identity(&id);
                h.network_id = "other".to_owned();
                h
            }),
            ("chain id", {
                let mut h = Handshake::for_identity(&id);
                h.chain_id = "other".to_owned();
                h
            }),
            ("genesis hash", {
                let mut h = Handshake::for_identity(&id);
                h.genesis_hash = OTHER_GENESIS.to_owned();
                h
            }),
        ];

        for (field, peer) in cases {
            let err = id
                .verify_handshake(&peer)
                .expect_err("a mismatched peer was accepted");
            let text = err.to_string();
            assert!(
                text.contains(field),
                "the error for a {field} mismatch does not mention it: {text}"
            );
        }

        // And confirm the accepted case still passes, so the test is not
        // vacuously rejecting everything.
        assert_eq!(id.network_magic, NETWORK_MAGIC_MAINNET);
        assert!(id.verify_handshake(&Handshake::for_identity(&id)).is_ok());
    }

    #[test]
    fn the_handshake_round_trips_through_json() {
        // The wire form has to survive serialisation, and a field renamed by
        // accident would silently become absent.
        let id = mainnet();
        let peer = Handshake::for_identity(&id);

        let json = serde_json::to_string(&peer).expect("serialising the handshake");
        let back: Handshake = serde_json::from_str(&json).expect("deserialising the handshake");

        assert_eq!(peer, back);
        assert!(id.verify_handshake(&back).is_ok());
    }
}
