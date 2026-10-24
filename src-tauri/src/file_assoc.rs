//! Windows shell integration: Jello in Explorer's "Open with" menu, in Default
//! Apps as a web browser (http/https), and as a handler for the file types the
//! user picks in Settings. Everything lives under HKCU, so no elevation is
//! needed; Windows still lets only the user choose actual defaults (UserChoice
//! is hash-protected), which is why Settings links to the Default Apps page.

use tauri::AppHandle;

/// ProgIDs Jello registers. Shared with the uninstaller hook (hooks.nsh).
pub const PROGID_FILE: &str = "JelloHTML";
pub const PROGID_URL: &str = "JelloURL";
const CLIENT_KEY: &str = "Software\\Clients\\StartMenuInternet\\Jello";
const APP_KEY: &str = "Software\\Classes\\Applications\\jello.exe";
const MARKER_KEY: &str = "Software\\Jello";
/// Bump when write_registration changes so existing installs rewrite their keys.
const REGISTRATION_REV: u32 = 2;
/// Tauri's installer stamps shortcuts with the bundle identifier as the AUMID.
const APP_USER_MODEL_ID: &str = "com.jello.app";

/// File-type groups offered in Settings → Files: (key, label, extensions).
pub const GROUPS: &[(&str, &str, &[&str])] = &[
    ("web", "Web pages (.html, .htm, .mhtml…)", &["htm", "html", "shtml", "xht", "xhtml", "mht", "mhtml"]),
    ("pdf", "PDF documents", &["pdf"]),
    ("images", "Images (.svg, .webp, .avif, .png, .jpg, .gif…)", &["svg", "webp", "avif", "gif", "png", "jpg", "jpeg", "bmp", "ico"]),
    ("media", "Audio & video (.mp4, .webm, .mp3, .ogg…)", &["mp4", "webm", "ogg", "ogv", "mp3", "wav", "m4a", "flac", "opus"]),
    ("text", "Text & data (.txt, .json, .xml, .md…)", &["txt", "json", "xml", "md", "log", "csv"]),
];

const DEFAULT_GROUPS: &[&str] = &["web", "pdf"];

fn all_exts() -> impl Iterator<Item = &'static str> {
    GROUPS.iter().flat_map(|(_, _, exts)| exts.iter().copied())
}

fn exts_for(groups: &[String]) -> Vec<&'static str> {
    GROUPS
        .iter()
        .filter(|(k, _, _)| groups.iter().any(|g| g == k))
        .flat_map(|(_, _, exts)| exts.iter().copied())
        .collect()
}

/// The groups saved in the `fileTypes` setting (default: web pages + PDF).
fn saved_groups(app: &AppHandle) -> Vec<String> {
    crate::capture::screenshot::get_setting(app, "fileTypes")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_else(|| DEFAULT_GROUPS.iter().map(|s| s.to_string()).collect())
}

/// Registering from a dev build would point every handler at target\debug and
/// break the installed Jello, so debug builds only register when asked to.
fn registration_allowed() -> bool {
    !cfg!(debug_assertions) || std::env::var("JELLO_SHELL_REGISTER").as_deref() == Ok("1")
}

#[cfg(target_os = "windows")]
mod reg {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn open(path: &str) -> Option<HKEY> {
        let p = wide(path);
        let mut key = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), None, PCWSTR::null(),
                REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None,
            )
            .ok()
            .ok()?;
        }
        Some(key)
    }

    /// Set a REG_SZ value ("" name = the key's default value).
    pub fn set_str(path: &str, name: &str, value: &str) {
        let Some(key) = open(path) else { return };
        let n = wide(name);
        let v = wide(value);
        unsafe {
            let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
            let name_ptr = if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) };
            let _ = RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes));
            let _ = RegCloseKey(key);
        }
    }

    pub fn delete_value(path: &str, name: &str) {
        let p = wide(path);
        let n = wide(name);
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()));
        }
    }

    pub fn delete_tree(path: &str) {
        let p = wide(path);
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()));
        }
    }

    pub fn get_str(path: &str, name: &str) -> Option<String> {
        let p = wide(path);
        let n = wide(name);
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER, PCWSTR(p.as_ptr()),
                if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) },
                RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len),
            )
            .ok()
            .ok()?;
        }
        let chars = (len as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..chars.min(buf.len())]))
    }
}

