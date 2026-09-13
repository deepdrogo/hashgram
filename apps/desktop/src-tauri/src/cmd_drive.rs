//! Drive commands over `one.drive()`.
//!
//! Files are read and written on the Rust side; the webview only names
//! paths (chosen with the dialog plugin). Uploads above the v1 cap are
//! refused with a clear message: the SDK's `upload` takes the whole file
//! in memory, and a streaming path (`SegmentEncryptor` + chunked blob
//! upload) is SDK work noted in `docs/HASHGRAM_ONE_AI_HANDOFF.md`.

use std::sync::Arc;

use hashgram_sdk::drive::DriveUsage;
use hashgram_sdk::protocol::drive::EntryView;
use hashgram_sdk::protocol::pb as app;
use hashgram_sdk::HashgramOne;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::error::{CmdResult, UiError};
use crate::state::AppState;
use crate::views::{CapabilityView, FolderEntryView, ShareRecordView, SharedWithMeView, VersionView};

type S<'a> = State<'a, Arc<AppState>>;

/// Largest file `drive_upload` accepts in v1.
pub const MAX_UPLOAD_BYTES: u64 = 256 * 1024 * 1024;

/// Upload / download progress for the webview.
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    /// Operation id (client-chosen).
    pub op: String,
    /// `reading` | `encrypting` | `uploading` | `done` | `failed`.
    pub stage: &'static str,
    /// Bytes so far.
    pub done: u64,
    /// Total bytes.
    pub total: u64,
    /// Message on failure.
    pub message: String,
}

fn progress(app: &AppHandle, op: &str, stage: &'static str, done: u64, total: u64, message: &str) {
    let _ = app.emit(
        "drive:progress",
        Progress {
            op: op.to_owned(),
            stage,
            done,
            total,
            message: message.to_owned(),
        },
    );
}

/// Children of a folder (`""` = root).
#[tauri::command]
pub async fn drive_list(state: S<'_>, parent: String) -> CmdResult<Vec<EntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().list(&parent)?)
}

/// Trash.
#[tauri::command]
pub async fn drive_trash_list(state: S<'_>) -> CmdResult<Vec<EntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().trash_list())
}

/// Starred.
#[tauri::command]
pub async fn drive_starred(state: S<'_>) -> CmdResult<Vec<EntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().starred())
}

/// Name search.
#[tauri::command]
pub async fn drive_search(state: S<'_>, q: String, limit: Option<usize>) -> CmdResult<Vec<EntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().search(&q, limit.unwrap_or(100).clamp(1, 500)))
}

/// One entry.
#[tauri::command]
pub async fn drive_entry(state: S<'_>, id: String) -> CmdResult<Option<EntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().entry(&id)?)
}

/// Versions of a file, oldest first (the last one is current).
#[tauri::command]
pub async fn drive_versions(state: S<'_>, id: String) -> CmdResult<Vec<VersionView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().versions(&id)?.iter().map(VersionView::from).collect())
}

/// Usage.
#[tauri::command]
pub async fn drive_usage(state: S<'_>) -> CmdResult<DriveUsage> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().usage())
}

/// Resolves `/a/b/c` to an id.
#[tauri::command]
pub async fn drive_resolve_path(state: S<'_>, path: String) -> CmdResult<Option<String>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().resolve_path(&path))
}

fn check_name(name: &str) -> CmdResult<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 255 || n.contains('/') || n.contains('\\') {
        return Err(UiError::invalid("a name is 1–255 characters without slashes"));
    }
    Ok(n.to_owned())
}

/// Creates a folder.
#[tauri::command]
pub async fn drive_mkdir(state: S<'_>, parent: String, name: String) -> CmdResult<String> {
    let name = check_name(&name)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.drive().mkdir(&parent, &name)?;
    Ok(id)
}

async fn read_capped(path: &str) -> CmdResult<(String, String, Vec<u8>)> {
    let p = std::path::PathBuf::from(path);
    let meta = tokio::fs::metadata(&p).await?;
    if !meta.is_file() {
        return Err(UiError::invalid("not a file"));
    }
    if meta.len() > MAX_UPLOAD_BYTES {
        return Err(UiError::invalid(format!(
            "files over {} MiB are not supported in this version",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        )));
    }
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_owned();
    let mime = mime_guess::from_path(&p).first_or_octet_stream().to_string();
    let bytes = tokio::fs::read(&p).await?;
    Ok((name, mime, bytes))
}

