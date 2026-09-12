//! Hashgram for Windows — the Rust side of the Tauri application.
//!
//! Everything that is not pixels lives here: the vault, the network link,
//! chain access with cross-checking, the encrypted database, transaction
//! building and signing, Windows integration (DPAPI, Hello, tray, toasts,
//! deep links, single instance). The frontend (SolidJS in WebView2) calls
//! the commands in [`commands`] and listens for a handful of events.

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

pub mod chain_access;
pub mod chain_proxy;
pub mod chat;
pub mod commands;
pub mod commands_node;
pub mod commands_social;
pub mod crypto;
pub mod db;
pub mod help;
pub mod net;
pub mod node_manager;
pub mod paths;
pub mod perf;
pub mod settings;
pub mod social_hub;
pub mod state;
pub mod tx;
pub mod winsec;

use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::state::AppState;

fn init_logging(perf: Arc<perf::PerfStore>, level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(format!(
            "{level},libp2p_gossipsub=warn,libp2p_kad=warn,libp2p_swarm=warn,quinn=warn,quinn_udp=error,hyper=warn"
        ))
    });
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact();
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt)
        .with(perf::PerfLayer::new(perf))
        .try_init();
}

fn build_state() -> Result<Arc<AppState>, String> {
    let data = paths::ensure_dirs().map_err(|e| e.to_string())?;
    let settings = settings::Settings::load(&paths::settings_path());
    let perf = Arc::new(perf::PerfStore::default());
    init_logging(perf.clone(), &settings.advanced.log_level);
    tracing::info!(dir = %data.display(), "Hashgram for Windows starting");
    let db = Arc::new(db::Db::open(&paths::db_path())?);
    Ok(Arc::new(AppState {
        settings: tokio::sync::RwLock::new(settings),
        session: tokio::sync::RwLock::new(None),
        pending_mnemonic: tokio::sync::Mutex::new(None),
        net: Arc::new(net::NetManager::default()),
        chain: Arc::new(chain_access::ChainAccess::default()),
        db,
        perf,
        pending: tokio::sync::Mutex::new(Vec::new()),
        chat: Arc::new(chat::ChatHub::default()),
        social: Arc::new(social_hub::SocialHub::default()),
    }))
}

