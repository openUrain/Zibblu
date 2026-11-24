use crate::{
    commands::{Desktop, Result as CommandResult},
    images, media,
};
use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
};
use tauri::{State, WebviewWindow, ipc::InvokeBody};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use zibblu_core::{
    Attachment, AttachmentKind, CHUNK_SIZE, Client, FileIdentity, MAX_PASTED_IMAGE_BYTES,
    SourceSnapshot, Transfer, capture_source_snapshot, open_verified_local_file,
    verify_open_handle,
};

pub(crate) const MAX_DRAFT_ATTACHMENTS: usize = 8;

#[derive(Clone, Serialize)]
pub(crate) struct DraftAttachmentDto {
    pub id: Uuid,
    pub name: String,
    pub kind: AttachmentKind,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_kind: Option<&'static str>,
    pub can_preview: bool,
}

#[derive(Serialize)]
pub(crate) struct AddDraftsResult {
    pub added: Vec<DraftAttachmentDto>,
    pub skipped_duplicates: usize,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PickDraftKind {
    File,
    Folder,
}

#[derive(Clone)]
enum DraftSource {
    Local {
        path: PathBuf,
        snapshot: SourceSnapshot,
    },
    PastedImage {
        bytes: Arc<Vec<u8>>,
    },
}

struct DraftEntry {
    id: Uuid,
    conversation: Uuid,
    upload_id: Uuid,
    name: String,
    kind: AttachmentKind,
    size: u64,
    source: DraftSource,
    thumbnail: Option<Vec<u8>>,
    enqueueing: bool,
}

impl DraftEntry {
    fn dto(&self) -> DraftAttachmentDto {
        let media = preview_media_kind(&self.name, self.kind);
        let can_preview = self.kind == AttachmentKind::File
            && self.size > 0
            && (is_inline_video(&self.name)
                || (is_inline_image(&self.name) && self.size <= MAX_PASTED_IMAGE_BYTES as u64));
        DraftAttachmentDto {
            id: self.id,
            name: self.name.clone(),
            kind: self.kind,
            size: self.size,
            media_kind: media,
            can_preview,
        }
    }

    fn local_identity(&self) -> Option<&FileIdentity> {
        match &self.source {
            DraftSource::Local { snapshot, .. } => Some(&snapshot.identity),
            DraftSource::PastedImage { .. } => None,
        }
    }
}

#[derive(Default)]
struct DraftStore {
    client: Option<Weak<Client>>,
    entries: HashMap<Uuid, DraftEntry>,
    order: HashMap<Uuid, Vec<Uuid>>,
}

impl DraftStore {
    fn bind(&mut self, client: &Arc<Client>) {
        let same = self
            .client
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|active| Arc::ptr_eq(&active, client));
        if !same {
            self.entries.clear();
            self.order.clear();
            self.client = Some(Arc::downgrade(client));
        }
    }

    fn remove_entry(&mut self, id: Uuid) -> Option<DraftEntry> {
        let entry = self.entries.remove(&id)?;
        if let Some(list) = self.order.get_mut(&entry.conversation) {
            list.retain(|item| *item != id);
            if list.is_empty() {
                self.order.remove(&entry.conversation);
            }
        }
        Some(entry)
    }
}

#[derive(Default)]
pub(crate) struct AttachmentDrafts {
    inner: StdMutex<DraftStore>,
}

impl AttachmentDrafts {
    pub(crate) fn clear(&self) {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.client = None;
        store.entries.clear();
        store.order.clear();
    }

    fn add_snapshots(
        &self,
        client: &Arc<Client>,
        conversation: Uuid,
        items: Vec<(PathBuf, Result<SourceSnapshot>)>,
    ) -> AddDraftsResult {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.bind(client);
        let mut added = Vec::new();
        let mut skipped_duplicates = 0usize;
        let mut errors = Vec::new();

        for (path, snapshot_result) in items {
            let fallback_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let snapshot = match snapshot_result {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    errors.push(format!("{fallback_name}: {error}"));
                    continue;
                }
            };
            let existing_ids = store.order.get(&conversation).cloned().unwrap_or_default();
            let duplicate = existing_ids.iter().any(|id| {
                store
                    .entries
                    .get(id)
                    .and_then(DraftEntry::local_identity)
                    .is_some_and(|identity| identity == &snapshot.identity)
            });
            if duplicate {
                skipped_duplicates += 1;
                continue;
            }
            if existing_ids.len() >= MAX_DRAFT_ATTACHMENTS {
                errors.push(format!(
                    "Up to {MAX_DRAFT_ATTACHMENTS} attachments can be queued per message."
                ));
                break;
            }
            let id = Uuid::new_v4();
            let entry = DraftEntry {
                id,
                conversation,
                upload_id: Uuid::new_v4(),
                name: snapshot.name.clone(),
                kind: snapshot.kind,
                size: snapshot.size,
                source: DraftSource::Local { path, snapshot },
                thumbnail: None,
                enqueueing: false,
            };
            let dto = entry.dto();
            store.entries.insert(id, entry);
            store.order.entry(conversation).or_default().push(id);
            added.push(dto);
        }

