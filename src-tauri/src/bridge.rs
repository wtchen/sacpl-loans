//! Transport to the hidden "bridge" webview: eval plumbing, the
//! `window.__store` polling convention, and `window.__bridge.*` calls.
//! No app policy lives here.

use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

/// The catalog origin. `lib_open_url` only allows links on this host.
pub(crate) const CATALOG_ORIGIN: &str = "https://catalog.saclibrary.org/";

pub(crate) const BRIDGE_URL: &str = "https://catalog.saclibrary.org/MyAccount/Home";

pub(crate) const BRIDGE_JS: &str = include_str!("../bridge.js");

/// Evaluate a synchronous expression in the bridge webview on the main
/// thread and wait for the serialized result. `Ok(None)` when nothing
/// meaningful came back (timeout, empty, `undefined`/`null`).
pub(crate) fn eval_bridge(app: &AppHandle, expr: &str, timeout: Duration) -> Result<Option<String>, String> {
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

/// Fire-and-forget eval (sessionStorage bookkeeping etc.).
pub(crate) fn eval_fire(app: &AppHandle, expr: &str) {
    let expr = expr.to_string();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(bridge) = handle.get_webview_window("bridge") {
            let _ = bridge.eval(&expr);
        }
    });
}

/// Unwrap an eval result that may be double-encoded (JS `String` results are
/// serialized as JSON strings of JSON strings).
pub(crate) fn json_value(s: &str) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    Some(match v {
        serde_json::Value::String(inner) => {
            serde_json::from_str(&inner).unwrap_or(serde_json::Value::Null)
        }
        other => other,
    })
}

/// Read a `window.__store[key]` value from the bridge.
pub(crate) fn read_store(app: &AppHandle, key: &str) -> Option<serde_json::Value> {
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
/// Same as [`bridge_call_in`] with the store key equal to the method name.
pub(crate) fn bridge_call(
    app: &AppHandle,
    method: &str,
    args: &str,
    timeout_secs: u64,
    refire: bool,
) -> Result<serde_json::Value, String> {
    bridge_call_in(app, method, method, args, timeout_secs, refire)
}

/// [`bridge_call`] variant with an explicit store key, so concurrent calls
/// to the same method (e.g. cover downloads) don't overwrite each other.
pub(crate) fn bridge_call_in(
    app: &AppHandle,
    method: &str,
    store_key: &str,
    args: &str,
    timeout_secs: u64,
    refire: bool,
) -> Result<serde_json::Value, String> {
    eval_fire(app, &format!("delete window.__store['{store_key}']"));
    eval_fire(app, &format!("window.__bridge.{method}({args})"));

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut last_fire = Instant::now();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(v) = read_store(app, store_key) {
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
    Err("The catalog did not respond in time. It may still be loading \u{2014} try again.".to_string())
}

/// Escape a string for safe interpolation into JS (JSON string syntax is a
/// valid JS string literal).
pub(crate) fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_value_unwraps_double_encoded_strings() {
        let double = r#""{\"success\":true,\"count\":3}""#;
        let v = json_value(double).expect("parses");
        assert_eq!(v["success"], serde_json::Value::Bool(true));
        assert_eq!(v["count"], serde_json::Value::from(3));
    }

    #[test]
    fn json_value_passes_through_plain_json() {
        let v = json_value("{\"a\":1}").expect("parses");
        assert_eq!(v["a"], serde_json::Value::from(1));
    }

    #[test]
    fn json_value_single_encoded_string_stays_a_string() {
        let v = json_value(r#""hello""#).expect("parses");
        assert_eq!(v, serde_json::Value::Null);
    }

    #[test]
    fn json_value_rejects_garbage() {
        assert!(json_value("not json").is_none());
    }

    #[test]
    fn js_str_escapes_quotes_and_backslashes() {
        assert_eq!(js_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn js_str_is_a_valid_json_string() {
        let raw = "He said \"hi\" \u{2014} line\nbreak";
        let v: serde_json::Value = serde_json::from_str(&js_str(raw)).expect("round-trips");
        assert_eq!(v.as_str().unwrap(), raw);
    }
}