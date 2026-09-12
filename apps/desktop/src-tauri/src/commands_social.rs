//! Stage 2 commands: messages, feed, reels, stories, channels, calls.

use std::sync::Arc;

use hashgram_sdk::chat;
use hashgram_sdk::link::Link;
use hashgram_sdk::{pb, ChainClient, NetworkIdentity};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::chat::{self as chatmod, AttachmentView, ConversationMeta, MessageView};
use crate::social_hub::{self as social, EventView, MediaView};
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

async fn net(state: &AppState) -> Result<(Arc<Link>, NetworkIdentity), String> {
    let link = state
        .net
        .link()
        .await
        .ok_or_else(|| "not connected to the network yet".to_owned())?;
    let identity = state
        .net
        .identity()
        .await
        .ok_or_else(|| "network not started".to_owned())?;
    Ok((link, identity))
}

async fn chain_opt(state: &AppState) -> Option<ChainClient> {
    let s = state.settings.read().await.clone();
    let link = state.net.link().await;
    state
        .chain
        .client(link, &s.network.local_node_api, &s.network.https_endpoints)
        .await
        .ok()
        .map(|(_, c)| c)
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// Conversations.
#[tauri::command]
pub async fn chat_list(state: S<'_>) -> Result<Vec<ConversationMeta>, String> {
    let (_, key, me) = state.session_handles().await?;
    state.chat.conversations(&state.db, &key, &me).await
}

/// History of a conversation (newest last).
#[tauri::command]
pub async fn chat_history(
    state: S<'_>,
    group_id: String,
    before_ts: Option<u64>,
    limit: Option<usize>,
) -> Result<Vec<MessageView>, String> {
    let (_, key, _) = state.session_handles().await?;
    chatmod::history(
        &state.db,
        &key,
        &group_id,
        before_ts,
        limit.unwrap_or(80).min(500),
    )
}

/// Starts or reuses a direct chat. Returns the group id.
#[tauri::command]
pub async fn chat_start_direct(state: S<'_>, address: String) -> Result<String, String> {
    crate::tx::validate_address(&address, crate::tx::ADDRESS_PREFIX)?;
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let chain = chain_opt(&state)
        .await
        .ok_or_else(|| "no chain source to look up the recipient's devices".to_owned())?;
    state
        .chat
        .start_direct(&account, &link, &chain, &network, &address)
        .await
}

/// Creates a group.
#[tauri::command]
pub async fn chat_create_group(
    state: S<'_>,
    name: String,
    members: Vec<String>,
) -> Result<String, String> {
    for m in &members {
        crate::tx::validate_address(m, crate::tx::ADDRESS_PREFIX)?;
    }
    if name.trim().is_empty() {
        return Err("a group needs a name".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let chain = chain_opt(&state)
        .await
        .ok_or_else(|| "no chain source to look up members' devices".to_owned())?;
    state
        .chat
        .create_group(&account, &link, &chain, &network, name.trim(), &members)
        .await
}

/// Adds a member.
#[tauri::command]
pub async fn chat_add_member(
    state: S<'_>,
    group_id: String,
    address: String,
) -> Result<(), String> {
    crate::tx::validate_address(&address, crate::tx::ADDRESS_PREFIX)?;
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let chain = chain_opt(&state)
        .await
        .ok_or_else(|| "no chain source".to_owned())?;
    state
        .chat
        .add_member(&account, &link, &chain, &network, &group_id, &address)
        .await
}

/// Removes a member.
#[tauri::command]
pub async fn chat_remove_member(
    state: S<'_>,
    group_id: String,
    address: String,
) -> Result<usize, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    state
        .chat
        .remove_member(&account, &link, &network, &group_id, &address)
        .await
}

/// Sends text (optionally a reply).
#[tauri::command]
pub async fn chat_send_text(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    text: String,
    reply_to: Option<String>,
) -> Result<MessageView, String> {
    if text.trim().is_empty() {
        return Err("empty message".into());
    }
    if text.len() > 16_000 {
        return Err("message too long".into());
    }
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let disappear = chatmod::disappear_of(&state.db, &key, &group_id);
    let msg = chatmod::text_message(text.trim(), reply_to.as_deref(), disappear, vec![]);
    let v = state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(v)
}

/// Sends a file from disk as an encrypted attachment.
#[tauri::command]
pub async fn chat_send_file(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    path: String,
    caption: Option<String>,
) -> Result<MessageView, String> {
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    if data.len() > chatmod::MAX_ATTACHMENT_BYTES {
        return Err("attachments are limited to 100 MiB".into());
    }
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    let (mime, kind) = chatmod::mime_for(&name);
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let att =
        chatmod::upload_attachment(&link, &network, &device, &data, mime, &name, kind, 0).await?;
    let disappear = chatmod::disappear_of(&state.db, &key, &group_id);
    let msg = chatmod::text_message(
        caption.as_deref().unwrap_or("").trim(),
        None,
        disappear,
        vec![att],
    );
    let v = state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(v)
}

/// Sends a voice note recorded in the webview (audio/webm bytes, base64).
#[tauri::command]
pub async fn chat_send_voice(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    audio_base64: String,
    duration_ms: u32,
) -> Result<MessageView, String> {
    let data =
        base64_decode(&audio_base64).ok_or_else(|| "audio is not valid base64".to_owned())?;
    if data.is_empty() || data.len() > 20 * 1024 * 1024 {
        return Err("voice note is empty or too large".into());
    }
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let att = chatmod::upload_attachment(
        &link,
        &network,
        &device,
        &data,
        "audio/webm",
        "voice.weba",
        "audio",
        duration_ms,
    )
    .await?;
    let disappear = chatmod::disappear_of(&state.db, &key, &group_id);
    let msg = chatmod::text_message("", None, disappear, vec![att]);
    let v = state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(v)
}

/// Downloads an attachment into the media cache and returns its path.
#[tauri::command]
pub async fn chat_attachment(state: S<'_>, attachment: AttachmentView) -> Result<String, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let chain = chain_opt(&state).await;
    let receipt = chain.as_ref().map(|c| (c, &network, &device));
    let p = chatmod::fetch_attachment(&link, &attachment, receipt).await?;
    Ok(p.display().to_string())
}

/// Reacts to a message (empty reaction removes).
#[tauri::command]
pub async fn chat_react(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    target: String,
    reaction: String,
) -> Result<(), String> {
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let msg =
        chatmod::control_message(chat::ChatKind::Reaction, Some(&target), "", reaction.trim());
    state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(())
}

/// Edits one of our messages.
#[tauri::command]
pub async fn chat_edit(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    target: String,
    text: String,
) -> Result<(), String> {
    let (account, key, me) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let msg = chatmod::control_message(chat::ChatKind::Edit, Some(&target), text.trim(), "");
    state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    // Apply locally too.
    let _ = chatmod_apply_edit(&state.db, &key, &group_id, &target, &me, text.trim());
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(())
}

fn chatmod_apply_edit(
    db: &crate::db::Db,
    key: &crate::crypto::DbKey,
    group_id: &str,
    target: &str,
    me: &str,
    text: &str,
) -> Result<(), String> {
    // Reuse the same code path receivers use.
    crate::chat::apply_edit_public(db, key, group_id, target, me, text)
}

/// Deletes one of our messages for everyone (tombstone).
#[tauri::command]
pub async fn chat_delete(
    state: S<'_>,
    app: AppHandle,
    group_id: String,
    target: String,
) -> Result<(), String> {
    let (account, key, me) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let msg = chatmod::control_message(chat::ChatKind::Delete, Some(&target), "", "");
    state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    let _ = crate::chat::tombstone_public(&state.db, &key, &group_id, &target, &me);
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(())
}

/// Marks a conversation read and sends a READ receipt.
#[tauri::command]
pub async fn chat_mark_read(state: S<'_>, app: AppHandle, group_id: String) -> Result<(), String> {
    let (account, key, me) = state.session_handles().await?;
    let last = chatmod::mark_read(&state.db, &key, &group_id, &me)?;
    if let Some(target) = last {
        if let Ok((link, network)) = net(&state).await {
            let msg = chatmod::control_message(chat::ChatKind::Read, Some(&target), "", "");
            let _ = state
                .chat
                .send(&account, &state.db, &key, &link, &network, &group_id, msg)
                .await;
        }
    }
    update_badge(&app, &state);
    let _ = app.emit("chat:changed", serde_json::json!({ "group_id": group_id }));
    Ok(())
}

/// Typing signal (best effort, ephemeral).
#[tauri::command]
pub async fn chat_typing(state: S<'_>, group_id: String) -> Result<(), String> {
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let msg = chatmod::control_message(chat::ChatKind::Typing, None, "", "");
    let _ = state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await;
    Ok(())
}

/// Who is typing.
#[tauri::command]
pub async fn chat_typing_in(state: S<'_>, group_id: String) -> Result<Vec<String>, String> {
    Ok(state.chat.typing_in(&group_id).await)
}

/// Sets the disappearing-messages timer for a conversation (seconds; 0 off).
#[tauri::command]
pub async fn chat_set_disappear(state: S<'_>, group_id: String, secs: u32) -> Result<(), String> {
    let (_, key, _) = state.session_handles().await?;
    chatmod::set_disappear(&state.db, &key, &group_id, secs)
}

/// Chat info: members with their devices, and the store nodes holding our
/// mailbox.
#[derive(Debug, Clone, Serialize)]
pub struct ChatInfo {
    /// Members: address, device key hex, device label from chain (if known).
    pub members: Vec<(String, String, String)>,
    /// Store peers holding this device's mailbox.
    pub store_nodes: Vec<String>,
    /// Disappearing timer.
    pub disappear_secs: u32,
    /// Seconds since the last mailbox sync.
    pub last_sync_secs: Option<u64>,
}

/// Chat info.
#[tauri::command]
pub async fn chat_info(state: S<'_>, group_id: String) -> Result<ChatInfo, String> {
    let (account, key, _) = state.session_handles().await?;
    let members = state.chat.members(&group_id).await?;
    let chain = chain_opt(&state).await;
    let mut out = Vec::new();
    let mut labels: std::collections::HashMap<String, std::collections::HashMap<String, String>> =
        Default::default();
    for (addr, dev) in members {
        if !labels.contains_key(&addr) {
            let mut m = std::collections::HashMap::new();
            if let Some(c) = &chain {
                if let Ok(devs) = hashgram_sdk::account::devices_on_chain(c, &addr).await {
                    for d in devs {
                        m.insert(hex::encode(&d.device_pubkey), d.label);
                    }
                }
            }
            labels.insert(addr.clone(), m);
        }
        let label = labels
            .get(&addr)
            .and_then(|m| m.get(&dev))
            .cloned()
            .unwrap_or_default();
        out.push((addr, dev, label));
    }
    let store_nodes = match net(&state).await {
        Ok((link, _)) => {
            let device = account.lock().await.device().map_err(|e| e.to_string())?;
            link.providers(hashgram_sdk::proto::dht::mailbox_for_device(
                &device.public_key(),
            ))
            .await
            .into_iter()
            .map(|p| p.to_string())
            .collect()
        }
        Err(_) => Vec::new(),
    };
    Ok(ChatInfo {
        members: out,
        store_nodes,
        disappear_secs: chatmod::disappear_of(&state.db, &key, &group_id),
        last_sync_secs: state.chat.last_sync_secs().await,
    })
}

/// Searches local plaintext (decrypted in memory).
#[tauri::command]
pub async fn chat_search(state: S<'_>, query: String) -> Result<Vec<MessageView>, String> {
    let (_, key, _) = state.session_handles().await?;
    chatmod::search(&state.db, &key, &query, 100)
}

/// Syncs mailboxes now.
#[tauri::command]
pub async fn chat_sync_now(state: S<'_>, app: AppHandle) -> Result<u32, String> {
    sync_once(&state, &app).await
}

/// One sync pass: fetch, store, notify, badge. Used by the command and the
/// background loop.
pub async fn sync_once(state: &AppState, app: &AppHandle) -> Result<u32, String> {
    if !state.chat.is_open().await {
        return Ok(0);
    }
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(state).await?;
    let chain = chain_opt(state).await;
    let report = state
        .chat
        .sync(&account, &state.db, &key, &link, &network, chain.as_ref())
        .await?;
    let total: u32 = report.new_by_group.values().sum();
    if total > 0 || report.groups_changed {
        let _ = app.emit("chat:changed", serde_json::json!({ "new": total }));
        update_badge(app, state);
    }
    let notify_on = state.settings.read().await.notifications.messages;
    if notify_on {
        for (sender, preview, _) in report.notify.into_iter().take(3) {
            use tauri_plugin_notification::NotificationExt;
            let _ = app
                .notification()
                .builder()
                .title(format!(
                    "Message from {}",
                    crate::tx::truncate_middle(&sender, 10, 6)
                ))
                .body(preview)
                .show();
        }
    }
    Ok(total)
}

fn update_badge(app: &AppHandle, state: &AppState) {
    let n = chatmod::unread_total(&state.db);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(if n > 0 {
            format!("Hashgram — {n} unread")
        } else {
            "Hashgram".to_owned()
        }));
    }
    let _ = app.emit("chat:unread", n);
}

