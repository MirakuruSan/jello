use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder, Manager};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// Raw Win32 HWND of the "main" window, stored at creation on the main thread
/// (`window.hwnd()` is safe there). Used by the self-healing show/hide helpers
///: in the degraded runtime state tauri's own visibility plumbing can
/// lie or no-op, so we drive `ShowWindow` on this HWND directly as the
/// authoritative fallback. 0 = not yet resolved.
pub static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// Record the main window's HWND (called from attach_window_plumbing on the
/// main thread, and on defensive recreate).
pub fn set_main_hwnd(hwnd: isize) {
    MAIN_HWND.store(hwnd, Ordering::SeqCst);
}

/// Raw-Win32 show of the main window — bypasses tauri entirely so it works even
/// when tao's internal window map/flags have desynced from reality and
/// `win.show()` silently no-ops. Restores if minimized, shows if
/// hidden, then foregrounds. No-op if the HWND isn't resolved yet.
pub fn force_show_main() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
        };
        let raw = MAIN_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return;
        }
        unsafe {
            let h = HWND(raw as *mut _);
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            if !IsWindowVisible(h).as_bool() {
                let _ = ShowWindow(h, SW_SHOW);
            }
            let _ = SetForegroundWindow(h);
        }
    }
}

/// Raw-Win32 hide of the main window — the counterpart to force_show_main, so
/// tao's flag and on-screen reality can't diverge in the hide direction either.
pub fn force_hide_main() {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindow, SW_HIDE};
        let raw = MAIN_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return;
        }
        unsafe {
            let h = HWND(raw as *mut _);
            if IsWindowVisible(h).as_bool() {
                let _ = ShowWindow(h, SW_HIDE);
            }
        }
    }
}

// ── Engine health: sleep/resume + browser-process-death recovery (bug: after
// the PC sleeps with Jello in the tray, Jello neither works nor shows until a
// full manual restart — the WebView2 browser process can die across a sleep,
// leaving EVERY webview, including the chrome overlay, permanently dead while
// the tao event loop keeps running). Two independent detectors, one recovery:
// restart the process (session/tabs persist in the DB).

/// The WebView2 browser process id, captured at startup from the main webview.
static BROWSER_PID: AtomicIsize = AtomicIsize::new(0);

/// Restart Jello to recover from a dead WebView2 engine. Saves the session
/// first; tabs/history persist in the DB, so this is a clean self-heal.
fn restart_after_engine_death(app: &AppHandle) {
    tracing::error!("WebView2 engine is dead — restarting Jello to recover");
    crate::sessions::on_exit(app);
    app.restart();
}

/// Detector 1: the official `ProcessFailed` event. Registered on the main
/// chrome webview (the browser process is shared, so one registration sees a
/// browser-process exit). Renderer crashes are left to WebView2's own sad-tab
/// handling; only an engine-level death triggers the restart.
pub fn register_engine_watch(app: &AppHandle, webview: &tauri::Webview<tauri::Wry>) {
    #[cfg(target_os = "windows")]
    {
        let app_h = app.clone();
        let _ = webview.with_webview(move |w| unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
            };
            use webview2_com::ProcessFailedEventHandler;
            let core = match w.controller().CoreWebView2() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("engine watch: CoreWebView2 unavailable: {:?}", e);
                    return;
                }
            };
            let mut pid = 0u32;
            if core.BrowserProcessId(&mut pid).is_ok() && pid != 0 {
                BROWSER_PID.store(pid as isize, Ordering::SeqCst);
            }
            let handler = ProcessFailedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                    let _ = args.ProcessFailedKind(&mut kind);
                    tracing::error!("webview2 ProcessFailed: kind={:?}", kind);
                    if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
                        let app_r = app_h.clone();
                        // Never restart from inside the COM event handler.
                        std::thread::spawn(move || restart_after_engine_death(&app_r));
                    }
                }
                Ok(())
            }));
            let mut token: i64 = 0;
            if let Err(e) = core.add_ProcessFailed(&handler, &mut token as *mut i64) {
                tracing::warn!("engine watch: add_ProcessFailed failed: {:?}", e);
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, webview);
}

#[cfg(target_os = "windows")]
fn process_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let mut code = 0u32;
                let alive = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
                let _ = CloseHandle(h);
                alive
            }
            Err(_) => false,
        }
    }
}

