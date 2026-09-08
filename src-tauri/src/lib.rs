//! SacPL Loans — macOS menu bar app for the Sacramento Public Library.
//!
//! The catalog sits behind Cloudflare, which blocks non-browser TLS
//! fingerprints, so all catalog calls are same-origin `fetch()`es made inside
//! a hidden "bridge" webview (a real WebKit engine, see `bridge.js`). Its
//! cookies persist the Aspen session across app restarts. The visible "panel"
//! is the Svelte UI, shown as a dropdown below the menu bar icon.
//!
//! This file is only the composition root (windows, tray, threads, command
//! registry); each module documents its own role.

mod bridge;
mod cache;
mod catalog;
mod credentials;
mod debug;
mod log;
mod panel;
mod probes;
mod refresh;
mod session;
mod settings;

use panel::{PANEL_HEIGHT, PANEL_WIDTH};
use settings::AppState;
use std::sync::atomic::AtomicU64;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::bridge::{BRIDGE_JS, BRIDGE_URL};
use crate::log::app_log;
use crate::debug::DEBUG_ACTIVE;

#[tauri::command]
fn lib_quit(app: tauri::AppHandle) {
    app_log(&app, "app", "quitting");
    app.exit(0);
}

/// The Aspen session cookie doesn't survive an app restart; restore it from
/// the Keychain credentials on launch if the page reports signed out.
fn start_session_restore(handle: tauri::AppHandle) {
    std::thread::spawn(move || {
        let status = match catalog::wait_status(&handle, 90) {
            Ok(s) => s,
            Err(_) => return,
        };
        if status.ready && !status.logged_in {
            let _ = session::try_relogin(&handle);
        }
    });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Menu bar agent: no Dock icon.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    app_log(
        app.handle(),
        "app",
        &format!(
            "launched v{}{}",
            app.config().version.clone().unwrap_or_default(),
            if DEBUG_ACTIVE { " — developer tooling active" } else { "" }
        )
    );

    // Hidden bridge webview, revealed via Developer Settings for inspection.
    let _bridge = WebviewWindowBuilder::new(
        app,
        "bridge",
        WebviewUrl::External(BRIDGE_URL.parse().expect("static bridge url")),
    )
    .initialization_script(BRIDGE_JS)
    .title("SacPL Loans — Hidden Browser")
    .inner_size(1080.0, 720.0)
    .min_inner_size(420.0, 360.0)
    .visible(false)
    .build()?;

    // Dropdown panel, created hidden and shown on tray click.
    let _panel = WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html".into()))
        .title("SacPL Loans")
        .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    // Debug Events window, only in developer builds.
    if DEBUG_ACTIVE {
        WebviewWindowBuilder::new(app, "debug-events", WebviewUrl::App("debug-events".into()))
            .title("SacPL Loans — Debug Events")
            .inner_size(420.0, 620.0)
            .min_inner_size(360.0, 420.0)
            .visible(false)
            .build()?;
    }

    panel::setup_tray(app)?;
    probes::maybe_start(app.handle().clone());
    start_session_restore(app.handle().clone());

    // Renewal side effects (bridge call + app log), managed as state so
    // lib_renew_one stays self-contained.
    app.manage(catalog::RenewDeps::real(app.handle().clone()));

    // Background refresh loop; the interval is read from shared state.
    let initial_secs = settings::initial_refresh_secs(app.handle());
    app.manage(AppState {
        refresh_secs: AtomicU64::new(initial_secs),
    });
    refresh::start_loop(app.handle().clone());

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(setup)
        .on_window_event(panel::handle_window_event)
        .invoke_handler(tauri::generate_handler![
            catalog::lib_status,
            session::lib_login,
            session::lib_logout,
            catalog::lib_checkouts,
            catalog::lib_renew_one,
            catalog::lib_renew_all,
            lib_quit,
            cache::lib_cached_checkouts,
            catalog::lib_open_url,
            settings::lib_get_settings,
            settings::lib_set_refresh_secs,
            cache::lib_clear_cache,
            cache::lib_show_cache_dir,
            debug::lib_show_bridge,
            debug::lib_reset_bridge,
            log::lib_read_log,
            debug::lib_show_debug_events,
            debug::lib_enter_debug_mode,
            debug::lib_add_debug_loan,
            debug::lib_debug_set_refresh_time,
            debug::lib_debug_event
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}