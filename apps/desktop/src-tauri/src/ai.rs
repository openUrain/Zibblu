use crate::commands::{Desktop, Result as CommandResult};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{Notify, watch};
use uuid::Uuid;
use zeroize::Zeroizing;
use zibblu_core::{
    AiLocalPrompt, AiLocalReply, AiScope, Attachment, AttachmentKind, Client, MAX_AI_PROMPT_BYTES,
    assistant::{
        AiCancelledError, AiFile, AiMessage, AiProvider, AiSnapshot, ModelDiscovery, valid_model_id,
    },
    assistant_extract::{
        AuthorizedAiAttachment, ExtractError, ExtractedAttachment, attachment_type_label,
        build_chunked_requests, check_supported_attachment_for_provider, extract_attachment,
    },
};

#[cfg(windows)]
#[path = "ai_agent.rs"]
pub(crate) mod agent;

#[cfg(windows)]
#[path = "ai_windows.rs"]
mod windows;

#[cfg(windows)]
use windows::{protect_key, read_text_file};

#[cfg(target_os = "ios")]
use crate::ios::{delete_key, protect_key, read_text_file};

#[cfg(not(any(windows, target_os = "ios")))]
fn protect_key(_: AiProvider, _: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("AI assistants require secure OS credential storage")
}

#[cfg(not(any(windows, target_os = "ios")))]
fn read_text_file(_: &Path) -> Result<AiFile> {
    anyhow::bail!("AI file selection is unavailable on this platform")
}

const MAX_FILES: usize = 3;
#[cfg(any(windows, target_os = "ios"))]
pub(crate) const MAX_FILE_BYTES: u64 = 32 * 1024;
const AI_CHANGED_EVENT: &str = "zibblu-ai-changed";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Assistant {
    provider: AiProvider,
    name: String,
    model: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    agent_enabled: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredAssistant {
    info: Assistant,
    protected_key: Vec<u8>,
}

fn load_assistants(profile: &Path) -> Result<Vec<StoredAssistant>> {
    match fs::read(profile.join("assistants.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Could not read assistant settings"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).context("Could not open assistant settings"),
    }
}

fn store_assistants(profile: &Path, assistants: &[StoredAssistant]) -> Result<Vec<Assistant>> {
    let mut file =
        tempfile::NamedTempFile::new_in(profile).context("Could not save assistant settings")?;
    serde_json::to_writer(file.as_file_mut(), assistants)?;
    file.persist(profile.join("assistants.json"))
        .map_err(|_| anyhow::anyhow!("Could not save assistant settings"))?;
    Ok(assistants
        .iter()
        .map(|assistant| assistant.info.clone())
        .collect())
}

#[tauri::command]
pub(crate) async fn ai_assistants(state: State<'_, Desktop>) -> CommandResult<Vec<Assistant>> {
    let client = state.client().await?;
    Ok(load_assistants(client.profile_dir())?
        .into_iter()
        .map(|assistant| assistant.info)
        .collect())
}

#[tauri::command]
pub(crate) async fn ai_save_assistant(
    app: AppHandle,
    state: State<'_, Desktop>,
    provider: AiProvider,
    name: String,
    model: String,
    key: Option<String>,
    base_url: Option<String>,
) -> CommandResult<Vec<Assistant>> {
    let key = key.map(Zeroizing::new);
    let client = state.client().await?;
    let name = name.trim().to_lowercase();
    let model = model.trim().to_owned();
    let base_url = base_url
        .filter(|url| !url.trim().is_empty())
        .map(|url| provider.base_url(Some(&url)).map(|base| base.to_string()))
        .transpose()?;
    let _settings = state.ai.lock().expect("AI state");
    let mut assistants = load_assistants(client.profile_dir())?;
    let result = (|| -> Result<_> {
        ensure!(
            (1..=32).contains(&name.chars().count())
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_')),
            "Use 1–32 letters, numbers, underscores or hyphens for the mention name"
        );
        ensure!(valid_model_id(&model), "Enter the provider's model ID");
        ensure!(
            assistants
                .iter()
                .all(|entry| entry.info.provider == provider
                    || entry.info.name.trim().to_lowercase() != name),
            "That mention name is already in use"
        );
        let protected_key = if let Some(key) = key.as_ref() {
            protect_key(provider, validated_key(key)?.as_bytes(), true)?
        } else {
            stored_at(&assistants, provider, base_url.as_deref())?
                .protected_key
                .clone()
        };
        #[cfg(target_os = "ios")]
        let rotated = assistants
            .iter()
            .find(|entry| entry.info.provider == provider)
            .filter(|_| key.is_some())
            .map(|entry| entry.protected_key.clone());
        let agent_enabled = assistants.iter().any(|entry| entry.info.provider == provider
            && entry.info.base_url == base_url && entry.info.model == model && entry.info.agent_enabled);
        assistants.retain(|entry| entry.info.provider != provider);
        assistants.push(StoredAssistant {
            info: Assistant {
                provider,
                name,
                model,
                base_url,
                agent_enabled,
            },
            protected_key,
        });
        let stored = store_assistants(client.profile_dir(), &assistants);
        #[cfg(target_os = "ios")]
        if key.is_some() {
            if stored.is_err() {
                delete_key(
                    provider,
                    &assistants.last().expect("new assistant").protected_key,
                );
            } else if let Some(previous) = rotated {
                delete_key(provider, &previous);
            }
        }
        stored
    })();
    let stored = result?;
    let _ = app.emit("zibblu-assistant-settings", ());
    Ok(stored)
}

fn validated_key(key: &str) -> Result<&str> {
    let key = key.trim();
    ensure!(
        (1..=512).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic()),
        "Enter a valid API key"
    );
    Ok(key)
}

fn stored_at<'a>(
    assistants: &'a [StoredAssistant],
    provider: AiProvider,
    base: Option<&str>,
) -> Result<&'a StoredAssistant> {
    let stored = assistants
        .iter()
        .find(|entry| entry.info.provider == provider)
        .context("Enter an API key")?;
    ensure!(
        provider.base_url(stored.info.base_url.as_deref())? == provider.base_url(base)?,
        "Enter the API key for the new base URL"
    );
    Ok(stored)
}

