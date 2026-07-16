use tauri::{AppHandle, Manager, State, Window, Emitter};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use rusqlite::OptionalExtension;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct HitRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub struct OverlayState {
    /// Hit rects (CSS pixels) per window label — each window reports its own.
    pub hit_rects: Mutex<HashMap<String, Vec<HitRect>>>,
    pub overlay_hwnds: Mutex<HashMap<String, isize>>,
    /// Whether a content webview is currently shown in the window. When false
    /// (new-tab page / no tabs) the overlay must accept input everywhere.
    pub has_content: Mutex<HashMap<String, bool>>,
    pub scale_factors: Mutex<HashMap<String, f64>>,
}

pub static OVERLAY_INTERACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Clip the overlay webview's host HWND to the union of the chrome's hit
/// rects with SetWindowRgn. This is the ONLY reliable way to let clicks fall
/// through to the content webview underneath: WS_EX_TRANSPARENT on the host
/// does nothing because the Chromium CHILD windows inside it still hit-test
/// (that was the "can't interact with pages" bug). A window region clips
/// hit-testing of the host and all its children. Full region (None) while a
/// panel is open or while the chrome layer IS the page (new-tab, capture).
pub fn apply_overlay_region(app: &AppHandle, label: &str) {
    #[cfg(target_os = "windows")]
    {
        let state = app.state::<Arc<OverlayState>>();
        let hwnd_val = match state.overlay_hwnds.lock().unwrap().get(label).copied() {
            Some(h) => h,
            None => return, // not resolved yet; initial region stays full
        };
        // NOTE: PANEL_OPEN deliberately does NOT force a full region anymore —
        // that made an open tab/settings panel eat every click on the page (#2).
        // The open panel's own rect is included in the hit rects instead, so the
        // page stays interactive around it. PANEL_OPEN still gates Esc handling.
        let overlay_interactive = OVERLAY_INTERACTIVE.load(std::sync::atomic::Ordering::Relaxed);
        let has_content = state.has_content.lock().unwrap().get(label).copied().unwrap_or(false);
        let full = overlay_interactive || !has_content;
        let scale = state.scale_factors.lock().unwrap().get(label).copied().unwrap_or(1.0);
        let rects: Vec<(i32, i32, i32, i32)> = if full {
            Vec::new()
        } else {
            state
                .hit_rects
                .lock()
                .unwrap()
                .get(label)
                .map(|rs| {
                    rs.iter()
                        .filter(|r| r.width > 0 && r.height > 0)
                        .map(|r| {
                            let x = (r.x as f64 * scale).floor() as i32;
                            let y = (r.y as f64 * scale).floor() as i32;
                            let x2 = ((r.x + r.width) as f64 * scale).ceil() as i32;
                            let y2 = ((r.y + r.height) as f64 * scale).ceil() as i32;
                            (x, y, x2, y2)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let _ = app.run_on_main_thread(move || unsafe {
            use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, RGN_OR};
            use windows::Win32::Graphics::Gdi::SetWindowRgn;
            let hwnd = windows::Win32::Foundation::HWND(hwnd_val as *mut _);
            if full {
                let _ = SetWindowRgn(hwnd, None, true);
            } else {
                // Empty rect list -> empty region -> fully click-through.
                let rgn = CreateRectRgn(0, 0, 0, 0);
                for (x, y, x2, y2) in rects {
                    let piece = CreateRectRgn(x, y, x2, y2);
                    let _ = CombineRgn(Some(rgn), Some(rgn), Some(piece), RGN_OR);
                    let _ = DeleteObject(piece.into());
                }
                // SetWindowRgn takes ownership of rgn.
                let _ = SetWindowRgn(hwnd, Some(rgn), true);
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, label);
}

/// Record whether a content view is visible in `label` and re-apply the region.
pub fn overlay_mark_content(app: &AppHandle, label: &str, has_content: bool) {
    {
        let state = app.state::<Arc<OverlayState>>();
        state.has_content.lock().unwrap().insert(label.to_string(), has_content);
    }
    apply_overlay_region(app, label);
}

#[tauri::command]
pub fn overlay_set_interactive(interactive: bool, window: Window, app: AppHandle) {
    OVERLAY_INTERACTIVE.store(interactive, std::sync::atomic::Ordering::Relaxed);
    apply_overlay_region(&app, window.label());
}

#[tauri::command]
pub fn overlay_set_hit_rects(rects: Vec<HitRect>, window: Window, app: AppHandle, state: tauri::State<Arc<OverlayState>>) {
    state.hit_rects.lock().unwrap().insert(window.label().to_string(), rects);
    apply_overlay_region(&app, window.label());
}

#[tauri::command]
pub fn test_command() -> String {
    "Hello from Jello!".to_string()
}

#[tauri::command]
pub fn overlay_set_panel_open(open: bool, window: Window, app: AppHandle) {
    #[cfg(target_os = "windows")]
    crate::engine::webview2::PANEL_OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(target_os = "windows"))]
    let _ = open;
    apply_overlay_region(&app, window.label());
}

pub struct StartupArgState(pub Mutex<Option<String>>);

#[tauri::command]
pub async fn process_startup_arg(
    app: AppHandle,
    state: State<'_, StartupArgState>,
) -> Result<(), String> {
    let mut lock = state.0.lock().unwrap();
    if let Some(arg) = lock.take() {
        crate::deeplink::handle_open_argument(&app, &arg)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn window_controls(action: String, window: Window, app: AppHandle) -> Result<(), String> {
    match action.as_str() {
        "min" => window.minimize().map_err(|e| e.to_string()),
        "max" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize().map_err(|e| e.to_string())?;
            } else {
                window.maximize().map_err(|e| e.to_string())?;
            }
            // The Resized event handler (attach_window_plumbing) now resizes the
            // active tab on EVERY resize path — button, Win+Up, snap, title-drag —
            // with a debounced off-main retry when the pool is busy (P2.1). No
            // per-command retry needed here; one mechanism covers them all.
            Ok(())
        }
        "close" => {
            // For the main window, apply the minimize-to-tray choice DIRECTLY
            // here. Going through window.close() -> CloseRequested from a command
            // handler proved unreliable (the hide never took effect), so we hide
            // or exit here and let CloseRequested only handle the OS × / Alt+F4.
            if window.label() == "main" {
                let minimize_to_tray = crate::capture::screenshot::get_setting(&app, "minimizeToTray")
                    .map(|v| v != "false")
                    .unwrap_or(true);
                if minimize_to_tray {
                    window.hide().map_err(|e| e.to_string())?;
                    // Follow tauri's hide with the raw fallback so tao's flag and
                    // on-screen reality can't diverge (P0.1 step 6).
                    crate::windows::force_hide_main();
                    crate::windows::on_main_hidden(&app);
                    Ok(())
                } else {
                    crate::sessions::on_exit(&app);
                    app.exit(0);
                    Ok(())
                }
            } else {
                window.close().map_err(|e| e.to_string())
            }
        }
        _ => Err(format!("Unknown window action: {}", action)),
    }
}

/// Toggle always-on-top for the calling window and persist the choice.
#[tauri::command]
pub async fn window_set_pinned(
    pinned: bool,
    window: Window,
    db: State<'_, crate::db::DbState>,
) -> Result<(), String> {
    window.set_always_on_top(pinned).map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = conn.execute(
            "INSERT INTO settings (key, value_json) VALUES ('pinnedOnTop', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = ?1",
            [if pinned { "true" } else { "false" }],
        );
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(0)).map(|_| ()).map_err(|e| e.to_string())
}

/// Read the persisted pinned-on-top preference.
#[tauri::command]
pub async fn window_pinned_state(app: AppHandle) -> bool {
    crate::capture::screenshot::get_setting(&app, "pinnedOnTop")
        .map(|v| v == "true")
        .unwrap_or(false)
}

#[tauri::command]
pub async fn nav_back(pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>) -> Result<(), String> {
    pool.lock().unwrap().nav_back()
}

#[tauri::command]
pub async fn nav_forward(pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>) -> Result<(), String> {
    pool.lock().unwrap().nav_forward()
}

#[tauri::command]
pub async fn nav_reload(
    pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>,
    db: State<'_, crate::db::DbState>,
    app: AppHandle,
) -> Result<(), String> {
    pool.lock().unwrap().reload_or_reactivate(&db, &app)
}

#[tauri::command]
pub async fn nav_stop(pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>) -> Result<(), String> {
    pool.lock().unwrap().nav_stop()
}

#[tauri::command]
pub async fn find_in_page(
    text: String,
    forward: bool,
    pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>,
) -> Result<(), String> {
    pool.lock().unwrap().find_active(&text, forward)
}

#[tauri::command]
pub async fn zoom_set(
    factor: f64,
    host: Option<String>,
    db: State<'_, crate::db::DbState>,
    pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>,
) -> Result<(), String> {
    let clamped = factor.clamp(0.25, 5.0);
    pool.lock().unwrap().zoom_active(clamped)?;
    if let Some(h) = host {
        if !h.is_empty() {
            let key = format!("zoom:{}", h);
            db.execute(move |conn| {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO settings (key, value_json) VALUES (?1, ?2)",
                    rusqlite::params![key, clamped.to_string()],
                );
            });
        }
    }
    Ok(())
}

/// Read the saved per-host zoom factor (P2.3). Returns None when the host has no
/// saved zoom so the frontend can leave the current factor untouched.
#[tauri::command]
pub async fn zoom_get(host: String, db: State<'_, crate::db::DbState>) -> Result<Option<f64>, String> {
    if host.is_empty() {
        return Ok(None);
    }
    let key = format!("zoom:{}", host);
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let v: Option<String> = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get(0),
            )
            .ok();
        let _ = tx.send(v);
    });
    let parsed = rx.recv().unwrap_or(None).and_then(|s| s.trim().trim_matches('"').parse::<f64>().ok());
    Ok(parsed)
}