fn spawn_background(app: tauri::AppHandle, state: Arc<AppState>) {
    // 1. Start the network as soon as the process is up; peers appear as
    //    they verify. The UI never waits on this.
    {
        let st = state.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let s = st.settings.read().await.clone();
            if let Ok(identity) = net::identity_for(&s) {
                st.chain.set_chain_id(&identity.chain_id).await;
            }
            match st.net.start(&s, &st.db).await {
                Ok(_) => tracing::info!("network link started"),
                Err(e) => tracing::warn!(error = %e, "network link did not start"),
            }
            let _ = app.emit("net:changed", ());
        });
    }
    // 2. Periodic: latency, pending transactions, auto-lock, net events.
    {
        let st = state.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut n: u64 = 0;
            loop {
                tick.tick().await;
                n += 1;
                commands::poll_pending(&st, &app).await;
                if n.is_multiple_of(15) {
                    st.net.measure_latency().await;
                    let _ = app.emit("net:changed", ());
                }
                if n % 15 == 7 && st.auto_lock_if_due().await {
                    let _ = app.emit("session:locked", ());
                }
                if n.is_multiple_of(5) {
                    let _ = app.emit("net:changed", ());
                }
            }
        });
    }
    // 3. Loopback chain gateway for a node on this PC (and hashgram-client).
    {
        let st = state.clone();
        tauri::async_runtime::spawn(chain_proxy::serve(st));
    }
    // 4. Messaging: mailbox sync every 4 s while unlocked, feed refresh
    //    every 60 s, expired (disappearing) messages swept every 30 s.
    {
        let st = state.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(4));
            let mut n: u64 = 0;
            let mut published_keys = false;
            loop {
                tick.tick().await;
                n += 1;
                if !st.chat.is_open().await {
                    published_keys = false;
                    continue;
                }
                if !published_keys {
                    if let Ok((account, _, _)) = st.session_handles().await {
                        if let (Some(link), Some(identity)) =
                            (st.net.link().await, st.net.identity().await)
                        {
                            match st
                                .chat
                                .publish_key_packages(&account, &link, &identity)
                                .await
                            {
                                Ok(n) if n > 0 => {
                                    tracing::info!(stores = n, "key packages published");
                                    published_keys = true;
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::debug!(error = %e, "key packages not published yet")
                                }
                            }
                        }
                    }
                }
                if let Err(e) = commands_social::sync_once(&st, &app).await {
                    tracing::debug!(error = %e, "mailbox sync");
                }
                if n.is_multiple_of(8) {
                    if let Ok(k) = chat::sweep_expired(&st.db) {
                        if k > 0 {
                            let _ = app.emit("chat:changed", serde_json::json!({ "expired": k }));
                        }
                    }
                }
                if n % 15 == 1 {
                    if let Err(e) = commands_social::refresh_feed(&st, &app).await {
                        tracing::debug!(error = %e, "feed refresh");
                    }
                }
            }
        });
    }
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
        .tooltip("Hashgram")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "lock" => {
                let st = app.state::<Arc<AppState>>().inner().clone();
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    st.lock().await;
                    let _ = app.emit("session:locked", ());
                });
            }
            "quit" => app.exit(0),
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
    let state = match build_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hashgram-desktop: {e}");
            std::process::exit(1);
        }
    };

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second launch (or a hashgram:// link) focuses the running
            // window and hands the link over.
            show_main(app);
            for a in args.iter().skip(1) {
                if a.starts_with("hashgram://") {
                    let _ = app.emit("deep-link", commands::DeepLink { url: a.clone() });
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
        .plugin(tauri_plugin_updater::Builder::new().build());

    builder = builder.manage(state.clone());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::onboarding_generate,
            commands::onboarding_check_words,
            commands::onboarding_create,
            commands::onboarding_restore_preview,
            commands::onboarding_restore,
            commands::unlock,
            commands::lock,
            commands::touch,
            commands::change_passphrase,
            commands::hello_enable,
            commands::hello_unlock,
            commands::hello_disable,
            commands::wipe_local_data,
            commands::settings_get,
            commands::settings_set,
            commands::net_snapshot,
            commands::net_measure_latency,
            commands::net_forget_peers,
            commands::net_reconnect,
            commands::chain_health,
            commands::diagnostics_export,
            commands::chain_get,
            commands::chain_get_many,
            commands::wallet_overview,
            commands::tx_preview,
            commands::tx_submit,
            commands::tx_recent,
            commands::tx_status,
            commands::tx_has_pending,
            commands::identity_status,
            commands::identity_register,
            commands::search_resolve,
            commands::search_recent,
            commands::qr_svg,
            commands::help_list,
            commands::help_page,
            commands::perf_snapshot,
            commands::perf_mark,
            commands::perf_memory,
            commands::open_data_dir,
            commands::save_text_file,
            commands::ui_log,
            commands_social::chat_list,
            commands_social::chat_history,
            commands_social::chat_start_direct,
            commands_social::chat_create_group,
            commands_social::chat_add_member,
            commands_social::chat_remove_member,
            commands_social::chat_send_text,
            commands_social::chat_send_file,
            commands_social::chat_send_voice,
            commands_social::chat_attachment,
            commands_social::chat_react,
            commands_social::chat_edit,
            commands_social::chat_delete,
            commands_social::chat_mark_read,
            commands_social::chat_typing,
            commands_social::chat_typing_in,
            commands_social::chat_set_disappear,
            commands_social::chat_info,
            commands_social::chat_search,
            commands_social::chat_sync_now,
            commands_social::chat_publish_key_packages,
            commands_social::feed,
            commands_social::feed_refresh,
            commands_social::post_create,
            commands_social::comment_create,
            commands_social::social_react,
            commands_social::repost,
            commands_social::follow,
            commands_social::follows,
            commands_social::social_block,
            commands_social::social_blocks,
            commands_social::profile_get,
            commands_social::profile_update,
            commands_social::post_thread,
            commands_social::media_upload,
            commands_social::media_fetch,
            commands_social::reel_publish,
            commands_social::story_publish,
            commands_social::channel_create,
            commands_social::channels,
            commands_social::channel_posts,
            commands_social::safety_verdict,
            commands_social::calls_discover,
            commands_social::calls_turn,
            commands_social::calls_signal,
            commands_social::calls_signals,
            commands_node::node_overview,
            commands_node::node_configure,
            commands_node::node_install,
            commands_node::node_start,
            commands_node::node_stop,
            commands_node::node_uninstall,
            commands_node::node_generate_cold_address,
            commands_node::node_log_tail,
        ])
        .setup(move |app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // Registers hashgram:// for this user when not installed by
                // the installer (dev runs).
                let _ = app.deep_link().register_all();
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        let _ = handle.emit(
                            "deep-link",
                            commands::DeepLink {
                                url: url.to_string(),
                            },
                        );
                    }
                });
            }
            setup_tray(app)?;
            spawn_background(app.handle().clone(), state.clone());
            #[cfg(debug_assertions)]
            if std::env::var("HASHGRAM_DEVTOOLS")
                .map(|v| v == "1")
                .unwrap_or(false)
            {
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
