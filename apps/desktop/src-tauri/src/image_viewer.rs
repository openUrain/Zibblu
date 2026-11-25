use crate::commands::{Desktop, Result as CommandResult};
use crate::media;
use anyhow::{Context, Result, ensure};
#[cfg(windows)]
use image::{ImageDecoder, metadata::Orientation};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::io::Cursor;
use std::sync::{Arc, Weak};
#[cfg(windows)]
use tauri::{LogicalSize, Manager, PhysicalPosition, PhysicalSize};
use tauri::{AppHandle, Emitter, State, WebviewWindow, ipc::InvokeBody};
use uuid::Uuid;
use zibblu_core::{
    Attachment, Client, ConversationRequest, Direction, Directory, FileFilter, ForwardRequest,
    ForwardTarget, MAX_PASTED_IMAGE_BYTES, MediaVariant, Message, OriginalDownloadStatus, Transfer,
    ViewerForwardItem,
};

pub(crate) const LABEL: &str = "image-viewer";

pub(crate) fn close_deleted(app: &tauri::AppHandle, state: &Desktop, conversation: Uuid, seq: Option<i64>, include_drafts: bool) {
    #[cfg(windows)]
    {
        let mut scope = state.image_scope.write().expect("image scope");
        let close_id = scope
            .as_ref()
            .filter(|selection| {
                selection.conversation == conversation
                    && (!selection.draft || include_drafts)
                    && seq.is_none_or(|seq| selection.seq == seq)
            })
            .map(|selection| (selection.id, selection.draft));
        if let Some((att_id, is_draft)) = close_id {
            scope.take();
            drop(scope);
            if !is_draft {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let desktop = app_handle.state::<Desktop>();
                    desktop.video_exports.cancel_for_attachment(att_id).await;
                    if let Ok(client) = desktop.client().await {
                        let _ = client.cancel_original_video_download(att_id).await;
                    }
                });
            }
            if let Some(viewer) = app.get_webview_window(LABEL) {
                let _ = viewer.close();
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (app, state, conversation, seq, include_drafts);
}

pub(crate) fn close_recalled(app: &tauri::AppHandle, state: &Desktop, recalled: &[i64], deleted: &[i64]) {
    #[cfg(windows)]
    {
        if recalled.is_empty() && deleted.is_empty() {
            return;
        }
        let mut scope = state.image_scope.write().expect("image scope");
        let close_id = scope
            .as_ref()
            .filter(|selection| {
                !selection.draft && (recalled.contains(&selection.seq) || deleted.contains(&selection.seq))
            })
            .map(|selection| selection.id);
        if let Some(att_id) = close_id {
            scope.take();
            drop(scope);
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let desktop = app_handle.state::<Desktop>();
                desktop.video_exports.cancel_for_attachment(att_id).await;
                if let Ok(client) = desktop.client().await {
                    let _ = client.cancel_original_video_download(att_id).await;
                }
            });
            if let Some(viewer) = app.get_webview_window(LABEL) {
                let _ = viewer.close();
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (app, state, recalled, deleted);
}
#[cfg(any(windows, test))]
// Full-size controls, group and column gaps, split button, and clearance beside each caption area.
const MIN_VIEWER_WIDTH: f64 = 560.0;

#[derive(Clone)]
pub(crate) struct ImageScope {
    client: Weak<Client>,
    conversation: Uuid,
    seq: i64,
    id: Uuid,
    draft: bool,
}

impl ImageScope {
    async fn client(&self, state: &Desktop) -> Result<Arc<Client>> {
        let client = self.client.upgrade().context("The session has ended")?;
        ensure!(state.is_current(&client).await, "The session has ended");
        Ok(client)
    }

    fn allows_media_file(file: &Attachment) -> bool {
        let Some(mime) = file.preview_mime() else {
            return false;
        };
        if mime.starts_with("image/") {
            return file.size <= zibblu_core::MAX_PASTED_IMAGE_BYTES as u64;
        }
        mime.starts_with("video/")
    }

    fn allows(&self, message: &Message, cutoff: i64, now: i64) -> bool {
        message.conversation_id == self.conversation
            && message.seq > cutoff
            && !message.recalled
            && (message.local_file
                || message.expires_at.is_none_or(|end| end > now) && message.expired.is_none())
            && message.attachment.as_ref().is_some_and(Self::allows_media_file)
    }

    async fn allows_async(&self, client: &Client, message: &Message, cutoff: i64, now: i64) -> bool {
        if self.allows(message, cutoff, now) {
            return true;
        }
        if message.conversation_id != self.conversation || message.seq <= cutoff || message.recalled {
            return false;
        }
        let Some(ref file) = message.attachment else {
            return false;
        };
        if !Self::allows_media_file(file) {
            return false;
        }
        let variant = if file.preview_mime().is_some_and(|m| m.starts_with("video/")) {
            MediaVariant::VideoPreview
        } else {
            MediaVariant::ImageOriginal
        };
        client
            .has_cached_media_variant(message.conversation_id, message.seq, Some(file.id), variant, &file.sha256)
            .await
            .unwrap_or(false)
    }

    async fn cutoff(&self, client: &Client) -> Result<i64> {
        Ok(client
            .directory()
            .await?
            .conversations
            .into_iter()
            .find(|chat| chat.id == self.conversation)
            .context("This conversation is no longer available")?
            .joined_after
            .unwrap_or(0))
    }

    async fn image(&self, client: &Client) -> Result<Message> {
        let message = client.attachment(self.id).await?;
        let cutoff = self.cutoff(client).await?;
        ensure!(
            message.seq == self.seq
                && self.allows_async(client, &message, cutoff, client.server_now()).await,
            "This media has expired or is no longer available"
        );
        Ok(message)
    }

    async fn neighbor(
        &self,
        client: &Client,
        forward: bool,
        cutoff: i64,
    ) -> Result<Option<Message>> {
        let mut cursor = self.seq;
        loop {
            let page = client
                .files(
                    FileFilter {
                        conversation_id: Some(self.conversation),
                        oldest_first: forward,
                        ..Default::default()
                    },
                    Some(cursor),
                )
                .await?;
            let now = client.server_now();
            for message in &page.items {
                if self.allows_async(client, message, cutoff, now).await {
                    return Ok(Some(message.clone()));
                }
            }
            match page.items.last() {
                Some(last) if page.has_more && last.seq != cursor => cursor = last.seq,
                _ => return Ok(None),
            }
        }
    }
}

fn require(condition: bool, message: &str) -> Result<()> {
    ensure!(condition, "{message}");
    Ok(())
}

fn scope(state: &Desktop) -> Result<ImageScope> {
    state
        .image_scope
        .read()
        .expect("image scope")
        .clone()
        .context("Open an image from chat")
}

pub(crate) async fn authorize(state: &Desktop, id: Uuid) -> Result<Arc<Client>> {
    let selection = scope(state)?;
    ensure!(!selection.draft && selection.id == id, "This image is outside the viewer");
    let client = selection.client(state).await?;
    selection.image(&client).await?;
    let selected = scope(state)?;
    ensure!(
        !selected.draft && selected.id == id && selected.client.ptr_eq(&selection.client),
        "The selected image changed"
    );
    Ok(client)
}

pub(crate) async fn authorize_draft(state: &Desktop, id: Uuid) -> Result<Arc<Client>> {
    let selection = scope(state)?;
    ensure!(selection.draft && selection.id == id, "This image is outside the viewer");
    let client = selection.client(state).await?;
    let (conversation, _) = state.drafts.draft_image_entry(&client, id)?;
    ensure!(conversation == selection.conversation, "This image is outside the viewer");
    let selected = scope(state)?;
    ensure!(
        selected.draft && selected.id == id && selected.client.ptr_eq(&selection.client),
        "The selected image changed"
    );
    Ok(client)
}

#[tauri::command]
pub(crate) async fn open_draft_image_viewer(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    #[cfg(windows)]
    {
        require(window.label() == "main", "Open an image from chat")?;
        let _opening = state.image_window.lock().await;
        let client = state.client().await?;
        let (conversation, _) = state.drafts.draft_image_entry(&client, id)?;
        let (_, bytes) = state.drafts.preview_image(&client, id, false).await?;
        let dimensions = bytes_dimensions(bytes).await;
        require(state.is_current(&client).await, "The session has ended")?;
        let (latest_conv, _) = state.drafts.draft_image_entry(&client, id)?;
        require(latest_conv == conversation, "This draft image is no longer available")?;
        let scope = ImageScope {
            client: Arc::downgrade(&client),
            conversation,
            seq: 0,
            id,
            draft: true,
        };
        open_or_reuse_viewer(&window, &state, &client, scope, dimensions).await
    }
    #[cfg(not(windows))]
    {
        let _ = (window, state, id);
        Err(anyhow::anyhow!("The standalone image viewer is available on Windows").into())
    }
}

#[tauri::command]
pub(crate) async fn open_image_viewer(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    #[cfg(windows)]
    {
        require(window.label() == "main", "Open an image from chat")?;
        let _opening = state.image_window.lock().await;
        let client = state.client().await?;
        let message = client.attachment(id).await?;
        let scope = ImageScope {
            client: Arc::downgrade(&client),
            conversation: message.conversation_id,
            seq: message.seq,
            id,
            draft: false,
        };
        let file = scope
            .image(&client)
            .await?
            .attachment
            .context("Missing image")?;
        let dimensions = image_dimensions(&client, message.conversation_id, message.seq, file).await;
        require(state.is_current(&client).await, "The session has ended")?;
        scope.image(&client).await?;
        open_or_reuse_viewer(&window, &state, &client, scope, dimensions).await
    }
    #[cfg(not(windows))]
    {
        let _ = (window, state, id);
        Err(anyhow::anyhow!("The standalone image viewer is available on Windows").into())
    }
}

#[cfg(windows)]
async fn open_or_reuse_viewer(
    window: &WebviewWindow,
    state: &Desktop,
    client: &Arc<Client>,
    scope: ImageScope,
    dimensions: Option<(u32, u32)>,
) -> CommandResult<()> {
    let app = window.app_handle();
    if let Some(viewer) = app.get_webview_window(LABEL) {
        *state.image_scope.write().expect("image scope") = Some(scope);
        viewer.unminimize().map_err(anyhow::Error::from)?;
        fit_open_window(&viewer, dimensions)?;
        viewer.set_focus().map_err(anyhow::Error::from)?;
        return Ok(());
    }
    let monitor = window
        .current_monitor()
        .map_err(anyhow::Error::from)?
        .or(window.primary_monitor().map_err(anyhow::Error::from)?)
        .context("No display is available")?;
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let work = area.size.to_logical::<f64>(scale);
    let (width, height) = initial_size(dimensions, work.width, work.height);
    *state.image_scope.write().expect("image scope") = Some(scope);
    let viewer = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Image viewer")
    .on_navigation(crate::local_navigation)
    .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
    .data_directory(state.webview_directory())
    .decorations(false)
    .inner_size(width, height)
    .min_inner_size(MIN_VIEWER_WIDTH.min(width), 176.0_f64.min(height))
    .visible(false)
    .build()
    .map_err(anyhow::Error::from)?;
    let position = PhysicalPosition::new(area.position.x, area.position.y);
    let result = (|| {
        viewer.set_position(position)?;
        viewer.set_size(LogicalSize::new(width, height).to_physical::<u32>(scale))?;
        let outer = viewer.outer_size()?;
        let inner = viewer.inner_size()?;
        let limit = PhysicalSize::new(
            area.size
                .width
                .saturating_sub(outer.width.saturating_sub(inner.width)),
            area.size
                .height
                .saturating_sub(outer.height.saturating_sub(inner.height)),
        );
        if inner.width > limit.width || inner.height > limit.height {
            viewer.set_size(PhysicalSize::new(
                inner.width.min(limit.width),
                inner.height.min(limit.height),
            ))?;
        }
        let outer = viewer.outer_size()?;
        viewer.set_position(PhysicalPosition::new(
            position.x + (area.size.width.saturating_sub(outer.width) / 2) as i32,
            position.y + (area.size.height.saturating_sub(outer.height) / 2) as i32,
        ))?;
        viewer.show()?;
        viewer.set_focus()
    })();
    if let Err(error) = result {
        let _ = viewer.destroy();
        return Err(anyhow::Error::from(error).into());
    }
    // The account may have signed out while the native window was being created.
    if !state.is_current(client).await {
        let _ = viewer.destroy();
    }
    Ok(())
}

#[cfg(windows)]
async fn bytes_dimensions(bytes: Vec<u8>) -> Option<(u32, u32)> {
    tauri::async_runtime::spawn_blocking(move || {
        let reader = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut decoder = reader.into_decoder().ok()?;
        let (width, height) = decoder.dimensions();
        Some(match decoder.orientation().ok()? {
            Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH => (height, width),
            _ => (width, height),
        })
    })
    .await
    .ok()
    .flatten()
}

#[cfg(windows)]
async fn image_dimensions(
    client: &Client,
    conversation_id: Uuid,
    seq: i64,
    file: Attachment,
) -> Option<(u32, u32)> {
    if file.preview_mime().is_some_and(|mime| mime.starts_with("video/")) {
        if let Ok(Some((_, bytes))) = client
            .read_media_bytes(conversation_id, seq, Some(file.id), MediaVariant::VideoCover, &file.sha256)
            .await
            && let Some(dims) = bytes_dimensions(bytes).await
        {
            return Some(dims);
        }
        return Some((720, 440));
    }
    let (_, bytes) = media::image_preview(client, conversation_id, seq, file, false)
        .await
        .ok()?;
    bytes_dimensions(bytes).await
}

#[cfg(windows)]
fn fit_open_window(viewer: &WebviewWindow, dimensions: Option<(u32, u32)>) -> Result<()> {
    let Some(dimensions) = dimensions else {
        return Ok(());
    };
    if viewer.is_maximized()? || viewer.is_fullscreen()? {
        return Ok(());
    }
    let Some(monitor) = viewer.current_monitor()?.or(viewer.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let work = area.size.to_logical::<f64>(scale);
    let (width, height) = initial_size(Some(dimensions), work.width, work.height);
    let desired = LogicalSize::new(width, height).to_physical::<u32>(scale);
    let inner = viewer.inner_size()?;
    let outer = viewer.outer_size()?;
    let position = viewer.outer_position()?;
    viewer.set_size(PhysicalSize::new(
        desired.width.min(
            area.size
                .width
                .saturating_sub(outer.width.saturating_sub(inner.width)),
        ),
        desired.height.min(
            area.size
                .height
                .saturating_sub(outer.height.saturating_sub(inner.height)),
        ),
    ))?;
    let fitted = viewer.outer_size()?;
    let x = position.x + (outer.width as i32 - fitted.width as i32) / 2;
    let y = position.y + (outer.height as i32 - fitted.height as i32) / 2;
    viewer.set_position(PhysicalPosition::new(
        x.clamp(
            area.position.x,
            area.position.x + area.size.width.saturating_sub(fitted.width) as i32,
        ),
        y.clamp(
            area.position.y,
            area.position.y + area.size.height.saturating_sub(fitted.height) as i32,
        ),
    ))?;
    Ok(())
}

#[cfg(any(windows, test))]
fn initial_size(dimensions: Option<(u32, u32)>, width: f64, height: f64) -> (f64, f64) {
    const CHROME_HEIGHT: f64 = 44.0;
    let (w, h) = dimensions.unwrap_or((416, 280));
    let (w, h) = (f64::from(w), f64::from(h));
    let factor = ((width * 0.9) / w)
        .min((height * 0.9 - CHROME_HEIGHT).max(1.0) / h)
        .min(dimensions.map_or(1.0, |_| (MIN_VIEWER_WIDTH / w.max(h)).max(1.0)));
    (
        (w * factor).max(MIN_VIEWER_WIDTH).min(width),
        (h * factor + CHROME_HEIGHT).max(176.0).min(height),
    )
}

#[derive(Serialize)]
pub(crate) struct ViewerVideoState {
    has_local_original: bool,
    has_local_preview: bool,
    has_local_cover: bool,
    original_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    original_download: Option<OriginalDownloadStatus>,
}

#[derive(Serialize)]
pub(crate) struct ViewerSelection {
    conversation_id: Uuid,
    seq: i64,
    file: Attachment,
    media_kind: &'static str,
    previous: bool,
    next: bool,
    download: Option<Transfer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    video: Option<ViewerVideoState>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    draft: bool,
}

#[tauri::command]
pub(crate) async fn image_viewer_selection(
    window: WebviewWindow,
    state: State<'_, Desktop>,
) -> CommandResult<ViewerSelection> {
    require(window.label() == LABEL, "Open the image viewer")?;
    let selection = scope(&state)?;
    let client = selection.client(&state).await?;
    if selection.draft {
        let (conversation, file) = state.drafts.draft_image_entry(&client, selection.id)?;
        require(conversation == selection.conversation, "This draft image is no longer available")?;
        let previous = state
            .drafts
            .draft_image_neighbor(&client, conversation, selection.id, false)
            .is_some();
        let next = state
            .drafts
            .draft_image_neighbor(&client, conversation, selection.id, true)
            .is_some();
        authorize_draft(&state, file.id).await?;
        return Ok(ViewerSelection {
            conversation_id: conversation,
            seq: 0,
            file,
            media_kind: "image",
            previous,
            next,
            download: None,
            video: None,
            draft: true,
        });
    }
    let message = selection.image(&client).await?;
    let file = message.attachment.context("Missing media")?;
    let cutoff = selection.cutoff(&client).await?;
    let (previous, next) = tokio::try_join!(
        selection.neighbor(&client, false, cutoff),
        selection.neighbor(&client, true, cutoff)
    )?;
    let download = client.transfers().await?.into_iter().find(|transfer| {
        transfer.direction == Direction::Download && transfer.attachment.id == file.id
    });
    let is_video = file.preview_mime().is_some_and(|mime| mime.starts_with("video/"));
    let video = if is_video {
        let (has_orig, has_prev, has_cov, dl) = tokio::join!(
            client.has_cached_media_variant(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoOriginal,
                &file.sha256,
            ),
            client.has_cached_media_variant(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoPreview,
                &file.sha256,
            ),
            client.has_cached_media_variant(
                message.conversation_id,
                message.seq,
                Some(file.id),
                MediaVariant::VideoCover,
                &file.sha256,
            ),
            client.original_video_download_status(file.id),
        );
        Some(ViewerVideoState {
            has_local_original: has_orig.unwrap_or(false),
            has_local_preview: has_prev.unwrap_or(false),
            has_local_cover: has_cov.unwrap_or(false),
            original_size: file.size,
            original_download: dl,
        })
    } else {
        None
    };
    authorize(&state, file.id).await?;
    Ok(ViewerSelection {
        conversation_id: message.conversation_id,
        seq: message.seq,
        file,
        media_kind: if is_video { "video" } else { "image" },
        previous: previous.is_some(),
        next: next.is_some(),
        download,
        video,
        draft: false,
    })
}

#[tauri::command]
pub(crate) async fn viewer_download_original_video(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<OriginalDownloadStatus> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let client = authorize(&state, id).await?;
    Ok(client.start_original_video_download(id).await?)
}

#[tauri::command]
pub(crate) async fn viewer_cancel_original_video(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let client = authorize(&state, id).await?;
    Ok(client.cancel_original_video_download(id).await?)
}

async fn authorize_video_original(
    state: &Desktop,
    id: Uuid,
) -> Result<(Arc<Client>, std::path::PathBuf, Attachment)> {
    let client = authorize(state, id).await?;
    let message = client.attachment(id).await?;
    let file = message.attachment.context("Missing video")?;
    ensure!(
        file.preview_mime().is_some_and(|mime| mime.starts_with("video/")),
        "Only video attachments can be trimmed"
    );
    let path = client
        .local_video_original_path(message.conversation_id, message.seq, &file, true)
        .await?
        .context("Download and verify the original video before editing")?;
    authorize(state, id).await?;
    Ok((client, path, file))
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((combined >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((combined >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((combined >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(combined & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[tauri::command]
pub(crate) async fn viewer_video_probe(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<crate::video_edit::VideoProbeInfo> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let (_client, path, _file) = authorize_video_original(&state, id).await?;
    let bundled = state.video_exports.bundled_ffmpeg_dir();
    Ok(crate::video_edit::probe_video(&path, bundled.as_deref()).await?)
}

#[tauri::command]
pub(crate) async fn viewer_video_thumbnails(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
    count: Option<usize>,
) -> CommandResult<Vec<String>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let (_client, path, _file) = authorize_video_original(&state, id).await?;
    let bundled = state.video_exports.bundled_ffmpeg_dir();
    let frames = crate::video_edit::extract_thumbnails(&path, count.unwrap_or(10), bundled.as_deref()).await?;
    Ok(frames
        .into_iter()
        .map(|bytes| format!("data:image/jpeg;base64,{}", encode_base64(&bytes)))
        .collect())
}

#[tauri::command]
pub(crate) async fn viewer_start_video_export(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
    start_sec: f64,
    end_sec: f64,
    duration_sec: f64,
) -> CommandResult<crate::video_edit::VideoExportProgress> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let (client, path, _file) = authorize_video_original(&state, id).await?;
    Ok(state
        .video_exports
        .start_trim_export(
            id,
            &path,
            client.media_root(),
            Some(client.as_ref()),
            start_sec,
            end_sec,
            duration_sec,
        )
        .await?)
}

#[tauri::command]
pub(crate) async fn viewer_video_export_status(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    export_id: Uuid,
) -> CommandResult<Option<crate::video_edit::VideoExportProgress>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    Ok(state.video_exports.status(export_id))
}

#[tauri::command]
pub(crate) async fn viewer_cancel_video_export(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    export_id: Uuid,
) -> CommandResult<Option<crate::video_edit::VideoExportProgress>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    Ok(state.video_exports.cancel(export_id).await)
}

#[derive(Serialize)]
pub(crate) struct ViewerForwardTargets {
    own_id: Uuid,
    current_conversation_id: Uuid,
    directory: Directory,
}

#[derive(Clone, Serialize)]
struct ViewerMediaSentEvent {
    sent: usize,
    queued: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnnotatedSendMeta {
    source_id: Uuid,
    batch_id: Uuid,
    targets: Vec<ForwardTarget>,
    file_name: String,
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    ensure!(bytes.len() % 4 == 0, "Invalid metadata encoding");
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let a = val(chunk[0]).context("Invalid base64 byte")?;
        let b = val(chunk[1]).context("Invalid base64 byte")?;
        let c = if chunk[2] == b'=' {
            None
        } else {
            Some(val(chunk[2]).context("Invalid base64 byte")?)
        };
        let d = if chunk[3] == b'=' {
            None
        } else {
            Some(val(chunk[3]).context("Invalid base64 byte")?)
        };
        let combined = (a << 18) | (b << 12) | (c.unwrap_or(0) << 6) | d.unwrap_or(0);
        out.push(((combined >> 16) & 0xff) as u8);
        if c.is_some() {
            out.push(((combined >> 8) & 0xff) as u8);
        }
        if d.is_some() {
            out.push((combined & 0xff) as u8);
        }
    }
    Ok(out)
}

fn sanitize_viewer_filename(raw: &str, fallback_stem: &str, ext: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .trim_end_matches(['.', ' ']);
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let candidate = if trimmed.is_empty() {
        format!("{fallback_stem}.{ext}")
    } else if !trimmed.contains('.') {
        format!("{trimmed}.{ext}")
    } else {
        trimmed.to_owned()
    };
    if candidate.len() > 160 {
        format!("{fallback_stem}.{ext}")
    } else {
        candidate
    }
}

fn emit_viewer_media_sent(app: &AppHandle, state: &Desktop, items: &[ViewerForwardItem]) {
    let sent = items.iter().filter(|item| item.status == "sent").count();
    let queued = items.iter().filter(|item| item.status == "queued").count();
    if queued > 0 {
        state.wake_outbox();
    }
    if sent > 0 || queued > 0 {
        let _ = app.emit_to("main", "viewer-media-sent", ViewerMediaSentEvent { sent, queued });
    }
}

#[tauri::command]
pub(crate) async fn viewer_forward_targets(
    window: WebviewWindow,
    state: State<'_, Desktop>,
) -> CommandResult<ViewerForwardTargets> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let selection = scope(&state)?;
    require(!selection.draft, "Draft media cannot be forwarded")?;
    let client = selection.client(&state).await?;
    let directory = client.directory().await?;
    Ok(ViewerForwardTargets {
        own_id: client.user.id,
        current_conversation_id: selection.conversation,
        directory,
    })
}

#[tauri::command]
pub(crate) async fn viewer_resolve_direct_target(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    contact_id: Uuid,
) -> CommandResult<Uuid> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let selection = scope(&state)?;
    require(!selection.draft, "Draft media cannot be forwarded")?;
    let client = selection.client(&state).await?;
    Ok(client
        .create_conversation(ConversationRequest::Direct { contact_id })
        .await?)
}

#[tauri::command]
pub(crate) async fn viewer_send_unedited(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Desktop>,
    source_id: Uuid,
    batch_id: Uuid,
    targets: Vec<ForwardTarget>,
) -> CommandResult<Vec<ViewerForwardItem>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let client = authorize(&state, source_id).await?;
    let items = client
        .forward_or_queue_media(
            source_id,
            &ForwardRequest {
                batch_id,
                targets,
                skip_attachment_ids: Vec::new(),
            },
        )
        .await?;
    emit_viewer_media_sent(&app, &state, &items);
    Ok(items)
}

#[tauri::command]
pub(crate) async fn viewer_send_annotated_image(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<Vec<ViewerForwardItem>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let encoded_meta = request
        .headers()
        .get("zibblu-forward-meta")
        .and_then(|value| value.to_str().ok())
        .context("Missing send metadata")?;
    let meta_bytes = decode_base64(encoded_meta)?;
    let meta: AnnotatedSendMeta = serde_json::from_slice(&meta_bytes).context("Invalid send metadata")?;

    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(anyhow::anyhow!("Missing exported image payload").into());
    };
    require(
        !bytes.is_empty() && bytes.len() <= MAX_PASTED_IMAGE_BYTES,
        "Annotated images must be between 1 byte and 16 MiB",
    )?;
    let owned = bytes.to_vec();
    let mime = {
        let permit = media::IMAGE_JOBS.acquire().await.map_err(anyhow::Error::from)?;
        let bytes_for_check = owned.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            crate::images::verified_mime(&bytes_for_check)
        })
        .await
        .map_err(anyhow::Error::from)??
    };
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/gif" => "gif",
        _ => "png",
    };
    let safe_name = sanitize_viewer_filename(&meta.file_name, "annotated", ext);
    let client = authorize(&state, meta.source_id).await?;
    let tmp_dir = client.media_root().join("tmp");
    tokio::fs::create_dir_all(&tmp_dir)
        .await
        .context("Could not prepare temporary directory")?;
    let prefix = format!("annotated-{}-", Uuid::new_v4().simple());
    let _temp_guard = client.track_temp_prefix(&prefix);
    let tmp_path = tmp_dir.join(format!("{prefix}export.{ext}"));
    tokio::fs::write(&tmp_path, &owned)
        .await
        .context("Could not write temporary annotated image")?;

    let queue_res = client
        .queue_edited_media_batch(safe_name, tmp_path.clone(), meta.batch_id, meta.targets.clone())
        .await;
    let _ = tokio::fs::remove_file(&tmp_path).await;
    queue_res?;

    let items: Vec<ViewerForwardItem> = meta
        .targets
        .into_iter()
        .map(|target| ViewerForwardItem {
            target,
            status: "queued".into(),
            error: None,
            cache_error: None,
        })
        .collect();
    emit_viewer_media_sent(&app, &state, &items);
    Ok(items)
}

