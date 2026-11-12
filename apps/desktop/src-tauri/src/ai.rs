use crate::commands::{Desktop, Result as CommandResult};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use zeroize::Zeroizing;
use zibblu_core::{
    AiScope, MAX_MESSAGE_BYTES,
    assistant::{AiFile, AiProvider, AiSnapshot},
};

#[cfg(windows)]
#[path = "ai_windows.rs"]
mod windows;

#[cfg(windows)]
use windows::{protect_key, read_text_file};

#[cfg(not(windows))]
fn protect_key(_: AiProvider, _: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("AI assistants require Windows credential protection")
}

#[cfg(not(windows))]
fn read_text_file(_: &Path) -> Result<AiFile> {
    anyhow::bail!("AI file selection is available on Windows")
}

const MAX_FILES: usize = 3;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Assistant {
    provider: AiProvider,
    name: String,
    model: String,
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
    state: State<'_, Desktop>,
    provider: AiProvider,
    name: String,
    model: String,
    key: Option<String>,
) -> CommandResult<Vec<Assistant>> {
    let key = key.map(Zeroizing::new);
    let client = state.client().await?;
    let name = name.trim().to_lowercase();
    let model = model.trim().to_owned();
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
        ensure!(
            (1..=128).contains(&model.len())
                && model
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_' | b':')),
            "Enter the provider's model ID"
        );
        ensure!(
            assistants
                .iter()
                .all(|entry| entry.info.provider == provider || entry.info.name != name),
            "That mention name is already in use"
        );
        let protected_key = if let Some(key) = key.as_ref() {
            let key = key.trim();
            ensure!(
                (1..=512).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic()),
                "Enter a valid API key"
            );
            protect_key(provider, key.as_bytes(), true)?
        } else {
            assistants
                .iter()
                .find(|entry| entry.info.provider == provider)
                .context("Enter an API key")?
                .protected_key
                .clone()
        };
        assistants.retain(|entry| entry.info.provider != provider);
        assistants.push(StoredAssistant {
            info: Assistant {
                provider,
                name,
                model,
            },
            protected_key,
        });
        store_assistants(client.profile_dir(), &assistants)
    })();
    Ok(result?)
}

#[tauri::command]
pub(crate) async fn ai_remove_assistant(
    state: State<'_, Desktop>,
    provider: AiProvider,
) -> CommandResult<Vec<Assistant>> {
    let client = state.client().await?;
    let _settings = state.ai.lock().expect("AI state");
    let mut assistants = load_assistants(client.profile_dir())?;
    assistants.retain(|entry| entry.info.provider != provider);
    Ok(store_assistants(client.profile_dir(), &assistants)?)
}

#[derive(Default)]
pub(crate) struct AiState {
    view: Option<NormalView>,
}

struct NormalView {
    id: Uuid,
    conversation: Uuid,
    draft: Option<Draft>,
}

struct Draft {
    id: Uuid,
    files: Vec<(Uuid, AiFile)>,
    approval: Option<Approval>,
}

struct Approval {
    id: Uuid,
    assistant: StoredAssistant,
    input: String,
}

impl AiState {
    pub(crate) fn clear(&mut self) {
        self.view = None;
    }

