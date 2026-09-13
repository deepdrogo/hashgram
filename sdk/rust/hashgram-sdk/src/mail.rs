//! HashMail: send, receive, file and thread end-to-end encrypted mail.
//!
//! Transport: one MLS group per participant set (sender + To + CC), found
//! or created through [`HashgramOne::conversation_group`]; each BCC
//! recipient gets a separate copy in a sender+recipient group. The message
//! model, its bounds and threading rules live in `hashgram_app::mail`; this
//! module adds the network round trips and the local mailbox.
//!
//! # Local mailbox
//!
//! Messages are stored in the local store under `mail/msg/<message_id>`
//! as [`MailRecord`] (message + folder + flags). An in-memory index
//! ([`MailState`]) of `(received_at_ms, id)` per folder and per thread is
//! rebuilt at open and kept current, so listing a folder is a slice, not a
//! scan. Flag changes are queued into a `MailStateHint` and flushed to the
//! self group on the next sync so the user's other devices converge.
//!
//! Folder names are fixed strings ([`folder`]); labels are free-form.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hashgram_app::mail as m;
use hashgram_app::pb as app;
use hashgram_app::spam;
use hashgram_app::AppError;
use tracing::{debug, warn};

use crate::app::HashgramOne;
use crate::messaging::Received;
use crate::SdkError;

/// Folder names.
pub mod folder {
    /// Inbox.
    pub const INBOX: &str = "inbox";
    /// Sent.
    pub const SENT: &str = "sent";
    /// Drafts.
    pub const DRAFTS: &str = "drafts";
    /// Archive.
    pub const ARCHIVE: &str = "archive";
    /// Trash.
    pub const TRASH: &str = "trash";
    /// Spam.
    pub const SPAM: &str = "spam";
    /// Unknown senders awaiting a decision.
    pub const REQUESTS: &str = "requests";
    /// All known folders.
    pub const ALL: [&str; 7] = [INBOX, SENT, DRAFTS, ARCHIVE, TRASH, SPAM, REQUESTS];
}

const NS_MSG: &str = "mail/msg";
const NS_META: &str = "mail/meta";
const NS_DRAFT: &str = "mail/draft";

/// Most non-contact recipients one message copy carries postage for.
const MAX_STAMPS_PER_MESSAGE: usize = 8;

/// Appends labels while staying inside the message's label bound.
fn push_labels(msg: &mut app::MailMessage, labels: Vec<String>) {
    for l in labels {
        if msg.labels.len() >= m::MAX_LABELS {
            break;
        }
        msg.labels.push(l);
    }
}

/// A stored message with its local state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailRecord {
    /// The message.
    pub message: app::MailMessage,
    /// Folder.
    pub folder: String,
    /// Read.
    pub read: bool,
    /// Starred.
    pub starred: bool,
    /// Labels.
    pub labels: Vec<String>,
    /// When this device stored it (ms).
    pub received_at_ms: u64,
    /// Sender address as authenticated by MLS (may differ from
    /// `message.from` if a member lied; readers show this one).
    pub authenticated_sender: String,
    /// MLS group the message arrived in / was sent to (hex).
    pub group_id: String,
    /// Delivery receipts received for a sent message: address → at_ms.
    #[serde(default)]
    pub delivered_to: BTreeMap<String, u64>,
    /// Read receipts: address → at_ms.
    #[serde(default)]
    pub read_by: BTreeMap<String, u64>,
    /// Whether this is our own copy of a sent message.
    #[serde(default)]
    pub outgoing: bool,
    /// Spam disposition score at arrival.
    #[serde(default)]
    pub trust_score: i32,
}

/// A folder listing row (no bodies, for fast lists).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MailSummary {
    /// Hex message id.
    pub id: String,
    /// Hex thread id.
    pub thread_id: String,
    /// Folder.
    pub folder: String,
    /// From (authenticated address).
    pub from: String,
    /// From username hint.
    pub from_username: String,
    /// To addresses.
    pub to: Vec<String>,
    /// Subject.
    pub subject: String,
    /// First 160 chars of the text body.
    pub preview: String,
    /// Sender clock.
    pub created_at_ms: u64,
    /// Local arrival.
    pub received_at_ms: u64,
    /// Read.
    pub read: bool,
    /// Starred.
    pub starred: bool,
    /// Attachment count.
    pub attachments: usize,
    /// Labels.
    pub labels: Vec<String>,
    /// Bridged from external mail.
    pub external: bool,
    /// BCC copy.
    pub bcc_copy: bool,
    /// Outgoing.
    pub outgoing: bool,
}

/// A thread.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Thread {
    /// Hex thread id.
    pub id: String,
    /// Subject of the first message.
    pub subject: String,
    /// Messages oldest first.
    pub messages: Vec<MailRecord>,
    /// Participants.
    pub participants: Vec<String>,
    /// Unread count.
    pub unread: usize,
}

/// Per-folder counters.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FolderCounts {
    /// Total.
    pub total: usize,
    /// Unread.
    pub unread: usize,
}

/// Local mail settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailSettings {
    /// Send read receipts when a sender asks.
    pub send_read_receipts: bool,
    /// Keep sent messages.
    pub keep_sent: bool,
    /// Days after which trash/spam are purged (0 = never).
    pub purge_after_days: u32,
}

