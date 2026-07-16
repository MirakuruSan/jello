use crate::engine::{ContentView, Rect, TabRuntimeState};
use crate::ipc_types::ViewId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// True while a Jello overlay panel (find bar / tab panel / in-window palette)
/// is open. Only then does the accelerator handler swallow Esc; otherwise Esc
/// passes through to the web page (so site modals / video fullscreen exit work).
pub static PANEL_OPEN: AtomicBool = AtomicBool::new(false);

/// JS that reads title, favicon, and scroll position from the page.
const SNAPSHOT_JS: &str = r#"
    (function() {
        try {
            let fav = '';
            let link = document.querySelector("link[rel~='icon']");
            if (link) { fav = link.href; }
            else { fav = window.location.origin + '/favicon.ico'; }
            return JSON.stringify({
                title: document.title || '',
                faviconUrl: fav,
                scrollY: window.scrollY || 0.0
            });
        } catch(e) {
            return JSON.stringify({ title: '', faviconUrl: '', scrollY: 0.0 });
        }
    })()
"#;

#[derive(serde::Deserialize)]
struct JsSnapshot {
    title: String,
    #[serde(rename = "faviconUrl")]
    favicon_url: String,
    #[serde(rename = "scrollY")]
    scroll_y: f64,
}

/// Scroll position only — polled slowly since there is no scroll event.
const SCROLL_JS: &str = "(window.scrollY||0).toString()";

/// Write a tab's freshly-changed URL and/or title through to the DB (or the
/// incognito store) and emit `tab:updated` so the overlay + tab panel refresh
/// live. Driven by WebView2 DocumentTitleChanged/SourceChanged events — no
/// polling. `url`/`title` are None when that field didn't change.
#[cfg(target_os = "windows")]
fn persist_tab_meta(
    app: &tauri::AppHandle,
    view_id: i32,
    url: Option<String>,
    title: Option<String>,
    is_incognito: bool,
) {
    use tauri::{Manager, Emitter};
    // Chrome Web Store intercept (Phase 4): the store's own "Add to Chrome" can
    // never work in WebView2, so when a content tab lands on a detail page we
    // offer Jello's working sideload path via an overlay banner instead.
    if let Some(u) = &url {
        if u.contains("chromewebstore.google.com/detail/")
            || u.contains("chrome.google.com/webstore/detail/")
            // Edge Add-ons detail pages (P1.4) — same "Install to Jello" banner.
            || u.contains("microsoftedge.microsoft.com/addons/detail/")
        {
            let _ = app.emit("webstore:detected", u.clone());
        }
    }
    if is_incognito || view_id < 0 {
        if let Some(mut t) = crate::incognito::get_incognito_tab(view_id) {
            if let Some(u) = url { t.url = u; }
            if let Some(ti) = title { if !ti.is_empty() { t.title = Some(ti); } }
            crate::incognito::update_incognito_tab(view_id, t.url.clone(), t.title.clone(), t.scroll_y);
            let _ = app.emit("tab:updated", &t);
        }
        return;
    }
    let app2 = app.clone();
    let db = app.state::<crate::db::DbState>();
    db.execute(move |conn| {
        if let Ok(Some(mut t)) = crate::db::tabs_repo::get_tab(conn, view_id) {
            let mut url_changed = false;
            if let Some(u) = &url {
                url_changed = t.url != *u;
                t.url = u.clone();
            }
            if let Some(ti) = &title {
                if !ti.is_empty() { t.title = Some(ti.clone()); }
            }
            let _ = crate::db::tabs_repo::update_tab(conn, &t);
            // Record a history visit on real URL changes to http(s) pages.
            if url_changed {
                if let Some(u) = &url {
                    if u.starts_with("http://") || u.starts_with("https://") {
                        let th = t.title.clone();
                        let _ = crate::db::history::record_visit(conn, u, th.as_deref(), false);
                    }
                }
            }
            let _ = app2.emit("tab:updated", &t);
        }
    });
}

pub struct WebView2ContentView {
    id: ViewId,
    webview: tauri::Webview<tauri::Wry>,
    bounds: std::sync::Mutex<Rect>,
    visible: Arc<AtomicBool>,
    /// Event-driven cache of page runtime state. Updated by a background poller
    /// and by refresh_snapshot(); read synchronously (never blocks) by snapshot().
    cache: Arc<Mutex<TabRuntimeState>>,
    alive: Arc<AtomicBool>,
}