/// Uploads a file from disk into a folder. Progress on `drive:progress`.
#[tauri::command]
pub async fn drive_upload(state: S<'_>, app: AppHandle, parent: String, path: String, op: Option<String>) -> CmdResult<String> {
    let op = op.unwrap_or_default();
    progress(&app, &op, "reading", 0, 0, "");
    let (name, mime, bytes) = match read_capped(&path).await {
        Ok(x) => x,
        Err(e) => {
            progress(&app, &op, "failed", 0, 0, &e.message);
            return Err(e);
        }
    };
    let total = bytes.len() as u64;
    progress(&app, &op, "uploading", 0, total, "");
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    match one.drive().upload(&parent, &name, &mime, &bytes).await {
        Ok(id) => {
            let _ = one.save();
            progress(&app, &op, "done", total, total, "");
            Ok(id)
        }
        Err(e) => {
            let e = UiError::from(e);
            progress(&app, &op, "failed", 0, total, &e.message);
            Err(e)
        }
    }
}

/// Uploads small dropped bytes (base64, ≤ 32 MiB).
#[tauri::command]
pub async fn drive_upload_bytes(
    state: S<'_>,
    parent: String,
    name: String,
    mime: String,
    base64: String,
) -> CmdResult<String> {
    let name = check_name(&name)?;
    let bytes = crate::util::base64_decode(&base64).ok_or_else(|| UiError::invalid("bad base64"))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(UiError::invalid("use the file picker for files over 32 MiB"));
    }
    let mime = if mime.trim().is_empty() {
        mime_guess::from_path(&name).first_or_octet_stream().to_string()
    } else {
        mime
    };
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let id = one.drive().upload(&parent, &name, &mime, &bytes).await?;
    one.save()?;
    Ok(id)
}

/// Replaces a file's content with a new version from disk.
#[tauri::command]
pub async fn drive_update(state: S<'_>, app: AppHandle, id: String, path: String, note: Option<String>, op: Option<String>) -> CmdResult<u64> {
    let op = op.unwrap_or_default();
    let (_, _, bytes) = read_capped(&path).await?;
    let total = bytes.len() as u64;
    progress(&app, &op, "uploading", 0, total, "");
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    match one.drive().update(&id, &bytes, note.as_deref().unwrap_or("")).await {
        Ok(no) => {
            let _ = one.save();
            progress(&app, &op, "done", total, total, "");
            Ok(no)
        }
        Err(e) => {
            let e = UiError::from(e);
            progress(&app, &op, "failed", 0, total, &e.message);
            Err(e)
        }
    }
}

/// Downloads and decrypts a file to `out_path`.
#[tauri::command]
pub async fn drive_download(state: S<'_>, id: String, out_path: String) -> CmdResult<u64> {
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        one.drive().download(&id).await?
    };
    tokio::fs::write(&out_path, &bytes).await?;
    Ok(bytes.len() as u64)
}

/// Downloads a specific version to `out_path`.
#[tauri::command]
pub async fn drive_download_version(state: S<'_>, id: String, version_no: u64, out_path: String) -> CmdResult<u64> {
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        one.drive().download_version(&id, version_no).await?
    };
    tokio::fs::write(&out_path, &bytes).await?;
    Ok(bytes.len() as u64)
}

fn scratch(id: &str, name: &str) -> CmdResult<std::path::PathBuf> {
    let dir = crate::paths::tmp_dir().join(&id[..id.len().min(12)]);
    std::fs::create_dir_all(&dir)?;
    let safe: String = name
        .chars()
        .map(|c| if c.is_control() || "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    Ok(dir.join(if safe.trim().is_empty() { "file".to_owned() } else { safe }))
}

/// Decrypts to the scratch folder and opens with the default application.
#[tauri::command]
pub async fn drive_open(state: S<'_>, app: AppHandle, id: String) -> CmdResult<String> {
    let (name, bytes) = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let e = one.drive().entry(&id)?.ok_or_else(|| UiError::not_found("entry"))?;
        (e.name, one.drive().download(&id).await?)
    };
    let p = scratch(&id, &name)?;
    tokio::fs::write(&p, &bytes).await?;
    crate::util::open_path(&app, &p)?;
    Ok(p.display().to_string())
}

