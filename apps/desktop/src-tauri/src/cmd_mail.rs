//! Mail commands over `one.mail()`.
//!
//! Attachments never cross into the webview with their keys: a draft lives
//! in the SDK's encrypted store under a client-chosen id, files are
//! attached to it on the Rust side (`mail_attach_file` reads the file and
//! calls `make_attachment`), and `mail_send(draft_id)` builds the message
//! from the stored draft. The composer only ever holds text fields and
//! attachment metadata.

use std::collections::BTreeMap;
use std::sync::Arc;

use hashgram_sdk::mail::{folder, DraftRecord, FolderCounts, MailSettings, MailSummary};
use hashgram_sdk::protocol::mail::{self as m, AddressForm};
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::HashgramOne;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::views::{AttachmentView, DraftView, MailView, ThreadView};

type S<'a> = State<'a, Arc<AppState>>;

const MAX_LIST: usize = 500;

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(100).clamp(1, MAX_LIST)
}

/// Folder counters.
#[tauri::command]
pub async fn mail_counts(state: S<'_>) -> CmdResult<BTreeMap<String, FolderCounts>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().counts())
}

/// A page of a folder. `folder` may also be `starred` or `label:<name>`.
#[tauri::command]
pub async fn mail_list(
    state: S<'_>,
    folder: String,
    before_ms: Option<u64>,
    limit: Option<usize>,
    threaded: Option<bool>,
) -> CmdResult<Vec<MailSummary>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let limit = clamp_limit(limit);
    let before = before_ms.unwrap_or(0);
    if folder == "starred" {
        return Ok(one.mail().starred(limit)?);
    }
    if let Some(label) = folder.strip_prefix("label:") {
        return Ok(one.mail().by_label(label, limit)?);
    }
    if !folder::ALL.contains(&folder.as_str()) {
        return Err(UiError::invalid(format!("unknown folder {folder}")));
    }
    if threaded.unwrap_or(false) {
        Ok(one.mail().threads(&folder, before, limit)?)
    } else {
        Ok(one.mail().list(&folder, before, limit)?)
    }
}

/// A whole thread.
#[tauri::command]
pub async fn mail_thread(state: S<'_>, thread_id: String) -> CmdResult<Option<ThreadView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().thread(&thread_id)?.as_ref().map(ThreadView::from))
}

/// One message.
#[tauri::command]
pub async fn mail_get(state: S<'_>, id: String) -> CmdResult<Option<MailView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().get(&id)?.as_ref().map(MailView::from))
}

/// Full-text search (bounded scan in the SDK).
#[tauri::command]
pub async fn mail_search(state: S<'_>, q: String, limit: Option<usize>) -> CmdResult<Vec<MailSummary>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().search(&q, clamp_limit(limit))?)
}

// ---------------------------------------------------------------------------
// Flags and folders
// ---------------------------------------------------------------------------

/// Marks read/unread (sends a READ receipt when the sender asked and the
/// settings allow).
#[tauri::command]
pub async fn mail_mark_read(state: S<'_>, id: String, read: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().mark_read(&id, read).await?;
    Ok(())
}

/// Star / unstar.
#[tauri::command]
pub async fn mail_star(state: S<'_>, id: String, starred: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().star(&id, starred)?;
    Ok(())
}

/// Moves to a folder.
#[tauri::command]
pub async fn mail_move(state: S<'_>, id: String, folder: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().move_to(&id, &folder)?;
    Ok(())
}

/// Archive.
#[tauri::command]
pub async fn mail_archive(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().archive(&id)?;
    Ok(())
}

/// Trash (a second call on a trashed message deletes it for good).
#[tauri::command]
pub async fn mail_trash(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().trash(&id)?;
    Ok(())
}

/// Permanent delete.
#[tauri::command]
pub async fn mail_delete(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().delete(&id)?;
    Ok(())
}

