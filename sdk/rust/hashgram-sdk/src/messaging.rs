//! Private messaging: MLS groups delivered through store-and-forward
//! mailboxes.
//!
//! # Flow
//!
//! ```text
//! sender device                          store nodes                 recipient device
//!   MLS encrypt ──▶ Envelope{mailbox=H(dev key), ciphertext}
//!                 ──▶ MailboxPut to providers of that mailbox ──▶ held, encrypted
//!                                                              ◀── MailboxFetch (signed)
//!                                                              ──▶ envelopes
//!                                                                    MLS process ──▶ plaintext
//!                                                              ◀── MailboxAck
//! ```
//!
//! A conversation with N participants who own M devices in total is one MLS
//! group with M members. Sending a message produces one MLS ciphertext and
//! M−1 envelopes, one per other device, each addressed by that device's
//! mailbox. Store nodes see mailbox ids and sizes.
//!
//! # Finding devices and key packages
//!
//! Devices come from the chain (`x/identity`), which is the only place a
//! device's authority is recorded. Key packages come from the store nodes
//! that advertise the device's mailbox in the DHT; the device put them there.

use std::collections::HashMap;

use hashgram_mls::{GroupMeta, Inbound, MlsClient};
use hashgram_net::NetworkIdentity;
use hashgram_p2p::PeerId;
use hashgram_proto::chat;
use hashgram_proto::limits::WIRE_VERSION;
use hashgram_proto::pb;
use hashgram_proto::{dht, signing, Ed25519Signer};
use prost::Message;
use tracing::{debug, warn};

use crate::account::{devices_on_chain, Account};
use crate::link::Link;
use crate::SdkError;

/// Key under which the MLS snapshot is kept in the vault's `extra` map.
pub const VAULT_MLS_KEY: &str = "mls_state";
/// Key for the mailbox cursor per store peer.
pub const VAULT_CURSOR_KEY: &str = "mailbox_cursors";
/// Key for recently processed envelope ids.
pub const VAULT_SEEN_KEY: &str = "seen_envelopes";
/// How many processed envelope ids to remember. An envelope stored on
/// several store nodes arrives several times; MLS refuses the second copy
/// (secret reuse), so the copies are recognised here first.
const SEEN_CAP: usize = 5000;

/// How long a key package is advertised for.
pub const KEY_PACKAGE_TTL_SECS: u64 = 30 * 24 * 3600;
/// Default envelope lifetime requested.
pub const ENVELOPE_TTL_SECS: u64 = 14 * 24 * 3600;
/// One-time key packages to keep queued at each store node.
pub const KEY_PACKAGE_BATCH: u32 = 8;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A decrypted inbound chat message.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Received {
    /// Group id, hex.
    pub group_id: String,
    /// Sender address.
    pub sender: String,
    /// Sender device key, hex.
    pub sender_device: String,
    /// The message.
    pub message: ChatView,
}

/// A chat message for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatView {
    /// Kind name.
    pub kind: String,
    /// Message id, hex.
    pub id: String,
    /// Sender clock, ms.
    pub timestamp_ms: u64,
    /// Text.
    pub text: String,
    /// Target id (edit/delete/reaction/receipt), hex.
    pub target: String,
    /// Reaction.
    pub reaction: String,
    /// Attachments (cid hex, mime, size).
    pub attachments: Vec<(String, String, u64)>,
    /// Group name for GROUP_INFO.
    pub group_name: String,
}

impl From<chat::ChatMessage> for ChatView {
    fn from(m: chat::ChatMessage) -> Self {
        Self {
            kind: chat::ChatKind::try_from(m.kind)
                .map(|k| k.as_str_name().trim_start_matches("CHAT_KIND_").to_owned())
                .unwrap_or_else(|_| "UNKNOWN".into()),
            id: hex::encode(&m.id),
            timestamp_ms: m.timestamp_ms,
            text: m.text,
            target: hex::encode(&m.target),
            reaction: m.reaction,
            attachments: m
                .attachments
                .iter()
                .map(|a| (hex::encode(&a.cid), a.mime.clone(), a.size))
                .collect(),
            group_name: m.group_info.map(|g| g.name).unwrap_or_default(),
        }
    }
}

/// Messaging state for one device.
pub struct Messaging {
    mls: MlsClient,
    device: Ed25519Signer,
    address: String,
    /// Store peers this device fetches from.
    cursors: HashMap<String, Vec<u8>>,
    /// Recently processed envelope ids, hex, oldest first.
    seen: Vec<String>,
}