/// Image preview bytes (base64) for the Drive grid; ≤ 8 MiB images only.
#[tauri::command]
pub async fn drive_preview(state: S<'_>, id: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let e = one.drive().entry(&id)?.ok_or_else(|| UiError::not_found("entry"))?;
    if !e.mime.starts_with("image/") || e.size > 8 * 1024 * 1024 {
        return Err(UiError::invalid("preview only for images up to 8 MiB"));
    }
    let bytes = one.drive().download(&id).await?;
    Ok(crate::util::base64_encode(&bytes))
}

/// Rename.
#[tauri::command]
pub async fn drive_rename(state: S<'_>, id: String, name: String) -> CmdResult<()> {
    let name = check_name(&name)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().rename(&id, &name)?;
    Ok(())
}

/// Move.
#[tauri::command]
pub async fn drive_move(state: S<'_>, id: String, new_parent: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().mv(&id, &new_parent)?;
    Ok(())
}

/// Copy (no re-upload).
#[tauri::command]
pub async fn drive_copy(state: S<'_>, id: String, new_parent: String, name: String) -> CmdResult<String> {
    let name = check_name(&name)?;
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().copy(&id, &new_parent, &name)?)
}

/// Trash.
#[tauri::command]
pub async fn drive_trash(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().trash(&id)?;
    Ok(())
}

/// Restore from trash.
#[tauri::command]
pub async fn drive_restore(state: S<'_>, id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().restore(&id)?;
    Ok(())
}

/// Permanently delete (must be trashed). Returns entries removed.
#[tauri::command]
pub async fn drive_delete(state: S<'_>, id: String) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().delete(&id)?)
}

/// Empties the trash.
#[tauri::command]
pub async fn drive_empty_trash(state: S<'_>) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().empty_trash()?)
}

/// Star.
#[tauri::command]
pub async fn drive_star(state: S<'_>, id: String, on: bool) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().star(&id, on)?;
    Ok(())
}

/// Restores an older version as the current one.
#[tauri::command]
pub async fn drive_restore_version(state: S<'_>, id: String, version_no: u64) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().restore_version(&id, version_no).await?;
    Ok(())
}

/// Re-seals a file under a fresh key and revokes its live shares.
#[tauri::command]
pub async fn drive_rekey(state: S<'_>, id: String) -> CmdResult<u64> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let no = one.drive().rekey(&id).await?;
    one.save()?;
    Ok(no)
}

/// Re-keys every file (slow). Returns the count.
#[tauri::command]
pub async fn drive_rekey_all(state: S<'_>) -> CmdResult<usize> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let n = one.drive().rekey_all().await?;
    one.save()?;
    Ok(n)
}

/// Publishes the manifest now (the sync engine does it too when dirty).
#[tauri::command]
pub async fn drive_commit(state: S<'_>) -> CmdResult<u64> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let r = one.drive().commit().await?;
    one.save()?;
    Ok(r)
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

/// Shares an entry with addresses (resolved here). Returns the capability
/// view.
#[tauri::command]
pub async fn drive_share(
    state: S<'_>,
    id: String,
    grantees: Vec<String>,
    live: bool,
    note: Option<String>,
) -> CmdResult<CapabilityView> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let mut addrs = Vec::new();
    for gr in grantees {
        let r = one.people().resolve(gr.trim()).await?;
        addrs.push(r.address);
    }
    if addrs.is_empty() {
        return Err(UiError::invalid("choose at least one person"));
    }
    let cap = one
        .drive()
        .share(
            &id,
            &addrs.join(","),
            if live {
                app::DriveShareMode::Live
            } else {
                app::DriveShareMode::Snapshot
            },
            app::DrivePermission::Read,
            note.as_deref().unwrap_or("").trim(),
        )
        .await?;
    one.save()?;
    Ok(CapabilityView::from(&cap))
}

/// Revokes a share.
#[tauri::command]
pub async fn drive_revoke(state: S<'_>, share_id: String) -> CmdResult<()> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    one.drive().revoke(&share_id).await?;
    one.save()?;
    Ok(())
}

