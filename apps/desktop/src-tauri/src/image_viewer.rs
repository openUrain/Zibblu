use crate::commands::{Desktop, Result as CommandResult};
#[cfg(windows)]
use crate::media;
use anyhow::{Context, Result, ensure};
#[cfg(windows)]
use image::{ImageDecoder, metadata::Orientation};
use serde::Serialize;
#[cfg(windows)]
use std::io::Cursor;
use std::sync::{Arc, Weak};
#[cfg(windows)]
use tauri::{LogicalSize, Manager, PhysicalPosition, PhysicalSize};
use tauri::{State, WebviewWindow};
use uuid::Uuid;
use zibblu_core::{Attachment, Client, Direction, FileFilter, Message, Transfer};

pub(crate) const LABEL: &str = "image-viewer";

pub(crate) fn close_deleted(app: &tauri::AppHandle, state: &Desktop, conversation: Uuid, seq: Option<i64>, include_drafts: bool) {
    #[cfg(windows)]
    {
        let mut scope = state.image_scope.write().expect("image scope");
        let close = scope.as_ref().is_some_and(|selection| selection.conversation == conversation
            && (!selection.draft || include_drafts)
            && seq.is_none_or(|seq| selection.seq == seq));
        if close {
            scope.take();
            drop(scope);
            if let Some(viewer) = app.get_webview_window(LABEL) {
                let _ = viewer.close();
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (app, state, conversation, seq, include_drafts);
}
#[cfg(any(windows, test))]
// Full-size controls, group and column gaps, and 12px of clearance beside each caption area.
const MIN_VIEWER_WIDTH: f64 = 8.0 * 36.0 + 4.0 * 8.0 + 2.0 * (3.0 * 44.0 + 12.0);

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

    fn allows(&self, message: &Message, cutoff: i64, now: i64) -> bool {
        message.conversation_id == self.conversation
            && message.seq > cutoff
            && !message.recalled
            && (message.local_file
                || message.expires_at.is_none_or(|end| end > now) && message.expired.is_none())
            && message.attachment.as_ref().is_some_and(|file| {
                file.preview_mime()
                    .is_some_and(|mime| mime.starts_with("image/"))
                    && file.size <= zibblu_core::MAX_PASTED_IMAGE_BYTES as u64
            })
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
        ensure!(
            message.seq == self.seq
                && self.allows(&message, self.cutoff(client).await?, client.server_now()),
            "This image has expired or is no longer available"
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
            if let Some(message) = page
                .items
                .iter()
                .find(|message| self.allows(message, cutoff, now))
            {
                return Ok(Some(message.clone()));
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
pub(crate) struct ViewerSelection {
    file: Attachment,
    previous: bool,
    next: bool,
    download: Option<Transfer>,
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
            file,
            previous,
            next,
            download: None,
            draft: true,
        });
    }
    let file = selection
        .image(&client)
        .await?
        .attachment
        .context("Missing image")?;
    let cutoff = selection.cutoff(&client).await?;
    let (previous, next) = tokio::try_join!(
        selection.neighbor(&client, false, cutoff),
        selection.neighbor(&client, true, cutoff)
    )?;
    let download = client.transfers().await?.into_iter().find(|transfer| {
        transfer.direction == Direction::Download && transfer.attachment.id == file.id
    });
    authorize(&state, file.id).await?;
    Ok(ViewerSelection {
        file,
        previous: previous.is_some(),
        next: next.is_some(),
        download,
        draft: false,
    })
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
