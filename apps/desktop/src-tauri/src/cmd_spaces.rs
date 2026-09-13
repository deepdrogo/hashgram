//! Spaces commands over `one.spaces()`.
//!
//! Rule violations come back from the SDK as `SdkError::Invalid("space
//! rule: …")` → `UiError{code: "invalid"}`; the UI disables controls from
//! `my_role` first and shows these as toasts only when the state raced.

use std::sync::Arc;

use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::protocol::space::Member;
use hashgram_sdk::spaces::SpaceSummary;
use tauri::State;

use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::views::{SpaceContentView, SpaceSharedEntryView, SpaceStateView};

type S<'a> = State<'a, Arc<AppState>>;

fn role_of(s: &str) -> CmdResult<app::SpaceRole> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "guest" => app::SpaceRole::Guest,
        "member" => app::SpaceRole::Member,
        "admin" => app::SpaceRole::Admin,
        "owner" => app::SpaceRole::Owner,
        other => return Err(UiError::invalid(format!("unknown role {other}"))),
    })
}

/// Our spaces.
#[tauri::command]
pub async fn spaces_list(state: S<'_>) -> CmdResult<Vec<SpaceSummary>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.spaces().list()?)
}

/// Creates a Space (we become Owner).
#[tauri::command]
pub async fn spaces_create(state: S<'_>, name: String, description: String) -> CmdResult<String> {
    if name.trim().is_empty() || name.len() > 128 {
        return Err(UiError::invalid("a name is 1–128 characters"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().create(name.trim(), description.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Replayed state.
#[tauri::command]
pub async fn spaces_state(state: S<'_>, space: String) -> CmdResult<SpaceStateView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let me = one.address().to_owned();
    let st = one.spaces().state(&space)?;
    Ok(SpaceStateView::of(&st, &me))
}

/// Members by rank.
#[tauri::command]
pub async fn spaces_members(state: S<'_>, space: String) -> CmdResult<Vec<Member>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.spaces().members(&space)?)
}

/// Content page (posts, comments, announcements), newest first.
#[tauri::command]
pub async fn spaces_content(state: S<'_>, space: String, before_ms: Option<u64>, limit: Option<usize>) -> CmdResult<Vec<SpaceContentView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .spaces()
        .content(&space, before_ms.unwrap_or(0), limit.unwrap_or(100).clamp(1, 500))?
        .iter()
        .map(SpaceContentView::from)
        .collect())
}

/// Space drive listing.
#[tauri::command]
pub async fn spaces_drive(state: S<'_>, space: String) -> CmdResult<Vec<SpaceSharedEntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.spaces().drive_entries(&space)?.iter().map(SpaceSharedEntryView::from).collect())
}

/// Invites (address or name resolved here) with a role.
#[tauri::command]
pub async fn spaces_invite(state: S<'_>, space: String, member: String, role: String) -> CmdResult<()> {
    let role = role_of(&role)?;
    if role == app::SpaceRole::Owner {
        return Err(UiError::invalid("ownership is transferred with a role change, not an invite"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let a = one.people().resolve(member.trim()).await?.address;
    one.spaces().invite(&space, &a, role).await?;
    one.save()?;
    Ok(())
}

/// Removes a member (or leaves when it is us).
#[tauri::command]
pub async fn spaces_remove(state: S<'_>, space: String, address: String, reason: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.spaces().remove(&space, address.trim(), reason.trim()).await?;
    one.save()?;
    Ok(())
}

/// Changes a role (`owner` transfers ownership).
#[tauri::command]
pub async fn spaces_set_role(state: S<'_>, space: String, address: String, role: String) -> CmdResult<String> {
    let role = role_of(&role)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().set_role(&space, address.trim(), role).await?;
    one.save()?;
    Ok(id)
}

/// Updates name / description.
#[tauri::command]
pub async fn spaces_set_info(state: S<'_>, space: String, name: String, description: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().set_info(&space, name.trim(), description.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Announcement (Admin+).
#[tauri::command]
pub async fn spaces_announce(state: S<'_>, space: String, title: String, text: String) -> CmdResult<String> {
    if title.trim().is_empty() && text.trim().is_empty() {
        return Err(UiError::invalid("write a title or a text"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().announce(&space, title.trim(), text.trim(), Vec::new()).await?;
    one.save()?;
    Ok(id)
}

/// Post (Member+).
#[tauri::command]
pub async fn spaces_post(state: S<'_>, space: String, text: String) -> CmdResult<String> {
    if text.trim().is_empty() {
        return Err(UiError::invalid("write something"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().post(&space, text.trim(), Vec::new(), Vec::new()).await?;
    one.save()?;
    Ok(id)
}

/// Comment (Member+).
#[tauri::command]
pub async fn spaces_comment(state: S<'_>, space: String, post: String, text: String) -> CmdResult<String> {
    if text.trim().is_empty() {
        return Err(UiError::invalid("write something"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().comment(&space, &post, text.trim()).await?;
    one.save()?;
    Ok(id)
}

/// Shares one of our Drive entries into the Space drive.
#[tauri::command]
pub async fn spaces_share_drive(state: S<'_>, space: String, entry: String, path: String, live: bool) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().share_drive(&space, &entry, path.trim().trim_matches('/'), live).await?;
    one.save()?;
    Ok(id)
}

/// Unshares.
#[tauri::command]
pub async fn spaces_unshare_drive(state: S<'_>, space: String, share: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().unshare_drive(&space, &share).await?;
    one.save()?;
    Ok(id)
}

/// Mail to every member.
#[tauri::command]
pub async fn spaces_mail(state: S<'_>, space: String, subject: String, body: String) -> CmdResult<String> {
    if subject.trim().is_empty() && body.trim().is_empty() {
        return Err(UiError::invalid("write a subject or a body"));
    }
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.spaces().mail(&space, subject.trim(), body.trim()).await?;
    one.save()?;
    Ok(id)
}

fn space_cap(one: &mut hashgram_sdk::HashgramOne, space: &str, share: &str) -> CmdResult<app::DriveCapability> {
    let st = one.spaces().state(space)?;
    if let Some(e) = st.drive.get(share) {
        return Ok(e.capability.clone());
    }
    // Announcement / post attachments.
    for c in &st.content {
        for cap in &c.drive_refs {
            if hex::encode(&cap.share_id) == share {
                return Ok(cap.clone());
            }
        }
    }
    Err(UiError::not_found("shared entry"))
}

/// Downloads a Space drive file (by share id) to `out_path`.
#[tauri::command]
pub async fn spaces_drive_download(state: S<'_>, space: String, share: String, out_path: String) -> CmdResult<u64> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let cap = space_cap(one, &space, &share)?;
    if cap.folder {
        return Err(UiError::invalid("open the folder and download its files"));
    }
    crate::cmd_drive::download_cap_to(one, &cap, &out_path).await
}

/// Opens a Space drive file with the default application.
#[tauri::command]
pub async fn spaces_drive_open(state: S<'_>, app: tauri::AppHandle, space: String, share: String) -> CmdResult<String> {
    let (name, bytes) = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let cap = space_cap(one, &space, &share)?;
        if cap.folder {
            return Err(UiError::invalid("open the folder and open its files"));
        }
        (cap.name.clone(), one.drive().download_capability(&cap).await?)
    };
    let dir = crate::paths::tmp_dir().join("space");
    std::fs::create_dir_all(&dir)?;
    let safe: String = name
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let p = dir.join(format!("{}-{}", &share[..share.len().min(8)], if safe.trim().is_empty() { "file".to_owned() } else { safe }));
    tokio::fs::write(&p, &bytes).await?;
    crate::util::open_path(&app, &p)?;
    Ok(p.display().to_string())
}

/// Saves a Space drive file into our own Drive.
#[tauri::command]
pub async fn spaces_drive_save(state: S<'_>, space: String, share: String, parent: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let cap = space_cap(one, &space, &share)?;
    Ok(one.drive().save_capability(&cap, &parent)?)
}

/// Lists the entries of a Space folder share.
#[tauri::command]
pub async fn spaces_drive_folder_list(state: S<'_>, space: String, share: String) -> CmdResult<Vec<crate::views::FolderEntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let cap = space_cap(one, &space, &share)?;
    let fm = one.drive().shared_folder_entries(&cap).await?;
    Ok(fm.entries.iter().map(crate::views::FolderEntryView::from).collect())
}