/// Accepts a Requests message: moves it to the Inbox and, when the sender
/// is not blocked, records that we have written to them so their next
/// messages land in the Inbox directly.
#[tauri::command]
pub async fn mail_accept_request(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().accept_request(&id)?;
    Ok(())
}

/// Adds / removes a label.
#[tauri::command]
pub async fn mail_label(state: S<'_>, id: String, label: String, on: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().set_label(&id, label.trim(), on)?;
    Ok(())
}

/// Mail settings (receipts, retention).
#[tauri::command]
pub async fn mail_settings_get(state: S<'_>) -> CmdResult<MailSettings> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().settings().clone())
}

/// Updates mail settings.
#[tauri::command]
pub async fn mail_settings_set(state: S<'_>, settings: MailSettings) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.mail().set_settings(settings)?;
    Ok(())
}

/// Purges trash/spam past retention and expired messages. Returns count.
#[tauri::command]
pub async fn mail_purge(state: S<'_>) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().purge()?)
}

// ---------------------------------------------------------------------------
// Recipients
// ---------------------------------------------------------------------------

/// What a typed recipient resolved to.
#[derive(Debug, Clone, Serialize)]
pub struct RecipientResolution {
    /// As typed.
    pub input: String,
    /// `hashgram` | `external` | `invalid`.
    pub kind: &'static str,
    /// Address (hashgram) or e-mail (external).
    pub address: String,
    /// Username.
    pub username: String,
    /// Display name.
    pub display_name: String,
    /// The address has an on-chain identity with active devices.
    pub has_identity: bool,
    /// Active device count.
    pub devices: usize,
    /// Why it is invalid / cannot be sent to.
    pub error: String,
}

async fn resolve_one(one: &mut HashgramOne, input: &str, gateway_configured: bool) -> RecipientResolution {
    let mut r = RecipientResolution {
        input: input.to_owned(),
        kind: "invalid",
        address: String::new(),
        username: String::new(),
        display_name: String::new(),
        has_identity: false,
        devices: 0,
        error: String::new(),
    };
    match m::parse_address(input) {
        Ok(AddressForm::External(e)) => {
            r.kind = "external";
            r.address = e;
            if !gateway_configured {
                r.error =
                    "external e-mail needs a mail gateway; set one in Settings → Network".into();
            }
        }
        Ok(_) => match one.people().resolve(input).await {
            Ok(res) => {
                r.kind = "hashgram";
                r.address = res.address;
                r.username = res.username;
                r.display_name = res.display_name;
                r.has_identity = res.has_identity;
                r.devices = res.devices;
                if !res.has_identity {
                    r.error = "this identity has no device on chain yet; they must register (open Hashgram once with HASH for the fee)".into();
                }
            }
            Err(e) => {
                r.error = UiError::from(e).message;
            }
        },
        Err(e) => {
            r.error = e.to_string();
        }
    }
    r
}

