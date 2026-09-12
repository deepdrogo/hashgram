//! Messages: MLS conversations over store-and-forward mailboxes.
//!
//! The SDK does the cryptography (`hashgram_sdk::messaging`). This module
//! owns the local view: conversations and decrypted messages sealed into
//! the database, unread counts, receipts, disappearing timers, attachments
//! (encrypted before upload, hash-verified after download), and the
//! background sync loop that fetches mailboxes while the vault is unlocked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::state::SharedAccount;
use hashgram_sdk::account::Account;
use hashgram_sdk::blob;
use hashgram_sdk::chat;
use hashgram_sdk::link::Link;
use hashgram_sdk::messaging::{Messaging, Received};
use hashgram_sdk::proto::limits::WIRE_VERSION;
use hashgram_sdk::{ChainClient, NetworkIdentity};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::crypto::DbKey;
use crate::db::Db;

/// A conversation as the list shows it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversationMeta {
    /// Group id, hex.
    pub group_id: String,
    /// Name (empty for direct chats).
    pub name: String,
    /// Direct chat with one other person.
    pub direct: bool,
    /// Member addresses (including us).
    pub members: Vec<String>,
    /// Last message preview (text or kind).
    pub last_preview: String,
    /// Last message time, ms.
    pub last_ts: u64,
    /// Disappearing timer applied to outgoing messages (seconds, 0 = off).
    pub disappear_secs: u32,
    /// Unread count.
    pub unread: u32,
}

/// An attachment with everything needed to fetch and open it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AttachmentView {
    /// CID hex.
    pub cid: String,
    /// Key hex (private blobs).
    pub key: String,
    /// Nonce hex.
    pub nonce: String,
    /// MIME.
    pub mime: String,
    /// Plaintext size.
    pub size: u64,
    /// File name.
    pub name: String,
    /// image, video, audio, file.
    pub kind: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Duration ms.
    pub duration_ms: u32,
    /// BLAKE3 of plaintext, hex.
    pub plaintext_hash: String,
}

impl From<&chat::Attachment> for AttachmentView {
    fn from(a: &chat::Attachment) -> Self {
        Self {
            cid: hex::encode(&a.cid),
            key: hex::encode(&a.key),
            nonce: hex::encode(&a.nonce),
            mime: a.mime.clone(),
            size: a.size,
            name: a.name.clone(),
            kind: a.kind.clone(),
            width: a.width,
            height: a.height,
            duration_ms: a.duration_ms,
            plaintext_hash: hex::encode(&a.plaintext_hash),
        }
    }
}

/// A call signal as the UI's WebRTC needs it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallSignalView {
    /// offer, answer, ice, hangup, busy, ring.
    pub kind: String,
    /// Call id hex.
    pub call_id: String,
    /// SDP.
    pub sdp: String,
    /// ICE candidate.
    pub candidate: String,
    /// sdpMid.
    pub sdp_mid: String,
    /// sdpMLineIndex.
    pub sdp_mline_index: u32,
    /// Video requested.
    pub video: bool,
}

/// A message as the UI shows it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageView {
    /// Message id hex.
    pub id: String,
    /// Group id hex.
    pub group_id: String,
    /// Kind: TEXT, VOICE, EDIT, DELETE, REACTION, GROUP_INFO, CALL…
    pub kind: String,
    /// Sender address.
    pub sender: String,
    /// Sender device public key hex.
    pub sender_device: String,
    /// Sent by this account (any device).
    pub outgoing: bool,
    /// Sent by this very device.
    pub this_device: bool,
    /// Sender clock, ms.
    pub timestamp_ms: u64,
    /// Text.
    pub text: String,
    /// Reply target id hex.
    pub reply_to: String,
    /// Target id hex (edit/delete/reaction/receipt).
    pub target: String,
    /// Reaction string.
    pub reaction: String,
    /// Attachments.
    pub attachments: Vec<AttachmentView>,
    /// Disappearing timer in seconds.
    pub disappear_after_secs: u32,
    /// `sent`, `delivered`, `read` (ours) or `received`.
    pub state: String,
    /// Unix seconds when this message is deleted locally (0 = never).
    pub expires: u64,
    /// Call signal kind if CALL.
    pub call_kind: String,
    /// Full call signal if CALL (the webview drives WebRTC with it).
    #[serde(default)]
    pub call: Option<CallSignalView>,
    /// Reactions aggregated for this message: reaction → senders.
    #[serde(default)]
    pub reactions: HashMap<String, Vec<String>>,
    /// Group name for GROUP_INFO.
    pub group_name: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_s() -> u64 {
    now_ms().div_euclid(1000)
}

fn random_id() -> Vec<u8> {
    let mut id = vec![0u8; 16];
    let _ = getrandom::fill(&mut id);
    id
}

fn kind_name(k: i32) -> String {
    chat::ChatKind::try_from(k)
        .map(|k| k.as_str_name().trim_start_matches("CHAT_KIND_").to_owned())
        .unwrap_or_else(|_| "UNKNOWN".into())
}