impl WebView2ContentView {
    /// Fire a non-blocking cache refresh: reads title/favicon/scroll (async eval)
    /// and can_go_back/forward (async COM) and writes them into the cache when they
    /// arrive. Never blocks the caller.
    fn spawn_cache_refresh(&self) {
        let cache = self.cache.clone();
        let js_cache = cache.clone();
        let _ = self.webview.eval_with_callback(SNAPSHOT_JS, move |res| {
            if let Ok(parsed) = serde_json::from_str::<JsSnapshot>(&res) {
                if let Ok(mut c) = js_cache.lock() {
                    c.title = parsed.title;
                    c.favicon_url = parsed.favicon_url;
                    c.scroll_y = parsed.scroll_y;
                }
            }
        });
        if let Ok(u) = self.webview.url() {
            if let Ok(mut c) = cache.lock() {
                c.url = u.to_string();
            }
        }
        #[cfg(target_os = "windows")]
        {
            let nav_cache = cache;
            let _ = self.webview.with_webview(move |w| {
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let mut can_back = 0i32;
                        let mut can_fwd = 0i32;
                        let _ = core.CanGoBack(&mut can_back as *mut i32 as *mut _);
                        let _ = core.CanGoForward(&mut can_fwd as *mut i32 as *mut _);
                        if let Ok(mut c) = nav_cache.lock() {
                            c.can_go_back = can_back != 0;
                            c.can_go_forward = can_fwd != 0;
                        }
                    }
                }
            });
        }
    }

    pub fn new(id: ViewId, webview: tauri::Webview<tauri::Wry>, is_incognito: bool) -> Self {
        use tauri::Manager;
        // Bound before the windows-only handler block below: the event-driven
        // title/URL handlers capture it, and it also backs the ContentView.
        let cache = Arc::new(Mutex::new(TabRuntimeState {
            url: webview.url().map(|u| u.to_string()).unwrap_or_default(),
            title: String::new(),
            favicon_url: String::new(),
            scroll_y: 0.0,
            can_go_back: false,
            can_go_forward: false,
        }));
        #[cfg(target_os = "windows")]
        {
            let app_handle = webview.app_handle().clone();
            let _ = webview.with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2AcceleratorKeyPressedEventHandler;
                
                unsafe {
                    let controller = w.controller();
                    let handler_ptr = Box::into_raw(Box::new(RawAcceleratorHandler {
                        lpVtbl: &RAW_VTBL,
                        app_handle,
                        ref_count: std::sync::atomic::AtomicU32::new(1),
                    }));
                    // The windows-rs interface wrapper holds the COM object
                    // POINTER — transmute the pointer itself. (Dereferencing
                    // the struct as the wrapper passes the vtable address as
                    // the object → AddRef through garbage → segfault.)
                    let handler: ICoreWebView2AcceleratorKeyPressedEventHandler =
                        std::mem::transmute::<*mut RawAcceleratorHandler, _>(handler_ptr);
                    let mut token: i64 = 0;
                    let _ = controller.add_AcceleratorKeyPressed(
                        &handler,
                        &mut token as *mut i64 as *mut _,
                    );
                    // Registration AddRef'd it; dropping `handler` releases our
                    // initial ref, leaving ownership with the controller.
                }
            });

            // Enable built-in password autosave + general autofill (SPEC §8).
            // Defaults on; a settings toggle change applies on next launch.
            let _ = webview.with_webview(|w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4;
                use windows::core::Interface;
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        if let Ok(settings) = core.Settings() {
                            // Ctrl+scroll and Ctrl+± zoom (default on; force it so
                            // wry can't leave it disabled).
                            let _ = settings.SetIsZoomControlEnabled(true);
                            // Native right-click context menus (Phase 5). This
                            // restores the full default menu (Back/Reload/Copy/
                            // Copy link/Save image/Inspect) — the user-reported
                            // "no right-click options" fix. SPEC-GAP: adding
                            // custom Jello items (Open link in new tab, Search
                            // selection) needs ICoreWebView2Environment9::
                            // CreateContextMenuItem, and the environment isn't
                            // cleanly reachable through wry's webview handle —
                            // deferred. Overlay chrome (tab rows / domain pill)
                            // gets custom menus via the frontend instead.
                            let _ = settings.SetAreDefaultContextMenusEnabled(true);
                            if let Ok(s4) = settings.cast::<ICoreWebView2Settings4>() {
                                let _ = s4.SetIsPasswordAutosaveEnabled(true);
                                let _ = s4.SetIsGeneralAutofillEnabled(true);
                            }
                        }
                    }
                }
            });

            // Persist zoom per host: when the user Ctrl+scrolls, WebView2 fires
            // ZoomFactorChanged on the controller. Emit it so the overlay updates
            // its zoom state and stores the per-host value via zoom_set.
            let zoom_app = webview.app_handle().clone();
            let _ = webview.with_webview(move |w| {
                use webview2_com::ZoomFactorChangedEventHandler;
                use tauri::Emitter;
                unsafe {
                    let controller = w.controller();
                    let handler = ZoomFactorChangedEventHandler::create(Box::new(move |sender, _args| {
                        if let Some(ctrl) = sender {
                            let mut factor = 1.0f64;
                            if ctrl.ZoomFactor(&mut factor).is_ok() {
                                let _ = zoom_app.emit("zoom:changed", factor);
                            }
                        }
                        Ok(())
                    }));
                    let mut token: i64 = 0;
                    let _ = controller.add_ZoomFactorChanged(&handler, &mut token as *mut i64);
                }
            });

            // Register a DownloadStarting handler that tracks the operation so it
            // can be paused/resumed/cancelled, and streams progress (P3.1).
            let dl_app = webview.app_handle().clone();
            let _ = webview.with_webview(move |w| {
                use webview2_com::DownloadStartingEventHandler;
                use windows::core::Interface;
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let handler = DownloadStartingEventHandler::create(Box::new(move |_sender, args| {
                            if let Some(args) = args {
                                if let Ok(op) = args.DownloadOperation() {
                                    register_download(&dl_app, op);
                                }
                            }
                            Ok(())
                        }));
                        let mut token: i64 = 0;
                        if let Ok(core4) = core.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_4>() {
                            let _ = core4.add_DownloadStarting(&handler, &mut token as *mut i64);
                        }
                    }
                }
            });

            // Middle-click / target=_blank / window.open → open in a BACKGROUND
            // tab in the same window instead of a popup (#15). WebView2 fires
            // NewWindowRequested; we take the Uri, mark it handled (no popup), and
            // create the tab off the COM thread.
            let nw_app = webview.app_handle().clone();
            let nw_view_id = id.0 as i32;
            let nw_incog = is_incognito;
            let _ = webview.with_webview(move |w| {
                use webview2_com::NewWindowRequestedEventHandler;
                use windows::core::PWSTR;
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let handler = NewWindowRequestedEventHandler::create(Box::new(move |_s, args| {
                            if let Some(args) = args {
                                let mut uri = PWSTR::null();
                                let mut url = String::new();
                                if args.Uri(&mut uri).is_ok() && !uri.is_null() {
                                    url = uri.to_string().unwrap_or_default();
                                }
                                // We handle it ourselves → suppress the popup window.
                                let _ = args.SetHandled(true);
                                if url.starts_with("http") {
                                    let app = nw_app.clone();
                                    std::thread::spawn(move || {
                                        use tauri::Manager;
                                        // Resolve which window this content tab lives in.
                                        let win_id = if nw_incog || nw_view_id < 0 {
                                            crate::incognito::get_incognito_tab(nw_view_id)
                                                .map(|t| t.window_id)
                                                .unwrap_or(1)
                                        } else {
                                            let db = app.state::<crate::db::DbState>();
                                            let (tx, rx) = std::sync::mpsc::channel();
                                            let vid = nw_view_id;
                                            db.execute(move |conn| {
                                                let w = crate::db::tabs_repo::get_tab(conn, vid)
                                                    .map(|o| o.map(|t| t.window_id).unwrap_or(1))
                                                    .unwrap_or(1);
                                                let _ = tx.send(w);
                                            });
                                            rx.recv().unwrap_or(1)
                                        };
                                        let db = app.state::<crate::db::DbState>();
                                        let pool = app.state::<Arc<Mutex<crate::engine::pool::TabPool>>>();
                                        let _ = crate::tabs::tabs_create_impl(
                                            Some(url), Some(true), Some(win_id), &db, &pool, &app,
                                        );
                                    });
                                }
                            }
                            Ok(())
                        }));
                        let mut token: i64 = 0;
                        let _ = core.add_NewWindowRequested(&handler, &mut token as *mut i64);
                    }
                }
            });

            // Event-driven tab metadata: DocumentTitleChanged + SourceChanged
            // write through to the DB and emit tab:updated, so titles/URLs in the
            // tab panel and domain pill stay fresh live — replacing the old 250ms
            // poller that locked the TabPool N times/sec (freeze + staleness).
            let meta_app = webview.app_handle().clone();
            let meta_view_id = id.0 as i32;
            let meta_incog = is_incognito;
            let meta_cache = cache.clone();
            let _ = webview.with_webview(move |w| {
                use webview2_com::{DocumentTitleChangedEventHandler, SourceChangedEventHandler, FaviconChangedEventHandler};
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_15;
                use windows::core::{PWSTR, Interface};
                use tauri::Emitter;
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        // Title changes
                        let t_app = meta_app.clone();
                        let t_cache = meta_cache.clone();
                        let title_handler = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _args| {
                            if let Some(core) = sender {
                                let mut title = PWSTR::null();
                                if core.DocumentTitle(&mut title).is_ok() && !title.is_null() {
                                    let title_str = title.to_string().unwrap_or_default();
                                    if let Ok(mut c) = t_cache.lock() { c.title = title_str.clone(); }
                                    persist_tab_meta(&t_app, meta_view_id, None, Some(title_str), meta_incog);
                                }
                            }
                            Ok(())
                        }));
                        let mut tok: i64 = 0;
                        let _ = core.add_DocumentTitleChanged(&title_handler, &mut tok as *mut i64);

                        // URL (Source) changes
                        let s_app = meta_app.clone();
                        let s_cache = meta_cache.clone();
                        let src_handler = SourceChangedEventHandler::create(Box::new(move |sender, _args| {
                            if let Some(core) = sender {
                                let mut src = PWSTR::null();
                                if core.Source(&mut src).is_ok() && !src.is_null() {
                                    let url = src.to_string().unwrap_or_default();
                                    if !url.is_empty() {
                                        if let Ok(mut c) = s_cache.lock() { c.url = url.clone(); }
                                        persist_tab_meta(&s_app, meta_view_id, Some(url), None, meta_incog);
                                    }
                                }
                            }
                            Ok(())
                        }));
                        let mut tok2: i64 = 0;
                        let _ = core.add_SourceChanged(&src_handler, &mut tok2 as *mut i64);

                        // Favicon changes → cache the URL event-driven (consumed
                        // by favicon rendering in a later phase). Needs
                        // ICoreWebView2_15; silently degrades if unavailable
                        // (the JS snapshot path still caches favicon_url).
                        if let Ok(core15) = core.cast::<ICoreWebView2_15>() {
                            let f_cache = meta_cache.clone();
                            let fav_handler = FaviconChangedEventHandler::create(Box::new(move |sender, _args| {
                                if let Some(core) = sender {
                                    if let Ok(c15) = core.cast::<ICoreWebView2_15>() {
                                        let mut uri = PWSTR::null();
                                        if c15.FaviconUri(&mut uri).is_ok() && !uri.is_null() {
                                            let fav = uri.to_string().unwrap_or_default();
                                            if !fav.is_empty() {
                                                if let Ok(mut c) = f_cache.lock() { c.favicon_url = fav; }
                                            }
                                        }
                                    }
                                }
                                Ok(())
                            }));
                            let mut tok3: i64 = 0;
                            let _ = core15.add_FaviconChanged(&fav_handler, &mut tok3 as *mut i64);
                        }

                        // Navigation loading state → nav:loading {tabId, loading}
                        // so the overlay can show a top progress bar (Phase 8).
                        use webview2_com::{NavigationStartingEventHandler, NavigationCompletedEventHandler};
                        let ns_app = meta_app.clone();
                        let ns_handler = NavigationStartingEventHandler::create(Box::new(move |_s, a| {
                            if let Some(a) = a {
                                let mut urip = windows::core::PWSTR::null();
                                let url = unsafe { a.Uri(&mut urip).ok().and_then(|_| urip.to_string().ok()) }.unwrap_or_default();
                                tracing::info!("nav starting tab {}: {}", meta_view_id, url);
                            }
                            let _ = ns_app.emit("nav:loading", serde_json::json!({"tabId": meta_view_id, "loading": true}));
                            Ok(())
                        }));
                        let mut tok4: i64 = 0;
                        let _ = core.add_NavigationStarting(&ns_handler, &mut tok4 as *mut i64);

                        let nc_app = meta_app.clone();
                        let nc_handler = NavigationCompletedEventHandler::create(Box::new(move |_s, a| {
                            if let Some(a) = a {
                                let mut ok = windows::core::BOOL::default();
                                let mut status = webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_ERROR_STATUS::default();
                                unsafe {
                                    let _ = a.IsSuccess(&mut ok);
                                    let _ = a.WebErrorStatus(&mut status);
                                }
                                tracing::info!("nav completed tab {}: success={:?} status={:?}", meta_view_id, ok.as_bool(), status);
                            }
                            let _ = nc_app.emit("nav:loading", serde_json::json!({"tabId": meta_view_id, "loading": false}));
                            Ok(())
                        }));
                        let mut tok5: i64 = 0;
                        let _ = core.add_NavigationCompleted(&nc_handler, &mut tok5 as *mut i64);
                    }
                }
            });
        }

        let visible = Arc::new(AtomicBool::new(true));
        let alive = Arc::new(AtomicBool::new(true));

        let view = Self {
            id,
            webview,
            bounds: std::sync::Mutex::new(Rect { x: 0, y: 0, width: 800, height: 600 }),
            visible: visible.clone(),
            cache: cache.clone(),
            alive: alive.clone(),
        };

        // Lightweight scroll poller. Title/URL are event-driven now; the only
        // page state without an event is scroll position, captured every 1.5s
        // for the *visible* tab so eviction can restore it. Critically it uses
        // try_lock on the TabPool — it can NEVER block on the pool (the old
        // 250ms blocking poller was a freeze + sluggishness root cause).
        let webview_poll = view.webview.clone();
        let poll_app = view.webview.app_handle().clone();
        let visible_poll = visible.clone();
        let alive_poll = alive.clone();
        let cache_poll = cache.clone();
        let view_id = id.0 as i32;

        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                if !alive_poll.load(Ordering::Relaxed) {
                    break;
                }
                // Only the visible tab scrolls; skip the rest cheaply.
                if !visible_poll.load(Ordering::Relaxed) {
                    continue;
                }
                // Non-blocking suspension check — never wait on the pool.
                let suspended = poll_app
                    .state::<Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>()
                    .inner()
                    .try_lock()
                    .map(|pool| pool.is_tab_suspended(view_id))
                    .unwrap_or(false);
                if suspended {
                    continue;
                }
                let js_cache = cache_poll.clone();
                let _ = webview_poll.eval_with_callback(SCROLL_JS, move |res| {
                    if let Ok(y) = res.trim().trim_matches('"').parse::<f64>() {
                        if let Ok(mut c) = js_cache.lock() {
                            c.scroll_y = y;
                        }
                    }
                });
            }
        });

        view
    }
}

