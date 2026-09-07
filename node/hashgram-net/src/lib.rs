//! Hashgram network identity, signing domain separation and the peer
//! handshake.
//!
//! This crate is the Rust half of a contract with the Go implementation in
//! `app/params`. Both sides must produce **byte-identical** signing preimages,
//! because a signature is only verifiable if the verifier builds the same
//! preimage the signer built. The tests check against vectors generated from
//! the Go code (`tools/signing-vectors`), not against a second reading of the
//! specification: a specification can be misread twice in the same way, and a
//! generated vector cannot.
//!
//! # Why this crate exists separately from the P2P layer
//!
//! Network identity is a protocol fact. It is used by the P2P handshake, by
//! social event signing, by device certificates and by every client SDK. Those
//! have wildly different dependency needs — a mobile SDK does not want libp2p
//! — so identity lives in a crate with almost no dependencies.
//!
//! # The gap this closes
//!
//! CometBFT's peer handshake compares chain ids. It does **not** hash the
//! genesis file, which was verified by running a fork that kept the real chain
//! id and changed only the genesis: the transport connection opened. Consensus
//! still refused it, but the connection happened.
//!
//! [`Handshake`] closes that gap at the Hashgram P2P layer by verifying all
//! five identity parts, including the genesis hash.

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

mod handshake;
mod identity;
mod purpose;

pub use handshake::{Handshake, HandshakeError, HandshakeResult};
pub use identity::{
    compute_genesis_hash, NetworkIdentity, GENESIS_HASH_LEN, MAGIC_LEN, NETWORK_MAGIC_DEVNET,
    NETWORK_MAGIC_MAINNET, PROTOCOL_MAJOR_VERSION,
};
pub use purpose::{ParsePurposeError, SigningPurpose, ALL_PURPOSES};
