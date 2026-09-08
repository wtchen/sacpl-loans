//! Credential persistence (macOS Keychain). WKWebView drops Aspen's
//! browser-session cookies on exit, so we keep the credentials ourselves and
//! re-authenticate silently on startup — the app's "Keep Me Signed In".

const KEYCHAIN_SERVICE: &str = "SacPL Loans";
const KEYCHAIN_USER: &str = "patron";

pub(crate) fn save_creds(user: &str, pin: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER).map_err(|e| e.to_string())?;
    entry.set_password(&format!("{user}\u{1}{pin}")).map_err(|e| e.to_string())
}

pub(crate) fn load_creds() -> Option<(String, String)> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER).ok()?;
    let raw = entry.get_password().ok()?;
    let (user, pin) = raw.split_once('\u{1}')?;
    Some((user.to_string(), pin.to_string()))
}

pub(crate) fn clear_creds() {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_USER) {
        let _ = entry.delete_credential();
    }
}