impl Messaging {
    /// Loads MLS state from the vault, or starts fresh.
    pub fn open(account: &Account) -> Result<Self, SdkError> {
        let seed = account.device_seed()?;
        let device = account.device()?;
        let mls = match account.contents.extra.get(VAULT_MLS_KEY) {
            Some(hex_snap) => {
                let snap = hex::decode(hex_snap).map_err(|e| {
                    SdkError::Vault(hashgram_identity::VaultError::Format {
                        path: account.vault.path().to_path_buf(),
                        reason: e.to_string(),
                    })
                })?;
                MlsClient::restore(account.address(), &seed, &snap)?
            }
            None => MlsClient::new(account.address(), &seed)?,
        };
        let cursors = account
            .contents
            .extra
            .get(VAULT_CURSOR_KEY)
            .and_then(|h| hex::decode(h).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let seen = account
            .contents
            .extra
            .get(VAULT_SEEN_KEY)
            .and_then(|h| hex::decode(h).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Self {
            mls,
            device,
            address: account.address().to_owned(),
            cursors,
            seen,
        })
    }

    /// Writes MLS state back into the account (caller saves the vault).
    pub fn persist(&self, account: &mut Account) -> Result<(), SdkError> {
        let snap = self.mls.snapshot()?;
        account
            .contents
            .extra
            .insert(VAULT_MLS_KEY.into(), hex::encode(snap));
        let cursors = serde_json::to_vec(&self.cursors).unwrap_or_default();
        account
            .contents
            .extra
            .insert(VAULT_CURSOR_KEY.into(), hex::encode(cursors));
        let seen = serde_json::to_vec(&self.seen).unwrap_or_default();
        account
            .contents
            .extra
            .insert(VAULT_SEEN_KEY.into(), hex::encode(seen));
        Ok(())
    }

    /// The MLS client.
    #[must_use]
    pub fn mls(&self) -> &MlsClient {
        &self.mls
    }

    /// Publishes key packages to every store peer, and so registers this
    /// device's mailbox with them: a last-resort package plus enough
    /// one-time packages to bring each store's queue to
    /// [`KEY_PACKAGE_BATCH`]. Returns how many store nodes accepted.
    /// Called on first run and by [`Self::sync`] to replenish.
    pub async fn publish_key_package(
        &self,
        link: &Link,
        network: &NetworkIdentity,
    ) -> Result<usize, SdkError> {
        let stores = link.peers_with_role("store").await;
        if stores.is_empty() {
            return Err(SdkError::Link(crate::link::LinkError::NoPeer("store")));
        }
        let t = now();
        let mut ok = 0;
        for p in stores {
            // Last resort first: it also tells us the current one-time depth.
            let mut lr = pb::KeyPackagePublish {
                key_package: self.mls.key_package_last_resort()?,
                created_at: t,
                expires_at: t + KEY_PACKAGE_TTL_SECS,
                last_resort: true,
                ..Default::default()
            };
            signing::sign_key_package(network, &self.device, &mut lr)?;
            let mut remaining = match link
                .request(p, pb::request::Body::KeyPackagePublish(lr))
                .await
            {
                Ok(pb::response::Body::KeyPackagePublish(r)) if r.stored => r.remaining,
                Ok(_) => continue,
                Err(e) => {
                    debug!(%p, error = %e, "key package publish failed");
                    continue;
                }
            };
            while remaining < KEY_PACKAGE_BATCH {
                let mut msg = pb::KeyPackagePublish {
                    key_package: self.mls.key_package()?,
                    created_at: t,
                    expires_at: t + KEY_PACKAGE_TTL_SECS,
                    ..Default::default()
                };
                signing::sign_key_package(network, &self.device, &mut msg)?;
                match link
                    .request(p, pb::request::Body::KeyPackagePublish(msg))
                    .await
                {
                    Ok(pb::response::Body::KeyPackagePublish(r))
                        if r.stored && r.remaining > remaining =>
                    {
                        remaining = r.remaining
                    }
                    _ => break,
                }
            }
            ok += 1;
        }
        Ok(ok)
    }

    /// Fetches a key package for a device key: from DHT providers of its
    /// mailbox first, then from any connected store peer.
    async fn fetch_key_package(
        &self,
        link: &Link,
        device_pubkey: &[u8],
    ) -> Result<Vec<u8>, SdkError> {
        let mut candidates: Vec<PeerId> =
            link.providers(dht::mailbox_for_device(device_pubkey)).await;
        for p in link.peers_with_role("store").await {
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
        for p in candidates {
            if let Ok(pb::response::Body::KeyPackageFetch(r)) = link
                .request(
                    p,
                    pb::request::Body::KeyPackageFetch(pb::KeyPackageFetch {
                        device_pubkey: device_pubkey.to_vec(),
                    }),
                )
                .await
            {
                if r.found {
                    if let Some(k) = r.key_package {
                        return Ok(k.key_package);
                    }
                }
            }
        }
        Err(SdkError::NoKeyPackage(hex::encode(device_pubkey)))
    }

    /// Key packages for every active device of an address, from the chain
    /// and the store nodes. Skips this device.
    async fn key_packages_for(
        &self,
        link: &Link,
        chain: &hashgram_chain::Client,
        address: &str,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, SdkError> {
        let devices = devices_on_chain(chain, address).await?;
        let mut out = Vec::new();
        for d in devices.into_iter().filter(|d| !d.revoked) {
            let key = hex::decode(&d.device_pubkey).unwrap_or_default();
            if key == self.device.public_key() {
                continue;
            }
            match self.fetch_key_package(link, &key).await {
                Ok(kp) => out.push((key, kp)),
                Err(e) => {
                    warn!(device = d.device_id, error = %e, "no key package; device will not be in the group")
                }
            }
        }
        Ok(out)
    }

    /// Starts a conversation with `participants` (addresses). Every active
    /// device of every participant, plus this device, becomes a member.
    /// Returns the group id.
    pub async fn create_conversation(
        &mut self,
        link: &Link,
        chain: &hashgram_chain::Client,
        network: &NetworkIdentity,
        name: &str,
        participants: &[String],
    ) -> Result<Vec<u8>, SdkError> {
        let direct = participants.len() == 1 && name.is_empty();
        let gid = self.mls.create_group(GroupMeta {
            name: name.to_owned(),
            direct,
            joined_at: now(),
        })?;
        let mut kps = Vec::new();
        let mut targets: Vec<Vec<u8>> = Vec::new();
        for p in participants {
            for (key, kp) in self.key_packages_for(link, chain, p).await? {
                targets.push(key);
                kps.push(kp);
            }
        }
        // Also this account's other devices, so the user's own phone sees it.
        for (key, kp) in self.key_packages_for(link, chain, &self.address).await? {
            targets.push(key);
            kps.push(kp);
        }
        if kps.is_empty() {
            return Err(SdkError::NoRecipients);
        }
        let (_commit, welcome) = self.mls.add_members(&gid, &kps)?;
        for key in &targets {
            self.deliver(link, network, key, pb::EnvelopeKind::MlsWelcome, &welcome)
                .await?;
        }
        if !name.is_empty() {
            self.send(
                link,
                network,
                &gid,
                chat::ChatMessage {
                    version: WIRE_VERSION,
                    kind: chat::ChatKind::GroupInfo as i32,
                    id: random_id(),
                    timestamp_ms: now_ms(),
                    group_info: Some(chat::GroupInfo {
                        name: name.to_owned(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        }
        Ok(gid)
    }

    /// Adds a participant's devices to an existing group.
    pub async fn add_participant(
        &mut self,
        link: &Link,
        chain: &hashgram_chain::Client,
        network: &NetworkIdentity,
        group_id: &[u8],
        address: &str,
    ) -> Result<(), SdkError> {
        let mut kps = Vec::new();
        let mut targets = Vec::new();
        for (key, kp) in self.key_packages_for(link, chain, address).await? {
            targets.push(key);
            kps.push(kp);
        }
        if kps.is_empty() {
            return Err(SdkError::NoRecipients);
        }
        let existing = self.mls.recipient_devices(group_id)?;
        let (commit, welcome) = self.mls.add_members(group_id, &kps)?;
        for key in &existing {
            self.deliver(link, network, key, pb::EnvelopeKind::MlsMessage, &commit)
                .await?;
        }
        for key in &targets {
            self.deliver(link, network, key, pb::EnvelopeKind::MlsWelcome, &welcome)
                .await?;
        }
        Ok(())
    }

    /// Removes every device of an address from a group.
    pub async fn remove_participant(
        &mut self,
        link: &Link,
        network: &NetworkIdentity,
        group_id: &[u8],
        address: &str,
    ) -> Result<usize, SdkError> {
        let members = self.mls.members(group_id)?;
        let indices: Vec<u32> = members
            .iter()
            .filter(|m| m.address() == address)
            .map(|m| m.index)
            .collect();
        if indices.is_empty() {
            return Ok(0);
        }
        let commit = self.mls.remove_members(group_id, &indices)?;
        for key in self.mls.recipient_devices(group_id)? {
            self.deliver(link, network, &key, pb::EnvelopeKind::MlsMessage, &commit)
                .await?;
        }
        Ok(indices.len())
    }

    /// Sends a text message.
    pub async fn send_text(
        &mut self,
        link: &Link,
        network: &NetworkIdentity,
        group_id: &[u8],
        text: &str,
    ) -> Result<Vec<u8>, SdkError> {
        let id = random_id();
        self.send(
            link,
            network,
            group_id,
            chat::ChatMessage {
                version: WIRE_VERSION,
                kind: chat::ChatKind::Text as i32,
                id: id.clone(),
                timestamp_ms: now_ms(),
                text: text.to_owned(),
                ..Default::default()
            },
        )
        .await?;
        Ok(id)
    }

    /// Sends any chat message.
    pub async fn send(
        &mut self,
        link: &Link,
        network: &NetworkIdentity,
        group_id: &[u8],
        msg: chat::ChatMessage,
    ) -> Result<(), SdkError> {
        let ct = self.mls.encrypt(group_id, &msg.encode_to_vec())?;
        let recipients = self.mls.recipient_devices(group_id)?;
        let mut delivered = 0;
        for key in &recipients {
            if self
                .deliver(link, network, key, pb::EnvelopeKind::MlsMessage, &ct)
                .await
                .is_ok()
            {
                delivered += 1;
            }
        }
        if delivered == 0 && !recipients.is_empty() {
            return Err(SdkError::Delivery(
                "no store node accepted the envelope".into(),
            ));
        }
        Ok(())
    }

    /// Puts an envelope for a device into the store nodes that hold its
    /// mailbox (DHT providers), falling back to any connected store peer.
    async fn deliver(
        &self,
        link: &Link,
        network: &NetworkIdentity,
        device_pubkey: &[u8],
        kind: pb::EnvelopeKind,
        ciphertext: &[u8],
    ) -> Result<(), SdkError> {
        let t = now();
        let mut env = pb::Envelope {
            network_id: network.network_id.clone(),
            version: WIRE_VERSION,
            mailbox: signing::mailbox_for(device_pubkey).to_vec(),
            kind: kind as i32,
            ciphertext: ciphertext.to_vec(),
            created_at: t,
            expires_at: t + ENVELOPE_TTL_SECS,
            ..Default::default()
        };
        env.id = signing::envelope_id(&env).to_vec();

        let mut candidates: Vec<PeerId> =
            link.providers(dht::mailbox_for_device(device_pubkey)).await;
        for p in link.peers_with_role("store").await {
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
        let mut stored = 0;
        for p in candidates.iter().take(3) {
            match link
                .request(
                    *p,
                    pb::request::Body::MailboxPut(pb::MailboxPut {
                        envelope: Some(env.clone()),
                    }),
                )
                .await
            {
                Ok(pb::response::Body::MailboxPut(r)) if r.stored => stored += 1,
                Ok(pb::response::Body::MailboxPut(r)) => {
                    debug!(%p, reason = r.reason, "envelope refused")
                }
                Ok(_) => {}
                Err(e) => debug!(%p, error = %e, "envelope delivery failed"),
            }
        }
        if stored == 0 {
            return Err(SdkError::Delivery(format!(
                "no store node accepted an envelope for device {}",
                hex::encode(device_pubkey)
            )));
        }
        Ok(())
    }

    /// Fetches this device's mailbox from every store peer, processes
    /// everything through MLS, acknowledges, and returns decrypted messages.
    pub async fn sync(
        &mut self,
        link: &Link,
        network: &NetworkIdentity,
    ) -> Result<Vec<Received>, SdkError> {
        let mut peers: Vec<PeerId> = link
            .providers(dht::mailbox_for_device(&self.device.public_key()))
            .await;
        for p in link.peers_with_role("store").await {
            if !peers.contains(&p) {
                peers.push(p);
            }
        }
        let mut out = Vec::new();
        for p in peers {
            let mut cursor = self
                .cursors
                .get(&p.to_string())
                .cloned()
                .unwrap_or_default();
            loop {
                let mut f = pb::MailboxFetch {
                    cursor: cursor.clone(),
                    limit: 50,
                    timestamp: now(),
                    ..Default::default()
                };
                signing::sign_mailbox_fetch(network, &self.device, &mut f)?;
                let page = match link.request(p, pb::request::Body::MailboxFetch(f)).await {
                    Ok(pb::response::Body::MailboxFetch(r)) => r,
                    Ok(_) => break,
                    Err(e) => {
                        debug!(%p, error = %e, "mailbox fetch failed");
                        break;
                    }
                };
                if page.envelopes.is_empty() {
                    break;
                }
                let mut acked = Vec::new();
                for env in &page.envelopes {
                    let id_hex = hex::encode(&env.id);
                    if self.seen.contains(&id_hex) {
                        acked.push(env.id.clone());
                        continue;
                    }
                    self.seen.push(id_hex);
                    if self.seen.len() > SEEN_CAP {
                        let excess = self.seen.len() - SEEN_CAP;
                        self.seen.drain(..excess);
                    }
                    match self.process_envelope(env) {
                        Ok(Some(r)) => {
                            out.push(r);
                            acked.push(env.id.clone());
                        }
                        Ok(None) => acked.push(env.id.clone()),
                        Err(e) => {
                            // Leave it in the mailbox: another device of
                            // ours, or a welcome that arrives before the
                            // key package is ready. It expires eventually.
                            debug!(error = %e, "envelope not processed");
                            acked.push(env.id.clone());
                        }
                    }
                }
                let mut ack = pb::MailboxAck {
                    envelope_ids: acked,
                    timestamp: now(),
                    ..Default::default()
                };
                signing::sign_mailbox_ack(network, &self.device, &mut ack)?;
                let _ = link.request(p, pb::request::Body::MailboxAck(ack)).await;
                if page.cursor.is_empty() {
                    cursor.clear();
                    break;
                }
                cursor = page.cursor;
            }
            self.cursors.insert(p.to_string(), cursor);
        }
        // Replenish one-time key packages consumed by Welcomes since the
        // last sync. Failure here is not a sync failure.
        if let Err(e) = self.publish_key_package(link, network).await {
            debug!(error = %e, "key package replenish skipped");
        }
        Ok(out)
    }

    fn process_envelope(&mut self, env: &pb::Envelope) -> Result<Option<Received>, SdkError> {
        match pb::EnvelopeKind::try_from(env.kind) {
            Ok(pb::EnvelopeKind::MlsWelcome) => {
                let gid = self.mls.join(
                    &env.ciphertext,
                    GroupMeta {
                        joined_at: now(),
                        ..Default::default()
                    },
                )?;
                // A two-address group with no name is a direct chat.
                let addrs: std::collections::BTreeSet<String> = self
                    .mls
                    .members(&gid)?
                    .iter()
                    .map(|m| m.address().to_owned())
                    .collect();
                if addrs.len() <= 2 {
                    let mut meta = self.mls.groups().get(&gid).cloned().unwrap_or_default();
                    meta.direct = true;
                    self.mls.set_meta(&gid, meta);
                }
                debug!(group = hex::encode(&gid), "joined group from welcome");
                Ok(None)
            }
            Ok(pb::EnvelopeKind::MlsMessage) => match self.mls.process(&env.ciphertext)? {
                Inbound::Application {
                    group_id,
                    sender,
                    sender_key,
                    data,
                } => {
                    let msg = chat::ChatMessage::decode(data.as_slice())
                        .map_err(|e| SdkError::Delivery(e.to_string()))?;
                    if msg.kind == chat::ChatKind::GroupInfo as i32 {
                        if let Some(info) = &msg.group_info {
                            let mut meta = self
                                .mls
                                .groups()
                                .get(&group_id)
                                .cloned()
                                .unwrap_or_default();
                            meta.name = info.name.clone();
                            if !info.name.is_empty() {
                                meta.direct = false;
                            }
                            self.mls.set_meta(&group_id, meta);
                        }
                    }
                    let (addr, _) = hashgram_mls::parse_identity(&sender).unwrap_or_default();
                    Ok(Some(Received {
                        group_id: hex::encode(group_id),
                        sender: addr,
                        sender_device: hex::encode(sender_key),
                        message: msg.into(),
                    }))
                }
                Inbound::Commit { .. } | Inbound::Proposal { .. } | Inbound::Joined { .. } => {
                    Ok(None)
                }
            },
            _ => Ok(None),
        }
    }

    /// Groups this device is in, with metadata and member addresses.
    pub fn conversations(&self) -> Vec<(String, GroupMeta, Vec<String>)> {
        self.mls
            .groups()
            .iter()
            .map(|(gid, meta)| {
                let mut addrs: Vec<String> = self
                    .mls
                    .members(gid)
                    .map(|ms| ms.iter().map(|m| m.address().to_owned()).collect())
                    .unwrap_or_default();
                addrs.sort();
                addrs.dedup();
                (hex::encode(gid), meta.clone(), addrs)
            })
            .collect()
    }
}

fn random_id() -> Vec<u8> {
    let mut id = vec![0u8; 16];
    let _ = getrandom::fill(&mut id);
    id
}
