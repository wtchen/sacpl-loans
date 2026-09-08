//! Background refresh of the checkout list: a timer driven by the shared
//! settings atomic, plus one refresh tick (with session restore and panel
//! notification via "checkouts-updated").

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::cache::{resolve_covers, save_cache};
use crate::catalog::{simulated_tag, wait_status};
use crate::credentials::load_creds;
use crate::log::{app_log, diag_append};
use crate::session::fetch_with_relogin;
use crate::settings::AppState;

/// Refresh the checkout list. If the session lapsed since the last tick
/// (Aspen expires idle sessions), silently re-authenticates first; when
/// that's impossible the panel is sent `session-expired` so it can show the
/// login form. Fetch failures keep the last good list on screen.
pub(crate) fn refresh_checkouts_bg(app: &AppHandle) {
    let started = Instant::now();
    let status = match wait_status(app, 30) {
        Ok(s) => s,
        Err(_) => return,
    };
    if !status.ready {
        app_log(app, "autocheck", "skipped — the catalog page is not ready yet");
        return;
    }
    // Deliberately signed out (Sign Out cleared the credentials): nothing to
    // restore, and the panel is already on the login screen.
    if !status.logged_in && load_creds().is_none() {
        app_log(app, "autocheck", "skipped — signed out");
        return;
    }
    // Dedicated store key so this can't interleave with a manual Refresh
    // from the panel (both poll window.__store by name).
    match fetch_with_relogin(app, "checkoutsBg", !status.logged_in) {
        Ok(mut v) if v.get("success").and_then(|x| x.as_bool()) == Some(true) => {
            resolve_covers(app, &mut v);
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

/// Spawn the refresh loop; the interval is re-read every 5s so Settings
/// changes apply without a restart.
pub(crate) fn start_loop(handle: AppHandle) {
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
}