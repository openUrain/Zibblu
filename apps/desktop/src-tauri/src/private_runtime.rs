use crate::commands::Desktop;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
#[cfg(not(target_os = "ios"))]
use tauri::{LogicalSize, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};
#[cfg(target_os = "ios")]
use tauri::Emitter;
use uuid::Uuid;
use zibblu_core::{
    Client, Directory, LanCredential, User,
    private_chat::{DeviceKey, DiscoveryStatus, PrivateLobby},
};
#[cfg(not(target_os = "ios"))]
use zibblu_core::private_chat::Phase;

pub(crate) const INVITATION: &str = "private-invitation";

pub(crate) fn memberships(directory: &Directory) -> HashMap<Uuid, Vec<User>> {
    directory
        .conversations
        .iter()
        .map(|chat| (chat.id, chat.members.clone()))
        .collect()
}

pub(crate) fn prompt_window(app: &AppHandle, desktop: &Desktop) -> Option<&'static str> {
    if !desktop.private_available() || desktop.private_quiet() || !notifications_allowed() {
        return None;
    }
    let main = app.get_webview_window("main")?;
    if cfg!(windows) && !main.is_visible().ok()? {
        return None;
    }
    Some(if cfg!(windows) && main.is_minimized().ok()? {
        INVITATION
    } else {
        "main"
    })
}

#[cfg(not(target_os = "ios"))]
pub(crate) async fn run(app: AppHandle, client: Arc<Client>) {
    tokio::join!(maintain_lobby(&app, &client), run_prompts(&app));
}

#[cfg(target_os = "ios")]
pub(crate) async fn run(app: AppHandle, client: Arc<Client>) {
    use std::sync::atomic::Ordering;
    let desktop = app.state::<Desktop>();
    let mut network = crate::ios::network();
    loop {
        let current = *network.borrow_and_update();
        desktop.private_generation.store(current.generation, Ordering::Release);
        if current.available {
            tokio::select! {
                biased;
                _ = network.changed() => {}
                _ = maintain_lobby(&app, &client) => { let _ = network.changed().await; }
            }
        } else if network.changed().await.is_err() {
            return;
        }
        if let Some(lobby) = desktop.private.lock().await.take() {
            lobby.shutdown();
        }
        *desktop.discovery.lock().expect("discovery status") = DiscoveryStatus::Starting;
        let _ = app.emit("ios-private-reset", ());
    }
}

async fn maintain_lobby(app: &AppHandle, client: &Client) {
    let desktop = app.state::<Desktop>();
    let Ok(key) = DeviceKey::generate() else {
        *desktop.discovery.lock().expect("discovery status") = DiscoveryStatus::DiscoveryFailed;
        return;
    };
    let mut credential: Option<LanCredential> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        {
            let mut current = desktop.private.lock().await;
            let expired = credential.as_ref().is_some_and(|cert| cert.expires_at <= now);
            let moved = current.as_ref().is_some_and(|lobby| !lobby.network_matches());
            #[cfg(windows)]
            if moved && !expired && let Some(lobby) = current.as_ref() {
                lobby.suspend_transport();
            }
            if (expired || !cfg!(windows) && moved)
                && let Some(lobby) = current.take()
            {
                lobby.shutdown();
            }
        }
        let status = PrivateLobby::discovery_status();
        if !matches!(status, DiscoveryStatus::Ready) {
            *desktop.discovery.lock().expect("discovery status") = status;
            continue;
        }
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
        let directory = if desktop.private.lock().await.is_none() {
            client.membership_directory().await.ok()
        } else {
            None
        };
        let mut current = desktop.private.lock().await;
        #[cfg(windows)]
        if let Some(lobby) = current.as_ref() && !lobby.network_matches() {
            *desktop.discovery.lock().expect("discovery status") =
                if lobby.restore_transport(client.server().to_owned()).is_ok() {
                    DiscoveryStatus::Ready
                } else {
                    DiscoveryStatus::Starting
                };
        }
        if current.is_none() {
            let lobby = match (
                credential.as_ref().filter(|cert| cert.expires_at > now),
                directory,
            ) {
                (Some(cert), Some(directory)) => PrivateLobby::new(
                    client.server().to_owned(),
                    client.user.clone(),
                    key.clone(),
                    cert.clone(),
                    memberships(&directory),
                    desktop.dismissed_rooms.clone(),
                )
                .ok(),
                _ => None,
            };
            *desktop.discovery.lock().expect("discovery status") = if lobby.is_some() {
                DiscoveryStatus::Ready
            } else {
                DiscoveryStatus::DiscoveryFailed
            };
            #[cfg(windows)]
            if let Some(lobby) = &lobby {
                let app = app.clone();
                lobby.on_exclusive(move |conversation| crate::ai::suspend_conversation(&app, conversation));
            }
            *current = lobby.map(Arc::new);
        }
    }
}

#[cfg(not(target_os = "ios"))]
async fn run_prompts(app: &AppHandle) {
    let desktop = app.state::<Desktop>();
    let mut shown = None;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        #[cfg(windows)]
        {
            desktop.update_tray(app, None).await;
            close_exclusive_chat_windows(app, &desktop).await;
        }
        let invitation = if prompt_window(app, &desktop) == Some(INVITATION) {
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
                    && show_desktop_prompt(app, &desktop, invitation.invited.len() > 1).is_ok() =>
            {
                shown = Some(invitation.id);
            }
            _ => {}
        }
    }
}

#[cfg(windows)]
async fn close_exclusive_chat_windows(app: &AppHandle, desktop: &Desktop) {
    let open: Vec<_> = app.webview_windows().keys()
        .filter_map(|label| crate::chat_window::conversation_id_for_label(label)).collect();
    if open.is_empty() { return; }
    let Some(lobby) = desktop.private.lock().await.clone() else { return; };
    let active: Vec<_> = open.into_iter().filter(|id| lobby.has_active_conversation(*id)).collect();
    if active.is_empty() { return; }
    let Ok(client) = desktop.client().await else { return; };
    let Ok(directory) = client.directory().await else { return; };
    for chat in directory.conversations {
        if chat.title.is_none() && active.contains(&chat.id) {
            crate::chat_window::close_chat_window(app, chat.id);
        }
    }
}

#[cfg(not(target_os = "ios"))]
fn show_desktop_prompt(
    app: &AppHandle,
    desktop: &Desktop,
    show_invitees: bool,
) -> tauri::Result<()> {
    let Some(main) = app.get_webview_window("main") else {
        return Ok(());
    };
    let Some(monitor) = main.current_monitor()?.or(main.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let height = if show_invitees { 276.0 } else { 208.0 };
    let desired: PhysicalSize<u32> = LogicalSize::new(336.0, height).to_physical(scale);
    let size = PhysicalSize::new(
        desired.width.min(area.size.width),
        desired.height.min(area.size.height),
    );
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

#[cfg(not(any(windows, target_os = "ios")))]
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
