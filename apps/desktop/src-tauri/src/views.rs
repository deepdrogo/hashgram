//! What the webview is allowed to see.
//!
//! SDK records carry key material in places (a `BlobRef` has the blob key
//! and nonce, a `DriveCapability`/`DriveObjectRef` the object key). None of
//! it may cross into JavaScript. Every command that returns mail, Drive,
//! Space or Circle content maps it through these views, which keep ids,
//! names, sizes and hashes and drop keys. A test greps the serialised shape
//! for forbidden field names.

use std::collections::BTreeMap;

use hashgram_sdk::drive::SharedWithMe;
use hashgram_sdk::mail::{DraftRecord, MailRecord, Thread};
use hashgram_sdk::protocol::circle::Item as CircleItem;
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::protocol::space::{Content, Member, SharedEntry, State};
use serde::Serialize;

/// A mail participant.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AddressView {
    /// Address.
    pub address: String,
    /// Username hint.
    pub username: String,
    /// Display name hint.
    pub display_name: String,
}

impl From<&app::MailAddress> for AddressView {
    fn from(a: &app::MailAddress) -> Self {
        Self {
            address: a.address.clone(),
            username: a.username.clone(),
            display_name: a.display_name.clone(),
        }
    }
}

/// Media without its key.
#[derive(Debug, Clone, Serialize)]
pub struct MediaView {
    /// Hex CID.
    pub cid: String,
    /// MIME.
    pub mime: String,
    /// Size.
    pub size: u64,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl From<&app::BlobRef> for MediaView {
    fn from(b: &app::BlobRef) -> Self {
        Self {
            cid: hex::encode(&b.cid),
            mime: b.mime.clone(),
            size: b.size,
            name: b.name.clone(),
            kind: b.kind.clone(),
            width: b.width,
            height: b.height,
        }
    }
}

/// A Drive capability without its object key.
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityView {
    /// Hex share id.
    pub share_id: String,
    /// Owner address.
    pub owner: String,
    /// Owner's entry id (hex).
    pub entry_id: String,
    /// Name.
    pub name: String,
    /// MIME.
    pub mime: String,
    /// Size.
    pub size: u64,
    /// `snapshot` | `live`.
    pub mode: &'static str,
    /// `read` | `write`.
    pub permission: &'static str,
    /// Owner's version number.
    pub version_no: u64,
    /// Granted at.
    pub granted_at_ms: u64,
    /// Folder share.
    pub folder: bool,
    /// Hex BLAKE3 of the plaintext.
    pub plaintext_hash: String,
}

impl From<&app::DriveCapability> for CapabilityView {
    fn from(c: &app::DriveCapability) -> Self {
        Self {
            share_id: hex::encode(&c.share_id),
            owner: c.owner.clone(),
            entry_id: hex::encode(&c.entry_id),
            name: c.name.clone(),
            mime: c.mime.clone(),
            size: c.size,
            mode: if c.mode == app::DriveShareMode::Live as i32 {
                "live"
            } else {
                "snapshot"
            },
            permission: if c.permission == app::DrivePermission::Write as i32 {
                "write"
            } else {
                "read"
            },
            version_no: c.version_no,
            granted_at_ms: c.granted_at_ms,
            folder: c.folder,
            plaintext_hash: c
                .object
                .as_ref()
                .map(|o| hex::encode(&o.plaintext_hash))
                .unwrap_or_default(),
        }
    }
}

/// A mail attachment without its key.
#[derive(Debug, Clone, Serialize)]
pub struct AttachmentView {
    /// Index inside the message.
    pub index: usize,
    /// Name.
    pub name: String,
    /// MIME.
    pub mime: String,
    /// Size.
    pub size: u64,
    /// `inline` | `blob` | `drive`.
    pub kind: &'static str,
    /// Live Drive attachment.
    pub live: bool,
    /// Drive share id (hex) when `kind == drive`.
    pub share_id: String,
    /// Owner's version number when `kind == drive`.
    pub version_no: u64,
    /// Folder share.
    pub folder: bool,
    /// Content-ID for inline HTML references.
    pub content_id: String,
    /// Hex BLAKE3 of the plaintext.
    pub plaintext_hash: String,
}

impl AttachmentView {
    /// Maps one attachment.
    #[must_use]
    pub fn of(index: usize, a: &app::MailAttachment) -> Self {
        let (kind, live, share_id, version_no, folder) = match &a.source {
            Some(app::mail_attachment::Source::InlineData(_)) => ("inline", false, String::new(), 0, false),
            Some(app::mail_attachment::Source::Blob(_)) => ("blob", false, String::new(), 0, false),
            Some(app::mail_attachment::Source::Drive(c)) => (
                "drive",
                c.mode == app::DriveShareMode::Live as i32,
                hex::encode(&c.share_id),
                c.version_no,
                c.folder,
            ),
            None => ("inline", false, String::new(), 0, false),
        };
        Self {
            index,
            name: a.name.clone(),
            mime: a.mime.clone(),
            size: a.size,
            kind,
            live,
            share_id,
            version_no,
            folder,
            content_id: a.content_id.clone(),
            plaintext_hash: hex::encode(&a.plaintext_hash),
        }
    }
}

/// What a gateway preserved from an Internet message.
#[derive(Debug, Clone, Serialize)]
pub struct ExternalView {
    /// Gateway operator address.
    pub gateway: String,
    /// Original From header.
    pub from_header: String,
    /// Original Message-ID.
    pub message_id_header: String,
    /// Verdicts.
    pub auth_results: Vec<String>,
    /// Spam score [0, 1000].
    pub spam_score: u32,
}

/// A full message for the reading pane.
#[derive(Debug, Clone, Serialize)]
pub struct MailView {
    /// Hex id.
    pub id: String,
    /// Hex thread id.
    pub thread_id: String,
    /// Folder.
    pub folder: String,
    /// The claimed From header (hints only).
    pub from: AddressView,
    /// The sender as MLS authenticated it. Display this.
    pub authenticated_sender: String,
    /// Whether the claimed From matches the authenticated sender.
    pub sender_matches: bool,
    /// To.
    pub to: Vec<AddressView>,
    /// CC.
    pub cc: Vec<AddressView>,
    /// This is a BCC copy.
    pub bcc_copy: bool,
    /// Sender clock.
    pub created_at_ms: u64,
    /// Local arrival.
    pub received_at_ms: u64,
    /// Subject.
    pub subject: String,
    /// Plain body.
    pub body_text: String,
    /// HTML body (rendered in a sandbox only).
    pub body_html: String,
    /// Attachments.
    pub attachments: Vec<AttachmentView>,
    /// Hex id replied to.
    pub in_reply_to: String,
    /// Hex ancestor ids.
    pub references: Vec<String>,
    /// Bridged from the Internet.
    pub external: Option<ExternalView>,
    /// Sender asked for a read receipt.
    pub request_read_receipt: bool,
    /// 0 normal, 1 low, 2 high.
    pub importance: i32,
    /// Sender-chosen labels.
    pub sender_labels: Vec<String>,
    /// Our labels.
    pub labels: Vec<String>,
    /// Read.
    pub read: bool,
    /// Starred.
    pub starred: bool,
    /// Delivered receipts (address → ms).
    pub delivered_to: BTreeMap<String, u64>,
    /// Read receipts.
    pub read_by: BTreeMap<String, u64>,
    /// Our own sent copy.
    pub outgoing: bool,
    /// Spam trust score at arrival.
    pub trust_score: i32,
    /// Advisory expiry.
    pub expire_after_secs: u32,
}

impl From<&MailRecord> for MailView {
    fn from(r: &MailRecord) -> Self {
        let m = &r.message;
        let from = m.from.as_ref().map(AddressView::from).unwrap_or_default();
        Self {
            id: hex::encode(&m.message_id),
            thread_id: hex::encode(&m.thread_id),
            folder: r.folder.clone(),
            sender_matches: from.address == r.authenticated_sender
                || m.origin == app::MailOrigin::ExternalGateway as i32,
            from,
            authenticated_sender: r.authenticated_sender.clone(),
            to: m.to.iter().map(AddressView::from).collect(),
            cc: m.cc.iter().map(AddressView::from).collect(),
            bcc_copy: m.bcc_copy,
            created_at_ms: m.created_at_ms,
            received_at_ms: r.received_at_ms,
            subject: m.subject.clone(),
            body_text: m.body_text.clone(),
            body_html: m.body_html.clone(),
            attachments: m
                .attachments
                .iter()
                .enumerate()
                .map(|(i, a)| AttachmentView::of(i, a))
                .collect(),
            in_reply_to: hex::encode(&m.in_reply_to),
            references: m.references.iter().map(hex::encode).collect(),
            external: if m.origin == app::MailOrigin::ExternalGateway as i32 {
                Some(match &m.external {
                    Some(x) => ExternalView {
                        gateway: x.gateway.clone(),
                        from_header: x.from_header.clone(),
                        message_id_header: x.message_id_header.clone(),
                        auth_results: x.auth_results.clone(),
                        spam_score: x.spam_score,
                    },
                    None => ExternalView {
                        gateway: String::new(),
                        from_header: String::new(),
                        message_id_header: String::new(),
                        auth_results: Vec::new(),
                        spam_score: 0,
                    },
                })
            } else {
                None
            },
            request_read_receipt: m.request_read_receipt,
            importance: m.importance,
            sender_labels: m.labels.clone(),
            labels: r.labels.clone(),
            read: r.read,
            starred: r.starred,
            delivered_to: r.delivered_to.clone(),
            read_by: r.read_by.clone(),
            outgoing: r.outgoing,
            trust_score: r.trust_score,
            expire_after_secs: m.expire_after_secs,
        }
    }
}

/// A thread for the reading pane.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadView {
    /// Hex id.
    pub id: String,
    /// Subject.
    pub subject: String,
    /// Messages oldest first.
    pub messages: Vec<MailView>,
    /// Participants.
    pub participants: Vec<String>,
    /// Unread.
    pub unread: usize,
}

