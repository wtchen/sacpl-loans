//! Sign-in, sign-out, and silent re-authentication, plus the "fetch, and
//! when the catalog reports a dead session, restore it and retry" flow.

use std::time::{Duration, Instant};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::bridge::{bridge_call, eval_bridge, eval_fire, json_value, js_str};
use crate::catalog::{wait_status, LibStatus};
use crate::credentials::{clear_creds, load_creds, save_creds};
use crate::log::{app_log, diag_append};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LoginResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

/// Re-authenticate with the stored Keychain credentials. Err(reason) when
/// there are no credentials, the sign-in was rejected (stale credentials are
/// dropped so the user gets a normal login screen), or the catalog was
/// unreachable.
///
/// The page reloads right after a successful login POST, which can wipe
/// `window.__store` before the poll sees the result — so the authoritative
/// outcome is sessionStorage, which survives the reload, plus the reloaded
/// page's login state.
pub(crate) fn try_relogin(app: &AppHandle) -> Result<(), String> {
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
        // PIN changed or similar: drop the stale creds.
        clear_creds();
        app_log(app, "session", "saved credentials were rejected — dropped");
        return Err("your saved sign-in was rejected".into());
    }
    // The reload can take a few seconds, so a ready-but-logged-out reply
    // right after the POST is usually just the mid-reload old page — keep
    // asking until the login flips the flag or the deadline passes.
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

/// Payloads that must be retried after a re-login: the catalog reporting a
/// dead session, or Cloudflare blocking the request with a 403 (a fresh
/// sign-in re-establishes the clearance in the bridge webview).
fn needs_relogin(v: &serde_json::Value) -> bool {
    v.get("loggedOut").and_then(|x| x.as_bool()) == Some(true)
}