/// Resolves typed recipients (addresses, `@names`, `name@hashgram.io`,
/// external e-mail) for the composer's chips.
#[tauri::command]
pub async fn mail_resolve_recipients(state: S<'_>, inputs: Vec<String>) -> CmdResult<Vec<RecipientResolution>> {
    let gateway = !state.settings.read().await.network.gateway_address.trim().is_empty();
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut out = Vec::with_capacity(inputs.len());
    for i in inputs.iter().take(m::MAX_RECIPIENTS) {
        out.push(resolve_one(one, i.trim(), gateway).await);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Drafts and sending
// ---------------------------------------------------------------------------

fn new_draft_id() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    format!("d{}", hex::encode(b))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn load_draft(one: &mut HashgramOne, id: &str) -> CmdResult<DraftRecord> {
    one.mail()
        .drafts()?
        .into_iter()
        .find(|(k, _)| k == id)
        .map(|(_, d)| d)
        .ok_or_else(|| UiError::not_found("draft"))
}

fn addr_string(a: &app::MailAddress) -> String {
    if a.username.is_empty() {
        a.address.clone()
    } else {
        format!("@{}", a.username)
    }
}

/// Fields the composer edits.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DraftFields {
    /// To, as typed.
    #[serde(default)]
    pub to: Vec<String>,
    /// CC.
    #[serde(default)]
    pub cc: Vec<String>,
    /// BCC.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Subject.
    #[serde(default)]
    pub subject: String,
    /// Body.
    #[serde(default)]
    pub body_text: String,
    /// Optional HTML.
    #[serde(default)]
    pub body_html: String,
}

/// Starts a draft: empty, a reply (`reply_to` + `all`) or a forward.
#[tauri::command]
pub async fn mail_draft_new(
    state: S<'_>,
    reply_to: Option<String>,
    all: Option<bool>,
    forward: Option<String>,
) -> CmdResult<DraftView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = new_draft_id();
    let mut rec = DraftRecord {
        updated_at_ms: now_ms(),
        ..Default::default()
    };
    if let Some(orig) = reply_to.as_deref() {
        let d = one.mail().reply_draft(orig, all.unwrap_or(false))?;
        rec.to = d.to.iter().map(addr_string).collect();
        rec.cc = d.cc.iter().map(addr_string).collect();
        rec.subject = d.subject;
        rec.in_reply_to = orig.to_owned();
        // Quote the original.
        if let Some(o) = one.mail().get(orig)? {
            let who = o
                .message
                .from
                .as_ref()
                .map(addr_string)
                .unwrap_or_else(|| o.authenticated_sender.clone());
            let quoted: String = o
                .message
                .body_text
                .lines()
                .map(|l| format!("> {l}"))
                .collect::<Vec<_>>()
                .join("\n");
            rec.body_text = format!("\n\n{who} wrote:\n{quoted}");
        }
    } else if let Some(orig) = forward.as_deref() {
        let d = one.mail().forward_draft(orig)?;
        rec.subject = d.subject;
        rec.body_text = d.body_text;
        rec.attachments = d.attachments;
    }
    one.mail().save_draft(&id, &rec)?;
    Ok(DraftView::of(&id, &rec))
}