#[tauri::command]
pub(crate) async fn viewer_send_trimmed_video(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, Desktop>,
    source_id: Uuid,
    export_id: Uuid,
    batch_id: Uuid,
    targets: Vec<ForwardTarget>,
    file_name: String,
) -> CommandResult<Vec<ViewerForwardItem>> {
    require(window.label() == LABEL, "Open the media viewer")?;
    let client = authorize(&state, source_id).await?;
    let path = state
        .video_exports
        .take_completed_path(export_id)
        .context("Trimmed video export is not ready")?;
    let safe_name = sanitize_viewer_filename(&file_name, "trimmed", "mp4");
    let queue_res = client
        .queue_edited_media_batch(safe_name, path.clone(), batch_id, targets.clone())
        .await;
    state.video_exports.cleanup_entry(export_id).await;
    let _ = tokio::fs::remove_file(&path).await;
    queue_res?;

    let items: Vec<ViewerForwardItem> = targets
        .into_iter()
        .map(|target| ViewerForwardItem {
            target,
            status: "queued".into(),
            error: None,
            cache_error: None,
        })
        .collect();
    emit_viewer_media_sent(&app, &state, &items);
    Ok(items)
}

#[tauri::command]
pub(crate) async fn image_viewer_step(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
    forward: bool,
) -> CommandResult<()> {
    require(window.label() == LABEL, "Open the image viewer")?;
    #[cfg(windows)]
    let _opening = state.image_window.lock().await;
    let selection = scope(&state)?;
    if selection.draft {
        let client = authorize_draft(&state, id).await?;
        let Some(next_id) =
            state
                .drafts
                .draft_image_neighbor(&client, selection.conversation, id, forward)
        else {
            return Ok(());
        };
        let (_, bytes) = state.drafts.preview_image(&client, next_id, false).await?;
        #[cfg(windows)]
        let dimensions = bytes_dimensions(bytes).await;
        #[cfg(not(windows))]
        let _ = bytes;
        let next = ImageScope {
            id: next_id,
            ..selection.clone()
        };
        {
            let mut active = state.image_scope.write().expect("image scope");
            if !active.as_ref().is_some_and(|active| {
                active.draft && active.id == id && active.client.ptr_eq(&selection.client)
            }) {
                return Ok(());
            }
            *active = Some(next);
        }
        #[cfg(windows)]
        fit_open_window(&window, dimensions)?;
        return Ok(());
    }
    let client = authorize(&state, id).await?;
    state.video_exports.cancel_for_attachment(id).await;
    let cutoff = selection.cutoff(&client).await?;
    let Some(message) = selection.neighbor(&client, forward, cutoff).await? else {
        return Ok(());
    };
    let file = message.attachment.context("Missing image")?;
    let next = ImageScope {
        seq: message.seq,
        id: file.id,
        ..selection.clone()
    };
    #[cfg(windows)]
    let dimensions = image_dimensions(&client, next.conversation, next.seq, file).await;
    next.image(&client).await?;
    {
        let mut active = state.image_scope.write().expect("image scope");
        if !active
            .as_ref()
            .is_some_and(|active| active.id == id && active.client.ptr_eq(&selection.client))
        {
            return Ok(());
        }
        *active = Some(next);
    }
    #[cfg(windows)]
    fit_open_window(&window, dimensions)?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn copy_viewer_image(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    id: Uuid,
) -> CommandResult<()> {
    #[cfg(windows)]
    {
        require(
            window.label() == LABEL && window.is_focused().map_err(anyhow::Error::from)?,
            "Focus the image viewer before copying",
        )?;
        let selection = scope(&state)?;
        let bytes = if selection.draft {
            let client = authorize_draft(&state, id).await?;
            state.drafts.preview_image(&client, id, false).await?.1
        } else {
            let client = authorize(&state, id).await?;
            let message = client.attachment(id).await?;
            media::image_preview(
                &client,
                message.conversation_id,
                message.seq,
                message.attachment.context("Missing image")?,
                false,
            )
            .await?
            .1
        };
        let permit = media::IMAGE_JOBS
            .acquire()
            .await
            .map_err(anyhow::Error::from)?;
        let png = tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            crate::images::clipboard_formats(&bytes)
        })
        .await
        .map_err(anyhow::Error::from)??;
        if selection.draft {
            authorize_draft(&state, id).await?;
        } else {
            authorize(&state, id).await?;
        }
        require(
            window.is_focused().map_err(anyhow::Error::from)?,
            "Focus the image viewer before copying",
        )?;
        crate::clipboard::write_image(&window, &png.0, &png.1)?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (window, state, id);
        Err(anyhow::anyhow!("Image copying is available on Windows").into())
    }
}