/// Fetch via the given bridge method; when the fetch reports a dead session
/// or a 403, restore the session from the Keychain and refetch. The panel is
/// kept in the loop via `session-reconnecting` / `session-reconnected` /
/// `session-expired` events so it can show a signing-in state and fall back
/// to the login form when restore fails.
pub(crate) fn fetch_with_relogin(
    app: &AppHandle,
    method: &str,
    already_logged_out: bool,
) -> Result<serde_json::Value, String> {
    // Skip the doomed first fetch when the session is already gone.
    let first = if already_logged_out {
        Ok(serde_json::json!({ "loggedOut": true }))
    } else {
        bridge_call(app, method, "", 60, true)
    };
    match first {
        Ok(v) if needs_relogin(&v) => {
            let why = v["message"].as_str().unwrap_or("the session ended");
            diag_append(&format!("[relogin] re-login needed (method={method}): {why}\n"));
            app_log(app, "session", &format!("catalog blocked the fetch ({why}) — signing in again"));
        }
        other => return other,
    }

    let _ = app.emit("session-reconnecting", true);
    let out = match try_relogin(app) {
        Ok(()) => {
            diag_append("[relogin] stored credentials accepted\n");
            match bridge_call(app, method, "", 90, true) {
                Ok(v) if needs_relogin(&v) => {
                    // Bounced again right after signing in — give up.
                    let _ = app.emit("session-expired", true);
                    Err("Your library session ended. Please sign in again.".into())
                }
                Ok(v) => {
                    let _ = app.emit("session-reconnected", true);
                    Ok(v)
                }
                Err(e) => {
                    // Not a session problem — keep the account view, let the error toast.
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

// Tauri commands (called from the Svelte UI)

#[tauri::command]
pub(crate) async fn lib_login(app: AppHandle, username: String, password: String) -> Result<LoginResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // The login POST reloads the page (wiping __store and possibly this
        // promise), so the authoritative result is polled from sessionStorage.
        eval_fire(&app, "sessionStorage.removeItem('__lib_login_result')");
        let args = format!("{},{}", js_str(&username), js_str(&password));
        let _ = bridge_call(&app, "doLogin", &args, 12, false);

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
        let status = wait_status(&app, 60).unwrap_or_else(|_| LibStatus::unreachable());
        let (ok, message) = login_outcome(result.as_ref(), &status);

        if ok {
            let _ = save_creds(&username, &password); // for silent re-login later
            app_log(&app, "sign-in", "signed in");
        } else {
            app_log(&app, "sign-in", &format!("failed: {message}"));
        }
        Ok(LoginResult { ok, message })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Combine the sessionStorage result and the reloaded page's status into the
/// overall outcome; server error messages are surfaced verbatim.
fn login_outcome(result: Option<&serde_json::Value>, status: &LibStatus) -> (bool, String) {
    match (result, status.logged_in) {
        (_, true) => (true, String::new()),
        (Some(v), false) if v.get("ok").and_then(|x| x.as_bool()) == Some(false) => {
            (false, v["error"].as_str().unwrap_or("Login failed").to_string())
        }
        (_, false) if !status.ready => (false, "Could not reach the library catalog. Check your network.".into()),
        (_, false) => (false, if status.login_error.is_empty() {
            "Sign in failed. Check your library ID and PIN.".into()
        } else {
            status.login_error.clone()
        }),
    }
}

#[tauri::command]
pub(crate) async fn lib_logout(app: AppHandle) -> Result<LoginResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        eval_fire(&app, "sessionStorage.removeItem('__lib_logout_result')");
        // The logout happens server-side even if the reload wipes the store
        // before the response is readable — so we don't wait long for it.
        let _ = bridge_call(&app, "logout", "", 6, false);
        clear_creds();
        crate::cache::clear_cache(&app);
        app_log(&app, "sign-in", "signed out — saved credentials and cache cleared");
        // The page reloads signed out after a successful logout; wait for
        // that state so `ok: true` means the session is really gone.
        let status = wait_status(&app, 45);
        Ok(match status {
            Ok(s) => LoginResult { ok: !s.logged_in, message: String::new() },
            Err(_) => LoginResult { ok: true, message: String::new() },
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_session_and_403_payloads_need_relogin() {
        assert!(needs_relogin(&serde_json::json!({ "loggedOut": true })));
        assert!(needs_relogin(&serde_json::json!({
            "success": false, "loggedOut": true, "message": "Catalog returned HTTP 403"
        })));
        assert!(!needs_relogin(&serde_json::json!({ "success": true, "count": 3 })));
        assert!(!needs_relogin(&serde_json::json!({ "success": false, "message": "Catalog returned HTTP 500" })));
    }

    fn status(ready: bool, logged_in: bool, login_error: &str) -> LibStatus {
        LibStatus { ready, logged_in, login_error: login_error.into() }
    }

    #[test]
    fn signed_in_page_wins_over_a_stale_bridge_result() {
        let result = serde_json::json!({ "ok": false, "error": "nope" });
        let (ok, msg) = login_outcome(Some(&result), &status(true, true, ""));
        assert!(ok);
        assert_eq!(msg, "");
    }

    #[test]
    fn server_rejection_surfaces_the_catalog_message() {
        let result = serde_json::json!({ "ok": false, "error": "Invalid PIN" });
        let (ok, msg) = login_outcome(Some(&result), &status(true, false, ""));
        assert!(!ok);
        assert_eq!(msg, "Invalid PIN");
    }

    #[test]
    fn unreachable_page_reports_a_network_problem() {
        let (ok, msg) = login_outcome(None, &status(false, false, ""));
        assert!(!ok);
        assert!(msg.contains("Could not reach"));
    }

    #[test]
    fn server_rendered_login_error_is_passed_through() {
        let (ok, msg) = login_outcome(None, &status(true, false, "Expired PIN"));
        assert!(!ok);
        assert_eq!(msg, "Expired PIN");
    }

    #[test]
    fn generic_failure_without_details_gets_the_fallback_message() {
        let (ok, msg) = login_outcome(Some(&serde_json::json!({})), &status(true, false, ""));
        assert!(!ok);
        assert!(msg.contains("Sign in failed"));
    }
}