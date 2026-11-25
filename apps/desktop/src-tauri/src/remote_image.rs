use crate::{commands::Desktop, images};
use anyhow::{Context, Result, ensure};
use image::ImageFormat;
use reqwest::{Client, StatusCode, Url, header, redirect, retry};
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, State, WebviewWindow, ipc::Response};
use tauri_plugin_opener::OpenerExt;
use tokio::{net::lookup_host, sync::oneshot, time::timeout};
use uuid::Uuid;

const MAX_URL: usize = 4096;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const DEADLINE: Duration = Duration::from_secs(6);

#[derive(Default)]
pub(crate) struct RemotePreview(Mutex<PreviewSlot>);

#[derive(Default)]
struct PreviewSlot {
    active: Option<ActivePreview>,
    canceled_before_start: VecDeque<Uuid>,
}

struct ActivePreview {
    id: Uuid,
    cancel: Option<oneshot::Sender<()>>,
}

impl PreviewSlot {
    fn start(&mut self, id: Uuid, cancel: oneshot::Sender<()>) -> Result<()> {
        if let Some(index) = self
            .canceled_before_start
            .iter()
            .position(|pending| *pending == id)
        {
            self.canceled_before_start.remove(index);
            anyhow::bail!("Preview closed before it started");
        }
        ensure!(self.active.is_none(), "Another preview is still active");
        self.active = Some(ActivePreview {
            id,
            cancel: Some(cancel),
        });
        Ok(())
    }

    fn cancel(&mut self, id: Uuid) {
        if let Some(active) = self.active.as_mut().filter(|active| active.id == id) {
            if let Some(cancel) = active.cancel.take() {
                let _ = cancel.send(());
            }
        } else {
            // The synchronous cancel IPC can arrive before the async preview task is polled.
            if self.canceled_before_start.len() == 8 {
                self.canceled_before_start.pop_front();
            }
            self.canceled_before_start.push_back(id);
        }
    }
}

fn web_url(text: &str) -> Result<Url> {
    ensure!(text.len() <= MAX_URL, "URL too long");
    let url = Url::parse(text)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "Expected an HTTP(S) URL without credentials"
    );
    Ok(url)
}

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => !matches!(
            address.octets(),
            [0 | 10 | 127 | 224..=255, ..]
                | [100, 64..=127, ..]
                | [168, 63, 129, 16]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 0, 0 | 2, _]
                | [192, 88, 99, _]
                | [192, 168, ..]
                | [198, 18 | 19, ..]
                | [198, 51, 100, _]
                | [203, 0, 113, _]
        ),
        IpAddr::V6(address) => {
            let [first, second, ..] = address.segments();
            // Only global unicast, excluding protocol, tunnel and documentation ranges.
            first & 0xe000 == 0x2000
                && !(first == 0x2001 && (second < 0x0200 || second == 0x0db8))
                && first != 0x2002
                && !(first == 0x3fff && second < 0x1000)
        }
    }
}

async fn pinned_client(url: &Url) -> Result<(Client, Vec<SocketAddr>)> {
    ensure!(url.scheme() == "https", "Preview requires HTTPS");
    let host = url.host_str().context("Missing host")?;
    let port = url.port_or_known_default().context("Missing port")?;
    let mut builder = Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(redirect::Policy::none())
        .retry(retry::never())
        .no_gzip()
        .no_brotli()
        .no_zstd()
        .no_deflate();
    let addresses = if let Some(domain) = url.domain() {
        let name = domain.trim_end_matches('.');
        ensure!(
            name.contains('.')
                && ![".localhost", ".local", ".internal", ".home.arpa"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
                && name != "home.arpa",
            "Expected a public host"
        );
        // A fully qualified DNS query prevents search suffixes from changing the target.
        lookup_host((format!("{name}."), port)).await?.collect()
    } else {
        let ip = host.trim_matches(['[', ']']).parse::<IpAddr>()?;
        vec![SocketAddr::new(ip, port)]
    };
    ensure!(
        !addresses.is_empty() && addresses.iter().all(|address| public_address(address.ip())),
        "Preview target is not public"
    );
    let local = if_addrs::get_if_addrs()?;
    ensure!(
        addresses
            .iter()
            .all(|address| local.iter().all(|interface| interface.ip() != address.ip())),
        "Preview target belongs to this device"
    );
    if url.domain().is_some() {
        // TLS still verifies the original URL hostname. The connector uses these vetted IPs.
        builder = builder.resolve_to_addrs(host, &addresses);
    }
    Ok((builder.build()?, addresses))
}

fn preview_format(mime: &str) -> Result<ImageFormat> {
    match mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => Ok(ImageFormat::Png),
        "image/jpeg" => Ok(ImageFormat::Jpeg),
        "image/webp" => Ok(ImageFormat::WebP),
        _ => anyhow::bail!("Unsupported preview type"),
    }
}