        AddDraftsResult {
            added,
            skipped_duplicates,
            errors,
        }
    }

    fn add_pasted_image(
        &self,
        client: &Arc<Client>,
        conversation: Uuid,
        bytes: Vec<u8>,
        mime: &'static str,
    ) -> Result<DraftAttachmentDto> {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.bind(client);
        let count = store.order.get(&conversation).map_or(0, Vec::len);
        ensure!(
            count < MAX_DRAFT_ATTACHMENTS,
            "Up to {MAX_DRAFT_ATTACHMENTS} attachments can be queued per message."
        );
        let id = Uuid::new_v4();
        let ext = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "png",
        };
        let size = bytes.len() as u64;
        let entry = DraftEntry {
            id,
            conversation,
            upload_id: Uuid::new_v4(),
            name: format!("Pasted image {}.{ext}", &id.simple().to_string()[..8]),
            kind: AttachmentKind::File,
            size,
            source: DraftSource::PastedImage {
                bytes: Arc::new(bytes),
            },
            thumbnail: None,
            enqueueing: false,
        };
        let dto = entry.dto();
        store.entries.insert(id, entry);
        store.order.entry(conversation).or_default().push(id);
        Ok(dto)
    }

    pub(crate) fn remove(&self, client: &Arc<Client>, id: Uuid) {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.bind(client);
        store.remove_entry(id);
    }

    pub(crate) fn has_draft(&self, client: &Arc<Client>, id: Uuid) -> bool {
        let store = self.inner.lock().expect("attachment drafts");
        store
            .client
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|active| Arc::ptr_eq(&active, client))
            && store.entries.contains_key(&id)
    }

    pub(crate) fn draft_image_entry(
        &self,
        client: &Arc<Client>,
        id: Uuid,
    ) -> Result<(Uuid, Attachment)> {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.bind(client);
        let entry = store.entries.get(&id).context("Draft attachment not found")?;
        let dto = entry.dto();
        ensure!(
            dto.media_kind == Some("image") && dto.can_preview,
            "This draft attachment is not a previewable image"
        );
        Ok((
            entry.conversation,
            Attachment {
                id: entry.id,
                name: entry.name.clone(),
                kind: entry.kind,
                size: entry.size,
                sha256: String::new(),
            },
        ))
    }

    pub(crate) fn draft_image_neighbor(
        &self,
        client: &Arc<Client>,
        conversation: Uuid,
        id: Uuid,
        forward: bool,
    ) -> Option<Uuid> {
        let mut store = self.inner.lock().expect("attachment drafts");
        store.bind(client);
        let images: Vec<Uuid> = store
            .order
            .get(&conversation)?
            .iter()
            .copied()
            .filter(|entry_id| {
                store.entries.get(entry_id).is_some_and(|entry| {
                    let dto = entry.dto();
                    dto.media_kind == Some("image") && dto.can_preview
                })
            })
            .collect();
        let pos = images.iter().position(|entry_id| *entry_id == id)?;
        if forward {
            images.get(pos + 1).copied()
        } else {
            pos.checked_sub(1).and_then(|idx| images.get(idx).copied())
        }
    }

    pub(crate) async fn preview_image(
        &self,
        client: &Arc<Client>,
        id: Uuid,
        thumbnail: bool,
    ) -> Result<(String, Vec<u8>)> {
        let (source, cached_thumb, size) = {
            let mut store = self.inner.lock().expect("attachment drafts");
            store.bind(client);
            let entry = store.entries.get(&id).context("Draft attachment not found")?;
            ensure!(entry.kind == AttachmentKind::File, "Folders cannot be previewed as images");
            (entry.source.clone(), entry.thumbnail.clone(), entry.size)
        };
        if thumbnail && let Some(thumb) = cached_thumb {
            if let DraftSource::Local { path, snapshot } = &source {
                let path = path.clone();
                let snapshot = snapshot.clone();
                let valid = tauri::async_runtime::spawn_blocking(move || {
                    open_verified_local_file(&path, &snapshot).map(|_| ())
                })
                .await?;
                if let Err(error) = valid {
                    let mut store = self.inner.lock().expect("attachment drafts");
                    if let Some(entry) = store.entries.get_mut(&id) {
                        entry.thumbnail = None;
                    }
                    return Err(error);
                }
            }
            ensure!(self.has_draft(client, id), "Draft attachment not found");
            return Ok(("image/png".into(), thumb));
        }
        let raw = match source {
            DraftSource::PastedImage { bytes } => bytes.as_ref().clone(),
            DraftSource::Local { path, snapshot } => {
                ensure!(
                    size <= MAX_PASTED_IMAGE_BYTES as u64,
                    "Image too large for inline preview"
                );
                tauri::async_runtime::spawn_blocking(move || read_verified_local_image(&path, &snapshot))
                    .await??
            }
        };
        let permit = media::IMAGE_JOBS.acquire().await?;
        let raw_for_work = raw.clone();
        let (mime, thumb) = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            images::preview_image(&raw_for_work)
        })
        .await??;
        {
            let mut store = self.inner.lock().expect("attachment drafts");
            ensure!(
                store
                    .client
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .is_some_and(|active| Arc::ptr_eq(&active, client))
                    && store.entries.contains_key(&id),
                "Draft attachment not found"
            );
            if let Some(entry) = store.entries.get_mut(&id) {
                entry.thumbnail = Some(thumb.clone());
            }
        }
        if thumbnail {
            Ok(("image/png".into(), thumb))
        } else {
            Ok((mime.into(), raw))
        }
    }

    pub(crate) async fn preview_video_info(
        &self,
        client: &Arc<Client>,
        id: Uuid,
    ) -> Result<(PathBuf, SourceSnapshot, &'static str, u64)> {
        let (path, snapshot, name) = {
            let mut store = self.inner.lock().expect("attachment drafts");
            store.bind(client);
            let entry = store.entries.get(&id).context("Draft attachment not found")?;
            let DraftSource::Local { path, snapshot } = &entry.source else {
                anyhow::bail!("Unsupported video draft");
            };
            (path.clone(), snapshot.clone(), entry.name.clone())
        };
        let mime = video_mime(&name).context("Unsupported video format")?;
        let check_path = path.clone();
        let check_snap = snapshot.clone();
        tauri::async_runtime::spawn_blocking(move || open_verified_local_file(&check_path, &check_snap).map(|_| ()))
            .await??;
        ensure!(self.has_draft(client, id), "Draft attachment not found");
        let size = snapshot.size;
        Ok((path, snapshot, mime, size))
    }

    pub(crate) async fn read_video_range(
        &self,
        client: &Arc<Client>,
        id: Uuid,
        start: u64,
        end: u64,
    ) -> Result<(&'static str, u64, u64, u64, Vec<u8>)> {
        let (path, snapshot, mime, size) = self.preview_video_info(client, id).await?;
        ensure!(size > 0 && start < size, "Range out of bounds");
        let capped_end = end.min(size - 1).min(start.saturating_add(CHUNK_SIZE - 1));
        let len = (capped_end - start + 1) as usize;
        let bytes = tauri::async_runtime::spawn_blocking(move || {
            read_verified_local_range(&path, &snapshot, start, len)
        })
        .await??;
        ensure!(self.has_draft(client, id), "Draft attachment not found");
        Ok((mime, size, start, capped_end, bytes))
    }
}

