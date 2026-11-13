use crate::commands::{Desktop, Result as CommandResult};
use anyhow::{Context, Result, ensure};
use tauri::{
    AppHandle, Manager, State, UriSchemeContext, UriSchemeResponder, Wry,
    http::{Method, Request, Response, StatusCode, header},
    ipc::InvokeBody,
};
use uuid::Uuid;
use zibblu_core::{
    CHUNK_SIZE, MAX_PASTED_IMAGE_BYTES, MAX_PROFILE_IMAGE_BYTES, ProfileImage, Transfer,
};

#[tauri::command]
pub(crate) async fn set_profile_image(
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<()> {
    let kind = match request
        .headers()
        .get("zibblu-profile-image")
        .and_then(|value| value.to_str().ok())
    {
        Some("avatar") => ProfileImage::Avatar,
        Some("cover") => ProfileImage::Cover,
        _ => return Err(anyhow::anyhow!("Choose an avatar or cover image").into()),
    };
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(anyhow::anyhow!("Choose a PNG image").into());
    };
    if bytes.len() > MAX_PROFILE_IMAGE_BYTES {
        return Err(anyhow::anyhow!("Profile images can be up to 2 MiB").into());
    }
    Ok(state
        .client()
        .await?
        .set_profile_image(kind, bytes.clone())
        .await?)
}

#[tauri::command]
pub(crate) async fn paste_image(
    state: State<'_, Desktop>,
    request: tauri::ipc::Request<'_>,
) -> CommandResult<Transfer> {
    let result = (|| -> Result<_> {
        let conversation = request
            .headers()
            .get("zibblu-conversation")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
            .context("Open a normal chat before pasting an image")?;
        let InvokeBody::Raw(bytes) = request.body() else {
            anyhow::bail!("Paste an image from the clipboard");
        };
        ensure!(
            bytes.len() <= MAX_PASTED_IMAGE_BYTES,
            "Pasted images can be up to 16 MiB"
        );
        Ok((conversation, bytes.clone()))
    })()?;
    Ok(state
        .client()
        .await?
        .queue_pasted_image(result.0, result.1)
        .await?)
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
        let bytes = client
            .profile_image(Uuid::parse_str(id)?, Uuid::parse_str(version)?)
            .await?;
        return Ok(Response::builder()
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CACHE_CONTROL, "no-store")
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
            .header(
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; sandbox",
            )
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
    let mime = file.preview_mime().context("Unsupported media type")?;
    let video = mime.starts_with("video/");
    let response = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; sandbox",
        )
        .header(header::ACCEPT_RANGES, if video { "bytes" } else { "none" });
    if *request.method() == Method::HEAD {
        return Ok(response
            .header(header::CONTENT_LENGTH, file.size)
            .body(Vec::new())?);
    }

    if video {
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
    let mut bytes = Vec::with_capacity(file.size as usize);
    for offset in (0..file.size).step_by(CHUNK_SIZE as usize) {
        bytes.extend(
            client
                .read_attachment_chunk(id, offset, (file.size - offset).min(CHUNK_SIZE) as usize)
                .await?,
        );
    }
    Ok(response
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(bytes)?)
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
