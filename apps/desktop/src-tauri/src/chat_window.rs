use crate::commands::Desktop;
use anyhow::Context;
use serde::Serialize;
#[cfg(windows)]
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use uuid::Uuid;
use zibblu_core::{Directory, User};

pub(crate) const LABEL_PREFIX: &str = "chat-";

pub(crate) fn is_chat_window(label: &str) -> bool {
    conversation_id_for_label(label).is_some()
}

pub(crate) fn is_chat_or_main_window(label: &str) -> bool {
    label == "main" || is_chat_window(label)
}

pub(crate) fn window_allows_conversation(label: &str, conversation: Uuid) -> bool {
    label == "main" || conversation_id_for_label(label) == Some(conversation)
}

pub(crate) fn conversation_id_for_label(label: &str) -> Option<Uuid> {
    let rest = label.strip_prefix(LABEL_PREFIX)?;
    if rest.len() != 32 {
        return None;
    }
    Uuid::parse_str(rest).ok()
}

pub(crate) fn window_label(conversation: Uuid) -> String {
    format!("{LABEL_PREFIX}{}", conversation.simple())
}

#[derive(Serialize)]
pub(crate) struct ChatWindowSession {
    pub user: User,
    pub server: String,
    pub conversation_id: Uuid,
}

fn conversation_title(directory: &Directory, own_id: Uuid, conversation: Uuid) -> Option<String> {
    if cfg!(windows) && conversation == zibblu_core::ZIBBLU_ASSISTANT {
        return Some("Zibblu Assistant".into());
    }
    let chat = directory
        .conversations
        .iter()
        .find(|chat| chat.id == conversation && chat.members.iter().any(|member| member.id == own_id))?;
    if let Some(title) = chat.title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(title.to_owned());
    }
    let peer = chat.members.iter().find(|member| member.id != own_id)?;
    let name = directory
        .remarks
        .get(&peer.id)
        .or_else(|| directory.names.get(&peer.id))
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&peer.handle);
    Some(name.to_owned())
}

#[cfg(windows)]
fn chat_window_classname(webview_dir: &std::path::Path) -> String {
    let base = webview_dir.parent().unwrap_or(webview_dir);
    let canonical = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    format!(
        "Zibblu.Chat.{:x}",
        Sha256::digest(canonical.as_os_str().as_encoded_bytes())
    )
}

#[allow(dead_code)]
pub(crate) async fn open_chat_window(
    app: &AppHandle,
    state: &Desktop,
    conversation: Uuid,
) -> crate::commands::Result<()> {
    #[cfg(windows)]
    {
        let client = state.client().await?;
        let directory = client.directory().await?;
        if directory.conversations.iter().any(|chat| chat.id == conversation && chat.title.is_none())
            && state.private.lock().await.as_ref().is_some_and(|lobby| lobby.has_active_conversation(conversation))
        {
            return Err(anyhow::anyhow!("Open the active private conversation from the main chat list.").into());
        }
        let title = conversation_title(&directory, client.user.id, conversation)
            .context("This conversation is unavailable")?;
        let label = window_label(conversation);
        if let Some(existing) = app.get_webview_window(&label) {
            let _ = existing.set_title(&title);
            existing.unminimize().map_err(anyhow::Error::from)?;
            existing.show().map_err(anyhow::Error::from)?;
            existing.set_focus().map_err(anyhow::Error::from)?;
            return Ok(());
        }
        let webview_dir = state.webview_directory();
        let window_class = chat_window_classname(&webview_dir);
        let window = tauri::WebviewWindowBuilder::new(
            app,
            label,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title(title)
        .on_navigation(crate::local_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .data_directory(webview_dir)
        .window_classname(window_class)
        .transparent(true)
        .effects(tauri::utils::config::WindowEffectsConfig {
            effects: vec![tauri::utils::WindowEffect::Acrylic],
            ..Default::default()
        })
        .decorations(false)
        .inner_size(780.0, 680.0)
        .min_inner_size(480.0, 500.0)
        .visible(false)
        .build()
        .map_err(anyhow::Error::from)?;
        if !state.is_current(&client).await {
            let _ = window.destroy();
            return Err(anyhow::anyhow!("The session has ended").into());
        }
        let _ = window.center();
        if let Err(error) = window.show().and_then(|_| window.set_focus()) {
            let _ = window.destroy();
            return Err(anyhow::Error::from(error).into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (app, state, conversation);
        Err(anyhow::anyhow!("Standalone chat windows are available on Windows").into())
    }
}

#[allow(dead_code)]
pub(crate) fn close_chat_window(app: &AppHandle, conversation: Uuid) {
    if let Some(window) = app.get_webview_window(&window_label(conversation)) {
        let _ = window.close();
    }
}

#[allow(dead_code)]
pub(crate) fn close_all_chat_windows(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if is_chat_window(&label) {
            let _ = window.close();
        }
    }
}

#[allow(dead_code)]
#[tauri::command]
pub(crate) async fn chat_window_session(
    window: WebviewWindow,
    state: State<'_, Desktop>,
) -> crate::commands::Result<ChatWindowSession> {
    let conversation_id = conversation_id_for_label(window.label())
        .context("Open this conversation in a standalone chat window")?;
    let client = state.client().await?;
    let directory = client.directory().await?;
    if conversation_title(&directory, client.user.id, conversation_id).is_none() {
        return Err(anyhow::anyhow!("This conversation is no longer available").into());
    }
    Ok(ChatWindowSession {
        user: client.user.clone(),
        server: client.server().to_owned(),
        conversation_id,
    })
}
