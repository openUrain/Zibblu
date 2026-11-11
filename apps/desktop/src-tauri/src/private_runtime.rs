use crate::commands::Desktop;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindowBuilder,
};
use uuid::Uuid;
use zibblu_core::{
    Client, Directory, LanCredential, User,
    private_chat::{DeviceKey, Phase, PrivateLobby},
};

pub(crate) const INVITATION: &str = "private-invitation";

pub(crate) fn memberships(directory: &Directory) -> HashMap<Uuid, Vec<User>> {
    directory
        .conversations
        .iter()
        .map(|chat| (chat.id, chat.members.clone()))
        .collect()
}

pub(crate) fn prompt_window(app: &AppHandle, desktop: &Desktop) -> Option<&'static str> {
    if desktop.private_quiet() || !notifications_allowed() {
        return None;
    }
    let main = app.get_webview_window("main")?;
    Some(if cfg!(windows) && main.is_minimized().ok()? {
        INVITATION
    } else {
        "main"
    })
}

pub(crate) async fn run(app: AppHandle, client: Arc<Client>) {
    let Ok(key) = DeviceKey::generate() else {
        return;
    };
    let desktop = app.state::<Desktop>();
    let mut credential: Option<LanCredential> = None;
    let mut next_network_check = Instant::now();
    let mut shown = None;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        if Instant::now() >= next_network_check {
            next_network_check = Instant::now() + Duration::from_secs(5);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            if credential
                .as_ref()
                .is_none_or(|cert| cert.expires_at <= now + 120)
                && let Ok(next) = client.lan_credential(key.public_key()).await
            {
                let mut current = desktop.private.lock().await;
                if let Some(lobby) = current.as_ref()
                    && lobby.renew(key.clone(), next.clone()).is_err()
                {
                    lobby.shutdown();
                    *current = None;
                }
                credential = Some(next);
            }
            let mut current = desktop.private.lock().await;
            if current
                .as_ref()
                .is_some_and(|lobby| !lobby.network_matches())
                || credential
                    .as_ref()
                    .is_some_and(|cert| cert.expires_at <= now)
            {
                if let Some(lobby) = current.take() {
                    lobby.shutdown();
                }
            }
            if current.is_none()
                && let Some(cert) = credential.as_ref().filter(|cert| cert.expires_at > now)
                && let Ok(directory) = client.directory().await
                && let Ok(lobby) = PrivateLobby::new(
                    client.server().to_owned(),
                    client.user.clone(),
                    key.clone(),
                    cert.clone(),
                    memberships(&directory),
                )
            {
                *current = Some(Arc::new(lobby));
            }
        }
        let invitation = if prompt_window(&app, &desktop) == Some(INVITATION) {
            desktop.private.lock().await.as_ref().and_then(|lobby| {
                lobby
                    .list()
                    .into_iter()
                    .find(|room| room.phase == Phase::Invited)
            })
        } else {
            None
        };
        let window = app.get_webview_window(INVITATION);
        match invitation {
            None => {
                shown = None;
                if let Some(window) = window {
                    let _ = window.close();
                }
            }
            Some(invitation)
                if window.is_none()
                    && shown != Some(invitation.id)
                    && show_desktop_prompt(&app, &desktop).is_ok() =>
            {
                shown = Some(invitation.id);
            }
            _ => {}
        }
    }
}

fn show_desktop_prompt(app: &AppHandle, desktop: &Desktop) -> tauri::Result<()> {
    let Some(main) = app.get_webview_window("main") else {
        return Ok(());
    };
    let Some(monitor) = main.current_monitor()?.or(main.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let size: PhysicalSize<u32> = LogicalSize::new(336.0, 208.0).to_physical(scale);
    let gap = (16.0 * scale).round() as i32;
    let window = WebviewWindowBuilder::new(app, INVITATION, WebviewUrl::App("index.html".into()))
        .title("Zibblu")
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .content_protected(true)
        .data_directory(desktop.webview_directory())
        .build()?;
    let position = PhysicalPosition::new(
        area.position.x + (area.size.width as i32 - size.width as i32 - gap).max(0),
        area.position.y + (area.size.height as i32 - size.height as i32 - gap).max(0),
    );
    let result = window
        .set_position(position)
        .and_then(|_| window.set_size(size))
        .and_then(|_| {
            if prompt_window(app, desktop) == Some(INVITATION) {
                show_without_activation(&window)
            } else {
                window.close()
            }
        });
    if result.is_err() {
        let _ = window.close();
    }
    result
}

#[cfg(windows)]
fn show_without_activation(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindowAsync(window: *mut std::ffi::c_void, command: i32) -> i32;
    }
    const SW_SHOWNA: i32 = 8;
    // The HWND belongs to the popup and remains valid while this show is posted.
    if unsafe { ShowWindowAsync(window.hwnd()?.0, SW_SHOWNA) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn show_without_activation(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    window.show()
}

#[cfg(windows)]
fn notifications_allowed() -> bool {
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHQueryUserNotificationState(state: *mut i32) -> i32;
    }
    let mut state = 0;
    // Win32 writes one QUERY_USER_NOTIFICATION_STATE value.
    let result = unsafe { SHQueryUserNotificationState(&mut state) };
    result >= 0 && state == 5
}

#[cfg(not(windows))]
fn notifications_allowed() -> bool {
    true
}
