//! Hashgram chain client.
//!
//! What a client needs from the chain, and nothing a node should never do:
//!
//! - [`Wallet`]: a secp256k1 account key from a BIP-39 mnemonic on the
//!   Cosmos path `m/44'/118'/0'/0/n`, producing `hash1…` addresses. Keys are
//!   generated and used here; they are stored by the caller's keystore.
//! - [`Client`]: queries over the REST gateway, and transaction building,
//!   signing (SIGN_MODE_DIRECT) and broadcasting.
//! - [`msgs`]: constructors for every Hashgram message a client sends, as
//!   protobuf `Any`.
//!
//! There is no code path that generates a Founder key, mints, or touches a
//! consensus key. The chain client is a wallet and a query surface.

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

/// Generated chain module types.
#[allow(
    missing_docs,
    unreachable_pub,
    clippy::all,
    clippy::pedantic,
    clippy::nursery
)]
pub mod pb {
    /// `cosmos.base.v1beta1`, for `Coin` fields in Hashgram messages.
    pub mod cosmos {
        pub mod base {
            pub mod v1beta1 {
                include!(concat!(env!("OUT_DIR"), "/cosmos.base.v1beta1.rs"));
            }
        }
    }
    pub mod hashgram {
        pub mod identity {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/hashgram.identity.v1.rs"));
            }
        }
        pub mod username {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/hashgram.username.v1.rs"));
            }
        }
        pub mod welcome {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/hashgram.welcome.v1.rs"));
            }
        }
        pub mod serviceproof {
            pub mod v1 {
                include!(concat!(env!("OUT_DIR"), "/hashgram.serviceproof.v1.rs"));
            }
        }
    }
    pub use hashgram::identity::v1 as identity;
    pub use hashgram::serviceproof::v1 as serviceproof;
    pub use hashgram::username::v1 as username;
    pub use hashgram::welcome::v1 as welcome;
}

pub mod client;
pub mod identity;
pub mod msgs;
pub mod wallet;

pub use client::{Client, ClientError, TxResult};
pub use wallet::{Wallet, WalletError, BECH32_PREFIX, DENOM};

/// Re-exported for callers that build their own `Any`.
pub use cosmrs::Any;
