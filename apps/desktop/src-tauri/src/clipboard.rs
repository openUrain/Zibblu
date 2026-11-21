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
pub(crate) async fn read_files(window: &WebviewWindow) -> Result<Vec<std::path::PathBuf>> {
    if window.label() != "main" || !window.is_focused().unwrap_or(false) {
        return Err(anyhow!("Focus Zibblu before pasting").into());
    }
    let owner = window.hwnd().map_err(anyhow::Error::from)?.0 as isize;
    Ok(tauri::async_runtime::spawn_blocking(move || windows::read_files(owner as _))
        .await
        .map_err(anyhow::Error::from)??)
}

#[cfg(windows)]
pub(crate) fn write_image(window: &WebviewWindow, png: &[u8], dib: &[u8]) -> anyhow::Result<()> {
    windows::write_image(window.hwnd()?.0 as _, png, dib)
}

#[cfg(windows)]
mod windows {
    use anyhow::{Context, Result, ensure};
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr::null_mut, thread, time::Duration};
    use windows_sys::Win32::{
        Foundation::{GlobalFree, HWND},
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
        },
        UI::Shell::DragQueryFileW,
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

    pub(super) fn read_files(owner: HWND) -> Result<Vec<PathBuf>> {
        let format = u32::from(CF_HDROP);
        if unsafe { IsClipboardFormatAvailable(format) } == 0 {
            return Ok(Vec::new());
        }
        let mut opened = Clipboard::open(owner);
        // WebView2 may briefly own the clipboard during the same Ctrl+V.
        for _ in 0..5 {
            if opened.is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
            opened = Clipboard::open(owner);
        }
        let _clipboard = opened?;
        let drop = unsafe { GetClipboardData(format) };
        ensure!(!drop.is_null(), "The clipboard file list is unavailable");
        let count = unsafe { DragQueryFileW(drop, u32::MAX, null_mut(), 0) };
        ensure!(count <= 8, "Paste up to 8 attachments at a time");
        (0..count)
            .map(|index| {
                let length = unsafe { DragQueryFileW(drop, index, null_mut(), 0) };
                ensure!(
                    (1..32768).contains(&length),
                    "The clipboard contains an invalid file path"
                );
                let mut name = vec![0; length as usize + 1];
                let copied = unsafe { DragQueryFileW(drop, index, name.as_mut_ptr(), length + 1) };
                ensure!(copied == length, "Could not read a clipboard file path");
                let path = PathBuf::from(OsString::from_wide(&name[..length as usize]));
                ensure!(
                    path.is_absolute() && path.file_name().is_some(),
                    "Paste a file or folder from File Explorer"
                );
                Ok(path)
            })
            .collect()
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

    pub(super) fn write_image(owner: HWND, png: &[u8], dib: &[u8]) -> Result<()> {
        let _clipboard = Clipboard::open(owner)?;
        let png_format = unsafe { RegisterClipboardFormatW(windows_sys::w!("PNG")) };
        ensure!(png_format != 0 && unsafe { EmptyClipboard() } != 0, "Could not prepare the clipboard");
        put_bytes(png_format, png)?;
        // PNG is already on the clipboard when a legacy bitmap format is unavailable.
        let _ = put_bytes(u32::from(CF_DIB), dib);
        Ok(())
    }

    fn put_bytes(format: u32, bytes: &[u8]) -> Result<()> {
        let data = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
        ensure!(!data.is_null(), "Could not prepare clipboard image");
        let ptr = unsafe { GlobalLock(data) }.cast::<u8>();
        if !ptr.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                GlobalUnlock(data);
                if !SetClipboardData(format, data).is_null() {
                    return Ok(());
                }
            }
        }
        unsafe { GlobalFree(data) };
        anyhow::bail!("Could not write to the clipboard. Try again.")
    }
}