#[tauri::command]
pub(crate) async fn ai_test_connection(
    state: State<'_, Desktop>,
    provider: AiProvider,
    base_url: Option<String>,
    key: Option<String>,
) -> CommandResult<ModelDiscovery> {
    let key = key.map(Zeroizing::new);
    let client = state.client().await?;
    let base = provider.base_url(base_url.as_deref())?;
    let bytes = match &key {
        Some(key) => validated_key(key)?.as_bytes().to_vec(),
        None => {
            let assistants = load_assistants(client.profile_dir())?;
            let stored = stored_at(&assistants, provider, base_url.as_deref())?;
            protect_key(provider, &stored.protected_key, false)?
        }
    };
    let key = Zeroizing::new(bytes);
    let key = std::str::from_utf8(&key).context("Replace the stored API key in Settings")?;
    Ok(provider.discover_models(&base, key).await?)
}

#[tauri::command]
pub(crate) async fn ai_remove_assistant(
    app: AppHandle,
    state: State<'_, Desktop>,
    provider: AiProvider,
) -> CommandResult<Vec<Assistant>> {
    let client = state.client().await?;
    let _settings = state.ai.lock().expect("AI state");
    let mut assistants = load_assistants(client.profile_dir())?;
    #[cfg(target_os = "ios")]
    let removed = assistants
        .iter()
        .find(|entry| entry.info.provider == provider)
        .map(|entry| entry.protected_key.clone());
    assistants.retain(|entry| entry.info.provider != provider);
    let stored = store_assistants(client.profile_dir(), &assistants)?;
    #[cfg(target_os = "ios")]
    if let Some(reference) = removed {
        delete_key(provider, &reference);
    }
    let _ = app.emit("zibblu-assistant-settings", ());
    Ok(stored)
}

#[derive(Default)]
pub(crate) struct AiState {
    views: HashMap<Uuid, NormalView>,
    jobs: HashMap<Uuid, ActiveAiJob>,
}

struct NormalView {
    conversation: Uuid,
    draft: Option<Draft>,
}

struct Draft {
    id: Uuid,
    files: Vec<(Uuid, AiFile)>,
    approval: Option<Approval>,
    preparing: Option<Uuid>,
}

#[derive(Clone)]
struct ApprovedAttachment {
    seq: i64,
    attachment: Attachment,
    cached: bool,
}

struct Approval {
    id: Uuid,
    conversation_id: Uuid,
    assistant: StoredAssistant,
    prompt: String,
    message_seqs: Vec<i64>,
    messages: Vec<AiMessage>,
    files: Vec<AiFile>,
    attachments: Vec<ApprovedAttachment>,
    autonomous: bool,
    #[cfg(windows)]
    effort: Option<String>,
}

struct ActiveAiJob {
    id: Uuid,
    prompt_id: Uuid,
    conversation_id: Uuid,
    provider: AiProvider,
    phase: String,
    status_text: String,
    progress: Option<(u64, u64)>,
    error: Option<String>,
    created_at: i64,
    cancelled: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    cancel_tx: watch::Sender<bool>,
    #[cfg(windows)]
    autonomous: bool,
    #[cfg(windows)]
    access: Option<agent::PendingAccess>,
    #[cfg(windows)]
    targets: std::collections::HashSet<Uuid>,
}

impl ActiveAiJob {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.cancel_tx.send(true);
        self.cancel_notify.notify_waiters();
    }

    fn view(&self) -> AiJobView {
        AiJobView {
            id: self.id,
            prompt_id: self.prompt_id,
            conversation_id: self.conversation_id,
            provider: self.provider,
            phase: self.phase.clone(),
            status_text: self.status_text.clone(),
            progress: self.progress,
            error: self.error.clone(),
            created_at: self.created_at,
            #[cfg(windows)]
            access: self.access.as_ref().map(|pending| pending.view.clone()),
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct AiJobView {
    id: Uuid,
    prompt_id: Uuid,
    conversation_id: Uuid,
    provider: AiProvider,
    phase: String,
    status_text: String,
    progress: Option<(u64, u64)>,
    error: Option<String>,
    created_at: i64,
    #[cfg(windows)]
    access: Option<agent::AccessView>,
}

impl AiState {
    #[cfg(windows)]
    fn suspend_conversation(&mut self, conversation: Uuid) -> Vec<Uuid> {
        self.views.retain(|_, view| view.conversation != conversation);
        let mut origins = Vec::new();
        self.jobs.retain(|_, job| {
            if job.conversation_id == conversation || job.targets.contains(&conversation) {
                job.cancel();
                origins.push(job.conversation_id);
                false
            } else {
                true
            }
        });
        origins
    }

    fn begin_saving(&mut self, id: Uuid) -> bool {
        let Some(job) = self.jobs.get_mut(&id) else { return false; };
        if job.cancelled.load(Ordering::Acquire) { return false; }
        job.phase = "saving".into();
        true
    }

    pub(crate) fn clear_pending_views(&mut self) {
        self.views.clear();
    }

    pub(crate) fn clear(&mut self) {
        for (_, job) in self.jobs.drain() {
            job.cancel();
        }
        self.views.clear();
    }

    pub(crate) fn clear_conversation(&mut self, conversation: Uuid) {
        let mut cancel_ids = Vec::new();
        for (id, job) in &self.jobs {
            if job.conversation_id == conversation {
                job.cancel();
                cancel_ids.push(*id);
            }
        }
        for id in cancel_ids {
            self.jobs.remove(&id);
        }
        for view in self.views.values_mut() {
            if view.conversation == conversation {
                view.draft = None;
            }
        }
    }

    fn view(&mut self, id: Uuid) -> Result<&mut NormalView> {
        self.views
            .get_mut(&id)
            .context("Open a normal chat to ask an AI assistant")
    }

    fn draft(&mut self, view: Uuid, draft: Uuid) -> Result<&mut Draft> {
        self.view(view)?
            .draft
            .as_mut()
            .filter(|entry| entry.id == draft)
            .context("Open a new AI request")
    }

    fn consume(&mut self, view: Uuid, draft: Uuid, approval: Uuid) -> Result<Approval> {
        let request = self.draft(view, draft)?;
        ensure!(
            request
                .approval
                .as_ref()
                .is_some_and(|entry| entry.id == approval),
            "Review a fresh disclosure before sending"
        );
        let mut request = self.view(view)?.draft.take().expect("checked draft");
        Ok(request.approval.take().expect("checked approval"))
    }
}

#[cfg(windows)]
pub(crate) fn suspend_conversation(app: &AppHandle, conversation: Uuid) {
    let desktop = app.state::<Desktop>();
    let origins = desktop.ai.lock().expect("AI state").suspend_conversation(conversation);
    if !origins.is_empty() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            for origin in origins {
                let _ = app.emit(AI_CHANGED_EVENT, origin);
            }
        });
    }
}

