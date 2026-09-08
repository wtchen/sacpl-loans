//! Developer diagnostics: SACPL_DIAG startup probes. Each probe value runs
//! a scripted scenario against the real app and appends results to
//! $TMPDIR/sacpl-diag.log. Developer builds only; inert otherwise.

use std::sync::mpsc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::bridge::{bridge_call, eval_bridge, json_value, js_str};
use crate::catalog::wait_status;
use crate::credentials::{clear_creds, load_creds, save_creds};
use crate::debug::{lib_debug_event, lib_show_bridge};
use crate::log::log_path;
use crate::panel::activate_app;
use crate::session::fetch_with_relogin;

/// Spawn the diagnostic thread when SACPL_DIAG is set (developer builds only).
pub(crate) fn maybe_start(handle: AppHandle) {
    if !crate::debug::DEBUG_ACTIVE || std::env::var("SACPL_DIAG").is_err() {
        return;
    }
    std::thread::spawn(move || run(&handle));
}

fn append(path: &std::path::PathBuf, s: &str) {
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, s.as_bytes()));
}

fn run(handle: &AppHandle) {
    let path = std::env::temp_dir().join("sacpl-diag.log");
    let handle = handle.clone();

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
        append(&path, &format!("[{}] {:?}\n", label, r));
    };
    append(&path, "diag-thread-started\n");
    std::thread::sleep(Duration::from_secs(20));
    inspect("t20");
    std::thread::sleep(Duration::from_secs(30));
    inspect("t50");
    let status = wait_status(&handle, 45);
    append(&path, &format!("status={:?}\n", status));

    if std::env::var("SACPL_DIAG").as_deref() == Ok("logout") {
        append(&path, "logout-probe starting...\n");
        let _ = bridge_call(&handle, "logout", "", 6, false);
        clear_creds();
        let after = wait_status(&handle, 45);
        append(&path, &format!("post-logout-status={:?}\n", after));
    }

    // SACPL_DIAG=login: sign in with SACPL_LOGIN_USER/SACPL_LOGIN_PIN, then
    // dump the resulting status and checkouts. Skipped when signed in.
    if std::env::var("SACPL_DIAG").as_deref() == Ok("login") {
        let already = wait_status(&handle, 30)
            .map(|s| s.logged_in)
            .unwrap_or(false);
        if !already {
            let user = std::env::var("SACPL_LOGIN_USER").unwrap_or_else(|_| "__diag_user__".into());
            let pin = std::env::var("SACPL_LOGIN_PIN").unwrap_or_else(|_| "__diag_pin__".into());
            append(&path, "login-probe starting...\n");
            let args = format!("{},{}", js_str(&user), js_str(&pin));
            let probe = bridge_call(&handle, "doLogin", &args, 12, false);
            append(&path, &format!("login-probe={:?}\n", probe));
            let ss = eval_bridge(&handle, "sessionStorage.getItem('__lib_login_result')", Duration::from_secs(8));
            append(&path, &format!("login-session={:?}\n", ss));
            if ss.iter()
                .flatten()
                .flat_map(|s| json_value(s.as_str()))
                .any(|v| v.get("ok").and_then(|x| x.as_bool()) == Some(true))
            {
                match save_creds(&user, &pin) {
                    Ok(()) => append(&path, "keychain=SAVED\n"),
                    Err(e) => append(&path, &format!("keychain=FAILED {e}\n")),
                }
            }
        }
        let after = wait_status(&handle, 60);
        append(&path, &format!("post-probe-status={:?}\n", after));
        if after.as_ref().map(|s| s.logged_in).unwrap_or(false) {
            let co = bridge_call(&handle, "checkouts", "", 60, true);
            append(&path, &format!("checkouts-probe={:?}\n", co));
        }
    }

    // SACPL_DIAG=relogin: log out, then verify a fetch detects the expired
    // session and signs back in from the Keychain, with and without creds.
    if std::env::var("SACPL_DIAG").as_deref() == Ok("relogin") {
        append(&path, "relogin-probe: waiting for bridge...\n");
        let st = wait_status(&handle, 60);
        append(&path, &format!("relogin-probe: status={:?}\n", st));
        let _ = bridge_call(&handle, "logout", "", 6, false);
        std::thread::sleep(Duration::from_secs(5));
        let st2 = wait_status(&handle, 60);
        append(&path, &format!("relogin-probe: post-logout={:?}\n", st2));

        // 1) With credentials stored: fetch must detect the expired
        //    session, re-authenticate, and return fresh items.
        let r1 = fetch_with_relogin(&handle, "checkouts", false);
        append(&path, &format!(
            "relogin-probe: with-creds={:?}\n",
            r1.as_ref().map(|v| v["count"].as_i64())
        ));

        // 2) Without credentials: must fail with session-expired.
        if let Some((user, pin)) = load_creds() {
            let _ = bridge_call(&handle, "logout", "", 6, false);
            std::thread::sleep(Duration::from_secs(5));
            clear_creds();
            let r2 = fetch_with_relogin(&handle, "checkouts", false);
            append(&path, &format!(
                "relogin-probe: no-creds err={:?}\n",
                r2.as_ref().err()
            ));
            // Restore credentials and session so the probe leaves
            // everything as it found it.
            let _ = save_creds(&user, &pin);
            let r3 = fetch_with_relogin(&handle, "checkouts", false);
            append(&path, &format!(
                "relogin-probe: after-restore={:?}\n",
                r3.as_ref().map(|v| v["count"].as_i64())
            ));
        }
    }

    // SACPL_DIAG=bridge: exercise the developer-browser show/hide path and
    // verify the panel does NOT auto-hide when the browser takes focus.
    // Both windows appear briefly on screen while this runs.
    if std::env::var("SACPL_DIAG").as_deref() == Ok("bridge") {
        append(&path, "bridge-probe: waiting for bridge...\n");
        let _ = wait_status(&handle, 60);
        let toggle = |h: &AppHandle, show: bool| -> bool {
            let h2 = h.clone();
            let (tx, rx) = mpsc::channel::<bool>();
            let _ = h.run_on_main_thread(move || {
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
        append(&path, &format!(
            "bridge-probe: panel shown={} (pre-browser)\n",
            rx.recv_timeout(Duration::from_secs(10)).unwrap_or(false)
        ));

        // 2) Open the browser window — it steals focus. Without the
        //    panel-stays fix the panel would hide within ~180ms.
        let vis_show = toggle(&handle, true);        append(&path, &format!("bridge-probe: browser show → is_visible={vis_show}\n"));
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
        append(&path, &format!(
            "bridge-probe: panel still visible with browser open={panel_still}\n"
        ));

        // 4) Hold the windows on screen briefly for manual observation, then
        //    restore the original state.
        std::thread::sleep(Duration::from_secs(2));
        let vis_hide = toggle(&handle, false);
        append(&path, &format!("bridge-probe: browser hide → is_visible={vis_hide}\n"));
        let h5 = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if let Some(p) = h5.get_webview_window("panel") {
                let _ = p.hide();
            }
        });
        append(&path, "bridge-probe: cleanup done\n");
    }

    // SACPL_DIAG=theme: verify the panel tracks the system appearance.
    if std::env::var("SACPL_DIAG").as_deref() == Ok("theme") {
        run_theme_probe(&handle, &path);
    }

    // SACPL_DIAG=debug: exercise every Debug Events variant, plus the real
    // logout, which the app then recovers from via stored credentials.
    if std::env::var("SACPL_DIAG").as_deref() == Ok("debug") {
        append(&path, "debug-probe: waiting for bridge...\n");
        let _ = wait_status(&handle, 60);
        for sim in ["http403", "syncing", "changedData", "loggedOut"] {
            let armed = tauri::async_runtime::block_on(lib_debug_event(
                handle.clone(),
                sim.to_string(),
                false,
            ));
            append(&path, &format!("debug-probe: arm {sim} → {:?}\n", armed));
            let out = fetch_with_relogin(&handle, "checkouts", false);
            append(&path, &format!(
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
        append(&path, &format!("debug-probe: expiredUnrecoverable → {r:?}\n"));
        std::thread::sleep(Duration::from_secs(4));

        // Real logout: the next fetch must detect it and recover
        // from the Keychain.
        let r = tauri::async_runtime::block_on(lib_debug_event(
            handle.clone(),
            "loggedOut".to_string(),
            true,
        ));
        append(&path, &format!("debug-probe: REAL loggedOut → {r:?}\n"));
        std::thread::sleep(Duration::from_secs(6));
        let out = fetch_with_relogin(&handle, "checkouts", false);
        append(&path, &format!(
            "debug-probe: fetch after REAL logout = {}\n",
            out.as_ref()
                .map(|v| format!("count={:?}", v["count"].as_i64()))
                .unwrap_or_else(|e| format!("ERR {e}"))
        ));
    }
}

/// The theme probe: webview appearance plumbing + event delivery + the
/// 30-book scroll geometry, all observed through the panel's DOM.
fn run_theme_probe(handle: &AppHandle, path: &std::path::PathBuf) {
    // Wait until the panel has painted the account view (the
    // cache-served log line) before probing.
    let app_log_ready = {
        let mut ready = false;
        for _ in 0..60 {
            if let Some(p) = log_path(handle) {
                if let Ok(data) = std::fs::read_to_string(&p) {
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
    append(path, &format!("theme-probe: app log reached cache paint = {}\n", app_log_ready));

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
        handle,
        "JSON.stringify({ dark: matchMedia('(prefers-color-scheme: dark)').matches, panelBg: getComputedStyle(document.querySelector('.panel')).backgroundColor })",
    );
    append(path, &format!("theme-probe: panel = {:?}\n", r));

    // Event registry introspection: does the panel webview have
    // the Tauri internals and any registered event listeners?
    let r = panel_eval(
        handle,
        "JSON.stringify({ hasInternals: !!window.__TAURI_INTERNALS__, hasEventPluginInternals: !!window.__TAURI_EVENT_PLUGIN_INTERNALS__ })",
    );
    append(path, &format!("event-probe: internals = {:?}\n", r));
    let r = panel_eval(
        handle,
        "(function(){ var out = { hasDispatch: typeof window.__internal_unstable_listeners_function_id__, hasListenersObj: typeof window.__internal_unstable_listeners_object_id__, events: {} }; try { var lo = window.__internal_unstable_listeners_object_id__; if (lo) { var names = Object.getOwnPropertyNames(lo); for (var i = 0; i < names.length; i++) { var k = names[i]; try { out.events[k] = Object.getOwnPropertyNames(lo[k]).length; } catch(e2) { out.events[k] = 'err'; } } out.ownNames = names; } } catch(e) { out.err = String(e); } return JSON.stringify(out); })()",
    );
    append(path, &format!("event-probe: listener registry = {:?}\n", r));

    // --- Event delivery test (DOM-observable): hidden, then shown. ---
    let _ = handle.emit("session-reconnecting", true);
    let hidden_banner = panel_has(handle, ".reconnecting", 20);
    append(path, &format!("event-probe: banner while HIDDEN = {hidden_banner}\n"));
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
    let shown_banner = panel_has(handle, ".reconnecting", 20);
    append(path, &format!("event-probe: banner while SHOWN = {shown_banner}\n"));
    let _ = handle.emit("session-reconnected", true);

    // --- Scroll geometry: 30 simulated books; only .loans scrolls. ---
    let sim = tauri::async_runtime::block_on(lib_debug_event(
        handle.clone(),
        "changedData".to_string(),
        false,
    ));
    append(path, &format!("scroll-probe: armed changedData → {sim:?}\n"));
    let out = fetch_with_relogin(handle, "checkouts", false);
    append(path, &format!(
        "scroll-probe: fetched simulated payload = {}\n",
        out.as_ref()
            .map(|v| format!("count={:?}", v["count"].as_i64()))
            .unwrap_or_else(|e| format!("ERR {e}"))
    ));
    let _ = handle.emit(
        "checkouts-updated",
        out.unwrap_or(serde_json::json!({ "success": false })),
    );
    let got_books = panel_has(handle, ".loans li", 30);
    let geo = panel_eval(
        handle,
        "(function(){ var b = document.body; var l = document.querySelector('.loans'); return JSON.stringify({ bodyOverflow: b.scrollHeight - b.clientHeight, loansClientH: l ? l.clientHeight : null, loansScrollH: l ? l.scrollHeight : null, loansScrollable: l ? l.scrollHeight > l.clientHeight : null, books: l ? l.children.length : 0 }); })()",
    );
    append(path, &format!(
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