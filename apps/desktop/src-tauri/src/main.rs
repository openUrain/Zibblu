#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod commands;
mod media;
mod private_runtime;
#[cfg(windows)]
mod windows_shell;

use commands::{Desktop, DropOffer};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, WebviewWindowBuilder, WindowEvent};
use uuid::Uuid;

fn quit(app: &AppHandle) {
    static EXITING: AtomicBool = AtomicBool::new(false);
    if !EXITING.swap(true, Ordering::Relaxed) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            app.state::<Desktop>().close_private(&app).await;
            app.exit(0);
        });
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("zibblu-media", media::preview)
        .setup(|app| {
            let data = match std::env::var_os("ZIBBLU_DATA_DIR") {
                Some(path) => std::path::absolute(PathBuf::from(path))?,
                None => app.path().app_local_data_dir()?,
            };
            std::fs::create_dir_all(&data)?;
            #[cfg(windows)]
            let window_class = match windows_shell::claim_profile(&data)? {
                Some((lock, window_class)) => {
                    app.manage(lock);
                    window_class
                }
                None => {
                    app.handle().exit(0);
                    return Ok(());
                }
            };
            app.manage(Desktop::new(data.clone()));
            let config = &app.config().app.windows[0];
            let builder = WebviewWindowBuilder::from_config(app, config)?
                .data_directory(data.join("webview"))
                .on_navigation(|url| {
                    matches!(url.scheme(), "tauri") && url.host_str() == Some("localhost")
                        || matches!(url.scheme(), "http" | "https")
                            && url.host_str() == Some("tauri.localhost")
                        || cfg!(debug_assertions)
                            && url.scheme() == "http"
                            && url.host_str() == Some("127.0.0.1")
                            && url.port() == Some(1420)
                });
            #[cfg(windows)]
            {
                let window = builder
                    .window_classname(window_class)
                    .visible(false)
                    .decorations(false)
                    .build()?;
                windows_shell::center_on_startup(&window, config)?;
                windows_shell::create_tray(app.handle())?;
                window.show()?;
            }
            #[cfg(not(windows))]
            builder.build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                #[cfg(windows)]
                windows_shell::request_close(window);
                #[cfg(not(windows))]
                quit(window.app_handle());
                return;
            }
            let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event else {
                return;
            };
            let state = window.state::<Desktop>();
            let [path] = paths.as_slice() else {
                let _ = window.emit(
                    "attachment-drop-error",
                    "Drop one file or folder at a time.",
                );
                return;
            };
            let ticket = Uuid::new_v4();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            *state.drop.lock().expect("drop state") = Some((ticket, path.clone()));
            let _ = window.emit("attachment-drop", DropOffer { ticket, name });
        })
        .invoke_handler(tauri::generate_handler![
            commands::preferences,
            commands::set_close_action,
            commands::login,
            commands::logout,
            commands::sync,
            commands::directory,
            commands::messages,
            commands::files,
            commands::find_user,
            commands::add_contact,
            commands::respond_contact,
            commands::remove_contact,
            commands::profile,
            commands::set_bio,
            commands::set_contact_notes,
            media::set_profile_image,
            commands::set_read_receipts,
            commands::read_update,
            commands::hide_message,
            commands::quoted_message,
            commands::create_conversation,
            commands::group_invite,
            commands::group_preview,
            commands::group_join,
            commands::send_message,
            commands::recall_message,
            commands::publish_ai_reply,
            media::paste_image,
            ai::ai_assistants,
            ai::ai_save_assistant,
            ai::ai_test_connection,
            ai::ai_remove_assistant,
            ai::ai_open_view,
            ai::ai_close_view,
            ai::ai_begin,
            ai::ai_cancel,
            ai::ai_pick_file,
            ai::ai_remove_file,
            ai::ai_prepare,
            ai::ai_send,
            commands::private_start,
            commands::private_nearby,
            commands::private_invite_nearby,
            commands::private_list,
            commands::private_view,
            commands::private_respond,
            commands::private_send,
            commands::private_end,
            commands::private_qr,
            commands::private_scan,
            commands::set_private_quiet,
            commands::share,
            commands::save_attachment,
            commands::transfers,
            commands::pause_transfer,
            commands::resume_transfer,
            commands::discard_transfer,
        ])
        .run(tauri::generate_context!())
        .expect("Could not start Zibblu");
}
