//! People commands over `one.people()`.

use std::sync::Arc;

use hashgram_sdk::people::{Profile, Resolved};
use hashgram_sdk::protocol::pb::ContactRecord;
use tauri::State;

use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::views::CardView;

type S<'a> = State<'a, Arc<AppState>>;

/// Resolves an address, `@name`, `name@hashgram.io` or bare name.
#[tauri::command]
pub async fn people_resolve(state: S<'_>, input: String) -> CmdResult<Resolved> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().resolve(input.trim()).await?)
}

/// Public profile (social log) with our contact flags.
#[tauri::command]
pub async fn people_profile(state: S<'_>, address: String) -> CmdResult<Profile> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().profile(address.trim()).await?)
}

/// Sends a contact request.
#[tauri::command]
pub async fn people_request(state: S<'_>, input: String, message: String) -> CmdResult<Resolved> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let r = one.people().request(input.trim(), message.trim()).await?;
    one.save()?;
    Ok(r)
}

/// Accepts or rejects an incoming request.
#[tauri::command]
pub async fn people_respond(state: S<'_>, address: String, accept: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().respond(address.trim(), accept).await?;
    one.save()?;
    Ok(())
}

/// Removes a friend.
#[tauri::command]
pub async fn people_remove(state: S<'_>, address: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().remove(address.trim())?;
    Ok(())
}

/// Blocks.
#[tauri::command]
pub async fn people_block(state: S<'_>, address: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().block(address.trim())?;
    Ok(())
}

/// Unblocks.
#[tauri::command]
pub async fn people_unblock(state: S<'_>, address: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().unblock(address.trim())?;
    Ok(())
}

/// Mute on/off.
#[tauri::command]
pub async fn people_mute(state: S<'_>, address: String, on: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().mute(address.trim(), on)?;
    Ok(())
}

/// Trust on/off.
#[tauri::command]
pub async fn people_trust(state: S<'_>, address: String, on: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().trust(address.trim(), on)?;
    Ok(())
}

/// Follow / unfollow (public event).
#[tauri::command]
pub async fn people_follow(state: S<'_>, address: String, on: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().follow(address.trim(), on).await?;
    one.save()?;
    Ok(())
}

/// A list: `friends` | `incoming` | `outgoing` | `blocked` | `all` |
/// `following`.
#[tauri::command]
pub async fn people_list(state: S<'_>, which: String) -> CmdResult<Vec<ContactRecord>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let p = one.people();
    let mut v = match which.as_str() {
        "friends" => p.friends(),
        "incoming" => p.incoming_requests(),
        "outgoing" => p.outgoing_requests(),
        "blocked" => p.blocked(),
        "all" => p.all(),
        "following" => p
            .all()
            .into_iter()
            .filter(|r| r.states.iter().any(|s| s == hashgram_sdk::protocol::people::FOLLOWING))
            .collect(),
        other => return Err(UiError::invalid(format!("unknown list {other}"))),
    };
    v.sort_by(|a, b| {
        let ka = if a.display_name.is_empty() { &a.username } else { &a.display_name };
        let kb = if b.display_name.is_empty() { &b.username } else { &b.display_name };
        ka.to_lowercase().cmp(&kb.to_lowercase()).then_with(|| a.address.cmp(&b.address))
    });
    Ok(v)
}

/// Local search over the contact book.
#[tauri::command]
pub async fn people_search_local(state: S<'_>, q: String) -> CmdResult<Vec<ContactRecord>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().search_local(q.trim()))
}

/// Our private display name (mail headers, cards).
#[tauri::command]
pub async fn people_set_display_name(state: S<'_>, name: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().set_my_display_name(name.trim())?;
    Ok(())
}

/// Our private display name.
#[tauri::command]
pub async fn people_my_display_name(state: S<'_>) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().my_display_name())
}

/// Sends our card to a contact (optionally disclosing the wallet address).
#[tauri::command]
pub async fn people_send_card(state: S<'_>, address: String, bio: String, disclose_wallet: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.people().send_card(address.trim(), bio.trim(), disclose_wallet).await?;
    one.save()?;
    Ok(())
}

/// The last card a contact sent us.
#[tauri::command]
pub async fn people_card_of(state: S<'_>, address: String) -> CmdResult<Option<CardView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().card_of(address.trim())?.as_ref().map(CardView::from))
}

/// Username of an address (cached).
#[tauri::command]
pub async fn people_username_of(state: S<'_>, address: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.people().username_of(address.trim()).await?)
}