impl Default for MailSettings {
    fn default() -> Self {
        Self {
            send_read_receipts: false,
            keep_sent: true,
            purge_after_days: 30,
        }
    }
}

/// In-memory indexes.
#[derive(Debug, Default)]
pub struct MailState {
    /// folder → sorted (received_at_ms, id hex), newest last.
    by_folder: HashMap<String, Vec<(u64, String)>>,
    /// thread → message ids.
    by_thread: HashMap<String, Vec<String>>,
    /// id → (folder, read, starred, thread)
    index: HashMap<String, (String, bool, bool, String)>,
    /// Pending flag changes for other devices.
    pending_hint: app::MailStateHint,
    /// Rate window for unknown senders.
    window: spam::RateWindow,
    /// Settings.
    pub settings: MailSettings,
}

impl MailState {
    pub(crate) fn load(store: &crate::store::LocalStore) -> Result<Self, SdkError> {
        let mut s = Self {
            settings: store.get(NS_META, b"settings")?.unwrap_or_default(),
            window: store.get(NS_META, b"rate_window")?.unwrap_or_default(),
            ..Default::default()
        };
        let records: Vec<(Vec<u8>, MailRecord)> = store.scan(NS_MSG)?;
        for (k, r) in records {
            s.index_record(&hex::encode(k), &r);
        }
        for v in s.by_folder.values_mut() {
            v.sort();
        }
        Ok(s)
    }

    fn index_record(&mut self, id: &str, r: &MailRecord) {
        self.unindex(id);
        self.by_folder
            .entry(r.folder.clone())
            .or_default()
            .push((r.received_at_ms, id.to_owned()));
        let thread = hex::encode(&r.message.thread_id);
        self.by_thread
            .entry(thread.clone())
            .or_default()
            .push(id.to_owned());
        self.index
            .insert(id.to_owned(), (r.folder.clone(), r.read, r.starred, thread));
    }

    fn unindex(&mut self, id: &str) {
        if let Some((folder, _, _, thread)) = self.index.remove(id) {
            if let Some(v) = self.by_folder.get_mut(&folder) {
                v.retain(|(_, i)| i != id);
            }
            if let Some(v) = self.by_thread.get_mut(&thread) {
                v.retain(|i| i != id);
            }
        }
    }

    fn insert_sorted(&mut self, folder: &str, at: u64, id: &str) {
        let v = self.by_folder.entry(folder.to_owned()).or_default();
        let pos = v.partition_point(|(t, i)| (*t, i.as_str()) < (at, id));
        v.insert(pos, (at, id.to_owned()));
    }
}

/// The Mail API.
pub struct Mail<'a> {
    pub(crate) one: &'a mut HashgramOne,
}

impl<'a> Mail<'a> {
    // -----------------------------------------------------------------------
    // Sending
    // -----------------------------------------------------------------------

    /// Resolves user-typed recipients into `MailAddress`es through the
    /// chain. External addresses are refused here; the gateway flow wraps
    /// them (see `docs/MAIL_GATEWAY.md`).
    pub async fn resolve_recipients(
        &mut self,
        inputs: &[String],
    ) -> Result<Vec<app::MailAddress>, SdkError> {
        let mut out = Vec::with_capacity(inputs.len());
        for i in inputs {
            let r = self.one.people().resolve(i).await?;
            out.push(app::MailAddress {
                address: r.address,
                username: r.username,
                display_name: r.display_name,
            });
        }
        Ok(out)
    }

    /// Our own `MailAddress`.
    pub async fn my_address(&mut self) -> app::MailAddress {
        let me = self.one.account.address().to_owned();
        let username = self.one.people().my_username().await.unwrap_or_default();
        app::MailAddress {
            address: me,
            username,
            display_name: self.one.people_state.my_display_name.clone(),
        }
    }

    /// Sends a draft. Returns the message id (hex). The draft's `from` is
    /// overwritten with our authenticated address.
    pub async fn send(&mut self, mut draft: m::Draft) -> Result<String, SdkError> {
        draft.from = self.my_address().await;
        let mut out = draft.build_all()?;
        self.stamp_first_contacts(&mut out).await;
        let id = self.send_built(out.main, out.bcc_copies).await?;
        // Live Drive attachments: remember the group so updates flow.
        for a in &draft.attachments {
            if let Some(app::mail_attachment::Source::Drive(cap)) = &a.source {
                if cap.mode == app::DriveShareMode::Live as i32 {
                    debug!(share = hex::encode(&cap.share_id), "live attachment sent");
                }
            }
        }
        Ok(id)
    }