#[cfg(target_os = "windows")]
static RESUME_APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// Detector 2: system resume. If the browser process was killed while the PC
/// slept (ProcessFailed can be missed/undelivered across a suspend), verify it
/// is still alive shortly after resume and restart if not. Also re-register the
/// global hotkeys immediately — the OS can silently drop them across a sleep
/// (the watchdog would catch it, but only within a minute).
#[cfg(target_os = "windows")]
unsafe extern "system" fn on_power_event(
    _context: *const core::ffi::c_void,
    event_type: u32,
    _setting: *const core::ffi::c_void,
) -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};
    if event_type == PBT_APMRESUMEAUTOMATIC || event_type == PBT_APMRESUMESUSPEND {
        if let Some(app) = RESUME_APP.get() {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                tracing::info!("system resumed from sleep — re-registering hotkeys, checking engine");
                if let Err(e) = crate::app::reregister_all_shortcuts(&app) {
                    tracing::warn!("resume: hotkey re-registration failed: {}", e);
                }
                let pid = BROWSER_PID.load(Ordering::SeqCst);
                if pid != 0 && !process_alive(pid as u32) {
                    restart_after_engine_death(&app);
                }
            });
        }
    }
    0
}

/// Subscribe to suspend/resume notifications (works for a tray app with no
/// visible window — no WM_POWERBROADCAST needed).
pub fn register_resume_watch(app: &AppHandle) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Power::{
            RegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
        };
        use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
        let _ = RESUME_APP.set(app.clone());
        // Leaked on purpose: the subscription lives for the process lifetime.
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power_event),
            Context: std::ptr::null_mut(),
        }));
        unsafe {
            match RegisterSuspendResumeNotification(
                HANDLE(params as *mut _ as *mut core::ffi::c_void),
                DEVICE_NOTIFY_CALLBACK,
            ) {
                Ok(_) => tracing::info!("suspend/resume notifications registered"),
                Err(e) => tracing::warn!("RegisterSuspendResumeNotification failed: {:?}", e),
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = app;
}

/// OS-level truth: is the main window actually visible right now? Reads
/// `IsWindowVisible` on the stored HWND rather than trusting tauri's
/// `is_visible()` (which can report true while the window is hidden — see §2).
pub fn main_is_visible() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
        let raw = MAIN_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return false;
        }
        unsafe { IsWindowVisible(HWND(raw as *mut _)).as_bool() }
    }
    #[cfg(not(target_os = "windows"))]
    false
}

/// OS-level truth: is the main window the foreground window?
pub fn main_is_foreground() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let raw = MAIN_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return false;
        }
        unsafe { GetForegroundWindow() == HWND(raw as *mut _) }
    }
    #[cfg(not(target_os = "windows"))]
    false
}

/// Move keyboard focus into the main window's CHROME overlay webview. Bringing
/// the Win32 window to the foreground does NOT focus any WebView2 controller,
/// so after summon the user could see the window but typing went nowhere until
/// a click. `input.focus()` in an unfocused webview is likewise a no-op — the
/// addressbar hotkey needs this before emitting Ctrl+L.
pub fn focus_main_chrome(app: &AppHandle) {
    focus_chrome(app, "main");
}

/// Focus any window's chrome overlay webview by label (works for secondary and
/// incognito windows too).
pub fn focus_chrome(app: &AppHandle, label: &str) {
    #[cfg(target_os = "windows")]
    if let Some(overlay) = app.get_webview(label) {
        let _ = overlay.with_webview(|w| {
            use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
            unsafe {
                let _ = w.controller().MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, label);
}

/// Focus the active content tab's webview if one exists, else the chrome
/// overlay — so a summoned window is immediately usable with the keyboard.
pub fn focus_main_content_or_chrome(app: &AppHandle) {
    let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
    let focused_content = pool
        .try_lock()
        .ok()
        .and_then(|p| p.get_active_tab_id().map(|id| { p.focus_tab(id); true }))
        .unwrap_or(false);
    if !focused_content {
        focus_main_chrome(app);
    }
}

/// Called whenever the main window hides (close-to-tray, summon-hide). Two-stage
/// memory trim: immediately hint Chromium to a LOW memory target on all views
/// (does NOT freeze JS or pause media — safe for playing audio, downloads, and
/// in-page tasks), and after 60s still-hidden, TrySuspend background tabs (the
/// same freeze the 5-minute idle path already applies; active + audio-playing
/// tabs are skipped by suspend_all).
pub fn on_main_hidden(app: &AppHandle) {
    let pool_state = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
    if let Ok(pool) = pool_state.try_lock() {
        pool.set_memory_target_all(true);
    }
    pip_active_tab(app, crate::engine::PipRequest::Auto);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if main_is_visible() {
            return; // was shown again — leave everything running
        }
        let db = app.state::<crate::db::DbState>().inner().clone();
        let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>().inner().clone();
        if let Ok(mut p) = pool.lock() {
            p.suspend_all(&db, 1);
        };
    });
}

/// Bring a window to the foreground even while another app owns it. Windows
/// ignores SetForegroundWindow from a background process unless the calling
/// thread shares input state with the current foreground thread, so attach to
/// it for the duration of the call. Main thread only.
pub fn force_foreground(window: &tauri::Window) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = window.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
        use windows::Win32::UI::WindowsAndMessaging::{
            BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
        };
        unsafe {
            let target = HWND(hwnd.0);
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = BringWindowToTop(target);
            let _ = SetForegroundWindow(target);
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window.set_focus();
}

/// Picture-in-picture "Back to tab": restore Jello (from minimized or the tray)
/// and switch to the tab the video belongs to.
pub fn return_from_pip(app: &AppHandle, tab_id: i32) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        ensure_main_window(&app2);
    });
    let db = app.state::<crate::db::DbState>();
    let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>();
    if let Err(e) = crate::tabs::tabs_activate_impl(tab_id, &db, &pool, app) {
        tracing::warn!("returning to tab {} from picture-in-picture failed: {}", tab_id, e);
    }
}

