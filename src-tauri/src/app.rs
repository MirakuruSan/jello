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
        let panel_open = crate::engine::webview2::PANEL_OPEN.load(std::sync::atomic::Ordering::Relaxed)
            || OVERLAY_INTERACTIVE.load(std::sync::atomic::Ordering::Relaxed);
        let has_content = state.has_content.lock().unwrap().get(label).copied().unwrap_or(false);
        let full = panel_open || !has_content;
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
pub fn window_controls(action: String, window: Window) -> Result<(), String> {
    match action.as_str() {
        "min" => window.minimize().map_err(|e| e.to_string()),
        "max" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize().map_err(|e| e.to_string())
            } else {
                window.maximize().map_err(|e| e.to_string())
            }
        }
        "close" => window.close().map_err(|e| e.to_string()),
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
pub async fn nav_reload(pool: State<'_, Arc<Mutex<crate::engine::pool::TabPool>>>) -> Result<(), String> {
    pool.lock().unwrap().nav_reload()
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

#[tauri::command]
pub async fn nav_to(
    input: String,
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
    let target = match classify_input(&input) {
        InputClassification::Url(u) => u,
        InputClassification::SearchQuery(q) => route_query(&q, &engines, "https://duckduckgo.com/?q=%s"),
    };

    let active = pool.lock().unwrap().get_active_tab_id();
    match active {
        Some(tid) => pool.lock().unwrap().navigate_tab(&db, &app, tid, &target),
        None => {
            crate::tabs::tabs_create_impl(Some(target), Some(false), None, &db, &pool, &app)?;
            Ok(())
        }
    }
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
                    if let Some(win) = scale_app.get_webview_window("main") {
                        let _ = win.hide();
                    }
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

fn get_hotkeys_sync(db: &crate::db::DbState) -> Vec<HotkeyItem> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = (|| -> rusqlite::Result<Vec<HotkeyItem>> {
            let mut stmt = conn.prepare("SELECT value_json FROM settings WHERE key = 'global_hotkeys'")?;
            let opt: Option<String> = stmt.query_row([], |row| row.get::<_, String>(0)).optional()?;
            if let Some(json_str) = opt {
                if let Ok(items) = serde_json::from_str::<Vec<HotkeyItem>>(&json_str) {
                    return Ok(items);
                }
            }
            Ok(vec![
                HotkeyItem { action: "summon".to_string(), shortcut: "Ctrl+Shift+Space".to_string() },
                HotkeyItem { action: "palette".to_string(), shortcut: "Ctrl+Alt+Space".to_string() },
                HotkeyItem { action: "screenshot".to_string(), shortcut: "Ctrl+Alt+S".to_string() },
                HotkeyItem { action: "ocr".to_string(), shortcut: "Ctrl+Alt+T".to_string() },
                HotkeyItem { action: "incognito".to_string(), shortcut: "Ctrl+Alt+N".to_string() },
                HotkeyItem { action: "addressbar".to_string(), shortcut: "Ctrl+Alt+L".to_string() },
                HotkeyItem { action: "leader".to_string(), shortcut: "Ctrl+Space".to_string() },
            ])
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
/// sequence is a plain combo (no comma = not a leader chord).
pub fn reregister_all_shortcuts(app: &AppHandle) -> Result<(), String> {
    let global_shortcut = app.global_shortcut();
    let _ = global_shortcut.unregister_all();

    let db = app.state::<crate::db::DbState>();
    for item in get_hotkeys_sync(&db) {
        match item.shortcut.parse::<Shortcut>() {
            Ok(shortcut) => {
                if let Err(e) = global_shortcut.register(shortcut) {
                    tracing::error!("Failed to register '{}': {}", item.shortcut, e);
                }
            }
            Err(_) => tracing::error!("Failed to parse shortcut '{}'", item.shortcut),
        }
    }

    for slot in get_quick_launch_slots(&db) {
        if slot.sequence.contains(',') {
            continue; // leader chord, handled by the low-level hook
        }
        if let Ok(shortcut) = slot.sequence.parse::<Shortcut>() {
            if let Err(e) = global_shortcut.register(shortcut) {
                tracing::error!("Failed to register quick-launch '{}': {}", slot.sequence, e);
            }
        }
    }
    Ok(())
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
                    let db = app.state::<crate::db::DbState>();
                    let items = get_hotkeys_sync(&db);
                    
                    let matched_item = items.iter().find(|item| {
                        if let Ok(s) = item.shortcut.parse::<Shortcut>() {
                            s == *shortcut
                        } else {
                            false
                        }
                    });
                    
                    // No action hotkey matched -> maybe a quick-launch plain combo.
                    if matched_item.is_none() {
                        let slots = get_quick_launch_slots(&db);
                        if let Some(slot) = slots.into_iter().find(|s| {
                            !s.sequence.contains(',')
                                && s.sequence.parse::<Shortcut>().map(|sc| sc == *shortcut).unwrap_or(false)
                        }) {
                            let app_h = app.clone();
                            std::thread::spawn(move || {
                                let _ = crate::chords::execute_quick_launch_public(app_h, slot);
                            });
                        }
                    }

                    if let Some(item) = matched_item {
                        match item.action.as_str() {
                            "summon" => {
                                // Toggle: hide if visible, else bring back (recreating if needed).
                                match app.get_window("main") {
                                    Some(win) if win.is_visible().unwrap_or(false) => {
                                        let _ = win.hide();
                                    }
                                    _ => {
                                        crate::windows::ensure_main_window(app);
                                    }
                                }
                            }
                            "palette" => {
                                let app_h = app.clone();
                                std::thread::spawn(move || crate::palette::show_palette(&app_h, "search", ""));
                            }
                            "addressbar" => {
                                // Summon the main window, then open the palette in
                                // address-bar mode (Enter navigates the current tab).
                                crate::windows::ensure_main_window(app);
                                let app_h = app.clone();
                                std::thread::spawn(move || crate::palette::show_palette(&app_h, "addressbar", ""));
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
            let staged = crate::extensions::rebuild_active_extensions(app.handle(), &db_state);
            tracing::info!("staged {} browser extension(s) for loading", staged);

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
            crate::extensions::extensions_list,
            crate::extensions::extensions_install,
            crate::extensions::extensions_set_enabled,
            crate::extensions::extensions_install_ubol,
            crate::extensions::extensions_uninstall,
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