    /// Attaches postage stamps (`hashgram_app::spam`) for recipients who
    /// are not our contacts: a proof of work bound to each such recipient
    /// and this message id, which their client scores as trust. Contacts
    /// need none (they file us to Inbox regardless). Bounded to
    /// [`MAX_STAMPS_PER_MESSAGE`] recipients per copy so a wide send stays
    /// cheap and a bulk sender gains nothing from it. Never fails: a stamp
    /// that cannot be minted is simply absent.
    async fn stamp_first_contacts(&self, out: &mut m::Outgoing) {
        let me = self.one.account.address().to_owned();
        let needs_stamp = |addr: &str| -> bool {
            addr != me
                && addr.starts_with("hash1")
                && !self.one.people_state.contacts.sender_facts(addr).is_contact
        };
        let mut jobs: Vec<(Vec<String>, Vec<u8>)> = Vec::new();
        if let Some(main) = &out.main {
            let strangers: Vec<String> = main
                .to
                .iter()
                .chain(main.cc.iter())
                .map(|a| a.address.clone())
                .filter(|a| needs_stamp(a))
                .take(MAX_STAMPS_PER_MESSAGE)
                .collect();
            jobs.push((strangers, main.message_id.clone()));
        }
        for (recipient, copy) in &out.bcc_copies {
            let list = if needs_stamp(&recipient.address) {
                vec![recipient.address.clone()]
            } else {
                Vec::new()
            };
            jobs.push((list, copy.message_id.clone()));
        }
        if jobs.iter().all(|(r, _)| r.is_empty()) {
            return;
        }
        // CPU work off the async runtime.
        let minted = tokio::task::spawn_blocking(move || {
            jobs.into_iter()
                .map(|(recipients, id)| {
                    recipients
                        .iter()
                        .filter_map(|r| {
                            spam::mint_postage(r, &id, spam::POSTAGE_BITS, spam::POSTAGE_MAX_ITERS)
                        })
                        .collect::<Vec<String>>()
                })
                .collect::<Vec<Vec<String>>>()
        })
        .await
        .unwrap_or_default();
        let mut it = minted.into_iter();
        if let Some(main) = &mut out.main {
            if let Some(labels) = it.next() {
                push_labels(main, labels);
            }
        }
        for (_, copy) in &mut out.bcc_copies {
            if let Some(labels) = it.next() {
                push_labels(copy, labels);
            }
        }
    }

    /// Delivers already-built copies of one message: the To+CC copy to its
    /// participant group and each BCC copy to a sender+recipient group,
    /// then files our Sent record. Returns the message id (hex).
    ///
    /// This is the delivery half of [`Mail::send`], exposed for callers
    /// that must adjust the built `MailMessage` before it leaves — the
    /// external mail gateway sets `origin = EXTERNAL_GATEWAY` and fills
    /// `external` (see `docs/MAIL_GATEWAY.md`); `Draft::build_all` always
    /// produces native messages and that is the right default for every
    /// other caller. The messages are re-validated here, so a caller cannot
    /// bypass the bounds in `hashgram_app::mail::validate`.
    pub async fn send_built(
        &mut self,
        main: Option<app::MailMessage>,
        bcc_copies: Vec<(app::MailAddress, app::MailMessage)>,
    ) -> Result<String, SdkError> {
        if let Some(main) = &main {
            m::validate(main)?;
        }
        for (_, copy) in &bcc_copies {
            m::validate(copy)?;
        }
        let out = m::Outgoing { main, bcc_copies };
        let me = self.one.account.address().to_owned();
        let mut sent_record: Option<MailRecord> = None;
        let mut errors = Vec::new();
        if let Some(main) = &out.main {
            let participants: Vec<String> = m::participant_set(main)
                .into_iter()
                .filter(|a| *a != me)
                .collect();
            match self.one.conversation_group(&participants).await {
                Ok(gid) => {
                    match self
                        .one
                        .send_app(&gid, app::app_message::Body::Mail(main.clone()))
                        .await
                    {
                        Ok(_) => {
                            sent_record = Some(self.record_outgoing(main, &hex::encode(&gid)));
                        }
                        Err(e) => errors.push(format!("main copy: {e}")),
                    }
                }
                Err(e) => errors.push(format!("main copy: {e}")),
            }
        }
        for (recipient, copy) in &out.bcc_copies {
            match self
                .one
                .conversation_group(std::slice::from_ref(&recipient.address))
                .await
            {
                Ok(gid) => {
                    if let Err(e) = self
                        .one
                        .send_app(&gid, app::app_message::Body::Mail(copy.clone()))
                        .await
                    {
                        errors.push(format!("bcc {}: {e}", recipient.address));
                    } else if sent_record.is_none() {
                        let mut r = self.record_outgoing(copy, &hex::encode(&gid));
                        r.message.bcc_copy = false;
                        sent_record = Some(r);
                    }
                }
                Err(e) => errors.push(format!("bcc {}: {e}", recipient.address)),
            }
        }
        let Some(mut rec) = sent_record else {
            return Err(SdkError::Delivery(errors.join("; ")));
        };
        if !errors.is_empty() {
            warn!(count = errors.len(), "some mail copies were not delivered");
            rec.labels.push("partial-delivery".into());
        }
        // Keep the BCC list on our own Sent copy only.
        if self.one.mail_state.settings.keep_sent {
            let id = hex::encode(&rec.message.message_id);
            self.put(&id, &rec)?;
        }
        Ok(hex::encode(&rec.message.message_id))
    }

    fn record_outgoing(&self, msg: &app::MailMessage, gid: &str) -> MailRecord {
        MailRecord {
            message: msg.clone(),
            folder: folder::SENT.into(),
            read: true,
            starred: false,
            labels: Vec::new(),
            received_at_ms: hashgram_app::ids::now_ms(),
            authenticated_sender: self.one.account.address().to_owned(),
            group_id: gid.to_owned(),
            delivered_to: BTreeMap::new(),
            read_by: BTreeMap::new(),
            outgoing: true,
            trust_score: 0,
        }
    }

