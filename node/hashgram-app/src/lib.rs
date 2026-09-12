//! Hashgram One application protocol.
//!
//! Everything a Hashgram One client needs to *understand* Mail, Drive,
//! People, Circles and Spaces, without any network or storage dependency:
//! the versioned message models, their validation bounds, the cryptography
//! that turns a file into an encrypted Drive object and back, the
//! deterministic merge of Drive manifests written by several devices, the
//! capability model for sharing, the signed hash-chained Space event log and
//! its role state machine, and the local spam policy for unknown senders.
//!
//! The wire types are generated from `proto/hashgram/app/v1/app.proto` into
//! [`hashgram_proto::app`]; this crate wraps them with the rules. The SDK
//! (`hashgram-sdk`) adds transport: it puts these messages into MLS groups
//! and moves the encrypted bytes through store nodes. Nothing here knows
//! about peers, and so nothing here can leak to one.
//!
//! # Versioning rule
//!
//! Every reader goes through [`version::check`]. An `AppMessage` whose
//! envelope version is unknown, whose `min_reader_version` exceeds ours, or
//! whose body arm did not decode, is reported as [`AppError::Unsupported`]
//! and the caller keeps the ciphertext so a later build can render it.
//! Partial rendering of a message a sender marked as needing a newer reader
//! is never attempted.

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

pub mod circle;
pub mod drive;
pub mod envelope;
pub mod ids;
pub mod mail;
pub mod people;
pub mod signing;
pub mod space;
pub mod spam;
pub mod version;

pub use hashgram_proto::app as pb;

/// Everything that can go wrong at the application-protocol layer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The message is from a newer (or unknown) protocol generation.
    #[error("unsupported application message: {0}")]
    Unsupported(String),
    /// A bound was exceeded or a required field is missing/malformed.
    #[error("invalid: {0}")]
    Invalid(String),
    /// An encrypted object failed to authenticate (wrong key or tampered).
    #[error("integrity failure: {0}")]
    Integrity(String),
    /// A signature did not verify or the signer is not allowed to act.
    #[error("unauthorised: {0}")]
    Unauthorised(String),
    /// A referenced object does not exist in the local state.
    #[error("not found: {0}")]
    NotFound(String),
    /// The operating system gave no randomness.
    #[error("no randomness available")]
    NoRandomness,
    /// Canonical encoding refused a field.
    #[error(transparent)]
    Canonical(#[from] hashgram_net::CanonicalError),
    /// Protobuf decoding failed.
    #[error("decode: {0}")]
    Decode(String),
}

impl From<prost::DecodeError> for AppError {
    fn from(e: prost::DecodeError) -> Self {
        Self::Decode(e.to_string())
    }
}