impl From<&Thread> for ThreadView {
    fn from(t: &Thread) -> Self {
        Self {
            id: t.id.clone(),
            subject: t.subject.clone(),
            messages: t.messages.iter().map(MailView::from).collect(),
            participants: t.participants.clone(),
            unread: t.unread,
        }
    }
}

/// A draft as the composer sees it.
#[derive(Debug, Clone, Serialize)]
pub struct DraftView {
    /// Draft id.
    pub id: String,
    /// To, as typed.
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
    pub attachments: Vec<AttachmentView>,
    /// Hex id of the message being answered.
    pub in_reply_to: String,
    /// Last edit.
    pub updated_at_ms: u64,
}

impl DraftView {
    /// Maps a stored draft.
    #[must_use]
    pub fn of(id: &str, d: &DraftRecord) -> Self {
        Self {
            id: id.to_owned(),
            to: d.to.clone(),
            cc: d.cc.clone(),
            bcc: d.bcc.clone(),
            subject: d.subject.clone(),
            body_text: d.body_text.clone(),
            body_html: d.body_html.clone(),
            attachments: d
                .attachments
                .iter()
                .enumerate()
                .map(|(i, a)| AttachmentView::of(i, a))
                .collect(),
            in_reply_to: d.in_reply_to.clone(),
            updated_at_ms: d.updated_at_ms,
        }
    }
}

