//! Menu bar presentation: the dropdown panel window, tray icon, and the
//! window-event rules (hide on blur, keep developer windows alive).

use std::time::Duration;
use tauri::{
    image::Image,
    menu::{Menu, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

use crate::log::app_log;

pub(crate) const PANEL_WIDTH: f64 = 380.0;
pub(crate) const PANEL_HEIGHT: f64 = 560.0;

#[cfg(target_os = "macos")]
pub(crate) fn activate_app() {
    use objc2::{class, msg_send};
    use objc2::runtime::AnyObject;
    unsafe {
        let nsapp: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !nsapp.is_null() {
            let _: bool = msg_send![&*nsapp, activateIgnoringOtherApps: true];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn activate_app() {}

/// Dropdown position for a tray click: centered under the menu bar icon,
/// clamped to the screen with an 8px margin.
fn dropdown_position(
    cursor: (f64, f64),
    panel_w: f64,
    scale: f64,
    screen_w: f64,
) -> (f64, f64) {
    let half = panel_w / 2.0;
    let x = (cursor.0 - half).clamp(8.0, (screen_w - panel_w - 8.0).max(8.0));
    let y = (cursor.1 + 6.0 * scale).max(4.0);
    (x, y)
}

pub(crate) fn show_panel(app: &AppHandle, cursor: Option<tauri::PhysicalPosition<f64>>) {
    let Some(panel) = app.get_webview_window("panel") else {
        return;
    };

    if let Some(pos) = cursor {
        let size = panel.inner_size().ok().unwrap_or(tauri::PhysicalSize::new(
            (PANEL_WIDTH * 2.0) as u32,
            (PANEL_HEIGHT * 2.0) as u32,
        ));
        let scale = app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.scale_factor())
            .unwrap_or(2.0);
        let screen_w = app
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().width as f64)
            .unwrap_or(PANEL_WIDTH * scale * 2.0);
        let (x, y) = dropdown_position((pos.x, pos.y), size.width as f64, scale, screen_w);
        let _ = panel.set_position(tauri::PhysicalPosition::new(x, y));
    }

    let _ = panel.show();
    let _ = panel.set_focus();
    activate_app();
    app_log(app, "panel", "shown");
    // The webview may have been suspended while hidden and missed
    // "checkouts-updated" — let the panel refresh stale data.
    let _ = app.emit("panel-shown", true);
}

pub(crate) fn toggle_panel(app: &AppHandle, cursor: Option<tauri::PhysicalPosition<f64>>) {
    if let Some(panel) = app.get_webview_window("panel") {
        if panel.is_visible().unwrap_or(false) {
            let _ = panel.hide();
            return;
        }
    }
    show_panel(app, cursor);
}

/// Tray icon: a monochrome template image macOS tints to the menu bar
/// styling (artwork: tools/make_icons.swift). Left click toggles the panel.
pub(crate) fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let icon = Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
        .expect("embedded tray icon");
    let quit = PredefinedMenuItem::quit(app.handle(), Some("Quit SacPL Loans"))?;
    let menu = Menu::with_items(app.handle(), &[&quit])?;

    TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(true)
        .tooltip(format!(
            "SacPL Loans v{} \u{2014} Sacramento Public Library",
            app.config().version.clone().unwrap_or_default()
        ))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                position,
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_panel(tray.app_handle(), Some(position));
            }
        })
        .build(app)?;
    Ok(())
}

/// Window-event rules shared by all windows: closing the bridge or Debug
/// Events windows hides them (the bridge is the app's lifeline to the
/// catalog and rebuilding it would lose the session), and the panel hides
/// on blur unless a developer window took focus.
pub(crate) fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() == "bridge" {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
            app_log(
                window.app_handle(),
                "dev",
                "hidden browser closed — kept alive, window hidden"
            );
            let _ = window.app_handle().emit("bridge-visibility", false);
        }
    }
    if window.label() == "debug-events" {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window.hide();
        }
    }
    if window.label() == "panel" {
        if let tauri::WindowEvent::Focused(false) = event {
            if window.is_visible().unwrap_or(false) {
                let w = window.clone();
                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    // The blur event can fire before the focus handoff to a
                    // developer window completes, so wait 180ms and then
                    // hide only if no developer window holds focus.
                    std::thread::sleep(Duration::from_millis(180));
                    let developer_window_focused = ["bridge", "debug-events"].iter().any(|label| {
                        app.get_webview_window(label)
                            .and_then(|window| window.is_focused().ok())
                            .unwrap_or(false)
                    });
                    if !developer_window_focused {
                        let _ = w.hide();
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropdown_centers_below_the_tray_icon() {
        let (x, y) = dropdown_position((1000.0, 48.0), 380.0, 2.0, 1440.0);
        assert_eq!(x, 810.0);
        assert_eq!(y, 60.0);
    }

    #[test]
    fn dropdown_is_clamped_to_the_screen_edges() {
        let (x, _) = dropdown_position((4.0, 0.0), 380.0, 2.0, 1440.0);
        assert_eq!(x, 8.0);
        let (x, _) = dropdown_position((1436.0, 0.0), 380.0, 2.0, 1440.0);
        assert_eq!(x, 1052.0); // 1440 - 380 - 8
    }

    #[test]
    fn dropdown_stays_on_screen_on_a_tiny_display() {
        // Screen narrower than the panel: the clamp must not panic.
        let (x, _) = dropdown_position((10.0, 0.0), 380.0, 1.0, 100.0);
        assert_eq!(x, 8.0);
    }
}