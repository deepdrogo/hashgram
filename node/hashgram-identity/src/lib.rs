//! Client identity: an encrypted keystore holding the keys a Hashgram user's
//! device needs, and the identity model on top of it.
//!
//! # Model
//!
//! ```text
//! Account (secp256k1)   the hash1… address; pays fees, owns the identity on chain
//!   └─ Root key (ed25519)   signs device certificates; may live offline
//!        ├─ Device #1 (ed25519)   signs events and messages from this device
//!        ├─ Device #2 …
//!        └─ Recovery config     guardians and delay, on chain
//! ```
//!
//! A device holds its own device key and, optionally, the account and root
//! keys. A phone that only holds its device key can post and message but
//! cannot add another device; the laptop with the root key can. Losing a
//! device costs that device's key and nothing more, because the root revokes
//! it on chain.
//!
//! # Keystore
//!
//! One file, encrypted with XChaCha20-Poly1305 under a key derived from the
//! passphrase with Argon2id. Nothing is ever written in the clear, and the
//! file is `0600`. This is the client-side counterpart of the rule that no
//! server holds a user's private key: the keys exist on the user's device,
//! encrypted at rest, and nowhere else.

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

pub mod vault;

pub use vault::{Vault, VaultContents, VaultError};
