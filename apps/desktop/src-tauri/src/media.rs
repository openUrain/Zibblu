use crate::commands::{Desktop, Result as CommandResult};
use crate::images;
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
#[cfg(not(target_os = "ios"))]
use std::io::Read;
#[cfg(not(target_os = "ios"))]
use tauri_plugin_dialog::DialogExt;
use tauri::{
    AppHandle, Manager, State, UriSchemeContext, UriSchemeResponder, WebviewWindow, Wry,
    http::{Method, Request, Response, StatusCode, header, response::Builder},
    ipc::InvokeBody,
};
use tokio::sync::Semaphore;
use uuid::Uuid;
use zibblu_core::{CHUNK_SIZE, MAX_PASTED_IMAGE_BYTES, ProfileImage, Transfer};

static IMAGE_JOBS: Semaphore = Semaphore::const_new(2);

fn image_body(body: &InvokeBody) -> Result<&[u8]> {
    let InvokeBody::Raw(bytes) = body else {
        anyhow::bail!("Choose a {} image.", images::FORMATS);
    };
    ensure!(
        bytes.len() <= MAX_PASTED_IMAGE_BYTES,
        "Images can be up to 16 MiB."
    );
    Ok(bytes)
}

async fn process_upload<T: Send + 'static>(
    body: &InvokeBody,
    work: impl FnOnce(Vec<u8>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let bytes = image_body(body)?;
    let permit = IMAGE_JOBS.acquire().await?;
    let bytes = bytes.to_vec();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        work(bytes)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn pick_image(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    for_profile: bool,
) -> CommandResult<tauri::ipc::Response> {
    if window.label() != "main" {
        return Err(anyhow::anyhow!("Open the profile in the main window").into());
    }
    let client = state.client().await?;
    #[cfg(target_os = "ios")]
    let selected = {
        let _ = for_profile;
        crate::ios::pick_photo().await?
    };
    #[cfg(not(target_os = "ios"))]
    let selected = tauri::async_runtime::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
        let picker = window.dialog().file()
            .set_title(if for_profile { "Choose a profile image" } else { "Choose a QR image" })
            .add_filter("Images", &["png", "apng", "jpg", "jpeg", "webp", "gif", "bmp"]);
        #[cfg(desktop)]
        let picker = picker.set_parent(&window);
        let Some(path) = picker.blocking_pick_file() else { return Ok(None) };
        let file = std::fs::File::open(path.into_path()?).context("Could not open the selected image")?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file() && metadata.len() <= MAX_PASTED_IMAGE_BYTES as u64,
            "Choose an image up to 16 MiB");
        let mut bytes = Vec::new();
        file.take(MAX_PASTED_IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }).await.context("Image picker was interrupted")??;
    let Some(bytes) = selected else { return Ok(tauri::ipc::Response::new(Vec::<u8>::new())) };
    let body = InvokeBody::Raw(bytes);
    let png = process_upload(&body, |bytes| images::profile_png(&bytes)).await?;
    if !state.is_current(&client).await {
        return Err(anyhow::anyhow!("The session has ended").into());
    }
    Ok(tauri::ipc::Response::new(png))
}

#[tauri::command]
pub(crate) async fn set_profile_image(
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<Uuid> {
    let kind = match request
        .headers()
        .get("zibblu-profile-image")
        .and_then(|value| value.to_str().ok())
    {
        Some("avatar") => ProfileImage::Avatar,
        Some("cover") => ProfileImage::Cover,
        _ => return Err(anyhow::anyhow!("Choose an avatar or cover image").into()),
    };
    let client = state.client().await?;
    let (png, thumbnail) = process_upload(request.body(), move |bytes| {
        let png = images::profile_png(&bytes)?;
        let thumbnail = if cfg!(any(windows, target_os = "ios")) && matches!(kind, ProfileImage::Avatar) {
            images::saved_avatar(&png).ok()
        } else {
            None
        };
        Ok((png, thumbnail))
    })
    .await?;
    let version = client.set_profile_image(kind, png).await?;
    if let Some(thumbnail) = thumbnail {
        state.remember_avatar(&client, thumbnail).await;
    }
    Ok(version)
}

#[tauri::command]
pub(crate) async fn check_pasted_image(
    request: tauri::ipc::Request<'_>,
) -> CommandResult<&'static str> {
    Ok(process_upload(request.body(), |bytes| images::verified_mime(&bytes)).await?)
}

#[tauri::command]
pub(crate) async fn read_clipboard_image(window: WebviewWindow) -> CommandResult<tauri::ipc::Response> {
    #[cfg(windows)]
    {
        let _permit = IMAGE_JOBS.acquire().await.map_err(anyhow::Error::from)?;
        let bytes = tauri::async_runtime::spawn_blocking(move || -> CommandResult<Vec<u8>> {
            let Some(bitmap) = crate::clipboard::read_bitmap(&window)? else {
                return Ok(Vec::new());
            };
            Ok(images::clipboard_png(&bitmap)?)
        })
        .await
        .context("Opening the clipboard was interrupted")??;
        Ok(tauri::ipc::Response::new(bytes))
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err(anyhow::anyhow!("Use the system Paste action").into())
    }
}