fn view_of(
    m: &chat::ChatMessage,
    group_id: &str,
    sender: &str,
    sender_device: &str,
    me: &str,
    my_device: &str,
) -> MessageView {
    MessageView {
        id: hex::encode(&m.id),
        group_id: group_id.to_owned(),
        kind: kind_name(m.kind),
        sender: sender.to_owned(),
        sender_device: sender_device.to_owned(),
        outgoing: sender == me,
        this_device: sender_device == my_device,
        timestamp_ms: m.timestamp_ms,
        text: m.text.clone(),
        reply_to: hex::encode(&m.reply_to),
        target: hex::encode(&m.target),
        reaction: m.reaction.clone(),
        attachments: m.attachments.iter().map(AttachmentView::from).collect(),
        disappear_after_secs: m.disappear_after_secs,
        state: if sender == me {
            "sent".into()
        } else {
            "received".into()
        },
        expires: 0,
        call_kind: m.call.as_ref().map(|c| c.kind.clone()).unwrap_or_default(),
        call: m.call.as_ref().map(|c| CallSignalView {
            kind: c.kind.clone(),
            call_id: hex::encode(&c.call_id),
            sdp: c.sdp.clone(),
            candidate: c.candidate.clone(),
            sdp_mid: c.sdp_mid.clone(),
            sdp_mline_index: c.sdp_mline_index,
            video: c.video,
        }),
        reactions: HashMap::new(),
        group_name: m
            .group_info
            .as_ref()
            .map(|g| g.name.clone())
            .unwrap_or_default(),
    }
}

/// The hub.
pub struct ChatHub {
    inner: Mutex<Option<Messaging>>,
    syncing: AtomicBool,
    last_sync: Mutex<Option<Instant>>,
    /// Typing signals seen recently: group → (address, at).
    typing: Mutex<HashMap<String, Vec<(String, Instant)>>>,
}

impl Default for ChatHub {
    fn default() -> Self {
        Self {
            inner: Mutex::new(None),
            syncing: AtomicBool::new(false),
            last_sync: Mutex::new(None),
            typing: Mutex::new(HashMap::new()),
        }
    }
}

/// What a sync produced.
#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    /// New messages by group.
    pub new_by_group: HashMap<String, u32>,
    /// Messages for notifications (sender, preview, group).
    pub notify: Vec<(String, String, String)>,
    /// Groups whose membership changed (welcomes).
    pub groups_changed: bool,
}

impl ChatHub {
    /// Opens MLS state from the account (at unlock).
    pub async fn open(&self, account: &Account) -> Result<(), String> {
        let m = Messaging::open(account).map_err(|e| e.to_string())?;
        *self.inner.lock().await = Some(m);
        Ok(())
    }

    /// Drops MLS state (at lock).
    pub async fn close(&self) {
        *self.inner.lock().await = None;
    }

    /// Whether messaging is open.
    pub async fn is_open(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    /// Seconds since the last sync, if any.
    pub async fn last_sync_secs(&self) -> Option<u64> {
        self.last_sync.lock().await.map(|t| t.elapsed().as_secs())
    }

    /// Persists MLS state into the vault (session key: no KDF).
    async fn persist(m: &Messaging, account: &SharedAccount) -> Result<(), String> {
        let mut a = account.lock().await;
        m.persist(&mut a).map_err(|e| e.to_string())?;
        a.save().map_err(|e| e.to_string())
    }

    /// Publishes key packages (first run and replenish).
    pub async fn publish_key_packages(
        &self,
        account: &SharedAccount,
        link: &Link,
        network: &NetworkIdentity,
    ) -> Result<usize, String> {
        let mut guard = self.inner.lock().await;
        let m = guard
            .as_mut()
            .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
        let out = m
            .publish_key_package(link, network)
            .await
            .map_err(|e| e.to_string());
        Self::persist(m, account).await?;
        out
    }

    /// Conversations known to MLS, merged with local metadata.
    pub async fn conversations(
        &self,
        db: &Db,
        key: &DbKey,
        me: &str,
    ) -> Result<Vec<ConversationMeta>, String> {
        let guard = self.inner.lock().await;
        let m = guard
            .as_ref()
            .ok_or_else(|| "messaging is not open".to_owned())?;
        let mut out = Vec::new();
        for (gid, meta, members) in m.conversations() {
            let mut cm = load_meta(db, key, &gid)?.unwrap_or_default();
            cm.group_id = gid.clone();
            cm.direct = meta.direct;
            if !meta.name.is_empty() {
                cm.name = meta.name.clone();
            }
            cm.members = members;
            if cm.direct && cm.name.is_empty() {
                cm.name = cm
                    .members
                    .iter()
                    .find(|a| *a != me)
                    .cloned()
                    .unwrap_or_else(|| "(you)".into());
            }
            out.push(cm);
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.last_ts));
        Ok(out)
    }