    /// Reply (sender only) draft skeleton for a stored message.
    pub fn reply_draft(&self, id: &str, all: bool) -> Result<m::Draft, SdkError> {
        let rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        let me = self.one.account.address();
        let r = if all {
            m::reply_all_recipients(&rec.message, me)
        } else {
            m::reply_recipients(&rec.message)
        };
        let subject =
            if m::normalised_subject(&rec.message.subject) == rec.message.subject.to_lowercase() {
                format!("Re: {}", rec.message.subject)
            } else {
                rec.message.subject.clone()
            };
        Ok(m::Draft {
            to: r.to,
            cc: r.cc,
            subject,
            in_reply_to: Some(m::ReplyTarget {
                message_id: rec.message.message_id.clone(),
                thread_id: rec.message.thread_id.clone(),
                references: rec.message.references.clone(),
            }),
            ..Default::default()
        })
    }

    /// Forward draft skeleton.
    pub fn forward_draft(&self, id: &str) -> Result<m::Draft, SdkError> {
        let rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        Ok(m::forward_draft(app::MailAddress::default(), &rec.message))
    }

    // -----------------------------------------------------------------------
    // Drafts
    // -----------------------------------------------------------------------

    /// Saves a draft under a client-chosen id.
    pub fn save_draft(&mut self, id: &str, draft: &DraftRecord) -> Result<(), SdkError> {
        self.one.store.put(NS_DRAFT, id.as_bytes(), draft)
    }
    /// Lists drafts.
    pub fn drafts(&self) -> Result<Vec<(String, DraftRecord)>, SdkError> {
        Ok(self
            .one
            .store
            .scan::<DraftRecord>(NS_DRAFT)?
            .into_iter()
            .map(|(k, v)| (String::from_utf8_lossy(&k).into_owned(), v))
            .collect())
    }
    /// Deletes a draft.
    pub fn delete_draft(&mut self, id: &str) -> Result<bool, SdkError> {
        self.one.store.delete(NS_DRAFT, id.as_bytes())
    }

    // -----------------------------------------------------------------------
    // Receiving
    // -----------------------------------------------------------------------