#[tauri::command]
pub async fn nav_to(
    input: String,
    tab_id: Option<i32>,
    window_id: Option<i32>,
    db: State<'_, crate::db::DbState>,
    pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    use crate::search::{classify_input, InputClassification, get_search_engines, route_query};
    let (tx, rx) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let _ = tx.send(get_search_engines(conn));
    });
    let engines = rx.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;
    let default_engine = crate::search::default_search_url(&app);
    let target = match classify_input(&input) {
        InputClassification::Url(u) => u,
        InputClassification::SearchQuery(q) => route_query(&q, &engines, &default_engine),
    };

    // Route to the CALLING window's active tab (the frontend passes its own
    // activeTabId + windowId). The pool has a single GLOBAL active tab, so
    // relying on it navigated the wrong window when more than one was open (#1).
    if let Some(tid) = tab_id.filter(|&t| t != 0) {
        let res = pool.lock().unwrap().navigate_tab(&db, &app, tid, &target);
        if res.is_ok() {
            return Ok(());
        }
        // Tab no longer exists → fall through to create in the given window.
    }
    crate::tabs::tabs_create_impl(Some(target), Some(false), window_id, &db, &pool, &app)?;
    Ok(())
}

#[tauri::command]
pub async fn bookmark_current_tab(
    url: String,
    title: String,
    db: State<'_, crate::db::DbState>,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let _ = tx.send(crate::db::bookmarks::add_bookmark(conn, &url, &title));
    });
    rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn autostart_enable(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().enable().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn autostart_disable(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().disable().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn autostart_status(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn should_show_on_startup(
    app: AppHandle,
) -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--minimized") {
        return false;
    }
    
    let start_minimized = crate::capture::screenshot::get_setting(&app, "startMinimized")
        .map(|v| v == "true")
        .unwrap_or(false);
    if start_minimized {
        return false;
    }
    true
}

#[tauri::command]
pub async fn tabs_mru_switch(
    forward: bool,
    db: State<'_, crate::db::DbState>,
    pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    let target = pool.lock().unwrap().mru_target(forward);
    if let Some(id) = target {
        crate::tabs::tabs_activate_impl(id, &db, &pool, &app)?;
    }
    Ok(())
}

/// Resolve the overlay webview's host HWND (the wry child window hosting the
/// chrome layer) and store it, then apply the initial window region. Replaces
/// the old 30 Hz mouse watcher: pass-through is now done with SetWindowRgn,
/// updated event-driven from overlay_set_hit_rects / panel-open / content
/// changes (see apply_overlay_region).
fn resolve_overlay_hwnd(window: Window, state: Arc<OverlayState>) {
    #[cfg(target_os = "windows")]
    {
        let label = window.label().to_string();
        let app_handle = window.app_handle().clone();
        if let Some(overlay) = app_handle.get_webview(&label) {
            let state_cb = state.clone();
            let label_cb = label.clone();
            let app_cb = app_handle.clone();
            let _ = overlay.with_webview(move |w| unsafe {
                let controller = w.controller();
                let mut parent_hwnd = windows::Win32::Foundation::HWND::default();
                if controller.ParentWindow(&mut parent_hwnd as *mut _ as *mut _).is_ok() {
                    state_cb
                        .overlay_hwnds
                        .lock()
                        .unwrap()
                        .insert(label_cb.clone(), parent_hwnd.0 as isize);
                    apply_overlay_region(&app_cb, &label_cb);
                }
            });
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, state);
}

/// Wire the pass-through mouse watcher and the content-view resize handler onto a
/// window. Called for every window (main, secondary, incognito) so overlay
/// pass-through and full-window content sizing work everywhere.
pub fn attach_window_plumbing(app: &AppHandle, window: Window) {
    let overlay_state = app.state::<Arc<OverlayState>>().inner().clone();
    let tab_pool = app.state::<Arc<Mutex<crate::engine::pool::TabPool>>>().inner().clone();

    // Install WM_NCHITTEST subclass on the top-level window for resize support on Windows
    #[cfg(target_os = "windows")]
    {
        let hwnd = match window.hwnd() {
            Ok(h) => windows::Win32::Foundation::HWND(h.0),
            Err(_) => windows::Win32::Foundation::HWND::default(),
        };
        if !hwnd.is_invalid() {
            crate::platform::win_window::install_resize_subclass(hwnd);
        }
        // Store the main window's HWND for the self-healing raw-Win32 show/hide
        // fallback (P0.1). Runs at creation on the main thread, so hwnd() is safe.
        if window.label() == "main" && !hwnd.is_invalid() {
            crate::windows::set_main_hwnd(hwnd.0 as isize);
        }
    }

    overlay_state
        .scale_factors
        .lock()
        .unwrap()
        .insert(window.label().to_string(), window.scale_factor().unwrap_or(1.0));

    resolve_overlay_hwnd(window.clone(), overlay_state.clone());

    let resize_window = window.clone();
    let scale_state = overlay_state;
    let scale_label = window.label().to_string();
    let scale_app = app.clone();
    // Debounce flag for the resize-retry thread (P2.1): at most one retry thread
    // in flight per window, so a resize storm doesn't spawn a thread per event.
    let resize_retry_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    window.on_window_event(move |event| {
        match event {
            tauri::WindowEvent::ScaleFactorChanged { scale_factor: new_scale, .. } => {
                scale_state.scale_factors.lock().unwrap().insert(scale_label.clone(), *new_scale);
                apply_overlay_region(&scale_app, &scale_label);
            }
            tauri::WindowEvent::Resized(_) => {
                let rect = crate::windows::content_rect(&resize_window);
                if let Ok(mut pool) = tab_pool.try_lock() {
                    if let Some(active_id) = pool.get_active_tab_id() {
                        pool.resize_tab(active_id, rect);
                    }
                } else if !resize_retry_pending.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    // Pool busy — the try_lock skip is exactly why OS-driven
                    // maximize (Win+Up, snap, title-drag-to-top) left content at
                    // the pre-maximize size (P2.1). Spawn ONE retry thread that
                    // blocks on the lock off-main and recomputes the rect fresh.
                    // resize_tab only calls webview set_size/set_position, which
                    // marshal internally, so running it off the main thread is safe.
                    let pool_c = tab_pool.clone();
                    let win_c = resize_window.clone();
                    let flag = resize_retry_pending.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(60));
                        flag.store(false, std::sync::atomic::Ordering::SeqCst);
                        let rect = crate::windows::content_rect(&win_c);
                        if let Ok(mut pool) = pool_c.lock() {
                            if let Some(active_id) = pool.get_active_tab_id() {
                                pool.resize_tab(active_id, rect);
                            }
                        }
                    });
                }
            }
            // Drag-and-drop install (P1.3.2): a dropped .crx/.zip installs as an
            // extension (with the normal consent dialog inside the command).
            tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                for p in paths {
                    let ext = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
                    if matches!(ext.as_deref(), Some("crx") | Some("zip")) {
                        let app_h = scale_app.clone();
                        let path_str = p.to_string_lossy().to_string();
                        tauri::async_runtime::spawn(async move {
                            let db = app_h.state::<crate::db::DbState>();
                            match crate::extensions::extensions_install_file(path_str, db, app_h.clone()).await {
                                Ok(ext) => { let _ = app_h.emit("toast:show", format!("Installed extension: {}", ext.name)); }
                                Err(e) => { let _ = app_h.emit("toast:show", format!("Extension install failed: {}", e)); }
                            }
                        });
                        break;
                    }
                }
            }
            // Closing the primary window minimizes to the system tray when
            // minimizeToTray is on (default). This keeps tabs/session alive and,
            // crucially, keeps the "main" window ALIVE so tray/summon/relaunch
            // can bring it back (window.close() would destroy it → dead reopen).
            tauri::WindowEvent::CloseRequested { api, .. } if scale_label == "main" => {
                let minimize_to_tray = crate::capture::screenshot::get_setting(&scale_app, "minimizeToTray")
                    .map(|v| v != "false")
                    .unwrap_or(true);
                if minimize_to_tray {
                    api.prevent_close();
                    if let Some(win) = scale_app.get_window("main") {
                        let _ = win.hide();
                    }
                    crate::windows::force_hide_main();
                    crate::windows::on_main_hidden(&scale_app);
                } else {
                    // Full quit: no zombie tray process left behind.
                    scale_app.exit(0);
                }
            }
            _ => {}
        }
    });
}