    /// Starts (or reuses) a direct conversation.
    pub async fn start_direct(
        &self,
        account: &SharedAccount,
        link: &Link,
        chain: &ChainClient,
        network: &NetworkIdentity,
        address: &str,
    ) -> Result<String, String> {
        let to = address.to_owned();
        let mut guard = self.inner.lock().await;
        let m = guard
            .as_mut()
            .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
        let existing = m
            .conversations()
            .into_iter()
            .find(|(_, meta, addrs)| meta.direct && addrs.contains(&to) && addrs.len() <= 2)
            .map(|(gid, _, _)| gid);
        if let Some(g) = existing {
            return Ok(g);
        }
        let out = m
            .create_conversation(link, chain, network, "", std::slice::from_ref(&to))
            .await
            .map_err(|e| e.to_string());
        Self::persist(m, account).await?;
        out.map(hex::encode)
    }

    /// Creates a named group.
    pub async fn create_group(
        &self,
        account: &SharedAccount,
        link: &Link,
        chain: &ChainClient,
        network: &NetworkIdentity,
        name: &str,
        members: &[String],
    ) -> Result<String, String> {
        let mut guard = self.inner.lock().await;
        let m = guard
            .as_mut()
            .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
        let out = m
            .create_conversation(link, chain, network, name, members)
            .await
            .map_err(|e| e.to_string());
        Self::persist(m, account).await?;
        out.map(hex::encode)
    }

    /// Adds a member.
    pub async fn add_member(
        &self,
        account: &SharedAccount,
        link: &Link,
        chain: &ChainClient,
        network: &NetworkIdentity,
        group_id: &str,
        address: &str,
    ) -> Result<(), String> {
        let gid = hex::decode(group_id).map_err(|e| e.to_string())?;
        let mut guard = self.inner.lock().await;
        let m = guard
            .as_mut()
            .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
        let out = m
            .add_participant(link, chain, network, &gid, address)
            .await
            .map_err(|e| e.to_string());
        Self::persist(m, account).await?;
        out
    }

    /// Removes a member's devices.
    pub async fn remove_member(
        &self,
        account: &SharedAccount,
        link: &Link,
        network: &NetworkIdentity,
        group_id: &str,
        address: &str,
    ) -> Result<usize, String> {
        let gid = hex::decode(group_id).map_err(|e| e.to_string())?;
        let mut guard = self.inner.lock().await;
        let m = guard
            .as_mut()
            .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
        let out = m
            .remove_participant(link, network, &gid, address)
            .await
            .map_err(|e| e.to_string());
        Self::persist(m, account).await?;
        out
    }

