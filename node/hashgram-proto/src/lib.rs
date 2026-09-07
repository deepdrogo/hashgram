//! Hashgram off-chain wire types.
//!
//! Generated from `proto/hashgram/p2p/v1`, plus the three things protobuf
//! does not give you and a network protocol needs:
//!
//! - **Frame bounds** ([`limits`]): every message family has a maximum
//!   encoded size, checked before decoding, so an oversized frame is refused
//!   at the length prefix rather than allocated.
//! - **Canonical signing** ([`signing`]): the exact bytes each signed object
//!   commits to, built with `hashgram-net`'s canonical encoder in a fixed
//!   field order, under a domain-separated purpose. Protobuf encoding is not
//!   canonical and is never what gets signed.
//! - **Content identifiers** ([`blob`]): BLAKE3 over the canonical manifest,
//!   so the same content always has the same CID regardless of who encoded
//!   the manifest.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
    )
)]

/// The generated protobuf types.
#[allow(
    missing_docs,
    unreachable_pub,
    clippy::all,
    clippy::pedantic,
    clippy::nursery
)]
pub mod pb {
    include!(concat!(env!("OUT_DIR"), "/hashgram.p2p.v1.rs"));
}

pub mod blob;
pub mod dht;
pub mod frame;
pub mod keys;
pub mod limits;
pub mod signing;
pub mod validate;

pub use frame::{decode_frame, encode_frame, FrameError};
pub use keys::{Ed25519Signer, KeyError};
pub use signing::{SignError, VerifyError};
