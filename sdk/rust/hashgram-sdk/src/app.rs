//! `HashgramOne`: the application facade a client program holds.
//!
//! One value owns the opened account, the P2P link, the chain client, the
//! MLS transport and the local store, and exposes the applications as
//! borrowing views:
//!
//! ```text
//! let mut one = HashgramOne::open(config, passphrase).await?;
//! one.mail().send(draft).await?;
//! one.drive().upload(&[], "report.pdf", "application/pdf", &bytes).await?;
//! one.people().request(&address, "hi, it's me").await?;
//! one.sync().round().await?;
//! one.save()?;
//! ```
//!
//! Every mutation that touches MLS or the vault is followed by
//! [`HashgramOne::save`] by the caller (a desktop app does it at the end of
//! each command). The facade never blocks on the network at open: the link
//! connects in the background and applications return `NoPeer` errors until
//! a peer is verified.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hashgram_app::envelope;
use hashgram_app::pb as app;
use hashgram_net::NetworkIdentity;
use hashgram_proto::chat;

use crate::account::Account;
use crate::link::Link;
use crate::messaging::{Messaging, Received};
use crate::store::LocalStore;
use crate::{ChainClient, Multiaddr, SdkError};
use hashgram_identity::vault::KdfCost;

/// Where a client keeps its files.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Directory holding everything below.
    pub home: PathBuf,
}

impl Paths {
    /// Standard layout under `home`.
    #[must_use]
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }
    /// The encrypted vault.
    #[must_use]
    pub fn vault(&self) -> PathBuf {
        self.home.join("keystore.json")
    }
    /// The local store.
    #[must_use]
    pub fn store(&self) -> PathBuf {
        self.home.join("local.redb")
    }
    /// The client peerstore.
    #[must_use]
    pub fn peerstore(&self) -> PathBuf {
        self.home.join("peerstore.json")
    }
    /// Downloaded / cached Drive objects.
    #[must_use]
    pub fn cache(&self) -> PathBuf {
        self.home.join("cache")
    }
}

/// How to reach the network.
#[derive(Debug, Clone)]
pub struct Config {
    /// File layout.
    pub paths: Paths,
    /// Network pin.
    pub network: NetworkIdentity,
    /// Bootstrap peers; empty means the compiled-in Mainnet list.
    pub bootstrap: Vec<Multiaddr>,
    /// Optional REST gateway; `None` reads the chain through the P2P relay.
    pub chain_api: Option<String>,
    /// KDF cost for the vault.
    pub kdf: KdfCost,
    /// How long `open` waits for the first verified peer before returning
    /// (applications work offline meanwhile).
    pub connect_wait: Duration,
}

/// Group tags kept in the local store so a Circle is never reused as a
/// mail channel and vice versa.
pub mod group_kind {
    /// Namespace.
    pub const NS: &str = "group_kind";
    /// A conversation (chat or mail) keyed by participant set.
    pub const CONVERSATION: &str = "conversation";
    /// The account's own devices only.
    pub const SELF: &str = "self";
    /// A Circle.
    pub const CIRCLE: &str = "circle";
    /// A Space.
    pub const SPACE: &str = "space";
}

/// The facade.
pub struct HashgramOne {
    /// Opened account.
    pub account: Account,
    /// Network pin.
    pub network: NetworkIdentity,
    /// The P2P link.
    pub link: Arc<Link>,
    /// Chain client (relay or REST).
    pub chain: ChainClient,
    /// MLS transport.
    pub messaging: Messaging,
    /// Local encrypted store.
    pub store: LocalStore,
    /// Paths.
    pub paths: Paths,
    pub(crate) mail_state: crate::mail::MailState,
    pub(crate) drive_state: crate::drive::DriveState,
    pub(crate) people_state: crate::people::PeopleState,
    pub(crate) spaces_state: crate::spaces::SpacesState,
    pub(crate) circles_state: crate::circles::CirclesState,
    pub(crate) sync_state: crate::sync::SyncState,
}

impl HashgramOne {
    /// Opens an existing account and connects.
    pub async fn open(config: Config, passphrase: &str) -> Result<Self, SdkError> {
        let account = Account::open(&config.paths.vault(), passphrase, config.kdf)?;
        Self::with_account(config, account).await
    }

    /// Builds the facade around an already-opened account (used by
    /// onboarding after `Account::create`).
    pub async fn with_account(config: Config, account: Account) -> Result<Self, SdkError> {
        let bootstrap: Vec<Multiaddr> =
            if config.bootstrap.is_empty() && config.network.is_mainnet() {
                hashgram_net::mainnet_bootstrap_peers()
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect()
            } else {
                config.bootstrap.clone()
            };
        let link = Arc::new(
            Link::connect(
                &config.network,
                &bootstrap,
                Some(&config.paths.peerstore()),
                config.connect_wait,
            )
            .await?,
        );
        let chain = match &config.chain_api {
            Some(url) if !url.is_empty() => ChainClient::new(url, &config.network.chain_id)?,
            _ => crate::chain_client_over_link(link.clone(), &config.network.chain_id),
        };
        let messaging = Messaging::open(&account)?;
        let store = LocalStore::open(&config.paths.store(), &account.device_seed()?)?;
        let mut one = Self {
            mail_state: crate::mail::MailState::load(&store)?,
            drive_state: crate::drive::DriveState::load(&account, &store)?,
            people_state: crate::people::PeopleState::load(&store)?,
            spaces_state: crate::spaces::SpacesState::load(&store)?,
            circles_state: crate::circles::CirclesState::load(&store)?,
            sync_state: crate::sync::SyncState::default(),
            account,
            network: config.network,
            link,
            chain,
            messaging,
            store,
            paths: config.paths,
        };
        one.people_state.ensure_self(one.account.address());
        Ok(one)
    }