fn read_verified_local_image(path: &std::path::Path, snapshot: &SourceSnapshot) -> Result<Vec<u8>> {
    ensure!(
        snapshot.size <= MAX_PASTED_IMAGE_BYTES as u64,
        "Image too large for inline preview"
    );
    let mut file = open_verified_local_file(path, snapshot)?;
    let mut bytes = Vec::with_capacity(snapshot.size as usize);
    (&mut file)
        .take(MAX_PASTED_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == snapshot.size && bytes.len() <= MAX_PASTED_IMAGE_BYTES,
        zibblu_core::SourceChangedError
    );
    verify_open_handle(path, &file, snapshot)?;
    Ok(bytes)
}

fn read_verified_local_range(
    path: &std::path::Path,
    snapshot: &SourceSnapshot,
    start: u64,
    len: usize,
) -> Result<Vec<u8>> {
    let mut file = open_verified_local_file(path, snapshot)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    verify_open_handle(path, &file, snapshot)?;
    Ok(buf)
}

fn is_inline_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "apng" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
}

fn is_inline_video(name: &str) -> bool {
    video_mime(name).is_some()
}

pub(crate) fn video_mime(name: &str) -> Option<&'static str> {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

fn preview_media_kind(name: &str, kind: AttachmentKind) -> Option<&'static str> {
    if kind != AttachmentKind::File {
        return None;
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "png" | "apng" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif" | "heic" | "heif" | "tif" | "tiff" | "svg" | "ico"
    ) {
        Some("image")
    } else if matches!(
        ext.as_str(),
        "mp4" | "m4v" | "webm" | "mov" | "avi" | "mkv" | "wmv" | "flv" | "mpg" | "mpeg" | "3gp" | "ogv"
    ) {
        Some("video")
    } else {
        None
    }
}

