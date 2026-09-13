//! OS notifications from sync events. Nothing leaves the device; the toast
//! never carries a subject or a body — only who and what kind.

use std::sync::Arc;

use hashgram_sdk::sync::SyncEvent;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

fn short(addr: &str) -> String {
    if addr.len() > 16 {
        format!("{}…{}", &addr[..10], &addr[addr.len() - 4..])
    } else {
        addr.to_owned()
    }
}

/// Sends a notification for events the settings allow.
pub async fn on_sync_event(app: &AppHandle, state: &Arc<AppState>, ev: &SyncEvent) {
    let prefs = state.settings.read().await.notifications.clone();
    let (title, body) = match ev {
        SyncEvent::NewMail { id, folder } => {
            let inbox = folder == hashgram_sdk::mail::folder::INBOX;
            let requests = folder == hashgram_sdk::mail::folder::REQUESTS;
            if (inbox && !prefs.mail) || (requests && !prefs.requests) || !(inbox || requests) {
                return;
            }
            // Who, never what: the sender's name or short address.
            let who = {
                let mut g = state.one.lock().await;
                match g.as_mut() {
                    Some(one) => {
                        let rec = one.mail().get(id).ok().flatten();
                        match rec {
                            Some(r) => {
                                let sender = r.authenticated_sender.clone();
                                let name = one.people().username_of(&sender).await.unwrap_or_default();
                                if name.is_empty() {
                                    short(&sender)
                                } else {
                                    format!("@{name}")
                                }
                            }
                            None => "someone".to_owned(),
                        }
                    }
                    None => return,
                }
            };
            if inbox {
                ("New mail".to_owned(), format!("New mail from {who}"))
            } else {
                ("New request".to_owned(), format!("{who} wrote to you for the first time"))
            }
        }
        SyncEvent::ContactsChanged => {
            if !prefs.requests {
                return;
            }
            ("People".to_owned(), "Your contacts changed".to_owned())
        }
        SyncEvent::SpaceActivity(_) => {
            if !prefs.spaces {
                return;
            }
            ("Spaces".to_owned(), "New activity in a Space".to_owned())
        }
        SyncEvent::CircleActivity(_) => {
            if !prefs.circles {
                return;
            }
            ("Circles".to_owned(), "New activity in a Circle".to_owned())
        }
        _ => return,
    };
    // Only when the window is not focused: a visible app needs no toast.
    let focused = app
        .webview_windows()
        .get("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::debug!(error = %e, "notification not shown");
    }
}
