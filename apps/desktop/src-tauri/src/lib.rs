//! Hashgram One for Windows — the Rust side of the Tauri application.
//!
//! Everything that is not pixels lives here, and everything cryptographic
//! lives below here, in `hashgram_sdk::HashgramOne`. The commands in the
//! `cmd_*` modules take ids and plain inputs, lock the facade, call the
//! SDK, save, and return views (`views.rs`) that carry no key material.
//! The frontend (SolidJS in WebView2) calls those commands and listens for
//! `sync:phase`, `sync:event`, `session:*`, `net:changed`, `tx:update`,
//! `drive:progress` and `deep-link`.

#![cfg_attr(not(test), forbid(clippy::unwrap_used))]
// Tests may unwrap and index: a panic there is a failed test, not a crash.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::integer_division
    )
)]
// Unit conversions for display (bytes → MiB, ms → s) truncate on purpose;
// Tauri commands mirror form fields one-to-one.
#![allow(clippy::integer_division, clippy::too_many_arguments)]

pub mod chain_proxy;
pub mod cmd_drive;
pub mod cmd_earn;
pub mod cmd_feed;
pub mod cmd_hashwall;
pub mod cmd_identity;
pub mod cmd_mail;
pub mod cmd_network;
pub mod cmd_people;
pub mod cmd_settings;
pub mod cmd_spaces;
pub mod cmd_sync;
pub mod cmd_wallet;
pub mod crypto;
pub mod db;
pub mod error;
pub mod help;
pub mod node_manager;
pub mod notify;
pub mod paths;
pub mod perf;
pub mod session;
pub mod settings;
pub mod state;
pub mod tx;
pub mod util;
pub mod views;
pub mod winsec;

use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::state::AppState;

/// A deep link handed to the webview.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeepLink {
    /// `hashgram://…`.
    pub url: String,
}

fn init_logging(perf: Arc<perf::PerfStore>, level: &str) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!(
            "{level},libp2p_gossipsub=warn,libp2p_kad=warn,libp2p_swarm=warn,quinn=warn,quinn_udp=error,hyper=warn,hickory_proto=warn,hickory_resolver=warn"
        ))
    });
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact();
    // A rotating file under logs/: one file per day, the last 7 kept. The
    // logging policy (never subjects, bodies, contact addresses or key
    // material) is enforced by the SDK's and this crate's log lines, which
    // carry ids, counts and error kinds only.
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(7)
        .filename_prefix("hashgram")
        .filename_suffix("log")
        .build(paths::logs_dir())
        .ok();
    let (file_layer, guard) = match file {
        Some(f) => {
            let (nb, guard) = tracing_appender::non_blocking(f);
            (
                Some(
                    tracing_subscriber::fmt::layer()
                        .with_writer(nb)
                        .with_ansi(false)
                        .with_target(false)
                        .compact(),
                ),
                Some(guard),
            )
        }
        None => (None, None),
    };
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file_layer)
        .with(perf::PerfLayer::new(perf))
        .try_init();
    guard
}