async fn add_local_paths(
    state: &Desktop,
    client: &Arc<Client>,
    conversation: Uuid,
    paths: Vec<PathBuf>,
) -> Result<AddDraftsResult> {
    let snapshots = tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                let snap = capture_source_snapshot(&path);
                (path, snap)
            })
            .collect::<Vec<_>>()
    })
    .await
    .context("Reading attachment metadata was interrupted")?;
    ensure!(state.is_current(client).await, "The session has ended");
    Ok(state.drafts.add_snapshots(client, conversation, snapshots))
}

#[tauri::command]
pub(crate) async fn add_clipboard_files(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    conversation: Uuid,
) -> CommandResult<Option<AddDraftsResult>> {
    #[cfg(windows)]
    {
        let client = state.client().await?;
        let paths = crate::clipboard::read_files(&window).await?;
        if paths.is_empty() {
            return Ok(None);
        }
        Ok(Some(add_local_paths(&state, &client, conversation, paths).await?))
    }
    #[cfg(not(windows))]
    {
        let _ = (window, state, conversation);
        Ok(None)
    }
}

#[tauri::command]
pub(crate) async fn add_dropped_files(
    state: State<'_, Desktop>,
    conversation: Uuid,
    ticket: Uuid,
) -> CommandResult<AddDraftsResult> {
    let client = state.client().await?;
    let paths = state.take_attachment_offer(ticket)?;
    Ok(add_local_paths(&state, &client, conversation, paths).await?)
}

#[tauri::command]
pub(crate) async fn pick_draft_attachments(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    conversation: Uuid,
    kind: PickDraftKind,
) -> CommandResult<Option<AddDraftsResult>> {
    let client = state.client().await?;
    let paths: Vec<PathBuf> = tauri::async_runtime::spawn_blocking(move || {
        let dialog = window.dialog().file();
        #[cfg(desktop)]
        let dialog = dialog.set_parent(&window);
        match kind {
            PickDraftKind::File => {
                let picked = dialog.set_title("Attach files").blocking_pick_files();
                match picked {
                    Some(list) => list
                        .into_iter()
                        .map(|path| path.into_path().map_err(anyhow::Error::from))
                        .collect::<Result<Vec<_>>>(),
                    None => Ok(Vec::new()),
                }
            }
            #[cfg(desktop)]
            PickDraftKind::Folder => {
                let picked = dialog.set_title("Attach a folder").blocking_pick_folder();
                match picked {
                    Some(path) => Ok(vec![path.into_path()?]),
                    None => Ok(Vec::new()),
                }
            }
            #[cfg(mobile)]
            PickDraftKind::Folder => anyhow::bail!("Choose a file or ZIP archive on iOS."),
        }
    })
    .await
    .context("File picker was interrupted")??;
    if paths.is_empty() {
        return Ok(None);
    }
    Ok(Some(add_local_paths(&state, &client, conversation, paths).await?))
}

