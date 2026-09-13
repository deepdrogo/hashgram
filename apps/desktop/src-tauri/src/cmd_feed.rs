//! Feed and Circles commands over `one.feed()` and `one.circles()`.
//!
//! Public media for posts is read from disk here and uploaded as public
//! blobs; Circle media travel inside the group and are uploaded as private
//! blobs whose keys stay inside the SDK record.

use std::sync::Arc;

use hashgram_sdk::circles::CircleInfo;
use hashgram_sdk::feed::{FeedItem, PostThread};
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::HashgramOne;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::views::CircleItemView;

type S<'a> = State<'a, Arc<AppState>>;

const MAX_MEDIA: usize = 20;
const MAX_MEDIA_BYTES: u64 = 64 * 1024 * 1024;

fn limit_of(l: Option<usize>) -> usize {
    l.unwrap_or(50).clamp(1, 200)
}

async fn read_media(path: &str) -> CmdResult<(Vec<u8>, String, String)> {
    let p = std::path::PathBuf::from(path);
    let meta = tokio::fs::metadata(&p).await?;
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(UiError::invalid("media over 64 MiB is not supported"));
    }
    let mime = mime_guess::from_path(&p).first_or_octet_stream().to_string();
    let kind = if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("audio/") {
        "audio"
    } else {
        "file"
    };
    Ok((tokio::fs::read(&p).await?, mime, kind.to_owned()))
}

/// Following feed (chronological, local cache).
#[tauri::command]
pub async fn feed_following(state: S<'_>, before: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<FeedItem>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().following(before.unwrap_or(0), limit_of(limit))?)
}

/// Friends feed.
#[tauri::command]
pub async fn feed_friends(state: S<'_>, before: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<FeedItem>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().friends(before.unwrap_or(0), limit_of(limit))?)
}

/// One author's posts (refreshes from the network first when online).
#[tauri::command]
pub async fn feed_author(state: S<'_>, address: String, before: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<FeedItem>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let _ = one.feed().refresh_author(address.trim(), 100).await;
    Ok(one.feed().author(address.trim(), before.unwrap_or(0), limit_of(limit))?)
}

/// Explore: the indexer's chronological or hashtag feed. `None` when no
/// indexer is configured.
#[tauri::command]
pub async fn feed_explore(state: S<'_>, before: Option<u64>, limit: Option<usize>, tag: Option<String>) -> CmdResult<Option<serde_json::Value>> {
    let indexer = state.settings.read().await.indexer().map(str::to_owned);
    let Some(base) = indexer else { return Ok(None) };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let limit = limit_of(limit);
    let before = before.unwrap_or(0);
    let path = match tag.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            let t: String = t.trim_start_matches('#').chars().filter(|c| c.is_alphanumeric() || *c == '_').take(64).collect();
            format!("/v1/feed/hashtag/{t}?limit={limit}&before={before}")
        }
        None => format!("/v1/feed/chronological?limit={limit}&before={before}"),
    };
    Ok(one.network_api().indexer(Some(&base), &path).await?)
}

/// A post with comments and reactions (fetches it if unknown).
#[tauri::command]
pub async fn feed_thread(state: S<'_>, post: String) -> CmdResult<Option<PostThread>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    if one.feed().thread(&post)?.is_none() {
        let _ = one.feed().fetch(std::slice::from_ref(&post)).await;
    }
    Ok(one.feed().thread(&post)?)
}

