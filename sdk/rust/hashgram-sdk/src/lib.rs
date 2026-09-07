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
//! | Module | Does |
//! | --- | --- |
//! | [`account`] | Encrypted vault; wallet, root and device keys; on-chain identity |
//! | [`link`] | libp2p client swarm with the Hashgram handshake |
//! | [`messaging`] | MLS groups over store-and-forward mailboxes |
//! | [`social`] | Signed social events |
//! | [`blob`] | Upload, download, verify, private encryption |

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
pub mod blob;
pub mod link;
pub mod messaging;
pub mod social;

pub use hashgram_chain::{self as chain, Client as ChainClient, Wallet};
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
