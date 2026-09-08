//! Developer tooling: the `debug-mode` build flag and Developer Settings
//! commands (hidden browser, Debug Events window, event simulations).
//! Compiled out of default release builds.

use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::bridge::{bridge_call, eval_fire, js_str, BRIDGE_URL};
use crate::log::app_log;
use crate::panel::activate_app;
use crate::refresh::refresh_checkouts_bg;

/// True in dev mode (`npm run tauri dev`) and in release builds built with
/// the `debug-mode` cargo feature. Folds to `false` in default builds, which
/// lets the compiler eliminate all gated code.
pub(crate) const DEBUG_ACTIVE: bool = cfg!(feature = "debug-mode") || tauri::is_dev();

pub(crate) fn devtools_unavailable() -> String {
    "Developer tools are only available in dev mode and debug builds.".into()
}

pub(crate) fn dev_guard() -> Result<(), String> {
    if DEBUG_ACTIVE {
        Ok(())
    } else {
        Err(devtools_unavailable())
    }
}

/// Show/hide the bridge browser for inspection. It shares the app's session
/// and Cloudflare clearance; showing it first returns a navigated-away
/// bridge to the catalog so same-origin calls keep working.
#[tauri::command]
pub(crate) fn lib_show_bridge(app: AppHandle, show: bool) -> Result<bool, String> {
    dev_guard()?;
    let Some(bridge) = app.get_webview_window("bridge") else {
        return Err("The hidden browser is not available.".into());
    };
    if show {
        let url = bridge.url().map(|u| u.to_string()).unwrap_or_default();
        if !url.starts_with(crate::bridge::CATALOG_ORIGIN) {
            let _ = bridge.eval(format!("location.href = {}", js_str(BRIDGE_URL)));
        }
        let _ = bridge.show();
        let _ = bridge.set_focus();
        activate_app();
    } else {
        let _ = bridge.hide();
    }
    // The bridge can be closed/hidden via its own window close button —
    // outside the Settings button — so report the new state to the panel.
    let visible = bridge.is_visible().unwrap_or(false);
    app_log(
        &app,
        "dev",
        if visible { "hidden browser opened" } else { "hidden browser hidden" }
    );
    let _ = app.emit("bridge-visibility", visible);
    Ok(visible)
}

/// Navigate the bridge browser back to the catalog's My Account page.
#[tauri::command]
pub(crate) fn lib_reset_bridge(app: AppHandle) -> Result<(), String> {
    dev_guard()?;
    eval_fire(&app, &format!("location.href = {}", js_str(BRIDGE_URL)));
    Ok(())
}

/// Show or hide the separate Debug Events window.
#[tauri::command]
pub(crate) fn lib_show_debug_events(app: AppHandle, show: bool) -> Result<(), String> {
    dev_guard()?;
    let Some(window) = app.get_webview_window("debug-events") else {
        return Err("The Debug Events window is not available.".into());
    };
    if show {
        let _ = window.show();
        let _ = window.set_focus();
        activate_app();
    } else {
        let _ = window.hide();
    }
    Ok(())
}

/// Enter runtime debug mode from the Debug Events window and refocus the panel.
#[tauri::command]
pub(crate) fn lib_enter_debug_mode(app: AppHandle) -> Result<(), String> {
    dev_guard()?;
    let Some(panel) = app.get_webview_window("panel") else {
        return Err("The main loans window is not available.".into());
    };
    let _ = panel.show();
    let _ = panel.set_focus();
    activate_app();
    let _ = app.emit("debug-activate", ());
    Ok(())
}

/// Forward a fake debug loan from the Debug Events window to the panel.
#[tauri::command]
pub(crate) fn lib_add_debug_loan(app: AppHandle, loan: serde_json::Value) -> Result<(), String> {
    dev_guard()?;
    app.emit("debug-add-loan", loan).map_err(|e| e.to_string())
}

/// Pretend the list was last refreshed at `at_ms`, to exercise the
/// "Updated …" label and the stale-on-open refresh.
#[tauri::command]
pub(crate) fn lib_debug_set_refresh_time(app: AppHandle, at_ms: i64) -> Result<(), String> {
    dev_guard()?;
    app.emit("debug-last-updated", at_ms).map_err(|e| e.to_string())
}

/// Debug Events: simulate catalog events (one-shot flags consumed by the
/// next checkout fetch), or trigger them for real through the bridge where
/// possible. Every trigger is logged as SIMULATED or REAL.
#[tauri::command]
pub(crate) async fn lib_debug_event(app: AppHandle, event: String, real: bool) -> Result<String, String> {
    dev_guard()?;
    match (event.as_str(), real) {
        ("http403", false)
        | ("http500", false)
        | ("unreachable", false)
        | ("syncing", false)
        | ("loggedOut", false)
        | ("changedData", false) => {
            eval_fire(&app, &format!("window.__sim = {}", js_str(&event)));
            app_log(
                &app,
                "debug",
                &format!("SIMULATED event armed: {event} — fires on the next refresh"),
            );
            Ok(format!(
                "Simulated \u{201c}{event}\u{201d} armed — it fires on the next refresh (manual or background)."
            ))
        }
        // UI events only; credentials are never touched.
        ("expiredUnrecoverable", false) => {
            let _ = app.emit("session-reconnecting", true);
            app_log(
                &app,
                "debug",
                "SIMULATED event: unrecoverable session expiry (UI events only)",
            );
            let app2 = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2500));
                let _ = app2.emit("session-expired", true);
            });
            Ok("Simulating an unrecoverable session expiry — watch the loans view (~2s).".into())
        }
        ("loggedOut", true) => {
            let app2 = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let _ = bridge_call(&app2, "logout", "", 6, false);
            });
            app_log(
                &app,
                "debug",
                "REAL event triggered: signed the catalog session out (credentials kept — the next refresh recovers)",
            );
            Ok("Signed the catalog out for real — the next refresh will sign back in from the Keychain.".into())
        }
        ("changedData", true) => {
            let app2 = app.clone();
            std::thread::spawn(move || refresh_checkouts_bg(&app2));
            app_log(
                &app,
                "debug",
                "REAL event triggered: background refresh from the catalog",
            );
            Ok("Fetched fresh data from the catalog (background refresh).".into())
        }
        (_, true) => Err(
            "This event can't be triggered for real — uncheck the box to simulate it.".into()
        ),
        _ => Err("Unknown debug event.".into()),
    }
}