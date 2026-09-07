//! SacPL Loans — macOS menu bar app for the Sacramento Public Library.
//!
//! Architecture:
//! - A hidden "bridge" webview loads `https://catalog.saclibrary.org/MyAccount/Home`
//!   in a real WebKit engine. This matters because the catalog sits behind
//!   Cloudflare, which blocks non-browser TLS fingerprints. A WKWebView is a
//!   real browser, so it passes any challenge automatically.
//! - All library API calls are same-origin `fetch()` calls made inside that
//!   bridge webview (see `bridge.js`). Their cookies (the Aspen session) live
//!   in WKWebView's persistent data store, so the login survives app restarts.
//! - The visible "panel" webview is the Svelte UI, shown as a dropdown below
//!   the menu bar icon.
//! - Rust relays calls via `Webview::eval_with_callback` (Tauri >= 2.11),
//!   which resolves the Promise returned by the bridge script.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{
    image::Image,
    menu::{Menu, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

const BRIDGE_URL: &str = "https://catalog.saclibrary.org/MyAccount/Home";
const PANEL_WIDTH: f64 = 380.0;
const PANEL_HEIGHT: f64 = 560.0;

/// Background-refresh cadence bounds. The Settings UI enforces the 10-minute
/// floor; the SACPL_REFRESH_SECS env override (for diagnostics) may go lower.
const MIN_REFRESH_SECS: u64 = 600;
const DEFAULT_REFRESH_SECS: u64 = 3600;

/// Shared between the settings commands and the background timer thread so
/// interval changes apply without restarting the timer.
struct AppState {
    refresh_secs: AtomicU64,
}

const BRIDGE_JS: &str = include_str!("../bridge.js");

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LibStatus {
    /// The bridge page has finished loading (not still a challenge page).
    pub ready: bool,
    pub logged_in: bool,
    /// Server-rendered login error, if the last login attempt failed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login_error: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenewResult {
    pub success: bool,
    pub title: String,
    pub message: String,
    pub renewed: i64,
}

// ---------------------------------------------------------------------------
// Bridge plumbing
// ---------------------------------------------------------------------------

/// Evaluate a SYNCHRONOUS expression in the bridge webview on the main
/// thread and wait (blocking) for the serialized result. Returns `Ok(None)`
/// when nothing meaningful came back.
fn eval_bridge(app: &AppHandle, expr: &str, timeout: Duration) -> Result<Option<String>, String> {
    let (tx, rx) = mpsc::channel::<String>();
    let expr = expr.to_string();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let Some(bridge) = handle.get_webview_window("bridge") else {
            return;
        };
        let _ = bridge.eval_with_callback(&expr, move |json| {
            let _ = tx.send(json);
        });
    })
    .map_err(|e| e.to_string())?;

    match rx.recv_timeout(timeout) {
        Ok(s) => {
            let t = s.trim().to_string();
            if t.is_empty() || t == "undefined" || t == "null" {
                Ok(None)
            } else {
                Ok(Some(t))
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("bridge webview disconnected".into()),
    }
}

/// Fire-and-forget eval (used for sessionStorage bookkeeping etc.).
fn eval_fire(app: &AppHandle, expr: &str) {
    let expr = expr.to_string();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(bridge) = handle.get_webview_window("bridge") {
            let _ = bridge.eval(&expr);
        }
    });
}

/// Parse an eval result that may be double-encoded (JS `String` results are
/// serialized as JSON strings of JSON strings).
fn json_value(s: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    Some(match v {
        serde_json::Value::String(inner) => {
            serde_json::from_str(&inner).unwrap_or(serde_json::Value::Null)
        }
        other => other,
    })
}

/// Read a synchronous expression from the bridge, e.g.
/// `JSON.stringify(window.__store['checkouts'])`.
fn read_store(app: &AppHandle, key: &str) -> Option<serde_json::Value> {
    let expr = format!("JSON.stringify(window.__store ? window.__store['{key}'] : null)");
    let Ok(Some(json)) = eval_bridge(app, &expr, Duration::from_secs(5)) else {
        return None;
    };
    if json.is_empty() || json == "null" {
        return None;
    }
    json_value(&json)
}

/// Call a bridge method and wait for its result in `window.__store`.
///
/// NOTE: wry's `eval_with_callback` does not await Promises on macOS, so we
/// fire the call and poll the store slot the bridge script fills when the
/// promise settles.
///
/// `refire` should be true for idempotent read-only calls (status,
/// checkouts): if the bridge page is mid-reload the first call is lost, so
/// we re-fire periodically until the result lands. Never re-fire
/// state-changing calls (login, renew).
fn bridge_call(
    app: &AppHandle,
    method: &str,
    args: &str,
    timeout_secs: u64,
    refire: bool,
) -> Result<serde_json::Value, String> {
    eval_fire(app, &format!("delete window.__store['{method}']"));
    eval_fire(app, &format!("window.__bridge.{method}({args})"));

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_fire = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(v) = read_store(app, method) {
            return match v.get("ok").and_then(|x| x.as_bool()) {
                Some(true) => Ok(v["data"].clone()),
                Some(false) => Err(v["error"].as_str().unwrap_or("bridge error").to_string()),
                None => Ok(v),
            };
        }
        if refire && last_fire.elapsed() >= Duration::from_secs(10) {
            last_fire = Instant::now();
            eval_fire(app, &format!("window.__bridge.{method}({args})"));
        }
    }
    Err(format!(
        "The catalog did not respond in time. It may still be loading \u{2014} try again."
    ))
}

/// Wait until the bridge page is fully loaded and report login state.
fn wait_status(app: &AppHandle, timeout_secs: u64) -> Result<LibStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        eval_fire(app, "delete window.__store['status']");
        eval_fire(app, "window.__bridge.status()");
        std::thread::sleep(Duration::from_millis(1000));
        if let Some(v) = read_store(app, "status") {
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                let d = &v["data"];
                if d.get("ready").and_then(|x| x.as_bool()) == Some(true) {
                    return Ok(LibStatus {
                        ready: true,
                        logged_in: d["loggedIn"].as_bool().unwrap_or(false),
                        login_error: d["loginError"].as_str().unwrap_or("").to_string(),
                    });
                }
            }
        }
    }
    Ok(LibStatus { ready: false, logged_in: false, login_error: String::new() })
}

/// Escape a string for safe interpolation into JS. JSON string syntax is a
/// valid JS string literal.
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Marker appended to outcome log lines when the payload came from a Debug
/// Events simulation rather than the real catalog.
fn simulated_tag(v: &serde_json::Value) -> &str {
    if v.get("simulated").and_then(|x| x.as_bool()) == Some(true) {
        " — simulated data"
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// Credential persistence (macOS Keychain)
// ---------------------------------------------------------------------------
// Aspen sessions ride on browser-session cookies that WKWebView drops on
// exit, so we store the credentials in the Keychain and re-authenticate
// silently on startup — equivalent to the site's "Keep Me Signed In".

const KEYCHAIN_SERVICE: &str = "SacPL Loans";
const KEYCHAIN_USER: &str = "patron";

fn save_creds(user: &str, pin: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER).map_err(|e| e.to_string())?;
    entry.set_password(&format!("{user}\u{1}{pin}")).map_err(|e| e.to_string())
}

fn load_creds() -> Option<(String, String)> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER).ok()?;
    let raw = entry.get_password().ok()?;
    let (user, pin) = raw.split_once('\u{1}')?;
    Some((user.to_string(), pin.to_string()))
}

