use crate::commands::{Desktop, Result as CommandResult};
#[cfg(windows)]
use crate::media;
use anyhow::{Context, Result, ensure};
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

#[derive(Clone)]
pub(crate) struct ImageScope {
    client: Weak<Client>,
    conversation: Uuid,
    seq: i64,
    id: Uuid,
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
    ensure!(selection.id == id, "This image is outside the viewer");
    let client = selection.client(state).await?;
    selection.image(&client).await?;
    let selected = scope(state)?;
    ensure!(
        selected.id == id && selected.client.ptr_eq(&selection.client),
        "The selected image changed"
    );
    Ok(client)
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
        };
        let file = scope
            .image(&client)
            .await?
            .attachment
            .context("Missing image")?;
        let app = window.app_handle();
        if let Some(viewer) = app.get_webview_window(LABEL) {
            *state.image_scope.write().expect("image scope") = Some(scope);
            viewer.unminimize().map_err(anyhow::Error::from)?;
            viewer.set_focus().map_err(anyhow::Error::from)?;
            return Ok(());
        }
        let dimensions = match media::image_preview(&client, message.seq, file, false).await {
            Ok((_, bytes)) => tauri::async_runtime::spawn_blocking(move || {
                image::ImageReader::new(Cursor::new(bytes))
                    .with_guessed_format()?
                    .into_dimensions()
                    .map_err(anyhow::Error::from)
            })
            .await
            .map_err(anyhow::Error::from)?
            .ok(),
            Err(_) => None,
        };
        let monitor = window
            .current_monitor()
            .map_err(anyhow::Error::from)?
            .or(window.primary_monitor().map_err(anyhow::Error::from)?)
            .context("No display is available")?;
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let work = area.size.to_logical::<f64>(scale);
        let (width, height) = initial_size(dimensions, work.width, work.height);
        require(state.is_current(&client).await, "The session has ended")?;
        scope.image(&client).await?;
        *state.image_scope.write().expect("image scope") = Some(scope);
        let viewer = tauri::WebviewWindowBuilder::new(
            app,
            LABEL,
            tauri::WebviewUrl::App("index.html".into()),
        )
        .title("Zibblu · Image viewer")
        .on_navigation(crate::local_navigation)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .data_directory(state.webview_directory())
        .inner_size(width, height)
        .min_inner_size(336.0_f64.min(width), 176.0_f64.min(height))
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
        if !state.is_current(&client).await {
            let _ = viewer.destroy();
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (window, state, id);
        Err(anyhow::anyhow!("The standalone image viewer is available on Windows").into())
    }
}

#[cfg(any(windows, test))]
fn initial_size(dimensions: Option<(u32, u32)>, width: f64, height: f64) -> (f64, f64) {
    const TOOLBAR: f64 = 44.0;
    let (w, h) = dimensions.unwrap_or((416, 280));
    let (w, h) = (f64::from(w), f64::from(h));
    let factor = ((width * 0.9) / w)
        .min((height * 0.9 - TOOLBAR).max(1.0) / h)
        .min(1.0);
    (
        (w * factor).max(336.0).min(width),
        (h * factor + TOOLBAR).max(176.0).min(height),
    )
}

#[derive(Serialize)]
pub(crate) struct ViewerSelection {
    file: Attachment,
    previous: bool,
    next: bool,
    download: Option<Transfer>,
}

#[tauri::command]
pub(crate) async fn image_viewer_selection(
    window: WebviewWindow,
    state: State<'_, Desktop>,
) -> CommandResult<ViewerSelection> {
    require(window.label() == LABEL, "Open the image viewer")?;
    let selection = scope(&state)?;
    let client = selection.client(&state).await?;
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
    let selection = scope(&state)?;
    let client = authorize(&state, id).await?;
    let cutoff = selection.cutoff(&client).await?;
    let Some(message) = selection.neighbor(&client, forward, cutoff).await? else {
        return Ok(());
    };
    let mut scope = state.image_scope.write().expect("image scope");
    if let Some(active) = scope
        .as_mut()
        .filter(|active| active.id == id && active.client.ptr_eq(&selection.client))
    {
        active.seq = message.seq;
        active.id = message.attachment.context("Missing image")?.id;
    }
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
        let client = authorize(&state, id).await?;
        let message = client.attachment(id).await?;
        let (_, bytes) = media::image_preview(
            &client,
            message.seq,
            message.attachment.context("Missing image")?,
            false,
        )
        .await?;
        let png =
            tauri::async_runtime::spawn_blocking(move || crate::images::clipboard_formats(&bytes))
                .await
                .map_err(anyhow::Error::from)??;
        authorize(&state, id).await?;
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
    use super::initial_size;

    #[test]
    fn size_respects_image_ratio_and_logical_work_area() {
        assert_eq!(
            initial_size(Some((640, 640)), 1920.0, 1080.0),
            (640.0, 684.0)
        );
        let (w, h) = initial_size(Some((2400, 3600)), 1280.0, 720.0);
        assert!((w / (h - 44.0) - 2.0 / 3.0).abs() < 0.001);
        assert!(w <= 1280.0 && h <= 720.0);
    }
}
