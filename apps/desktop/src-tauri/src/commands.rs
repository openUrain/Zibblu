use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use uuid::Uuid;
use zibblu_core::{
    Client, ConversationRequest, Credentials, Direction, Directory, FileFilter, OnlineError, Page,
    SendMessage, Transfer, User,
};

pub(crate) struct Desktop {
    data: PathBuf,
    preferences: StdMutex<Preferences>,
    session: Mutex<Option<Arc<Client>>>,
    pub drop: StdMutex<Option<(Uuid, PathBuf)>>,
}

impl Desktop {
    pub fn new(data: PathBuf) -> Self {
        let preferences = std::fs::read(data.join("preferences.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            data,
            preferences: StdMutex::new(preferences),
            session: Mutex::new(None),
            drop: StdMutex::new(None),
        }
    }

    fn update_preferences(&self, change: impl FnOnce(&mut Preferences)) -> anyhow::Result<()> {
        let mut current = self.preferences.lock().expect("preferences");
        let mut next = current.clone();
        change(&mut next);
        let mut file = tempfile::NamedTempFile::new_in(&self.data)?;
        serde_json::to_writer(&mut file, &next)?;
        file.persist(self.data.join("preferences.json"))?;
        *current = next;
        Ok(())
    }

    async fn client(&self) -> Result<Arc<Client>> {
        Ok(self
            .session
            .lock()
            .await
            .clone()
            .context("Sign in to continue")?)
    }
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Preferences {
    server: String,
    theme: Theme,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            server: "http://127.0.0.1:7841".into(),
            theme: Theme::System,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct CommandError {
    message: String,
    expired: bool,
}

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        let expired = matches!(error.downcast_ref::<OnlineError>(),
            Some(OnlineError::Rejected { status, .. }) if status.as_u16() == 401);
        Self {
            message: error.to_string(),
            expired,
        }
    }
}

type Result<T> = std::result::Result<T, CommandError>;

#[derive(Serialize)]
pub(crate) struct SignedIn {
    user: User,
    server: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct DropOffer {
    pub ticket: Uuid,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ShareSource {
    File,
    Folder,
    Dropped { ticket: Uuid },
}

#[tauri::command]
pub(crate) fn preferences(state: State<'_, Desktop>) -> Preferences {
    state.preferences.lock().expect("preferences").clone()
}

#[tauri::command]
pub(crate) fn set_theme(state: State<'_, Desktop>, theme: Theme) -> Result<()> {
    Ok(state
        .update_preferences(|value| value.theme = theme)
        .context("Could not save appearance settings")?)
}

#[tauri::command]
pub(crate) async fn login(
    state: State<'_, Desktop>,
    server: String,
    handle: String,
    password: String,
    register: bool,
) -> Result<SignedIn> {
    let mut session = state.session.lock().await;
    if session.is_some() {
        return Err(anyhow!("Sign out before changing accounts").into());
    }
    let client = Client::login(
        &state.data,
        &server,
        Credentials { handle, password },
        register,
    )
    .await?;
    let server = client.server().to_owned();
    if state
        .update_preferences(|value| value.server.clone_from(&server))
        .is_err()
    {
        eprintln!("Could not remember the server address.");
    }
    let result = SignedIn {
        user: client.user.clone(),
        server,
    };
    *session = Some(client);
    Ok(result)
}

#[tauri::command]
pub(crate) async fn logout(state: State<'_, Desktop>) -> Result<()> {
    let mut session = state.session.lock().await;
    *state.drop.lock().expect("drop state") = None;
    if let Some(client) = session.take() {
        client.logout().await?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn sync(state: State<'_, Desktop>) -> Result<Directory> {
    Ok(state.client().await?.sync().await?)
}

#[tauri::command]
pub(crate) async fn directory(state: State<'_, Desktop>) -> Result<Directory> {
    Ok(state.client().await?.directory().await?)
}

#[tauri::command]
pub(crate) async fn messages(
    state: State<'_, Desktop>,
    conversation: Uuid,
    before: Option<i64>,
) -> Result<Page> {
    Ok(state.client().await?.messages(conversation, before).await?)
}

#[tauri::command]
pub(crate) async fn files(
    state: State<'_, Desktop>,
    filter: FileFilter,
    before: Option<i64>,
) -> Result<Page> {
    Ok(state.client().await?.files(filter, before).await?)
}

#[tauri::command]
pub(crate) async fn find_user(state: State<'_, Desktop>, handle: String) -> Result<User> {
    Ok(state.client().await?.find_user(&handle).await?)
}

#[tauri::command]
pub(crate) async fn add_contact(state: State<'_, Desktop>, user_id: Uuid) -> Result<()> {
    Ok(state.client().await?.add_contact(user_id).await?)
}

#[tauri::command]
pub(crate) async fn remove_contact(state: State<'_, Desktop>, user_id: Uuid) -> Result<()> {
    Ok(state.client().await?.remove_contact(user_id).await?)
}

#[tauri::command]
pub(crate) async fn create_conversation(
    state: State<'_, Desktop>,
    request: ConversationRequest,
) -> Result<Uuid> {
    Ok(state.client().await?.create_conversation(request).await?)
}

#[tauri::command]
pub(crate) async fn send_message(
    state: State<'_, Desktop>,
    conversation: Uuid,
    message: SendMessage,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .send_message(conversation, message)
        .await?)
}

#[tauri::command]
pub(crate) async fn share(
    app: AppHandle,
    state: State<'_, Desktop>,
    conversation: Uuid,
    source: ShareSource,
) -> Result<Option<Transfer>> {
    let client = state.client().await?;
    let path = match source {
        ShareSource::Dropped { ticket } => {
            let mut offer = state.drop.lock().expect("drop state");
            if offer.as_ref().is_none_or(|(id, _)| *id != ticket) {
                return Err(anyhow!("Drop the item again to share it").into());
            }
            offer.take().map(|(_, path)| path)
        }
        source => tauri::async_runtime::spawn_blocking(move || {
            let dialog = app.dialog().file();
            match source {
                ShareSource::File => dialog.set_title("Share a file").blocking_pick_file(),
                ShareSource::Folder => dialog.set_title("Share a folder").blocking_pick_folder(),
                ShareSource::Dropped { .. } => unreachable!(),
            }
            .map(|path| path.into_path())
            .transpose()
        })
        .await
        .context("File picker was interrupted")?
        .context("Choose a local file or folder")?,
    };
    match path {
        Some(path) => Ok(Some(client.queue_upload(conversation, path).await?)),
        None => Ok(None),
    }
}

#[tauri::command]
pub(crate) async fn save_attachment(
    app: AppHandle,
    state: State<'_, Desktop>,
    id: Uuid,
) -> Result<Option<Transfer>> {
    let client = state.client().await?;
    let message = client.attachment(id).await?;
    let attachment = message
        .attachment
        .filter(|file| file.is_valid())
        .context("Invalid attachment")?;
    let path = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Save attachment")
            .set_file_name(&attachment.name)
            .blocking_save_file()
            .map(|path| path.into_path())
            .transpose()
    })
    .await
    .context("Save dialog was interrupted")?
    .context("Choose a local save location")?;
    match path {
        Some(path) => Ok(Some(client.queue_download(id, path).await?)),
        None => Ok(None),
    }
}

#[tauri::command]
pub(crate) async fn transfers(state: State<'_, Desktop>) -> Result<Vec<Transfer>> {
    Ok(state.client().await?.transfers().await?)
}

#[tauri::command]
pub(crate) async fn pause_transfer(
    state: State<'_, Desktop>,
    id: Uuid,
    direction: Direction,
) -> Result<()> {
    Ok(state.client().await?.pause(id, direction).await?)
}

#[tauri::command]
pub(crate) async fn resume_transfer(
    state: State<'_, Desktop>,
    id: Uuid,
    direction: Direction,
) -> Result<Transfer> {
    Ok(state.client().await?.resume(id, direction).await?)
}

#[tauri::command]
pub(crate) async fn discard_transfer(
    state: State<'_, Desktop>,
    id: Uuid,
    direction: Direction,
) -> Result<()> {
    Ok(state.client().await?.discard(id, direction).await?)
}
