#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::{Desktop, DropOffer};
use std::path::PathBuf;
use tauri::{DragDropEvent, Emitter, Manager, WebviewEvent, WebviewWindowBuilder};
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
        .on_webview_event(|view, event| {
            let WebviewEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event else {
                return;
            };
            let state = view.state::<Desktop>();
            let [path] = paths.as_slice() else {
                let _ = view.emit(
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
            let _ = view.emit("attachment-drop", DropOffer { ticket, name });
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
            commands::send_message,
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
