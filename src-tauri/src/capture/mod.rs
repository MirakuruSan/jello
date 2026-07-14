pub mod ocr;
pub mod screenshot;

use tauri::{AppHandle, Manager, WebviewWindowBuilder, WebviewUrl, Emitter};
use crate::ipc_types::MonitorCaptureInfo;
use std::sync::Mutex;

/// Labels of Jello windows we hid so they wouldn't appear as ghosts in the
/// frozen screenshot; restored when the capture overlays close.
static HIDDEN_FOR_CAPTURE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Close all capture overlay windows and re-show any Jello windows that were
/// hidden for the capture (so the main window returns exactly when it was
/// visible before, and stays hidden when it wasn't).
fn close_overlays_and_restore(app: &AppHandle) {
    for win in app.webview_windows().values() {
        if win.label().starts_with("capture_") {
            let _ = win.close();
        }
    }
    let labels = std::mem::take(&mut *HIDDEN_FOR_CAPTURE.lock().unwrap());
    if labels.is_empty() { return; }
    let app_h = app.clone();
    let _ = app.run_on_main_thread(move || {
        for label in labels {
            if let Some(w) = app_h.get_webview_window(&label) {
                let _ = w.show();
            }
        }
    });
}

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

#[tauri::command]
pub async fn capture_trigger(app: AppHandle, mode: String) -> Result<Vec<MonitorCaptureInfo>, String> {
    // Hide Jello's own visible windows BEFORE the screenshot so the transparent
    // frameless chrome doesn't get frozen into the background as a ghost. Track
    // which we hid so they can be restored exactly when the overlays close.
    {
        let mut hidden = Vec::new();
        let app_m = app.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            for win in app_m.webview_windows().values() {
                let label = win.label().to_string();
                if label.starts_with("capture_") { continue; }
                if win.is_visible().unwrap_or(false) {
                    let _ = win.hide();
                    hidden.push(label);
                }
            }
            let _ = tx.send(hidden);
        });
        let hidden = rx.recv().unwrap_or_default();
        *HIDDEN_FOR_CAPTURE.lock().unwrap() = hidden;
    }
    // Let DWM drop the just-hidden windows from the composited frame before we
    // BitBlt the screen.
    std::thread::sleep(std::time::Duration::from_millis(90));

    let monitors = crate::platform::win_capture::capture_all_monitors(&app)?;

    // Close any existing capture windows first to be safe
    for win in app.webview_windows().values() {
        if win.label().starts_with("capture_") {
            let _ = win.close();
        }
    }
    
    // Spawn a borderless selection overlay window for each monitor
    for info in &monitors {
        let label = format!("capture_{}", info.index);
        let encoded_path = percent_encode(&info.image_path);
        let url_str = format!(
            "index.html?capture_index={}&image_path={}&scale={}&mode={}",
            info.index, encoded_path, info.scale_factor, mode
        );
        
        let logical_x = (info.x as f64) / info.scale_factor;
        let logical_y = (info.y as f64) / info.scale_factor;
        let logical_w = (info.width as f64) / info.scale_factor;
        let logical_h = (info.height as f64) / info.scale_factor;
        
        let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url_str.into()))
            .position(logical_x, logical_y)
            .inner_size(logical_w, logical_h)
            .always_on_top(true)
            .decorations(false)
            .transparent(true)
            .skip_taskbar(true)
            // Must match the other app windows: webviews sharing the app data
            // dir must agree on this, or WebView2 refuses to create the second
            // environment and the overlay webview never loads (regression from
            // enabling extensions on the main window).
            .browser_extensions_enabled(true)
            .build()
            .map_err(|e| format!("Failed to build capture overlay window: {}", e))?;
            
        // Ensure fullscreen/topmost z-order
        let _ = win.set_always_on_top(true);
    }
    
    Ok(monitors)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command: each arg is a distinct IPC field.
pub async fn capture_crop(
    app: AppHandle,
    source_path: String,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    action: String,
    is_ocr: bool,
) -> Result<serde_json::Value, String> {
    // Crop the image
    let crop_path = screenshot::crop_image(&source_path, x, y, width, height)?;
    
    if is_ocr {
        // Run OCR, copy text, and return it. Leave the overlay open so the
        // frontend can show the editable result popover; it closes overlays
        // via capture_cancel after the user picks an action.
        let text = ocr::run_ocr_on_image(&crop_path).map_err(|e| format!("OCR error: {}", e))?;
        let _ = screenshot::copy_text_to_clipboard(&text);
        Ok(serde_json::json!({ "ocrText": text, "imagePath": crop_path }))
    } else {
        screenshot::execute_screenshot_action(app.clone(), action, crop_path.clone(), None)?;
        // Screenshot action is terminal: close the overlays + restore windows.
        close_overlays_and_restore(&app);
        Ok(serde_json::json!({ "imagePath": crop_path }))
    }
}

/// Run a follow-up action from the OCR result popover, then close the overlays.
#[tauri::command]
pub async fn capture_ocr_action(
    app: AppHandle,
    text: String,
    action: String,
    image_path: String,
) -> Result<(), String> {
    match action.as_str() {
        "copy" => {
            screenshot::copy_text_to_clipboard(&text)?;
            let _ = app.emit("toast:show", "Text copied".to_string());
        }
        "search" => {
            // Front the main window first so the results tab is visible (Phase 8.5).
            crate::windows::ensure_main_window(&app);
            let query = percent_encode(&text);
            let url = format!("https://duckduckgo.com/?q={}", query);
            let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
            let db = app.state::<crate::db::DbState>();
            let _ = crate::tabs::tabs_create_impl(Some(url), Some(false), None, &db, &pool, &app);
        }
        "ask_ai" => {
            screenshot::execute_screenshot_action(app.clone(), "ask_ai".to_string(), image_path, Some(text))?;
        }
        _ => return Err(format!("Unknown OCR action: {}", action)),
    }
    // Close overlays + restore any windows we hid for the capture.
    close_overlays_and_restore(&app);
    Ok(())
}

#[tauri::command]
pub async fn capture_cancel(app: AppHandle) -> Result<(), String> {
    close_overlays_and_restore(&app);
    Ok(())
}