/// Placements of windows that are fullscreen right now, keyed by HWND. A
/// window that goes fullscreen while maximized keeps its maximized flag in
/// Tauri, which then clamps the page to the work area (a band is left where the
/// taskbar would be) and on the way out leaves the window covering the whole
/// monitor. Jello restores the window on the way in and re-maximizes it on the
/// way out.
#[cfg(target_os = "windows")]
static PRE_FULLSCREEN: std::sync::Mutex<Vec<(isize, windows::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT)>> =
    std::sync::Mutex::new(Vec::new());

/// Main thread, before Tauri processes the switch to fullscreen.
fn remember_placement(window: &tauri::Window) {
    #[cfg(target_os = "windows")]
    if let Ok(h) = window.hwnd() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, IsZoomed, ShowWindow, SW_RESTORE, WINDOWPLACEMENT};
        let hwnd = HWND(h.0);
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_ok() {
            let mut saved = PRE_FULLSCREEN.lock().unwrap();
            if !saved.iter().any(|(k, _)| *k == h.0 as isize) {
                saved.push((h.0 as isize, wp));
            }
        }
        // A real restore, not just dropping the style bit: Tauri re-applies
        // maximize from its own state, which it updates on WM_SIZE.
        unsafe {
            if IsZoomed(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

/// After the window has left fullscreen: one that was maximized before is
/// maximized again, with its original restore size.
fn restore_placement(window: &tauri::Window) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPlacement, ShowWindow, SW_RESTORE, SW_SHOWMAXIMIZED};
        let Ok(h) = window.hwnd() else { return };
        let saved = {
            let mut list = PRE_FULLSCREEN.lock().unwrap();
            let pos = list.iter().position(|(k, _)| *k == h.0 as isize);
            pos.map(|i| list.remove(i).1)
        };
        let Some(wp) = saved else { return };
        if wp.showCmd != SW_SHOWMAXIMIZED.0 as u32 {
            return;
        }
        // Queue behind Tauri's own exit-fullscreen work on the event loop.
        let w = window.clone();
        std::thread::spawn(move || {
            let w2 = w.clone();
            let _ = w.run_on_main_thread(move || {
                if let Ok(h) = w2.hwnd() {
                    unsafe {
                        let hwnd = HWND(h.0);
                        let _ = ShowWindow(hwnd, SW_RESTORE);
                        let _ = SetWindowPlacement(hwnd, &wp);
                    }
                }
            });
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = window;
}

/// A page entered or left fullscreen (main thread, from the content webview).
/// Tauri resizes the window; the toolbar layer hides while a page is fullscreen.
pub fn on_page_fullscreen(window: &tauri::Window, on: bool) {
    use tauri::Emitter;
    if on {
        remember_placement(window);
    } else {
        restore_placement(window);
    }
    let _ = window.emit_to(window.label(), "page:fullscreen", on);
}

/// F11: toggle fullscreen for the calling window.
#[tauri::command]
pub async fn window_toggle_fullscreen(window: tauri::Window) -> Result<(), String> {
    let on = !window.is_fullscreen().unwrap_or(false);
    let w = window.clone();
    window
        .run_on_main_thread(move || {
            if on {
                remember_placement(&w);
            }
            let _ = w.set_fullscreen(on);
            if !on {
                restore_placement(&w);
            }
        })
        .map_err(|e| e.to_string())
}

/// Move the window, or resize it from its top edge, starting from a mousedown
/// in the toolbar layer at (`x`, `y`) in CSS pixels. Windows anchors the move
/// at that point rather than wherever the cursor has got to by the time this
/// command runs, so the window doesn't slip under the cursor.
#[tauri::command]
pub async fn window_start_drag(window: tauri::Window, x: f64, y: f64, edge: Option<String>) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let scale = window.scale_factor().unwrap_or(1.0);
        let w = window.clone();
        window
            .run_on_main_thread(move || {
                use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_LBUTTON};
                use windows::Win32::UI::WindowsAndMessaging::{
                    IsZoomed, PostMessageW, HTCAPTION, HTTOP, HTTOPLEFT, HTTOPRIGHT, WM_NCLBUTTONDOWN,
                };
                let Ok(h) = w.hwnd() else { return };
                let hwnd = HWND(h.0);
                unsafe {
                    // A quick click is already over: don't start a move that
                    // would follow the cursor with no button held.
                    if (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) == 0 {
                        return;
                    }
                    let mut pt = POINT { x: (x * scale).round() as i32, y: (y * scale).round() as i32 };
                    let _ = ClientToScreen(hwnd, &mut pt);
                    let hit = if IsZoomed(hwnd).as_bool() {
                        HTCAPTION
                    } else {
                        match edge.as_deref() {
                            Some("top") => HTTOP,
                            Some("top-left") => HTTOPLEFT,
                            Some("top-right") => HTTOPRIGHT,
                            _ => HTCAPTION,
                        }
                    };
                    let _ = ReleaseCapture();
                    let lparam = ((pt.y as u32 & 0xffff) << 16) | (pt.x as u32 & 0xffff);
                    let _ = PostMessageW(Some(hwnd), WM_NCLBUTTONDOWN, WPARAM(hit as usize), LPARAM(lparam as isize));
                }
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y, edge);
        window.start_dragging().map_err(|e| e.to_string())
    }
}

