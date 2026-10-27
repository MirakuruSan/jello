pub mod ocr;
pub mod screenshot;

use tauri::{AppHandle, Manager, WebviewWindowBuilder, WebviewUrl, Emitter};
use crate::ipc_types::MonitorCaptureInfo;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Labels of Jello windows we hid so they wouldn't appear as ghosts in the
/// frozen screenshot; restored when the capture overlays close.
static HIDDEN_FOR_CAPTURE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Bumped on every capture so a stale safety timer never closes a newer one.
static CAPTURE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// How long an overlay may take to paint its screenshot before the capture is
/// abandoned and the hidden windows come back.
const OVERLAY_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Close all capture overlay windows and re-show any Jello windows that were
/// hidden for the capture (so the main window returns exactly when it was
/// visible before, and stays hidden when it wasn't).
fn close_overlays_and_restore(app: &AppHandle) {
    // Iterate windows() (Window), NOT webview_windows(): once content tabs are
    // added as child webviews, the main window drops out of webview_windows(), so
    // the old loop neither hid nor restored it → screenshot left main stuck
    // hidden with tabs open.
    for win in app.windows().values() {
        if win.label().starts_with("capture_") {
            let _ = win.close();
        }
    }
    let labels = std::mem::take(&mut *HIDDEN_FOR_CAPTURE.lock().unwrap());
    if labels.is_empty() { return; }
    let app_h = app.clone();
    let _ = app.run_on_main_thread(move || {
        for label in labels {
            if label == "main" {
                // Never gate the raw fallback behind a getter that returns None
                // when tabs are open — that was the restore bug.
                if let Some(w) = app_h.get_window("main") {
                    let _ = w.show();
                    set_dwm_transitions(&w, true);
                }
                crate::windows::force_show_main();
            } else if let Some(w) = app_h.get_window(&label) {
                let _ = w.show();
                set_dwm_transitions(&w, true);
            }
        }
    });
}

/// DWM fades a hidden window out over several frames, and a screenshot taken
/// mid-fade freezes Jello's own window into the capture. The fade is switched
/// off while a capture hides a window and back on once it is shown again.
fn set_dwm_transitions(win: &tauri::Window, enabled: bool) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = win.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
        let disabled: i32 = if enabled { 0 } else { 1 };
        unsafe {
            let _ = DwmSetWindowAttribute(
                HWND(hwnd.0),
                DWMWA_TRANSITIONS_FORCEDISABLED,
                &disabled as *const _ as *const _,
                std::mem::size_of_val(&disabled) as u32,
            );
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (win, enabled);
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
            for win in app_m.windows().values() {
                let label = win.label().to_string();
                if label.starts_with("capture_") { continue; }
                // For main, trust OS-level visibility (tauri's can lie once tabs
                // are open); for others, the Window's own is_visible.
                let visible = if label == "main" {
                    crate::windows::main_is_visible()
                } else {
                    win.is_visible().unwrap_or(false)
                };
                if visible {
                    set_dwm_transitions(win, false);
                    let _ = win.hide();
                    if label == "main" {
                        // Raw-hide so it can't ghost into the frozen frame.
                        crate::windows::force_hide_main();
                    }
                    hidden.push(label);
                }
            }
            let _ = tx.send(hidden);
        });
        let hidden = rx.recv().unwrap_or_default();
        // MERGE (dedupe) into the restore list — never overwrite. A second
        // trigger while overlays are open hides nothing new, so overwriting would
        // clobber the original list with [] and the windows would never restore.
        {
            let mut guard = HIDDEN_FOR_CAPTURE.lock().unwrap();
            for l in hidden {
                if !guard.contains(&l) { guard.push(l); }
            }
        }
    }
    // Let DWM drop the just-hidden windows from the composited frame before we
    // BitBlt the screen (with their fade-out disabled, one frame is enough).
    std::thread::sleep(std::time::Duration::from_millis(90));

    // Any failure from here on must give back the windows hidden above.
    let fail = |e: String| {
        tracing::warn!("screen capture failed: {}", e);
        close_overlays_and_restore(&app);
        e
    };

    let monitors = crate::platform::win_capture::capture_all_monitors(&app).map_err(fail)?;

    // Close any existing capture windows first to be safe
    for win in app.windows().values() {
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
        
        // Created hidden: the page reports back through capture_ready once the
        // screenshot has painted, so an empty window never flashes on screen.
        let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url_str.into()))
            .position(logical_x, logical_y)
            .inner_size(logical_w, logical_h)
            .visible(false)
            .always_on_top(true)
            .decorations(false)
            .resizable(false)
            .shadow(false)
            .skip_taskbar(true)
            // Must match the other app windows: webviews sharing the app data
            // dir must agree on this, or WebView2 refuses to create the second
            // environment and the overlay webview never loads.
            .browser_extensions_enabled(true)
            .build()
            .map_err(|e| fail(format!("Failed to build capture overlay window: {}", e)))?;
            
        // Pin the overlay to the monitor's exact physical bounds. The logical
        // size above is resolved at the creating monitor's scale, so on a
        // mixed-DPI setup the window lands on another monitor at the wrong size.
        let _ = win.set_position(tauri::PhysicalPosition::new(info.x, info.y));
        let _ = win.set_size(tauri::PhysicalSize::new(info.width.max(1) as u32, info.height.max(1) as u32));

        // Ensure fullscreen/topmost z-order
        let _ = win.set_always_on_top(true);
    }

    // If an overlay never paints (the screenshot failed to load), give the
    // user their windows back instead of leaving Jello hidden.
    let generation = CAPTURE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app_t = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(OVERLAY_READY_TIMEOUT);
        if CAPTURE_GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        let stalled = app_t
            .windows()
            .values()
            .any(|w| w.label().starts_with("capture_") && !w.is_visible().unwrap_or(false));
        if stalled {
            tracing::warn!("capture overlay did not become ready; closing the capture");
            close_overlays_and_restore(&app_t);
        }
    });

    Ok(monitors)
}

/// Called by an overlay once its screenshot has painted. Shows the window; the
/// overlay under the pointer also takes keyboard focus so Esc, Enter and the
/// arrow keys work immediately.
#[tauri::command]
pub async fn capture_ready(app: AppHandle, window: tauri::Window) -> Result<(), String> {
    let under_cursor = match (app.cursor_position(), window.outer_position(), window.outer_size()) {
        (Ok(c), Ok(p), Ok(s)) => {
            c.x >= p.x as f64
                && c.y >= p.y as f64
                && c.x < (p.x + s.width as i32) as f64
                && c.y < (p.y + s.height as i32) as f64
        }
        _ => true,
    };
    let app_m = app.clone();
    app.run_on_main_thread(move || {
        if under_cursor {
            let _ = window.show();
            crate::windows::force_foreground(&window);
            let _ = window.set_focus();
            crate::windows::focus_chrome(&app_m, window.label());
        } else {
            show_without_activating(&window);
        }
    })
    .map_err(|e| e.to_string())
}

fn show_without_activating(window: &tauri::Window) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = window.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNA};
        unsafe {
            let _ = ShowWindow(HWND(hwnd.0), SW_SHOWNA);
        }
        return;
    }
    let _ = window.show();
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
            // Front the main window first so the results tab is visible.
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
