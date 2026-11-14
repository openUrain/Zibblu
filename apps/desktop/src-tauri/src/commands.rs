use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use uuid::Uuid;
use zibblu_core::private_chat::{
    DiscoveryStatus, DismissedRooms, NearbyDevices, Phase, PrivateCode, PrivateLobby,
    PrivateSummary, PrivateView,
};
use zibblu_core::{
    Client, ContactNotes, ConversationRequest, Credentials, Direction, Directory, FileFilter,
    GroupInvite, GroupInvitePreview, Message, OnlineError, Page, Profile, PublishAiReply,
    ReadUpdate, SendMessage, SyncUpdate, Transfer, User,
};

const OFFICIAL_SERVER: &str = "https://zibblu-api.openurain.com";

pub(crate) struct Desktop {
    data: PathBuf,
    preferences: StdMutex<anyhow::Result<Preferences>>,
    session: Mutex<Option<Arc<Client>>>,
    pub(crate) ai: StdMutex<crate::ai::AiState>,
    pub(crate) private: Mutex<Option<Arc<PrivateLobby>>>,
    pub(crate) discovery: StdMutex<DiscoveryStatus>,
    pub(crate) dismissed_rooms: DismissedRooms,
    private_worker: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    pub drop: StdMutex<Option<(Uuid, PathBuf)>>,
}

impl Desktop {
    pub fn new(data: PathBuf) -> Self {
        let preferences = Preferences::load(&data);
        Self {
            data,
            preferences: StdMutex::new(preferences),
            session: Mutex::new(None),
            ai: StdMutex::default(),
            private: Mutex::new(None),
            discovery: StdMutex::new(DiscoveryStatus::Starting),
            dismissed_rooms: DismissedRooms::default(),
            private_worker: Mutex::new(None),
            drop: StdMutex::new(None),
        }
    }

    fn update_preferences(&self, change: impl FnOnce(&mut Preferences)) -> anyhow::Result<()> {
        let mut current = self.preferences.lock().expect("preferences");
        let mut next = current
            .as_ref()
            .map_err(|error| anyhow!("{error}"))?
            .clone();
        change(&mut next);
        let mut file = tempfile::NamedTempFile::new_in(&self.data)?;
        serde_json::to_writer(&mut file, &next)?;
        file.persist(self.data.join("preferences.json"))?;
        *current = Ok(next);
        Ok(())
    }

    pub(crate) async fn client(&self) -> Result<Arc<Client>> {
        Ok(self
            .session
            .lock()
            .await
            .clone()
            .context("Sign in to continue")?)
    }

    pub async fn close_private(&self, app: &AppHandle) {
        let mut worker = self.private_worker.lock().await;
        if let Some(task) = worker.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(window) = app.get_webview_window(crate::private_runtime::INVITATION) {
            let _ = window.close();
        }
        let lobby = self.private.lock().await.take();
        if let Some(lobby) = lobby {
            lobby.quit().await;
        }
        self.dismissed_rooms.clear();
        *self.discovery.lock().expect("discovery status") = DiscoveryStatus::Starting;
    }

    async fn lobby(&self) -> Result<Arc<PrivateLobby>> {
        Ok(self
            .private
            .lock()
            .await
            .clone()
            .context("Private chat is unavailable. Check the local network and sign-in.")?)
    }

    pub(crate) fn private_quiet(&self) -> bool {
        self.preferences
            .lock()
            .expect("preferences")
            .as_ref()
            .is_ok_and(|preferences| preferences.private_quiet)
    }