/// Shared-with-me row.
#[derive(Debug, Clone, Serialize)]
pub struct SharedWithMeView {
    /// The capability (no key).
    pub capability: CapabilityView,
    /// Owner.
    pub from: String,
    /// Group hex.
    pub group_id: String,
    /// Note.
    pub note: String,
    /// Received.
    pub received_at_ms: u64,
    /// Updates received (live).
    pub updates: u32,
    /// Revoked.
    pub revoked: bool,
}

impl From<&SharedWithMe> for SharedWithMeView {
    fn from(s: &SharedWithMe) -> Self {
        Self {
            capability: CapabilityView::from(&s.capability),
            from: s.from.clone(),
            group_id: s.group_id.clone(),
            note: s.note.clone(),
            received_at_ms: s.received_at_ms,
            updates: s.updates,
            revoked: s.revoked,
        }
    }
}

/// A share we granted.
#[derive(Debug, Clone, Serialize)]
pub struct ShareRecordView {
    /// Hex share id.
    pub share_id: String,
    /// Hex entry id.
    pub entry_id: String,
    /// Grantee address or `space:<hex>`.
    pub grantee: String,
    /// `snapshot` | `live`.
    pub mode: &'static str,
    /// `read` | `write`.
    pub permission: &'static str,
    /// Granted.
    pub granted_at_ms: u64,
    /// Revoked.
    pub revoked: bool,
    /// Revoked at.
    pub revoked_at_ms: u64,
}

