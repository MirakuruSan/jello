use tauri::{AppHandle, Manager, Emitter};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use windows::Win32::System::Registry::{
    RegCreateKeyExW, RegSetValueExW, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    HKEY_CURRENT_USER, HKEY, KEY_WRITE, KEY_READ, REG_SZ, REG_OPTION_NON_VOLATILE,
    REG_VALUE_TYPE, REG_CREATE_KEY_DISPOSITION,
};
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;

#[cfg(target_os = "windows")]
fn to_utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn get_reg_value() -> Option<String> {
    unsafe {
        let subkey = to_utf16("Software\\Classes\\jello\\shell\\open\\command");
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        ).is_ok() {
            let mut buf = vec![0u8; 1024];
            let mut len = buf.len() as u32;
            let mut dtype = REG_VALUE_TYPE::default();
            let res = RegQueryValueExW(
                hkey,
                PCWSTR::null(),
                None,
                Some(&mut dtype),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            );
            let _ = RegCloseKey(hkey);
            if res.is_ok() {
                let u16_len = len as usize / 2;
                let u16_slice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
                let actual_len = u16_slice.iter().position(|&c| c == 0).unwrap_or(u16_len);
                return String::from_utf16(&u16_slice[..actual_len]).ok();
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn set_reg_value(subkey_str: &str, name_str: &str, value_str: &str) -> Result<(), String> {
    unsafe {
        let subkey = to_utf16(subkey_str);
        let name = if name_str.is_empty() {
            Vec::new()
        } else {
            to_utf16(name_str)
        };
        let value = to_utf16(value_str);
        
        let mut hkey = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
        
        let res = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            Some(&mut disposition),
        );
        res.ok().map_err(|e| e.to_string())?;
        
        let name_ptr = if name_str.is_empty() {
            PCWSTR::null()
        } else {
            PCWSTR(name.as_ptr())
        };
        
        let u8_slice = std::slice::from_raw_parts(
            value.as_ptr() as *const u8,
            value.len() * 2,
        );

        let res = RegSetValueExW(
            hkey,
            name_ptr,
            None,
            REG_SZ,
            Some(u8_slice),
        );
        let _ = RegCloseKey(hkey);
        res.ok().map_err(|e| e.to_string())
    }
}

/// Registers the jello:// protocol handler in the Windows registry (HKCU).
pub fn register_jello_protocol() {
    #[cfg(target_os = "windows")]
    {
        std::thread::spawn(move || {
            if let Ok(exe_path) = std::env::current_exe() {
                let exe_str = exe_path.to_string_lossy();
                let cmd = format!("\"{}\" \"%1\"", exe_str);

                if let Some(current_val) = get_reg_value() {
                    if current_val == cmd {
                        return;
                    }
                }

                let _ = set_reg_value("Software\\Classes\\jello", "", "URL:Jello Protocol");
                let _ = set_reg_value("Software\\Classes\\jello", "URL Protocol", "");
                let _ = set_reg_value("Software\\Classes\\jello\\shell\\open\\command", "", &cmd);
            }
        });
    }
}

/// Handles a CLI or deep link argument.
pub fn handle_open_argument(app: &AppHandle, arg: &str) -> Result<(), String> {
    let arg_trimmed = arg.trim();
    if arg_trimmed.is_empty() {
        return Ok(());
    }

    let mut target = arg_trimmed;
    if target.starts_with("jello://") {
        target = &target["jello://".len()..];
    }

    // Check if target is a view command
    if target == "settings" || target == "history" || target == "bookmarks" || target == "downloads" {
        // Focus/summon the main window
        if let Some(win) = app.get_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
        }
        // Emit view event to frontend
        let _ = app.emit("window:open-view", target.to_string());
        return Ok(());
    }

    // Apply security check: file:// behind setting.
    if target.starts_with("file://") {
        let allow_file = crate::capture::screenshot::get_setting(app, "allowFileUrls")
            .map(|v| v == "true")
            .unwrap_or(false);
        if !allow_file {
            let _ = app.emit("toast:show", "Opening file:// URLs is disabled for security.".to_string());
            return Err("file:// URLs are disabled".to_string());
        }
    }

    // Get DB and TabPool
    let db = app.state::<crate::db::DbState>();
    let pool = app.state::<Arc<Mutex<crate::engine::pool::TabPool>>>();

    use crate::search::{classify_input, InputClassification, get_search_engines, route_query};
    let (tx, rx) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let _ = tx.send(get_search_engines(conn));
    });
    let engines = rx.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;

    let resolved_url = match classify_input(target) {
        InputClassification::Url(u) => u,
        InputClassification::SearchQuery(q) => {
            let template = crate::capture::screenshot::get_setting(app, "defaultSearch")
                .unwrap_or_else(|| "https://duckduckgo.com/?q=%s".to_string());
            route_query(&q, &engines, &template)
        }
    };

    let active = pool.lock().unwrap().get_active_tab_id();
    match active {
        Some(tid) => pool.lock().unwrap().navigate_tab(db.inner(), app, tid, &resolved_url)?,
        None => {
            crate::tabs::tabs_create_impl(Some(resolved_url), Some(false), None, &db, &pool, app)?;
        }
    }

    // Focus/summon the main window
    if let Some(win) = app.get_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }

    Ok(())
}
