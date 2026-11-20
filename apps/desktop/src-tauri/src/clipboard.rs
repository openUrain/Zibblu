use crate::commands::Result;
use anyhow::anyhow;
use tauri::WebviewWindow;

#[tauri::command]
pub(crate) fn copy_text(window: WebviewWindow, text: String) -> Result<()> {
    if window.label() != "main" {
        return Err(anyhow!("Copy from the main window").into());
    }
    #[cfg(windows)]
    {
        if !window.is_focused().unwrap_or(false) {
            return Err(anyhow!("Focus Zibblu before copying").into());
        }
        windows::write(window.hwnd().map_err(anyhow::Error::from)?.0 as _, &text)?;
        return Ok(());
    }
    #[cfg(target_os = "ios")]
    {
        crate::ios::copy_text(&text)?;
        return Ok(());
    }
    #[cfg(not(any(windows, target_os = "ios")))]
    {
        let _ = text;
        Err(anyhow!("System clipboard is unavailable").into())
    }
}

#[tauri::command]
pub(crate) fn paste_text(window: WebviewWindow) -> Result<Option<String>> {
    #[cfg(windows)]
    {
        if window.label() != "main" || !window.is_focused().unwrap_or(false) {
            return Err(anyhow!("Focus Zibblu before pasting").into());
        }
        return Ok(windows::read(window.hwnd().map_err(anyhow::Error::from)?.0 as _)?);
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err(anyhow!("Use the system Paste action").into())
    }
}

#[cfg(windows)]
pub(crate) fn read_bitmap(window: &WebviewWindow) -> Result<Option<Vec<u8>>> {
    if window.label() != "main" || !window.is_focused().unwrap_or(false) {
        return Err(anyhow!("Focus Zibblu before pasting").into());
    }
    Ok(windows::read_bitmap(
        window.hwnd().map_err(anyhow::Error::from)?.0 as _,
    )?)
}

#[cfg(windows)]
mod windows {
    use anyhow::{Context, Result, ensure};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT},
        },
    };

    const FORMAT: u32 = CF_UNICODETEXT as u32;
    const MAX_UNITS: usize = 1_048_576;

    struct Clipboard;

    impl Clipboard {
        fn open(owner: HWND) -> Result<Self> {
            ensure!(unsafe { OpenClipboard(owner) } != 0, "Clipboard is busy. Try again.");
            Ok(Self)
        }
    }

    impl Drop for Clipboard {
        fn drop(&mut self) {
            unsafe { CloseClipboard() };
        }
    }

    pub(super) fn read(owner: HWND) -> Result<Option<String>> {
        let _clipboard = Clipboard::open(owner)?;
        if unsafe { IsClipboardFormatAvailable(FORMAT) } == 0 {
            return Ok(None);
        }
        let data = unsafe { GetClipboardData(FORMAT) };
        ensure!(!data.is_null(), "The clipboard text is unavailable");
        let length = unsafe { GlobalSize(data) } / size_of::<u16>();
        ensure!(length <= MAX_UNITS, "Clipboard text is too large");
        let ptr = unsafe { GlobalLock(data) }.cast::<u16>();
        ensure!(!ptr.is_null(), "The clipboard text is unavailable");
        let text = unsafe { std::slice::from_raw_parts(ptr, length) };
        let value = text.iter().position(|unit| *unit == 0)
            .map(|end| String::from_utf16_lossy(&text[..end]));
        unsafe { GlobalUnlock(data) };
        Ok(Some(value.context("The clipboard text is invalid")?))
    }

    pub(super) fn read_bitmap(owner: HWND) -> Result<Option<Vec<u8>>> {
        let _clipboard = Clipboard::open(owner)?;
        let Some(format) = [CF_DIBV5, CF_DIB]
            .into_iter()
            .map(u32::from)
            .find(|format| unsafe { IsClipboardFormatAvailable(*format) } != 0)
        else {
            return Ok(None);
        };
        let data = unsafe { GetClipboardData(format) };
        ensure!(!data.is_null(), "The clipboard image is unavailable");
        let length = unsafe { GlobalSize(data) };
        ensure!(
            length <= zibblu_core::MAX_PASTED_IMAGE_BYTES,
            "Clipboard images can be up to 16 MiB."
        );
        let ptr = unsafe { GlobalLock(data) }.cast::<u8>();
        ensure!(!ptr.is_null(), "The clipboard image is unavailable");
        let bytes = unsafe { std::slice::from_raw_parts(ptr, length) }.to_vec();
        unsafe { GlobalUnlock(data) };
        Ok(Some(bytes))
    }

    pub(super) fn write(owner: HWND, text: &str) -> Result<()> {
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        ensure!(text.len() <= MAX_UNITS && !text[..text.len() - 1].contains(&0), "Cannot copy this text");
        let _clipboard = Clipboard::open(owner)?;
        let data = unsafe { GlobalAlloc(GMEM_MOVEABLE, text.len() * size_of::<u16>()) };
        ensure!(!data.is_null(), "Could not prepare clipboard text");
        let ptr = unsafe { GlobalLock(data) }.cast::<u16>();
        if !ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(text.as_ptr(), ptr, text.len());
                GlobalUnlock(data);
                if EmptyClipboard() != 0 && !SetClipboardData(FORMAT, data).is_null() {
                    // Windows owns the memory after SetClipboardData succeeds.
                    return Ok(());
                }
            }
        }
        unsafe { GlobalFree(data) };
        anyhow::bail!("Could not write to the clipboard. Try again.")
    }
}