async fn download(mut url: Url) -> Result<(ImageFormat, Vec<u8>)> {
    for hop in 0..=MAX_REDIRECTS {
        let (client, addresses) = pinned_client(&url).await?;
        let mut response = client
            .get(url.clone())
            .header(header::ACCEPT, "image/png,image/jpeg,image/webp")
            .send()
            .await?;
        ensure!(
            response
                .remote_addr()
                .is_some_and(|address| addresses.contains(&address)),
            "Preview connection did not use the vetted address"
        );
        match response.status() {
            StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT => {
                ensure!(hop < MAX_REDIRECTS, "Too many redirects");
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .context("Missing location")?;
                url = web_url(url.join(location.to_str()?)?.as_str())?;
                continue;
            }
            StatusCode::OK => {}
            _ => anyhow::bail!("Preview request failed"),
        }
        let format = preview_format(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .context("Missing content type")?
                .to_str()?,
        )?;
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= MAX_BYTES as u64),
            "Preview is too large"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                chunk.len() <= MAX_BYTES - bytes.len(),
                "Preview is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        ensure!(
            image::guess_format(&bytes)? == format,
            "Preview type does not match its content"
        );
        return Ok((format, bytes));
    }
    unreachable!("redirect limit checked on the last hop")
}

#[tauri::command]
pub(crate) fn open_message_link(
    app: AppHandle,
    window: WebviewWindow,
    url: String,
) -> Result<(), String> {
    const ERROR: &str = "Could not open this link.";
    if !crate::chat_window::is_chat_or_main_window(window.label()) {
        return Err(ERROR.into());
    }
    let url = web_url(&url).map_err(|_| ERROR.to_owned())?;
    app.opener()
        .open_url(url.as_str(), None::<&str>)
        .map_err(|_| ERROR.into())
}

#[tauri::command]
pub(crate) async fn preview_message_image(
    window: WebviewWindow,
    state: State<'_, Desktop>,
    preview: State<'_, RemotePreview>,
    id: Uuid,
    url: String,
) -> Result<Response, String> {
    async fn run(
        window: WebviewWindow,
        state: &Desktop,
        preview: &RemotePreview,
        id: Uuid,
        url: &str,
    ) -> Result<Response> {
        ensure!(
            crate::chat_window::is_chat_or_main_window(window.label()),
            "Preview is available in chat windows"
        );
        let session = state
            .client()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to preview images"))?;
        let url = web_url(url)?;
        let (cancel, mut canceled) = oneshot::channel();
        preview
            .0
            .lock()
            .expect("remote preview")
            .start(id, cancel)?;
        let result = tokio::select! {
            _ = &mut canceled => Err(anyhow::anyhow!("Preview closed")),
            result = timeout(DEADLINE, download(url)) => result.context("Preview timed out").and_then(|result| result),
        };
        let result = match result {
            Ok((format, bytes)) => {
                // Keep the slot occupied while a bounded, non-cancellable decode is running.
                tauri::async_runtime::spawn_blocking(move || {
                    images::remote_thumbnail(format, &bytes)
                })
                .await
                .context("Preview decoding stopped")
                .and_then(|result| result)
            }
            Err(error) => Err(error),
        };
        let active = preview.0.lock().expect("remote preview").active.take();
        ensure!(
            active.is_some_and(|active| active.cancel.is_some()),
            "Preview closed"
        );
        ensure!(state.is_current(&session).await, "Session ended");
        Ok(Response::new(result?))
    }
    // Do not surface request errors containing a URL from a private session.
    run(window, &state, &preview, id, &url)
        .await
        .map_err(|_| "Image preview unavailable.".into())
}

#[tauri::command]
pub(crate) fn cancel_message_image(
    window: WebviewWindow,
    preview: State<'_, RemotePreview>,
    id: Uuid,
) {
    if crate::chat_window::is_chat_or_main_window(window.label()) {
        preview.0.lock().expect("remote preview").cancel(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_covers_both_ipc_orders() {
        let id = Uuid::new_v4();
        let mut state = PreviewSlot::default();
        state.cancel(id);
        assert!(state.start(id, oneshot::channel().0).is_err());
        let (send, mut receive) = oneshot::channel();
        state.start(id, send).unwrap();
        assert!(state.start(Uuid::new_v4(), oneshot::channel().0).is_err());
        state.cancel(id);
        assert_eq!(receive.try_recv(), Ok(()));
    }

    #[test]
    fn preview_targets_and_mime() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "100.100.100.200",
            "168.63.129.16",
            "169.254.169.254",
            "192.168.1.1",
            "224.0.0.1",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "fe80::1",
            "fc00::1",
            "2001:db8::1",
            "2002:a00:1::1",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_address("93.184.215.14".parse().unwrap()));
        assert!(public_address("2606:4700::1111".parse().unwrap()));
        assert!(web_url("file:///C:/Windows/win.ini").is_err());
        assert!(web_url("https://user:password@example.com/image.png").is_err());
        assert_eq!(
            preview_format("image/png; charset=binary").unwrap(),
            ImageFormat::Png
        );
        assert!(preview_format("image/svg+xml").is_err());
        assert!(images::remote_thumbnail(ImageFormat::Png, b"<html></html>").is_err());
    }

    #[tokio::test]
    async fn rejects_local_targets_before_connecting() {
        for url in [
            "https://127.1/a.png",
            "https://[::1]/a.png",
            "https://localhost./a.png",
            "https://test.local/a.png",
            "http://93.184.215.14/a.png",
        ] {
            assert!(
                pinned_client(&web_url(url).unwrap()).await.is_err(),
                "{url}"
            );
        }
    }
}