fn clear_creds() {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER) {
        let _ = entry.delete_credential();
    }
}

// ---------------------------------------------------------------------------
// Checkout list cache (for instant startup paint + background refresh)
// ---------------------------------------------------------------------------

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cache_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("checkouts-cache.json"))
}

/// Persist the last good checkout payload so the panel can show it instantly
/// on launch while a fresh fetch runs in the background. Skips error payloads
/// and the transient "still syncing with the ILS" empty placeholder (right
/// after login) so a good cache is never overwritten with junk.
fn save_cache(app: &AppHandle, v: &serde_json::Value) {
    if v.get("success").and_then(|x| x.as_bool()) != Some(true) {
        return;
    }
    // Debug Events simulations never pollute the on-disk cache.
    if v.get("simulated").and_then(|x| x.as_bool()) == Some(true) {
        return;
    }
    let empty_sync = v.get("stillSyncing").and_then(|x| x.as_bool()) == Some(true)
        && v.get("count").and_then(|x| x.as_i64()).unwrap_or(0) == 0;
    if empty_sync {
        return;
    }
    let Some(path) = cache_path(app) else {
        return;
    };
    let user = load_creds().map(|(u, _)| u).unwrap_or_default();
    let wrapper = serde_json::json!({ "user": user, "savedAt": unix_now(), "data": v });
    if let Ok(s) = serde_json::to_string(&wrapper) {
        let _ = std::fs::write(path, s);
    }
}

fn clear_cache(app: &AppHandle) {
    if let Some(path) = cache_path(app) {
        let _ = std::fs::remove_file(path);
    }
}

fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("settings.json"))
}

/// Startup refresh interval: SACPL_REFRESH_SECS (diagnostics, >=30s) wins over
/// the value saved from Settings, which defaults to hourly.
fn initial_refresh_secs(app: &AppHandle) -> u64 {
    if let Some(n) = std::env::var("SACPL_REFRESH_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&s| s >= 30)
    {
        return n;
    }
    let saved = settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["refreshSecs"].as_u64());
    saved.map(|s| s.max(MIN_REFRESH_SECS)).unwrap_or(DEFAULT_REFRESH_SECS)
}

// ---------------------------------------------------------------------------
// App event log (live-updating, viewable in Developer Settings)
// ---------------------------------------------------------------------------

const LOG_FILE: &str = "app.log";
/// Keep the file bounded: rotate to the (line-aligned) tail half past this.
const LOG_MAX_BYTES: usize = 512 * 1024;

/// Developer tooling (Developer Settings, debug events, SACPL_DIAG probes)
/// is active in dev mode (`npm run tauri dev` — the Tauri CLI only enables
/// the `custom-protocol` feature for `tauri build`, so is_dev() is a
/// compile-time constant) and in release builds built with the `debug-mode`
/// cargo feature. Default builds fold this to `false`, which lets the
/// compiler eliminate the gated code and keeps the bundle lean.
const DEBUG_ACTIVE: bool = cfg!(feature = "debug-mode") || tauri::is_dev();

fn log_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(LOG_FILE))
}

/// Append a timestamped event to the app log, and broadcast the line to any
/// open viewer via the "log-appended" event. Small single-line appends from
/// any thread; never log credentials or PINs.
fn app_log(app: &AppHandle, category: &str, message: &str) {
    let Some(path) = log_path(app) else {
        return;
    };
    // Bound the file: when it grows past the cap, keep the tail half.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() as usize > LOG_MAX_BYTES {
            if let Ok(data) = std::fs::read(&path) {
                let cut = data.len().saturating_sub(LOG_MAX_BYTES / 2);
                let start = data[cut..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map(|i| cut + i + 1)
                    .unwrap_or(cut);
                let _ = std::fs::write(&path, &data[start..]);
            }
        }
    }
    let line = format!(
        "{} [{}] {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        category,
        message
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    let _ = app.emit("log-appended", line.trim_end());
}

/// The (line-aligned) tail of the app log for the Developer Settings viewer.
#[tauri::command]
fn lib_read_log(app: AppHandle) -> Result<String, String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    let Some(path) = log_path(&app) else {
        return Ok(String::new());
    };
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    if data.len() > LOG_MAX_BYTES / 2 {
        let cut = data.len() - LOG_MAX_BYTES / 2;
        let start = data[cut..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| cut + i + 1)
            .unwrap_or(cut);
        return Ok(String::from_utf8_lossy(&data[start..]).into_owned());
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

// ---------------------------------------------------------------------------
// Session restore (auto re-login when the catalog session expires)
// ---------------------------------------------------------------------------

/// Append a line to $TMPDIR/sacpl-diag.log when SACPL_DIAG is set.
/// No-op unless developer tooling is active (dev mode / debug builds).
fn diag_append(line: &str) {
    if !DEBUG_ACTIVE || std::env::var("SACPL_DIAG").is_err() {
        return;
    }
    let path = std::env::temp_dir().join("sacpl-diag.log");
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// Try to restore the catalog session with the stored Keychain credentials.
/// Ok(()) on success; Err(reason) when there are no credentials, the sign-in
/// was definitively rejected (stale credentials are then dropped so the user
/// gets a normal login screen), or the catalog could not be reached.
///
/// The bridge reloads the page right after a successful login POST, which can
/// wipe `window.__store` before bridge_call's poll sees the result (a fast
/// POST beats the 500ms poll + 150ms reload race). So the authoritative
/// outcomes are sessionStorage — which survives the reload, same as the
/// panel's own sign-in flow — and the reloaded page's login state.
fn try_relogin(app: &AppHandle) -> Result<(), String> {
    let Some((user, pin)) = load_creds() else {
        app_log(app, "session", "re-login attempted, but no credentials are saved");
        return Err("no saved sign-in".into());
    };
    app_log(app, "session", "signing back in with the saved credentials…");
    let args = format!("{},{}", js_str(&user), js_str(&pin));
    eval_fire(app, "sessionStorage.removeItem('__lib_login_result')");
    eval_fire(app, &format!("window.__bridge.doLogin({args})"));

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut rejected = false;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(Some(json)) = eval_bridge(
            app,
            "sessionStorage.getItem('__lib_login_result')",
            Duration::from_secs(8),
        ) {
            if let Some(v) = json_value(&json) {
                match v.get("ok").and_then(|x| x.as_bool()) {
                    Some(false) => {
                        rejected = true;
                        break;
                    }
                    Some(true) => break,
                    None => {}
                }
            }
        }
    }
    if rejected {
        // Definitive rejection (e.g. PIN changed): drop the stale creds.
        clear_creds();
        app_log(app, "session", "saved credentials were rejected — dropped");
        return Err("your saved sign-in was rejected".into());
    }
    // The page reloads on success; confirm against the real page state. The
    // reload can take a few seconds, so a ready-but-logged-out reply right
    // after the POST is usually just the mid-reload old page — keep asking
    // until the login flips the flag or the window closes.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ever_ready = false;
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        if let Ok(s) = wait_status(app, 3) {
            if s.ready {
                ever_ready = true;
                if s.logged_in {
                    app_log(app, "session", "session restored");
                    return Ok(());
                }
            }
        }
        if Instant::now() >= deadline {
            if ever_ready {
                app_log(app, "session", "sign-in did not take effect");
                return Err("the sign-in did not take effect".into());
            }
            app_log(app, "session", "could not reach the catalog while re-logging in");
            return Err("could not reach the catalog".into());
        }
    }
}