impl ContentView for WebView2ContentView {
    fn id(&self) -> ViewId {
        self.id
    }
    fn navigate(&self, url: &str) {
        if let Ok(parsed) = tauri::Url::parse(url) {
            let _ = self.webview.navigate(parsed);
        }
    }
    fn back(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let _ = core.GoBack();
                    }
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = self.webview.eval("window.history.back()");
        }
    }
    fn forward(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let _ = core.GoForward();
                    }
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = self.webview.eval("window.history.forward()");
        }
    }
    fn reload(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let _ = core.Reload();
                    }
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = self.webview.eval("window.location.reload()");
        }
    }
    fn stop(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        let _ = core.Stop();
                    }
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = self.webview.eval("window.stop()");
        }
    }
    fn set_bounds(&self, rect: Rect) {
        let mut bounds = self.bounds.lock().unwrap();
        *bounds = rect;
        
        let visible = self.visible.load(Ordering::Relaxed);
        if visible {
            let _ = self.webview.set_size(tauri::LogicalSize::new(rect.width as f64, rect.height as f64));
            let _ = self.webview.set_position(tauri::LogicalPosition::new(rect.x as f64, rect.y as f64));
        }
    }
    fn set_visible(&self, v: bool) {
        self.visible.store(v, Ordering::Relaxed);
        
        if v {
            let rect = self.bounds.lock().unwrap();
            let _ = self.webview.set_size(tauri::LogicalSize::new(rect.width as f64, rect.height as f64));
            let _ = self.webview.set_position(tauri::LogicalPosition::new(rect.x as f64, rect.y as f64));
        } else {
            let _ = self.webview.set_position(tauri::LogicalPosition::new(-20000.0, -20000.0));
            let _ = self.webview.set_size(tauri::LogicalSize::new(0.0, 0.0));
        }
    }
    fn try_suspend(&self, done: Box<dyn FnOnce(bool) + Send>) {
        #[cfg(target_os = "windows")]
        {
            // WebView2 refuses to suspend a visible controller, so hide first.
            self.set_visible(false);
            let done_share = std::sync::Mutex::new(Some(done));
            let _ = self.webview.with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_7;
                use webview2_com::TrySuspendCompletedHandler;
                use windows::core::Interface;

                let done = done_share.lock().unwrap().take().unwrap();
                unsafe {
                    if let Ok(controller) = w.controller().CoreWebView2() {
                        if let Ok(webview_7) = controller.cast::<ICoreWebView2_7>() {
                            let handler = TrySuspendCompletedHandler::create(Box::new(
                                move |hr: windows::core::Result<()>, is_successful: bool| {
                                    done(hr.is_ok() && is_successful);
                                    Ok(())
                                },
                            ));
                            if webview_7.TrySuspend(&handler).is_err() {
                                // Handler will not fire; nothing more we can do.
                                // Pool's recv timeout resolves this to "not suspended".
                            }
                            return;
                        }
                    }
                }
                done(false);
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            done(false);
        }
    }
    fn resume(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_7;
                use windows::core::Interface;

                unsafe {
                    if let Ok(controller) = w.controller().CoreWebView2() {
                        if let Ok(webview_7) = controller.cast::<ICoreWebView2_7>() {
                            let _ = webview_7.Resume();
                        }
                    }
                }
            });
        }
    }
    fn snapshot(&self) -> TabRuntimeState {
        // Non-blocking: return the cached state (kept fresh by the 2s poller and
        // refresh_snapshot on switch-away) plus a synchronous current-url read.
        let mut state = self.cache.lock().map(|c| c.clone()).unwrap_or(TabRuntimeState {
            url: String::new(),
            title: String::new(),
            favicon_url: String::new(),
            scroll_y: 0.0,
            can_go_back: false,
            can_go_forward: false,
        });
        if let Ok(u) = self.webview.url() {
            state.url = u.to_string();
        }
        state
    }
    fn refresh_snapshot(&self) {
        self.spawn_cache_refresh();
    }
    fn mute(&self, m: bool) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
                use windows::core::Interface;

                unsafe {
                    if let Ok(controller) = w.controller().CoreWebView2() {
                        if let Ok(webview_8) = controller.cast::<ICoreWebView2_8>() {
                            let _ = webview_8.SetIsMuted(m);
                        }
                    }
                }
            });
        }
    }
    fn find(&self, text: &str, forward: bool) {
        // Escape the needle as a JSON string literal to avoid breakage/injection.
        let needle = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        let js = format!("window.find({}, false, {}, true, false, false, false)", needle, !forward);
        let _ = self.webview.eval(&js);
    }
    fn zoom(&self, factor: f64) {
        let _ = self.webview.set_zoom(factor);
    }
    fn focus(&self) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(|w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC;
                unsafe {
                    let _ = w.controller().MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                }
            });
        }
    }
    fn set_memory_target(&self, low: bool) {
        #[cfg(target_os = "windows")]
        {
            let _ = self.webview.with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::{
                    ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
                    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
                };
                use windows::core::Interface;
                unsafe {
                    if let Ok(core) = w.controller().CoreWebView2() {
                        if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                            let level = if low {
                                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                            } else {
                                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                            };
                            let _ = c19.SetMemoryUsageTargetLevel(level);
                        }
                    }
                }
            });
        }
        #[cfg(not(target_os = "windows"))]
        let _ = low;
    }
    fn close(self: Box<Self>) {
        self.alive.store(false, Ordering::Relaxed);
        let _ = self.webview.close();
    }
    fn is_audio_playing(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = self.webview.with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
                use windows::core::Interface;

                unsafe {
                    if let Ok(controller) = w.controller().CoreWebView2() {
                        if let Ok(webview_8) = controller.cast::<ICoreWebView2_8>() {
                            let mut is_playing = 0i32;
                            if webview_8.IsDocumentPlayingAudio(&mut is_playing as *mut i32 as *mut _).is_ok() {
                                let _ = tx.send(is_playing != 0);
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(false);
            });
            rx.recv().unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

#[cfg(target_os = "windows")]
fn is_in_window_shortcut(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<&'static str> {
    match (vk, ctrl, shift, alt) {
        // Dedicated browser back/forward keys (media keys, mouse thumb buttons
        // that synthesize them) — history navigation, per the user's muscle
        // memory. Work regardless of modifiers.
        (0xA6, ..) => Some("Nav+Back"),    // VK_BROWSER_BACK
        (0xA7, ..) => Some("Nav+Forward"), // VK_BROWSER_FORWARD
        // Toggle the chrome overlay UI
        (0x55, true, true, false) => Some("Ctrl+Shift+U"),
        // Ctrl+T
        (0x54, true, false, false) => Some("Ctrl+T"),
        // Ctrl+Shift+T
        (0x54, true, true, false) => Some("Ctrl+Shift+T"),
        // Ctrl+W
        (0x57, true, false, false) => Some("Ctrl+W"),
        // Ctrl+Shift+W — unload current tab
        (0x57, true, true, false) => Some("Ctrl+Shift+W"),
        // Ctrl+N
        (0x4E, true, false, false) => Some("Ctrl+N"),
        // Ctrl+Shift+N
        (0x4E, true, true, false) => Some("Ctrl+Shift+N"),
        // Ctrl+L
        (0x4C, true, false, false) => Some("Ctrl+L"),
        // F6
        (0x75, false, false, false) => Some("F6"),
        // Ctrl+Tab
        (0x09, true, false, false) => Some("Ctrl+Tab"),
        // Ctrl+Shift+Tab
        (0x09, true, true, false) => Some("Ctrl+Shift+Tab"),
        // Ctrl+1..9
        (0x31..=0x39, true, false, false) => Some("Ctrl+Num"),
        // Ctrl+F
        (0x46, true, false, false) => Some("Ctrl+F"),
        // Ctrl+R
        (0x52, true, false, false) => Some("Ctrl+R"),
        // F5
        (0x74, false, false, false) => Some("F5"),
        // Ctrl+Shift+R
        (0x52, true, true, false) => Some("Ctrl+Shift+R"),
        // Ctrl+Shift+E
        (0x45, true, true, false) => Some("Ctrl+Shift+E"),
        // Ctrl+J
        (0x4A, true, false, false) => Some("Ctrl+J"),
        // Ctrl+D
        (0x44, true, false, false) => Some("Ctrl+D"),
        // Ctrl+H
        (0x48, true, false, false) => Some("Ctrl+H"),
        // Ctrl+Shift+O
        (0x4F, true, true, false) => Some("Ctrl+Shift+O"),
        // Ctrl+= (0xBB is VK_OEM_PLUS)
        (0xBB, true, false, false) => Some("Ctrl+ZoomIn"),
        // Ctrl+- (0xBD is VK_OEM_MINUS)
        (0xBD, true, false, false) => Some("Ctrl+ZoomOut"),
        // Ctrl+0
        (0x30, true, false, false) => Some("Ctrl+ZoomReset"),
        // Numpad zoom: Ctrl+NumpadPlus / Ctrl+NumpadMinus / Ctrl+Numpad0
        (0x6B, true, false, false) => Some("Ctrl+ZoomIn"),   // VK_ADD
        (0x6D, true, false, false) => Some("Ctrl+ZoomOut"),  // VK_SUBTRACT
        (0x60, true, false, false) => Some("Ctrl+ZoomReset"), // VK_NUMPAD0
        // Ctrl+Shift+C
        (0x43, true, true, false) => Some("Ctrl+Shift+C"),
        // Ctrl+Shift+V
        (0x56, true, true, false) => Some("Ctrl+Shift+V"),
        // Ctrl+M
        (0x4D, true, false, false) => Some("Ctrl+M"),
        // Ctrl+Q
        (0x51, true, false, false) => Some("Ctrl+Q"),
        // Alt+Left / Alt+Right — switch between tabs (prev/next)
        (0x25, false, false, true) => Some("Tab+Prev"),
        (0x27, false, false, true) => Some("Tab+Next"),
        // F11
        (0x7A, false, false, false) => Some("F11"),
        // Esc
        (0x1B, false, false, false) => Some("Esc"),
        _ => None,
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[allow(non_snake_case)]
struct RawAcceleratorHandler {
    lpVtbl: *const ICoreWebView2AcceleratorKeyPressedEventHandlerVtbl,
    app_handle: tauri::AppHandle,
    ref_count: std::sync::atomic::AtomicU32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[allow(non_snake_case)]
struct ICoreWebView2AcceleratorKeyPressedEventHandlerVtbl {
    QueryInterface: unsafe extern "system" fn(this: *mut RawAcceleratorHandler, riid: *const windows::core::GUID, ppvObject: *mut *mut std::ffi::c_void) -> windows::core::HRESULT,
    AddRef: unsafe extern "system" fn(this: *mut RawAcceleratorHandler) -> u32,
    Release: unsafe extern "system" fn(this: *mut RawAcceleratorHandler) -> u32,
    Invoke: unsafe extern "system" fn(this: *mut RawAcceleratorHandler, sender: *mut std::ffi::c_void, args: *mut std::ffi::c_void) -> windows::core::HRESULT,
}

#[cfg(target_os = "windows")]
static RAW_VTBL: ICoreWebView2AcceleratorKeyPressedEventHandlerVtbl = ICoreWebView2AcceleratorKeyPressedEventHandlerVtbl {
    QueryInterface: raw_query_interface,
    AddRef: raw_add_ref,
    Release: raw_release,
    Invoke: raw_invoke,
};

#[cfg(target_os = "windows")]
#[allow(non_snake_case)]
unsafe extern "system" fn raw_query_interface(
    this: *mut RawAcceleratorHandler,
    riid: *const windows::core::GUID,
    ppvObject: *mut *mut std::ffi::c_void,
) -> windows::core::HRESULT {
    if ppvObject.is_null() {
        return windows::core::HRESULT(-2147467261); // E_POINTER (0x80004003)
    }
    
    let iunknown_guid = windows::core::GUID::from_u128(0x00000000_0000_0000_C000_000000000046);
    let handler_iid = windows::core::GUID::from_u128(0xb29c7e28_fa79_41a8_8e44_65811c76dcb2);
    let riid_ref = &*riid;
    if *riid_ref == iunknown_guid || *riid_ref == handler_iid {
        raw_add_ref(this);
        *ppvObject = this as *mut std::ffi::c_void;
        windows::core::HRESULT(0) // S_OK
    } else {
        *ppvObject = std::ptr::null_mut();
        windows::core::HRESULT(-2147467262) // E_NOINTERFACE (0x80004002)
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_add_ref(this: *mut RawAcceleratorHandler) -> u32 {
    let handler = &*this;
    handler.ref_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_release(this: *mut RawAcceleratorHandler) -> u32 {
    let handler = &*this;
    let old_count = handler.ref_count.fetch_sub(1, std::sync::atomic::Ordering::Release);
    if old_count == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
        let _ = Box::from_raw(this);
        0
    } else {
        old_count - 1
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_invoke(
    this: *mut RawAcceleratorHandler,
    _sender: *mut std::ffi::c_void,
    args: *mut std::ffi::c_void,
) -> windows::core::HRESULT {
    use tauri::Emitter;
    let handler = &*this;
    if !args.is_null() {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2AcceleratorKeyPressedEventArgs;
        // args IS the COM object pointer; borrow it as the interface wrapper.
        // (Never `&*(ptr as *const Interface)` — that treats the object memory
        // as the wrapper and reads the vtable as the object. See ::new above.)
        let Some(args_com) =
            windows::core::Interface::from_raw_borrowed(&args)
        else {
            return windows::core::HRESULT(0);
        };
        let args_com: &ICoreWebView2AcceleratorKeyPressedEventArgs = args_com;
        
        let mut key_event_kind = webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
        let _ = args_com.KeyEventKind(&mut key_event_kind);
        
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        };
        
        if key_event_kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
            || key_event_kind == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
        {
            let mut vk = 0u32;
            let _ = args_com.VirtualKey(&mut vk);

            use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT, VK_MENU};
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let alt = GetKeyState(VK_MENU.0 as i32) < 0;

            if let Some(action) = is_in_window_shortcut(vk, ctrl, shift, alt) {
                // Let Esc reach the page unless a Jello panel is open.
                if action == "Esc" && !PANEL_OPEN.load(Ordering::Relaxed) {
                    return windows::core::HRESULT(0);
                }
                let _ = args_com.SetHandled(true);
                let detail = if action == "Ctrl+Num" {
                    let num = vk - 0x30;
                    format!("Ctrl+{}", num)
                } else {
                    action.to_string()
                };
                let _ = handler.app_handle.emit("window:shortcut", detail);
            }
        }
    }
    windows::core::HRESULT(0) // S_OK
}

// ── Download management (P3.1) ───────────────────────────────────────────────
// The DownloadStarting handler used to drop the DownloadOperation; now we retain
// it so the download can be paused/resumed/cancelled and its progress streamed.

#[cfg(target_os = "windows")]
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DownloadOperation;

/// A tracked download operation. COM objects aren't Send; the documented
/// invariant is that this is ONLY ever touched on the main (UI) thread — the
/// DownloadStarting/StateChanged handlers run there, and pause/resume/cancel
/// marshal onto it via run_on_main_thread (same pattern as chords' SendHhook).
#[cfg(target_os = "windows")]
struct SendDownloadOp(ICoreWebView2DownloadOperation);
#[cfg(target_os = "windows")]
unsafe impl Send for SendDownloadOp {}

#[cfg(target_os = "windows")]
static DOWNLOADS: Mutex<Vec<(String, SendDownloadOp)>> = Mutex::new(Vec::new());
#[cfg(target_os = "windows")]
static LAST_DL_EMIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(target_os = "windows")]
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn register_download(app: &tauri::AppHandle, op: ICoreWebView2DownloadOperation) {
    use tauri::Emitter;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
    };
    use webview2_com::{BytesReceivedChangedEventHandler, StateChangedEventHandler};
    use windows::core::PWSTR;
    unsafe {
        let id = now_millis().to_string();

        let mut url = String::new();
        let mut uri = PWSTR::null();
        if op.Uri(&mut uri).is_ok() && !uri.is_null() {
            url = uri.to_string().unwrap_or_default();
        }
        let mut path = String::new();
        let mut p = PWSTR::null();
        if op.ResultFilePath(&mut p).is_ok() && !p.is_null() {
            path = p.to_string().unwrap_or_default();
        }
        let file_name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
        let mut total: i64 = 0;
        let _ = op.TotalBytesToReceive(&mut total);

        tracing::info!("download started id={} file={} total={}", id, file_name, total);
        let _ = app.emit("download:started", serde_json::json!({
            "id": id, "fileName": file_name, "url": url, "state": "started",
            "total": total, "path": path,
        }));

        // Progress (throttled ~4/s).
        let prog_app = app.clone();
        let prog_id = id.clone();
        let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(op) = sender {
                let now = now_millis();
                let last = LAST_DL_EMIT.load(std::sync::atomic::Ordering::Relaxed);
                if now.saturating_sub(last) >= 250 {
                    LAST_DL_EMIT.store(now, std::sync::atomic::Ordering::Relaxed);
                    let mut received: i64 = 0;
                    let _ = op.BytesReceived(&mut received);
                    let mut tot: i64 = 0;
                    let _ = op.TotalBytesToReceive(&mut tot);
                    let _ = prog_app.emit("download:progress", serde_json::json!({
                        "id": prog_id, "received": received, "total": tot,
                    }));
                }
            }
            Ok(())
        }));
        let mut tok: i64 = 0;
        let _ = op.add_BytesReceivedChanged(&bytes_handler, &mut tok);

        // Terminal state → download:done + drop from the map.
        let state_app = app.clone();
        let state_id = id.clone();
        let state_handler = StateChangedEventHandler::create(Box::new(move |sender, _args| {
            if let Some(op) = sender {
                let mut st = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
                let _ = op.State(&mut st);
                let mut rp = PWSTR::null();
                let mut result_path = String::new();
                if op.ResultFilePath(&mut rp).is_ok() && !rp.is_null() {
                    result_path = rp.to_string().unwrap_or_default();
                }
                let state_str = if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                    "completed"
                } else if st == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                    "interrupted"
                } else {
                    "inprogress"
                };
                let _ = state_app.emit("download:done", serde_json::json!({
                    "id": state_id, "state": state_str, "path": result_path,
                }));
                if st == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
                    || st == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED
                {
                    DOWNLOADS.lock().unwrap().retain(|(k, _)| k != &state_id);
                }
            }
            Ok(())
        }));
        let mut tok2: i64 = 0;
        let _ = op.add_StateChanged(&state_handler, &mut tok2);

        DOWNLOADS.lock().unwrap().push((id, SendDownloadOp(op)));
    }
}

#[cfg(target_os = "windows")]
fn download_action(app: &tauri::AppHandle, id: String, action: u8) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let map = DOWNLOADS.lock().unwrap();
        let res = match map.iter().find(|(k, _)| k == &id) {
            Some((_, SendDownloadOp(op))) => unsafe {
                match action {
                    0 => op.Pause().map_err(|e| e.to_string()),
                    1 => op.Resume().map_err(|e| e.to_string()),
                    _ => op.Cancel().map_err(|e| e.to_string()),
                }
            },
            None => Err("download not found (already finished?)".to_string()),
        };
        let _ = tx.send(res);
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn download_pause(id: String, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { download_action(&app, id, 0) }
    #[cfg(not(target_os = "windows"))]
    { let _ = (id, app); Ok(()) }
}

#[tauri::command]
pub async fn download_resume(id: String, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { download_action(&app, id, 1) }
    #[cfg(not(target_os = "windows"))]
    { let _ = (id, app); Ok(()) }
}

#[tauri::command]
pub async fn download_cancel(id: String, app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    { download_action(&app, id, 2) }
    #[cfg(not(target_os = "windows"))]
    { let _ = (id, app); Ok(()) }
}

/// Reveal a completed download in the OS file manager (selecting the file).
#[tauri::command]
pub async fn download_reveal(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("explorer")
            .creation_flags(0x08000000)
            .arg("/select,")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    { let _ = path; Ok(()) }
}
