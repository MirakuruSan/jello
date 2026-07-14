use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder, Manager};

/// The full logical client rect of a window — content webviews fill the whole
/// window (chromeless: all UI floats above the page). SPEC §F.
pub fn content_rect(window: &tauri::Window) -> crate::engine::Rect {
    let scale = window.scale_factor().unwrap_or(1.0);
    let size = window
        .inner_size()
        .unwrap_or(tauri::PhysicalSize::new(800, 600));
    crate::engine::Rect {
        x: 0,
        y: 0,
        width: (size.width as f64 / scale).round() as i32,
        height: (size.height as f64 / scale).round() as i32,
    }
}

/// Return the primary "main" window, showing/unminimizing/focusing it. The
/// window is normally hidden (never destroyed) on close, so this reliably brings
/// it back for tray-click, summon, and single-instance relaunch. If it has
/// somehow been destroyed, recreate it with the startup options so reopen can
/// never silently no-op (past bug: window.close() killed it → dead reopen).
pub fn ensure_main_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return Some(win);
    }
    // Defensive recreate (mirrors tauri.conf.json "main" window options).
    match WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Jello")
        .inner_size(800.0, 600.0)
        .min_inner_size(480.0, 360.0)
        .decorations(false)
        .transparent(true)
        .visible(true)
        // Uniform with the config-defined main window + content webviews so all
        // app-data-dir-sharing webviews agree on the extensions setting (Phase 4).
        .browser_extensions_enabled(true)
        .build()
    {
        Ok(win) => {
            crate::app::attach_window_plumbing(app, win.as_ref().window());
            let _ = win.set_focus();
            Some(win)
        }
        Err(e) => {
            tracing::error!("Failed to recreate main window: {}", e);
            None
        }
    }
}

pub fn create_palette_window(app: &AppHandle) -> Result<tauri::WebviewWindow, Box<dyn std::error::Error>> {
    // Slim single-composer pill (Alt+Space style). Starts at just the input
    // height; grows via palette_resize as results appear.
    let window = WebviewWindowBuilder::new(
        app,
        "palette",
        WebviewUrl::App("index.html#/palette".into()),
    )
    .inner_size(680.0, 60.0)
    .title("Jello Palette")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .visible(false)
    .browser_extensions_enabled(true)
    .build()?;

    // Apply Win11 Acrylic backdrop
    #[cfg(target_os = "windows")]
    crate::platform::win_window::apply_acrylic(&window);

    Ok(window)
}

pub fn window_new_impl(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
    let label = format!("main_{}", id);
    
    let db = app.state::<crate::db::DbState>();
    let created_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let dummy_tab = crate::ipc_types::Tab {
        id: 0,
        window_id: id,
        url: "about:blank".to_string(),
        title: None,
        favicon_id: None,
        pinned: false,
        muted: false,
        order_key: "a".to_string(),
        scroll_y: 0.0,
        last_active: Some(created_at),
        created_at,
    };
    
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = crate::db::tabs_repo::insert_tab(conn, &dummy_tab);
        let _ = tx.send(res);
    });
    let _ = rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows)).map_err(|e| e.to_string())?;

    // Build + show the window ON THE MAIN THREAD. Creating it from the async
    // command thread produced a broken window (0x0, then created-but-hidden).
    let (tx2, rx2) = std::sync::mpsc::channel();
    let app_m = app.clone();
    let label_m = label.clone();
    app.run_on_main_thread(move || {
        let res = (|| -> Result<(), String> {
            let built = WebviewWindowBuilder::new(
                &app_m,
                &label_m,
                WebviewUrl::App("index.html".into()),
            )
            .inner_size(800.0, 600.0)
            .title("Jello")
            .decorations(false)
            .transparent(true)
            // Explicit position (not .center(), which scattered the window onto
            // whichever monitor and read as "didn't open") — cascade near the
            // top-left of the primary monitor.
            .position(140.0, 120.0)
            .browser_extensions_enabled(true)
            .build()
            .map_err(|e| e.to_string())?;
            if let Some(win) = app_m.get_window(&label_m) {
                crate::app::attach_window_plumbing(&app_m, win);
            }
            // The frameless window comes up 0x0 because it's shown before the
            // WM_NCCALCSIZE subclass exists; now that plumbing installed it,
            // force a resize so the client area recomputes to the real size.
            let _ = built.set_size(tauri::LogicalSize::new(800.0, 600.0));
            let _ = built.show();
            let _ = built.set_focus();
            Ok(())
        })();
        let _ = tx2.send(res);
    }).map_err(|e| e.to_string())?;
    rx2.recv().unwrap_or(Ok(()))?;

    Ok(())
}

/// Async command: window creation must not run synchronously inside the
/// WebView2 IPC event handler on the main thread (deadlocks — see tabs.rs).
#[tauri::command]
pub async fn window_new(app: AppHandle) -> Result<(), String> {
    window_new_impl(&app)
}

pub fn window_new_incognito_impl(app: &AppHandle) -> Result<(), String> {
    let app = app.clone();
    let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
    let label = format!("incognito_{}", id);
    
    crate::incognito::add_incognito_tab(id, "about:blank".to_string());

    // Same main-thread build + force-resize + show as window_new_impl.
    let (tx2, rx2) = std::sync::mpsc::channel();
    let app_m = app.clone();
    let label_m = label.clone();
    app.run_on_main_thread(move || {
        let res = (|| -> Result<(), String> {
            let built = WebviewWindowBuilder::new(
                &app_m,
                &label_m,
                WebviewUrl::App("index.html?incognito=true".into()),
            )
            .inner_size(800.0, 600.0)
            .title("Jello (Incognito)")
            .decorations(false)
            .transparent(true)
            .position(160.0, 140.0)
            .incognito(true)
            .build()
            .map_err(|e| e.to_string())?;
            if let Some(win) = app_m.get_window(&label_m) {
                crate::app::attach_window_plumbing(&app_m, win);
            }
            let _ = built.set_size(tauri::LogicalSize::new(800.0, 600.0));
            let _ = built.show();
            let _ = built.set_focus();
            Ok(())
        })();
        let _ = tx2.send(res);
    }).map_err(|e| e.to_string())?;
    rx2.recv().unwrap_or(Ok(()))?;

    Ok(())
}

#[tauri::command]
pub async fn window_new_incognito(app: AppHandle) -> Result<(), String> {
    window_new_incognito_impl(&app)
}