/// Write every registration for `exe`, associating `exts`. Extensions not in
/// `exts` get Jello's OpenWithProgids entry removed, so unticking a group in
/// Settings really takes Jello out of that "Open with" list.
#[cfg(target_os = "windows")]
fn write_registration(exe: &str, exts: &[&str]) {
    let open_cmd = format!("\"{}\" \"%1\"", exe);
    // Layout below mirrors what Chromium-based browsers register (unquoted
    // "path,index" icons, a full Application block, empty-string
    // OpenWithProgids values).
    let icon = format!("{},0", exe);

    for (progid, desc) in [(PROGID_FILE, "Jello Document"), (PROGID_URL, "Jello URL")] {
        let base = format!("Software\\Classes\\{}", progid);
        reg::set_str(&base, "", desc);
        reg::set_str(&format!("{}\\DefaultIcon", base), "", &icon);
        reg::set_str(&format!("{}\\shell\\open\\command", base), "", &open_cmd);
        // Name, icon and publisher the shell shows for this handler.
        let app_info = format!("{}\\Application", base);
        reg::set_str(&app_info, "ApplicationName", "Jello");
        reg::set_str(&app_info, "ApplicationIcon", &icon);
        reg::set_str(&app_info, "ApplicationDescription", "Browse the web");
        reg::set_str(&app_info, "ApplicationCompany", "MirakuruSan");
        reg::set_str(&app_info, "AppUserModelId", APP_USER_MODEL_ID);
    }
    reg::set_str(&format!("Software\\Classes\\{}", PROGID_URL), "URL Protocol", "");

    // Explorer's "Open with → Choose another app" list.
    reg::delete_tree(&format!("{}\\SupportedTypes", APP_KEY));
    reg::set_str(APP_KEY, "FriendlyAppName", "Jello");
    reg::set_str(&format!("{}\\DefaultIcon", APP_KEY), "", &icon);
    reg::set_str(&format!("{}\\shell\\open\\command", APP_KEY), "", &open_cmd);
    for ext in exts {
        reg::set_str(&format!("{}\\SupportedTypes", APP_KEY), &format!(".{}", ext), "");
    }

    // Per-extension "Open with" submenu entries.
    for ext in all_exts() {
        let key = format!("Software\\Classes\\.{}\\OpenWithProgids", ext);
        if exts.contains(&ext) {
            reg::set_str(&key, PROGID_FILE, "");
        } else {
            reg::delete_value(&key, PROGID_FILE);
        }
    }

    // Default Apps: a registered browser with its file + URL capabilities.
    reg::set_str(CLIENT_KEY, "", "Jello");
    reg::set_str(&format!("{}\\DefaultIcon", CLIENT_KEY), "", &icon);
    reg::set_str(&format!("{}\\shell\\open\\command", CLIENT_KEY), "", &format!("\"{}\"", exe));
    let caps = format!("{}\\Capabilities", CLIENT_KEY);
    reg::set_str(&caps, "ApplicationName", "Jello");
    reg::set_str(&caps, "ApplicationIcon", &icon);
    reg::set_str(&caps, "ApplicationDescription", "Jello — a light, keyboard-first web browser");
    reg::set_str(&format!("{}\\StartMenu", caps), "StartMenuInternet", "Jello");
    reg::delete_tree(&format!("{}\\FileAssociations", caps));
    for ext in exts {
        reg::set_str(&format!("{}\\FileAssociations", caps), &format!(".{}", ext), PROGID_FILE);
    }
    for scheme in ["http", "https"] {
        reg::set_str(&format!("{}\\URLAssociations", caps), scheme, PROGID_URL);
    }
    reg::set_str("Software\\RegisteredApplications", "Jello", &caps);

    unsafe {
        use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

/// Register (or refresh) the shell integration for the saved file-type groups.
/// Skips the registry writes when nothing changed since the last run.
pub fn apply(app: &AppHandle, force: bool) -> Result<(), String> {
    if !registration_allowed() {
        return Err("shell registration is disabled in development builds (set JELLO_SHELL_REGISTER=1)".into());
    }
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?.to_string_lossy().to_string();
        let groups = saved_groups(app);
        let stamp = format!("{}|r{}|{}|{}", env!("CARGO_PKG_VERSION"), REGISTRATION_REV, exe, groups.join(","));
        if !force && reg::get_str(MARKER_KEY, "ShellRegistration").as_deref() == Some(stamp.as_str()) {
            return Ok(());
        }
        write_registration(&exe, &exts_for(&groups));
        reg::set_str(MARKER_KEY, "ShellRegistration", &stamp);
        tracing::info!("shell integration registered for groups [{}]", groups.join(","));
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, force);
    Ok(())
}

/// Startup: refresh the registration off the main thread.
pub fn register_on_startup(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = apply(&app, false) {
            tracing::info!("shell integration not registered: {}", e);
        }
    });
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTypeGroup {
    key: String,
    label: String,
    enabled: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAssocStatus {
    groups: Vec<FileTypeGroup>,
    /// Jello is Windows' current default for https links.
    default_browser: bool,
    /// Registration is possible in this build (false in dev builds).
    available: bool,
}

#[tauri::command]
pub async fn file_assoc_status(app: AppHandle) -> Result<FileAssocStatus, String> {
    let saved = saved_groups(&app);
    #[cfg(target_os = "windows")]
    let default_browser = reg::get_str(
        "Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice",
        "ProgId",
    )
    .as_deref()
        == Some(PROGID_URL);
    #[cfg(not(target_os = "windows"))]
    let default_browser = false;
    Ok(FileAssocStatus {
        groups: GROUPS
            .iter()
            .map(|(k, label, _)| FileTypeGroup {
                key: k.to_string(),
                label: label.to_string(),
                enabled: saved.iter().any(|g| g == k),
            })
            .collect(),
        default_browser,
        available: registration_allowed(),
    })
}

/// Save which file-type groups Jello handles and re-register.
#[tauri::command]
pub async fn file_assoc_set(groups: Vec<String>, app: AppHandle) -> Result<(), String> {
    let groups: Vec<String> = groups.into_iter().filter(|g| GROUPS.iter().any(|(k, _, _)| k == g)).collect();
    let json = serde_json::to_string(&groups).map_err(|e| e.to_string())?;
    let db = tauri::Manager::state::<crate::db::DbState>(&app);
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let _ = tx.send(
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value_json) VALUES ('fileTypes', ?1)",
                [json],
            )
            .map_err(|e| e.to_string()),
        );
    });
    rx.recv().map_err(|e| e.to_string())??;
    apply(&app, true)
}

/// Open Windows' Default Apps page for Jello, where the user picks defaults.
#[tauri::command]
pub async fn file_assoc_open_defaults(app: AppHandle) -> Result<(), String> {
    let _ = apply(&app, false);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .creation_flags(0x08000000)
            .arg("ms-settings:defaultapps?registeredAppUser=Jello")
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_map_to_extensions() {
        let exts = exts_for(&["pdf".to_string(), "web".to_string()]);
        assert!(exts.contains(&"pdf") && exts.contains(&"html") && !exts.contains(&"png"));
        assert!(exts_for(&["bogus".to_string()]).is_empty());
    }

    #[test]
    fn uninstall_hook_cleans_every_extension() {
        let hook = include_str!("../windows/hooks.nsh");
        for ext in all_exts() {
            assert!(hook.contains(&format!("Software\\Classes\\.{}\\OpenWithProgids", ext)), "hooks.nsh misses .{}", ext);
        }
        for key in [PROGID_FILE, PROGID_URL, CLIENT_KEY, APP_KEY] {
            assert!(hook.contains(key), "hooks.nsh misses {}", key);
        }
    }
}
