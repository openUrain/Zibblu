mod ai;
mod clipboard;
mod commands;
#[cfg(windows)]
mod dpapi;
mod images;
mod image_viewer;
#[cfg(target_os = "ios")]
mod ios;
mod media;
mod private_runtime;
mod remote_image;
mod saved_session;
#[cfg(windows)]
mod windows_shell;

use commands::Desktop;
#[cfg(not(target_os = "ios"))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, WebviewWindowBuilder, WindowEvent};

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

fn local_navigation(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "tauri") && url.host_str() == Some("localhost")
        || matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost")
        || cfg!(debug_assertions) && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1") && url.port() == Some(1420)
}

fn restrict_viewer(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if invoke.message.webview().label() == image_viewer::LABEL
            && !matches!(invoke.message.command(), "image_viewer_selection" | "image_viewer_step" | "copy_viewer_image" | "save_attachment")
        {
            invoke.resolver.reject("This command is unavailable in the image viewer");
            return true;
        }
        handler(invoke)
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(remote_image::RemotePreview::default())
        .register_asynchronous_uri_scheme_protocol("zibblu-media", media::preview)
        .setup(|app| {
            #[cfg(target_os = "ios")]
            let data = ios::data_directory()?;
            #[cfg(not(target_os = "ios"))]
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
            #[cfg(target_os = "ios")]
            ios::install();
            let config = &app.config().app.windows[0];
            let builder = WebviewWindowBuilder::from_config(app, config)?.on_navigation(local_navigation);
            #[cfg(not(target_os = "ios"))]
            let builder = builder.data_directory(data.join("webview"));
            #[cfg(windows)]
            {
                let window = builder
                    .window_classname(window_class)
                    .transparent(true)
                    .effects(tauri::utils::config::WindowEffectsConfig {
                        effects: vec![tauri::utils::WindowEffect::Acrylic],
                        ..Default::default()
                    })
                    .visible(false)
                    .decorations(false)
                    .build()?;
                windows_shell::center_on_startup(&window, config)?;
                windows_shell::create_tray(app.handle())?;
                window.show()?;
            }
            #[cfg(target_os = "ios")]
            ios::install_layout(&builder.build()?)?;
            #[cfg(not(any(windows, target_os = "ios")))]
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
            if paths.len() != 1 {
                let _ = window.emit_to(
                    "main",
                    "attachment-drop-error",
                    "Drop one file or folder at a time.",
                );
                return;
            }
            let offer = state.offer_attachments(paths.clone());
            let _ = window.emit_to("main", "attachment-drop", offer);
        })
        .invoke_handler(restrict_viewer(tauri::generate_handler![
            clipboard::copy_text,
            clipboard::paste_text,
            commands::ios_layout,
            commands::ios_scan_qr,
            commands::preferences,
            commands::set_close_action,
            commands::login,
            commands::restore_session,
            commands::logout,
            commands::sync,
            commands::directory,
            commands::local_update,
            commands::devices,
            commands::retire_device,
            commands::messages,
            commands::files,
            commands::find_user,
            commands::add_contact,
            commands::respond_contact,
            commands::remove_contact,
            commands::profile,
            commands::cached_profile,
            commands::set_display_name,
            commands::set_bio,
            commands::set_contact_notes,
            media::pick_image,
            media::set_profile_image,
            media::preview_image_bytes,
            remote_image::open_message_link,
            remote_image::preview_message_image,
            remote_image::cancel_message_image,
            media::upload_image_size,
            commands::set_read_receipts,
            commands::read_update,
            commands::hide_message,
            commands::quoted_message,
            commands::create_conversation,
            commands::group_invite,
            commands::group_preview,
            commands::group_join,
            commands::leave_group,
            commands::send_message,
            commands::retry_message,
            commands::recall_message,
            commands::publish_ai_reply,
            media::paste_image,
            media::check_pasted_image,
            media::read_clipboard_image,
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
            commands::private_mark_read,
            commands::private_respond,
            commands::private_send,
            commands::private_end,
            commands::private_qr,
            commands::private_scan,
            commands::set_private_quiet,
            commands::offer_clipboard_files,
            commands::cancel_attachment_offer,
            commands::share,
            commands::save_attachment,
            commands::open_saved_attachment,
            image_viewer::open_image_viewer,
            image_viewer::image_viewer_selection,
            image_viewer::image_viewer_step,
            image_viewer::copy_viewer_image,
            commands::transfers,
            commands::pause_transfer,
            commands::resume_transfer,
            commands::discard_transfer,
        ]))
        .run(tauri::generate_context!())
        .expect("Could not start Zibblu");
}