impl From<&app::DriveShareRecord> for ShareRecordView {
    fn from(s: &app::DriveShareRecord) -> Self {
        Self {
            share_id: hex::encode(&s.share_id),
            entry_id: hex::encode(&s.entry_id),
            grantee: s.grantee.clone(),
            mode: if s.mode == app::DriveShareMode::Live as i32 {
                "live"
            } else {
                "snapshot"
            },
            permission: if s.permission == app::DrivePermission::Write as i32 {
                "write"
            } else {
                "read"
            },
            granted_at_ms: s.granted_at_ms,
            revoked: s.revoked,
            revoked_at_ms: s.revoked_at_ms,
        }
    }
}

/// A file version.
#[derive(Debug, Clone, Serialize)]
pub struct VersionView {
    /// Number.
    pub version_no: u64,
    /// Plaintext size.
    pub size: u64,
    /// Created.
    pub created_at_ms: u64,
    /// Device that wrote it (hex public key).
    pub device_pubkey: String,
    /// Note.
    pub note: String,
    /// Hex plaintext hash.
    pub plaintext_hash: String,
}

impl From<&app::DriveVersion> for VersionView {
    fn from(v: &app::DriveVersion) -> Self {
        Self {
            version_no: v.version_no,
            size: v.object.as_ref().map(|o| o.size).unwrap_or(0),
            created_at_ms: v.created_at_ms,
            device_pubkey: hex::encode(&v.device_pubkey),
            note: v.note.clone(),
            plaintext_hash: v
                .object
                .as_ref()
                .map(|o| hex::encode(&o.plaintext_hash))
                .unwrap_or_default(),
        }
    }
}

/// An entry of a shared folder manifest.
#[derive(Debug, Clone, Serialize)]
pub struct FolderEntryView {
    /// Hex id.
    pub id: String,
    /// Hex parent id.
    pub parent_id: String,
    /// `file` | `folder`.
    pub kind: &'static str,
    /// Name.
    pub name: String,
    /// MIME.
    pub mime: String,
    /// Size.
    pub size: u64,
    /// Modified.
    pub modified_at_ms: u64,
}

impl From<&app::DriveEntry> for FolderEntryView {
    fn from(e: &app::DriveEntry) -> Self {
        Self {
            id: hex::encode(&e.id),
            parent_id: hex::encode(&e.parent_id),
            kind: if e.kind == app::DriveEntryKind::Folder as i32 {
                "folder"
            } else {
                "file"
            },
            name: e.name.clone(),
            mime: e.mime.clone(),
            size: e.size,
            modified_at_ms: e.modified_at_ms,
        }
    }
}

/// Space content row.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceContentView {
    /// Hex id.
    pub id: String,
    /// Kind.
    pub kind: &'static str,
    /// Actor.
    pub actor: String,
    /// Time.
    pub at_ms: u64,
    /// Title.
    pub title: String,
    /// Text.
    pub text: String,
    /// Post id for comments.
    pub post_id: String,
    /// Drive refs.
    pub drive_refs: Vec<CapabilityView>,
    /// Media.
    pub media: Vec<MediaView>,
}