/// Publishes key packages now (first run / troubleshooting).
#[tauri::command]
pub async fn chat_publish_key_packages(state: S<'_>) -> Result<usize, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    state
        .chat
        .publish_key_packages(&account, &link, &network)
        .await
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let s = s.split(',').next_back().unwrap_or(s); // strip a data: prefix
    let mut out = Vec::with_capacity((s.len() * 3).div_euclid(4));
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' || c == b'\n' || c == b'\r' {
            continue;
        }
        let v = T.iter().position(|t| *t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Social
// ---------------------------------------------------------------------------

/// Feed page.
#[derive(Debug, Clone, Serialize)]
pub struct FeedPage {
    /// Events.
    pub events: Vec<EventView>,
    /// Events hidden as unverifiable since start.
    pub hidden: u32,
    /// Authors the feed covers.
    pub authors: Vec<String>,
}

fn apply_blocks(db: &crate::db::Db, events: &mut Vec<EventView>) {
    if let Ok(b) = social::blocks(db) {
        let set: std::collections::HashSet<String> = b.into_iter().map(|(a, _)| a).collect();
        events.retain(|e| !set.contains(&e.author));
    }
}

/// The chronological following feed (from the local cache; call
/// `feed_refresh` to pull).
#[tauri::command]
pub async fn feed(
    state: S<'_>,
    kinds: Option<Vec<String>>,
    tag: Option<String>,
    before_ts: Option<u64>,
    limit: Option<usize>,
) -> Result<FeedPage, String> {
    let (_, _, me) = state.session_handles().await?;
    let mut authors = social::follows(&state.db)?;
    authors.push(me.clone());
    let kinds_v: Vec<String> =
        kinds.unwrap_or_else(|| vec!["POST_CREATE".into(), "REPOST".into(), "REEL_CREATE".into()]);
    let kinds_ref: Vec<&str> = kinds_v.iter().map(String::as_str).collect();
    let mut events = social::feed(
        &state.db,
        if tag.is_some() { &[] } else { &authors },
        &kinds_ref,
        tag.as_deref(),
        before_ts,
        limit.unwrap_or(50).min(200),
        &me,
    )?;
    apply_blocks(&state.db, &mut events);
    Ok(FeedPage {
        events,
        hidden: state.social.hidden_count(),
        authors,
    })
}

/// Pulls the latest events of every followed author (and ours).
#[tauri::command]
pub async fn feed_refresh(state: S<'_>, app: AppHandle) -> Result<usize, String> {
    refresh_feed(&state, &app).await
}

/// Refreshes followed authors; used by the command and the background loop.
pub async fn refresh_feed(state: &AppState, app: &AppHandle) -> Result<usize, String> {
    let (_, _, me) = state.session_handles().await?;
    let (link, network) = net(state).await?;
    let chain = chain_opt(state).await;
    let mut authors = social::follows(&state.db)?;
    authors.push(me);
    let mut total = 0;
    for a in authors {
        match state
            .social
            .refresh_author(&state.db, &link, &network, chain.as_ref(), &a, 100)
            .await
        {
            Ok(n) => total += n,
            Err(e) => tracing::debug!(author = %a, error = %e, "feed refresh"),
        }
    }
    if total > 0 {
        let _ = app.emit("feed:changed", total);
    }
    Ok(total)
}

/// Publishes a post (optionally with media already uploaded via `media_upload`).
#[tauri::command]
pub async fn post_create(
    state: S<'_>,
    app: AppHandle,
    text: String,
    hashtags: Vec<String>,
    channel: Option<String>,
    reply_to: Option<String>,
    media: Option<Vec<MediaView>>,
) -> Result<EventView, String> {
    if text.trim().is_empty() && media.as_ref().map(|m| m.is_empty()).unwrap_or(true) {
        return Err("a post needs text or media".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let tags: Vec<String> = hashtags
        .iter()
        .map(|t| t.trim_start_matches('#').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let mentions: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.starts_with("hash1") && w.len() > 40)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned())
        .collect();
    let payload = pb::PostCreate {
        text: text.trim().to_owned(),
        hashtags: tags,
        mentions,
        channel: channel
            .and_then(|c| hex::decode(c).ok())
            .unwrap_or_default(),
        reply_to: reply_to
            .and_then(|c| hex::decode(c).ok())
            .unwrap_or_default(),
        ..Default::default()
    };
    let refs = media_refs(media);
    let ev = state
        .social
        .publish(&account, &link, &network, "POST_CREATE", &payload, refs)
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

fn media_refs(media: Option<Vec<MediaView>>) -> Vec<pb::MediaReference> {
    media
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            Some(pb::MediaReference {
                cid: hex::decode(&m.cid).ok()?,
                mime: m.mime,
                size: m.size,
                kind: m.kind,
                width: m.width,
                height: m.height,
                duration_ms: m.duration_ms,
                content_hash: hex::decode(&m.content_hash).unwrap_or_default(),
                ..Default::default()
            })
        })
        .collect()
}

fn social_view(ev: &pb::SocialEvent) -> EventView {
    // Re-read through the cache so folding applies uniformly.
    EventView {
        id: hex::encode(&ev.id),
        kind: ev.r#type.clone(),
        author: ev.author.clone(),
        username: None,
        display_name: None,
        sequence: ev.sequence,
        timestamp: ev.timestamp,
        payload: hashgram_sdk::social::payload_json(ev),
        media: ev
            .media
            .iter()
            .map(|m| MediaView {
                cid: hex::encode(&m.cid),
                mime: m.mime.clone(),
                size: m.size,
                kind: m.kind.clone(),
                width: m.width,
                height: m.height,
                duration_ms: m.duration_ms,
                content_hash: hex::encode(&m.content_hash),
            })
            .collect(),
        device: hex::encode(&ev.device_pubkey),
        reactions: Default::default(),
        comments: 0,
        reposts: 0,
        my_reaction: None,
    }
}

/// Comments on a post.
#[tauri::command]
pub async fn comment_create(
    state: S<'_>,
    app: AppHandle,
    post: String,
    text: String,
) -> Result<EventView, String> {
    if text.trim().is_empty() {
        return Err("empty comment".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::CommentCreate {
        post: hex::decode(&post).map_err(|e| e.to_string())?,
        text: text.trim().to_owned(),
        ..Default::default()
    };
    let ev = state
        .social
        .publish(
            &account,
            &link,
            &network,
            "COMMENT_CREATE",
            &payload,
            vec![],
        )
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

/// Reacts to an event (empty reaction removes).
#[tauri::command]
pub async fn social_react(
    state: S<'_>,
    app: AppHandle,
    target: String,
    reaction: String,
) -> Result<(), String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::Reaction {
        target: hex::decode(&target).map_err(|e| e.to_string())?,
        reaction: reaction.trim().to_owned(),
    };
    let ev = state
        .social
        .publish(&account, &link, &network, "REACTION", &payload, vec![])
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(())
}

/// Reposts.
#[tauri::command]
pub async fn repost(
    state: S<'_>,
    app: AppHandle,
    post: String,
    comment: Option<String>,
) -> Result<(), String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::Repost {
        post: hex::decode(&post).map_err(|e| e.to_string())?,
        comment: comment.unwrap_or_default(),
    };
    let ev = state
        .social
        .publish(&account, &link, &network, "REPOST", &payload, vec![])
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(())
}

/// Follows (publishes FOLLOW and records locally).
#[tauri::command]
pub async fn follow(state: S<'_>, app: AppHandle, address: String, on: bool) -> Result<(), String> {
    crate::tx::validate_address(&address, crate::tx::ADDRESS_PREFIX)?;
    let (account, _, _) = state.session_handles().await?;
    social::set_follow(&state.db, &address, on)?;
    if let Ok((link, network)) = net(&state).await {
        let res = if on {
            state
                .social
                .publish(
                    &account,
                    &link,
                    &network,
                    "FOLLOW",
                    &pb::Follow {
                        target: address.clone(),
                    },
                    vec![],
                )
                .await
        } else {
            state
                .social
                .publish(
                    &account,
                    &link,
                    &network,
                    "UNFOLLOW",
                    &pb::Unfollow {
                        target: address.clone(),
                    },
                    vec![],
                )
                .await
        };
        if let Err(e) = res {
            tracing::debug!(error = %e, "follow event not published (recorded locally)");
        }
    }
    let _ = app.emit("feed:changed", 1);
    Ok(())
}

/// Follow list.
#[tauri::command]
pub async fn follows(state: S<'_>) -> Result<Vec<String>, String> {
    social::follows(&state.db)
}

/// Mute or block locally (`mode` = "mute" | "block" | null to clear).
#[tauri::command]
pub async fn social_block(
    state: S<'_>,
    address: String,
    mode: Option<String>,
) -> Result<(), String> {
    social::set_block(&state.db, &address, mode.as_deref())
}

/// Mutes and blocks.
#[tauri::command]
pub async fn social_blocks(state: S<'_>) -> Result<Vec<(String, String)>, String> {
    social::blocks(&state.db)
}

/// A person's public profile.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    /// Address.
    pub address: String,
    /// Latest PROFILE_UPDATE payload, if any (display_name, bio, avatar_cid…).
    pub profile: Option<serde_json::Value>,
    /// We follow them.
    pub following: bool,
    /// Muted/blocked mode.
    pub block_mode: Option<String>,
    /// Number of cached events by them.
    pub events: u32,
}

/// Loads (and refreshes) a profile.
#[tauri::command]
pub async fn profile_get(
    state: S<'_>,
    address: String,
    refresh: bool,
) -> Result<ProfileView, String> {
    crate::tx::validate_address(&address, crate::tx::ADDRESS_PREFIX)?;
    if refresh {
        if let Ok((link, network)) = net(&state).await {
            let chain = chain_opt(&state).await;
            let _ = state
                .social
                .refresh_author(&state.db, &link, &network, chain.as_ref(), &address, 100)
                .await;
        }
    }
    let profile = social::profile(&state.db, &address)?;
    let following = social::follows(&state.db)?.contains(&address);
    let block_mode = social::blocks(&state.db)?
        .into_iter()
        .find(|(a, _)| *a == address)
        .map(|(_, m)| m);
    let events: i64 = state
        .db
        .with(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM social_events WHERE author = ?1 AND verified = 1",
                [&address],
                |r| r.get(0),
            )
        })
        .unwrap_or(0);
    Ok(ProfileView {
        address,
        profile,
        following,
        block_mode,
        events: events.max(0) as u32,
    })
}