#[tauri::command]
pub(crate) async fn add_pasted_image_draft(
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<DraftAttachmentDto> {
    let conversation = request
        .headers()
        .get("zibblu-conversation")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("Open a normal chat before pasting an image")?;
    let client = state.client().await?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(anyhow!("Choose a {} image.", images::FORMATS).into());
    };
    if bytes.len() > MAX_PASTED_IMAGE_BYTES {
        return Err(anyhow!("Images can be up to 16 MiB.").into());
    }
    let owned = bytes.to_vec();
    let permit = media::IMAGE_JOBS.acquire().await.map_err(anyhow::Error::from)?;
    let (bytes, mime) = tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<u8>, &'static str)> {
        let _permit = permit;
        let mime = images::verified_mime(&owned)?;
        Ok((owned, mime))
    })
    .await
    .context("Checking image was interrupted")??;
    if !state.is_current(&client).await {
        return Err(anyhow!("The session has ended").into());
    }
    Ok(state.drafts.add_pasted_image(&client, conversation, bytes, mime)?)
}

#[tauri::command]
pub(crate) async fn remove_draft_attachment(
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    let client = state.client().await?;
    state.drafts.remove(&client, id);
    Ok(())
}

#[tauri::command]
pub(crate) async fn enqueue_draft_attachment(
    state: State<'_, Desktop>,
    conversation: Uuid,
    id: Uuid,
) -> CommandResult<Transfer> {
    let client = state.client().await?;
    let (upload_id, source) = {
        let mut store = state.drafts.inner.lock().expect("attachment drafts");
        store.bind(&client);
        let entry = store
            .entries
            .get_mut(&id)
            .context("This attachment draft is no longer available")?;
        if entry.conversation != conversation {
            return Err(anyhow!("This attachment belongs to another conversation").into());
        }
        if entry.enqueueing {
            return Err(anyhow!("This attachment is already being queued").into());
        }
        entry.enqueueing = true;
        (entry.upload_id, entry.source.clone())
    };

    let result = match source {
        DraftSource::Local { path, snapshot } => {
            client
                .queue_upload_verified(conversation, upload_id, path, snapshot)
                .await
        }
        DraftSource::PastedImage { bytes } => {
            client
                .queue_pasted_image_with_id(conversation, upload_id, bytes.as_ref().clone())
                .await
        }
    };

    let mut store = state.drafts.inner.lock().expect("attachment drafts");
    let same_session = store
        .client
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|active| Arc::ptr_eq(&active, &client));
    match result {
        Ok(transfer) => {
            if same_session {
                store.remove_entry(id);
            }
            Ok(transfer)
        }
        Err(error) => {
            if same_session && let Some(entry) = store.entries.get_mut(&id) {
                entry.enqueueing = false;
            }
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zibblu_core::FileIdentity;

    #[test]
    fn media_kind_and_video_mime_detection() {
        assert_eq!(preview_media_kind("photo.JPG", AttachmentKind::File), Some("image"));
        assert_eq!(preview_media_kind("clip.mp4", AttachmentKind::File), Some("video"));
        assert_eq!(preview_media_kind("archive.zip", AttachmentKind::File), None);
        assert_eq!(preview_media_kind("photo.jpg", AttachmentKind::Folder), None);
        assert_eq!(video_mime("clip.MOV"), Some("video/quicktime"));
        assert_eq!(video_mime("doc.pdf"), None);
    }

    #[test]
    fn local_identity_deduplicates_only_within_same_conversation() {
        let id_a = FileIdentity::Path(PathBuf::from("C:/a/report.pdf"));
        let id_b = FileIdentity::Path(PathBuf::from("C:/b/report.pdf"));
        assert_ne!(id_a, id_b);
    }

    #[test]
    fn local_preview_and_video_range_fail_closed_on_replacement() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let img = dir.path().join("photo.png");
        let vid = dir.path().join("clip.mp4");
        std::fs::write(&img, b"fake-png-bytes-123456")?;
        std::fs::write(&vid, b"0123456789abcdef")?;

        let img_snap = capture_source_snapshot(&img)?;
        let vid_snap = capture_source_snapshot(&vid)?;
        assert_eq!(read_verified_local_image(&img, &img_snap)?, b"fake-png-bytes-123456");
        assert_eq!(read_verified_local_range(&vid, &vid_snap, 4, 6)?, b"456789");

        // Replace image with a different file of the same size -> fails closed.
        let other = dir.path().join("other.png");
        std::fs::write(&other, b"OTHER-PNG-BYTES-99999")?;
        std::fs::remove_file(&img)?;
        std::fs::rename(&other, &img)?;
        assert!(read_verified_local_image(&img, &img_snap).is_err());

        // Modify video between range reads -> subsequent range fails closed.
        std::fs::write(&vid, b"ABCDEFGHIJKLMNOP")?;
        assert!(read_verified_local_range(&vid, &vid_snap, 0, 4).is_err());
        Ok(())
    }
}