    /// Handles an incoming application message that concerns Mail.
    /// Returns the stored record when a new message was filed.
    pub(crate) async fn handle_incoming(
        &mut self,
        r: &Received,
        appmsg: &app::AppMessage,
    ) -> Result<Option<MailRecord>, SdkError> {
        use app::app_message::Body as B;
        match &appmsg.body {
            Some(B::Mail(msg)) => self.receive_mail(r, msg).await,
            Some(B::MailReceipt(rc)) => {
                self.receive_receipt(&r.sender, rc)?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn receive_mail(
        &mut self,
        r: &Received,
        msg: &app::MailMessage,
    ) -> Result<Option<MailRecord>, SdkError> {
        m::validate(msg)?;
        let id = hex::encode(&msg.message_id);
        if self.one.mail_state.index.contains_key(&id) {
            // Another device's copy or a second store node: already filed.
            return Ok(None);
        }
        let claimed = msg.from.as_ref().map(|a| a.address.as_str()).unwrap_or("");
        if claimed != r.sender && msg.origin != app::MailOrigin::ExternalGateway as i32 {
            // A member impersonating another address; we keep the message
            // but the authenticated sender is what we show, and it is spam.
            warn!("mail claims a from address that is not the MLS sender");
        }
        let me = self.one.account.address().to_owned();
        if r.sender == me {
            // Our own copy from another device: Sent.
            let mut rec = self.record_outgoing(msg, &r.group_id);
            rec.authenticated_sender = me;
            self.put(&id, &rec)?;
            return Ok(Some(rec));
        }
        let mut facts = self.one.people_state.contacts.sender_facts(&r.sender);
        if !facts.has_username {
            // The chain knows whether this stranger paid for a name.
            facts.has_username = !self
                .one
                .people()
                .username_of(&r.sender)
                .await
                .unwrap_or_default()
                .is_empty();
        }
        facts.previously_written_to = self
            .one
            .mail_state
            .by_folder
            .get(folder::SENT)
            .map(|v| {
                v.iter().any(|(_, sid)| {
                    self.get(sid)
                        .ok()
                        .flatten()
                        .map(|rec| {
                            rec.message
                                .to
                                .iter()
                                .chain(rec.message.cc.iter())
                                .any(|a| a.address == r.sender)
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        facts.prior_messages = self
            .one
            .mail_state
            .by_folder
            .get(folder::INBOX)
            .map(|v| v.len().min(100) as u32)
            .unwrap_or(0);
        if claimed != r.sender && msg.origin != app::MailOrigin::ExternalGateway as i32 {
            // Impersonation of another address is dropped outright.
            facts.is_blocked = true;
        }
        // Work the sender spent addressing *us* specifically.
        facts.postage_bits = spam::postage_bits(msg, &me);
        let score = spam::trust_score(&facts, msg);
        let now = hashgram_app::ids::now_secs();
        let disposition = spam::dispose(&facts, msg, &mut self.one.mail_state.window, now);
        self.one
            .store
            .put(NS_META, b"rate_window", &self.one.mail_state.window)?;
        let folder_name = match disposition {
            spam::Disposition::Drop => return Ok(None),
            spam::Disposition::Inbox => folder::INBOX,
            spam::Disposition::Requests => folder::REQUESTS,
            spam::Disposition::Spam => folder::SPAM,
        };
        let rec = MailRecord {
            message: msg.clone(),
            folder: folder_name.into(),
            read: false,
            starred: false,
            labels: Vec::new(),
            received_at_ms: hashgram_app::ids::now_ms(),
            authenticated_sender: r.sender.clone(),
            group_id: r.group_id.clone(),
            delivered_to: BTreeMap::new(),
            read_by: BTreeMap::new(),
            outgoing: false,
            trust_score: score,
        };
        self.put(&id, &rec)?;
        // Delivery receipt back into the same group (only for non-spam).
        if disposition != spam::Disposition::Spam {
            if let Ok(gid) = hex::decode(&r.group_id) {
                let receipt = app::MailReceipt {
                    version: hashgram_app::version::MAIL_VERSION,
                    message_id: msg.message_id.clone(),
                    kind: app::MailReceiptKind::Delivered as i32,
                    at_ms: hashgram_app::ids::now_ms(),
                };
                if let Err(e) = self
                    .one
                    .send_app(&gid, app::app_message::Body::MailReceipt(receipt))
                    .await
                {
                    debug!(error = %e, "delivery receipt not sent");
                }
            }
        }
        Ok(Some(rec))
    }

    fn receive_receipt(&mut self, sender: &str, rc: &app::MailReceipt) -> Result<(), SdkError> {
        let id = hex::encode(&rc.message_id);
        let Some(mut rec) = self.get(&id)? else {
            return Ok(());
        };
        if !rec.outgoing {
            return Ok(());
        }
        match app::MailReceiptKind::try_from(rc.kind) {
            Ok(app::MailReceiptKind::Delivered) => {
                rec.delivered_to.insert(sender.to_owned(), rc.at_ms);
            }
            Ok(app::MailReceiptKind::Read) => {
                rec.read_by.insert(sender.to_owned(), rc.at_ms);
            }
            _ => return Ok(()),
        }
        self.put(&id, &rec)
    }

    /// Applies a flag hint from another of our devices.
    pub(crate) fn apply_state_hint(&mut self, h: &app::MailStateHint) -> Result<(), SdkError> {
        type Apply = fn(&mut MailRecord);
        let sets: [(&Vec<Vec<u8>>, Apply); 7] = [
            (&h.read, |r| r.read = true),
            (&h.unread, |r| r.read = false),
            (&h.archived, |r| r.folder = folder::ARCHIVE.into()),
            (&h.trashed, |r| r.folder = folder::TRASH.into()),
            (&h.starred, |r| r.starred = true),
            (&h.unstarred, |r| r.starred = false),
            (&h.deleted, |r| r.folder = "__deleted".into()),
        ];
        for (ids, f) in sets {
            for id in ids {
                let id = hex::encode(id);
                if let Some(mut rec) = self.get(&id)? {
                    f(&mut rec);
                    if rec.folder == "__deleted" {
                        self.remove(&id)?;
                    } else {
                        self.put(&id, &rec)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// The pending hint for other devices, and clears it.
    pub(crate) fn take_pending_hint(&mut self) -> Option<app::MailStateHint> {
        let h = std::mem::take(&mut self.one.mail_state.pending_hint);
        let empty = h.read.is_empty()
            && h.unread.is_empty()
            && h.archived.is_empty()
            && h.trashed.is_empty()
            && h.starred.is_empty()
            && h.unstarred.is_empty()
            && h.deleted.is_empty();
        if empty {
            None
        } else {
            Some(app::MailStateHint {
                at_ms: hashgram_app::ids::now_ms(),
                ..h
            })
        }
    }

    // -----------------------------------------------------------------------
    // Local mailbox
    // -----------------------------------------------------------------------

    fn put(&mut self, id: &str, rec: &MailRecord) -> Result<(), SdkError> {
        let key = hex::decode(id).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.one.store.put(NS_MSG, &key, rec)?;
        self.one.mail_state.unindex(id);
        self.one
            .mail_state
            .insert_sorted(&rec.folder, rec.received_at_ms, id);
        let thread = hex::encode(&rec.message.thread_id);
        self.one
            .mail_state
            .by_thread
            .entry(thread.clone())
            .or_default()
            .push(id.to_owned());
        self.one.mail_state.index.insert(
            id.to_owned(),
            (rec.folder.clone(), rec.read, rec.starred, thread),
        );
        Ok(())
    }

    fn remove(&mut self, id: &str) -> Result<(), SdkError> {
        let key = hex::decode(id).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.one.store.delete(NS_MSG, &key)?;
        self.one.mail_state.unindex(id);
        Ok(())
    }

    /// One message with its local state.
    pub fn get(&self, id: &str) -> Result<Option<MailRecord>, SdkError> {
        let key = hex::decode(id).map_err(|e| SdkError::Invalid(e.to_string()))?;
        self.one.store.get(NS_MSG, &key)
    }

    /// A page of a folder, newest first, before `before_ms` (0 = now).
    pub fn list(
        &self,
        folder_name: &str,
        before_ms: u64,
        limit: usize,
    ) -> Result<Vec<MailSummary>, SdkError> {
        let Some(v) = self.one.mail_state.by_folder.get(folder_name) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(limit.min(v.len()));
        for (at, id) in v.iter().rev() {
            if before_ms != 0 && *at >= before_ms {
                continue;
            }
            if let Some(rec) = self.get(id)? {
                out.push(summary(id, &rec));
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Counts per folder.
    pub fn counts(&self) -> BTreeMap<String, FolderCounts> {
        let mut out: BTreeMap<String, FolderCounts> = BTreeMap::new();
        for f in folder::ALL {
            out.insert(f.into(), FolderCounts::default());
        }
        for (folder_name, read, _, _) in self.one.mail_state.index.values() {
            let c = out.entry(folder_name.clone()).or_default();
            c.total += 1;
            if !read {
                c.unread += 1;
            }
        }
        out
    }

    /// A whole thread.
    pub fn thread(&self, thread_id: &str) -> Result<Option<Thread>, SdkError> {
        let Some(ids) = self.one.mail_state.by_thread.get(thread_id) else {
            return Ok(None);
        };
        let mut msgs = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(r) = self.get(id)? {
                if r.folder != folder::TRASH && r.folder != folder::SPAM {
                    msgs.push(r);
                }
            }
        }
        msgs.sort_by_key(|r| (r.message.created_at_ms, r.message.message_id.clone()));
        let mut participants: BTreeSet<String> = BTreeSet::new();
        for r in &msgs {
            participants.insert(r.authenticated_sender.clone());
            for a in r.message.to.iter().chain(r.message.cc.iter()) {
                participants.insert(a.address.clone());
            }
        }
        Ok(Some(Thread {
            id: thread_id.to_owned(),
            subject: msgs
                .first()
                .map(|r| r.message.subject.clone())
                .unwrap_or_default(),
            unread: msgs.iter().filter(|r| !r.read).count(),
            participants: participants.into_iter().collect(),
            messages: msgs,
        }))
    }

    /// Threads in a folder, newest activity first: one summary per thread.
    pub fn threads(
        &self,
        folder_name: &str,
        before_ms: u64,
        limit: usize,
    ) -> Result<Vec<MailSummary>, SdkError> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for s in self.list(folder_name, before_ms, limit * 4)? {
            if seen.insert(s.thread_id.clone()) {
                out.push(s);
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    fn set_flag(
        &mut self,
        id: &str,
        f: impl Fn(&mut MailRecord),
        hint: impl Fn(&mut app::MailStateHint, Vec<u8>),
    ) -> Result<(), SdkError> {
        let mut rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        f(&mut rec);
        self.put(id, &rec)?;
        hint(
            &mut self.one.mail_state.pending_hint,
            rec.message.message_id.clone(),
        );
        Ok(())
    }

    /// Marks read; sends a read receipt when the sender asked and settings
    /// allow.
    pub async fn mark_read(&mut self, id: &str, read: bool) -> Result<(), SdkError> {
        let rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        let was_unread = !rec.read;
        if read {
            self.set_flag(id, |r| r.read = true, |h, i| h.read.push(i))?;
        } else {
            self.set_flag(id, |r| r.read = false, |h, i| h.unread.push(i))?;
        }
        if read
            && was_unread
            && !rec.outgoing
            && rec.message.request_read_receipt
            && self.one.mail_state.settings.send_read_receipts
        {
            if let Ok(gid) = hex::decode(&rec.group_id) {
                let receipt = app::MailReceipt {
                    version: hashgram_app::version::MAIL_VERSION,
                    message_id: rec.message.message_id.clone(),
                    kind: app::MailReceiptKind::Read as i32,
                    at_ms: hashgram_app::ids::now_ms(),
                };
                let _ = self
                    .one
                    .send_app(&gid, app::app_message::Body::MailReceipt(receipt))
                    .await;
            }
        }
        Ok(())
    }

    /// Star / unstar.
    pub fn star(&mut self, id: &str, starred: bool) -> Result<(), SdkError> {
        if starred {
            self.set_flag(id, |r| r.starred = true, |h, i| h.starred.push(i))
        } else {
            self.set_flag(id, |r| r.starred = false, |h, i| h.unstarred.push(i))
        }
    }

    /// Moves to a folder.
    pub fn move_to(&mut self, id: &str, folder_name: &str) -> Result<(), SdkError> {
        if !folder::ALL.contains(&folder_name) {
            return Err(SdkError::Invalid(format!("unknown folder {folder_name}")));
        }
        let f = folder_name.to_owned();
        let target = folder_name;
        self.set_flag(
            id,
            move |r| r.folder = f.clone(),
            move |h, i| match target {
                folder::ARCHIVE => h.archived.push(i),
                folder::TRASH => h.trashed.push(i),
                _ => {}
            },
        )
    }

    /// Archive.
    pub fn archive(&mut self, id: &str) -> Result<(), SdkError> {
        self.move_to(id, folder::ARCHIVE)
    }

    /// Trash (or permanently delete if already in trash).
    pub fn trash(&mut self, id: &str) -> Result<(), SdkError> {
        let rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        if rec.folder == folder::TRASH {
            return self.delete(id);
        }
        self.move_to(id, folder::TRASH)
    }

    /// Permanently deletes.
    pub fn delete(&mut self, id: &str) -> Result<(), SdkError> {
        let rec = self
            .get(id)?
            .ok_or_else(|| SdkError::NotFound(format!("mail {id}")))?;
        self.one
            .mail_state
            .pending_hint
            .deleted
            .push(rec.message.message_id.clone());
        self.remove(id)
    }

    /// Accepts a Requests message: moves it to Inbox and marks the sender
    /// as previously-written-to by adding them as a contact request
    /// acceptance path is up to the caller; here we only file it.
    pub fn accept_request(&mut self, id: &str) -> Result<(), SdkError> {
        self.move_to(id, folder::INBOX)
    }

    /// Adds / removes a label.
    pub fn set_label(&mut self, id: &str, label: &str, on: bool) -> Result<(), SdkError> {
        if label.is_empty() || label.len() > 64 {
            return Err(SdkError::Invalid("label length".into()));
        }
        let l = label.to_owned();
        self.set_flag(
            id,
            move |r| {
                r.labels.retain(|x| x != &l);
                if on {
                    r.labels.push(l.clone());
                }
            },
            |_, _| {},
        )
    }

    /// Messages carrying a label, newest first.
    pub fn by_label(&self, label: &str, limit: usize) -> Result<Vec<MailSummary>, SdkError> {
        let mut out = Vec::new();
        let mut all: Vec<(u64, String)> = self
            .one
            .mail_state
            .by_folder
            .values()
            .flatten()
            .cloned()
            .collect();
        all.sort();
        for (_, id) in all.iter().rev() {
            if let Some(r) = self.get(id)? {
                if r.labels.iter().any(|l| l == label) {
                    out.push(summary(id, &r));
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    /// Starred messages.
    pub fn starred(&self, limit: usize) -> Result<Vec<MailSummary>, SdkError> {
        let mut ids: Vec<(u64, String)> = Vec::new();
        for (id, (_, _, starred, _)) in &self.one.mail_state.index {
            if *starred {
                if let Some(r) = self.get(id)? {
                    ids.push((r.received_at_ms, id.clone()));
                }
            }
        }
        ids.sort();
        let mut out = Vec::new();
        for (_, id) in ids.iter().rev().take(limit) {
            if let Some(r) = self.get(id)? {
                out.push(summary(id, &r));
            }
        }
        Ok(out)
    }

    /// Local full-text search over subject, body and participants
    /// (case-insensitive substring; the desktop keeps its own index for
    /// large mailboxes). Bounded scan.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MailSummary>, SdkError> {
        let q = query.to_lowercase();
        if q.len() < 2 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut all: Vec<(u64, String)> = self
            .one
            .mail_state
            .by_folder
            .values()
            .flatten()
            .cloned()
            .collect();
        all.sort();
        for (_, id) in all.iter().rev() {
            if let Some(r) = self.get(id)? {
                let hit = r.message.subject.to_lowercase().contains(&q)
                    || r.message.body_text.to_lowercase().contains(&q)
                    || r.authenticated_sender.contains(&q)
                    || r.message
                        .from
                        .as_ref()
                        .map(|a| a.username.to_lowercase().contains(&q))
                        .unwrap_or(false)
                    || r.message
                        .to
                        .iter()
                        .any(|a| a.address.contains(&q) || a.username.to_lowercase().contains(&q))
                    || r.message
                        .attachments
                        .iter()
                        .any(|a| a.name.to_lowercase().contains(&q));
                if hit {
                    out.push(summary(id, &r));
                    if out.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    /// Purges trash/spam older than the configured retention and expired
    /// messages. Returns how many were removed.
    pub fn purge(&mut self) -> Result<usize, SdkError> {
        let days = self.one.mail_state.settings.purge_after_days;
        let now = hashgram_app::ids::now_ms();
        let mut victims = Vec::new();
        for f in [folder::TRASH, folder::SPAM] {
            if days == 0 {
                continue;
            }
            for (at, id) in self
                .one
                .mail_state
                .by_folder
                .get(f)
                .cloned()
                .unwrap_or_default()
            {
                if now.saturating_sub(at) > u64::from(days) * 86_400_000 {
                    victims.push(id);
                }
            }
        }
        // Expiring messages: expire_after_secs after first read.
        for (id, (_, read, _, _)) in self.one.mail_state.index.clone() {
            if !read {
                continue;
            }
            if let Some(r) = self.get(&id)? {
                if r.message.expire_after_secs > 0
                    && now.saturating_sub(r.received_at_ms)
                        > u64::from(r.message.expire_after_secs) * 1000
                {
                    victims.push(id);
                }
            }
        }
        victims.sort();
        victims.dedup();
        for id in &victims {
            self.remove(id)?;
        }
        Ok(victims.len())
    }

    /// Settings.
    pub fn settings(&self) -> &MailSettings {
        &self.one.mail_state.settings
    }
    /// Updates settings.
    pub fn set_settings(&mut self, s: MailSettings) -> Result<(), SdkError> {
        self.one.store.put(NS_META, b"settings", &s)?;
        self.one.mail_state.settings = s;
        Ok(())
    }

    /// Decrypts an inline or blob attachment to bytes (Drive capabilities
    /// go through `drive().download_capability`).
    pub async fn attachment_bytes(&mut self, a: &app::MailAttachment) -> Result<Vec<u8>, SdkError> {
        match &a.source {
            Some(app::mail_attachment::Source::InlineData(d)) => Ok(d.clone()),
            Some(app::mail_attachment::Source::Blob(b)) => {
                let (ct, _m, _p) = crate::blob::download(&self.one.link, &b.cid, None).await?;
                let mut key = [0u8; 32];
                let mut nonce = [0u8; 24];
                if b.key.len() != 32 || b.nonce.len() != 24 {
                    return Err(SdkError::Invalid("attachment key".into()));
                }
                key.copy_from_slice(&b.key);
                nonce.copy_from_slice(&b.nonce);
                let pt = crate::blob::decrypt_private(&ct, &crate::blob::FileKey { key, nonce })?;
                if !b.plaintext_hash.is_empty()
                    && blake3::hash(&pt).as_bytes() != b.plaintext_hash.as_slice()
                {
                    return Err(SdkError::Corrupt(
                        "attachment plaintext hash mismatch".into(),
                    ));
                }
                Ok(pt)
            }
            Some(app::mail_attachment::Source::Drive(cap)) => {
                self.one.drive().download_capability(cap).await
            }
            None => Err(SdkError::Invalid("attachment has no source".into())),
        }
    }

    /// Builds an attachment from bytes: inline when small, otherwise an
    /// encrypted blob uploaded to store nodes.
    pub async fn make_attachment(
        &mut self,
        name: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<app::MailAttachment, SdkError> {
        let hash = blake3::hash(bytes).as_bytes().to_vec();
        if bytes.len() <= m::MAX_INLINE_ATTACHMENT {
            return Ok(app::MailAttachment {
                name: name.into(),
                mime: mime.into(),
                size: bytes.len() as u64,
                plaintext_hash: hash,
                content_id: String::new(),
                source: Some(app::mail_attachment::Source::InlineData(bytes.to_vec())),
            });
        }
        let device = self.one.account.device()?;
        let up = crate::blob::upload(
            &self.one.link,
            &self.one.network,
            &device,
            bytes,
            mime,
            true,
            2,
        )
        .await?;
        let key = hex::decode(up.key.unwrap_or_default()).unwrap_or_default();
        let nonce = hex::decode(up.nonce.unwrap_or_default()).unwrap_or_default();
        Ok(app::MailAttachment {
            name: name.into(),
            mime: mime.into(),
            size: bytes.len() as u64,
            plaintext_hash: hash,
            content_id: String::new(),
            source: Some(app::mail_attachment::Source::Blob(app::BlobRef {
                cid: hex::decode(&up.cid).unwrap_or_default(),
                key,
                nonce,
                mime: mime.into(),
                size: bytes.len() as u64,
                name: name.into(),
                kind: "file".into(),
                ..Default::default()
            })),
        })
    }

    /// Builds a Drive-backed attachment from an existing Drive entry.
    pub async fn attach_from_drive(
        &mut self,
        entry_id: &str,
        recipients: &[String],
        live: bool,
    ) -> Result<app::MailAttachment, SdkError> {
        let cap = self
            .one
            .drive()
            .share(
                entry_id,
                &recipients.join(","),
                if live {
                    app::DriveShareMode::Live
                } else {
                    app::DriveShareMode::Snapshot
                },
                app::DrivePermission::Read,
                "",
            )
            .await?;
        Ok(app::MailAttachment {
            name: cap.name.clone(),
            mime: cap.mime.clone(),
            size: cap.size,
            plaintext_hash: cap
                .object
                .as_ref()
                .map(|o| o.plaintext_hash.clone())
                .unwrap_or_default(),
            content_id: String::new(),
            source: Some(app::mail_attachment::Source::Drive(cap)),
        })
    }
}

/// A draft as the client stores it (the composer's state).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DraftRecord {
    /// Recipients as typed.
    pub to: Vec<String>,
    /// CC.
    pub cc: Vec<String>,
    /// BCC.
    pub bcc: Vec<String>,
    /// Subject.
    pub subject: String,
    /// Body.
    pub body_text: String,
    /// HTML body.
    pub body_html: String,
    /// Attachments already prepared.
    pub attachments: Vec<app::MailAttachment>,
    /// Reply target (hex message id).
    pub in_reply_to: String,
    /// Last edit.
    pub updated_at_ms: u64,
}

fn summary(id: &str, r: &MailRecord) -> MailSummary {
    let preview: String = r.message.body_text.chars().take(160).collect();
    MailSummary {
        id: id.to_owned(),
        thread_id: hex::encode(&r.message.thread_id),
        folder: r.folder.clone(),
        from: r.authenticated_sender.clone(),
        from_username: r
            .message
            .from
            .as_ref()
            .map(|a| a.username.clone())
            .unwrap_or_default(),
        to: r.message.to.iter().map(|a| a.address.clone()).collect(),
        subject: r.message.subject.clone(),
        preview,
        created_at_ms: r.message.created_at_ms,
        received_at_ms: r.received_at_ms,
        read: r.read,
        starred: r.starred,
        attachments: r.message.attachments.len(),
        labels: r.labels.clone(),
        external: r.message.origin == app::MailOrigin::ExternalGateway as i32,
        bcc_copy: r.message.bcc_copy,
        outgoing: r.outgoing,
    }
}

impl From<AppError> for SdkError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Unsupported(s) => SdkError::Unsupported(s),
            AppError::NotFound(s) => SdkError::NotFound(s),
            AppError::Integrity(s) => SdkError::Corrupt(s),
            other => SdkError::Invalid(other.to_string()),
        }
    }
}