/// Fetch the checkout list via the given bridge method. If the catalog
/// reports the session has ended, emit `session-reconnecting`, restore the
/// session with the stored credentials, and fetch again (a fresh login needs
/// a longer budget while the ILS re-syncs). The panel is kept in the loop
/// via `session-reconnected` / `session-expired` so it can show a
/// signing-in state and fall back to the login form when restore fails.
fn fetch_with_relogin(
    app: &AppHandle,
    method: &str,
    already_logged_out: bool,
) -> Result<serde_json::Value, String> {
    let logged_out = |v: &serde_json::Value| {
        v.get("loggedOut").and_then(|x| x.as_bool()) == Some(true)
    };

    // Skip the (doomed) first fetch when the caller already knows the
    // session is gone.
    let first = if already_logged_out {
        Ok(serde_json::json!({ "loggedOut": true }))
    } else {
        bridge_call(app, method, "", 60, true)
    };
    match first {
        Ok(v) if logged_out(&v) => {
            diag_append(&format!("[relogin] session ended detected (method={method})\n"));
            app_log(app, "session", &format!("catalog session ended ({method} fetch) — re-authenticating"));
        }
        other => return other,
    }

    let _ = app.emit("session-reconnecting", true);
    let out = match try_relogin(app) {
        Ok(()) => {
            diag_append("[relogin] stored credentials accepted\n");
            match bridge_call(app, method, "", 90, true) {
                Ok(v) if logged_out(&v) => {
                    // Bounced again right after signing in — give up.
                    let _ = app.emit("session-expired", true);
                    Err("Your library session ended. Please sign in again.".into())
                }
                Ok(v) => {
                    let _ = app.emit("session-reconnected", true);
                    Ok(v)
                }
                // Signed in fine, but the refetch failed — not a session
                // problem; keep the account view and let the error toast.
                Err(e) => {
                    let _ = app.emit("session-reconnected", true);
                    Err(e)
                }
            }
        }
        Err(reason) => {
            diag_append(&format!("[relogin] failed: {reason}\n"));
            let _ = app.emit("session-expired", true);
            Err(format!("Could not sign back in — {reason}. Please sign in again."))
        }
    };
    diag_append(&format!(
        "[relogin] outcome: {}\n",
        out.as_ref().map(|v| v["count"].as_i64().unwrap_or(-1)).map(|n| format!("ok, {n} items")).unwrap_or_else(|e| format!("ERR: {e}"))
    ));
    out
}

// ---------------------------------------------------------------------------
// Tauri commands (called from the Svelte UI)
// ---------------------------------------------------------------------------