fn build_state() -> Result<(Arc<AppState>, Option<tracing_appender::non_blocking::WorkerGuard>), String> {
    let data = paths::ensure_dirs().map_err(|e| e.to_string())?;
    let settings = settings::Settings::load(&paths::settings_path());
    let perf = Arc::new(perf::PerfStore::default());
    let guard = init_logging(perf.clone(), &settings.advanced.log_level);
    tracing::info!(dir = %data.display(), "Hashgram One for Windows starting");
    let db = Arc::new(db::Db::open(&paths::db_path())?);
    let _ = std::fs::remove_dir_all(paths::tmp_dir());
    let _ = std::fs::create_dir_all(paths::tmp_dir());
    Ok((
        Arc::new(AppState {
            settings: tokio::sync::RwLock::new(settings),
            one: tokio::sync::Mutex::new(None),
            session: tokio::sync::RwLock::new(None),
            link: tokio::sync::RwLock::new(None),
            link_error: tokio::sync::RwLock::new(None),
            pending_mnemonic: tokio::sync::Mutex::new(None),
            db,
            perf,
            sync: tokio::sync::RwLock::new(state::SyncStatus::default()),
            sync_task: tokio::sync::Mutex::new(None),
            pending_tx: tokio::sync::Mutex::new(Vec::new()),
            sync_wake: tokio::sync::Notify::new(),
        }),
        guard,
    ))
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Open Hashgram", true, None::<&str>)?;
    let lock = MenuItem::with_id(app, "lock", "Lock", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &lock, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("icon".into()))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Hashgram One")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "lock" => {
                let st = app.state::<Arc<AppState>>().inner().clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    session::lock(&app, &st).await;
                });
            }
            "quit" => {
                let st = app.state::<Arc<AppState>>().inner().clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    session::lock(&app, &st).await;
                    app.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Runs the application.
pub fn run() {
    let (state, _log_guard) = match build_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hashgram-desktop: {e}");
            std::process::exit(1);
        }
    };

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            show_main(app);
            for a in args.iter().skip(1) {
                if a.starts_with("hashgram://") {
                    let _ = app.emit("deep-link", DeepLink { url: a.clone() });
                }
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state.clone());

    builder
        .invoke_handler(tauri::generate_handler![
            // identity / vault
            cmd_identity::app_status,
            cmd_identity::onboarding_generate,
            cmd_identity::onboarding_check_words,
            cmd_identity::onboarding_create,
            cmd_identity::onboarding_restore_preview,
            cmd_identity::onboarding_restore,
            cmd_identity::backup_inspect,
            cmd_identity::restore_from_backup,
            cmd_identity::unlock,
            cmd_identity::lock,
            cmd_identity::touch,
            cmd_identity::change_passphrase,
            cmd_identity::hello_enable,
            cmd_identity::hello_unlock,
            cmd_identity::hello_disable,
            cmd_identity::wipe_local_data,
            cmd_identity::identity_status,
            cmd_identity::identity_register,
            cmd_identity::devices_list,
            cmd_identity::this_device,
            cmd_identity::device_add,
            cmd_identity::device_revoke,
            cmd_identity::devices_reconcile,
            cmd_identity::devices_bootstrap,
            // mail
            cmd_mail::mail_counts,
            cmd_mail::mail_list,
            cmd_mail::mail_thread,
            cmd_mail::mail_get,
            cmd_mail::mail_search,
            cmd_mail::mail_mark_read,
            cmd_mail::mail_star,
            cmd_mail::mail_move,
            cmd_mail::mail_archive,
            cmd_mail::mail_trash,
            cmd_mail::mail_delete,
            cmd_mail::mail_accept_request,
            cmd_mail::mail_label,
            cmd_mail::mail_settings_get,
            cmd_mail::mail_settings_set,
            cmd_mail::mail_purge,
            cmd_mail::mail_resolve_recipients,
            cmd_mail::mail_draft_new,
            cmd_mail::mail_draft_save,
            cmd_mail::mail_draft_list,
            cmd_mail::mail_draft_get,
            cmd_mail::mail_draft_delete,
            cmd_mail::mail_attach_file,
            cmd_mail::mail_attach_bytes,
            cmd_mail::mail_attach_drive,
            cmd_mail::mail_draft_remove_attachment,
            cmd_mail::mail_send,
            cmd_mail::mail_attachment_save,
            cmd_mail::mail_attachment_open,
            cmd_mail::mail_attachment_preview,
            cmd_mail::mail_attachments,
            cmd_mail::mail_live_attachment_versions,
            // drive
            cmd_drive::drive_list,
            cmd_drive::drive_trash_list,
            cmd_drive::drive_starred,
            cmd_drive::drive_search,
            cmd_drive::drive_entry,
            cmd_drive::drive_versions,
            cmd_drive::drive_usage,
            cmd_drive::drive_resolve_path,
            cmd_drive::drive_mkdir,
            cmd_drive::drive_upload,
            cmd_drive::drive_upload_bytes,
            cmd_drive::drive_update,
            cmd_drive::drive_download,
            cmd_drive::drive_download_version,
            cmd_drive::drive_open,
            cmd_drive::drive_preview,
            cmd_drive::drive_rename,
            cmd_drive::drive_move,
            cmd_drive::drive_copy,
            cmd_drive::drive_trash,
            cmd_drive::drive_restore,
            cmd_drive::drive_delete,
            cmd_drive::drive_empty_trash,
            cmd_drive::drive_star,
            cmd_drive::drive_restore_version,
            cmd_drive::drive_rekey,
            cmd_drive::drive_rekey_all,
            cmd_drive::drive_commit,
            cmd_drive::drive_share,
            cmd_drive::drive_revoke,
            cmd_drive::drive_shares,
            cmd_drive::drive_shared_with_me,
            cmd_drive::drive_shared_download,
            cmd_drive::drive_shared_open,
            cmd_drive::drive_shared_save,
            cmd_drive::drive_shared_folder_list,
            cmd_drive::drive_shared_folder_download,
            // people
            cmd_people::people_resolve,
            cmd_people::people_profile,
            cmd_people::people_request,
            cmd_people::people_respond,
            cmd_people::people_remove,
            cmd_people::people_block,
            cmd_people::people_unblock,
            cmd_people::people_mute,
            cmd_people::people_trust,
            cmd_people::people_follow,
            cmd_people::people_list,
            cmd_people::people_search_local,
            cmd_people::people_set_display_name,
            cmd_people::people_my_display_name,
            cmd_people::people_send_card,
            cmd_people::people_card_of,
            cmd_people::people_username_of,
            // feed + circles
            cmd_feed::feed_following,
            cmd_feed::feed_friends,
            cmd_feed::feed_author,
            cmd_feed::feed_explore,
            cmd_feed::feed_thread,
            cmd_feed::feed_post,
            cmd_feed::feed_comment,
            cmd_feed::feed_react,
            cmd_feed::feed_repost,
            cmd_feed::feed_edit,
            cmd_feed::feed_delete,
            cmd_feed::feed_refresh,
            cmd_feed::feed_follows,
            cmd_feed::feed_profile_update,
            cmd_feed::feed_media_fetch,
            cmd_feed::circles_list,
            cmd_feed::circles_create,
            cmd_feed::circles_add_member,
            cmd_feed::circles_remove_member,
            cmd_feed::circles_leave,
            cmd_feed::circles_post,
            cmd_feed::circles_comment,
            cmd_feed::circles_react,
            cmd_feed::circles_vote,
            cmd_feed::circles_delete,
            cmd_feed::circles_set_info,
            cmd_feed::circles_posts,
            cmd_feed::circles_comments,
            cmd_feed::circles_merged,
            cmd_feed::circles_media_fetch,
            // hashwall: explore, walls, profile, avatars, rich list
            cmd_hashwall::hashwall_explore,
            cmd_hashwall::hashwall_digest,
            cmd_hashwall::hashwall_thread,
            cmd_hashwall::walls_create,
            cmd_hashwall::walls_info,
            cmd_hashwall::walls_page,
            cmd_hashwall::walls_pin,
            cmd_hashwall::walls_pinned,
            cmd_hashwall::people_profile_cached,
            cmd_hashwall::people_avatar,
            cmd_hashwall::profile_me,
            cmd_hashwall::profile_my_events,
            cmd_hashwall::network_holders,
            cmd_hashwall::network_providers,
            // spaces
            cmd_spaces::spaces_list,
            cmd_spaces::spaces_create,
            cmd_spaces::spaces_state,
            cmd_spaces::spaces_members,
            cmd_spaces::spaces_content,
            cmd_spaces::spaces_drive,
            cmd_spaces::spaces_invite,
            cmd_spaces::spaces_remove,
            cmd_spaces::spaces_set_role,
            cmd_spaces::spaces_set_info,
            cmd_spaces::spaces_announce,
            cmd_spaces::spaces_post,
            cmd_spaces::spaces_comment,
            cmd_spaces::spaces_share_drive,
            cmd_spaces::spaces_unshare_drive,
            cmd_spaces::spaces_mail,
            cmd_spaces::spaces_drive_download,
            cmd_spaces::spaces_drive_open,
            cmd_spaces::spaces_drive_save,
            cmd_spaces::spaces_drive_folder_list,
            // earn + node
            cmd_earn::earn_status,
            cmd_earn::earn_earnings,
            cmd_earn::earn_providers,
            cmd_earn::earn_register,
            cmd_earn::earn_update,
            cmd_earn::earn_unbond,
            cmd_earn::earn_withdraw,
            cmd_earn::node_overview,
            cmd_earn::node_configure,
            cmd_earn::node_install,
            cmd_earn::node_start,
            cmd_earn::node_stop,
            cmd_earn::node_uninstall,
            cmd_earn::node_generate_cold_address,
            cmd_earn::node_log_tail,
            // wallet
            cmd_wallet::wallet_balance,
            cmd_wallet::wallet_overview,
            cmd_wallet::tx_preview,
            cmd_wallet::tx_submit,
            cmd_wallet::tx_recent,
            cmd_wallet::tx_has_pending,
            cmd_wallet::wallet_parse_amount,
            cmd_wallet::wallet_preview_send,
            cmd_wallet::wallet_send,
            cmd_wallet::wallet_stake,
            cmd_wallet::wallet_unstake,
            cmd_wallet::wallet_withdraw_rewards,
            cmd_wallet::wallet_username_availability,
            cmd_wallet::wallet_register_username,
            cmd_wallet::wallet_renew_username,
            cmd_wallet::wallet_delegations,
            cmd_wallet::wallet_rewards,
            cmd_wallet::wallet_unbonding,
            cmd_wallet::wallet_history,
            cmd_wallet::wallet_tx,
            cmd_wallet::wallet_usernames,
            cmd_wallet::chain_query,
            cmd_wallet::qr_svg,
            // network
            cmd_network::network_overview,
            cmd_network::network_validators,
            cmd_network::network_supply,
            cmd_network::network_top,
            cmd_network::network_stats,
            cmd_network::net_reconnect,
            cmd_network::net_forget_peers,
            cmd_network::diagnostics_export,
            // sync
            cmd_sync::sync_status,
            cmd_sync::sync_now,
            // settings, backup, misc
            cmd_settings::settings_get,
            cmd_settings::settings_set,
            cmd_settings::backup_export,
            cmd_settings::help_list,
            cmd_settings::help_page,
            cmd_settings::perf_snapshot,
            cmd_settings::perf_mark,
            cmd_settings::perf_memory,
            cmd_settings::open_data_dir,
            cmd_settings::ui_log,
            cmd_settings::save_text_file,
            cmd_settings::search_recent,
            cmd_settings::search_note,
            cmd_settings::about_info,
            cmd_settings::leases_list,
            cmd_settings::leases_verify,
            cmd_settings::window_hide,
        ])
        .setup(move |app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let _ = app.deep_link().register_all();
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        let _ = handle.emit(
                            "deep-link",
                            DeepLink {
                                url: url.to_string(),
                            },
                        );
                    }
                });
            }
            setup_tray(app)?;
            let handle = app.handle().clone();
            session::spawn_link(handle.clone(), state.clone());
            session::spawn_housekeeping(handle, state.clone());
            tauri::async_runtime::spawn(chain_proxy::serve(state.clone()));
            #[cfg(debug_assertions)]
            if std::env::var("HASHGRAM_DEVTOOLS").map(|v| v == "1").unwrap_or(false) {
                if let Some(w) = app.get_webview_window("main") {
                    w.open_devtools();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides to the tray; Quit is in the tray menu.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("hashgram-desktop: {e}");
            std::process::exit(1);
        });
}
