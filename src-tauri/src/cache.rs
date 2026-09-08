//! Checkout list cache: persists the last good payload for an instant paint
//! at startup while a fresh fetch runs in the background.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use crate::credentials::load_creds;
use crate::log::app_log;

pub(crate) fn unix_now() -> u64 {
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

/// Reject error payloads, Debug Events simulations, and the transient
/// "still syncing with the ILS" empty placeholder, so a good cache is never
/// overwritten with junk.
fn cache_worthy(v: &serde_json::Value) -> bool {
    if v.get("success").and_then(|x| x.as_bool()) != Some(true) {
        return false;
    }
    if v.get("simulated").and_then(|x| x.as_bool()) == Some(true) {
        return false;
    }
    let empty_sync = v.get("stillSyncing").and_then(|x| x.as_bool()) == Some(true)
        && v.get("count").and_then(|x| x.as_i64()).unwrap_or(0) == 0;
    !empty_sync
}

pub(crate) fn save_cache(app: &AppHandle, v: &serde_json::Value) {
    if !cache_worthy(v) {
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

pub(crate) fn clear_cache(app: &AppHandle) {
    if let Some(path) = cache_path(app) {
        let _ = std::fs::remove_file(path);
    }
}

/// "37 min ago" / "1.5 h ago" for the served-from-cache log line.
fn format_age(age_secs: u64) -> String {
    if age_secs < 90 * 60 {
        format!("{} min ago", age_secs / 60)
    } else {
        format!("{:.1} h ago", age_secs as f64 / 3600.0)
    }
}

/// Serve the last good checkout list from disk, but only to the patron whose
/// credentials are stored (the account the next silent login would use).
#[tauri::command]
pub(crate) async fn lib_cached_checkouts(app: AppHandle) -> Result<Option<serde_json::Value>, String> {
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
        app_log(
            &app,
            "cache",
            &format!("served saved list ({count} items, saved {})", format_age(age))
        );
        Some(data)
    })
    .await
    .map_err(|e| e.to_string())
}

/// Delete the cached list; the panel reloads from the catalog right after.
#[tauri::command]
pub(crate) fn lib_clear_cache(app: AppHandle) {
    clear_cache(&app);
    app_log(&app, "cache", "saved list cleared");
}

/// Open the app data folder (the cache location) in Finder, for developers
/// to inspect the saved list, settings file, and app log.
#[tauri::command]
pub(crate) fn lib_show_cache_dir(app: AppHandle) -> Result<String, String> {
    crate::debug::dev_guard()?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("open")
        .arg(&dir)
        .status()
        .map_err(|e| e.to_string())?;
    app_log(&app, "dev", "opened cache folder");
    Ok(dir.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Cover images (downloaded through the bridge so the cache is offline-ready)
// ---------------------------------------------------------------------------

fn covers_dir(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("covers"))
}

/// Deterministic hash of a cover URL — the local file name's stem.
fn hash_hex(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Cover file name: hash of the URL, extension from the mime type.
fn cover_file_name(url: &str, mime: &str) -> String {
    let ext = match mime {
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "jpg",
    };
    format!("{}.{}", hash_hex(url), ext)
}

/// Distinct remote cover URLs in the payload's `items`; empty and
/// already-local covers are left alone.
fn collect_cover_urls(payload: &serde_json::Value) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let Some(items) = payload.get("items").and_then(|i| i.as_array()) else { return urls };
    for item in items {
        if let Some(cover) = item.get("cover").and_then(|c| c.as_str()) {
            if cover.starts_with("http") && !urls.iter().any(|u| u == cover) {
                urls.push(cover.to_string());
            }
        }
    }
    urls
}

/// Point each item's `cover` at its downloaded file.
fn apply_cover_paths(payload: &mut serde_json::Value, local: &HashMap<String, String>) {
    let Some(items) = payload.get_mut("items").and_then(|i| i.as_array_mut()) else {
        return;
    };
    for item in items {
        let Some(obj) = item.as_object_mut() else { continue };
        let Some(cover) = obj.get("cover").and_then(|c| c.as_str()).map(String::from) else {
            continue;
        };
        if let Some(path) = local.get(&cover) {
            obj.insert("cover".into(), serde_json::Value::String(path.clone()));
        }
    }
}

fn decode_cover(v: &serde_json::Value) -> Option<(Vec<u8>, String)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(v["base64"].as_str()?)
        .ok()?;
    Some((bytes, v["mime"].as_str().unwrap_or("image/jpeg").to_string()))
}

/// Download cover images through the bridge webview (the catalog is
/// Cloudflare-protected — the bridge is the only client that passes it) and
/// rewrite the payload's `cover` fields to the local files, so the panel and
/// the cache reference disk copies instead of the network. Existing files are
/// reused; covers no longer checked out are pruned. Failures leave the
/// remote URL in place (the panel loads it as before).
pub(crate) fn resolve_covers(app: &AppHandle, payload: &mut serde_json::Value) {
    let urls = collect_cover_urls(payload);
    if urls.is_empty() {
        return;
    }
    let Some(dir) = covers_dir(app) else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let referenced: HashSet<String> = urls.iter().map(|u| hash_hex(u)).collect();
    let existing: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();

    // Reuse files that already exist (the steady state — no downloads).
    let mut local: HashMap<String, String> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    for url in &urls {
        let hex = hash_hex(url);
        if let Some(name) = existing.iter().find(|n| n.starts_with(&hex)) {
            local.insert(url.clone(), dir.join(name).to_string_lossy().into_owned());
        } else {
            missing.push(url.clone());
        }
    }

    // Download the rest, a few in parallel (each goes through the bridge's
    // 500ms result poll, so parallelism keeps refreshes snappy).
    let downloaded = Mutex::new(0usize);
    let results: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
    let workers = missing.len().clamp(1, 4);
    let chunk_len = missing.len().div_ceil(workers);
    std::thread::scope(|scope| {
        for chunk in missing.chunks(chunk_len) {
            let dir = &dir;
            let (results, downloaded, app) = (&results, &downloaded, app);
            scope.spawn(move || {
                for url in chunk {
                    let args = crate::bridge::js_str(url);
                    let Ok(v) = crate::bridge::bridge_call_in(
                        app,
                        "fetchCover",
                        &format!("fetchCover:{url}"),
                        &args,
                        20,
                        false,
                    ) else {
                        continue;
                    };
                    let Some((bytes, mime)) = decode_cover(&v) else { continue };
                    let name = cover_file_name(url, &mime);
                    if std::fs::write(dir.join(&name), &bytes).is_ok() {
                        *downloaded.lock().unwrap() += 1;
                        results
                            .lock()
                            .unwrap()
                            .insert(url.clone(), name);
                    }
                }
            });
        }
    });
    for (url, name) in results.into_inner().unwrap() {
        local.insert(url, dir.join(name).to_string_lossy().into_owned());
    }
    apply_cover_paths(payload, &local);

    // Prune covers for items no longer checked out.
    for name in existing {
        let hex = name.split('.').next().unwrap_or("");
        if !referenced.contains(hex) {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }

    let n = downloaded.into_inner().unwrap();
    if n > 0 {
        app_log(app, "covers", &format!("downloaded {n} cover images"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_successful_real_payloads_are_cached() {
        assert!(cache_worthy(&serde_json::json!({ "success": true, "count": 2 })));
        assert!(!cache_worthy(&serde_json::json!({ "success": false })));
        assert!(!cache_worthy(&serde_json::json!({})));
        assert!(!cache_worthy(&serde_json::json!({ "success": true, "simulated": true })));
        assert!(!cache_worthy(&serde_json::json!({ "success": true, "stillSyncing": true, "count": 0 })));
        assert!(cache_worthy(&serde_json::json!({ "success": true, "stillSyncing": true, "count": 3 })));
    }

    #[test]
    fn age_is_human_readable() {
        assert_eq!(format_age(0), "0 min ago");
        assert_eq!(format_age(5 * 60), "5 min ago");
        assert_eq!(format_age(89 * 60), "89 min ago");
        assert_eq!(format_age(90 * 60), "1.5 h ago");
        assert_eq!(format_age(3 * 3600), "3.0 h ago");
    }

    #[test]
    fn cover_file_name_is_stable_and_uses_the_mime_extension() {
        let a = cover_file_name("https://catalog.example/bookcover.php?id=1", "image/jpeg");
        assert_eq!(a, cover_file_name("https://catalog.example/bookcover.php?id=1", "image/jpeg"));
        assert!(a.ends_with(".jpg"));
        assert!(cover_file_name("https://x", "image/png").ends_with(".png"));
        assert!(cover_file_name("https://x", "image/webp").ends_with(".webp"));
        assert!(cover_file_name("https://x", "application/octet-stream").ends_with(".jpg"));
        assert_ne!(a, cover_file_name("https://catalog.example/bookcover.php?id=2", "image/jpeg"));
    }

    #[test]
    fn cover_urls_are_collected_deduped_and_local_ones_skipped() {
        let payload = serde_json::json!({
            "items": [
                { "cover": "https://catalog.example/a.jpg" },
                { "cover": "" },
                { "cover": "/local/cover.jpg" },
                { "cover": "https://catalog.example/a.jpg" },
                { "cover": "https://cdn.example/b.jpg" }
            ]
        });
        assert_eq!(
            collect_cover_urls(&payload),
            vec![
                "https://catalog.example/a.jpg".to_string(),
                "https://cdn.example/b.jpg".to_string()
            ]
        );
    }

    #[test]
    fn cover_paths_are_applied_to_matching_items() {
        let mut payload = serde_json::json!({
            "items": [
                { "title": "A", "cover": "https://catalog.example/a.jpg" },
                { "title": "B", "cover": "" }
            ]
        });
        let mut local = HashMap::new();
        local.insert(
            "https://catalog.example/a.jpg".to_string(),
            "abc123.jpg".to_string(),
        );
        apply_cover_paths(&mut payload, &local);
        assert_eq!(payload["items"][0]["cover"], "abc123.jpg");
        assert_eq!(payload["items"][1]["cover"], "");
    }

    #[test]
    fn cover_decode_round_trips_base64() {
        let (bytes, mime) = decode_cover(&serde_json::json!({ "mime": "image/png", "base64": "iVBORw0KGgo=" }))
            .expect("decodes");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, vec![137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(decode_cover(&serde_json::json!({})).is_none());
    }
}