/// Creates a public post; `media_paths` are read here and uploaded.
/// `channel` (optional, hex id) puts the post on a wall.
#[tauri::command]
pub async fn feed_post(
    state: S<'_>,
    text: String,
    hashtags: Vec<String>,
    media_paths: Vec<String>,
    sensitive: bool,
    channel: Option<String>,
) -> CmdResult<String> {
    let channel = channel.map(|c| c.trim().to_ascii_lowercase()).unwrap_or_default();
    if !channel.is_empty() && (channel.len() != 64 || !channel.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(UiError::invalid("wall id"));
    }
    if text.trim().is_empty() && media_paths.is_empty() {
        return Err(UiError::invalid("write something or add media"));
    }
    if media_paths.len() > MAX_MEDIA {
        return Err(UiError::invalid("20 media at most"));
    }
    let mut files = Vec::new();
    for p in &media_paths {
        files.push(read_media(p).await?);
    }
    let tags: Vec<String> = hashtags
        .into_iter()
        .map(|t| t.trim().trim_start_matches('#').to_lowercase())
        .filter(|t| !t.is_empty() && t.len() <= 64)
        .collect();
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut media = Vec::new();
    for (bytes, mime, kind) in files {
        media.push(one.feed().upload_media(&bytes, &mime, &kind).await?);
    }
    let id = one
        .feed()
        .post_on(text.trim(), tags, media, sensitive, &channel)
        .await?;
    one.save()?;
    Ok(id)
}