#[tauri::command]
async fn lib_status(app: AppHandle) -> Result<LibStatus, String> {
    tauri::async_runtime::spawn_blocking(move || wait_status(&app, 12))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn lib_login(app: AppHandle, username: String, password: String) -> Result<LoginResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Clear any stale result, then start the login. The bridge posts the
        // form and reloads the page (which may kill this promise), so the
        // authoritative result is read back from sessionStorage.
        eval_fire(&app, "sessionStorage.removeItem('__lib_login_result')");
        let args = format!("{},{}", js_str(&username), js_str(&password));
        // On success the bridge reloads the page (wiping __store), so don't
        // wait long here — the authoritative result is polled from
        // sessionStorage below.
        let _ = bridge_call(&app, "doLogin", &args, 12, false);

        // Poll sessionStorage for the result (survives the reload).
        let deadline = Instant::now() + Duration::from_secs(45);
        let result: Option<serde_json::Value> = loop {
            if let Ok(Some(json)) = eval_bridge(
                &app,
                "sessionStorage.getItem('__lib_login_result')",
                Duration::from_secs(8),
            ) {
                if let Some(v) = json_value(&json) {
                    if v.is_object() {
                        break Some(v);
                    }
                }
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(500));
        };

        // Give the reloaded page a moment, then check the real state.
        let status = wait_status(&app, 60).unwrap_or(LibStatus { ready: false, logged_in: false, login_error: String::new() });

        let (ok, message) = match (result, status.logged_in) {
            (_, true) => (true, String::new()),
            // The server's own error message (wrong PIN, expired PIN, 2FA…)
            // comes straight from the AJAX response — surface it verbatim.
            (Some(v), false) if v.get("ok").and_then(|x| x.as_bool()) == Some(false) => {
                (false, v["error"].as_str().unwrap_or("Login failed").to_string())
            }
            (_, false) if !status.ready => (false, "Could not reach the library catalog. Check your network.".into()),
            (_, false) => (false, if status.login_error.is_empty() {
                "Sign in failed. Check your library ID and PIN.".into()
            } else {
                status.login_error.clone()
            }),
        };

        if ok {
            // Persist for the silent re-login on next launch (and for the
            // hourly background refresh).
            let _ = save_creds(&username, &password);
            app_log(&app, "sign-in", "signed in");
        } else {
            app_log(&app, "sign-in", &format!("failed: {message}"));
        }
        Ok(LoginResult { ok, message })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn lib_logout(app: AppHandle) -> Result<LoginResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        eval_fire(&app, "sessionStorage.removeItem('__lib_logout_result')");
        // The logout fetch + reload may wipe the store before we can read it;
        // the side effect is what matters, so keep this timeout short.
        let _ = bridge_call(&app, "logout", "", 6, false);
        // Logout is also local: drop the Keychain credentials and the cached
        // checkout list so the next launch starts signed out.
        clear_creds();
        clear_cache(&app);
        app_log(&app, "sign-in", "signed out — saved credentials and cache cleared");
        // After logout the page reloads logged out; confirm.
        let status = wait_status(&app, 45);
        Ok(match status {
            Ok(s) => LoginResult { ok: !s.logged_in, message: String::new() },
            Err(_) => LoginResult { ok: true, message: String::new() },
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn lib_checkouts(app: AppHandle) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let app2 = app.clone();
    // Session expiry is handled inside: on a logged-out bounce the Rust side
    // re-authenticates from the Keychain before this resolves (the panel
    // shows a signing-in state via events meanwhile).
    let result = tauri::async_runtime::spawn_blocking(move || {
        fetch_with_relogin(&app2, "checkouts", false)
    })
    .await
    .map_err(|e| e.to_string())?;
    if let Ok(v) = &result {
        save_cache(&app, v);
    }
    diag_append(&match &result {
        Ok(v) => format!(
            "[lib_checkouts] {} ms, ok, {} items\n",
            started.elapsed().as_millis(),
            v["count"].as_i64().unwrap_or(-1)
        ),
        Err(e) => format!("[lib_checkouts] {} ms, ERR: {e}\n", started.elapsed().as_millis()),
    });
    match &result {
        Ok(v) => app_log(
            &app,
            "refresh",
            &format!(
                "{} ({:.1}s){}",
                if v.get("success").and_then(|x| x.as_bool()) == Some(true) {
                    format!(
                        "ok, {} items",
                        v["count"].as_i64().unwrap_or(-1)
                    )
                } else {
                    format!(
                        "failed: {}",
                        v["message"].as_str().unwrap_or("catalog returned an error")
                    )
                },
                started.elapsed().as_secs_f64(),
                simulated_tag(v)
            )
        ),
        Err(e) => app_log(&app, "refresh", &format!("failed: {e}")),
    }
    result
}

#[tauri::command]
async fn lib_renew_one(
    app: AppHandle,
    kind: String,
    patron_id: String,
    record_id: String,
    renew_indicator: String,
) -> Result<RenewResult, String> {
    let app2 = app.clone();
    let v: serde_json::Value = tauri::async_runtime::spawn_blocking(move || {
        let args = format!(
            "{},{},{},{}",
            js_str(&kind),
            js_str(&patron_id),
            js_str(&record_id),
            js_str(&renew_indicator)
        );
        bridge_call(&app2, "renewOne", &args, 60, false)
    })
    .await
    .map_err(|e| e.to_string())??;

    let ok = v["success"].as_bool().unwrap_or(false);
    let title = v["title"].as_str().unwrap_or("Item");
    let line = if ok {
        format!("\"{title}\" renewed")
    } else {
        format!(
            "\"{title}\" failed — {}",
            v["message"].as_str().unwrap_or("unknown error")
        )
    };
    app_log(&app, "renew", &line);

    Ok(RenewResult {
        success: v["success"].as_bool().unwrap_or(false),
        title: v["title"].as_str().unwrap_or("").to_string(),
        message: v["message"].as_str().unwrap_or("").to_string(),
        renewed: v["renewed"].as_i64().unwrap_or(0),
    })
}

#[tauri::command]
async fn lib_renew_all(app: AppHandle) -> Result<RenewResult, String> {
    let v: serde_json::Value =
        tauri::async_runtime::spawn_blocking(move || bridge_call(&app, "renewAll", "", 90, false))
            .await
            .map_err(|e| e.to_string())??;

    Ok(RenewResult {
        success: v["success"].as_bool().unwrap_or(false),
        title: v["title"].as_str().unwrap_or("").to_string(),
        message: v["message"].as_str().unwrap_or("").to_string(),
        renewed: v["renewed"].as_i64().unwrap_or(0),
    })
}

#[tauri::command]
fn lib_quit(app: AppHandle) {
    app_log(&app, "app", "quitting");
    app.exit(0);
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct AppSettings {
    refresh_secs: u64,
    /// Whether the hidden bridge browser is currently shown (dev feature).
    bridge_visible: bool,
    /// Whether this build includes the Developer Settings debug features
    /// (hidden browser, app log viewer, debug events) — they are compiled out
    /// of default builds.
    debug_mode: bool,
}

fn bridge_visible(app: &AppHandle) -> bool {
    app.get_webview_window("bridge")
        .and_then(|b| b.is_visible().ok())
        .unwrap_or(false)
}

#[tauri::command]
fn lib_get_settings(app: AppHandle) -> AppSettings {
    let secs = app
        .try_state::<AppState>()
        .map(|s| s.refresh_secs.load(Ordering::Relaxed))
        .unwrap_or(DEFAULT_REFRESH_SECS);
    AppSettings { refresh_secs: secs, bridge_visible: bridge_visible(&app), debug_mode: DEBUG_ACTIVE }
}

/// Adjust the background refresh interval. The floor lives here (not only in
/// the UI) so the catalog is never polled faster than every 10 minutes.
#[tauri::command]
fn lib_set_refresh_secs(app: AppHandle, secs: u64) -> Result<AppSettings, String> {
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
    Ok(AppSettings { refresh_secs: secs, bridge_visible: bridge_visible(&app), debug_mode: DEBUG_ACTIVE })
}

/// Delete the on-disk checkout cache. The panel reloads from the catalog
/// right after this returns (see the Settings view).
#[tauri::command]
fn lib_clear_cache(app: AppHandle) {
    clear_cache(&app);
    app_log(&app, "cache", "saved list cleared");
}

/// Show or hide the hidden bridge browser — the WKWebView that talks to the
/// catalog. Developer feature: the user can inspect exactly what the app
/// sees, with the same session and Cloudflare clearance the bridge holds.
/// If the bridge was navigated off the catalog, showing it returns it there
/// so the app's same-origin calls keep working.
#[tauri::command]
fn lib_show_bridge(app: AppHandle, show: bool) -> Result<bool, String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    let Some(bridge) = app.get_webview_window("bridge") else {
        return Err("The hidden browser is not available.".into());
    };
    if show {
        let url = bridge.url().map(|u| u.to_string()).unwrap_or_default();
        if !url.starts_with("https://catalog.saclibrary.org/") {
            let _ = bridge.eval(&format!("location.href = {}", js_str(BRIDGE_URL)));
        }
        let _ = bridge.show();
        let _ = bridge.set_focus();
        activate_app();
    } else {
        let _ = bridge.hide();
    }
    // Report the new state so the Settings view can auto-detect it.
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
fn lib_reset_bridge(app: AppHandle) -> Result<(), String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    eval_fire(&app, &format!("location.href = {}", js_str(BRIDGE_URL)));
    Ok(())
}

/// Show or hide the separate Debug Events window.
#[tauri::command]
fn lib_show_debug_events(app: AppHandle, show: bool) -> Result<(), String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
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

/// Enter runtime debug mode from the separate Debug Events window and return
/// focus to the main loans panel.
#[tauri::command]
fn lib_enter_debug_mode(app: AppHandle) -> Result<(), String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    let Some(panel) = app.get_webview_window("panel") else {
        return Err("The main loans window is not available.".into());
    };
    let _ = panel.show();
    let _ = panel.set_focus();
    activate_app();
    let _ = app.emit("debug-activate", ());
    Ok(())
}

/// Forward a fake debug loan from the Debug Events window to the main panel.
#[tauri::command]
fn lib_add_debug_loan(app: AppHandle, loan: serde_json::Value) -> Result<(), String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    app.emit("debug-add-loan", loan).map_err(|e| e.to_string())
}