#[tauri::command]
pub async fn quicklaunch_list(db: State<'_, crate::db::DbState>) -> Result<Vec<crate::ipc_types::QuickLaunchItem>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = crate::db::quick_launch::list_quick_launch(conn);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn quicklaunch_set(item: crate::ipc_types::QuickLaunchItem, db: State<'_, crate::db::DbState>, app: AppHandle) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = crate::db::quick_launch::set_quick_launch(conn, &item);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(())).map_err(|e| e.to_string())?;
    // Re-register so a new plain combo takes effect immediately.
    reregister_all_shortcuts(&app)
}

#[tauri::command]
pub async fn quicklaunch_remove(id: i32, db: State<'_, crate::db::DbState>, app: AppHandle) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = crate::db::quick_launch::remove_quick_launch(conn, id);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(())).map_err(|e| e.to_string())?;
    reregister_all_shortcuts(&app)
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct HotkeyItem {
    pub action: String,
    pub shortcut: String,
}

/// Hot-path caches (P0.2): the global-shortcut handler runs on the plugin's
/// poller thread and must NOT touch the DB on a keypress (that blocking round
/// trip on the hot path was a source of dropped/laggy hotkeys). These are
/// refreshed from the DB only inside `reregister_all_shortcuts`; the handler
/// reads them lock-and-clone.
static HOTKEY_CACHE: Mutex<Vec<HotkeyItem>> = Mutex::new(Vec::new());
static QUICKLAUNCH_CACHE: Mutex<Vec<crate::ipc_types::QuickLaunchItem>> = Mutex::new(Vec::new());
/// Serializes `reregister_all_shortcuts` so a startup registration and a
/// settings-save can't interleave `unregister_all` / `register` and silently
/// drop hotkeys (this raced before).
static REREGISTER_LOCK: Mutex<()> = Mutex::new(());

fn default_hotkeys() -> Vec<HotkeyItem> {
    vec![
        HotkeyItem { action: "summon".to_string(), shortcut: "Ctrl+Shift+Space".to_string() },
        HotkeyItem { action: "palette".to_string(), shortcut: "Ctrl+Alt+Space".to_string() },
        HotkeyItem { action: "screenshot".to_string(), shortcut: "Ctrl+Alt+S".to_string() },
        HotkeyItem { action: "ocr".to_string(), shortcut: "Ctrl+Alt+T".to_string() },
        HotkeyItem { action: "incognito".to_string(), shortcut: "Ctrl+Alt+N".to_string() },
        HotkeyItem { action: "addressbar".to_string(), shortcut: "Ctrl+Alt+L".to_string() },
        HotkeyItem { action: "aichat".to_string(), shortcut: "Ctrl+Alt+A".to_string() },
        HotkeyItem { action: "leader".to_string(), shortcut: "Ctrl+Space".to_string() },
    ]
}

/// The AI chatbot URL to open for the aichat hotkey / new-tab AI button (#7,#13),
/// from the `aiChatUrl` setting, defaulting to ChatGPT.
pub fn ai_chat_url(app: &AppHandle) -> String {
    // `defaultChatbot` is what the wizard writes. It may be a query template
    // (…?q=%s); for the "open the chatbot" hotkey we strip the %s so it lands on
    // a ready-to-type page (e.g. Google AI Mode …&q= , ChatGPT …?q=).
    let raw = crate::capture::screenshot::get_setting(app, "defaultChatbot")
        .filter(|s| s.starts_with("http"))
        .unwrap_or_else(|| "https://chatgpt.com/".to_string());
    raw.replace("%s", "")
}

fn get_hotkeys_sync(db: &crate::db::DbState) -> Vec<HotkeyItem> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = (|| -> rusqlite::Result<Vec<HotkeyItem>> {
            let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key = 'global_hotkeys'")?;
            let opt: Option<String> = stmt.query_row([], |row| row.get::<_, String>(0)).optional()?;
            // Always start from the full default set so every action renders with
            // a value; a saved list that is a subset (or has a blank entry) must
            // never leave a hotkey row empty. Saved non-empty shortcuts override.
            let mut merged = default_hotkeys();
            if let Some(json_str) = opt {
                if let Ok(saved) = serde_json::from_str::<Vec<HotkeyItem>>(&json_str) {
                    for s in saved {
                        if s.shortcut.trim().is_empty() { continue; }
                        if let Some(existing) = merged.iter_mut().find(|d| d.action == s.action) {
                            existing.shortcut = s.shortcut;
                        } else {
                            merged.push(s);
                        }
                    }
                }
            }
            Ok(merged)
        })();
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(Vec::new())).unwrap_or(Vec::new())
}