    fn view(&mut self, id: Uuid) -> Result<&mut NormalView> {
        self.view
            .as_mut()
            .filter(|view| view.id == id)
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

#[tauri::command]
pub(crate) fn ai_open_view(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    view: Uuid,
    conversation: Uuid,
) -> CommandResult<()> {
    if window.label() != "main" {
        return Err(anyhow::anyhow!("AI is only available in normal chats").into());
    }
    state.ai.lock().expect("AI state").view = Some(NormalView {
        id: view,
        conversation,
        draft: None,
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn ai_close_view(state: State<'_, Desktop>, view: Uuid) {
    let mut ai = state.ai.lock().expect("AI state");
    if ai.view(view).is_ok() {
        ai.clear();
    }
}

#[tauri::command]
pub(crate) fn ai_begin(state: State<'_, Desktop>, view: Uuid, draft: Uuid) -> CommandResult<()> {
    state.ai.lock().expect("AI state").view(view)?.draft = Some(Draft {
        id: draft,
        files: Vec::new(),
        approval: None,
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
    window: tauri::WebviewWindow,
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
        let Some(path) = window
            .dialog()
            .file()
            .set_parent(&window)
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
pub(crate) struct Prepared {
    id: Uuid,
    endpoint: &'static str,
    assistant: Assistant,
    snapshot: AiSnapshot,
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
    let conversation = {
        let mut ai = state.ai.lock().expect("AI state");
        ai.draft(view, draft)?.approval = None;
        ai.view(view)?.conversation
    };
    let messages = client.ai_messages(conversation, selection.scope).await?;
    let mut ai = state.ai.lock().expect("AI state");
    let request = ai.draft(view, draft)?;
    let prepared = (|| -> Result<_> {
        ensure!(
            !prompt.is_empty() && prompt.len() <= MAX_MESSAGE_BYTES,
            "Enter a prompt of up to 8 KiB"
        );
        let assistant = load_assistants(client.profile_dir())?
            .into_iter()
            .find(|entry| entry.info.provider == selection.provider)
            .context("Configure this AI assistant in Settings")?;
        let files = select_files(request, selection.files)?;
        let snapshot = AiSnapshot {
            prompt,
            messages,
            files,
        };
        let input = snapshot.encode()?;
        let id = Uuid::new_v4();
        let prepared = Prepared {
            id,
            endpoint: selection.provider.endpoint(),
            assistant: assistant.info.clone(),
            snapshot,
        };
        request.approval = Some(Approval {
            id,
            assistant,
            input,
        });
        Ok(prepared)
    })()?;
    Ok(prepared)
}

#[tauri::command]
pub(crate) async fn ai_send(
    state: State<'_, Desktop>,
    view: Uuid,
    draft: Uuid,
    approval: Uuid,
) -> CommandResult<String> {
    state.client().await?;
    let Approval {
        assistant, input, ..
    } = state
        .ai
        .lock()
        .expect("AI state")
        .consume(view, draft, approval)?;
    let provider = assistant.info.provider;
    let bytes = protect_key(provider, &assistant.protected_key, false)?;
    let key = Zeroizing::new(bytes);
    let key = std::str::from_utf8(&key).context("Replace the stored API key in Settings")?;
    Ok(provider.answer(&assistant.info.model, key, &input).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_approval_is_single_use_and_limited_to_the_active_draft() -> Result<()> {
        let view = Uuid::new_v4();
        let draft = Uuid::new_v4();
        let approved = Uuid::new_v4();
        let file = Uuid::new_v4();
        let request = Draft {
            id: draft,
            files: vec![(
                file,
                AiFile {
                    name: "selected.txt".into(),
                    text: "selected".into(),
                },
            )],
            approval: Some(Approval {
                id: approved,
                assistant: StoredAssistant {
                    info: Assistant {
                        provider: AiProvider::OpenAi,
                        name: "review".into(),
                        model: "fixture".into(),
                    },
                    protected_key: vec![],
                },
                input: "reviewed snapshot".into(),
            }),
        };
        assert!(select_files(&request, vec![Uuid::new_v4()]).is_err());
        assert!(select_files(&request, vec![file, file]).is_err());
        assert_eq!(select_files(&request, vec![file])?[0].text, "selected");
        let mut ai = AiState {
            view: Some(NormalView {
                id: view,
                conversation: Uuid::new_v4(),
                draft: Some(request),
            }),
        };
        assert!(ai.consume(Uuid::new_v4(), draft, approved).is_err());
        assert!(ai.consume(view, draft, Uuid::new_v4()).is_err());
        assert_eq!(
            ai.consume(view, draft, approved)?.input,
            "reviewed snapshot"
        );
        assert!(ai.consume(view, draft, approved).is_err());
        ai.clear();
        assert!(ai.view(view).is_err());
        Ok(())
    }
}