/// Debug: pretend the list was last refreshed at the given time (ms epoch),
/// to exercise the "Updated …" label and the stale-on-open refresh.
#[tauri::command]
fn lib_debug_set_refresh_time(app: AppHandle, at_ms: i64) -> Result<(), String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    app.emit("debug-last-updated", at_ms).map_err(|e| e.to_string())
}

/// Developer Settings ▸ Debug Events: simulate (or, where possible, really
/// trigger) events related to the library catalog, to exercise the app's
/// reactions without waiting for the real thing.
///
/// Simulations arm a one-shot flag in the bridge — the next checkout fetch
/// returns the simulated outcome instead of hitting the network — except the
/// unrecoverable-expiry variant, which plays the session UI events directly
/// (credentials are never touched). Real variants act on the actual catalog
/// through the bridge webview. Every trigger is logged to the app log as
/// SIMULATED or REAL.
#[tauri::command]
async fn lib_debug_event(app: AppHandle, event: String, real: bool) -> Result<String, String> {
    if !DEBUG_ACTIVE {
        return Err("Developer tools are only available in dev mode and debug builds.".into());
    }
    match (event.as_str(), real) {
        // One-shot bridge simulations: consumed by the next checkout fetch.
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
                "Simulated “{event}” armed — it fires on the next refresh (manual or background)."
            ))
        }
        // Pure UI-event simulation: reconnect banner, then the login screen.
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
        // Real variants (through the bridge webview, where possible):
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