/// Why the main window's pages are resting: minimized, or nobody can see the
/// window (platform::win_occlusion). `MAIN_HIDDEN` is what was last applied.
static MAIN_MINIMIZED: AtomicBool = AtomicBool::new(false);
static MAIN_OCCLUDED: AtomicBool = AtomicBool::new(false);
static MAIN_HIDDEN: AtomicBool = AtomicBool::new(false);
static MAIN_HIDDEN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The main window was minimized or restored. Minimizing floats a playing video
/// (auto picture-in-picture) and, a moment later, once the video has floated,
/// hides the page from the engine and trims memory. Restoring undoes both.
pub fn on_main_minimized(app: &AppHandle, minimized: bool) {
    let req = if minimized { crate::engine::PipRequest::Auto } else { crate::engine::PipRequest::ExitAuto };
    pip_active_tab(app, req);
    let app = app.clone();
    std::thread::spawn(move || {
        if minimized {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let still_minimized = app.get_window("main").and_then(|w| w.is_minimized().ok()).unwrap_or(false);
            if !still_minimized {
                return;
            }
        }
        MAIN_MINIMIZED.store(minimized, Ordering::SeqCst);
        apply_main_hidden(&app);
    });
}

/// Nobody can see the main window any more (covered, on another desktop,
/// hidden, locked screen), or someone can again. Its pages rest meanwhile,
/// as while minimized, but no video is floated: the user put something else
/// in front on purpose.
pub fn on_main_occluded(app: &AppHandle, occluded: bool) {
    MAIN_OCCLUDED.store(occluded, Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || apply_main_hidden(&app));
}

/// Rest or wake the main window's pages: the visible page is hidden from the
/// engine and every view trims its memory, and the toolbar layer's timers
/// throttle and it stops painting.
fn apply_main_hidden(app: &AppHandle) {
    let _serial = MAIN_HIDDEN_LOCK.lock();
    let hidden = MAIN_MINIMIZED.load(Ordering::SeqCst) || MAIN_OCCLUDED.load(Ordering::SeqCst);
    if MAIN_HIDDEN.swap(hidden, Ordering::SeqCst) == hidden {
        return;
    }
    tracing::info!("main window pages {}", if hidden { "resting" } else { "shown" });
    let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>().inner().clone();
    if let Ok(p) = pool.lock() {
        p.set_window_hidden(hidden);
    };
    #[cfg(target_os = "windows")]
    if let Some(overlay) = app.get_webview("main") {
        let _ = overlay.with_webview(move |w| unsafe {
            let _ = w.controller().SetIsVisible(!hidden);
        });
    }
}

