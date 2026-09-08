//! The catalog entity: the Aspen checkout system as seen by the app. Wraps
//! the bridge transport with its operations (status, checkouts, renewals).

use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;
use std::time::{Duration, Instant};
use serde::Serialize;
use tauri::AppHandle;

use crate::bridge::{bridge_call, js_str, CATALOG_ORIGIN};
use crate::cache::{resolve_covers, save_cache};
use crate::log::{app_log, diag_append};
use crate::session::fetch_with_relogin;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibStatus {
    /// The page has loaded past any Cloudflare challenge.
    pub ready: bool,
    pub logged_in: bool,
    /// Server-rendered login error from the last failed login attempt.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login_error: String,
}

impl LibStatus {
    pub(crate) fn unreachable() -> Self {
        LibStatus { ready: false, logged_in: false, login_error: String::new() }
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RenewResult {
    pub success: bool,
    pub title: String,
    pub message: String,
    pub renewed: i64,
}

impl RenewResult {
    fn from_value(v: &serde_json::Value) -> Self {
        RenewResult {
            success: v["success"].as_bool().unwrap_or(false),
            title: v["title"].as_str().unwrap_or("").to_string(),
            message: v["message"].as_str().unwrap_or("").to_string(),
            renewed: v["renewed"].as_i64().unwrap_or(0),
        }
    }
}

/// Log suffix when a payload came from a Debug Events simulation.
pub(crate) fn simulated_tag(v: &serde_json::Value) -> &str {
    if v.get("simulated").and_then(|x| x.as_bool()) == Some(true) {
        " — simulated data"
    } else {
        ""
    }
}

/// Poll until the bridge page is loaded, then report login state.
pub(crate) fn wait_status(app: &AppHandle, timeout_secs: u64) -> Result<LibStatus, String> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        crate::bridge::eval_fire(app, "delete window.__store['status']");
        crate::bridge::eval_fire(app, "window.__bridge.status()");
        std::thread::sleep(Duration::from_millis(1000));
        if let Some(v) = crate::bridge::read_store(app, "status") {
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
    Ok(LibStatus::unreachable())
}

// Tauri commands (called from the Svelte UI)

#[tauri::command]
pub(crate) async fn lib_status(app: AppHandle) -> Result<LibStatus, String> {
    tauri::async_runtime::spawn_blocking(move || wait_status(&app, 12))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn lib_checkouts(app: AppHandle) -> Result<serde_json::Value, String> {
    let started = Instant::now();
    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut result = fetch_with_relogin(&app2, "checkouts", false);
        // Persist covers and point items at the local files before caching.
        if let Ok(v) = &mut result {
            resolve_covers(&app2, v);
            save_cache(&app2, v);
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?;
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

/// The catalog side of a renewal, managed as Tauri state so the whole flow
/// lives in `lib_renew_one` while tests can manage a mock instead. The real
/// impl talks to the bridge webview and the app log.
type BridgeCall = Arc<dyn Fn(&str) -> Result<serde_json::Value, String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct RenewDeps {
    call: BridgeCall,
    log: Arc<dyn Fn(&str) + Send + Sync>,
}

impl RenewDeps {
    pub(crate) fn real(app: AppHandle) -> Self {
        let log_app = app.clone();
        RenewDeps {
            call: Arc::new(move |args| bridge_call(&app, "renewOne", args, 60, false)),
            log: Arc::new(move |line| app_log(&log_app, "renew", line)),
        }
    }

    /// The positional, JSON-escaped `renewOne` argument list.
    #[cfg(test)]
    fn test_mock(
        payload: serde_json::Value,
        calls: Arc<Mutex<Vec<String>>>,
        lines: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        RenewDeps {
            call: Arc::new(move |args| {
                calls.lock().unwrap().push(args.to_string());
                Ok(payload.clone())
            }),
            log: Arc::new(move |line| lines.lock().unwrap().push(line.to_string())),
        }
    }
}

/// Renew one item: compose the escaped argument list, call the catalog,
/// map the payload onto the entity, and log the outcome — successes,
/// catalog-reported failures, and bridge/transport errors all land in the
/// app log. All logic lives here; the bridge/log sides come from managed
/// state (`RenewDeps`) so tests can substitute mocks.
#[tauri::command]
pub(crate) async fn lib_renew_one(
    deps: tauri::State<'_, RenewDeps>,
    kind: String,
    patron_id: String,
    record_id: String,
    renew_indicator: String,
) -> Result<RenewResult, String> {
    let deps = deps.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let args = format!(
            "{},{},{},{}",
            js_str(&kind),
            js_str(&patron_id),
            js_str(&record_id),
            js_str(&renew_indicator)
        );
        let v = match (deps.call)(&args) {
            Ok(v) => v,
            Err(e) => {
                (deps.log)(&format!("failed: {e}"));
                return Err(e);
            }
        };
        let r = RenewResult::from_value(&v);
        (deps.log)(&if r.success {
            format!("\"{}\" renewed", r.title)
        } else {
            format!(
                "\"{}\" failed — {}",
                r.title,
                if r.message.is_empty() { "unknown error".to_string() } else { r.message.clone() }
            )
        });
        Ok(r)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(crate) async fn lib_renew_all(app: AppHandle) -> Result<RenewResult, String> {
    let v: serde_json::Value =
        tauri::async_runtime::spawn_blocking(move || bridge_call(&app, "renewAll", "", 90, false))
            .await
            .map_err(|e| e.to_string())??;

    Ok(RenewResult::from_value(&v))
}

/// Open a catalog page in the default browser, which holds the patron's
/// catalog session. Catalog-host links only.
#[tauri::command]
pub(crate) fn lib_open_url(url: String) -> Result<(), String> {
    if !url.starts_with(CATALOG_ORIGIN) {
        return Err("Not a library catalog link.".into());
    }
    std::process::Command::new("open")
        .arg(&url)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tauri::Manager;

    #[test]
    fn renew_result_maps_the_bridge_payload() {
        let v = serde_json::json!({
            "success": true, "title": "The Map", "message": "Renewed.", "renewed": 2
        });
        let r = RenewResult::from_value(&v);
        assert!(r.success);
        assert_eq!(r.title, "The Map");
        assert_eq!(r.message, "Renewed.");
        assert_eq!(r.renewed, 2);
    }

    #[test]
    fn renew_result_defaults_missing_fields() {
        let r = RenewResult::from_value(&serde_json::json!({}));
        assert!(!r.success);
        assert_eq!(r.title, "");
        assert_eq!(r.message, "");
        assert_eq!(r.renewed, 0);
    }

    #[test]
    fn simulated_tag_flags_simulated_payloads_only() {
        assert_eq!(simulated_tag(&serde_json::json!({ "simulated": true })), " — simulated data");
        assert_eq!(simulated_tag(&serde_json::json!({})), "");
        assert_eq!(simulated_tag(&serde_json::json!({ "simulated": false })), "");
    }

    #[test]
    fn lib_renew_one_with_a_mocked_bridge() {
        use tauri::Manager;

        let calls = Arc::new(Mutex::new(Vec::new()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let deps = RenewDeps::test_mock(
            serde_json::json!({
                "success": true, "title": "The Map", "message": "Renewed.", "renewed": 2
            }),
            calls.clone(),
            lines.clone(),
        );
        let app = tauri::test::mock_app();
        app.manage(deps);

        let r = tauri::async_runtime::block_on(lib_renew_one(
            app.state::<RenewDeps>(),
            "ils".into(),
            "p1".into(),
            "r2".into(),
            "debug-renewable".into(),
        ))
        .expect("renewal succeeds");

        // The entity mapped from the payload.
        assert!(r.success);
        assert_eq!(r.title, "The Map");
        assert_eq!(r.renewed, 2);
        // Exactly one bridge call with the escaped arguments.
        assert_eq!(calls.lock().unwrap().as_slice(), ["\"ils\",\"p1\",\"r2\",\"debug-renewable\""]);
        // The outcome was logged.
        assert_eq!(lines.lock().unwrap().as_slice(), ["\"The Map\" renewed"]);
    }

    #[test]
    fn lib_renew_one_propagates_bridge_errors_and_logs_them() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = lines.clone();
        let deps = RenewDeps {
            call: Arc::new(|_| Err("bridge error".to_string())),
            log: Arc::new(move |line| captured.lock().unwrap().push(line.to_string())),
        };
        let app = tauri::test::mock_app();
        app.manage(deps);

        let err = tauri::async_runtime::block_on(lib_renew_one(
            app.state::<RenewDeps>(),
            "ils".into(),
            "p".into(),
            "r".into(),
            "".into(),
        ))
        .expect_err("error propagates");
        assert_eq!(err, "bridge error");
        assert_eq!(lines.lock().unwrap().as_slice(), ["failed: bridge error"]);
    }

    #[test]
    fn lib_renew_one_maps_a_failed_payload_and_logs_the_message() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let deps = RenewDeps::test_mock(
            serde_json::json!({
                "success": false, "title": "Libby Book", "message": "No renewals left", "renewed": 0
            }),
            Arc::new(Mutex::new(Vec::new())),
            lines.clone(),
        );
        let app = tauri::test::mock_app();
        app.manage(deps);

        let r = tauri::async_runtime::block_on(lib_renew_one(
            app.state::<RenewDeps>(),
            "overdrive".into(),
            "p".into(),
            "r".into(),
            "".into(),
        ))
        .expect("payload mapped");
        assert!(!r.success);
        assert_eq!(r.message, "No renewals left");
        assert_eq!(lines.lock().unwrap().as_slice(), ["\"Libby Book\" failed — No renewals left"]);
    }

    #[test]
    fn lib_renew_one_escapes_arguments() {        let calls = Arc::new(Mutex::new(Vec::new()));
        let deps = RenewDeps::test_mock(
            serde_json::json!({}),
            calls.clone(),
            Arc::new(Mutex::new(Vec::new())),
        );
        let app = tauri::test::mock_app();
        app.manage(deps);

        tauri::async_runtime::block_on(lib_renew_one(
            app.state::<RenewDeps>(),
            "ils".into(),
            "p\"1".into(),
            "r2".into(),
            "".into(),
        ))
        .expect("payload mapped");
        assert_eq!(calls.lock().unwrap().as_slice(), ["\"ils\",\"p\\\"1\",\"r2\",\"\""]);
    }

    /// The real IPC dispatch: build the request exactly as the panel's
    /// `invoke("lib_renew_one", { kind, patronId, recordId, renewIndicator })`
    /// does — the named args and nothing else — and verify the command runs
    /// through the full generate_handler path with `deps` injected from
    /// managed state, not from the payload.
    #[test]
    fn lib_renew_one_ipc_dispatch_from_the_frontend() {
        use tauri::ipc::{CallbackFn, InvokeBody};
        use tauri::test::{assert_ipc_response, mock_builder, mock_context, noop_assets};

        let calls = Arc::new(Mutex::new(Vec::new()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let deps = RenewDeps::test_mock(
            serde_json::json!({
                "success": true, "title": "The Map", "message": "Renewed.", "renewed": 2
            }),
            calls.clone(),
            lines.clone(),
        );
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![lib_renew_one])
            .build(mock_context(noop_assets()))
            .expect("mock app");
        app.manage(deps);
        let webview = tauri::webview::WebviewWindowBuilder::new(
            &app,
            "main",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .build()
        .expect("mock webview");

        assert_ipc_response(
            &webview,
            tauri::webview::InvokeRequest {
                cmd: "lib_renew_one".into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::Json(serde_json::json!({
                    "kind": "ils",
                    "patronId": "p1",
                    "recordId": "r2",
                    "renewIndicator": "debug-renewable",
                })),
                headers: Default::default(),
                invoke_key: tauri::test::INVOKE_KEY.to_string(),
            },
            Ok(serde_json::json!({
                "success": true, "title": "The Map", "message": "Renewed.", "renewed": 2
            })),
        );
        // The bridge mock saw the escaped args; the outcome was logged.
        assert_eq!(calls.lock().unwrap().as_slice(), ["\"ils\",\"p1\",\"r2\",\"debug-renewable\""]);
        assert_eq!(lines.lock().unwrap().as_slice(), ["\"The Map\" renewed"]);
    }
}