#[tauri::command]
pub async fn hotkey_list(db: State<'_, crate::db::DbState>) -> Result<Vec<HotkeyItem>, String> {
    Ok(get_hotkeys_sync(&db))
}

#[tauri::command]
pub async fn hotkey_rebind(action: String, shortcut: String, db: State<'_, crate::db::DbState>, app: AppHandle) -> Result<(), String> {
    // Validate the combo parses.
    if shortcut.parse::<Shortcut>().is_err() {
        return Err(format!("'{}' is not a valid shortcut", shortcut));
    }
    let current = get_hotkeys_sync(&db);
    // Conflict with a different action's binding?
    if current.iter().any(|i| i.action != action && i.shortcut.eq_ignore_ascii_case(&shortcut)) {
        return Err(format!("combo '{}' is already in use", shortcut));
    }
    // Conflict with a quick-launch plain combo?
    if get_quick_launch_slots(&db)
        .iter()
        .any(|s| !s.sequence.contains(',') && s.sequence.eq_ignore_ascii_case(&shortcut))
    {
        return Err(format!("combo '{}' is already in use by a quick-launch slot", shortcut));
    }

    let mut updated = current;
    if let Some(item) = updated.iter_mut().find(|i| i.action == action) {
        item.shortcut = shortcut.clone();
    } else {
        updated.push(HotkeyItem { action: action.clone(), shortcut: shortcut.clone() });
    }

    let json_str = serde_json::to_string(&updated).map_err(|e| e.to_string())?;
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn: &mut rusqlite::Connection| {
        let res = conn.execute(
            "INSERT OR REPLACE INTO settings (key, value_json) VALUES ('global_hotkeys', ?1)",
            rusqlite::params![json_str],
        ).map(|_| ());
        let _ = tx.send(res);
    });
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e: rusqlite::Error| e.to_string())?;

    reregister_all_shortcuts(&app)?;
    Ok(())
}

