//! The Hashgram client SDK.
//!
//! Everything a native application needs to be a Hashgram client, in one
//! crate with no UI: identity and keys, the wallet, end-to-end encrypted
//! messaging, public social events, media, and the network link to reach
//! nodes. Swift, Kotlin, TypeScript and C# bindings are expected to wrap
//! this crate (or reimplement it against the same protobuf definitions and
//! signing vectors) rather than reimplement the cryptography.
//!
//! # Trust model, in one paragraph
//!
//! The SDK trusts the chain for who owns what (addresses, devices, names)
//! and for balances. It trusts nobody for content: blobs are hash-verified,
//! social events are signature-verified against on-chain devices, messages
//! are authenticated by MLS. Nodes are interchangeable providers of
//! availability; the SDK talks to several and drops any that serves bad
//! bytes. Nothing here can be told "trust this node" by a node.
//!
//! # Layers
//!
//! **Hashgram One application layer** (what a client program calls):
//!
//! | Module | Does |
//! | --- | --- |
//! | [`app`] | [`app::HashgramOne`] facade: opens the account, connects, exposes the APIs below |
//! | [`mail`] | HashMail: send/receive/thread/file E2EE mail |
//! | [`drive`] | HashDrive: encrypted files, folders, versions, sharing |
//! | [`people`] | Lookup, contact requests, friends, blocks, profiles |
//! | [`feed`] | Public posts/comments/reactions, chronological feeds |
//! | [`circles`] | Private groups over MLS |
//! | [`spaces`] | Shared environments with roles (signed event log) |
//! | [`devices`] | Multi-device reconciliation and bootstrap |
//! | [`sync`] | The sync engine state machine |
//! | [`wallet`] | Balances, send, stake, usernames |
//! | [`provider`] | Earn: provider lifecycle and earnings |
//! | [`network`] | Peers, validators, supply, indexer read model |
//! | [`store`] | Encrypted local store |
//! | [`backup`] | Encrypted vault backup export/import |
//! | [`storage_lease`] | Paid storage leases (ADR option B) |
//! | [`ai`] | Hash AI interfaces (no implementation transmits anything) |
//!
//! **Protocol layer** (what the application layer is built on):
//!
//! | Module | Does |
//! | --- | --- |
//! | [`account`] | Encrypted vault; wallet, root and device keys; on-chain identity |
//! | [`link`] | libp2p client swarm with the Hashgram handshake |
//! | [`messaging`] | MLS groups over store-and-forward mailboxes |
//! | [`social`] | Signed social events |
//! | [`blob`] | Upload, download, verify, private encryption |
//! | [`chain_relay`] | Chain reads/broadcast through the P2P relay, cross-checked |

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

pub mod account;
pub mod ai;
pub mod app;
pub mod backup;
pub mod blob;
pub mod calls;
pub mod chain_relay;
pub mod circles;
pub mod devices;
pub mod drive;
pub mod feed;
pub mod link;
pub mod mail;
pub mod messaging;
pub mod network;
pub mod people;
pub mod provider;
pub mod social;
pub mod spaces;
pub mod storage_lease;
pub mod store;
pub mod sync;
pub mod wallet;

pub use app::{Config, HashgramOne, Paths};
pub use hashgram_app::{self as protocol, AppError};

pub use hashgram_chain::{
    self as chain, ChainTransport, Client as ChainClient, Verification, Wallet,
};

/// A chain client that reads and broadcasts through the P2P relay of the
/// nodes `link` is connected to, cross-checking every read across two
/// operators (see [`chain_relay`]). No HTTP endpoint is involved.
#[must_use]
pub fn chain_client_over_link(link: std::sync::Arc<link::Link>, chain_id: &str) -> ChainClient {
    ChainClient::over(
        std::sync::Arc::new(chain_relay::P2pChainTransport::new(link)),
        chain_id,
    )
}
pub use hashgram_identity::{self as identity, vault::KdfCost, Vault, VaultContents};
pub use hashgram_mls::{self as mls, GroupMeta};
pub use hashgram_net::{self as net, NetworkIdentity};
pub use hashgram_p2p::{Multiaddr, PeerId};
pub use hashgram_proto::{self as proto, chat, pb};

/// Everything that can go wrong.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// Vault.
    #[error(transparent)]
    Vault(#[from] hashgram_identity::VaultError),
    /// Wallet.
    #[error(transparent)]
    Wallet(#[from] hashgram_chain::WalletError),
    /// Chain.
    #[error(transparent)]
    Chain(#[from] hashgram_chain::ClientError),
    /// Link.
    #[error(transparent)]
    Link(#[from] link::LinkError),
    /// MLS.
    #[error(transparent)]
    Mls(#[from] hashgram_mls::MlsError),
    /// Signing.
    #[error(transparent)]
    Sign(#[from] hashgram_proto::SignError),
    /// Canonical.
    #[error(transparent)]
    Canonical(#[from] hashgram_net::CanonicalError),
    /// Key.
    #[error(transparent)]
    Key(#[from] hashgram_proto::KeyError),
    /// This device does not hold the account key.
    #[error("this device does not hold the account (wallet) key")]
    NoWalletKey,
    /// This device does not hold the root key.
    #[error("this device does not hold the identity root key")]
    NoRootKey,
    /// The vault has no device key.
    #[error("the vault has no device key")]
    NoDeviceKey,
    /// No key package could be found for a device.
    #[error(
        "no key package found for device {0}; it has not published one to any reachable store node"
    )]
    NoKeyPackage(String),
    /// Nobody to send to.
    #[error("no recipient devices found")]
    NoRecipients,
    /// Delivery.
    #[error("delivery failed: {0}")]
    Delivery(String),
    /// Something was refused as invalid.
    #[error("invalid: {0}")]
    Invalid(String),
    /// Content not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// A peer served corrupt data.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// The local store.
    #[error("local store: {0}")]
    Store(String),
    /// A message or object from a newer protocol generation.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl link::Link {
    /// Sends a request to any verified peer that will answer it: store
    /// peers first (they keep events), then anyone.
    pub async fn request_role_any(
        &self,
        body: pb::request::Body,
    ) -> Result<(PeerId, pb::response::Body), link::LinkError> {
        if let Ok(r) = self.request_role("store", body.clone()).await {
            return Ok(r);
        }
        if let Ok(r) = self.request_role("indexer", body.clone()).await {
            return Ok(r);
        }
        let peers = self.peers().await;
        let mut last = link::LinkError::NoPeer("any");
        for p in peers {
            match self.request(p.peer, body.clone()).await {
                Ok(b) => return Ok((p.peer, b)),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}