#[cfg(test)]
mod tests {
    use super::{MIN_VIEWER_WIDTH, initial_size};

    #[test]
    fn size_respects_image_ratio_and_logical_work_area() {
        assert_eq!(
            initial_size(Some((640, 640)), 1920.0, 1080.0),
            (640.0, 684.0)
        );
        let (w, h) = initial_size(Some((3600, 2400)), 1280.0, 720.0);
        assert!((w / (h - 44.0) - 3.0 / 2.0).abs() < 0.001);
        assert!(w <= 1280.0 && h <= 720.0);
        assert_eq!(
            initial_size(Some((320, 440)), 1280.0, 720.0).0,
            MIN_VIEWER_WIDTH
        );
    }

    #[test]
    fn small_images_get_room_to_browse_at_their_aspect_ratio() {
        let (landscape_w, landscape_h) = initial_size(Some((120, 80)), 1280.0, 720.0);
        assert_eq!(landscape_w, MIN_VIEWER_WIDTH);
        assert!((landscape_w / (landscape_h - 44.0) - 1.5).abs() < 0.001);
        let (portrait_w, portrait_h) = initial_size(Some((80, 120)), 1280.0, 720.0);
        assert_eq!(portrait_w, MIN_VIEWER_WIDTH);
        assert!(portrait_h > landscape_h && portrait_h <= 720.0);
    }
}
