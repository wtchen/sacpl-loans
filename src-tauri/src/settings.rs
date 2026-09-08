//! App settings: the background-refresh interval. The value lives in an
//! atomic shared by the settings commands and the refresh timer (changes
//! apply without a restart), and is persisted to the app data dir.

use std::sync::atomic::{AtomicU64, Ordering};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::debug::DEBUG_ACTIVE;
use crate::log::app_log;

/// The UI/Settings floor; SACPL_REFRESH_SECS (diagnostics) may go lower.
pub(crate) const MIN_REFRESH_SECS: u64 = 600;
pub(crate) const DEFAULT_REFRESH_SECS: u64 = 3600;
const MIN_ENV_REFRESH_SECS: u64 = 30;

/// Shared between the settings commands and the refresh timer thread.
pub(crate) struct AppState {
    pub(crate) refresh_secs: AtomicU64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSettings {
    pub refresh_secs: u64,
    /// Whether the hidden bridge browser is currently shown (dev feature).
    pub bridge_visible: bool,
    /// Whether this build includes the Developer Settings debug features.
    pub debug_mode: bool,
}

fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.json"))
}

fn saved_refresh_secs(app: &AppHandle) -> Option<u64> {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["refreshSecs"].as_u64())
}

/// SACPL_REFRESH_SECS (>=30s, diagnostics) wins over the saved Settings
/// value, which is floored at 10 minutes and defaults to hourly.
pub(crate) fn initial_refresh_secs(app: &AppHandle) -> u64 {
    let env_secs = std::env::var("SACPL_REFRESH_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s >= MIN_ENV_REFRESH_SECS);
    resolve_refresh_secs(env_secs, saved_refresh_secs(app))
}

fn resolve_refresh_secs(env_secs: Option<u64>, saved: Option<u64>) -> u64 {
    env_secs
        .or_else(|| saved.map(|s| s.max(MIN_REFRESH_SECS)))
        .unwrap_or(DEFAULT_REFRESH_SECS)
}

fn bridge_visible(app: &AppHandle) -> bool {
    app.get_webview_window("bridge")
        .and_then(|b| b.is_visible().ok())
        .unwrap_or(false)
}

fn current_settings(app: &AppHandle, secs: u64) -> AppSettings {
    AppSettings { refresh_secs: secs, bridge_visible: bridge_visible(app), debug_mode: DEBUG_ACTIVE }
}

#[tauri::command]
pub(crate) fn lib_get_settings(app: AppHandle) -> AppSettings {
    let secs = app
        .try_state::<AppState>()
        .map(|s| s.refresh_secs.load(Ordering::Relaxed))
        .unwrap_or(DEFAULT_REFRESH_SECS);
    current_settings(&app, secs)
}

/// Set the refresh interval. The 10-minute floor lives here, not just in the
/// UI, so the catalog is never polled faster from any caller.
#[tauri::command]
pub(crate) fn lib_set_refresh_secs(app: AppHandle, secs: u64) -> Result<AppSettings, String> {
    let secs = secs.max(MIN_REFRESH_SECS);
    if let Some(state) = app.try_state::<AppState>() {
        state.refresh_secs.store(secs, Ordering::Relaxed);
    }
    if let Some(path) = settings_path(&app) {
        let _ = std::fs::write(path, serde_json::json!({ "refreshSecs": secs }).to_string());
    }
    app_log(
        &app,
        "settings",
        &format!("auto-refresh interval set to {} minutes", secs / 60)
    );
    Ok(current_settings(&app, secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_and_is_not_clamped_to_the_ui_floor() {
        assert_eq!(resolve_refresh_secs(Some(30), Some(1200)), 30);
        assert_eq!(resolve_refresh_secs(Some(900), None), 900);
    }

    #[test]
    fn saved_interval_is_clamped_to_the_ten_minute_floor() {
        assert_eq!(resolve_refresh_secs(None, Some(1200)), 1200);
        assert_eq!(resolve_refresh_secs(None, Some(10)), 600);
    }

    #[test]
    fn nothing_saved_defaults_to_hourly() {
        assert_eq!(resolve_refresh_secs(None, None), 3600);
    }
}