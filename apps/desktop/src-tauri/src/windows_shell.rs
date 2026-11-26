use crate::{
    commands::{CloseAction, Desktop},
    quit,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{File, TryLockError},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::utils::config::WindowConfig;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_DISALLOW_PEEK, DWMWA_FORCE_ICONIC_REPRESENTATION, DwmInvalidateIconicBitmaps,
    DwmSetWindowAttribute,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindowAsync,
};

const TRAY_ID: &str = "zibblu-tray";
struct TrayUnread(AtomicBool);

pub(crate) fn set_private_preview(window: &WebviewWindow, private: bool) -> anyhow::Result<()> {
    let hwnd = window.hwnd()?.0;
    let enabled = i32::from(private);
    for attribute in [DWMWA_FORCE_ICONIC_REPRESENTATION, DWMWA_DISALLOW_PEEK] {
        let result = unsafe {
            DwmSetWindowAttribute(hwnd, attribute as u32, (&enabled as *const i32).cast(), 4)
        };
        anyhow::ensure!(result >= 0, "Windows could not update the task switch preview");
    }
    let result = unsafe { DwmInvalidateIconicBitmaps(hwnd) };
    anyhow::ensure!(result >= 0, "Windows could not refresh the task switch preview");
    Ok(())
}

#[derive(serde::Serialize)]
pub(crate) struct StorageVolume {
    name: String,
    zibblu_bytes: u64,
    total_bytes: Option<u64>,
    free_bytes: Option<u64>,
}

pub(crate) fn storage_volumes(paths: [(Option<&Path>, u64); 2]) -> Vec<StorageVolume> {
    let mut volumes: Vec<(Vec<u16>, StorageVolume)> = Vec::new();
    for (path, bytes) in paths {
        let mut root = vec![0u16; 32768];
        let mut volume_id = [0u16; 64];
        let found = path.is_some_and(|path| {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) != 0 }
        });
        let key = if found
            && unsafe {
                GetVolumeNameForVolumeMountPointW(root.as_ptr(), volume_id.as_mut_ptr(), volume_id.len() as u32)
            } != 0
        {
            volume_id.split(|value| *value == 0).next().unwrap_or_default().to_vec()
        } else {
            root.split(|value| *value == 0).next().unwrap_or_default().to_vec()
        };
        if let Some((_, volume)) = volumes.iter_mut().find(|(id, _)| *id == key) {
            volume.zibblu_bytes = volume.zibblu_bytes.saturating_add(bytes);
            continue;
        }
        let name = OsString::from_wide(root.split(|value| *value == 0).next().unwrap_or_default());
        let (mut total, mut free) = (0, 0);
        let readable = found
            && unsafe { GetDiskFreeSpaceExW(root.as_ptr(), std::ptr::null_mut(), &mut total, &mut free) } != 0
            && total > 0
            && free <= total;
        volumes.push((key, StorageVolume {
            name: name.to_string_lossy().trim_end_matches('\\').into(),
            zibblu_bytes: bytes,
            total_bytes: readable.then_some(total),
            free_bytes: readable.then_some(free),
        }));
    }
    volumes.into_iter().map(|(_, volume)| volume).collect()
}

pub(crate) fn claim_profile(data: &Path) -> anyhow::Result<Option<(File, String)>> {
    let data = data.canonicalize()?;
    let window_class = format!(
        "Zibblu.Main.{:x}",
        Sha256::digest(data.as_os_str().as_encoded_bytes())
    );
    let wide_class: Vec<_> = window_class
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("instance.lock"))?;

    for _ in 0..40 {
        match file.try_lock() {
            Ok(()) => return Ok(Some((file, window_class))),
            Err(TryLockError::Error(error)) => return Err(error.into()),
            Err(TryLockError::WouldBlock) => {}
        }
        // A simultaneous launch can observe the lock before the main HWND exists.
        let window = unsafe { FindWindowW(wide_class.as_ptr(), std::ptr::null()) };
        if !window.is_null() {
            show_main(window);
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Another Zibblu instance is already using this data directory.");
}

pub(crate) fn center_on_startup(
    window: &WebviewWindow,
    config: &WindowConfig,
) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };
    let area = monitor.work_area();
    let inner = window.inner_size()?;
    let outer = window.outer_size()?;
    let available = PhysicalSize::new(
        area.size
            .width
            .saturating_sub(outer.width.saturating_sub(inner.width)),
        area.size
            .height
            .saturating_sub(outer.height.saturating_sub(inner.height)),
    );
    let fitted = PhysicalSize::new(
        inner.width.min(available.width),
        inner.height.min(available.height),
    );
    if fitted != inner {
        let max_logical = fitted.to_logical::<f64>(monitor.scale_factor());
        window.set_min_size(Some(LogicalSize::new(
            config.min_width.unwrap_or_default().min(max_logical.width),
            config
                .min_height
                .unwrap_or_default()
                .min(max_logical.height),
        )))?;
        window.set_size(fitted)?;
    }
    let outer = window.outer_size()?;
    window.set_position(PhysicalPosition::new(
        area.position.x + (area.size.width.saturating_sub(outer.width) / 2) as i32,
        area.position.y + (area.size.height.saturating_sub(outer.height) / 2) as i32,
    ))
}

fn show_main(window: HWND) {
    unsafe {
        let action = if IsIconic(window) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindowAsync(window, action);
        SetForegroundWindow(window);
    }
}

fn restore_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && let Ok(handle) = window.hwnd()
    {
        show_main(handle.0);
    }
}

pub(crate) fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show-zibblu", "Show Zibblu", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit-zibblu", "Exit Zibblu", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &exit])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Zibblu")
        .icon(app.default_window_icon().expect("Zibblu icon").clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show-zibblu" => restore_main(app),
            "exit-zibblu" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                restore_main(tray.app_handle());
            }
        })
        .build(app)?;
    app.manage(TrayUnread(AtomicBool::new(false)));
    Ok(())
}

pub(crate) fn set_tray_unread(app: &AppHandle, unread: bool) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let state = app.state::<TrayUnread>();
    if state.0.load(Ordering::Relaxed) == unread {
        return;
    }
    let Some(icon) = app.default_window_icon() else {
        return;
    };
    let icon = if unread {
        let (width, height) = (icon.width(), icon.height());
        let mut rgba = icon.rgba().to_vec();
        let radius = f64::from(width.min(height)) * 0.15;
        let center_x = f64::from(width) - radius - 1.0;
        let center_y = radius + 1.0;
        for y in 0..height {
            for x in 0..width {
                let distance = (f64::from(x) - center_x).hypot(f64::from(y) - center_y);
                if distance <= radius {
                    let offset = ((y * width + x) * 4) as usize;
                    rgba[offset..offset + 4].copy_from_slice(&[224, 39, 62, 255]);
                }
            }
        }
        tauri::image::Image::new_owned(rgba, width, height)
    } else {
        icon.clone()
    };
    if tray.set_icon(Some(icon)).is_ok() {
        state.0.store(unread, Ordering::Relaxed);
    }
}

pub(crate) fn request_close(window: &Window) {
    match window.state::<Desktop>().close_action() {
        Some(CloseAction::Exit) => quit(window.app_handle()),
        Some(CloseAction::Tray) => {
            // Relaunch can show this HWND from another process, outside Tauri's cached visibility.
            if let Ok(handle) = window.hwnd() {
                unsafe { ShowWindowAsync(handle.0, SW_HIDE) };
            }
        }
        None => {
            let _ = window.emit_to("main", "choose-close-action", ());
        }
    }
}