#[tauri::command]
pub(crate) async fn paste_image(
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<Transfer> {
    let conversation = request
        .headers()
        .get("zibblu-conversation")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .context("Open a normal chat before pasting an image")?;
    let client = state.client().await?;
    let bytes = process_upload(request.body(), |bytes| {
        images::verified_mime(&bytes)?;
        Ok(bytes)
    })
    .await?;
    Ok(client.queue_pasted_image(conversation, bytes).await?)
}

pub(crate) fn preview(
    context: UriSchemeContext<'_, Wry>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    if context.webview_label() != "main" {
        responder.respond(
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Vec::new())
                .unwrap(),
        );
        return;
    }
    let app = context.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let response = match media_response(&app, request).await {
            Ok(response) => response,
            Err(_) => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Vec::new())
                .unwrap(),
        };
        responder.respond(response);
    });
}

async fn media_response(app: &AppHandle, request: Request<Vec<u8>>) -> Result<Response<Vec<u8>>> {
    ensure!(
        *request.method() == Method::GET || *request.method() == Method::HEAD,
        "Invalid media request"
    );
    ensure!(
        request
            .headers()
            .get("sec-fetch-dest")
            .is_none_or(|value| value == "image" || value == "video"),
        "Open the preview in a normal message"
    );
    let path = request.uri().path().trim_start_matches('/');
    let client = app
        .state::<Desktop>()
        .client()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to preview media"))?;
    if let Some((id, version)) = path
        .strip_prefix("profile_")
        .and_then(|path| path.split_once('_'))
    {
        let id = Uuid::parse_str(id)?;
        let version = Uuid::parse_str(version)?;
        let bytes = match client.cached_profile_image(id, version).await {
            Some(bytes) => bytes,
            None => client.profile_image(id, version).await?,
        };
        images::check_stored_profile(&bytes)?;
        ensure!(app.state::<Desktop>().is_current(&client).await, "The session has ended");
        return Ok(preview_headers("image/png")
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(if *request.method() == Method::HEAD {
                Vec::new()
            } else {
                bytes
            })?);
    }
    let id = Uuid::parse_str(path)?;
    let file = client
        .attachment(id)
        .await?
        .attachment
        .context("Missing attachment")?;
    let mime_hint = file.preview_mime().context("Unsupported media type")?;
    if mime_hint.starts_with("video/") {
        let response = preview_headers(mime_hint).header(header::ACCEPT_RANGES, "bytes");
        if *request.method() == Method::HEAD {
            return Ok(response
                .header(header::CONTENT_LENGTH, file.size)
                .body(Vec::new())?);
        }
        let range = request.headers().get(header::RANGE);
        let (start, end) = if let Some(value) = range {
            match value
                .to_str()
                .ok()
                .and_then(|value| preview_range(value, file.size))
            {
                Some(range) => range,
                None => {
                    return Ok(response
                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                        .header(header::CONTENT_RANGE, format!("bytes */{}", file.size))
                        .body(Vec::new())?);
                }
            }
        } else {
            (0, file.size - 1)
        };
        // Serve at most one existing transfer chunk per range.
        let end = end.min((start / CHUNK_SIZE + 1) * CHUNK_SIZE - 1);
        let offset = start - start % CHUNK_SIZE;
        let chunk = client
            .read_attachment_chunk(id, offset, (file.size - offset).min(CHUNK_SIZE) as usize)
            .await?;
        let body = chunk[(start - offset) as usize..=(end - offset) as usize].to_vec();
        return Ok(response
            .status(StatusCode::PARTIAL_CONTENT)
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{}", file.size),
            )
            .header(header::CONTENT_LENGTH, body.len())
            .body(body)?);
    }

    ensure!(
        file.size <= MAX_PASTED_IMAGE_BYTES as u64,
        "Image too large for inline preview"
    );
    let permit = IMAGE_JOBS.acquire().await?;
    let mut bytes = Vec::with_capacity(file.size as usize);
    for offset in (0..file.size).step_by(CHUNK_SIZE as usize) {
        bytes.extend(
            client
                .read_attachment_chunk(id, offset, (file.size - offset).min(CHUNK_SIZE) as usize)
                .await?,
        );
    }
    let (mime, bytes) = tauri::async_runtime::spawn_blocking(move || -> Result<_> {
        let _permit = permit;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == file.sha256,
            "Image verification failed"
        );
        let mime = images::verified_mime(&bytes)?;
        Ok((mime, bytes))
    })
    .await??;
    Ok(preview_headers(mime)
        .header(header::ACCEPT_RANGES, "none")
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(if *request.method() == Method::HEAD {
            Vec::new()
        } else {
            bytes
        })?)
}

fn preview_headers(mime: &str) -> Builder {
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
}

fn preview_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let length: u64 = end.parse().ok()?;
        return (length > 0).then(|| (size.saturating_sub(length), size - 1));
    }
    let start = start.parse().ok()?;
    let end = if end.is_empty() {
        size - 1
    } else {
        end.parse::<u64>().ok()?.min(size - 1)
    };
    (start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_preview_accepts_a_single_satisfiable_range() {
        assert_eq!(preview_range("bytes=0-", 500), Some((0, 499)));
        assert_eq!(preview_range("bytes=-10", 500), Some((490, 499)));
        assert_eq!(preview_range("bytes=200-800", 500), Some((200, 499)));
        assert_eq!(preview_range("bytes=500-", 500), None);
        assert_eq!(preview_range("bytes=0-1,3-4", 500), None);
    }
}