/// Updates our profile (display name is never identity: the verified handle
/// stays next to it everywhere).
#[tauri::command]
pub async fn profile_update(
    state: S<'_>,
    app: AppHandle,
    display_name: String,
    bio: String,
    avatar: Option<MediaView>,
) -> Result<EventView, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::ProfileUpdate {
        display_name: display_name.trim().chars().take(64).collect(),
        bio: bio.trim().chars().take(500).collect(),
        avatar_cid: avatar
            .as_ref()
            .and_then(|a| hex::decode(&a.cid).ok())
            .unwrap_or_default(),
        ..Default::default()
    };
    let ev = state
        .social
        .publish(
            &account,
            &link,
            &network,
            "PROFILE_UPDATE",
            &payload,
            media_refs(avatar.map(|a| vec![a])),
        )
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

/// A post with its comments.
#[tauri::command]
pub async fn post_thread(
    state: S<'_>,
    id: String,
) -> Result<(Option<EventView>, Vec<EventView>), String> {
    let (_, _, me) = state.session_handles().await?;
    social::thread(&state.db, &id, &me)
}

/// Uploads public media from disk (post image, reel video, avatar).
#[tauri::command]
pub async fn media_upload(
    state: S<'_>,
    path: String,
    duration_ms: Option<u32>,
) -> Result<MediaView, String> {
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let (mime, kind) = chatmod::mime_for(name);
    if kind == "video" && data.len() > social::MAX_REEL_BYTES {
        return Err("reels are limited to 100 MB (pre-encoded MP4)".into());
    }
    if kind == "video" && mime != "video/mp4" && mime != "video/webm" {
        return Err("reels must be MP4 (H.264/AAC) or WebM".into());
    }
    if data.len() > 100 * 1024 * 1024 {
        return Err("media is limited to 100 MB".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let r = social::upload_media(
        &link,
        &network,
        &device,
        &data,
        mime,
        kind,
        duration_ms.unwrap_or(0),
    )
    .await?;
    Ok(MediaView {
        cid: hex::encode(&r.cid),
        mime: r.mime,
        size: r.size,
        kind: r.kind,
        width: 0,
        height: 0,
        duration_ms: r.duration_ms,
        content_hash: hex::encode(&r.content_hash),
    })
}

/// Fetches public media into the cache and returns the path (hash-verified).
#[tauri::command]
pub async fn media_fetch(state: S<'_>, media: MediaView) -> Result<String, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let chain = chain_opt(&state).await;
    let receipt = chain.as_ref().map(|c| (c, &network, &device));
    let p = social::fetch_media(&link, &media, receipt).await?;
    Ok(p.display().to_string())
}

/// Publishes a reel: uploaded MP4 + REEL_CREATE.
#[tauri::command]
pub async fn reel_publish(
    state: S<'_>,
    app: AppHandle,
    media: MediaView,
    caption: String,
    hashtags: Vec<String>,
) -> Result<EventView, String> {
    if media.kind != "video" {
        return Err("a reel needs a video".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::ReelCreate {
        caption: caption.trim().to_owned(),
        hashtags: hashtags
            .iter()
            .map(|t| t.trim_start_matches('#').to_lowercase())
            .filter(|t| !t.is_empty())
            .collect(),
        video_index: 0,
        allow_comments: true,
        ..Default::default()
    };
    let ev = state
        .social
        .publish(
            &account,
            &link,
            &network,
            "REEL_CREATE",
            &payload,
            media_refs(Some(vec![media])),
        )
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

/// Publishes a story (expires in 24 h).
#[tauri::command]
pub async fn story_publish(
    state: S<'_>,
    app: AppHandle,
    media: MediaView,
    caption: String,
) -> Result<EventView, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 24 * 3600;
    let payload = pb::StoryCreate {
        caption: caption.trim().to_owned(),
        expires_at,
        ..Default::default()
    };
    let ev = state
        .social
        .publish(
            &account,
            &link,
            &network,
            "STORY_CREATE",
            &payload,
            media_refs(Some(vec![media])),
        )
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

/// Creates a channel.
#[tauri::command]
pub async fn channel_create(
    state: S<'_>,
    app: AppHandle,
    name: String,
    description: String,
    open_posting: bool,
) -> Result<EventView, String> {
    if name.trim().is_empty() {
        return Err("a channel needs a name".into());
    }
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let payload = pb::ChannelCreate {
        name: name.trim().to_owned(),
        description: description.trim().to_owned(),
        open_posting,
        ..Default::default()
    };
    let ev = state
        .social
        .publish(
            &account,
            &link,
            &network,
            "CHANNEL_CREATE",
            &payload,
            vec![],
        )
        .await?;
    social::store_event(&state.db, &ev, true)?;
    let _ = app.emit("feed:changed", 1);
    Ok(social_view(&ev))
}

/// Channels from followed authors and us.
#[tauri::command]
pub async fn channels(state: S<'_>) -> Result<Vec<EventView>, String> {
    let (_, _, me) = state.session_handles().await?;
    let mut authors = social::follows(&state.db)?;
    authors.push(me.clone());
    social::channels(&state.db, &authors, &me)
}

/// Posts in a channel.
#[tauri::command]
pub async fn channel_posts(state: S<'_>, channel_id: String) -> Result<Vec<EventView>, String> {
    let (_, _, me) = state.session_handles().await?;
    let mut posts = social::channel_posts(&state.db, &channel_id, 200, &me)?;
    apply_blocks(&state.db, &mut posts);
    Ok(posts)
}

/// Safety verdicts about a subject, read over P2P (`AttestationQuery`).
#[tauri::command]
pub async fn safety_verdict(state: S<'_>, subject: String) -> Result<serde_json::Value, String> {
    let (link, _) = net(&state).await?;
    let subject_bytes = hex::decode(&subject).unwrap_or_else(|_| subject.as_bytes().to_vec());
    let (peer, body) = link
        .request_role_any(pb::request::Body::AttestationQuery(pb::AttestationQuery {
            cids: vec![subject_bytes.clone()],
            event_ids: vec![subject_bytes],
        }))
        .await
        .map_err(|e| e.to_string())?;
    match body {
        pb::response::Body::AttestationQuery(r) => Ok(serde_json::json!({
            "peer": peer.to_string(),
            "attestations": r.attestations.iter().map(|a| serde_json::to_value(a).unwrap_or_default()).collect::<Vec<_>>(),
        })),
        _ => Err("unexpected answer".into()),
    }
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// Call infrastructure discovered on the network.
#[derive(Debug, Clone, Serialize)]
pub struct CallInfra {
    /// Nodes with the call role.
    pub nodes: Vec<serde_json::Value>,
    /// Any SFU announced (group calls need one).
    pub sfu_available: bool,
}

/// Discovers call nodes.
#[tauri::command]
pub async fn calls_discover(state: S<'_>) -> Result<CallInfra, String> {
    let (link, _) = net(&state).await?;
    let nodes = hashgram_sdk::calls::discover(&link)
        .await
        .map_err(|e| e.to_string())?;
    let sfu_available = nodes
        .iter()
        .any(|n| n.sfu_url.as_ref().map(|s| !s.is_empty()).unwrap_or(false));
    Ok(CallInfra {
        nodes: nodes
            .iter()
            .map(|n| serde_json::to_value(n).unwrap_or_default())
            .collect(),
        sfu_available,
    })
}

/// Fetches TURN credentials for a call.
#[tauri::command]
pub async fn calls_turn(state: S<'_>) -> Result<serde_json::Value, String> {
    let (account, _, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let device = account.lock().await.device().map_err(|e| e.to_string())?;
    let ice = hashgram_sdk::calls::turn_credentials(&link, &network, &device, None)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::to_value(ice).map_err(|e| e.to_string())
}

/// Sends a call signal (offer/answer/ice/hangup/ring/busy) over the E2EE chat.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // one argument per field of the JS signal payload
pub async fn calls_signal(
    state: S<'_>,
    group_id: String,
    kind: String,
    call_id: String,
    sdp: Option<String>,
    candidate: Option<String>,
    sdp_mid: Option<String>,
    sdp_mline_index: Option<u32>,
    video: bool,
) -> Result<(), String> {
    let (account, key, _) = state.session_handles().await?;
    let (link, network) = net(&state).await?;
    let signal = chat::CallSignal {
        kind,
        call_id: hex::decode(&call_id).map_err(|e| e.to_string())?,
        sdp: sdp.unwrap_or_default(),
        candidate: candidate.unwrap_or_default(),
        sdp_mid: sdp_mid.unwrap_or_default(),
        sdp_mline_index: sdp_mline_index.unwrap_or(0),
        video,
    };
    let msg = chat::ChatMessage {
        version: hashgram_sdk::proto::limits::WIRE_VERSION,
        kind: chat::ChatKind::Call as i32,
        call: Some(signal),
        ..Default::default()
    };
    state
        .chat
        .send(&account, &state.db, &key, &link, &network, &group_id, msg)
        .await?;
    Ok(())
}

/// Recent call signals in a conversation (the webview drives WebRTC).
#[tauri::command]
pub async fn calls_signals(
    state: S<'_>,
    group_id: String,
    since_ms: u64,
) -> Result<Vec<serde_json::Value>, String> {
    let (_, key, _) = state.session_handles().await?;
    let rows = chatmod::history(&state.db, &key, &group_id, None, 200)?;
    Ok(rows
        .into_iter()
        .filter(|m| m.kind == "CALL" && m.timestamp_ms > since_ms)
        .map(|m| serde_json::to_value(m).unwrap_or_default())
        .collect())
}