#[cfg(any(windows, test))]
fn ensure_ordinary(origin: Uuid, target: Uuid, is_private: impl Fn(Uuid) -> bool) -> Result<()> {
    if is_private(origin) || (target != origin && is_private(target)) {
        return Err(AiCancelledError.into());
    }
    Ok(())
}

pub(crate) async fn ensure_available(state: &Desktop, client: &Arc<Client>, origin: Uuid, target: Uuid) -> Result<()> {
    ensure!(state.is_current(client).await, "The session has ended.");
    #[cfg(windows)]
    {
        let lobby = state.private.lock().await;
        ensure_ordinary(origin, target, |id| {
            lobby.as_ref().is_some_and(|lobby| lobby.has_active_conversation(id))
        })?;
    }
    #[cfg(windows)]
    if target == zibblu_core::ZIBBLU_ASSISTANT {
        ensure!(origin == target, "This conversation is unavailable.");
        return Ok(());
    }
    if origin != target && !(cfg!(windows) && origin == zibblu_core::ZIBBLU_ASSISTANT) {
        client.ensure_ai_authorized(origin, &[], &[]).await?;
    }
    client.ensure_ai_authorized(target, &[], &[]).await
}

fn ensure_normal_chat_window(window: &WebviewWindow, conversation: Uuid) -> Result<()> {
    let label = window.label();
    if label == "main" {
        return Ok(());
    }
    if let Some(expected) = crate::chat_window::conversation_id_for_label(label) {
        ensure!(
            expected == conversation,
            "AI is only available in this chat"
        );
        return Ok(());
    }
    bail!("AI is only available in normal chats")
}