impl From<&Content> for SpaceContentView {
    fn from(c: &Content) -> Self {
        Self {
            id: c.id.clone(),
            kind: c.kind,
            actor: c.actor.clone(),
            at_ms: c.at_ms,
            title: c.title.clone(),
            text: c.text.clone(),
            post_id: c.post_id.clone(),
            drive_refs: c.drive_refs.iter().map(CapabilityView::from).collect(),
            media: c.media.iter().map(MediaView::from).collect(),
        }
    }
}

/// Space drive entry.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceSharedEntryView {
    /// Capability.
    pub capability: CapabilityView,
    /// Path inside the Space drive.
    pub path: String,
    /// Who shared it.
    pub by: String,
    /// When.
    pub at_ms: u64,
}

impl From<&SharedEntry> for SpaceSharedEntryView {
    fn from(s: &SharedEntry) -> Self {
        Self {
            capability: CapabilityView::from(&s.capability),
            path: s.path.clone(),
            by: s.by.clone(),
            at_ms: s.at_ms,
        }
    }
}

/// Replayed Space state for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct SpaceStateView {
    /// Hex id.
    pub space_id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Members sorted by rank.
    pub members: Vec<Member>,
    /// Our role (1 guest … 4 owner, 0 none).
    pub my_role: i32,
    /// Drive.
    pub drive: Vec<SpaceSharedEntryView>,
    /// Created.
    pub created_at_ms: u64,
    /// Content rows.
    pub content_count: usize,
}

impl SpaceStateView {
    /// Maps a state for `me`.
    #[must_use]
    pub fn of(s: &State, me: &str) -> Self {
        let mut drive: Vec<SpaceSharedEntryView> = s.drive.values().map(SpaceSharedEntryView::from).collect();
        drive.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.capability.name.cmp(&b.capability.name)));
        Self {
            space_id: s.space_id.clone(),
            name: s.name.clone(),
            description: s.description.clone(),
            members: s.members_sorted(),
            my_role: s.role_of(me) as i32,
            drive,
            created_at_ms: s.created_at_ms,
            content_count: s.content.len(),
        }
    }
}

/// A poll.
#[derive(Debug, Clone, Serialize)]
pub struct PollView {
    /// Question.
    pub question: String,
    /// Options.
    pub options: Vec<String>,
    /// Multiple choice.
    pub multiple_choice: bool,
    /// Closes at.
    pub closes_at_ms: u64,
}

/// A Circle post or comment.
#[derive(Debug, Clone, Serialize)]
pub struct CircleItemView {
    /// Hex id.
    pub id: String,
    /// Kind.
    pub kind: &'static str,
    /// Author.
    pub author: String,
    /// Time.
    pub at_ms: u64,
    /// Text.
    pub text: String,
    /// Parent post.
    pub post_id: String,
    /// Reply target.
    pub reply_to: String,
    /// Media.
    pub media: Vec<MediaView>,
    /// Drive refs.
    pub drive_refs: Vec<CapabilityView>,
    /// Poll.
    pub poll: Option<PollView>,
    /// Reactions.
    pub reactions: BTreeMap<String, u32>,
    /// Votes per option.
    pub votes: BTreeMap<u32, u32>,
    /// Deleted.
    pub deleted: bool,
}

impl From<&CircleItem> for CircleItemView {
    fn from(i: &CircleItem) -> Self {
        Self {
            id: i.id.clone(),
            kind: i.kind,
            author: i.author.clone(),
            at_ms: i.at_ms,
            text: i.text.clone(),
            post_id: i.post_id.clone(),
            reply_to: i.reply_to.clone(),
            media: i.media.iter().map(MediaView::from).collect(),
            drive_refs: i.drive_refs.iter().map(CapabilityView::from).collect(),
            poll: i.poll.as_ref().map(|p| PollView {
                question: p.question.clone(),
                options: p.options.clone(),
                multiple_choice: p.multiple_choice,
                closes_at_ms: p.closes_at_ms,
            }),
            reactions: i.reactions.clone(),
            votes: i.votes.clone(),
            deleted: i.deleted,
        }
    }
}

