//! Hashwall commands: Explore pages from the nearest nodes, walls (open
//! topics), the network digest, my profile, avatars and the rich list.
//!
//! Everything here is paged or cached so the UI never asks the network for
//! "everything": Explore is one page of posts at a time from the node with
//! the lowest measured round-trip, the digest is one small message a node
//! computes and caches, profiles are cached locally for a quarter of an
//! hour, avatars are fetched once into the cache folder.

use std::sync::Arc;

use hashgram_sdk::feed::{Digest, ExplorePage, ExploreQuery, MyActivity, PostThread, WallInfo};
use hashgram_sdk::network::Holders;
use hashgram_sdk::people::Profile;
use hashgram_sdk::provider::ProviderStatus;
use hashgram_sdk::wallet::Balance;
use serde::Serialize;
use tauri::State;

use crate::error::{CmdResult, UiError};
use crate::state::AppState;

type S<'a> = State<'a, Arc<AppState>>;

/// Profiles older than this are refreshed from the network.
const PROFILE_MAX_AGE_SECS: u64 = 15 * 60;
/// Page size bounds for remote timelines.
fn page_limit(l: Option<u32>) -> u32 {
    l.unwrap_or(20).clamp(1, 100)
}

fn check_hex_id(field: &str, h: &str) -> CmdResult<String> {
    let h = h.trim().to_ascii_lowercase();
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(UiError::invalid(format!("{field}: not an id")));
    }
    Ok(h)
}

// ---------------------------------------------------------------------------
// Explore
// ---------------------------------------------------------------------------

/// One page of public posts from the nearest node (peer-to-peer, no
/// indexer). `tag` narrows to a hashtag.
#[tauri::command]
pub async fn hashwall_explore(
    state: S<'_>,
    before: Option<u64>,
    limit: Option<u32>,
    tag: Option<String>,
) -> CmdResult<ExplorePage> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let page = one
        .feed()
        .explore(&ExploreQuery {
            before: before.unwrap_or(0),
            limit: page_limit(limit),
            hashtag: tag.unwrap_or_default(),
            ..Default::default()
        })
        .await?;
    Ok(page)
}

/// What is active on the network, from the nearest node: most active
/// authors, hashtags and walls over `window_secs` (default a week).
#[tauri::command]
pub async fn hashwall_digest(state: S<'_>, window_secs: Option<u64>, limit: Option<u32>) -> CmdResult<Digest> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .feed()
        .digest(window_secs.unwrap_or(0), limit.unwrap_or(20).clamp(1, 50))
        .await?)
}

/// A post with comments and reactions, pulled from the network so posts
/// discovered in Explore open with their engagement.
#[tauri::command]
pub async fn hashwall_thread(state: S<'_>, post: String) -> CmdResult<Option<PostThread>> {
    let post = check_hex_id("post", &post)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    match one.feed().thread_fetch(&post).await {
        Ok(t) => Ok(t),
        // Offline or no node answered: whatever the cache has.
        Err(_) => Ok(one.feed().thread(&post)?),
    }
}

// ---------------------------------------------------------------------------
// Walls
// ---------------------------------------------------------------------------

/// Opens a wall.
#[tauri::command]
pub async fn walls_create(state: S<'_>, name: String, description: String, open_posting: bool) -> CmdResult<WallInfo> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let w = one.feed().create_wall(&name, &description, open_posting).await?;
    one.save()?;
    Ok(w)
}

/// A wall's description.
#[tauri::command]
pub async fn walls_info(state: S<'_>, wall: String) -> CmdResult<WallInfo> {
    let wall = check_hex_id("wall", &wall)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().wall_info(&wall).await?)
}

/// One page of a wall's posts.
#[tauri::command]
pub async fn walls_page(state: S<'_>, wall: String, before: Option<u64>, limit: Option<u32>) -> CmdResult<ExplorePage> {
    let wall = check_hex_id("wall", &wall)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().wall_page(&wall, before.unwrap_or(0), page_limit(limit)).await?)
}

/// Pins or unpins a wall in the sidebar.
#[tauri::command]
pub async fn walls_pin(state: S<'_>, wall: String, on: bool) -> CmdResult<Vec<WallInfo>> {
    let wall = check_hex_id("wall", &wall)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    if on {
        // Make sure we hold its description before pinning.
        one.feed().wall_info(&wall).await?;
    }
    one.feed().pin_wall(&wall, on)?;
    Ok(one.feed().pinned_walls())
}

/// Pinned walls.
#[tauri::command]
pub async fn walls_pinned(state: S<'_>) -> CmdResult<Vec<WallInfo>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.feed().pinned_walls())
}

// ---------------------------------------------------------------------------
// People: cached profiles and avatars
// ---------------------------------------------------------------------------