fn get_quick_launch_slots(db: &crate::db::DbState) -> Vec<crate::ipc_types::QuickLaunchItem> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let _ = tx.send(crate::db::quick_launch::list_quick_launch(conn));
    });
    rx.recv().unwrap_or(Ok(Vec::new())).unwrap_or_default()
}

/// Register every global shortcut: action hotkeys plus quick-launch slots whose
/// sequence is a plain combo (no comma = not a leader chord). Refreshes the
/// hot-path caches from the DB, then rebuilds the registration atomically under
/// REREGISTER_LOCK. Registration failures are surfaced via a toast so they are
/// never silent again (P0.2.4).
pub fn reregister_all_shortcuts(app: &AppHandle) -> Result<(), String> {
    // Serialize concurrent callers (startup thread + settings save) so their
    // unregister_all/register can't interleave and drop hotkeys.
    let _guard = REREGISTER_LOCK.lock().unwrap();

    let db = app.state::<crate::db::DbState>();
    let hotkeys = get_hotkeys_sync(&db);
    let slots = get_quick_launch_slots(&db);

    // Refresh caches BEFORE (re)registering so the handler always reads the
    // set that is actually registered.
    *HOTKEY_CACHE.lock().unwrap() = hotkeys.clone();
    *QUICKLAUNCH_CACHE.lock().unwrap() = slots.clone();

    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();

    let mut failures: Vec<String> = Vec::new();

    for item in &hotkeys {
        match item.shortcut.parse::<Shortcut>() {
            Ok(shortcut) => {
                if let Err(e) = global_shortcut.register(shortcut) {
                    tracing::error!("Failed to register '{}': {}", item.shortcut, e);
                    failures.push(item.shortcut.clone());
                }
            }
            Err(_) => {
                tracing::error!("Failed to parse shortcut '{}'", item.shortcut);
                failures.push(item.shortcut.clone());
            }
        }
    }

    for slot in &slots {
        if slot.sequence.contains(',') {
            continue; // leader chord, handled by the low-level hook
        }
        if let Ok(shortcut) = slot.sequence.parse::<Shortcut>() {
            if let Err(e) = global_shortcut.register(shortcut) {
                tracing::error!("Failed to register quick-launch '{}': {}", slot.sequence, e);
                failures.push(slot.sequence.clone());
            }
        }
    }

    if !failures.is_empty() {
        let msg = format!(
            "Hotkey {} couldn't be registered (in use by another app)",
            failures.join(", ")
        );
        let _ = app.emit("toast:show", msg);
    }
    Ok(())
}