    /// Sends any chat message and records it locally.
    // The session handles travel together; bundling them into a struct would
    // only move the same eight values one level down.
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        &self,
        account: &SharedAccount,
        db: &Db,
        key: &DbKey,
        link: &Link,
        network: &NetworkIdentity,
        group_id: &str,
        mut msg: chat::ChatMessage,
    ) -> Result<MessageView, String> {
        let gid = hex::decode(group_id).map_err(|e| e.to_string())?;
        if msg.id.is_empty() {
            msg.id = random_id();
        }
        if msg.timestamp_ms == 0 {
            msg.timestamp_ms = now_ms();
        }
        msg.version = WIRE_VERSION;
        let (me, my_device) = {
            let a = account.lock().await;
            (
                a.address().to_owned(),
                hex::encode(a.device().map_err(|e| e.to_string())?.public_key()),
            )
        };
        {
            let mut guard = self.inner.lock().await;
            let m = guard
                .as_mut()
                .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
            let out = m
                .send(link, network, &gid, msg.clone())
                .await
                .map_err(|e| e.to_string());
            Self::persist(m, account).await?;
            out?;
        }
        let view = view_of(&msg, group_id, &me, &my_device, &me, &my_device);
        if is_stored_kind(msg.kind) {
            store_message(db, key, &view)?;
            touch_meta(db, key, group_id, &view, false)?;
        }
        Ok(view)
    }

    /// Fetches mailboxes and stores what arrived. Returns what is new so
    /// the caller can notify.
    pub async fn sync(
        &self,
        account: &SharedAccount,
        db: &Db,
        key: &DbKey,
        link: &Link,
        network: &NetworkIdentity,
        chain: Option<&ChainClient>,
    ) -> Result<SyncReport, String> {
        if self.syncing.swap(true, Ordering::SeqCst) {
            return Ok(SyncReport::default());
        }
        let res = self
            .sync_inner(account, db, key, link, network, chain)
            .await;
        self.syncing.store(false, Ordering::SeqCst);
        *self.last_sync.lock().await = Some(Instant::now());
        res
    }

    async fn sync_inner(
        &self,
        account: &SharedAccount,
        db: &Db,
        key: &DbKey,
        link: &Link,
        network: &NetworkIdentity,
        chain: Option<&ChainClient>,
    ) -> Result<SyncReport, String> {
        let (me, my_device) = {
            let a = account.lock().await;
            (
                a.address().to_owned(),
                hex::encode(a.device().map_err(|e| e.to_string())?.public_key()),
            )
        };
        let (received, before, after): (Vec<Received>, usize, usize) = {
            let mut guard = self.inner.lock().await;
            let m = guard
                .as_mut()
                .ok_or_else(|| "messaging is not open (locked?)".to_owned())?;
            let before = m.conversations().len();
            let out = m
                .sync(link, network, chain)
                .await
                .map_err(|e| e.to_string());
            Self::persist(m, account).await?;
            let after = m.conversations().len();
            (out?, before, after)
        };
        let mut report = SyncReport {
            groups_changed: after != before,
            ..Default::default()
        };
        let mut deliver: Vec<(String, Vec<u8>)> = Vec::new();
        for r in received {
            let raw = &r.raw;
            let view = view_of(
                raw,
                &r.group_id,
                &r.sender,
                &r.sender_device,
                &me,
                &my_device,
            );
            match raw.kind {
                k if k == chat::ChatKind::Typing as i32 => {
                    self.typing
                        .lock()
                        .await
                        .entry(r.group_id.clone())
                        .or_default()
                        .push((r.sender.clone(), Instant::now()));
                }
                k if k == chat::ChatKind::Read as i32 || k == chat::ChatKind::Delivered as i32 => {
                    let state = if k == chat::ChatKind::Read as i32 {
                        "read"
                    } else {
                        "delivered"
                    };
                    update_state(
                        db,
                        &r.group_id,
                        &view.target,
                        state,
                        k == chat::ChatKind::Read as i32,
                    )?;
                }
                k if k == chat::ChatKind::Reaction as i32 => {
                    store_message(db, key, &view)?;
                }
                k if k == chat::ChatKind::Delete as i32 => {
                    tombstone(db, key, &r.group_id, &view.target, &r.sender)?;
                }
                k if k == chat::ChatKind::Edit as i32 => {
                    apply_edit(db, key, &r.group_id, &view.target, &r.sender, &view.text)?;
                }
                _ => {
                    let outgoing = view.outgoing;
                    store_message(db, key, &view)?;
                    touch_meta(db, key, &r.group_id, &view, !outgoing)?;
                    if !outgoing {
                        *report.new_by_group.entry(r.group_id.clone()).or_default() += 1;
                        let preview = if view.text.is_empty() {
                            view.kind.to_lowercase()
                        } else {
                            view.text.chars().take(80).collect()
                        };
                        report
                            .notify
                            .push((r.sender.clone(), preview, r.group_id.clone()));
                        if is_stored_kind(raw.kind) && raw.kind != chat::ChatKind::GroupInfo as i32
                        {
                            deliver.push((r.group_id.clone(), raw.id.clone()));
                        }
                    }
                }
            }
        }
        // Delivery receipts for what we just received, best effort.
        for (gid, id) in deliver {
            let g = hex::decode(&gid).unwrap_or_default();
            let receipt = chat::ChatMessage {
                version: WIRE_VERSION,
                kind: chat::ChatKind::Delivered as i32,
                id: random_id(),
                timestamp_ms: now_ms(),
                target: id,
                ..Default::default()
            };
            let mut guard = self.inner.lock().await;
            if let Some(m) = guard.as_mut() {
                let _ = m.send(link, network, &g, receipt).await;
                let _ = Self::persist(m, account).await;
            }
        }
        Ok(report)
    }

    /// Who is typing in a group (last 6 s).
    pub async fn typing_in(&self, group_id: &str) -> Vec<String> {
        let mut t = self.typing.lock().await;
        let cutoff = Instant::now() - Duration::from_secs(6);
        if let Some(v) = t.get_mut(group_id) {
            v.retain(|(_, at)| *at > cutoff);
            let mut names: Vec<String> = v.iter().map(|(a, _)| a.clone()).collect();
            names.sort();
            names.dedup();
            return names;
        }
        Vec::new()
    }

    /// Members' identities as MLS sees them (address, device key hex).
    pub async fn members(&self, group_id: &str) -> Result<Vec<(String, String)>, String> {
        let guard = self.inner.lock().await;
        let m = guard
            .as_ref()
            .ok_or_else(|| "messaging is not open".to_owned())?;
        let gid = hex::decode(group_id).map_err(|e| e.to_string())?;
        let members = m.mls().members(&gid).map_err(|e| e.to_string())?;
        Ok(members
            .iter()
            .map(|mm| {
                let (addr, dev) =
                    hashgram_sdk::mls::parse_identity(&mm.identity).unwrap_or_default();
                (addr, hex::encode(dev))
            })
            .collect())
    }
}

fn is_stored_kind(k: i32) -> bool {
    k == chat::ChatKind::Text as i32
        || k == chat::ChatKind::Voice as i32
        || k == chat::ChatKind::GroupInfo as i32
        || k == chat::ChatKind::Call as i32
        || k == chat::ChatKind::Reaction as i32
}

// -- database ---------------------------------------------------------------