    pub(crate) fn webview_directory(&self) -> PathBuf {
        self.data.join("webview")
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Preferences {
    server: String,
    #[serde(default)]
    private_quiet: bool,
}

impl Preferences {
    fn load(data: &Path) -> anyhow::Result<Self> {
        match std::fs::read(data.join("preferences.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid server settings"),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self {
                server: OFFICIAL_SERVER.into(),
                private_quiet: false,
            }),
            Err(error) => Err(error.into()),
        }
        .context("Could not read saved server settings. Check preferences.json in your Zibblu data folder and restart the app.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_missing_settings_default_to_zibblu_online() -> anyhow::Result<()> {
        let data = tempfile::tempdir()?;
        assert_eq!(Preferences::load(data.path())?.server, OFFICIAL_SERVER);

        let file = data.path().join("preferences.json");
        for server in ["http://127.0.0.1:7841/", "https://chat.example.com:8443/"] {
            std::fs::write(
                &file,
                format!(r#"{{"server":"{server}","private_quiet":true}}"#),
            )?;
            let saved = Preferences::load(data.path())?;
            assert_eq!(saved.server, server);
            assert!(saved.private_quiet);
        }
        std::fs::write(file, b"{broken")?;
        assert!(Preferences::load(data.path()).is_err());
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct SignInPreferences {
    #[serde(flatten)]
    saved: Preferences,
    official_server: &'static str,
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

pub(crate) type Result<T> = std::result::Result<T, CommandError>;

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
pub(crate) fn preferences(state: State<'_, Desktop>) -> Result<SignInPreferences> {
    let current = state.preferences.lock().expect("preferences");
    Ok(SignInPreferences {
        saved: current
            .as_ref()
            .map_err(|error| anyhow!("{error}"))?
            .clone(),
        official_server: OFFICIAL_SERVER,
    })
}

#[tauri::command]
pub(crate) async fn login(
    app: AppHandle,
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
    *session = Some(client.clone());
    *state.private_worker.lock().await = Some(tauri::async_runtime::spawn(
        crate::private_runtime::run(app, client),
    ));
    Ok(result)
}

#[tauri::command]
pub(crate) async fn logout(app: AppHandle, state: State<'_, Desktop>) -> Result<()> {
    let mut session = state.session.lock().await;
    state.ai.lock().expect("AI state").clear();
    state.close_private(&app).await;
    *state.drop.lock().expect("drop state") = None;
    if let Some(client) = session.take() {
        client.logout().await?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn sync(state: State<'_, Desktop>) -> Result<SyncUpdate> {
    let client = state.client().await?;
    let update = client.sync_updates().await?;
    let session = state.session.lock().await;
    if session
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &client))
        && let Some(lobby) = state.private.lock().await.as_ref()
    {
        lobby.set_memberships(crate::private_runtime::memberships(&update.directory));
    }
    Ok(update)
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
    search: Option<String>,
) -> Result<Page> {
    Ok(state
        .client()
        .await?
        .search_messages(conversation, before, search.as_deref())
        .await?)
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
pub(crate) async fn add_contact(
    state: State<'_, Desktop>,
    user_id: Uuid,
    greeting: String,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .request_contact(user_id, greeting)
        .await?)
}

#[tauri::command]
pub(crate) async fn respond_contact(
    state: State<'_, Desktop>,
    user_id: Uuid,
    accept: bool,
    request_id: Option<Uuid>,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .decide_contact(user_id, accept, request_id)
        .await?)
}

#[tauri::command]
pub(crate) async fn profile(state: State<'_, Desktop>, id: Uuid) -> Result<Profile> {
    Ok(state.client().await?.profile(id).await?)
}

#[tauri::command]
pub(crate) async fn set_bio(state: State<'_, Desktop>, bio: String) -> Result<()> {
    Ok(state.client().await?.set_bio(bio).await?)
}

#[tauri::command]
pub(crate) async fn set_contact_notes(
    state: State<'_, Desktop>,
    id: Uuid,
    notes: ContactNotes,
) -> Result<()> {
    Ok(state.client().await?.set_contact_notes(id, notes).await?)
}

#[tauri::command]
pub(crate) async fn set_read_receipts(state: State<'_, Desktop>, enabled: bool) -> Result<()> {
    Ok(state.client().await?.set_read_receipts(enabled).await?)
}

#[tauri::command]
pub(crate) async fn read_update(
    state: State<'_, Desktop>,
    conversation: Uuid,
    update: ReadUpdate,
) -> Result<Vec<i64>> {
    Ok(state
        .client()
        .await?
        .read_update(conversation, update)
        .await?)
}

#[tauri::command]
pub(crate) async fn hide_message(
    state: State<'_, Desktop>,
    conversation: Uuid,
    seq: i64,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .hide_message(conversation, seq)
        .await?)
}

#[tauri::command]
pub(crate) async fn quoted_message(
    state: State<'_, Desktop>,
    conversation: Uuid,
    seq: i64,
) -> Result<Option<Message>> {
    Ok(state
        .client()
        .await?
        .quoted_message(conversation, seq)
        .await?)
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
pub(crate) async fn recall_message(
    state: State<'_, Desktop>,
    conversation: Uuid,
    seq: i64,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .recall_message(conversation, seq)
        .await?)
}

#[tauri::command]
pub(crate) async fn publish_ai_reply(
    state: State<'_, Desktop>,
    conversation: Uuid,
    reply: PublishAiReply,
) -> Result<()> {
    Ok(state
        .client()
        .await?
        .publish_ai_reply(conversation, reply)
        .await?)
}

#[tauri::command]
pub(crate) async fn private_start(
    state: State<'_, Desktop>,
    conversation: Option<Uuid>,
    invited: Vec<Uuid>,
    open: bool,
) -> Result<Uuid> {
    let id = state.lobby().await?.create(conversation, invited, open)?;
    state.ai.lock().expect("AI state").clear();
    Ok(id)
}

#[tauri::command]
pub(crate) async fn private_nearby(state: State<'_, Desktop>) -> Result<NearbyDevices> {
    let lobby = state.private.lock().await.clone();
    Ok(match lobby {
        Some(lobby) => lobby.nearby_devices().await,
        None => NearbyDevices {
            status: *state.discovery.lock().expect("discovery status"),
            devices: Vec::new(),
        },
    })
}

#[tauri::command]
pub(crate) async fn private_invite_nearby(
    state: State<'_, Desktop>,
    device: Uuid,
    account: Option<Uuid>,
) -> Result<Uuid> {
    let id = state.lobby().await?.invite_nearby(device, account).await?;
    state.ai.lock().expect("AI state").clear();
    Ok(id)
}

#[derive(Serialize)]
pub(crate) struct PrivateList {
    rooms: Vec<PrivateSummary>,
    prompt: Option<Uuid>,
}

#[tauri::command]
pub(crate) async fn private_list(
    state: State<'_, Desktop>,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<PrivateList> {
    let rooms = state
        .private
        .lock()
        .await
        .as_ref()
        .map_or_else(Vec::new, |lobby| lobby.list());
    let prompt = crate::private_runtime::prompt_window(&app, &state)
        .filter(|label| *label == window.label())
        .and_then(|_| rooms.iter().find(|room| room.phase == Phase::Invited))
        .map(|room| room.id);
    Ok(PrivateList { rooms, prompt })
}

#[tauri::command]
pub(crate) async fn private_view(
    state: State<'_, Desktop>,
    id: Uuid,
    after: u64,
) -> Result<PrivateView> {
    Ok(state.lobby().await?.view(id, after)?)
}

#[tauri::command]
pub(crate) async fn private_respond(
    state: State<'_, Desktop>,
    app: AppHandle,
    window: WebviewWindow,
    id: Uuid,
    accept: bool,
) -> Result<()> {
    let conversation = state.lobby().await?.respond(id, accept)?;
    if accept {
        state.ai.lock().expect("AI state").clear();
    }
    if accept && let Some(main) = app.get_webview_window("main") {
        let _ = main.emit(
            "private-invitation-accepted",
            serde_json::json!({ "id": id, "conversation": conversation }),
        );
        if window.label() == crate::private_runtime::INVITATION {
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn private_send(state: State<'_, Desktop>, id: Uuid, text: String) -> Result<()> {
    Ok(state.lobby().await?.send(id, text)?)
}

#[tauri::command]
pub(crate) async fn private_end(state: State<'_, Desktop>, id: Uuid) -> Result<()> {
    if let Some(lobby) = state.private.lock().await.as_ref() {
        lobby.end(id);
    } else {
        state.dismissed_rooms.record(id);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn private_qr(state: State<'_, Desktop>, id: Uuid) -> Result<PrivateCode> {
    Ok(state.lobby().await?.code(id)?)
}

#[tauri::command]
pub(crate) async fn private_scan(state: State<'_, Desktop>, code: PrivateCode) -> Result<Uuid> {
    let id = state.lobby().await?.scan(code).await?;
    state.ai.lock().expect("AI state").clear();
    Ok(id)
}

#[tauri::command]
pub(crate) fn set_private_quiet(state: State<'_, Desktop>, quiet: bool) -> Result<()> {
    Ok(state.update_preferences(|preferences| preferences.private_quiet = quiet)?)
}

#[tauri::command]
pub(crate) async fn group_invite(
    state: State<'_, Desktop>,
    conversation: Uuid,
) -> Result<GroupInvite> {
    Ok(state
        .client()
        .await?
        .issue_group_invite(conversation)
        .await?)
}

#[tauri::command]
pub(crate) async fn group_preview(
    state: State<'_, Desktop>,
    code: Uuid,
) -> Result<GroupInvitePreview> {
    Ok(state.client().await?.preview_group_invite(code).await?)
}

#[tauri::command]
pub(crate) async fn group_join(state: State<'_, Desktop>, code: Uuid) -> Result<Uuid> {
    Ok(state.client().await?.join_group_invite(code).await?)
}

#[tauri::command]
pub(crate) async fn share(
    window: WebviewWindow,
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
            let dialog = window.dialog().file().set_parent(&window);
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
    window: WebviewWindow,
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
        window
            .dialog()
            .file()
            .set_parent(&window)
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
