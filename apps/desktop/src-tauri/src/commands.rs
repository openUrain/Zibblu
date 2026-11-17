use crate::saved_session::{self, SavedAccount, SavedSession};
use anyhow::{Context, anyhow, ensure};
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
    CachedProfile, Client, ContactNotes, ConversationRequest, Credentials, Device, Direction, Directory,
    FileFilter, GroupInvite, GroupInvitePreview, Message, MessageReceipt, OnlineError, Page,
    PublishAiReply, ReadUpdate, SendMessage, SyncUpdate, Transfer, User,
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
    #[cfg(target_os = "ios")]
    pub(crate) private_generation: std::sync::atomic::AtomicU64,
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
            #[cfg(target_os = "ios")]
            private_generation: std::sync::atomic::AtomicU64::new(0),
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

    pub(crate) async fn is_current(&self, client: &Arc<Client>) -> bool {
        self.session.lock().await.as_ref().is_some_and(|active| Arc::ptr_eq(active, client))
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

    pub(crate) fn private_available(&self) -> bool {
        #[cfg(target_os = "ios")]
        return crate::ios::allows_private(self.private_generation.load(std::sync::atomic::Ordering::Acquire));
        #[cfg(not(target_os = "ios"))]
        true
    }

    async fn lobby(&self) -> Result<Arc<PrivateLobby>> {
        let private = self.private.lock().await;
        let lobby = self
            .private_available()
            .then(|| private.clone())
            .flatten()
            .context("Private chat is unavailable. Check the local network and sign-in.")?;
        #[cfg(target_os = "ios")]
        if !lobby.lan_ready() {
            return Err(anyhow!("Private chat is unavailable. Allow Local Network access for Zibblu and check Wi-Fi.").into());
        }
        Ok(lobby)
    }

    pub(crate) fn private_quiet(&self) -> bool {
        self.preferences
            .lock()
            .expect("preferences")
            .as_ref()
            .is_ok_and(|preferences| preferences.private_quiet)
    }

    #[cfg(windows)]
    pub(crate) fn close_action(&self) -> Option<CloseAction> {
        self.preferences
            .lock()
            .expect("preferences")
            .as_ref()
            .map_or(Some(CloseAction::Exit), |preferences| {
                preferences.close_action
            })
    }

    #[cfg(not(target_os = "ios"))]
    pub(crate) fn webview_directory(&self) -> PathBuf {
        self.data.join("webview")
    }

    fn remember(&self, client: &Client, avatar: Option<Vec<u8>>) -> anyhow::Result<()> {
        saved_session::save(
            &self.data,
            &SavedSession {
                account: SavedAccount {
                    user: client.user.clone(),
                    server: client.server().into(),
                    avatar,
                },
                token: client.session_token().into(),
            },
        )
    }

    pub(crate) async fn remember_avatar(&self, client: &Arc<Client>, avatar: Vec<u8>) {
        let session = self.session.lock().await;
        if session
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, client))
            && self.remember(client, Some(avatar)).is_err()
        {
            eprintln!("Could not update the saved account avatar.");
        }
    }

    async fn start_session(
        &self,
        app: AppHandle,
        session: &mut Option<Arc<Client>>,
        client: Arc<Client>,
        notice: Option<&'static str>,
    ) -> SignedIn {
        let result = SignedIn {
            user: client.user.clone(),
            server: client.server().into(),
            notice: notice.or_else(|| {
                client.device_re_registered().then_some(
                    "This device was registered again. Older messages may have expired while it was inactive.",
                )
            }),
        };
        *session = Some(client.clone());
        #[cfg(any(windows, target_os = "ios"))]
        {
            let app = app.clone();
            let client = client.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(profile) = client.profile(client.user.id).await
                    && let Some(version) = profile.avatar
                    && let Ok(bytes) = client.profile_image(client.user.id, version).await
                    && let Ok(avatar) = crate::images::saved_avatar(&bytes)
                {
                    app.state::<Desktop>().remember_avatar(&client, avatar).await;
                }
            });
        }
        *self.private_worker.lock().await = Some(tauri::async_runtime::spawn(
            crate::private_runtime::run(app, client),
        ));
        result
    }

    async fn end_session(
        &self,
        app: &AppHandle,
        session: &mut Option<Arc<Client>>,
        revoke: bool,
    ) -> anyhow::Result<()> {
        let cleared = saved_session::clear(&self.data)
            .context("Could not remove the saved sign-in from this device.");
        self.ai.lock().expect("AI state").clear();
        self.close_private(app).await;
        *self.drop.lock().expect("drop state") = None;
        let revoked = match session.take() {
            Some(client) if revoke => client.logout().await,
            Some(client) => {
                client.close().await;
                Ok(())
            }
            None => Ok(()),
        };
        cleared?;
        if let Err(error) = revoked {
            ensure!(
                is_unauthorized(&error),
                "Signed out on this device. The service could not confirm revocation."
            );
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Preferences {
    server: String,
    #[serde(default)]
    private_quiet: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close_action: Option<CloseAction>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloseAction {
    Exit,
    Tray,
}

impl Preferences {
    fn load(data: &Path) -> anyhow::Result<Self> {
        match std::fs::read(data.join("preferences.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid server settings"),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self {
                server: OFFICIAL_SERVER.into(),
                private_quiet: false,
                close_action: None,
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

    #[test]
    fn close_action_persists_without_replacing_existing_preferences() -> anyhow::Result<()> {
        let data = tempfile::tempdir()?;
        std::fs::write(
            data.path().join("preferences.json"),
            br#"{"server":"https://chat.example.com","private_quiet":true}"#,
        )?;
        assert_eq!(Preferences::load(data.path())?.close_action, None);
        let desktop = Desktop::new(data.path().to_owned());
        for action in [CloseAction::Tray, CloseAction::Exit] {
            desktop.update_preferences(|settings| settings.close_action = Some(action))?;
            let saved = Preferences::load(data.path())?;
            assert_eq!(saved.close_action, Some(action));
            assert_eq!(saved.server, "https://chat.example.com");
            assert!(saved.private_quiet);
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct SignInPreferences {
    #[serde(flatten)]
    saved: Preferences,
    official_server: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_account: Option<SavedAccount>,
}

#[derive(Serialize)]
pub(crate) struct CommandError {
    message: String,
    expired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}

impl From<anyhow::Error> for CommandError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            message: error.to_string(),
            expired: is_unauthorized(&error),
            status: match error.downcast_ref::<OnlineError>() {
                Some(OnlineError::Rejected { status, .. }) => Some(status.as_u16()),
                _ => None,
            },
        }
    }
}

fn is_unauthorized(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<OnlineError>(),
        Some(OnlineError::Rejected { status, .. }) if status.as_u16() == 401)
}

fn invalid_saved_session() -> CommandError {
    CommandError {
        message: "The saved sign-in is unavailable. Enter your password to sign in.".into(),
        expired: true,
        status: Some(401),
    }
}

pub(crate) type Result<T> = std::result::Result<T, CommandError>;

#[derive(Serialize)]
pub(crate) struct SignedIn {
    user: User,
    server: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<&'static str>,
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
pub(crate) fn preferences(
    window: WebviewWindow,
    state: State<'_, Desktop>,
) -> Result<SignInPreferences> {
    let saved = state
        .preferences
        .lock()
        .expect("preferences")
        .as_ref()
        .map_err(|error| anyhow!("{error}"))?
        .clone();
    let saved_account = (window.label() == "main")
        .then(|| {
            let session = saved_session::load(&state.data).ok().flatten()?;
            (session.account.server == saved.server).then(|| session.account.clone())
        })
        .flatten();
    Ok(SignInPreferences {
        saved,
        official_server: OFFICIAL_SERVER,
        saved_account,
    })
}

#[tauri::command]
pub(crate) fn set_close_action(state: State<'_, Desktop>, action: CloseAction) -> Result<()> {
    state.update_preferences(|settings| settings.close_action = Some(action))?;
    Ok(())
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
    let address_saved = state
        .update_preferences(|value| value.server = client.server().into())
        .is_ok();
    let notice = if cfg!(any(windows, target_os = "ios")) {
        let avatar = async {
            let version = client.cached_profile(client.user.id).await?.profile.avatar?;
            let bytes = client.cached_profile_image(client.user.id, version).await?;
            crate::images::saved_avatar(&bytes).ok()
        }.await;
        if address_saved && state.remember(&client, avatar).is_ok() {
            None
        } else if saved_session::clear(&state.data).is_ok() {
            Some("This sign-in could not be saved for next time.")
        } else {
            Some("The saved sign-in could not be replaced or removed.")
        }
    } else {
        (!address_saved).then_some("The server address could not be saved.")
    };
    Ok(state.start_session(app, &mut session, client, notice).await)
}

#[tauri::command]
pub(crate) async fn restore_session(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Desktop>,
    account_id: Uuid,
    server: String,
) -> Result<SignedIn> {
    if window.label() != "main" {
        return Err(invalid_saved_session());
    }
    let mut session = state.session.lock().await;
    if session.is_some() {
        return Err(anyhow!("Sign out before changing accounts").into());
    }
    let saved = saved_session::load(&state.data)
        .ok()
        .flatten()
        .ok_or_else(invalid_saved_session)?;
    let selected = state
        .preferences
        .lock()
        .expect("preferences")
        .as_ref()
        .map_err(|error| anyhow!("{error}"))?
        .server
        .clone();
    if selected != saved.account.server
        || server != saved.account.server
        || account_id != saved.account.user.id
    {
        return Err(invalid_saved_session());
    }
    let client = match Client::restore(
        &state.data,
        &saved.account.server,
        saved.account.user.id,
        &saved.token,
    )
    .await
    {
        Ok(client) => client,
        Err(error) if is_unauthorized(&error) => {
            if saved_session::clear(&state.data).is_err() {
                return Err(CommandError { message: "The saved sign-in was rejected and could not be removed. Enter your password to sign in.".into(), expired: true, status: Some(401) });
            }
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };
    Ok(state.start_session(app, &mut session, client, None).await)
}

#[tauri::command]
pub(crate) async fn logout(app: AppHandle, state: State<'_, Desktop>) -> Result<()> {
    let mut session = state.session.lock().await;
    Ok(state.end_session(&app, &mut session, true).await?)
}

#[tauri::command]
pub(crate) async fn sync(app: AppHandle, state: State<'_, Desktop>) -> Result<SyncUpdate> {
    let client = state.client().await?;
    let result = client.sync_updates().await;
    let mut session = state.session.lock().await;
    if !session
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, &client))
    {
        return Err(anyhow!("The session has ended").into());
    }
    let update = match result {
        Err(error) if is_unauthorized(&error) => {
            let cleanup = state.end_session(&app, &mut session, false).await;
            return Err(CommandError {
                expired: true,
                status: Some(401),
                message: match cleanup {
                    Ok(()) => "Your session has expired. Sign in again.".into(),
                    Err(error) => error.to_string(),
                },
            });
        }
        result => result?,
    };
    if let Some(lobby) = state.private.lock().await.as_ref() {
        lobby.set_memberships(crate::private_runtime::memberships(&update.directory));
    }
    Ok(update)
}

#[tauri::command]
pub(crate) async fn directory(state: State<'_, Desktop>) -> Result<Directory> {
    Ok(state.client().await?.directory().await?)
}

#[tauri::command]
pub(crate) async fn local_update(state: State<'_, Desktop>) -> Result<SyncUpdate> {
    Ok(state.client().await?.local_update().await?)
}

#[tauri::command]
pub(crate) async fn devices(state: State<'_, Desktop>) -> Result<Vec<Device>> {
    Ok(state.client().await?.devices().await?)
}

#[tauri::command]
pub(crate) async fn retire_device(state: State<'_, Desktop>, id: Uuid) -> Result<()> {
    Ok(state.client().await?.retire_device(id).await?)
}

#[tauri::command]
pub(crate) async fn leave_group(state: State<'_, Desktop>, conversation: Uuid) -> Result<()> {
    Ok(state.client().await?.leave_group(conversation).await?)
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
pub(crate) async fn profile(state: State<'_, Desktop>, id: Uuid) -> Result<CachedProfile> {
    let client = state.client().await?;
    let profile = client.profile(id).await?;
    if !state.is_current(&client).await {
        return Err(anyhow!("The session has ended").into());
    }
    Ok(Client::fresh_profile(profile))
}

#[tauri::command]
pub(crate) async fn cached_profile(state: State<'_, Desktop>, id: Uuid) -> Result<Option<CachedProfile>> {
    let client = state.client().await?;
    let profile = client.cached_profile(id).await;
    if !state.is_current(&client).await {
        return Err(anyhow!("The session has ended").into());
    }
    Ok(profile)
}

#[tauri::command]
pub(crate) async fn set_display_name(state: State<'_, Desktop>, name: String) -> Result<()> {
    Ok(state.client().await?.set_display_name(name).await?)
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
) -> Result<Option<MessageReceipt>> {
    Ok(state
        .client()
        .await?
        .update_receipt(conversation, update)
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
        .queue_message(conversation, message)
        .await?)
}

#[tauri::command]
pub(crate) async fn retry_message(state: State<'_, Desktop>, nonce: Uuid) -> Result<()> {
    Ok(state.client().await?.retry_message(nonce).await?)
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
    if !state.private_available() {
        return Ok(NearbyDevices { status: DiscoveryStatus::NetworkUnavailable, devices: Vec::new() });
    }
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
    if !state.private_available() {
        return Ok(PrivateList { rooms: Vec::new(), prompt: None });
    }
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
    _window: WebviewWindow,
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
        #[cfg(desktop)]
        if _window.label() == crate::private_runtime::INVITATION {
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
            let dialog = window.dialog().file();
            #[cfg(desktop)]
            let dialog = dialog.set_parent(&window);
            let chosen = match source {
                ShareSource::File => dialog.set_title("Share a file").blocking_pick_file(),
                #[cfg(desktop)]
                ShareSource::Folder => dialog.set_title("Share a folder").blocking_pick_folder(),
                #[cfg(mobile)]
                ShareSource::Folder => anyhow::bail!("Choose a file or ZIP archive on iOS."),
                ShareSource::Dropped { .. } => unreachable!(),
            };
            Ok::<_, anyhow::Error>(chosen.map(|path| path.into_path()).transpose()?)
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
    #[cfg(target_os = "ios")]
    {
        let _ = window;
        let directory = client.profile_dir().join("downloads").join(id.to_string());
        tokio::fs::create_dir_all(&directory).await.context("Could not prepare the app's download folder")?;
        let path = directory.join(&attachment.name);
        if path.is_file() {
            tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
                use sha2::{Digest, Sha256};
                use std::io::Read;
                let file = std::fs::File::open(&path)?;
                ensure!(file.metadata()?.is_file(), "The saved attachment is unavailable.");
                let mut hash = Sha256::new();
                let size = std::io::copy(&mut file.take(attachment.size + 1), &mut hash)?;
                ensure!(size == attachment.size && format!("{:x}", hash.finalize()) == attachment.sha256,
                    "The saved file no longer matches this attachment.");
                crate::ios::share_file(&path)
            }).await.context("File sharing was interrupted")??;
            return Ok(None);
        }
        Ok(Some(client.queue_download(id, path).await?))
    }
    #[cfg(not(target_os = "ios"))]
    {
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
