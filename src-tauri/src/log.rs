//! App event log, broadcast live to any open viewer, plus the developer
//! probe log at `$TMPDIR/sacpl-diag.log`.

use tauri::{AppHandle, Emitter, Manager};

use crate::debug::DEBUG_ACTIVE;

pub(crate) const LOG_FILE: &str = "app.log";
const LOG_MAX_BYTES: usize = 512 * 1024;

pub(crate) fn log_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(LOG_FILE))
}

/// Append a timestamped line to the log and broadcast it via
/// "log-appended". Safe from any thread; never log credentials or PINs.
pub(crate) fn app_log(app: &AppHandle, category: &str, message: &str) {
    let Some(path) = log_path(app) else {
        return;
    };
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() as usize > LOG_MAX_BYTES {
            if let Ok(data) = std::fs::read(&path) {
                let start = tail_start(&data, LOG_MAX_BYTES / 2);
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

/// The line-aligned tail of the log, for the Developer Settings viewer.
#[tauri::command]
pub(crate) fn lib_read_log(app: AppHandle) -> Result<String, String> {
    if !DEBUG_ACTIVE {
        return Err(crate::debug::devtools_unavailable());
    }
    let Some(path) = log_path(&app) else {
        return Ok(String::new());
    };
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    if data.len() > LOG_MAX_BYTES / 2 {
        let start = tail_start(&data, LOG_MAX_BYTES / 2);
        return Ok(String::from_utf8_lossy(&data[start..]).into_owned());
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// Byte offset of the first line of the last `keep` bytes (snapped forward
/// to the next newline; raw tail cut when there is none).
fn tail_start(data: &[u8], keep: usize) -> usize {
    if keep >= data.len() {
        return 0;
    }
    let cut = data.len() - keep;
    data[cut..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| cut + i + 1)
        .unwrap_or(cut)
}

/// Append to $TMPDIR/sacpl-diag.log when SACPL_DIAG is set (dev builds only).
pub(crate) fn diag_append(line: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn from_lines(lines: &[&str]) -> Vec<u8> {
        lines.join("\n").into_bytes()
    }

    #[test]
    fn tail_start_keeps_only_the_last_lines() {
        let data = from_lines(&["a", "b", "c", "d"]);
        assert_eq!(tail_start(&data, 4), 4);
    }

    #[test]
    fn tail_start_extends_to_a_line_boundary() {
        let data = from_lines(&["aaaa", "bb", "cc"]);
        assert_eq!(tail_start(&data, 5), 8);
    }

    #[test]
    fn tail_start_without_a_newline_keeps_the_raw_tail() {
        let data = b"only-one-line".to_vec();
        assert_eq!(tail_start(&data, 2), 11);
    }

    #[test]
    fn tail_start_of_empty_or_small_data_is_zero() {
        assert_eq!(tail_start(b"", 10), 0);
        assert_eq!(tail_start(b"ab\ncd", 10), 0);
    }
}