/// A public profile from the local cache (≤ 15 min old) or the network.
#[tauri::command]
pub async fn people_profile_cached(state: S<'_>, address: String) -> CmdResult<Profile> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .people()
        .profile_cached(address.trim(), PROFILE_MAX_AGE_SECS)
        .await?)
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("webp")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Fetches an avatar blob by CID into the cache folder and returns its
/// path. The bytes are sniffed: anything that is not an image is refused,
/// whatever the author declared.
#[tauri::command]
pub async fn people_avatar(state: S<'_>, cid: String) -> CmdResult<String> {
    let cid_hex = check_hex_id("cid", &cid)?;
    let dir = crate::paths::sdk_paths().cache().join("avatars");
    std::fs::create_dir_all(&dir)?;
    for ext in ["png", "jpg", "gif", "webp", "bmp"] {
        let p = dir.join(format!("{cid_hex}.{ext}"));
        if p.exists() {
            return Ok(p.display().to_string());
        }
    }
    let cid_bytes = hex::decode(&cid_hex).map_err(|_| UiError::invalid("cid"))?;
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let (ct, _m, _p) = hashgram_sdk::blob::download(&one.link, &cid_bytes, None).await?;
        ct
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(UiError::invalid("avatar over 8 MiB"));
    }
    let ext = image_extension(&bytes).ok_or_else(|| UiError::invalid("avatar is not an image"))?;
    let p = dir.join(format!("{cid_hex}.{ext}"));
    tokio::fs::write(&p, &bytes).await?;
    Ok(p.display().to_string())
}

// ---------------------------------------------------------------------------
// My profile
// ---------------------------------------------------------------------------

/// The profile page: who I am on the network, what I hold, what I did.
#[derive(Debug, Clone, Serialize)]
pub struct MyProfile {
    /// Address.
    pub address: String,
    /// On-chain username, if any.
    pub username: String,
    /// Display name from my latest PROFILE_UPDATE.
    pub display_name: String,
    /// Bio from my latest PROFILE_UPDATE.
    pub bio: String,
    /// Avatar CID (hex), empty when none.
    pub avatar_cid: String,
    /// Balance, when the chain answered.
    pub balance: Option<Balance>,
    /// Public activity counted from my event chain.
    pub activity: MyActivity,
    /// Whether the network's count of my events was refreshed just now.
    pub refreshed: bool,
    /// Pinned walls.
    pub walls: Vec<WallInfo>,
    /// Contacts marked friend.
    pub friends: usize,
    /// Addresses I follow.
    pub following: usize,
}

/// My profile. Refreshes my own event chain from the network first (a few
/// pages at most) so the counts reflect what the network holds.
#[tauri::command]
pub async fn profile_me(state: S<'_>) -> CmdResult<MyProfile> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let me = one.address().to_owned();
    let mut refreshed = false;
    for _ in 0..5 {
        match one.feed().refresh_author(&me, 200).await {
            Ok(n) => {
                refreshed = true;
                if n < 200 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let profile = one
        .people()
        .profile_cached(&me, PROFILE_MAX_AGE_SECS)
        .await
        .unwrap_or_default();
    let balance = one.wallet().balance(None).await.ok();
    let activity = one.feed().my_activity()?;
    let walls = one.feed().pinned_walls();
    let friends = one.people().friends().len();
    let following = one.feed().follows().len();
    let username = if profile.username.is_empty() {
        one.people().username_of(&me).await.unwrap_or_default()
    } else {
        profile.username
    };
    Ok(MyProfile {
        address: me,
        username,
        display_name: profile.display_name,
        bio: profile.bio,
        avatar_cid: profile.avatar_cid,
        balance,
        activity,
        refreshed,
        walls,
        friends,
        following,
    })
}

/// Everything I did, newest first (posts, comments, reactions, follows…).
#[tauri::command]
pub async fn profile_my_events(state: S<'_>, before: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<hashgram_sdk::feed::FeedItem>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .feed()
        .my_events(before.unwrap_or(0), limit.unwrap_or(50).clamp(1, 200))?)
}

// ---------------------------------------------------------------------------
// Network: rich list and providers without an indexer
// ---------------------------------------------------------------------------

/// Top HASH holders read from the chain's bank module through the P2P
/// relay. Slow-ish (walks all accounts in pages of 1000) but needs no
/// indexer; the UI caches it for the session.
#[tauri::command]
pub async fn network_holders(state: S<'_>, limit: Option<u32>) -> CmdResult<Holders> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .network_api()
        .holders_from_chain(limit.unwrap_or(25))
        .await?)
}

/// Registered providers from the chain, bond-heaviest first.
#[tauri::command]
pub async fn network_providers(state: S<'_>) -> CmdResult<Vec<ProviderStatus>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut list = one.provider().list().await?;
    list.sort_by(|a, b| {
        let ba = a.bond_uhash.parse::<u128>().unwrap_or(0);
        let bb = b.bond_uhash.parse::<u128>().unwrap_or(0);
        bb.cmp(&ba)
    });
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_image_bytes() {
        assert_eq!(image_extension(&[0x89, b'P', b'N', b'G', 0, 0, 0, 0]), Some("png"));
        assert_eq!(image_extension(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(image_extension(b"GIF89a"), Some("gif"));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        webp.extend_from_slice(&[0; 8]);
        assert_eq!(image_extension(&webp), Some("webp"));
        assert_eq!(image_extension(b"<html>"), None);
        assert_eq!(image_extension(b"MZ\x90"), None, "executables are not avatars");
    }

    #[test]
    fn ids_are_64_hex_chars() {
        assert!(check_hex_id("x", &"ab".repeat(32)).is_ok());
        assert!(check_hex_id("x", &"AB".repeat(32)).is_ok());
        assert!(check_hex_id("x", "abc").is_err());
        assert!(check_hex_id("x", &"zz".repeat(32)).is_err());
    }
}