/// Comments on a post.
#[tauri::command]
pub async fn feed_comment(state: S<'_>, post: String, text: String) -> CmdResult<String> {
    if text.trim().is_empty() {
        return Err(UiError::invalid("write something"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.feed().comment(&post, text.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Reacts (empty reaction removes).
#[tauri::command]
pub async fn feed_react(state: S<'_>, target: String, reaction: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.feed().react(&target, reaction.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Reposts.
#[tauri::command]
pub async fn feed_repost(state: S<'_>, post: String, comment: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.feed().repost(&post, comment.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Edits our post.
#[tauri::command]
pub async fn feed_edit(state: S<'_>, post: String, text: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.feed().edit_post(&post, text.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Deletes our post.
#[tauri::command]
pub async fn feed_delete(state: S<'_>, post: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.feed().delete_post(&post).await?;
    one.save()?;
    Ok(id)
}

/// Refreshes followed authors now. Returns new events.
#[tauri::command]
pub async fn feed_refresh(state: S<'_>) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().refresh().await?)
}

/// Addresses we follow.
#[tauri::command]
pub async fn feed_follows(state: S<'_>) -> CmdResult<Vec<String>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().follows().into_iter().collect())
}

/// Updates our public profile; `avatar_path` (optional) is uploaded as
/// public media.
#[tauri::command]
pub async fn feed_profile_update(state: S<'_>, name: String, bio: String, avatar_path: Option<String>) -> CmdResult<String> {
    if name.len() > 128 || bio.len() > 4096 {
        return Err(UiError::invalid("name ≤ 128 and bio ≤ 4096 characters"));
    }
    let avatar = match avatar_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Some(read_media(p).await?),
        None => None,
    };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let avatar_cid = match avatar {
        Some((bytes, mime, _)) => {
            if !mime.starts_with("image/") {
                return Err(UiError::invalid("the avatar must be an image"));
            }
            let m = one.feed().upload_media(&bytes, &mime, "image").await?;
            hex::encode(&m.cid)
        }
        None => String::new(),
    };
    let id = one.feed().update_profile(name.trim(), bio.trim(), &avatar_cid).await?;
    one.save()?;
    Ok(id)
}

/// Fetches public media by CID to the scratch folder and returns its path
/// (the webview shows it through the asset protocol).
#[tauri::command]
pub async fn feed_media_fetch(state: S<'_>, cid: String, mime: String) -> CmdResult<String> {
    let cid_hex = cid.trim().to_ascii_lowercase();
    if cid_hex.len() != 64 || !cid_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UiError::invalid("cid"));
    }
    let ext = mime_guess::get_mime_extensions_str(&mime)
        .and_then(|e| e.first())
        .copied()
        .unwrap_or("bin");
    let dir = crate::paths::sdk_paths().cache().join("media");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("{cid_hex}.{ext}"));
    if p.exists() {
        return Ok(p.display().to_string());
    }
    let cid_bytes = hex::decode(&cid_hex).map_err(|_| UiError::invalid("cid"))?;
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let (ct, _m, _p) = hashgram_sdk::blob::download(&one.link, &cid_bytes, None).await?;
        ct
    };
    tokio::fs::write(&p, &bytes).await?;
    Ok(p.display().to_string())
}

// ---------------------------------------------------------------------------
// Circles
// ---------------------------------------------------------------------------

/// Lists circles.
#[tauri::command]
pub async fn circles_list(state: S<'_>) -> CmdResult<Vec<CircleInfo>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.circles().list()?)
}

/// Creates a circle with members (addresses or names, resolved here).
#[tauri::command]
pub async fn circles_create(state: S<'_>, name: String, description: String, members: Vec<String>) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut addrs = Vec::new();
    for m in members {
        addrs.push(one.people().resolve(m.trim()).await?.address);
    }
    let id = one.circles().create(name.trim(), description.trim(), &addrs).await?;
    one.save()?;
    Ok(id)
}

/// Adds a member.
#[tauri::command]
pub async fn circles_add_member(state: S<'_>, circle: String, member: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let a = one.people().resolve(member.trim()).await?.address;
    one.circles().add_member(&circle, &a).await?;
    one.save()?;
    Ok(())
}

/// Removes a member.
#[tauri::command]
pub async fn circles_remove_member(state: S<'_>, circle: String, address: String) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let n = one.circles().remove_member(&circle, address.trim()).await?;
    one.save()?;
    Ok(n)
}

/// Leaves a circle.
#[tauri::command]
pub async fn circles_leave(state: S<'_>, circle: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.circles().leave(&circle).await?;
    one.save()?;
    Ok(())
}

/// A poll for a Circle post.
#[derive(Debug, Clone, Deserialize)]
pub struct PollInput {
    /// Question.
    pub question: String,
    /// Options (2–12).
    pub options: Vec<String>,
    /// Multiple choice.
    #[serde(default)]
    pub multiple_choice: bool,
    /// Closes at (ms), 0 = never.
    #[serde(default)]
    pub closes_at_ms: u64,
}

async fn private_media(one: &mut HashgramOne, paths: &[String]) -> CmdResult<Vec<app::BlobRef>> {
    if paths.len() > MAX_MEDIA {
        return Err(UiError::invalid("20 media at most"));
    }
    let mut out = Vec::new();
    for p in paths {
        let (bytes, mime, kind) = read_media(p).await?;
        let name = std::path::Path::new(p)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("media")
            .to_owned();
        let a = one.mail().make_attachment(&name, &mime, &bytes).await?;
        match a.source {
            Some(app::mail_attachment::Source::Blob(b)) => out.push(app::BlobRef { kind, ..b }),
            Some(app::mail_attachment::Source::InlineData(d)) => {
                // Small media still travel as blobs for Circles: upload.
                let device = one.account.device()?;
                let up = hashgram_sdk::blob::upload(&one.link, &one.network, &device, &d, &mime, true, 2).await?;
                out.push(app::BlobRef {
                    cid: hex::decode(&up.cid).unwrap_or_default(),
                    key: hex::decode(up.key.unwrap_or_default()).unwrap_or_default(),
                    nonce: hex::decode(up.nonce.unwrap_or_default()).unwrap_or_default(),
                    mime: mime.clone(),
                    size: d.len() as u64,
                    name,
                    kind,
                    plaintext_hash: blake3::hash(&d).as_bytes().to_vec(),
                    ..Default::default()
                });
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Posts to a circle (media files read here; optional poll).
#[tauri::command]
pub async fn circles_post(
    state: S<'_>,
    circle: String,
    text: String,
    media_paths: Vec<String>,
    poll: Option<PollInput>,
) -> CmdResult<String> {
    if text.trim().is_empty() && media_paths.is_empty() && poll.is_none() {
        return Err(UiError::invalid("write something, add media or a poll"));
    }
    let poll = match poll {
        Some(p) => {
            let opts: Vec<String> = p.options.into_iter().map(|o| o.trim().to_owned()).filter(|o| !o.is_empty()).collect();
            if opts.len() < 2 || opts.len() > 12 || p.question.trim().is_empty() {
                return Err(UiError::invalid("a poll needs a question and 2–12 options"));
            }
            Some(app::Poll {
                question: p.question.trim().to_owned(),
                options: opts,
                multiple_choice: p.multiple_choice,
                closes_at_ms: p.closes_at_ms,
            })
        }
        None => None,
    };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let media = private_media(one, &media_paths).await?;
    let id = one.circles().post(&circle, text.trim(), media, Vec::new(), poll).await?;
    one.save()?;
    Ok(id)
}

/// Comments.
#[tauri::command]
pub async fn circles_comment(state: S<'_>, circle: String, post: String, text: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.circles().comment(&circle, &post, text.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Reacts.
#[tauri::command]
pub async fn circles_react(state: S<'_>, circle: String, target: String, reaction: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.circles().react(&circle, &target, reaction.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Votes.
#[tauri::command]
pub async fn circles_vote(state: S<'_>, circle: String, post: String, choices: Vec<u32>) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.circles().vote(&circle, &post, choices).await?;
    one.save()?;
    Ok(id)
}

/// Deletes our post/comment.
#[tauri::command]
pub async fn circles_delete(state: S<'_>, circle: String, target: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.circles().delete(&circle, &target).await?;
    one.save()?;
    Ok(id)
}

/// Renames / describes.
#[tauri::command]
pub async fn circles_set_info(state: S<'_>, circle: String, name: String, description: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.circles().set_info(&circle, name.trim(), description.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Posts page.
#[tauri::command]
pub async fn circles_posts(state: S<'_>, circle: String, before_ms: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<CircleItemView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .circles()
        .posts(&circle, before_ms.unwrap_or(0), limit_of(limit))?
        .iter()
        .map(CircleItemView::from)
        .collect())
}

/// Comments of a post.
#[tauri::command]
pub async fn circles_comments(state: S<'_>, circle: String, post: String) -> CmdResult<Vec<CircleItemView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.circles().comments(&circle, &post)?.iter().map(CircleItemView::from).collect())
}

/// A merged item across circles.
#[derive(Debug, Clone, Serialize)]
pub struct MergedItem {
    /// Circle hex.
    pub circle: String,
    /// Item.
    pub item: CircleItemView,
}

/// Merged private timeline across all circles.
#[tauri::command]
pub async fn circles_merged(state: S<'_>, before_ms: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<MergedItem>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .circles()
        .merged(before_ms.unwrap_or(0), limit_of(limit))?
        .iter()
        .map(|(c, it)| MergedItem {
            circle: c.clone(),
            item: CircleItemView::from(it),
        })
        .collect())
}

/// Decrypts a Circle media blob to the scratch folder and returns its path.
#[tauri::command]
pub async fn circles_media_fetch(state: S<'_>, circle: String, item: String, index: usize) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    // Find the blob ref inside the item (keys never leave here).
    let posts = one.circles().posts(&circle, 0, 500)?;
    let found = posts
        .iter()
        .find(|p| p.id == item)
        .and_then(|p| p.media.get(index).cloned())
        .or_else(|| {
            posts
                .iter()
                .flat_map(|p| one.circles().comments(&circle, &p.id).unwrap_or_default())
                .find(|c| c.id == item)
                .and_then(|c| c.media.get(index).cloned())
        })
        .ok_or_else(|| UiError::not_found("media"))?;
    let att = app::MailAttachment {
        name: found.name.clone(),
        mime: found.mime.clone(),
        size: found.size,
        plaintext_hash: found.plaintext_hash.clone(),
        content_id: String::new(),
        source: Some(app::mail_attachment::Source::Blob(found.clone())),
    };
    let bytes = one.mail().attachment_bytes(&att).await?;
    let ext = mime_guess::get_mime_extensions_str(&found.mime)
        .and_then(|e| e.first())
        .copied()
        .unwrap_or("bin");
    let dir = crate::paths::tmp_dir().join("circle");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("{}-{index}.{ext}", &item[..item.len().min(12)]));
    tokio::fs::write(&p, &bytes).await?;
    Ok(p.display().to_string())
}