/// Send a picture-in-picture request to the active tab without blocking the
/// caller (window events run on the main thread; the pool may be busy).
pub fn pip_active_tab(app: &AppHandle, req: crate::engine::PipRequest) {
    // The setting gates floating a video automatically, never bringing back
    // one Jello already floated.
    if req == crate::engine::PipRequest::Auto && !crate::prefs::auto_pip() {
        return;
    }
    let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>().inner().clone();
    std::thread::spawn(move || {
        if let Ok(p) = pool.lock() {
            let _ = p.pip_active(req);
        };
    });
}

/// The full logical client rect of a window — content webviews fill the whole
/// window (chromeless: all UI floats above the page).
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
/// never silently no-op.
pub fn ensure_main_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    // IMPORTANT: use get_window, NOT get_webview_window. Once content tabs are
    // added as child webviews (window.add_child), the main window is no longer a
    // 1:1 WebviewWindow and get_webview_window("main") returns None — which made
    // this fall through to a doomed "recreate" (fails: label `main` already
    // exists) so the window never came back after any tab was opened. That was
    // the real cause of "summon dead" and "won't reopen". get_window always
    // finds it, and force_show_main drives the raw HWND regardless.
    if let Some(win) = app.get_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        force_show_main();
        // Window foreground ≠ webview keyboard focus: without MoveFocus the
        // summoned/tray-restored window looked focused but typing went nowhere
        // until a click. Focus the page (or chrome when no tabs).
        focus_main_content_or_chrome(app);
        // Restore full memory/performance after the hidden-mode LOW trim.
        // MUST NOT be a skippable try_lock: if the pool happened to be busy the
        // LOW target stayed on every view forever → everything felt sluggish.
        // Block on the lock from a background thread instead.
        {
            let pool = app.state::<std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>().inner().clone();
            std::thread::spawn(move || {
                if let Ok(p) = pool.lock() {
                    p.restore_active_memory_target();
                    let _ = p.pip_active(crate::engine::PipRequest::ExitAuto);
                };
            });
        }
        return app.get_webview_window("main");
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
        // app-data-dir-sharing webviews agree on the extensions setting.
        .browser_extensions_enabled(true)
        .build()
    {
        Ok(win) => {
            crate::app::attach_window_plumbing(app, win.as_ref().window());
            let _ = win.set_focus();
            force_show_main();
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

    crate::appearance::apply_saved_to(app, &window.as_ref().window());

    Ok(window)
}

pub fn window_new_impl(app: &AppHandle) -> Result<(), String> {
    if !FEATURE_MULTIWINDOW {
        feature_gated_toast(app, "New windows");
        return Ok(());
    }
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

// ── Feature gates ────────────────────────────────────────────────────────────
/// Multi-window and incognito are DISABLED in stable builds: TabPool keeps ONE
/// global active tab, so activating a tab in a second window hides every other
/// window's content ("opening a new window resets all windows"), and incognito
/// needs the same per-window state to be trustworthy. Both stay beta-gated
/// until per-window active-tab tracking lands. Every entry point funnels
/// through the two impls below, so gating here covers hotkeys, palette,
/// quick-launch, and keyboard shortcuts alike.
pub const FEATURE_MULTIWINDOW: bool = false;
pub const FEATURE_INCOGNITO: bool = false;

fn feature_gated_toast(app: &AppHandle, what: &str) {
    use tauri::Emitter;
    let _ = app.emit(
        "toast:show",
        format!("{} is disabled in this release while we finish it — coming back soon", what),
    );
}

/// Async command: window creation must not run synchronously inside the
/// WebView2 IPC event handler on the main thread (deadlocks — see tabs.rs).
#[tauri::command]
pub async fn window_new(app: AppHandle) -> Result<(), String> {
    window_new_impl(&app)
}

pub fn window_new_incognito_impl(app: &AppHandle) -> Result<(), String> {
    if !FEATURE_INCOGNITO {
        feature_gated_toast(app, "Incognito");
        return Ok(());
    }
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
            // Match the other app windows' extensions setting so WebView2 doesn't
            // refuse to create the environment (the capture-window constraint);
            // the InPrivate profile holds no installed extensions, so none load.
            .browser_extensions_enabled(true)
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