/// Background watchdog (P0.2.3): every 60s, re-register any cached shortcut the
/// OS has silently dropped. REGISTER ONLY — never unregister_all here (that is
/// what the reregister path is for) so we can't race a live rebind.
pub fn spawn_hotkey_watchdog(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let global_shortcut = app.global_shortcut();
            let hotkeys = HOTKEY_CACHE.lock().unwrap().clone();
            let slots = QUICKLAUNCH_CACHE.lock().unwrap().clone();
            let mut combos: Vec<String> = hotkeys.into_iter().map(|h| h.shortcut).collect();
            for slot in slots {
                if !slot.sequence.contains(',') {
                    combos.push(slot.sequence);
                }
            }
            for combo in combos {
                if let Ok(shortcut) = combo.parse::<Shortcut>() {
                    if !global_shortcut.is_registered(shortcut) {
                        if let Err(e) = global_shortcut.register(shortcut) {
                            tracing::error!("watchdog: re-register '{}' failed: {}", combo, e);
                        } else {
                            tracing::warn!("watchdog: re-registered dropped hotkey '{}'", combo);
                        }
                    }
                }
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn is_powertoys_run_active() -> bool {
    use std::process::Command;
    use std::os::windows::process::CommandExt;
    if let Ok(output) = Command::new("tasklist").creation_flags(0x08000000).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.contains("PowerToys.Run.exe")
    } else {
        false
    }
}

#[cfg(not(target_os = "windows"))]
fn is_powertoys_run_active() -> bool {
    false
}

pub fn run() {
    // Install the file logger FIRST so any startup failure below is captured.
    // Hold the guard for the whole process — it flushes the non-blocking writer.
    let _log_guard = crate::logging::init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.len() > 1 {
                let arg = args[1].clone();
                if !arg.starts_with("--") {
                    let app_handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = crate::deeplink::handle_open_argument(&app_handle, &arg);
                    });
                } else {
                    crate::windows::ensure_main_window(app);
                }
            } else {
                crate::windows::ensure_main_window(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    // HOT PATH: read the caches only — NEVER touch the DB here
                    // (blocking DB round trips on a keypress caused dropped/laggy
                    // hotkeys, P0.2). Clone out and drop the lock before acting.
                    let matched_item = HOTKEY_CACHE
                        .lock()
                        .unwrap()
                        .iter()
                        .find(|item| item.shortcut.parse::<Shortcut>().map(|s| s == *shortcut).unwrap_or(false))
                        .cloned();

                    // No action hotkey matched -> maybe a quick-launch plain combo.
                    if matched_item.is_none() {
                        let slot = QUICKLAUNCH_CACHE
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|s| {
                                !s.sequence.contains(',')
                                    && s.sequence.parse::<Shortcut>().map(|sc| sc == *shortcut).unwrap_or(false)
                            })
                            .cloned();
                        if let Some(slot) = slot {
                            let app_h = app.clone();
                            std::thread::spawn(move || {
                                let _ = crate::chords::execute_quick_launch_public(app_h, slot);
                            });
                        }
                    }

                    if let Some(item) = matched_item {
                        match item.action.as_str() {
                            "summon" => {
                                // Marshal window ops to the main thread (doing them
                                // on the shortcut-handler thread froze the app).
                                // Decide visible/focused via OS-level truth (raw
                                // Win32), NOT tauri's is_visible() which lies in the
                                // degraded state (§2). Hide via tauri THEN raw
                                // fallback; show via ensure_main_window (which
                                // already carries the raw force_show fallback).
                                let app_h = app.clone();
                                let _ = app.run_on_main_thread(move || {
                                    let visible = crate::windows::main_is_visible();
                                    let focused = crate::windows::main_is_foreground();
                                    if visible && focused {
                                        // get_window (not get_webview_window): the
                                        // latter returns None once tabs are open.
                                        if let Some(win) = app_h.get_window("main") {
                                            let _ = win.hide();
                                        }
                                        crate::windows::force_hide_main();
                                        crate::windows::on_main_hidden(&app_h);
                                    } else {
                                        crate::windows::ensure_main_window(&app_h);
                                    }
                                });
                            }
                            "palette" => {
                                let app_h = app.clone();
                                std::thread::spawn(move || crate::palette::show_palette(&app_h, "search", ""));
                            }
                            "addressbar" => {
                                // Show the main window and open its IN-WINDOW address
                                // bar (not the palette) — that's what "address bar"
                                // means to the user.
                                let app_h = app.clone();
                                let _ = app.run_on_main_thread(move || {
                                    // ensure_main_window carries the raw force_show
                                    // fallback, so the address bar reliably surfaces
                                    // even from the degraded hidden state.
                                    crate::windows::ensure_main_window(&app_h);
                                    // Focus the CHROME overlay first: input.focus()
                                    // inside an unfocused webview is a no-op, which
                                    // is why the address bar opened without a caret.
                                    crate::windows::focus_main_chrome(&app_h);
                                    let _ = app_h.emit("window:shortcut", "Ctrl+L");
                                });
                            }
                            "screenshot" => {
                                let app_h = app.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = crate::capture::capture_trigger(app_h, "screenshot".to_string()).await;
                                });
                            }
                            "ocr" => {
                                let app_h = app.clone();
                                tauri::async_runtime::spawn(async move {
                                    let _ = crate::capture::capture_trigger(app_h, "ocr".to_string()).await;
                                });
                            }
                            "incognito" => {
                                // Never create windows synchronously inside an event
                                // handler on the main thread — spawn to the runtime.
                                let app_h = app.clone();
                                std::thread::spawn(move || {
                                    let _ = crate::windows::window_new_incognito_impl(&app_h);
                                });
                            }
                            "aichat" => {
                                // Show the main window and open the configured AI
                                // chatbot site in a new tab for quick chat/research.
                                let app_h = app.clone();
                                std::thread::spawn(move || {
                                    let url = ai_chat_url(&app_h);
                                    let _ = app_h.run_on_main_thread({
                                        let app_i = app_h.clone();
                                        move || { crate::windows::ensure_main_window(&app_i); }
                                    });
                                    let db = app_h.state::<crate::db::DbState>();
                                    let pool = app_h.state::<Arc<Mutex<crate::engine::pool::TabPool>>>();
                                    let _ = crate::tabs::tabs_create_impl(Some(url), Some(false), Some(1), &db, &pool, &app_h);
                                });
                            }
                            "leader" => {
                                crate::chords::arm_chords(app.clone());
                            }
                            _ => {}
                        }
                    }
                }
            })
            .build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label();
                if label.starts_with("incognito_") {
                    if let Some(pos) = label.rfind('_') {
                        if let Ok(id) = label[pos + 1..].parse::<i32>() {
                            crate::incognito::unregister_incognito_window(id);
                        }
                    }
                }
            }
        })
        .setup(|app| {
            // DB initialization
            let db_tx = crate::db::init_db()?;
            let db_state = crate::db::DbState { sender: db_tx };
            app.manage(db_state.clone());

            // Build the enabled-extensions staging dir before any content webview
            // is created (they read extensions_path at build time) so uBOL etc.
            // load on the first tab (Phase 4).
            // Speed: with full uBO enabled, stacked redundant blockers made every
            // request pay multiple filter passes. Disable them BEFORE staging.
            let deduped = crate::extensions::dedupe_ad_blockers(&db_state);
            if deduped > 0 {
                let app_t = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(6));
                    let _ = app_t.emit(
                        "toast:show",
                        "Disabled redundant ad blockers (full uBlock Origin covers it) — re-enable in Settings if wanted".to_string(),
                    );
                });
            }

            let staged = crate::extensions::rebuild_active_extensions(app.handle(), &db_state);
            tracing::info!("staged {} browser extension(s) for loading", staged);

            // Purge stale PROFILE extensions: AddBrowserExtension persists in the
            // shared WebView2 profile across sessions, so disable/uninstall/dedupe
            // that only touched the DB + staging left every previously-added
            // extension running forever (stacked ad blockers → slow page loads).
            // The chrome webview shares the profile, so sync on it at startup.
            if let Some(wv) = app.webviews().get("main") {
                crate::extensions::sync_profile_extensions(app.handle(), wv);
                // Engine-death recovery (sleep/resume bug): if the WebView2
                // browser process dies, every webview — chrome included — goes
                // permanently dead while the event loop lives on. Watch for it
                // and self-restart (session persists in the DB).
                crate::windows::register_engine_watch(app.handle(), wv);
            }
            crate::windows::register_resume_watch(app.handle());

            // Deeplink registry registration
            crate::deeplink::register_jello_protocol();

            // Store startup command line argument if any
            let args: Vec<String> = std::env::args().collect();
            let startup_arg = if args.len() > 1 && !args[1].starts_with("--") {
                Some(args[1].clone())
            } else {
                None
            };
            app.manage(StartupArgState(Mutex::new(startup_arg)));

            // Create Tab Pool State
            let tab_pool = Arc::new(Mutex::new(crate::engine::pool::TabPool::new()));
            app.manage(tab_pool.clone());

            // Start background idle suspension thread (runs every 30 seconds)
            let tab_pool_bg = tab_pool.clone();
            let db_state_bg = db_state.clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    if let Ok(mut pool) = tab_pool_bg.lock() {
                        pool.suspend_idle(&db_state_bg);
                    }
                }
            });

            // Create Overlay State (hit rects keyed per window label)
            let overlay_state = Arc::new(OverlayState {
                hit_rects: Mutex::new(HashMap::new()),
                overlay_hwnds: Mutex::new(HashMap::new()),
                has_content: Mutex::new(HashMap::new()),
                scale_factors: Mutex::new(HashMap::new()),
            });
            app.manage(overlay_state.clone());

            // Retrieve main window (already created via tauri.conf.json)
            let window = app.get_window("main").unwrap();

            // Restore always-on-top preference.
            if crate::capture::screenshot::get_setting(app.handle(), "pinnedOnTop")
                .map(|v| v == "true")
                .unwrap_or(false)
            {
                let _ = window.set_always_on_top(true);
            }

            // Wire pass-through watcher + resize handler for the main window.
            attach_window_plumbing(app.handle(), window);

            // Reliably show the main window from the main thread (setup runs on
            // it). The frontend also calls show() after should_show_on_startup,
            // but that webview-thread show() is flaky for transparent frameless
            // windows and sometimes left the window hidden. Skip only when the
            // user opted into start-minimized / --minimized.
            {
                let args: Vec<String> = std::env::args().collect();
                let start_minimized = args.iter().any(|a| a == "--minimized")
                    || crate::capture::screenshot::get_setting(app.handle(), "startMinimized")
                        .map(|v| v == "true")
                        .unwrap_or(false);
                if !start_minimized {
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.set_size(tauri::LogicalSize::new(800.0, 600.0));
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                }
            }

            let app_h = app.handle().clone();
            std::thread::spawn(move || {
                // System tray icon.
                if let Err(e) = crate::tray::setup_tray(&app_h) {
                    tracing::error!("Failed to set up tray: {}", e);
                }

                // Register global hotkeys + quick-launch plain combos from DB.
                if let Err(e) = reregister_all_shortcuts(&app_h) {
                    tracing::error!("Failed to reregister shortcuts: {}", e);
                }

                // Self-healing watchdog: re-register any dropped hotkey within 60s
                // (starts AFTER the initial reregister has populated the caches).
                spawn_hotkey_watchdog(&app_h);

                // Consent-gated 24h update check.
                crate::updater::spawn_periodic_check(&app_h);
            });

            // Trigger PowerToys warning check after 2 seconds
            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if is_powertoys_run_active() {
                    let _ = app_handle.emit("toast:show", "PowerToys Run detected. Potential hotkey conflict!".to_string());
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            test_command,
            process_startup_arg,
            overlay_set_hit_rects,
            overlay_set_panel_open,
            overlay_set_interactive,
            autostart_enable,
            autostart_disable,
            autostart_status,
            should_show_on_startup,
            window_controls,
            window_set_pinned,
            window_pinned_state,
            nav_back,
            nav_forward,
            nav_reload,
            nav_stop,
            nav_to,
            find_in_page,
            zoom_set,
            zoom_get,
            bookmark_current_tab,
            tabs_mru_switch,
            crate::windows::window_new,
            crate::windows::window_new_incognito,
            crate::tabs::tabs_list,
            crate::tabs::tabs_create,
            crate::tabs::tabs_activate,
            crate::tabs::tabs_close,
            crate::tabs::tabs_reorder,
            crate::tabs::tabs_set_pinned,
            crate::tabs::tabs_set_muted,
            crate::tabs::tabs_duplicate,
            crate::tabs::tabs_suspend_all,
            crate::tabs::tabs_reopen_closed,
            crate::tabs::tabs_unload,
            crate::tabs::tabs_loaded_states,
            crate::extensions::extensions_list,
            crate::extensions::extensions_install,
            crate::extensions::extensions_set_enabled,
            crate::extensions::extensions_install_ubo,
            crate::extensions::extensions_uninstall,
            crate::extensions::extensions_open_options,
            crate::extensions::extensions_restart_app,
            crate::extensions::extensions_install_file,
            crate::extensions::extensions_install_file_dialog,
            crate::palette::palette_show,
            crate::palette::palette_hide,
            crate::palette::palette_resize,
            crate::palette::palette_query,
            crate::palette::palette_open,
            quicklaunch_list,
            quicklaunch_set,
            quicklaunch_remove,
            hotkey_list,
            hotkey_rebind,
            crate::capture::capture_trigger,
            crate::capture::capture_crop,
            crate::capture::capture_ocr_action,
            crate::capture::capture_cancel,
            crate::data_cmds::history_search,
            crate::data_cmds::history_delete,
            crate::data_cmds::bookmarks_list,
            crate::data_cmds::bookmarks_add,
            crate::data_cmds::bookmarks_update,
            crate::data_cmds::bookmarks_remove,
            crate::data_cmds::settings_get,
            crate::data_cmds::settings_set,
            crate::sessions::session_restore_last,
            crate::engine::webview2::download_pause,
            crate::engine::webview2::download_resume,
            crate::engine::webview2::download_cancel,
            crate::engine::webview2::download_reveal,
            crate::updater::updater_check,
            crate::updater::updater_apply,
            crate::updater::updater_enabled,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building jello application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            crate::sessions::on_exit(app_handle);
        }
    });
}
