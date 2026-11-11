#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod private_runtime;

use commands::{Desktop, DropOffer};
use std::path::PathBuf;
use tauri::{DragDropEvent, Emitter, Manager, WebviewWindowBuilder, WindowEvent};
use uuid::Uuid;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = match std::env::var_os("ZIBBLU_DATA_DIR") {
                Some(path) => std::path::absolute(PathBuf::from(path))?,
                None => app.path().app_local_data_dir()?,
            };
            std::fs::create_dir_all(&data)?;
            app.manage(Desktop::new(data.clone()));
            let config = &app.config().app.windows[0];
            WebviewWindowBuilder::from_config(app, config)?
                .data_directory(data.join("webview"))
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if matches!(event, WindowEvent::Destroyed) {
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    app.state::<Desktop>().close_private(&app).await;
                });
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
            commands::set_theme,
            commands::login,
            commands::logout,
            commands::sync,
            commands::directory,
            commands::messages,
            commands::files,
            commands::find_user,
            commands::add_contact,
            commands::remove_contact,
            commands::create_conversation,
            commands::group_invite,
            commands::group_preview,
            commands::group_join,
            commands::send_message,
            commands::private_start,
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