    /// The account address.
    #[must_use]
    pub fn address(&self) -> &str {
        self.account.address()
    }

    /// Persists MLS state and every module's vault-resident state, then
    /// writes the vault.
    pub fn save(&mut self) -> Result<(), SdkError> {
        self.messaging.persist(&mut self.account)?;
        self.drive_state.persist(&mut self.account)?;
        self.account.save()
    }

    /// Mail.
    pub fn mail(&mut self) -> crate::mail::Mail<'_> {
        crate::mail::Mail { one: self }
    }
    /// Drive.
    pub fn drive(&mut self) -> crate::drive::Drive<'_> {
        crate::drive::Drive { one: self }
    }
    /// People.
    pub fn people(&mut self) -> crate::people::People<'_> {
        crate::people::People { one: self }
    }
    /// Feed.
    pub fn feed(&mut self) -> crate::feed::Feed<'_> {
        crate::feed::Feed { one: self }
    }
    /// Circles.
    pub fn circles(&mut self) -> crate::circles::Circles<'_> {
        crate::circles::Circles { one: self }
    }
    /// Spaces.
    pub fn spaces(&mut self) -> crate::spaces::Spaces<'_> {
        crate::spaces::Spaces { one: self }
    }
    /// Devices.
    pub fn devices(&mut self) -> crate::devices::Devices<'_> {
        crate::devices::Devices { one: self }
    }
    /// Sync engine.
    pub fn sync(&mut self) -> crate::sync::Sync<'_> {
        crate::sync::Sync { one: self }
    }
    /// Wallet.
    pub fn wallet(&mut self) -> crate::wallet::WalletApi<'_> {
        crate::wallet::WalletApi { one: self }
    }
    /// Provider / Earn.
    pub fn provider(&mut self) -> crate::provider::Provider<'_> {
        crate::provider::Provider { one: self }
    }
    /// Network.
    pub fn network_api(&mut self) -> crate::network::Network<'_> {
        crate::network::Network { one: self }
    }
    /// Paid storage leases.
    pub fn leases(&mut self) -> crate::storage_lease::Lease<'_> {
        crate::storage_lease::Lease { one: self }
    }

    // -----------------------------------------------------------------------
    // Group plumbing shared by the applications
    // -----------------------------------------------------------------------

    /// Tag of a group.
    pub(crate) fn group_kind(&self, gid_hex: &str) -> Option<String> {
        self.store
            .get(group_kind::NS, gid_hex.as_bytes())
            .ok()
            .flatten()
    }

    pub(crate) fn set_group_kind(&self, gid_hex: &str, kind: &str) -> Result<(), SdkError> {
        self.store
            .put(group_kind::NS, gid_hex.as_bytes(), &kind.to_owned())
    }

    /// Finds the conversation group whose member address set is exactly
    /// `participants ∪ {me}`, or creates it. Circles and Spaces are never
    /// returned. Returns the group id bytes.
    pub(crate) async fn conversation_group(
        &mut self,
        participants: &[String],
    ) -> Result<Vec<u8>, SdkError> {
        let mut want: Vec<String> = participants.to_vec();
        want.push(self.account.address().to_owned());
        want.sort();
        want.dedup();
        for (gid_hex, _meta, mut addrs) in self.messaging.conversations() {
            addrs.sort();
            addrs.dedup();
            if addrs != want {
                continue;
            }
            match self.group_kind(&gid_hex).as_deref() {
                Some(group_kind::CIRCLE) | Some(group_kind::SPACE) => continue,
                _ => {}
            }
            return hex::decode(&gid_hex).map_err(|e| SdkError::Invalid(e.to_string()));
        }
        let others: Vec<String> = want
            .iter()
            .filter(|a| a.as_str() != self.account.address())
            .cloned()
            .collect();
        let gid = self
            .messaging
            .create_conversation(&self.link, &self.chain, &self.network, "", &others)
            .await?;
        let kind = if others.is_empty() {
            group_kind::SELF
        } else {
            group_kind::CONVERSATION
        };
        self.set_group_kind(&hex::encode(&gid), kind)?;
        Ok(gid)
    }

    /// The account's self group (all of the user's devices), if the account
    /// has more than one device with a key package published. `None` on a
    /// single-device account: there is nobody to sync with.
    pub(crate) async fn self_group(&mut self) -> Result<Option<Vec<u8>>, SdkError> {
        match self.conversation_group(&[]).await {
            Ok(g) => Ok(Some(g)),
            Err(SdkError::NoRecipients) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Sends an application message to a group.
    pub(crate) async fn send_app(
        &mut self,
        group_id: &[u8],
        body: app::app_message::Body,
    ) -> Result<Vec<u8>, SdkError> {
        let msg = envelope::wrap(body)?;
        let id = msg.id.clone();
        let chat_msg = envelope::to_chat(msg);
        self.messaging
            .send(&self.link, &self.network, group_id, chat_msg)
            .await?;
        Ok(id)
    }

    /// Whether a received chat message is an application message.
    #[must_use]
    pub fn app_of(r: &Received) -> Option<&app::AppMessage> {
        if r.raw.kind == chat::ChatKind::App as i32 {
            r.raw.app.as_ref()
        } else {
            None
        }
    }
}