/// Shares we granted (optionally for one entry).
#[tauri::command]
pub async fn drive_shares(state: S<'_>, entry_id: Option<String>) -> CmdResult<Vec<ShareRecordView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one
        .drive()
        .shares()
        .iter()
        .filter(|s| entry_id.as_ref().map(|e| hex::encode(&s.entry_id) == *e).unwrap_or(true))
        .map(ShareRecordView::from)
        .collect())
}

/// Capabilities shared with us.
#[tauri::command]
pub async fn drive_shared_with_me(state: S<'_>) -> CmdResult<Vec<SharedWithMeView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    Ok(one.drive().shared_with_me()?.iter().map(SharedWithMeView::from).collect())
}

fn shared_cap(one: &mut HashgramOne, share_id: &str) -> CmdResult<app::DriveCapability> {
    one.drive()
        .shared_with_me()?
        .into_iter()
        .find(|s| hex::encode(&s.capability.share_id) == share_id)
        .map(|s| s.capability)
        .ok_or_else(|| UiError::not_found("share"))
}

/// Downloads a shared file to `out_path`.
#[tauri::command]
pub async fn drive_shared_download(state: S<'_>, share_id: String, out_path: String) -> CmdResult<u64> {
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let cap = shared_cap(one, &share_id)?;
        if cap.folder {
            return Err(UiError::invalid("open the folder share and download its files"));
        }
        one.drive().download_capability(&cap).await?
    };
    tokio::fs::write(&out_path, &bytes).await?;
    Ok(bytes.len() as u64)
}

/// Opens a shared file with the default application.
#[tauri::command]
pub async fn drive_shared_open(state: S<'_>, app: AppHandle, share_id: String) -> CmdResult<String> {
    let (name, bytes) = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let cap = shared_cap(one, &share_id)?;
        if cap.folder {
            return Err(UiError::invalid("open the folder share and open its files"));
        }
        (cap.name.clone(), one.drive().download_capability(&cap).await?)
    };
    let p = scratch(&share_id, &name)?;
    tokio::fs::write(&p, &bytes).await?;
    crate::util::open_path(&app, &p)?;
    Ok(p.display().to_string())
}

/// Saves a shared file into our Drive (no re-upload).
#[tauri::command]
pub async fn drive_shared_save(state: S<'_>, share_id: String, parent: String) -> CmdResult<String> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let cap = shared_cap(one, &share_id)?;
    let id = one.drive().save_capability(&cap, &parent)?;
    Ok(id)
}

/// Lists a shared folder's entries.
#[tauri::command]
pub async fn drive_shared_folder_list(state: S<'_>, share_id: String) -> CmdResult<Vec<FolderEntryView>> {
    let mut g = state.one.lock().await;
    let one = AppState::unlocked(&mut g)?;
    let cap = shared_cap(one, &share_id)?;
    let fm = one.drive().shared_folder_entries(&cap).await?;
    Ok(fm.entries.iter().map(FolderEntryView::from).collect())
}

/// Downloads one file of a shared folder to `out_path`.
#[tauri::command]
pub async fn drive_shared_folder_download(
    state: S<'_>,
    share_id: String,
    entry_id: String,
    out_path: String,
) -> CmdResult<u64> {
    let bytes = {
        let mut g = state.one.lock().await;
        let one = AppState::unlocked(&mut g)?;
        let cap = shared_cap(one, &share_id)?;
        let fm = one.drive().shared_folder_entries(&cap).await?;
        let e = fm
            .entries
            .iter()
            .find(|e| hex::encode(&e.id) == entry_id)
            .ok_or_else(|| UiError::not_found("entry in shared folder"))?;
        let r = e
            .current
            .as_ref()
            .ok_or_else(|| UiError::invalid("that entry is a folder"))?;
        one.drive().download_object(r).await?
    };
    tokio::fs::write(&out_path, &bytes).await?;
    Ok(bytes.len() as u64)
}

/// Capabilities referenced from a Space or Circle post (by share id inside
/// the Space state) can be opened the same way; resolved by the Spaces
/// module.
pub(crate) async fn download_cap_to(one: &mut HashgramOne, cap: &app::DriveCapability, out_path: &str) -> CmdResult<u64> {
    let bytes = one.drive().download_capability(cap).await?;
    tokio::fs::write(out_path, &bytes).await?;
    Ok(bytes.len() as u64)
}
