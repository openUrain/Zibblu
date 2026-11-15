use crate::{
    commands::{CloseAction, Desktop},
    quit,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::utils::config::WindowConfig;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window,
};

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

pub(crate) fn restore_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.set_focus();
    }
}

pub(crate) fn create_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show-zibblu", "Show Zibblu", true, None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit-zibblu", "Exit Zibblu", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &exit])?;
    TrayIconBuilder::new()
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
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                restore_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub(crate) fn request_close(window: &Window) {
    match window.state::<Desktop>().close_action() {
        Some(CloseAction::Exit) => quit(window.app_handle()),
        Some(CloseAction::Tray) => {
            let _ = window.hide();
        }
        None => {
            let _ = window.emit_to("main", "choose-close-action", ());
        }
    }
}