/// Saves the typed fields of a draft (attachments are kept).
#[tauri::command]
pub async fn mail_draft_save(state: S<'_>, id: String, fields: DraftFields) -> CmdResult<DraftView> {
    if fields.subject.len() > m::MAX_SUBJECT {
        return Err(UiError::invalid("subject too long"));
    }
    if fields.body_text.len() > m::MAX_BODY_TEXT || fields.body_html.len() > m::MAX_BODY_HTML {
        return Err(UiError::invalid("body too long (1 MiB)"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &id).unwrap_or_default();
    rec.to = fields.to;
    rec.cc = fields.cc;
    rec.bcc = fields.bcc;
    rec.subject = fields.subject;
    rec.body_text = fields.body_text;
    rec.body_html = fields.body_html;
    rec.updated_at_ms = now_ms();
    one.mail().save_draft(&id, &rec)?;
    Ok(DraftView::of(&id, &rec))
}

/// Lists drafts, newest first.
#[tauri::command]
pub async fn mail_draft_list(state: S<'_>) -> CmdResult<Vec<DraftView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut v: Vec<DraftView> = one
        .mail()
        .drafts()?
        .iter()
        .map(|(k, d)| DraftView::of(k, d))
        .collect();
    v.sort_by_key(|d| std::cmp::Reverse(d.updated_at_ms));
    Ok(v)
}

/// One draft.
#[tauri::command]
pub async fn mail_draft_get(state: S<'_>, id: String) -> CmdResult<DraftView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let rec = load_draft(one, &id)?;
    Ok(DraftView::of(&id, &rec))
}

/// Deletes a draft.
#[tauri::command]
pub async fn mail_draft_delete(state: S<'_>, id: String) -> CmdResult<bool> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.mail().delete_draft(&id)?)
}

/// Attaches a file from disk to a draft (read here, never in the webview).
/// Small files ride inline; larger ones become encrypted blobs on store
/// nodes, so this needs the network.
#[tauri::command]
pub async fn mail_attach_file(state: S<'_>, draft_id: String, path: String) -> CmdResult<DraftView> {
    let p = std::path::PathBuf::from(&path);
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment")
        .to_owned();
    let meta = std::fs::metadata(&p)?;
    if meta.len() > 256 * 1024 * 1024 {
        return Err(UiError::invalid("attachments over 256 MiB: share the file from Drive instead"));
    }
    let bytes = tokio::fs::read(&p).await?;
    let mime = mime_guess::from_path(&p).first_or_octet_stream().to_string();
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &draft_id)?;
    if rec.attachments.len() >= m::MAX_ATTACHMENTS {
        return Err(UiError::invalid("64 attachments at most"));
    }
    let a = one.mail().make_attachment(&name, &mime, &bytes).await?;
    rec.attachments.push(a);
    rec.updated_at_ms = now_ms();
    one.mail().save_draft(&draft_id, &rec)?;
    one.save()?;
    Ok(DraftView::of(&draft_id, &rec))
}

/// Attaches small dropped bytes (base64, ≤ 8 MiB) to a draft.
#[tauri::command]
pub async fn mail_attach_bytes(
    state: S<'_>,
    draft_id: String,
    name: String,
    mime: String,
    base64: String,
) -> CmdResult<DraftView> {
    let bytes = crate::util::base64_decode(&base64).ok_or_else(|| UiError::invalid("bad base64"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(UiError::invalid("use the file picker for files over 8 MiB"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &draft_id)?;
    if rec.attachments.len() >= m::MAX_ATTACHMENTS {
        return Err(UiError::invalid("64 attachments at most"));
    }
    let mime = if mime.trim().is_empty() {
        mime_guess::from_path(&name).first_or_octet_stream().to_string()
    } else {
        mime
    };
    let a = one.mail().make_attachment(name.trim(), &mime, &bytes).await?;
    rec.attachments.push(a);
    rec.updated_at_ms = now_ms();
    one.mail().save_draft(&draft_id, &rec)?;
    one.save()?;
    Ok(DraftView::of(&draft_id, &rec))
}

/// Attaches one of our Drive entries (snapshot or live) to a draft. The
/// share is granted to the draft's current recipients, so add them first.
#[tauri::command]
pub async fn mail_attach_drive(
    state: S<'_>,
    draft_id: String,
    entry_id: String,
    live: bool,
) -> CmdResult<DraftView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &draft_id)?;
    let mut recipients = Vec::new();
    for r in rec.to.iter().chain(rec.cc.iter()).chain(rec.bcc.iter()) {
        if let Ok(res) = one.people().resolve(r).await {
            recipients.push(res.address);
        }
    }
    if recipients.is_empty() {
        return Err(UiError::invalid("add at least one Hashgram recipient before attaching from Drive"));
    }
    if rec.attachments.len() >= m::MAX_ATTACHMENTS {
        return Err(UiError::invalid("64 attachments at most"));
    }
    let a = one.mail().attach_from_drive(&entry_id, &recipients, live).await?;
    rec.attachments.push(a);
    rec.updated_at_ms = now_ms();
    one.mail().save_draft(&draft_id, &rec)?;
    one.save()?;
    Ok(DraftView::of(&draft_id, &rec))
}

/// Removes an attachment from a draft.
#[tauri::command]
pub async fn mail_draft_remove_attachment(state: S<'_>, draft_id: String, index: usize) -> CmdResult<DraftView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &draft_id)?;
    if index >= rec.attachments.len() {
        return Err(UiError::invalid("no such attachment"));
    }
    rec.attachments.remove(index);
    rec.updated_at_ms = now_ms();
    one.mail().save_draft(&draft_id, &rec)?;
    Ok(DraftView::of(&draft_id, &rec))
}

/// Send options.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SendOptions {
    /// Ask for a read receipt.
    #[serde(default)]
    pub request_read_receipt: bool,
    /// 0 normal, 1 low, 2 high.
    #[serde(default)]
    pub importance: i32,
    /// Advisory expiry (seconds), 0 = none.
    #[serde(default)]
    pub expire_after_secs: u32,
}

/// Sends a draft. External recipients (plain e-mail) go to the configured
/// gateway identity with `ext-to:` / `ext-cc:` labels (`docs/MAIL_GATEWAY.md`).
/// On success the draft is deleted and the Sent id is returned.
#[tauri::command]
pub async fn mail_send(
    state: S<'_>,
    fields: Option<DraftFields>,
    draft_id: String,
    options: Option<SendOptions>,
) -> CmdResult<String> {
    let gateway_setting = state.settings.read().await.network.gateway_address.trim().to_owned();
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut rec = load_draft(one, &draft_id)?;
    if let Some(f) = fields {
        rec.to = f.to;
        rec.cc = f.cc;
        rec.bcc = f.bcc;
        rec.subject = f.subject;
        rec.body_text = f.body_text;
        rec.body_html = f.body_html;
        one.mail().save_draft(&draft_id, &rec)?;
    }
    let opts = options.unwrap_or_default();
    let mut labels = Vec::new();
    let mut needs_gateway = false;
    let mut to = Vec::new();
    let mut cc = Vec::new();
    let mut bcc = Vec::new();
    for (list, out, ext_label) in [
        (&rec.to, &mut to, "ext-to"),
        (&rec.cc, &mut cc, "ext-cc"),
        (&rec.bcc, &mut bcc, "ext-bcc"),
    ] {
        for r in list {
            let r = r.trim();
            if r.is_empty() {
                continue;
            }
            match m::parse_address(r)? {
                AddressForm::External(e) => {
                    if ext_label == "ext-bcc" {
                        return Err(UiError::invalid(
                            "external addresses cannot be BCC'd through the gateway; use To or Cc",
                        ));
                    }
                    needs_gateway = true;
                    labels.push(format!("{ext_label}:{e}"));
                }
                _ => {
                    let mut res = one.mail().resolve_recipients(&[r.to_owned()]).await?;
                    if let Some(a) = res.pop() {
                        out.push(a);
                    }
                }
            }
        }
    }
    if needs_gateway {
        if gateway_setting.is_empty() {
            return Err(UiError::invalid(
                "sending to an e-mail address needs a mail gateway; set its @name or address in Settings → Network",
            ));
        }
        let mut gw = one.mail().resolve_recipients(&[gateway_setting]).await?;
        let gw = gw.pop().ok_or_else(|| UiError::invalid("gateway not resolved"))?;
        if !to.iter().any(|a| a.address == gw.address) {
            to.push(gw);
        }
    }
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(UiError::invalid("add at least one recipient"));
    }
    if labels.len() > m::MAX_LABELS {
        return Err(UiError::invalid("too many external recipients"));
    }
    let in_reply_to = if rec.in_reply_to.is_empty() {
        None
    } else {
        one.mail().get(&rec.in_reply_to)?.map(|o| m::ReplyTarget {
            message_id: o.message.message_id.clone(),
            thread_id: o.message.thread_id.clone(),
            references: o.message.references.clone(),
        })
    };
    let draft = m::Draft {
        to,
        cc,
        bcc,
        subject: rec.subject.clone(),
        body_text: rec.body_text.clone(),
        body_html: rec.body_html.clone(),
        attachments: rec.attachments.clone(),
        in_reply_to,
        expire_after_secs: opts.expire_after_secs,
        request_read_receipt: opts.request_read_receipt,
        importance: match opts.importance {
            1 => app::MailImportance::Low,
            2 => app::MailImportance::High,
            _ => app::MailImportance::Normal,
        },
        labels,
        ..Default::default()
    };
    let id = one.mail().send(draft).await?;
    let _ = one.mail().delete_draft(&draft_id);
    one.save()?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

fn attachment_of(one: &mut HashgramOne, id: &str, index: usize) -> CmdResult<app::MailAttachment> {
    let rec = one
        .mail()
        .get(id)?
        .ok_or_else(|| UiError::not_found("mail"))?;
    rec.message
        .attachments
        .get(index)
        .cloned()
        .ok_or_else(|| UiError::not_found("attachment"))
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let t = cleaned.trim().trim_matches('.');
    if t.is_empty() {
        "attachment".into()
    } else {
        t.chars().take(120).collect()
    }
}

/// Decrypts an attachment and writes it to `path` (chosen with the save
/// dialog by the webview).
#[tauri::command]
pub async fn mail_attachment_save(state: S<'_>, id: String, index: usize, path: String) -> CmdResult<u64> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let a = attachment_of(one, &id, index)?;
    let bytes = one.mail().attachment_bytes(&a).await?;
    drop(g);
    tokio::fs::write(&path, &bytes).await?;
    Ok(bytes.len() as u64)
}

/// Decrypts an attachment to the scratch folder and opens it with the
/// default application. The scratch folder is wiped at lock and at start.
#[tauri::command]
pub async fn mail_attachment_open(state: S<'_>, app: AppHandle, id: String, index: usize) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let a = attachment_of(one, &id, index)?;
    let bytes = one.mail().attachment_bytes(&a).await?;
    drop(g);
    let dir = crate::paths::tmp_dir().join(&id[..id.len().min(12)]);
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(safe_name(&a.name));
    tokio::fs::write(&p, &bytes).await?;
    crate::util::open_path(&app, &p)?;
    Ok(p.display().to_string())
}

/// Bytes of a small inline/blob attachment for previewing images in the
/// reading pane (base64; ≤ 8 MiB).
#[tauri::command]
pub async fn mail_attachment_preview(state: S<'_>, id: String, index: usize) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let a = attachment_of(one, &id, index)?;
    if a.size > 8 * 1024 * 1024 || !a.mime.starts_with("image/") {
        return Err(UiError::invalid("preview only for images up to 8 MiB"));
    }
    let bytes = one.mail().attachment_bytes(&a).await?;
    Ok(crate::util::base64_encode(&bytes))
}

/// Attachment metadata of a message (for lists that only have a summary).
#[tauri::command]
pub async fn mail_attachments(state: S<'_>, id: String) -> CmdResult<Vec<AttachmentView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let rec = one.mail().get(&id)?.ok_or_else(|| UiError::not_found("mail"))?;
    Ok(rec
        .message
        .attachments
        .iter()
        .enumerate()
        .map(|(i, a)| AttachmentView::of(i, a))
        .collect())
}

/// For a live Drive attachment: whether a newer version has arrived
/// through `Drive::shared_with_me` (share id → latest version number).
#[tauri::command]
pub async fn mail_live_attachment_versions(state: S<'_>) -> CmdResult<BTreeMap<String, u64>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .drive()
        .shared_with_me()?
        .into_iter()
        .map(|s| (hex::encode(&s.capability.share_id), s.capability.version_no))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(safe_name("../evil.exe"), "_evil.exe");
        assert_eq!(safe_name("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_name("   "), "attachment");
    }

    #[test]
    fn address_strings_prefer_usernames() {
        let a = app::MailAddress {
            address: "hash1abc".into(),
            username: "alice".into(),
            display_name: String::new(),
        };
        assert_eq!(addr_string(&a), "@alice");
        let b = app::MailAddress {
            address: "hash1abc".into(),
            ..Default::default()
        };
        assert_eq!(addr_string(&b), "hash1abc");
    }
}