/// The last good checkout list from disk, for instant paint at startup.
/// Only surfaced to the same patron whose Keychain credentials are stored
/// (i.e. the account the next silent login would use); cleared on logout.
#[tauri::command]
async fn lib_cached_checkouts(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = cache_path(&app)?;
        let raw = std::fs::read_to_string(path).ok()?;
        let wrapper: serde_json::Value = serde_json::from_str(&raw).ok()?;
        let cached_user = wrapper["user"].as_str().unwrap_or("");
        let (current_user, _) = load_creds()?;
        if cached_user.is_empty() || !current_user.eq_ignore_ascii_case(cached_user) {
            return None;
        }
        let mut data = wrapper["data"].clone();
        if let Some(obj) = data.as_object_mut() {
            obj.insert("cached".into(), serde_json::Value::Bool(true));
            obj.insert("savedAt".into(), wrapper["savedAt"].clone());
        }
        let count = data["count"].as_i64().unwrap_or(-1);
        let age = unix_now().saturating_sub(wrapper["savedAt"].as_u64().unwrap_or(0));
        let age_s = if age < 90 * 60 {
            format!("{} min ago", age / 60)
        } else {
            format!("{:.1} h ago", age as f64 / 3600.0)
        };
        app_log(
            &app,
            "cache",
            &format!("served saved list ({count} items, saved {age_s})")
        );
        Some(data)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Open a catalog page (e.g. a book's record) in the user's default browser.
/// Uses macOS `open` so the URL lands in the browser that already has the
/// patron's catalog session; restricted to the catalog host as a safeguard.
#[tauri::command]
fn lib_open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://catalog.saclibrary.org/") {
        return Err("Not a library catalog link.".into());
    }
    std::process::Command::new("open")
        .arg(&url)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Menu bar window management
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn activate_app() {
    use objc2::class;
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    unsafe {
        let nsapp: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !nsapp.is_null() {
            let _: bool = msg_send![&*nsapp, activateIgnoringOtherApps: true];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_app() {}

fn show_panel(app: &AppHandle, cursor: Option<tauri::PhysicalPosition<f64>>) {
    let Some(panel) = app.get_webview_window("panel") else {
        return;
    };

    // Position the dropdown just below the tray icon (cursor is on it).
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
        let half = size.width as f64 / 2.0;
        let x = (pos.x - half).clamp(8.0, screen_w - size.width as f64 - 8.0);
        // Cursor y is inside the menu bar; drop the panel just under it.
        let y = (pos.y + 6.0 * scale).max(4.0);
        let _ = panel.set_position(tauri::PhysicalPosition::new(x, y));
    }

    let _ = panel.show();
    let _ = panel.set_focus();
    activate_app();
    app_log(app, "panel", "shown");
    // Let the panel refresh if its data went stale while hidden (a hidden
    // webview can be suspended and miss background "checkouts-updated" events).
    let _ = app.emit("panel-shown", true);
}

fn toggle_panel(app: &AppHandle, cursor: Option<tauri::PhysicalPosition<f64>>) {
    if let Some(panel) = app.get_webview_window("panel") {
        if panel.is_visible().unwrap_or(false) {
            let _ = panel.hide();
            return;
        }
    }
    show_panel(app, cursor);
}

/// One tick of the hourly background refresh.
///
/// If the catalog session has lapsed since the last tick (Aspen expires
/// idle sessions server-side), silently re-authenticates with the Keychain
/// credentials before fetching; when that is impossible or rejected the
/// panel is sent a `session-expired` event so it can show the login form.
/// Pushes the parsed list to the panel via the "checkouts-updated" event;
/// fetch failures keep the last good list on screen.
fn refresh_checkouts_bg(app: &AppHandle) {
    let started = Instant::now();
    let status = match wait_status(app, 30) {
        Ok(s) => s,
        Err(_) => return,
    };
    if !status.ready {
        app_log(app, "autocheck", "skipped — the catalog page is not ready yet");
        return;
    }
    // Signed out on purpose (credentials were cleared by Sign Out)? Nothing
    // to restore and nobody to notify — the panel is already on the login
    // screen. Otherwise the session restore runs inside the fetch.
    if !status.logged_in && load_creds().is_none() {
        app_log(app, "autocheck", "skipped — signed out");
        return;
    }
    // Dedicated store key ("checkoutsBg") so this can't interleave with a
    // manual Refresh from the panel — both sides poll window.__store by name.
    match fetch_with_relogin(app, "checkoutsBg", !status.logged_in) {
        Ok(v) if v.get("success").and_then(|x| x.as_bool()) == Some(true) => {
            save_cache(app, &v);
            diag_append(&format!(
                "[bg-refresh] ok, {} items\n",
                v["count"].as_i64().unwrap_or(-1)
            ));
            app_log(
                app,
                "autocheck",
                &format!(
                    "ok, {} items ({:.1}s){}",
                    v["count"].as_i64().unwrap_or(-1),
                    started.elapsed().as_secs_f64(),
                    simulated_tag(&v)
                )
            );
            let _ = app.emit("checkouts-updated", v);
        }
        Ok(v) => app_log(
            app,
            "autocheck",
            &format!(
                "failed: {}",
                v["message"].as_str().unwrap_or("catalog returned an error")
            )
        ),
        Err(e) => app_log(app, "autocheck", &format!("failed: {e}")),
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Run as a menu bar agent (no Dock icon), like a real menubar app.
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

    // 1) Hidden bridge webview (real WebKit -> passes Cloudflare). It can be
    // revealed from Settings -> Developer Settings to inspect what the app
    // sees, so it gets a real window title/size for that case.
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

    // 2) The dropdown panel (created hidden; shown on tray click). Fixed
    //    size — content scrolls internally, never the window.
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

    // 3) Debug Events is a separate native window, created only for builds
    // with developer tooling and shown when debug mode is entered.
    if DEBUG_ACTIVE {
        WebviewWindowBuilder::new(app, "debug-events", WebviewUrl::App("debug-events".into()))
            .title("SacPL Loans — Debug Events")
            .inner_size(420.0, 620.0)
            .min_inner_size(360.0, 420.0)
            .visible(false)
            .build()?;
    }

    // 4) Tray icon — a monochrome open-book template image; macOS tints it to
    // match the menu bar's light/dark styling (artwork: tools/make_icons.swift).
    let icon = Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
        .expect("embedded tray icon");
    let quit = PredefinedMenuItem::quit(app.handle(), Some("Quit SacPL Loans"))?;
    let menu = Menu::with_items(app.handle(), &[&quit])?;

    let _tray = TrayIconBuilder::new()
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

    // 4) Optional startup diagnostic (SACPL_DIAG=…): probes for development
    //    — only when developer tooling is active (dev mode / debug builds).
    if DEBUG_ACTIVE && std::env::var("SACPL_DIAG").is_ok() {
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            let path = std::env::temp_dir().join("sacpl-diag.log");
            let append = |s: &str| {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .and_then(|mut f| std::io::Write::write_all(&mut f, s.as_bytes()));
            };
            let inspect = |label: &str| {
                let (tx, rx) = mpsc::channel::<String>();
                let h = handle.clone();
                let h2 = handle.clone();
                let _ = h.run_on_main_thread(move || {
                    if let Some(b) = h2.get_webview_window("bridge") {
                        let _ = b.eval_with_callback(
                            "(function(){ var g = window.Globals||{}; return JSON.stringify({href: location.href, ready: document.readyState, loggedIn: g.loggedIn, bridge: typeof window.__bridge}); })()",
                            move |json| {
                                let _ = tx.send(json);
                            },
                        );
                    }
                });
                let r = rx.recv_timeout(Duration::from_secs(30));
                append(&format!("[{}] {:?}\n", label, r));
            };
            append("diag-thread-started\n");
            std::thread::sleep(Duration::from_secs(20));
            inspect("t20");
            std::thread::sleep(Duration::from_secs(30));
            inspect("t50");
            let status = wait_status(&handle, 45);
            append(&format!("status={:?}\n", status));

            if std::env::var("SACPL_DIAG").as_deref() == Ok("logout") {
                append("logout-probe starting...\n");
                let _ = bridge_call(&handle, "logout", "", 6, false);
                clear_creds();
                let after = wait_status(&handle, 45);
                append(&format!("post-logout-status={:?}\n", after));
            }

            // SACPL_DIAG=login: attempt a login with credentials from the
            // environment (SACPL_LOGIN_USER / SACPL_LOGIN_PIN), then dump the
            // resulting status and checkouts. Skipped when already signed in.
            if std::env::var("SACPL_DIAG").as_deref() == Ok("login") {
                let already = wait_status(&handle, 30)
                    .map(|s| s.logged_in)
                    .unwrap_or(false);
                if !already {
                    let user = std::env::var("SACPL_LOGIN_USER").unwrap_or_else(|_| "__diag_user__".into());
                    let pin = std::env::var("SACPL_LOGIN_PIN").unwrap_or_else(|_| "__diag_pin__".into());
                    append("login-probe starting...\n");
                    let args = format!("{},{}", js_str(&user), js_str(&pin));
                    let probe = bridge_call(&handle, "doLogin", &args, 12, false);
                    append(&format!("login-probe={:?}\n", probe));
                    let ss = eval_bridge(&handle, "sessionStorage.getItem('__lib_login_result')", Duration::from_secs(8));
                    append(&format!("login-session={:?}\n", ss));
                    if ss.iter()
                        .flatten()
                        .flat_map(|s| json_value(s.as_str()))
                        .any(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true))
                    {
                        match save_creds(&user, &pin) {
                            Ok(()) => append("keychain=SAVED\n"),
                            Err(e) => append(&format!("keychain=FAILED {e}\n")),
                        }
                    }
                }
                let after = wait_status(&handle, 60);
                append(&format!("post-probe-status={:?}\n", after));
                if after.as_ref().map(|s| s.logged_in).unwrap_or(false) {
                    let co = bridge_call(&handle, "checkouts", "", 60, true);
                    append(&format!("checkouts-probe={:?}\n", co));
                }
            }

            // SACPL_DIAG=relogin: end the session the way a server-side
            // expiry would, then verify the refresh path detects it and
            // signs back in from the Keychain. Also exercises the
            // no-credentials path (credentials are restored right after).
            if std::env::var("SACPL_DIAG").as_deref() == Ok("relogin") {
                append("relogin-probe: waiting for bridge...\n");
                let st = wait_status(&handle, 60);
                append(&format!("relogin-probe: status={:?}\n", st));
                let _ = bridge_call(&handle, "logout", "", 6, false);
                std::thread::sleep(Duration::from_secs(5));
                let st2 = wait_status(&handle, 60);
                append(&format!("relogin-probe: post-logout={:?}\n", st2));

                // 1) With credentials stored: fetch must detect the expired
                //    session, re-authenticate, and return fresh items.
                let r1 = fetch_with_relogin(&handle, "checkouts", false);
                append(&format!(
                    "relogin-probe: with-creds={:?}\n",
                    r1.as_ref().map(|v| v["count"].as_i64())
                ));

                // 2) Without credentials: must fail with session-expired.
                if let Some((user, pin)) = load_creds() {
                    let _ = bridge_call(&handle, "logout", "", 6, false);
                    std::thread::sleep(Duration::from_secs(5));
                    clear_creds();
                    let r2 = fetch_with_relogin(&handle, "checkouts", false);
                    append(&format!(
                        "relogin-probe: no-creds err={:?}\n",
                        r2.as_ref().err()
                    ));
                    // Restore credentials and the session so the probe
                    // leaves everything as it found it.
                    let _ = save_creds(&user, &pin);
                    let r3 = fetch_with_relogin(&handle, "checkouts", false);
                    append(&format!(
                        "relogin-probe: after-restore={:?}\n",
                        r3.as_ref().map(|v| v["count"].as_i64())
                    ));
                }
            }

            // SACPL_DIAG=bridge: exercise the developer-browser show/hide
            // path (the same command the Settings button calls) and verify
            // the panel does NOT auto-hide when the browser window takes
            // focus. Both windows appear briefly on screen while this runs.
            if std::env::var("SACPL_DIAG").as_deref() == Ok("bridge") {
                append("bridge-probe: waiting for bridge...\n");
                let _ = wait_status(&handle, 60);
                let toggle = |h: &AppHandle, show: bool| -> bool {
                    let h2 = h.clone();
                    let (tx, rx) = mpsc::channel::<bool>();
                    let _ = h.run_on_main_thread(move || {
                        // Command fns are plain fns; run on the main thread
                        // because the show path activates the app.
                        let _ = tx.send(lib_show_bridge(h2, show).unwrap_or(false));
                    });
                    rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
                };

                // 1) Show and focus the panel first (as if opened from the tray).
                let (tx, rx) = mpsc::channel::<bool>();
                let h3 = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    let vis = h3
                        .get_webview_window("panel")
                        .map(|p| {
                            let _ = p.show();
                            let _ = p.set_focus();
                            activate_app();
                            p.is_visible().unwrap_or(false)
                        })
                        .unwrap_or(false);
                    let _ = tx.send(vis);
                });
                append(&format!(
                    "bridge-probe: panel shown={} (pre-browser)\n",
                    rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
                ));

                // 2) Open the browser window — it steals focus. Without the
                //    panel-stays fix the panel would hide within ~180ms.
                let vis_show = toggle(&handle, true);
                append(&format!("bridge-probe: browser show → is_visible={vis_show}\n"));
                std::thread::sleep(Duration::from_secs(4));

                // 3) The panel must still be visible while the browser is open.
                let (tx2, rx2) = mpsc::channel::<bool>();
                let h4 = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    let vis = h4
                        .get_webview_window("panel")
                        .and_then(|p| p.is_visible().ok())
                        .unwrap_or(false);
                    let _ = tx2.send(vis);
                });
                let panel_still = rx2.recv_timeout(Duration::from_secs(10)).unwrap_or(false);
                append(&format!(
                    "bridge-probe: panel still visible with browser open={panel_still}\n"
                ));

                // 4) Hold briefly (for external observation), then clean up.
                std::thread::sleep(Duration::from_secs(2));
                let vis_hide = toggle(&handle, false);
                append(&format!("bridge-probe: browser hide → is_visible={vis_hide}\n"));
                let h5 = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Some(p) = h5.get_webview_window("panel") {
                        let _ = p.hide();
                    }
                });
                append("bridge-probe: cleanup done\n");
            }

            // SACPL_DIAG=theme: report what the panel webview thinks the
            // color scheme is (to verify prefers-color-scheme tracks the
            // system appearance).
            if std::env::var("SACPL_DIAG").as_deref() == Ok("theme") {
                // Wait until the panel has painted the account view (the
                // cache-served log line) before probing.
                let app_log_ready = {
                    let mut ready = false;
                    for _ in 0..60 {
                        if let Some(path) = log_path(&handle) {
                            if let Ok(data) = std::fs::read_to_string(&path) {
                                if data.contains("[cache]") {
                                    ready = true;
                                    break;
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_millis(2000));
                    }
                    ready
                };
                append(&format!("theme-probe: app log reached cache paint = {}\n", app_log_ready));

                // Helper: evaluate JS in the panel webview on the main thread.
                let panel_eval = |h: &AppHandle, expr: &str| -> Option<String> {
                    let (tx, rx) = mpsc::channel::<String>();
                    let h2 = h.clone();
                    let expr = expr.to_string();
                    let _ = h.run_on_main_thread(move || {
                        if let Some(p) = h2.get_webview_window("panel") {
                            let _ = p.eval_with_callback(&expr, move |json| {
                                let _ = tx.send(json);
                            });
                        } else {
                            let _ = tx.send("no panel".into());
                        }
                    });
                    match rx.recv_timeout(Duration::from_secs(10)) {
                        Ok(s) if s.is_empty() || s == "undefined" || s == "null" => None,
                        Ok(s) => Some(s),
                        Err(_) => None,
                    }
                };
                // Helper: DOM-observable wait — poll a selector in the panel.
                let panel_has = |h: &AppHandle, selector: &str, tries: u32| -> bool {
                    let expr = format!("JSON.stringify({{ found: !!document.querySelector({}) }})", js_str(selector));
                    for _ in 0..tries {
                        if panel_eval(h, &expr)
                            .map(|s| s.contains("\"found\":true"))
                            .unwrap_or(false)
                        {
                            return true;
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    false
                };

                // Theme: the panel webview must track the system appearance.
                let r = panel_eval(
                    &handle,
                    "JSON.stringify({ dark: matchMedia('(prefers-color-scheme: dark)').matches, panelBg: getComputedStyle(document.querySelector('.panel')).backgroundColor })",
                );
                append(&format!("theme-probe: panel = {:?}\n", r));

                // Event registry introspection: does the panel webview have
                // the Tauri internals and any registered event listeners?
                let r = panel_eval(
                    &handle,
                    "JSON.stringify({ hasInternals: !!window.__TAURI_INTERNALS__, hasEventPluginInternals: !!window.__TAURI_EVENT_PLUGIN_INTERNALS__ })",
                );
                append(&format!("event-probe: internals = {:?}\n", r));
                let r = panel_eval(
                    &handle,
                    "(function(){ var out = { hasDispatch: typeof window.__internal_unstable_listeners_function_id__, hasListenersObj: typeof window.__internal_unstable_listeners_object_id__, events: {} }; try { var lo = window.__internal_unstable_listeners_object_id__; if (lo) { var names = Object.getOwnPropertyNames(lo); for (var i = 0; i < names.length; i++) { var k = names[i]; try { out.events[k] = Object.getOwnPropertyNames(lo[k]).length; } catch(e2) { out.events[k] = 'err'; } } out.ownNames = names; } } catch(e) { out.err = String(e); } return JSON.stringify(out); })()",
                );
                append(&format!("event-probe: listener registry = {:?}\n", r));

                // --- Event delivery test (DOM-observable): hidden, then shown. ---
                let _ = handle.emit("session-reconnecting", true);
                let hidden_banner = panel_has(&handle, ".reconnecting", 20);
                append(&format!("event-probe: banner while HIDDEN = {hidden_banner}\n"));
                let _ = handle.emit("session-reconnected", true);
                std::thread::sleep(Duration::from_millis(300));

                let h5 = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Some(p) = h5.get_webview_window("panel") {
                        let _ = p.show();
                    }
                });
                std::thread::sleep(Duration::from_millis(500));
                let _ = handle.emit("session-reconnecting", true);
                let shown_banner = panel_has(&handle, ".reconnecting", 20);
                append(&format!("event-probe: banner while SHOWN = {shown_banner}\n"));
                let _ = handle.emit("session-reconnected", true);

                // --- Scroll geometry: 30 simulated books; only .loans scrolls. ---
                let sim = tauri::async_runtime::block_on(lib_debug_event(
                    handle.clone(),
                    "changedData".to_string(),
                    false,
                ));
                append(&format!("scroll-probe: armed changedData → {sim:?}\n"));
                let out = fetch_with_relogin(&handle, "checkouts", false);
                append(&format!(
                    "scroll-probe: fetched simulated payload = {}\n",
                    out.as_ref()
                        .map(|v| format!("count={:?}", v["count"].as_i64()))
                        .unwrap_or_else(|e| format!("ERR {e}"))
                ));
                let _ = handle.emit(
                    "checkouts-updated",
                    out.unwrap_or(serde_json::json!({ "success": false })),
                );
                let got_books = panel_has(&handle, ".loans li", 30);
                let geo = panel_eval(
                    &handle,
                    "(function(){ var b = document.body; var l = document.querySelector('.loans'); return JSON.stringify({ bodyOverflow: b.scrollHeight - b.clientHeight, loansClientH: l ? l.clientHeight : null, loansScrollH: l ? l.scrollHeight : null, loansScrollable: l ? l.scrollHeight > l.clientHeight : null, books: l ? l.children.length : 0 }); })()",
                );
                append(&format!(
                    "scroll-probe: books appeared = {got_books}; geometry = {:?}\n",
                    geo
                ));

                // Cleanup: hide the panel again.
                let h6 = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let Some(p) = h6.get_webview_window("panel") {
                        let _ = p.hide();
                    }
                });
            }

            // SACPL_DIAG=debug: exercise the Debug Events command — every
            // simulated site event plus the real logout variant (which the
            // app then recovers from via the stored credentials). Verifies
            // the app log marks triggers as REAL vs SIMULATED.
            if std::env::var("SACPL_DIAG").as_deref() == Ok("debug") {
                append("debug-probe: waiting for bridge...\n");
                let _ = wait_status(&handle, 60);
                for sim in ["http403", "syncing", "changedData", "loggedOut"] {
                    let armed = tauri::async_runtime::block_on(lib_debug_event(
                        handle.clone(),
                        sim.to_string(),
                        false,
                    ));
                    append(&format!("debug-probe: arm {sim} → {:?}\n", armed));
                    let out = fetch_with_relogin(&handle, "checkouts", false);
                    append(&format!(
                        "debug-probe: fetch after {sim} = {}\n",
                        out
                            .as_ref()
                            .map(|v| format!(
                                "success={:?} count={:?} msg={:?} sim={:?}",
                                v["success"].as_bool(),
                                v["count"].as_i64(),
                                v["message"].as_str(),
                                v["simulated"].as_bool()
                            ))
                            .unwrap_or_else(|e| format!("ERR {e}"))
                    ));
                }
                // The unrecoverable-expiry simulation (UI events only).
                let r = tauri::async_runtime::block_on(lib_debug_event(
                    handle.clone(),
                    "expiredUnrecoverable".to_string(),
                    false,
                ));
                append(&format!("debug-probe: expiredUnrecoverable → {r:?}\n"));
                std::thread::sleep(Duration::from_secs(4));

                // The real logout variant: end the session for real, then a
                // fetch must detect it and recover from the Keychain.
                let r = tauri::async_runtime::block_on(lib_debug_event(
                    handle.clone(),
                    "loggedOut".to_string(),
                    true,
                ));
                append(&format!("debug-probe: REAL loggedOut → {r:?}\n"));
                std::thread::sleep(Duration::from_secs(6));
                let out = fetch_with_relogin(&handle, "checkouts", false);
                append(&format!(
                    "debug-probe: fetch after REAL logout = {}\n",
                    out.as_ref()
                        .map(|v| format!("count={:?}", v["count"].as_i64()))
                        .unwrap_or_else(|e| format!("ERR {e}"))
                ));
            }
        });
    }

    // 5) Silent re-login on startup: the Aspen session cookie does not
    // survive an app restart, so use the Keychain credentials (if any) to
    // restore the session transparently.
    let handle = app.handle().clone();
    std::thread::spawn(move || {
        let status = match wait_status(&handle, 90) {
            Ok(s) => s,
            Err(_) => return,
        };
        if status.ready && !status.logged_in {
            // Restore with the stored credentials, if any (drops stale ones
            // on a definitive rejection — see try_relogin).
            let _ = try_relogin(&handle);
        }
    });

    // 6) Hourly background refresh of the checkout list. The panel updates
    // from the "checkouts-updated" event even while hidden. The interval is
    // adjustable in Settings (persisted, min 10 min) and overridable via
    // SACPL_REFRESH_SECS (min 30s) for diagnostics. Changes apply without a
    // restart: the timer is driven by the shared atomic, checked every 5s.
    let handle = app.handle().clone();
    let initial_secs = initial_refresh_secs(&handle);
    app.manage(AppState {
        refresh_secs: AtomicU64::new(initial_secs),
    });
    std::thread::spawn(move || {
        let mut last_tick = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let secs = handle
                .state::<AppState>()
                .refresh_secs
                .load(Ordering::Relaxed)
                .max(30);
            if last_tick.elapsed() >= Duration::from_secs(secs) {
                refresh_checkouts_bg(&handle);
                last_tick = Instant::now();
            }
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(setup)
        .on_window_event(|window, event| {
            // The bridge webview is the app's lifeline to the catalog: when
            // the visible developer browser window is closed, just hide it —
            // destroying it would silently break every catalog call.
            if window.label() == "bridge" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    app_log(
                        window.app_handle(),
                        "dev",
                        "hidden browser closed — kept alive, window hidden"
                    );
                    // Auto-detection: the window state changed outside the
                    // Settings button — tell the panel.
                    let _ = window.app_handle().emit("bridge-visibility", false);
                }
            }
            // Keep the Debug Events window alive when its close button is
            // clicked so it can be reopened without rebuilding the webview.
            if window.label() == "debug-events" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            // Menubar behavior: hide the panel when it loses focus — unless
            // a developer window is what took it. The panel stays visible
            // while the hidden browser or Debug Events window is focused;
            // activating any other app still dismisses it.
            if window.label() == "panel" {
                if let WindowEvent::Focused(false) = event {
                    if window.is_visible().unwrap_or(false) {
                        let w = window.clone();
                        let app = window.app_handle().clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(180));
                            // The focus handoff to the browser settles well
                            // within the 180ms delay.
                            let developer_window_focused = ["bridge", "debug-events"].iter().any(|label| {
                                app.get_webview_window(*label)
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
        })
        .invoke_handler(tauri::generate_handler![
            lib_status,
            lib_login,
            lib_logout,
            lib_checkouts,
            lib_renew_one,
            lib_renew_all,
            lib_quit,
            lib_cached_checkouts,
            lib_open_url,
            lib_get_settings,
            lib_set_refresh_secs,
            lib_clear_cache,
            lib_show_bridge,
            lib_reset_bridge,
            lib_read_log,
            lib_show_debug_events,
            lib_enter_debug_mode,
            lib_add_debug_loan,
            lib_debug_set_refresh_time,
            lib_debug_event
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