/// A contact's private card.
#[derive(Debug, Clone, Serialize)]
pub struct CardView {
    /// Display name.
    pub display_name: String,
    /// Bio.
    pub bio: String,
    /// Disclosed wallet address (empty = not disclosed).
    pub wallet_address: String,
    /// When.
    pub at_ms: u64,
}

impl From<&app::ProfileCard> for CardView {
    fn from(c: &app::ProfileCard) -> Self {
        Self {
            display_name: c.display_name.clone(),
            bio: c.bio.clone(),
            wallet_address: c.wallet_address.clone(),
            at_ms: c.at_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap() -> app::DriveCapability {
        app::DriveCapability {
            version: 1,
            share_id: vec![1; 16],
            owner: "hash1owner".into(),
            entry_id: vec![2; 16],
            name: "plan.pdf".into(),
            mime: "application/pdf".into(),
            size: 10,
            object: Some(app::DriveObjectRef {
                version: 1,
                cid: vec![3; 32],
                key: Some(app::DriveKey {
                    key: vec![0xAA; 32],
                    base_nonce: vec![0xBB; 24],
                }),
                size: 10,
                segment_size: 1,
                plaintext_hash: vec![4; 32],
            }),
            mode: app::DriveShareMode::Live as i32,
            permission: 0,
            version_no: 3,
            granted_at_ms: 1,
            folder: false,
        }
    }

    fn forbidden(json: &str) {
        let j = json.to_ascii_lowercase();
        for word in ["\"key\"", "nonce", "seed", "secret", "mnemonic", "base_nonce", "manifest_key"] {
            assert!(!j.contains(word), "{word} leaked: {json}");
        }
        assert!(!j.contains(&hex::encode([0xAAu8; 32])), "object key bytes leaked");
    }

    #[test]
    fn capability_view_drops_the_key() {
        let v = CapabilityView::from(&cap());
        let json = serde_json::to_string(&v).unwrap();
        forbidden(&json);
        assert_eq!(v.mode, "live");
        assert_eq!(v.share_id, hex::encode([1u8; 16]));
    }

    #[test]
    fn mail_view_drops_attachment_keys_and_flags_impersonation() {
        let m = app::MailMessage {
            version: 1,
            message_id: vec![9; 16],
            thread_id: vec![9; 16],
            from: Some(app::MailAddress {
                address: "hash1claimed".into(),
                username: "eve".into(),
                display_name: String::new(),
            }),
            subject: "s".into(),
            body_text: "b".into(),
            attachments: vec![
                app::MailAttachment {
                    name: "a.bin".into(),
                    mime: "application/octet-stream".into(),
                    size: 3,
                    plaintext_hash: vec![0; 32],
                    content_id: String::new(),
                    source: Some(app::mail_attachment::Source::Blob(app::BlobRef {
                        cid: vec![1; 32],
                        key: vec![0xAA; 32],
                        nonce: vec![0xBB; 24],
                        ..Default::default()
                    })),
                },
                app::MailAttachment {
                    name: "plan.pdf".into(),
                    mime: "application/pdf".into(),
                    size: 10,
                    plaintext_hash: vec![4; 32],
                    content_id: String::new(),
                    source: Some(app::mail_attachment::Source::Drive(cap())),
                },
            ],
            ..Default::default()
        };
        let rec = MailRecord {
            message: m,
            folder: "inbox".into(),
            read: false,
            starred: false,
            labels: vec![],
            received_at_ms: 5,
            authenticated_sender: "hash1real".into(),
            group_id: "aa".into(),
            delivered_to: BTreeMap::new(),
            read_by: BTreeMap::new(),
            outgoing: false,
            trust_score: 0,
        };
        let v = MailView::from(&rec);
        let json = serde_json::to_string(&v).unwrap();
        forbidden(&json);
        assert!(!v.sender_matches);
        assert_eq!(v.authenticated_sender, "hash1real");
        assert_eq!(v.attachments[1].kind, "drive");
        assert!(v.attachments[1].live);
        assert_eq!(v.attachments[0].kind, "blob");
    }
}