fn load_meta(db: &Db, key: &DbKey, group_id: &str) -> Result<Option<ConversationMeta>, String> {
    let row: Option<(Vec<u8>, i64)> = db.with(|c| {
        c.query_row(
            "SELECT sealed, unread FROM conversations WHERE group_id = ?1",
            params![group_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
    })?;
    match row {
        Some((sealed, unread)) => {
            let json = crate::crypto::open(key, group_id.as_bytes(), &sealed)?;
            let mut m: ConversationMeta =
                serde_json::from_slice(&json).map_err(|e| e.to_string())?;
            m.unread = unread.max(0) as u32;
            Ok(Some(m))
        }
        None => Ok(None),
    }
}

fn save_meta(db: &Db, key: &DbKey, m: &ConversationMeta) -> Result<(), String> {
    let json = serde_json::to_vec(m).map_err(|e| e.to_string())?;
    let sealed = crate::crypto::seal(key, m.group_id.as_bytes(), &json)?;
    db.with(|c| {
        c.execute(
            "INSERT INTO conversations(group_id, sealed, updated, unread) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(group_id) DO UPDATE SET sealed = excluded.sealed, updated = excluded.updated, unread = excluded.unread",
            params![m.group_id, sealed, now_s() as i64, m.unread as i64],
        )
        .map(|_| ())
    })
}

fn touch_meta(
    db: &Db,
    key: &DbKey,
    group_id: &str,
    view: &MessageView,
    bump_unread: bool,
) -> Result<(), String> {
    let mut m = load_meta(db, key, group_id)?.unwrap_or_default();
    m.group_id = group_id.to_owned();
    if view.timestamp_ms >= m.last_ts {
        m.last_ts = view.timestamp_ms;
        m.last_preview = if !view.text.is_empty() {
            view.text.chars().take(80).collect()
        } else if let Some(first) = view.attachments.first() {
            format!("[{}]", first.kind)
        } else {
            view.kind.to_lowercase()
        };
    }
    if view.kind == "GROUP_INFO" && !view.group_name.is_empty() {
        m.name = view.group_name.clone();
    }
    if bump_unread {
        m.unread += 1;
    }
    save_meta(db, key, &m)
}

/// Sets a conversation's disappearing timer.
pub fn set_disappear(db: &Db, key: &DbKey, group_id: &str, secs: u32) -> Result<(), String> {
    let mut m = load_meta(db, key, group_id)?.unwrap_or_default();
    m.group_id = group_id.to_owned();
    m.disappear_secs = secs;
    save_meta(db, key, &m)
}

/// The disappearing timer for a conversation.
pub fn disappear_of(db: &Db, key: &DbKey, group_id: &str) -> u32 {
    load_meta(db, key, group_id)
        .ok()
        .flatten()
        .map(|m| m.disappear_secs)
        .unwrap_or(0)
}

fn store_message(db: &Db, key: &DbKey, v: &MessageView) -> Result<(), String> {
    let json = serde_json::to_vec(v).map_err(|e| e.to_string())?;
    let sealed = crate::crypto::seal(key, v.id.as_bytes(), &json)?;
    db.with(|c| {
        c.execute(
            "INSERT INTO messages(id, group_id, sender, device, ts, kind, sealed, state, expires)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0)
             ON CONFLICT(id) DO NOTHING",
            params![
                v.id,
                v.group_id,
                v.sender,
                v.sender_device,
                v.timestamp_ms as i64,
                v.kind,
                sealed,
                v.state
            ],
        )
        .map(|_| ())
    })
}

fn update_state(
    db: &Db,
    group_id: &str,
    target: &str,
    state: &str,
    is_read: bool,
) -> Result<(), String> {
    // Receipts move a message forward only: sent → delivered → read.
    db.with(|c| {
        if is_read {
            // A READ receipt covers everything up to and including the target.
            c.execute(
                "UPDATE messages SET state = 'read' WHERE group_id = ?1 AND state IN ('sent','delivered')
                 AND ts <= (SELECT ts FROM messages WHERE id = ?2)",
                params![group_id, target],
            )
            .map(|_| ())
        } else {
            c.execute(
                "UPDATE messages SET state = ?3 WHERE id = ?2 AND group_id = ?1 AND state = 'sent'",
                params![group_id, target, state],
            )
            .map(|_| ())
        }
    })
}

fn tombstone(db: &Db, key: &DbKey, group_id: &str, target: &str, by: &str) -> Result<(), String> {
    let existing: Option<Vec<u8>> = db.with(|c| {
        c.query_row(
            "SELECT sealed FROM messages WHERE id = ?1 AND group_id = ?2 AND sender = ?3",
            params![target, group_id, by],
            |r| r.get(0),
        )
        .optional()
    })?;
    if let Some(sealed) = existing {
        let json = crate::crypto::open(key, target.as_bytes(), &sealed)?;
        let mut v: MessageView = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        v.text = String::new();
        v.attachments.clear();
        v.kind = "DELETE".into();
        let json = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        let sealed = crate::crypto::seal(key, target.as_bytes(), &json)?;
        db.with(|c| {
            c.execute(
                "UPDATE messages SET sealed = ?2, kind = 'DELETE' WHERE id = ?1",
                params![target, sealed],
            )
            .map(|_| ())
        })?;
    }
    Ok(())
}

fn apply_edit(
    db: &Db,
    key: &DbKey,
    group_id: &str,
    target: &str,
    by: &str,
    text: &str,
) -> Result<(), String> {
    let existing: Option<Vec<u8>> = db.with(|c| {
        c.query_row(
            "SELECT sealed FROM messages WHERE id = ?1 AND group_id = ?2 AND sender = ?3",
            params![target, group_id, by],
            |r| r.get(0),
        )
        .optional()
    })?;
    if let Some(sealed) = existing {
        let json = crate::crypto::open(key, target.as_bytes(), &sealed)?;
        let mut v: MessageView = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        v.text = text.to_owned();
        let json = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        let sealed = crate::crypto::seal(key, target.as_bytes(), &json)?;
        db.with(|c| {
            c.execute(
                "UPDATE messages SET sealed = ?2 WHERE id = ?1",
                params![target, sealed],
            )
            .map(|_| ())
        })?;
    }
    Ok(())
}

/// Applies an edit (public wrapper for our own edits).
pub fn apply_edit_public(
    db: &Db,
    key: &DbKey,
    group_id: &str,
    target: &str,
    by: &str,
    text: &str,
) -> Result<(), String> {
    apply_edit(db, key, group_id, target, by, text)
}

/// Tombstones a message (public wrapper for our own deletes).
pub fn tombstone_public(
    db: &Db,
    key: &DbKey,
    group_id: &str,
    target: &str,
    by: &str,
) -> Result<(), String> {
    tombstone(db, key, group_id, target, by)
}

/// History of a group, newest last, with reactions folded in.
pub fn history(
    db: &Db,
    key: &DbKey,
    group_id: &str,
    before_ts: Option<u64>,
    limit: usize,
) -> Result<Vec<MessageView>, String> {
    let rows: Vec<(String, Vec<u8>, String, i64)> = db.with(|c| {
        let mut st = c.prepare(
            "SELECT id, sealed, state, expires FROM messages WHERE group_id = ?1 AND ts < ?2 AND kind != 'REACTION'
             ORDER BY ts DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![group_id, before_ts.map(|t| t as i64).unwrap_or(i64::MAX), limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect()
    })?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, sealed, state, expires) in rows {
        let json = crate::crypto::open(key, id.as_bytes(), &sealed)?;
        let mut v: MessageView = serde_json::from_slice(&json).map_err(|e| e.to_string())?;
        v.state = state;
        v.expires = expires.max(0) as u64;
        out.push(v);
    }
    out.reverse();
    // Reactions.
    let reactions: Vec<(String, Vec<u8>)> = db.with(|c| {
        let mut st =
            c.prepare("SELECT id, sealed FROM messages WHERE group_id = ?1 AND kind = 'REACTION'")?;
        let rows = st.query_map(params![group_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })?;
    let mut by_target: HashMap<String, HashMap<String, Vec<String>>> = HashMap::new();
    for (id, sealed) in reactions {
        if let Ok(json) = crate::crypto::open(key, id.as_bytes(), &sealed) {
            if let Ok(r) = serde_json::from_slice::<MessageView>(&json) {
                if r.reaction.is_empty() {
                    continue;
                }
                by_target
                    .entry(r.target)
                    .or_default()
                    .entry(r.reaction)
                    .or_default()
                    .push(r.sender);
            }
        }
    }
    for v in &mut out {
        if let Some(r) = by_target.remove(&v.id) {
            v.reactions = r;
        }
    }
    Ok(out)
}

/// Marks a conversation read: clears unread, starts disappearing timers
/// for incoming messages that carry one, and returns the id of the newest
/// incoming message so the caller can send a READ receipt.
pub fn mark_read(db: &Db, key: &DbKey, group_id: &str, me: &str) -> Result<Option<String>, String> {
    let mut m = load_meta(db, key, group_id)?.unwrap_or_default();
    m.group_id = group_id.to_owned();
    m.unread = 0;
    save_meta(db, key, &m)?;
    // Disappearing: the timer starts when read.
    let rows: Vec<(String, Vec<u8>)> = db.with(|c| {
        let mut st = c.prepare(
            "SELECT id, sealed FROM messages WHERE group_id = ?1 AND sender != ?2 AND expires = 0",
        )?;
        let rows = st.query_map(params![group_id, me], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })?;
    for (id, sealed) in rows {
        if let Ok(json) = crate::crypto::open(key, id.as_bytes(), &sealed) {
            if let Ok(v) = serde_json::from_slice::<MessageView>(&json) {
                if v.disappear_after_secs > 0 {
                    let exp = now_s() + u64::from(v.disappear_after_secs);
                    db.with(|c| {
                        c.execute(
                            "UPDATE messages SET expires = ?2 WHERE id = ?1",
                            params![id, exp as i64],
                        )
                        .map(|_| ())
                    })?;
                }
            }
        }
    }
    db.with(|c| {
        c.query_row(
            "SELECT id FROM messages WHERE group_id = ?1 AND sender != ?2 AND kind IN ('TEXT','VOICE') ORDER BY ts DESC LIMIT 1",
            params![group_id, me],
            |r| r.get::<_, String>(0),
        )
        .optional()
    })
}

/// Deletes messages whose disappearing timer elapsed. Returns how many.
pub fn sweep_expired(db: &Db) -> Result<usize, String> {
    db.with(|c| {
        c.execute(
            "DELETE FROM messages WHERE expires > 0 AND expires <= ?1",
            params![now_s() as i64],
        )
    })
}

/// Total unread across conversations (tray badge).
pub fn unread_total(db: &Db) -> u32 {
    db.with(|c| {
        c.query_row(
            "SELECT COALESCE(SUM(unread),0) FROM conversations",
            [],
            |r| r.get::<_, i64>(0),
        )
    })
    .unwrap_or(0)
    .max(0) as u32
}

/// Search over local plaintext (decrypts in memory; the database holds
/// only ciphertext).
pub fn search(db: &Db, key: &DbKey, query: &str, limit: usize) -> Result<Vec<MessageView>, String> {
    let q = query.to_lowercase();
    if q.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, Vec<u8>)> = db.with(|c| {
        let mut st = c.prepare(
            "SELECT id, sealed FROM messages WHERE kind IN ('TEXT') ORDER BY ts DESC LIMIT 5000",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })?;
    let mut out = Vec::new();
    for (id, sealed) in rows {
        if let Ok(json) = crate::crypto::open(key, id.as_bytes(), &sealed) {
            if let Ok(v) = serde_json::from_slice::<MessageView>(&json) {
                if v.text.to_lowercase().contains(&q) {
                    out.push(v);
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
    }
    Ok(out)
}

// -- attachments ------------------------------------------------------------

/// Encrypts and uploads a file as a private blob and returns the
/// attachment record to put in a message.
#[allow(clippy::too_many_arguments)] // mirrors the attachment record field by field
pub async fn upload_attachment(
    link: &Link,
    network: &NetworkIdentity,
    device: &hashgram_sdk::proto::Ed25519Signer,
    data: &[u8],
    mime: &str,
    name: &str,
    kind: &str,
    duration_ms: u32,
) -> Result<chat::Attachment, String> {
    let plaintext_hash = blake3::hash(data).as_bytes().to_vec();
    let up = blob::upload(link, network, device, data, mime, true, 2)
        .await
        .map_err(|e| e.to_string())?;
    let key = up
        .key
        .as_deref()
        .and_then(|k| hex::decode(k).ok())
        .unwrap_or_default();
    let nonce = up
        .nonce
        .as_deref()
        .and_then(|n| hex::decode(n).ok())
        .unwrap_or_default();
    Ok(chat::Attachment {
        cid: hex::decode(&up.cid).map_err(|e| e.to_string())?,
        key,
        nonce,
        mime: mime.to_owned(),
        size: data.len() as u64,
        name: name.to_owned(),
        kind: kind.to_owned(),
        duration_ms,
        plaintext_hash,
        ..Default::default()
    })
}

/// Downloads, decrypts and verifies an attachment into the media cache,
/// returning the file path. Idempotent: an existing verified file is reused.
pub async fn fetch_attachment(
    link: &Link,
    a: &AttachmentView,
    receipt: Option<(
        &ChainClient,
        &NetworkIdentity,
        &hashgram_sdk::proto::Ed25519Signer,
    )>,
) -> Result<std::path::PathBuf, String> {
    let dir = crate::paths::data_dir().join("media-cache");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ext = ext_for(&a.mime);
    let path = dir.join(format!("{}{ext}", a.cid));
    if path.exists() {
        return Ok(path);
    }
    let cid = hex::decode(&a.cid).map_err(|e| e.to_string())?;
    let (bytes, _manifest, _from) = blob::download(link, &cid, receipt)
        .await
        .map_err(|e| e.to_string())?;
    let plain = if !a.key.is_empty() {
        let fk = blob::FileKey {
            key: hex::decode(&a.key)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "attachment key is not 32 bytes".to_owned())?,
            nonce: hex::decode(&a.nonce)
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "attachment nonce is not 24 bytes".to_owned())?,
        };
        blob::decrypt_private(&bytes, &fk).map_err(|e| e.to_string())?
    } else {
        bytes
    };
    if !a.plaintext_hash.is_empty()
        && hex::encode(blake3::hash(&plain).as_bytes()) != a.plaintext_hash
    {
        return Err("attachment hash mismatch: the provider served corrupt data".into());
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &plain).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// File extension for a MIME type (for the media cache and the webview).
#[must_use]
pub fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "video/mp4" => ".mp4",
        "video/webm" => ".webm",
        "audio/webm" => ".weba",
        "audio/ogg" => ".ogg",
        "audio/mpeg" => ".mp3",
        "audio/mp4" | "audio/aac" => ".m4a",
        "application/pdf" => ".pdf",
        _ => ".bin",
    }
}

/// Guesses MIME and kind from a file name.
#[must_use]
pub fn mime_for(name: &str) -> (&'static str, &'static str) {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" => ("image/jpeg", "image"),
        "png" => ("image/png", "image"),
        "gif" => ("image/gif", "image"),
        "webp" => ("image/webp", "image"),
        "mp4" | "m4v" => ("video/mp4", "video"),
        "webm" => ("video/webm", "video"),
        "mp3" => ("audio/mpeg", "audio"),
        "m4a" | "aac" => ("audio/mp4", "audio"),
        "ogg" | "oga" => ("audio/ogg", "audio"),
        "pdf" => ("application/pdf", "file"),
        _ => ("application/octet-stream", "file"),
    }
}

/// Builds a TEXT message.
#[must_use]
pub fn text_message(
    text: &str,
    reply_to: Option<&str>,
    disappear_after_secs: u32,
    attachments: Vec<chat::Attachment>,
) -> chat::ChatMessage {
    chat::ChatMessage {
        version: WIRE_VERSION,
        kind: if attachments.iter().any(|a| a.kind == "audio") && text.is_empty() {
            chat::ChatKind::Voice as i32
        } else {
            chat::ChatKind::Text as i32
        },
        id: random_id(),
        timestamp_ms: now_ms(),
        text: text.to_owned(),
        reply_to: reply_to
            .and_then(|r| hex::decode(r).ok())
            .unwrap_or_default(),
        attachments,
        disappear_after_secs,
        ..Default::default()
    }
}

/// Builds a control message (REACTION, READ, TYPING, EDIT, DELETE).
#[must_use]
pub fn control_message(
    kind: chat::ChatKind,
    target: Option<&str>,
    text: &str,
    reaction: &str,
) -> chat::ChatMessage {
    chat::ChatMessage {
        version: WIRE_VERSION,
        kind: kind as i32,
        id: random_id(),
        timestamp_ms: now_ms(),
        text: text.to_owned(),
        target: target.and_then(|t| hex::decode(t).ok()).unwrap_or_default(),
        reaction: reaction.to_owned(),
        ..Default::default()
    }
}

/// Total size guard for attachments (100 MiB).
pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024 * 1024;

/// Shared hub handle type.
pub type SharedChat = Arc<ChatHub>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_guessing_and_extensions() {
        assert_eq!(mime_for("photo.JPG"), ("image/jpeg", "image"));
        assert_eq!(mime_for("clip.mp4"), ("video/mp4", "video"));
        assert_eq!(mime_for("note.weird"), ("application/octet-stream", "file"));
        assert_eq!(ext_for("video/mp4"), ".mp4");
        assert_eq!(ext_for("x/y"), ".bin");
    }

    #[test]
    fn text_message_becomes_voice_when_only_audio_is_attached() {
        let a = chat::Attachment {
            kind: "audio".into(),
            ..Default::default()
        };
        let m = text_message("", None, 0, vec![a]);
        assert_eq!(m.kind, chat::ChatKind::Voice as i32);
        let t = text_message("hi", None, 30, vec![]);
        assert_eq!(t.kind, chat::ChatKind::Text as i32);
        assert_eq!(t.disappear_after_secs, 30);
        assert_eq!(t.id.len(), 16);
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("hg-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("t.db")
    }

    #[test]
    fn history_receipts_reactions_and_disappearing() {
        let path = tmp("hist");
        let db = Db::open(&path).unwrap();
        let key = crate::crypto::generate_key().unwrap();
        let me = "hash1me";
        let other = "hash1other";
        let mine = MessageView {
            id: "aa".into(),
            group_id: "g1".into(),
            kind: "TEXT".into(),
            sender: me.into(),
            outgoing: true,
            timestamp_ms: 1000,
            text: "hello".into(),
            state: "sent".into(),
            ..Default::default()
        };
        let theirs = MessageView {
            id: "bb".into(),
            group_id: "g1".into(),
            kind: "TEXT".into(),
            sender: other.into(),
            timestamp_ms: 2000,
            text: "secret plan".into(),
            state: "received".into(),
            disappear_after_secs: 1,
            ..Default::default()
        };
        store_message(&db, &key, &mine).unwrap();
        store_message(&db, &key, &theirs).unwrap();
        touch_meta(&db, &key, "g1", &theirs, true).unwrap();
        assert_eq!(unread_total(&db), 1);
        // A reaction to my message.
        let react = MessageView {
            id: "cc".into(),
            group_id: "g1".into(),
            kind: "REACTION".into(),
            sender: other.into(),
            timestamp_ms: 2500,
            target: "aa".into(),
            reaction: "👍".into(),
            ..Default::default()
        };
        store_message(&db, &key, &react).unwrap();
        // A READ receipt for "aa".
        update_state(&db, "g1", "aa", "read", true).unwrap();
        let h = history(&db, &key, "g1", None, 50).unwrap();
        assert_eq!(h.len(), 2, "reactions are folded, not listed");
        assert_eq!(h[0].id, "aa");
        assert_eq!(h[0].state, "read");
        assert_eq!(h[0].reactions.get("👍").map(|v| v.len()), Some(1));
        // Marking read starts the disappearing timer on theirs.
        let last = mark_read(&db, &key, "g1", me).unwrap();
        assert_eq!(last.as_deref(), Some("bb"));
        assert_eq!(unread_total(&db), 0);
        let h = history(&db, &key, "g1", None, 50).unwrap();
        assert!(h[1].expires > 0);
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(sweep_expired(&db).unwrap(), 1);
        assert_eq!(history(&db, &key, "g1", None, 50).unwrap().len(), 1);
        // Search decrypts in memory.
        assert_eq!(search(&db, &key, "HELLO", 10).unwrap().len(), 1);
        // The plaintext is not in the file.
        drop(db);
        let mut bytes = std::fs::read(&path).unwrap_or_default();
        if let Ok(w) = std::fs::read(path.with_extension("db-wal")) {
            bytes.extend(w);
        }
        assert!(!bytes.windows(5).any(|w| w == b"hello"));
    }
}
