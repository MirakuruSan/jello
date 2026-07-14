use std::path::PathBuf;
use arboard::{Clipboard, ImageData};
use tauri::{AppHandle, Manager, Emitter};
use image::GenericImageView;

fn percent_encode(s: &str) -> String {
    let mut res = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                res.push(b as char);
            }
            _ => {
                res.push_str(&format!("%{:02X}", b));
            }
        }
    }
    res
}

pub fn crop_image(source_path: &str, x: i32, y: i32, w: i32, h: i32) -> Result<String, String> {
    let mut img = image::open(source_path).map_err(|e| e.to_string())?;
    let cropped = img.crop(x as u32, y as u32, w as u32, h as u32);
    
    let temp_dir = std::env::temp_dir().join("JelloCapture");
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
    
    let now_millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let crop_path = temp_dir.join(format!("crop_{}.png", now_millis));
    cropped.save(&crop_path).map_err(|e| e.to_string())?;
    
    Ok(crop_path.to_string_lossy().to_string())
}

pub fn copy_image_to_clipboard(image_path: &str) -> Result<(), String> {
    let img = image::open(image_path).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = img.dimensions();
    let pixels = img.into_raw();
    
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    let img_data = ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(pixels),
    };
    clipboard.set_image(img_data).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text.to_string()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn execute_screenshot_action(
    app: AppHandle,
    action: String,
    image_path: String,
    ocr_text: Option<String>,
) -> Result<(), String> {
    match action.as_str() {
        "copy" => {
            copy_image_to_clipboard(&image_path)?;
            let _ = app.emit("toast:show", "Image copied to clipboard!".to_string());
        }
        "save" => {
            let pictures_dir = std::env::var("USERPROFILE")
                .map(|p| PathBuf::from(p).join("Pictures").join("Jello"))
                .map_err(|_| "Could not find User Profile path".to_string())?;
                
            std::fs::create_dir_all(&pictures_dir).map_err(|e| e.to_string())?;
            
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let dest_path = pictures_dir.join(format!("jello_{}.png", now));
            std::fs::copy(&image_path, &dest_path).map_err(|e| e.to_string())?;
            
            let _ = app.emit("toast:show", format!("Saved to Pictures\\Jello\\jello_{}.png", now));
        }
        "search" => {
            copy_image_to_clipboard(&image_path)?;
            let _ = app.emit("toast:show", "Image copied! Press Ctrl+V to search Google Lens.".to_string());
            
            // Bring the main window forward first, or the tab opens into a
            // hidden window and the user sees "nothing happened" (same bug class
            // as the old palette path — Phase 8.5).
            crate::windows::ensure_main_window(&app);
            let active_win_id = get_focused_window_id(&app);
            let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
            let db = app.state::<crate::db::DbState>();
            let _ = crate::tabs::tabs_create_impl(
                Some("https://lens.google.com/upload".to_string()),
                Some(false),
                Some(active_win_id),
                &db,
                &pool,
                &app,
            );
        }
        "ask_ai" => {
            crate::windows::ensure_main_window(&app);
            let active_win_id = get_focused_window_id(&app);
            let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
            let db = app.state::<crate::db::DbState>();
            
            // Use the user's configured chatbot (may or may not contain %s).
            let template = get_setting(&app, "defaultChatbot")
                .unwrap_or_else(|| "https://chatgpt.com/?q=%s".to_string());
            if let Some(text) = ocr_text {
                let chat_url = if template.contains("%s") {
                    template.replace("%s", &percent_encode(&text))
                } else {
                    let _ = copy_text_to_clipboard(&text);
                    let _ = app.emit("toast:show", "Text copied! Press Ctrl+V in the chatbot.".to_string());
                    template.clone()
                };
                let _ = crate::tabs::tabs_create_impl(
                    Some(chat_url), Some(false), Some(active_win_id),
                    &db, &pool, &app,
                );
            } else {
                copy_image_to_clipboard(&image_path)?;
                let _ = app.emit("toast:show", "Image copied! Press Ctrl+V to paste in the chatbot.".to_string());
                let base = template.split('?').next().unwrap_or("https://chatgpt.com/").to_string();
                let _ = crate::tabs::tabs_create_impl(
                    Some(base), Some(false), Some(active_win_id),
                    &db, &pool, &app,
                );
            }
        }
        "pin" => {
            let img = image::open(&image_path).map_err(|e| e.to_string())?;
            let (w, h) = img.dimensions();
            
            let window = app.get_webview_window("main")
                .or_else(|| app.webview_windows().values().next().cloned())
                .ok_or_else(|| "No main window found".to_string())?;
            let monitor = window.current_monitor().map_err(|e| e.to_string())?.unwrap();
            let scale = monitor.scale_factor();
            
            let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
            let label = format!("pin_{}", id);
            
            let encoded_path = percent_encode(&image_path);
            let image_url = format!("index.html?pin_image={}", encoded_path);
            
            let _ = tauri::WebviewWindowBuilder::new(
                &app,
                &label,
                tauri::WebviewUrl::App(image_url.into()),
            )
            .inner_size((w as f64) / scale, (h as f64) / scale)
            .always_on_top(true)
            .decorations(false)
            .transparent(true)
            // Match the other app windows (see capture/mod.rs) so the pinned
            // image webview can create in the shared WebView2 environment.
            .browser_extensions_enabled(true)
            .build()
            .map_err(|e| e.to_string())?;
        }
        _ => return Err(format!("Unknown screenshot action: {}", action)),
    }
    
    Ok(())
}

/// Read a single setting value (raw string) from the DB, if present.
pub fn get_setting(app: &AppHandle, key: &str) -> Option<String> {
    let db = app.state::<crate::db::DbState>();
    let (tx, rx) = std::sync::mpsc::channel();
    let key = key.to_string();
    db.execute(move |conn| {
        let v: Option<String> = conn
            .query_row("SELECT value_json FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .ok();
        let _ = tx.send(v);
    });
    rx.recv().ok().flatten().map(|s| s.trim_matches('"').to_string())
}

fn get_focused_window_id(app: &AppHandle) -> i32 {
    for win in app.webview_windows().values() {
        if win.is_focused().unwrap_or(false) {
            let label = win.label();
            if label == "main" {
                return 1;
            }
            if let Some(pos) = label.rfind('_') {
                if let Ok(id) = label[pos + 1..].parse::<i32>() {
                    return id;
                }
            }
        }
    }
    1
}