#[tauri::command]
pub(crate) fn ai_open_view(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    view: Uuid,
    conversation: Uuid,
) -> CommandResult<()> {
    if conversation == zibblu_core::ZIBBLU_ASSISTANT {
        return Err(anyhow::anyhow!("Open the Zibblu Assistant conversation.").into());
    }
    ensure_normal_chat_window(&window, conversation)?;
    state.ai.lock().expect("AI state").views.insert(
        view,
        NormalView {
            conversation,
            draft: None,
        },
    );
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_close_view(state: State<'_, Desktop>, view: Uuid) {
    state.ai.lock().expect("AI state").views.remove(&view);
}

#[tauri::command]
pub(crate) fn ai_begin(state: State<'_, Desktop>, view: Uuid, draft: Uuid) -> CommandResult<()> {
    state.ai.lock().expect("AI state").view(view)?.draft = Some(Draft {
        id: draft,
        files: Vec::new(),
        approval: None,
        preparing: None,
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_cancel(state: State<'_, Desktop>, view: Uuid, draft: Uuid) {
    let mut ai = state.ai.lock().expect("AI state");
    if let Ok(active) = ai.view(view)
        && active.draft.as_ref().is_some_and(|entry| entry.id == draft)
    {
        active.draft = None;
    }
}

#[derive(Serialize)]
pub(crate) struct PickedFile {
    id: Uuid,
    name: String,
    bytes: usize,
}

#[tauri::command]
pub(crate) async fn ai_pick_file(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    view: Uuid,
    draft: Uuid,
) -> CommandResult<Option<PickedFile>> {
    let client = state.client().await?;
    let conversation = {
        let mut ai = state.ai.lock().expect("AI state");
        check_file_limit(ai.draft(view, draft)?)?;
        ai.view(view)?.conversation
    };
    client.ai_messages(conversation, AiScope::Prompt).await?;
    let chosen = tauri::async_runtime::spawn_blocking(move || -> Result<Option<AiFile>> {
        let dialog = window.dialog().file();
        #[cfg(desktop)]
        let dialog = dialog.set_parent(&window);
        let Some(path) = dialog
            .set_title("Choose a text file for this AI request")
            .blocking_pick_file()
        else {
            return Ok(None);
        };
        let path = path.into_path().context("Choose a local file")?;
        Ok(Some(read_text_file(&path)?))
    })
    .await
    .context("File selection was interrupted")??;
    let Some(file) = chosen else {
        return Ok(None);
    };
    let mut ai = state.ai.lock().expect("AI state");
    let request = ai.draft(view, draft)?;
    check_file_limit(request)?;
    let id = Uuid::new_v4();
    let picked = PickedFile {
        id,
        name: file.name.clone(),
        bytes: file.text.len(),
    };
    request.approval = None;
    request.preparing = None;
    request.files.push((id, file));
    Ok(Some(picked))
}

fn check_file_limit(request: &Draft) -> Result<()> {
    ensure!(
        request.files.len() < MAX_FILES,
        "Select up to three text files"
    );
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_remove_file(
    state: State<'_, Desktop>,
    view: Uuid,
    draft: Uuid,
    id: Uuid,
) -> CommandResult<()> {
    let mut ai = state.ai.lock().expect("AI state");
    let request = ai.draft(view, draft)?;
    request.approval = None;
    request.preparing = None;
    request.files.retain(|(file, _)| *file != id);
    Ok(())
}

fn select_files(request: &Draft, ids: Vec<Uuid>) -> Result<Vec<AiFile>> {
    let files: Vec<_> = request
        .files
        .iter()
        .filter(|(id, _)| ids.contains(id))
        .map(|(_, file)| file.clone())
        .collect();
    ensure!(
        files.len() == ids.len(),
        "Some files are unavailable for this request. Select them again."
    );
    Ok(files)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    provider: AiProvider,
    prompt: String,
    scope: AiScope,
    files: Vec<Uuid>,
}

#[derive(Serialize)]
pub(crate) struct PreparedMessage {
    sender: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct PreparedAttachment {
    seq: i64,
    id: Uuid,
    name: String,
    kind: AttachmentKind,
    file_type: String,
    size: u64,
    cached: bool,
}

#[derive(Serialize)]
pub(crate) struct PreparedSnapshot {
    prompt: String,
    messages: Vec<PreparedMessage>,
    attachments: Vec<PreparedAttachment>,
    files: Vec<AiFile>,
}

#[derive(Serialize)]
pub(crate) struct Prepared {
    id: Uuid,
    endpoint: String,
    assistant: Assistant,
    snapshot: PreparedSnapshot,
}

#[tauri::command]
pub(crate) async fn ai_prepare(
    state: State<'_, Desktop>,
    view: Uuid,
    draft: Uuid,
    selection: Selection,
) -> CommandResult<Prepared> {
    let client = state.client().await?;
    let prompt = selection.prompt.trim().to_owned();
    let id = Uuid::new_v4();
    let conversation = {
        let mut ai = state.ai.lock().expect("AI state");
        let request = ai.draft(view, draft)?;
        request.approval = None;
        request.preparing = Some(id);
        ai.view(view)?.conversation
    };
    ensure_available(&state, &client, conversation, conversation).await?;
    let context_messages = client
        .ai_context_messages(conversation, selection.scope)
        .await?;

    let mut prepared_messages = Vec::with_capacity(context_messages.len());
    let mut encoded_messages = Vec::with_capacity(context_messages.len());
    let mut approved_message_seqs = Vec::with_capacity(context_messages.len());
    let mut prepared_attachments = Vec::new();
    let mut approved_attachments = Vec::new();

    for msg in context_messages {
        approved_message_seqs.push(msg.seq);
        let att_name = msg.attachment.as_ref().map(|a| a.name.clone());
        let display_text = msg.text.clone().unwrap_or_default();
        let encoded_text = match (msg.text, &msg.attachment) {
            (Some(t), Some(a)) if !t.is_empty() => format!("{t}\n[Attachment: {}]", a.name),
            (Some(t), _) if !t.is_empty() => t,
            (_, Some(a)) => format!("[Attachment: {}]", a.name),
            _ => String::new(),
        };
        if let Some(att) = msg.attachment {
            check_supported_attachment_for_provider(&att, selection.provider)
                .map_err(|err| anyhow::anyhow!("{err}"))?;
            let cached = client
                .is_attachment_cached_for_ai(conversation, msg.seq, &att)
                .await?;
            prepared_attachments.push(PreparedAttachment {
                seq: msg.seq,
                id: att.id,
                name: att.name.clone(),
                kind: att.kind,
                file_type: attachment_type_label(&att),
                size: att.size,
                cached,
            });
            approved_attachments.push(ApprovedAttachment {
                seq: msg.seq,
                attachment: att,
                cached,
            });
        }
        prepared_messages.push(PreparedMessage {
            sender: msg.sender.clone(),
            text: display_text,
            attachment: att_name,
        });
        encoded_messages.push(AiMessage {
            sender: msg.sender,
            text: encoded_text,
        });
    }

    let mut ai = state.ai.lock().expect("AI state");
    let request = ai.draft(view, draft)?;
    let prepared = (|| -> Result<_> {
        ensure!(
            request.preparing == Some(id),
            "The selected context has changed"
        );
        ensure!(
            !prompt.is_empty() && prompt.len() <= MAX_AI_PROMPT_BYTES,
            "Enter a prompt of up to 8 KiB"
        );
        let assistant = load_assistants(client.profile_dir())?
            .into_iter()
            .find(|entry| entry.info.provider == selection.provider)
            .context("Configure this AI assistant in Settings")?;
        let files = select_files(request, selection.files)?;
        AiSnapshot {
            prompt: prompt.clone(),
            messages: encoded_messages.clone(),
            files: files.clone(),
        }
        .encode()?;
        let endpoint = selection
            .provider
            .base_url(assistant.info.base_url.as_deref())?
            .join("responses")
            .context("Invalid AI endpoint")?;
        let prepared = Prepared {
            id,
            endpoint: endpoint.to_string(),
            assistant: assistant.info.clone(),
            snapshot: PreparedSnapshot {
                prompt: prompt.clone(),
                messages: prepared_messages,
                attachments: prepared_attachments,
                files: files.clone(),
            },
        };
        request.approval = Some(Approval {
            id,
            conversation_id: conversation,
            assistant,
            prompt,
            message_seqs: approved_message_seqs,
            messages: encoded_messages,
            files,
            attachments: approved_attachments,
            autonomous: false,
            #[cfg(windows)]
            effort: None,
        });
        Ok(prepared)
    })()?;
    Ok(prepared)
}

#[derive(Serialize)]
pub(crate) struct SentPrompt {
    id: Uuid,
    created_at: i64,
}

#[tauri::command]
pub(crate) async fn ai_send(
    app: AppHandle,
    state: State<'_, Desktop>,
    view: Uuid,
    draft: Uuid,
    approval: Uuid,
) -> CommandResult<SentPrompt> {
    let client = state.client().await?;
    let approved = state
        .ai
        .lock()
        .expect("AI state")
        .consume(view, draft, approval)?;
    start_ai_job(app, &state, client, approved).await
}

async fn start_ai_job(
    app: AppHandle, state: &Desktop, client: Arc<Client>, approved: Approval,
) -> CommandResult<SentPrompt> {
    #[cfg(windows)]
    let ai_history = state.ai_history.lock().await;
    let conversation_id = approved.conversation_id;
    ensure_available(state, &client, conversation_id, conversation_id).await?;
    let provider = approved.assistant.info.provider;
    let _base = provider.base_url(approved.assistant.info.base_url.as_deref())?;
    let bytes = protect_key(provider, &approved.assistant.protected_key, false)?;
    let key_vec = Zeroizing::new(bytes);
    let key_str = Zeroizing::new(
        std::str::from_utf8(&key_vec)
            .context("Replace the stored API key in Settings")?
            .to_owned(),
    );

    let prompt_id = Uuid::new_v4();
    let created_at = client.server_now();
    let has_uncached = approved.attachments.iter().any(|item| !item.cached);
    let initial_phase = if approved.autonomous {
        "searching"
    } else if has_uncached {
        "downloading"
    } else if !approved.attachments.is_empty() {
        "preparing"
    } else {
        "generating"
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_notify = Arc::new(Notify::new());
    let (cancel_tx, cancel_rx) = watch::channel(false);

    {
        let mut ai = state.ai.lock().expect("AI state");
        ai.jobs.insert(
            prompt_id,
            ActiveAiJob {
                id: prompt_id,
                prompt_id,
                conversation_id,
                provider,
                phase: initial_phase.into(),
                status_text: String::new(),
                progress: None,
                error: None,
                created_at,
                cancelled: Arc::clone(&cancelled),
                cancel_notify: Arc::clone(&cancel_notify),
                cancel_tx,
                #[cfg(windows)]
                autonomous: approved.autonomous,
                #[cfg(windows)]
                access: None,
                #[cfg(windows)]
                targets: Default::default(),
            },
        );
    }
    if let Err(error) = client.insert_ai_prompt(&AiLocalPrompt {
        id: prompt_id,
        conversation_id,
        provider,
        prompt_text: approved.prompt.clone(),
        created_at,
    }).await {
        state.ai.lock().expect("AI state").jobs.remove(&prompt_id);
        return Err(error.into());
    }
    #[cfg(windows)]
    drop(ai_history);
    let _ = app.emit(AI_CHANGED_EVENT, conversation_id);

    tauri::async_runtime::spawn(run_ai_job(
        app,
        client,
        prompt_id,
        approved,
        key_str,
        cancelled,
        cancel_notify,
        cancel_rx,
    ));

    Ok(SentPrompt {
        id: prompt_id,
        created_at,
    })
}

#[tauri::command]
pub(crate) async fn ai_prompts(
    state: State<'_, Desktop>,
    conversation: Uuid,
) -> CommandResult<Vec<AiLocalPrompt>> {
    let client = state.client().await?;
    Ok(client.list_ai_prompts(conversation).await?)
}

#[tauri::command]
pub(crate) async fn ai_jobs(
    state: State<'_, Desktop>,
    conversation: Uuid,
) -> CommandResult<Vec<AiJobView>> {
    let _client = state.client().await?;
    let ai = state.ai.lock().expect("AI state");
    let mut jobs: Vec<AiJobView> = ai
        .jobs
        .values()
        .filter(|job| {
            job.conversation_id == conversation && !job.cancelled.load(Ordering::Acquire)
        })
        .map(ActiveAiJob::view)
        .collect();
    jobs.sort_by_key(|job| (job.created_at, job.id));
    Ok(jobs)
}

#[tauri::command]
pub(crate) async fn ai_replies(
    state: State<'_, Desktop>,
    conversation: Uuid,
) -> CommandResult<Vec<AiLocalReply>> {
    let client = state.client().await?;
    ensure_available(&state, &client, conversation, conversation).await?;
    Ok(client.list_ai_replies(conversation).await?)
}

#[tauri::command]
pub(crate) fn ai_cancel_job(
    app: AppHandle,
    state: State<'_, Desktop>,
    job_id: Uuid,
) -> CommandResult<()> {
    let conversation = {
        let mut ai = state.ai.lock().expect("AI state");
        if ai.jobs.get(&job_id).is_some_and(|job| job.phase == "saving") {
            return Ok(());
        }
        ai.jobs.remove(&job_id).map(|job| {
            job.cancel();
            job.conversation_id
        })
    };
    if let Some(conversation) = conversation {
        let _ = app.emit(AI_CHANGED_EVENT, conversation);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_dismiss_job(
    app: AppHandle,
    state: State<'_, Desktop>,
    job_id: Uuid,
) -> CommandResult<()> {
    ai_cancel_job(app, state, job_id)
}

#[tauri::command]
pub(crate) async fn ai_dismiss_reply(
    app: AppHandle,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    let conversation = state.client().await?.dismiss_ai_reply(id).await?;
    if let Some(conversation) = conversation {
        let _ = app.emit(AI_CHANGED_EVENT, conversation);
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalAiMessage {
    Prompt,
    Reply,
    Job,
}

#[cfg(windows)]
#[tauri::command]
pub(crate) async fn ai_delete_local(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Desktop>,
    conversation: Uuid,
    id: Uuid,
    kind: LocalAiMessage,
) -> CommandResult<()> {
    ensure_normal_chat_window(&window, conversation)?;
    let client = state.client().await?;
    ensure_available(&state, &client, conversation, conversation).await?;
    match kind {
        LocalAiMessage::Prompt => client.delete_ai_prompt(conversation, id).await?,
        LocalAiMessage::Reply => {
            let reply = client.ai_reply(id).await?.filter(|reply| reply.conversation_id == conversation)
                .context("This reply is unavailable.")?;
            client.delete_ai_reply_slot(conversation, reply.prompt_id).await?;
        }
        LocalAiMessage::Job => {
            client.delete_ai_reply_slot(conversation, id).await?;
            let mut ai = state.ai.lock().expect("AI state");
            if ai.jobs.get(&id).is_some_and(|job| job.conversation_id == conversation) {
                if let Some(job) = ai.jobs.remove(&id) { job.cancel(); }
            }
        }
    }
    let _ = app.emit(AI_CHANGED_EVENT, conversation);
    Ok(())
}

async fn run_with_cancel<T>(
    cancel_rx: &mut watch::Receiver<bool>,
    cancelled: &AtomicBool,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    if cancelled.load(Ordering::Acquire) || *cancel_rx.borrow() {
        return Err(AiCancelledError.into());
    }
    tokio::select! {
        biased;
        _ = cancel_rx.changed() => Err(AiCancelledError.into()),
        res = fut => res,
    }
}

fn update_job_status(
    app: &AppHandle,
    job_id: Uuid,
    conversation_id: Uuid,
    phase: &str,
    status_text: String,
    progress: Option<(u64, u64)>,
) {
    let updated = {
        let desktop = app.state::<Desktop>();
        let mut ai = desktop.ai.lock().expect("AI state");
        if let Some(job) = ai.jobs.get_mut(&job_id)
            && !job.cancelled.load(Ordering::Acquire)
        {
            job.phase = phase.into();
            job.status_text = status_text;
            job.progress = progress;
            true
        } else {
            false
        }
    };
    if updated {
        let _ = app.emit(AI_CHANGED_EVENT, conversation_id);
    }
}

async fn execute_ai_job(
    app: &AppHandle,
    client: &Arc<Client>,
    job_id: Uuid,
    approved: &Approval,
    key: &str,
    cancelled: &Arc<AtomicBool>,
    cancel_notify: &Arc<Notify>,
    cancel_rx: &mut watch::Receiver<bool>,
) -> Result<String> {
    let conversation_id = approved.conversation_id;
    let provider = approved.assistant.info.provider;
    let base = provider.base_url(approved.assistant.info.base_url.as_deref())?;

    let granted: Vec<(i64, zibblu_core::Attachment)> = approved
        .attachments
        .iter()
        .map(|item| (item.seq, item.attachment.clone()))
        .collect();
    let model = &approved.assistant.info.model;
    let base_ref = &base;
    let mut extract_cancel_rx = cancel_rx.clone();
    extract_and_send(
        &approved.prompt,
        &approved.messages,
        &approved.files,
        cancelled,
        || async {
            let mut extracted: Vec<ExtractedAttachment> =
                Vec::with_capacity(approved.attachments.len());
            for item in &approved.attachments {
                if cancelled.load(Ordering::Acquire) {
                    return Err(AiCancelledError.into());
                }
                let att_name = item.attachment.name.clone();
                if !item.cached {
                    update_job_status(
                        app,
                        job_id,
                        conversation_id,
                        "downloading",
                        att_name.clone(),
                        Some((0, item.attachment.size)),
                    );
                }
                let last_emit = StdMutex::new(0u64);
                let authorized: AuthorizedAiAttachment = run_with_cancel(
                    &mut extract_cancel_rx,
                    cancelled,
                    client.fetch_authorized_attachment_for_ai(
                        conversation_id,
                        item.seq,
                        &item.attachment,
                        Arc::clone(cancel_notify),
                        |received, total| {
                            if !item.cached {
                                let mut prev = last_emit.lock().expect("progress emit");
                                if received == total || received.saturating_sub(*prev) >= 256 * 1024
                                {
                                    *prev = received;
                                    update_job_status(
                                        app,
                                        job_id,
                                        conversation_id,
                                        "downloading",
                                        att_name.clone(),
                                        Some((received, total)),
                                    );
                                }
                            }
                        },
                    ),
                )
                .await?;

                if cancelled.load(Ordering::Acquire) {
                    return Err(AiCancelledError.into());
                }
                update_job_status(
                    app,
                    job_id,
                    conversation_id,
                    "preparing",
                    att_name,
                    None,
                );
                let attachment = item.attachment.clone();
                let extracted_item = run_with_cancel(&mut extract_cancel_rx, cancelled, async move {
                    let result = tauri::async_runtime::spawn_blocking(move || {
                        // Keep RAII guard alive inside the blocking extraction scope and drop after closing file handles.
                        let _authorized = authorized;
                        let path = _authorized.path().to_path_buf();
                        extract_attachment(&path, &attachment, provider)
                    })
                    .await
                    .context("Attachment extraction was interrupted")?;
                    result.map_err(|err| match err {
                        ExtractError::Other(inner) => inner,
                        other => anyhow::anyhow!("{other}"),
                    })
                })
                .await?;
                extracted.push(extracted_item);
            }
            Ok(extracted)
        },
        || async {
            client.ensure_ai_authorized(conversation_id, &approved.message_seqs, &granted).await?;
            ensure_available(&app.state::<Desktop>(), client, conversation_id, conversation_id).await
        },
        |idx, total, parts| {
            let status_text = if total > 1 {
                format!("{}/{}", idx + 1, total)
            } else {
                String::new()
            };
            update_job_status(app, job_id, conversation_id, "generating", status_text, None);
            let mut rx = cancel_rx.clone();
            async move {
                run_with_cancel(
                    &mut rx,
                    cancelled,
                    provider.answer_parts(base_ref, model, key, parts, Arc::clone(cancel_notify), None),
                )
                .await
            }
        },
    )
    .await
}

async fn extract_and_send<EFut, RFut, SFut>(
    prompt: &str,
    messages: &[AiMessage],
    files: &[AiFile],
    cancelled: &AtomicBool,
    extract_all: impl FnOnce() -> EFut,
    recheck: impl FnMut() -> RFut,
    send: impl FnMut(usize, usize, Vec<zibblu_core::assistant::AiInputPart>) -> SFut,
) -> Result<String>
where
    EFut: std::future::Future<Output = Result<Vec<ExtractedAttachment>>>,
    RFut: std::future::Future<Output = Result<()>>,
    SFut: std::future::Future<Output = Result<String>>,
{
    let extracted = extract_all().await?;
    let requests = build_chunked_requests(prompt, messages, files, &extracted)?;
    let mut answers = send_chunks(requests.requests, cancelled, recheck, send).await?;

    if answers.len() == 1 {
        Ok(answers.pop().expect("single chunk answer"))
    } else {
        let mut combined = format!("已顺序处理全部 {} 个分块：", answers.len());
        for (idx, chunk_answer) in answers.iter().enumerate() {
            combined.push_str(&format!(
                "\n\n[分块 {}/{}]\n{}",
                idx + 1,
                answers.len(),
                chunk_answer
            ));
        }
        Ok(combined)
    }
}

/// Re-checks authorization immediately before every provider request so a recall or
/// revocation that lands between chunks stops the remaining sends.
async fn send_chunks<P, RFut, SFut>(
    requests: Vec<P>,
    cancelled: &AtomicBool,
    mut recheck: impl FnMut() -> RFut,
    mut send: impl FnMut(usize, usize, P) -> SFut,
) -> Result<Vec<String>>
where
    RFut: std::future::Future<Output = Result<()>>,
    SFut: std::future::Future<Output = Result<String>>,
{
    let total = requests.len();
    let mut answers = Vec::with_capacity(total);
    for (idx, parts) in requests.into_iter().enumerate() {
        if cancelled.load(Ordering::Acquire) {
            return Err(AiCancelledError.into());
        }
        recheck().await?;
        if cancelled.load(Ordering::Acquire) {
            return Err(AiCancelledError.into());
        }
        answers.push(send(idx, total, parts).await?);
    }
    Ok(answers)
}

async fn run_ai_job(
    app: AppHandle,
    client: Arc<Client>,
    job_id: Uuid,
    approved: Approval,
    key: Zeroizing<String>,
    cancelled: Arc<AtomicBool>,
    cancel_notify: Arc<Notify>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let conversation_id = approved.conversation_id;
    let provider = approved.assistant.info.provider;
    #[cfg(windows)]
    let mut sources = Vec::new();
    let outcome = async {
        ensure_available(&app.state::<Desktop>(), &client, conversation_id, conversation_id).await?;
        #[cfg(windows)]
        if approved.autonomous {
            return match agent::execute(&app, &client, job_id, &approved, &key, &cancelled, &cancel_notify, &mut cancel_rx).await {
                Ok((text, found)) => { sources = found; Ok(text) }
                Err(error) => Err(error),
            };
        }
        execute_ai_job(&app, &client, job_id, &approved, &key, &cancelled, &cancel_notify, &mut cancel_rx).await
    }.await;

    let desktop = app.state::<Desktop>();
    if !desktop.is_current(&client).await {
        return;
    }

    let outcome = match outcome {
        Ok(text) => {
            if !desktop.ai.lock().expect("AI state").begin_saving(job_id) { return; }
            let reply = AiLocalReply {
                id: Uuid::new_v4(),
                prompt_id: job_id,
                conversation_id,
                provider,
                text,
                created_at: client.server_now(),
                #[cfg(windows)]
                sources,
                #[cfg(not(windows))]
                sources: Vec::new(),
            };
            let saved = client.insert_ai_reply(&reply).await;
            {
                let mut ai = desktop.ai.lock().expect("AI state");
                match saved {
                    Ok(_) => { ai.jobs.remove(&job_id); }
                    Err(error) => if let Some(job) = ai.jobs.get_mut(&job_id) {
                        job.phase = "failed".into();
                        job.progress = None;
                        job.error = Some(error.to_string());
                    },
                }
            }
            let _ = app.emit(AI_CHANGED_EVENT, conversation_id);
            return;
        }
        Err(error) => error,
    };
    let emit = {
        let mut ai = desktop.ai.lock().expect("AI state");
        if cancelled.load(Ordering::Acquire) || !ai.jobs.contains_key(&job_id) {
            false
        } else {
            if outcome.is::<AiCancelledError>() || outcome.to_string() == "Cancelled" {
                ai.jobs.remove(&job_id);
            } else if let Some(job) = ai.jobs.get_mut(&job_id) {
                job.phase = "failed".into();
                job.progress = None;
                job.error = Some(outcome.to_string());
            }
            true
        }
    };

    if emit {
        let _ = app.emit(AI_CHANGED_EVENT, conversation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_approval(conversation_id: Uuid, id: Uuid) -> Approval {
        Approval {
            id,
            conversation_id,
            assistant: StoredAssistant {
                info: Assistant {
                    provider: AiProvider::OpenAi,
                    name: "review".into(),
                    model: "fixture".into(),
                    base_url: None,
                    agent_enabled: false,
                },
                protected_key: vec![],
            },
            prompt: "Summarize".into(),
            message_seqs: vec![],
            messages: vec![],
            files: vec![],
            attachments: vec![],
            autonomous: false,
            #[cfg(windows)]
            effort: None,
        }
    }

    #[test]
    fn ai_approval_is_single_use_and_limited_to_the_active_draft() -> Result<()> {
        let view = Uuid::new_v4();
        let draft = Uuid::new_v4();
        let approved = Uuid::new_v4();
        let conversation = Uuid::new_v4();
        let file = Uuid::new_v4();
        let request = Draft {
            id: draft,
            preparing: None,
            files: vec![(
                file,
                AiFile {
                    name: "selected.txt".into(),
                    text: "selected".into(),
                },
            )],
            approval: Some(fixture_approval(conversation, approved)),
        };
        assert!(select_files(&request, vec![Uuid::new_v4()]).is_err());
        assert!(select_files(&request, vec![file, file]).is_err());
        assert_eq!(select_files(&request, vec![file])?[0].text, "selected");
        let mut ai = AiState::default();
        ai.views.insert(
            view,
            NormalView {
                conversation,
                draft: Some(request),
            },
        );
        assert!(ai.consume(Uuid::new_v4(), draft, approved).is_err());
        assert!(ai.consume(view, draft, Uuid::new_v4()).is_err());
        assert_eq!(ai.consume(view, draft, approved)?.prompt, "Summarize");
        assert!(ai.consume(view, draft, approved).is_err());
        ai.clear();
        assert!(ai.view(view).is_err());
        Ok(())
    }

    #[test]
    fn ai_multi_window_views_stay_isolated() -> Result<()> {
        let mut ai = AiState::default();
        let v1 = Uuid::new_v4();
        let v2 = Uuid::new_v4();
        let c1 = Uuid::new_v4();
        let c2 = Uuid::new_v4();
        ai.views.insert(
            v1,
            NormalView {
                conversation: c1,
                draft: None,
            },
        );
        ai.views.insert(
            v2,
            NormalView {
                conversation: c2,
                draft: None,
            },
        );
        ai.views.remove(&v1);
        assert!(ai.view(v1).is_err());
        assert_eq!(ai.view(v2)?.conversation, c2);
        Ok(())
    }

    #[test]
    fn revoked_attachment_stops_before_any_provider_request() {
        use std::sync::atomic::AtomicUsize;
        let cancelled = AtomicBool::new(false);
        let rechecks = AtomicUsize::new(0);
        let sends = AtomicUsize::new(0);
        let run = |revoke_after: usize| {
            rechecks.store(0, Ordering::SeqCst);
            sends.store(0, Ordering::SeqCst);
            tauri::async_runtime::block_on(send_chunks(
                vec!["a", "b", "c"],
                &cancelled,
                || {
                    let seen = rechecks.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if seen >= revoke_after {
                            anyhow::bail!("attachment revoked")
                        }
                        Ok(())
                    }
                },
                |_, _, part| {
                    sends.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(part.to_owned()) }
                },
            ))
        };
        assert!(run(0).is_err());
        assert_eq!(sends.load(Ordering::SeqCst), 0, "no request may leave once revoked");
        assert!(run(1).is_err());
        assert_eq!(sends.load(Ordering::SeqCst), 1, "later chunks must be blocked");
        assert_eq!(run(9).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn private_origin_or_target_stops_follow_up_provider_requests() {
        use std::cell::Cell;
        let origin = Uuid::new_v4();
        let target = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        for (read_from, enters_private, expected) in [
            (origin, origin, 1),
            (target, origin, 1),
            (target, target, 1),
            (target, unrelated, 3),
        ] {
            let sends = Cell::new(0);
            let cancelled = AtomicBool::new(false);
            let result = tauri::async_runtime::block_on(send_chunks(
                vec!["first", "second", "third"],
                &cancelled,
                || std::future::ready(ensure_ordinary(origin, read_from, |chat| {
                    sends.get() > 0 && chat == enters_private
                })),
                |_, _, chunk| {
                    sends.set(sends.get() + 1);
                    std::future::ready(Ok(chunk.to_owned()))
                },
            ));
            assert_eq!(sends.get(), expected);
            if expected == 1 {
                assert!(result.unwrap_err().is::<AiCancelledError>());
            } else {
                assert!(result.is_ok());
            }
        }
    }

    #[test]
    fn ai_job_cancel_prevents_resurrection_and_preserves_other_chats() {
        let mut ai = AiState::default();
        let c1 = Uuid::new_v4();
        let c2 = Uuid::new_v4();
        #[cfg(windows)]
        let target = Uuid::new_v4();
        let j1 = Uuid::new_v4();
        let j2 = Uuid::new_v4();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx1, _rx1) = watch::channel(false);
        let (tx2, _rx2) = watch::channel(false);

        ai.jobs.insert(
            j1,
            ActiveAiJob {
                id: j1,
                prompt_id: j1,
                conversation_id: c1,
                provider: AiProvider::OpenAi,
                phase: "generating".into(),
                status_text: String::new(),
                progress: None,
                error: None,
                created_at: 10,
                cancelled: Arc::clone(&cancelled),
                cancel_notify: Arc::new(Notify::new()),
                cancel_tx: tx1,
                #[cfg(windows)]
                autonomous: true,
                #[cfg(windows)]
                access: None,
                #[cfg(windows)]
                targets: [target].into_iter().collect(),
            },
        );
        ai.jobs.insert(
            j2,
            ActiveAiJob {
                id: j2,
                prompt_id: j2,
                conversation_id: c2,
                provider: AiProvider::DeepSeek,
                phase: "generating".into(),
                status_text: String::new(),
                progress: None,
                error: None,
                created_at: 20,
                cancelled: Arc::new(AtomicBool::new(false)),
                cancel_notify: Arc::new(Notify::new()),
                cancel_tx: tx2,
                #[cfg(windows)]
                autonomous: false,
                #[cfg(windows)]
                access: None,
                #[cfg(windows)]
                targets: Default::default(),
            },
        );

        #[cfg(windows)]
        assert_eq!(ai.suspend_conversation(target), vec![c1]);
        #[cfg(not(windows))]
        if let Some(job) = ai.jobs.remove(&j1) {
            job.cancel();
        }
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!ai.jobs.contains_key(&j1));
        assert!(ai.jobs.contains_key(&j2));

        // Simulate late arrival after cancellation: must not insert reply or re-add job.
        assert!(cancelled.load(Ordering::Acquire) || !ai.jobs.contains_key(&j1));
        assert!(!ai.begin_saving(j1));

        // Clear conversation c2 removes j2 only.
        ai.clear_conversation(c2);
        assert!(!ai.jobs.contains_key(&j2));
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn build_store_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for &(name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            let size = data.len() as u32;
            let name_bytes = name.as_bytes();
            let name_len = name_bytes.len() as u16;

            out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&name_len.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u32.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name_bytes);
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        let count = entries.len() as u16;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn incomplete_attachment_fails_before_any_provider_request() -> Result<()> {
        use std::sync::atomic::AtomicUsize;

        let dir = tempfile::tempdir()?;
        let make_att = |name: &str, bytes: &[u8]| -> Result<(std::path::PathBuf, Attachment)> {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes)?;
            let att = Attachment {
                id: Uuid::new_v4(),
                name: name.into(),
                kind: AttachmentKind::File,
                size: bytes.len() as u64,
                sha256: "a".repeat(64),
            };
            Ok((path, att))
        };

        let pictured_docx = build_store_zip(&[
            (
                "word/document.xml",
                br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Body text</w:t></w:r></w:p><w:p><w:r><w:drawing/></w:r></w:p></w:body></w:document>"#,
            ),
            ("word/media/image1.png", b"\x89PNG"),
        ]);
        let footnoted_docx = build_store_zip(&[
            (
                "word/document.xml",
                br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Body text</w:t></w:r></w:p></w:body></w:document>"#,
            ),
            (
                "word/footnotes.xml",
                br#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#,
            ),
        ]);
        let form_pdf = b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
            2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
            3 0 obj << /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >> endobj\n\
            4 0 obj << /Length 73 >>\nstream\nBT /F1 12 Tf (Searchable paragraph with plenty of characters) Tj ET /Fm1 Do\nendstream\nendobj\n\
            5 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length 0 >>\nstream\n\nendstream\nendobj\n%%EOF";

        let cases = [
            make_att("pictured.docx", &pictured_docx)?,
            make_att("footnoted.docx", &footnoted_docx)?,
            make_att("form.pdf", form_pdf)?,
        ];

        for (path, att) in &cases {
            let cancelled = AtomicBool::new(false);
            let sends = AtomicUsize::new(0);
            let res = tauri::async_runtime::block_on(extract_and_send(
                "Summarize",
                &[],
                &[],
                &cancelled,
                || async {
                    let extracted = extract_attachment(path, att, AiProvider::DeepSeek)
                        .map_err(|err| anyhow::anyhow!("{err}"))?;
                    Ok(vec![extracted])
                },
                || async { Ok(()) },
                |_, _, _| {
                    sends.fetch_add(1, Ordering::SeqCst);
                    async { Ok("should not run".into()) }
                },
            ));
            assert!(res.is_err(), "{} must fail before sending", att.name);
            assert_eq!(
                sends.load(Ordering::SeqCst),
                0,
                "provider call count must be zero for {}",
                att.name
            );
        }

        // Complete UTF-8 text attachment still reaches provider once.
        let (ok_path, ok_att) = make_att("notes.txt", b"Complete readable text")?;
        let cancelled = AtomicBool::new(false);
        let sends = AtomicUsize::new(0);
        let reply = tauri::async_runtime::block_on(extract_and_send(
            "Summarize",
            &[],
            &[],
            &cancelled,
            || async {
                let extracted = extract_attachment(&ok_path, &ok_att, AiProvider::DeepSeek)
                    .map_err(|err| anyhow::anyhow!("{err}"))?;
                Ok(vec![extracted])
            },
            || async { Ok(()) },
            |_, _, _| {
                sends.fetch_add(1, Ordering::SeqCst);
                async { Ok("ok".into()) }
            },
        ))?;
        assert_eq!(reply, "ok");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
