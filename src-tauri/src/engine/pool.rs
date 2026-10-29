use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, WebviewBuilder, WebviewUrl, LogicalPosition, LogicalSize};
use crate::engine::ContentView;
use crate::engine::webview2::WebView2ContentView;
use crate::ipc_types::ViewId;
use crate::db::tabs_repo;

pub enum TabState {
    Cold,
    Live {
        view: Box<dyn ContentView + Send + Sync>,
        last_active: Instant,
    },
    Suspended {
        view: Box<dyn ContentView + Send + Sync>,
        last_active: Instant,
    },
}

#[derive(Default)]
pub struct TabPool {
    tabs: HashMap<i32, TabState>,
    active_tab_id: Option<i32>,
    /// Most-recently-used tab ids, front = most recent. Drives Ctrl+Tab switching.
    mru: Vec<i32>,
    /// Views whose tab was closed/unloaded while a download they started is
    /// unfinished. WebView2 cancels a download when its view closes, so these
    /// stay alive, hidden, until reap_parked() finds them idle.
    parked: Vec<Box<dyn ContentView + Send + Sync>>,
}

/// Unique suffix for content webview labels: tab ids are reused by SQLite, and
/// a parked view can outlive its tab, so `content_tab_{id}` alone can collide.
static VIEW_SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Close parked views whose downloads have ended (called when one finishes).
pub fn reap_parked_views(app: &AppHandle) {
    let pool = app.state::<Arc<std::sync::Mutex<TabPool>>>();
    let lock = pool.lock();
    if let Ok(mut p) = lock {
        p.reap_parked();
    }
}

/// Outcome of a blank-document check on a tab (see heal_blank_tab).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlankHeal {
    /// Showing a real document, or nothing to heal.
    NotNeeded,
    /// Blank, but a navigation is still in flight — check again later.
    Wait,
    /// Blank with nothing loading: the navigation was re-issued.
    Healed,
}

fn raise_overlay(app: &AppHandle, window_label: &str) {
    let overlay_state = app.state::<Arc<crate::app::OverlayState>>();
    let overlay_hwnd_raw = overlay_state.overlay_hwnds.lock().unwrap().get(window_label).copied();
    if let Some(hwnd_val) = overlay_hwnd_raw {
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE};
        let overlay_hwnd = windows::Win32::Foundation::HWND(hwnd_val as *mut _);
        unsafe {
            let _ = SetWindowPos(overlay_hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

impl TabPool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_active_tab_id(&mut self, id: Option<i32>) {
        self.active_tab_id = id;
    }

    pub fn get_active_tab_id(&self) -> Option<i32> {
        self.active_tab_id
    }

    pub fn is_tab_suspended(&self, id: i32) -> bool {
        matches!(self.tabs.get(&id), Some(TabState::Suspended { .. }))
    }

    pub fn get_active_count(&self) -> usize {
        self.tabs.values().filter(|state| matches!(state, TabState::Live { .. } | TabState::Suspended { .. })).count()
    }

    pub fn remove_tab(&mut self, id: i32) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
            self.retire(view);
        }
        if self.active_tab_id == Some(id) {
            self.active_tab_id = None;
        }
        self.mru.retain(|&x| x != id);
    }

    /// Dispose of a view whose tab no longer shows it: close it, or park it
    /// hidden while a download it started is unfinished.
    fn retire(&mut self, view: Box<dyn ContentView + Send + Sync>) {
        if view.has_active_downloads() {
            tracing::info!("keeping webview of tab {} alive until its download ends", view.id().0);
            view.set_visible(false);
            view.set_memory_target(true);
            self.parked.push(view);
        } else {
            view.close();
        }
    }

    pub fn reap_parked(&mut self) {
        let (busy, idle): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.parked).into_iter().partition(|v| v.has_active_downloads());
        self.parked = busy;
        for view in idle {
            view.close();
        }
    }

    /// Where to land after tab `closing` goes away: the most recently used tab
    /// that still has a webview, among `candidates` (the closing tab's window,
    /// in strip order). Never a cold tab — reloading one is exactly the work
    /// closing a tab shouldn't cause. None → show the new-tab page.
    pub fn next_loaded_tab(&self, closing: i32, candidates: &[i32]) -> Option<i32> {
        let eligible = |id: i32| id != closing && candidates.contains(&id) && self.is_loaded(id);
        self.mru
            .iter()
            .copied()
            .find(|&id| eligible(id))
            .or_else(|| candidates.iter().copied().find(|&id| eligible(id)))
    }

    /// Views that must keep running in the background: audio, a running
    /// download, or a video floating in picture-in-picture.
    fn is_busy(view: &(dyn ContentView + Send + Sync)) -> bool {
        view.has_active_downloads() || view.in_picture_in_picture() || view.is_audio_playing()
    }

    fn touch_mru(&mut self, id: i32) {
        self.mru.retain(|&x| x != id);
        self.mru.insert(0, id);
    }

    /// The tab to switch to for Ctrl+Tab (forward=next-most-recent) /
    /// Ctrl+Shift+Tab (backward=least-recent). None if fewer than 2 tabs.
    pub fn mru_target(&self, forward: bool) -> Option<i32> {
        if self.mru.len() < 2 {
            return None;
        }
        if forward {
            self.mru.get(1).copied()
        } else {
            self.mru.last().copied()
        }
    }

    pub fn find_active(&self, text: &str, forward: bool) -> Result<(), String> {
        self.with_active_view(|v| v.find(text, forward))
    }

    pub fn zoom_active(&self, factor: f64) -> Result<(), String> {
        self.with_active_view(|v| v.zoom(factor))
    }

    pub fn evict_lru(&mut self, db: &crate::db::DbState) -> bool {
        // Find candidate for eviction
        // Criteria: Live or Suspended, NOT active_tab_id, NOT playing audio, NOT pinned
        let mut candidate_id: Option<i32> = None;
        let mut oldest_time: Option<Instant> = None;

        for (&id, state) in &self.tabs {
            if Some(id) == self.active_tab_id {
                continue;
            }

            match state {
                TabState::Live { view, last_active } | TabState::Suspended { view, last_active } => {
                    if Self::is_busy(view.as_ref()) {
                        continue;
                    }

                    let is_pinned = if id < 0 {
                        crate::incognito::get_incognito_tab(id)
                            .map(|t| t.pinned)
                            .unwrap_or(false)
                    } else {
                        let (tx, rx) = std::sync::mpsc::channel();
                        db.execute(move |conn| {
                            let is_pinned = tabs_repo::get_tab(conn, id)
                                .map(|opt| opt.map(|t| t.pinned).unwrap_or(false))
                                .unwrap_or(false);
                            let _ = tx.send(is_pinned);
                        });
                        rx.recv().unwrap_or(false)
                    };
                    if is_pinned {
                        // Pinned tabs can suspend but not evict
                        continue;
                    }

                    if oldest_time.is_none() || *last_active < oldest_time.unwrap() {
                        oldest_time = Some(*last_active);
                        candidate_id = Some(id);
                    }
                }
                _ => {}
            }
        }

        if let Some(id) = candidate_id {
            self.unload_to_cold(db, id);
            return true;
        }
        false
    }

    /// Save a loaded tab's URL/title/scroll and close its webview, leaving it
    /// Cold (it reloads from the saved state on next activation).
    fn unload_to_cold(&mut self, db: &crate::db::DbState, id: i32) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
            let snapshot = view.snapshot();
            // A view still on about:blank never committed its page; keep the
            // stored URL rather than overwriting it with the blank document.
            if !snapshot.url.is_empty() && snapshot.url != "about:blank" {
                if id < 0 {
                    crate::incognito::update_incognito_tab(id, snapshot.url, Some(snapshot.title), snapshot.scroll_y);
                } else {
                    db.execute(move |conn| {
                        if let Ok(Some(mut db_tab)) = tabs_repo::get_tab(conn, id) {
                            db_tab.url = snapshot.url;
                            db_tab.title = Some(snapshot.title);
                            db_tab.scroll_y = snapshot.scroll_y;
                            let _ = tabs_repo::update_tab(conn, &db_tab);
                        }
                    });
                }
            }
            self.retire(view);
        }
        self.tabs.insert(id, TabState::Cold);
    }

    pub fn activate_tab(&mut self, db: &crate::db::DbState, id: i32, app: &AppHandle) -> Result<(), String> {
        let prev_active = self.active_tab_id;
        self.active_tab_id = Some(id);
        self.touch_mru(id);

        // Hide all other views. Titles/URLs are now event-driven (COM handlers),
        // so only the tab we're switching *away from* needs a scroll-position
        // capture — not every live tab. This removes the N-async-eval fan-out
        // that ran under the pool lock on every switch.
        for (&other_id, other_state) in &self.tabs {
            if other_id != id {
                match other_state {
                    TabState::Live { view, .. } | TabState::Suspended { view, .. } => {
                        if Some(other_id) == prev_active {
                            view.refresh_snapshot();
                            // Leaving a tab with a playing video: float it.
                            if crate::prefs::auto_pip() {
                                view.picture_in_picture(crate::engine::PipRequest::Auto);
                            }
                            // Background tabs trim caches; JS and media keep running.
                            view.set_memory_target(true);
                        }
                        view.set_visible(false);
                    }
                    _ => {}
                }
            }
        }

        // Ensure state entry exists
        self.tabs.entry(id).or_insert(TabState::Cold);

        // Retrieve tab configuration
        let db_tab = if id < 0 {
            crate::incognito::get_incognito_tab(id)
                .ok_or_else(|| "Tab not found in incognito store".to_string())?
        } else {
            let (tx, rx) = std::sync::mpsc::channel();
            let db_clone = db.clone();
            db_clone.execute(move |conn| {
                let res = tabs_repo::get_tab(conn, id);
                let _ = tx.send(res);
            });
            rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Tab not found in database".to_string())?
        };

        let window_label = if db_tab.window_id == 1 {
            "main".to_string()
        } else {
            let main_label = format!("main_{}", db_tab.window_id);
            if app.get_window(&main_label).is_some() {
                main_label
            } else {
                format!("incognito_{}", db_tab.window_id)
            }
        };

        let window = app.get_window(&window_label)
            .ok_or_else(|| format!("Parent window {} not found", window_label))?;

        let is_blank = db_tab.url.is_empty() || db_tab.url == "about:blank";
        if is_blank {
            self.tabs.insert(id, TabState::Cold);
            // Chrome layer IS the page now — overlay must take input everywhere.
            crate::app::overlay_mark_content(app, &window_label, false);
            // Give the chrome overlay keyboard focus so the new-tab search pill
            // is typable immediately (Ctrl+T from a focused page otherwise left
            // focus in the old content webview).
            crate::windows::focus_chrome(app, &window_label);
            return Ok(());
        }

        let rect = crate::windows::content_rect(&window);

        // If it's already Live, update timestamp, resize it, and make visible
        if let Some(TabState::Live { last_active, view }) = self.tabs.get_mut(&id) {
            *last_active = Instant::now();
            view.set_bounds(rect);
            view.set_visible(true);
            view.set_memory_target(false);
            view.picture_in_picture(crate::engine::PipRequest::ExitAuto);
            // Activation = the user is switching to this page: move keyboard
            // focus into it so accelerators/typing work without a click.
            view.focus();
            raise_overlay(app, &window_label);
            crate::app::overlay_mark_content(app, &window_label, true);
            return Ok(());
        }

        // If it's Suspended, resume, resize, and make visible
        if let Some(state) = self.tabs.remove(&id) {
            if let TabState::Suspended { view, .. } = state {
                view.resume();
                view.set_bounds(rect);
                view.set_visible(true);
                view.set_memory_target(false);
                view.picture_in_picture(crate::engine::PipRequest::ExitAuto);
                view.focus();
                self.tabs.insert(id, TabState::Live {
                    view,
                    last_active: Instant::now(),
                });
                raise_overlay(app, &window_label);
                crate::app::overlay_mark_content(app, &window_label, true);
                return Ok(());
            }
            self.tabs.insert(id, state);
        }

        // It is Cold. Unload least-recently-used tabs to stay under the limit.
        while self.get_active_count() >= crate::prefs::max_loaded_tabs() {
            if !self.evict_lru(db) {
                break;
            }
        }

        let serial = VIEW_SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let label = format!("content_tab_{}_{}", id, serial);
        let is_incognito = crate::incognito::is_incognito_window(db_tab.window_id);

        // Webview creation MUST happen on the main thread — add_child from a
        // worker thread segfaults (WebView2 controller creation is tied to the
        // parent HWND's thread). Commands run on the async runtime (never
        // inside the WebView2 IPC handler — that deadlocks, see tabs.rs), so
        // dispatch the creation to the main thread and wait for the handle.
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let window = window.clone();
            let url = db_tab.url.clone();
            let label = label.clone();
            let pos = LogicalPosition::new(rect.x as f64, rect.y as f64);
            let size = LogicalSize::new(rect.width as f64, rect.height as f64);
            app.run_on_main_thread(move || {
                let webview_url = WebviewUrl::External(
                    url.parse().unwrap_or_else(|_| "about:blank".parse().unwrap())
                );
                let mut webview_builder = WebviewBuilder::new(&label, webview_url);
                if is_incognito {
                    // browser_extensions_enabled MUST match the parent incognito
                    // window's setting: WebView2 webviews sharing an app-data dir
                    // must agree on environment options or creation silently
                    // fails and the incognito content webview never loads. The
                    // InPrivate profile holds no installed extensions, so none
                    // actually load.
                    webview_builder = webview_builder.incognito(true).browser_extensions_enabled(true);
                } else {
                    // Enable extensions on the shared profile. The profile
                    // already holds every enabled extension (persisted, kept in
                    // sync at startup/install) — do NOT AddBrowserExtension
                    // here: it cancels this webview's own initial navigation
                    // (the "restored tab loads a blank page" root cause).
                    webview_builder = webview_builder.browser_extensions_enabled(true);
                }
                let res = window.add_child(webview_builder, pos, size).map_err(|e| e.to_string());
                let _ = tx.send(res);
            }).map_err(|e| e.to_string())?;
        }
        // Bounded wait: if the main thread is wedged, fail the activation
        // instead of holding the pool lock forever and freezing the app.
        let webview = rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "webview creation timed out on main thread".to_string())??;

        let webview_clone = webview.clone();

        // Create ContentView wrapper. Its internal bounds default to 800x600 and
        // set_visible re-applies that default — which shrank a webview that
        // add_child had just created at a larger (e.g. maximized) size. Seed the
        // real creation rect FIRST so a page opened while the window is already
        // maximized fills it (no Resized event fires afterward to correct it).
        let view = Box::new(WebView2ContentView::new(ViewId(id as u32), serial, webview, is_incognito));
        view.set_bounds(rect);
        view.set_visible(true);
        view.focus();

        // Restore scroll position after DOMContentLoaded
        if db_tab.scroll_y > 0.0 {
            let scroll_js = format!("window.scrollTo(0, {})", db_tab.scroll_y);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                let _ = webview_clone.eval(&scroll_js);
            });
        }

        self.tabs.insert(id, TabState::Live {
            view: view as Box<dyn ContentView + Send + Sync>,
            last_active: Instant::now(),
        });

        raise_overlay(app, &window_label);
        crate::app::overlay_mark_content(app, &window_label, true);
        Ok(())
    }

    fn is_pinned(db: &crate::db::DbState, id: i32) -> bool {
        if id < 0 {
            return crate::incognito::get_incognito_tab(id).map(|t| t.pinned).unwrap_or(false);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        db.execute(move |conn| {
            let pinned = tabs_repo::get_tab(conn, id)
                .map(|opt| opt.map(|t| t.pinned).unwrap_or(false))
                .unwrap_or(false);
            let _ = tx.send(pinned);
        });
        rx.recv().unwrap_or(false)
    }

    /// The 30s background sweep: freeze tabs idle past the suspend threshold
    /// and (memory saver) unload unpinned ones idle past the discard threshold.
    /// Busy views (audio, downloads, picture-in-picture) are never touched.
    pub fn suspend_idle(&mut self, db: &crate::db::DbState) {
        let now = Instant::now();
        let mut keys_to_suspend = Vec::new();
        let mut keys_to_discard = Vec::new();
        let suspend_after = crate::prefs::suspend_after();
        let discard_after = crate::prefs::discard_after();

        for (&id, state) in &self.tabs {
            if Some(id) == self.active_tab_id {
                continue;
            }
            let (view, last_active, live) = match state {
                TabState::Live { view, last_active } => (view, last_active, true),
                TabState::Suspended { view, last_active } => (view, last_active, false),
                TabState::Cold => continue,
            };
            let idle = now.duration_since(*last_active);
            let wants_suspend = live && suspend_after.map_or(false, |t| idle > t);
            let wants_discard = discard_after.map_or(false, |t| idle > t);
            if !wants_suspend && !wants_discard {
                continue;
            }
            if Self::is_busy(view.as_ref()) {
                continue;
            }
            let pinned = Self::is_pinned(db, id);
            if wants_discard && !pinned {
                keys_to_discard.push(id);
            } else if wants_suspend {
                // Pinned tabs stay live longer before freezing.
                let threshold = if pinned { Duration::from_secs(30 * 60) } else { Duration::ZERO };
                if idle > threshold {
                    keys_to_suspend.push(id);
                }
            }
        }

        for id in keys_to_discard {
            tracing::info!("memory saver: unloading idle tab {}", id);
            self.unload_to_cold(db, id);
        }
        self.reap_parked();

        for id in keys_to_suspend {
            if let Some(TabState::Live { view, last_active }) = self.tabs.remove(&id) {
                // Call try_suspend
                let (tx, rx) = std::sync::mpsc::channel();
                view.try_suspend(Box::new(move |success| {
                    let _ = tx.send(success);
                }));

                let suspended = rx.recv_timeout(Duration::from_millis(500)).unwrap_or(false);

                if suspended {
                    self.tabs.insert(id, TabState::Suspended { view, last_active });
                } else {
                    // Put back as Live if suspend failed
                    self.tabs.insert(id, TabState::Live { view, last_active });
                }
            }
        }
    }

    pub fn mute_tab(&self, id: i32, muted: bool) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.get(&id) {
            view.mute(muted);
        }
    }

    pub fn suspend_all(&mut self, db: &crate::db::DbState, window_id: i32) {
        let mut keys_to_suspend = Vec::new();
        for (&id, state) in &self.tabs {
            if Some(id) == self.active_tab_id {
                continue;
            }
            if let TabState::Live { view, .. } = state {
                if Self::is_busy(view.as_ref()) {
                    continue;
                }
                let win = if id < 0 {
                    crate::incognito::get_incognito_tab(id)
                        .map(|t| t.window_id)
                        .unwrap_or(0)
                } else {
                    let (tx, rx) = std::sync::mpsc::channel();
                    db.execute(move |conn| {
                        let win = tabs_repo::get_tab(conn, id)
                            .map(|opt| opt.map(|t| t.window_id).unwrap_or(0))
                            .unwrap_or(0);
                        let _ = tx.send(win);
                    });
                    rx.recv().unwrap_or(0)
                };
                if win == window_id {
                    keys_to_suspend.push(id);
                }
            }
        }

        for id in keys_to_suspend {
            if let Some(TabState::Live { view, last_active }) = self.tabs.remove(&id) {
                let (tx, rx) = std::sync::mpsc::channel();
                view.try_suspend(Box::new(move |success| {
                    let _ = tx.send(success);
                }));
                let suspended = rx.recv_timeout(Duration::from_millis(500)).unwrap_or(false);
                if suspended {
                    self.tabs.insert(id, TabState::Suspended { view, last_active });
                } else {
                    self.tabs.insert(id, TabState::Live { view, last_active });
                }
            }
        }
    }

    pub fn resize_tab(&mut self, id: i32, rect: crate::engine::Rect) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.get_mut(&id) {
            view.set_bounds(rect);
        }
    }

    /// Give a tab's webview keyboard focus (used after closing a tab so page
    /// accelerators keep working without a click).
    pub fn focus_tab(&self, id: i32) {
        if let Some(TabState::Live { view, .. }) = self.tabs.get(&id) {
            view.focus();
        }
    }

    /// Which tabs currently have a real webview: id → "live" | "suspended".
    /// Cold/unloaded tabs are absent. Drives the tab-panel state dots and the
    /// Unload menu item's enabled state.
    pub fn loaded_states(&self) -> Vec<(i32, &'static str)> {
        self.tabs
            .iter()
            .filter_map(|(&id, state)| match state {
                TabState::Live { .. } => Some((id, "live")),
                TabState::Suspended { .. } => Some((id, "suspended")),
                TabState::Cold => None,
            })
            .collect()
    }

    /// Is this tab currently loaded (has a webview)?
    pub fn is_loaded(&self, id: i32) -> bool {
        matches!(
            self.tabs.get(&id),
            Some(TabState::Live { .. } | TabState::Suspended { .. })
        )
    }

    /// Set the Chromium memory-usage target on every live/suspended view.
    /// LOW while the window is hidden trims caches without freezing JS or
    /// pausing media.
    pub fn set_memory_target_all(&self, low: bool) {
        for state in self.tabs.values() {
            if let TabState::Live { view, .. } | TabState::Suspended { view, .. } = state {
                view.set_memory_target(low);
            }
        }
    }

    /// Window shown again: only the visible tab goes back to NORMAL; background
    /// tabs keep the LOW target they get when left.
    pub fn restore_active_memory_target(&self) {
        if let Some(id) = self.active_tab_id {
            if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.get(&id) {
                view.set_memory_target(false);
            }
        }
    }

    /// The window was minimized or restored. While minimized the visible page
    /// is marked hidden, so it stops painting and its timers are throttled,
    /// and every view trims its memory. Restoring shows the page again at
    /// normal memory use; background tabs keep their LOW target.
    pub fn set_window_minimized(&self, minimized: bool) {
        if minimized {
            self.set_memory_target_all(true);
        } else {
            self.restore_active_memory_target();
        }
        if let Some(id) = self.active_tab_id {
            if let Some(TabState::Live { view, .. }) = self.tabs.get(&id) {
                view.set_page_hidden(minimized);
            }
        }
    }

    /// Picture-in-picture request for the active tab's page.
    pub fn pip_active(&self, req: crate::engine::PipRequest) -> Result<(), String> {
        self.with_active_view(|v| v.picture_in_picture(req))
    }

    /// Discard a tab's live webview (free memory) while keeping the tab row, and
    /// clear it as the active tab. The tab goes Cold and reloads on next
    /// activation ("Unload tab").
    pub fn discard_tab(&mut self, id: i32) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
            self.retire(view);
        }
        self.tabs.insert(id, TabState::Cold);
        if self.active_tab_id == Some(id) {
            self.active_tab_id = None;
        }
        self.mru.retain(|&x| x != id);
    }

    fn with_active_view<F: FnOnce(&(dyn ContentView + Send + Sync))>(&self, f: F) -> Result<(), String> {
        let id = self.active_tab_id.ok_or_else(|| "No active tab".to_string())?;
        match self.tabs.get(&id) {
            Some(TabState::Live { view, .. }) | Some(TabState::Suspended { view, .. }) => {
                f(view.as_ref());
                Ok(())
            }
            _ => Err("Active tab has no live view".to_string()),
        }
    }

    pub fn nav_back(&self) -> Result<(), String> {
        self.with_active_view(|v| v.back())
    }

    pub fn nav_forward(&self) -> Result<(), String> {
        self.with_active_view(|v| v.forward())
    }

    pub fn nav_reload(&self) -> Result<(), String> {
        self.with_active_view(|v| v.reload())
    }

    /// If tab `id` has a live view whose document is still about:blank even
    /// though `url` was supposed to load, re-issue the navigation. This is the
    /// signature of a CANCELED initial navigation (WebView2 aborts in-flight
    /// navigations profile-wide when an extension is added, among other causes):
    /// the tab looks open but shows nothing, and Reload would only reload the
    /// blank document. A blank document is expected while a slow page is still
    /// loading and after a navigation turned into a download — re-navigating
    /// then restarts the load or downloads the file twice — so those wait,
    /// unless `user_reload` (the user asked to retry the load).
    pub fn heal_blank_tab(&self, id: i32, url: &str, user_reload: bool) -> BlankHeal {
        if url.is_empty() || url == "about:blank" {
            return BlankHeal::NotNeeded;
        }
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.get(&id) {
            let cur = view.snapshot().url;
            if cur.is_empty() || cur == "about:blank" {
                if !view.blank_heal_allowed() && !user_reload {
                    return BlankHeal::Wait;
                }
                tracing::warn!(
                    "tab {}: initial navigation was lost (blank document, expected {}) — re-navigating",
                    id, url
                );
                view.navigate(url);
                return BlankHeal::Healed;
            }
        }
        BlankHeal::NotNeeded
    }

    /// Public wrapper for the post-activation heal check (tabs.rs).
    pub fn stored_url_pub(db: &crate::db::DbState, id: i32) -> Option<String> {
        Self::stored_url(db, id)
    }

    /// The DB/incognito-store URL for a tab (what the tab is SUPPOSED to show).
    fn stored_url(db: &crate::db::DbState, id: i32) -> Option<String> {
        if id < 0 {
            return crate::incognito::get_incognito_tab(id).map(|t| t.url);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let db = db.clone();
        db.execute(move |conn| {
            let _ = tx.send(tabs_repo::get_tab(conn, id).ok().flatten().map(|t| t.url));
        });
        rx.recv().ok().flatten()
    }

    /// Reload the active tab, recovering the two broken states a plain Reload
    /// can't: a COLD tab (failed activation — nothing to reload) is re-activated,
    /// and a live view stuck on about:blank (canceled initial navigation) is
    /// re-navigated to its stored URL.
    pub fn reload_or_reactivate(
        &mut self,
        db: &crate::db::DbState,
        app: &AppHandle,
    ) -> Result<(), String> {
        let id = self.active_tab_id.ok_or_else(|| "No active tab".to_string())?;
        if let Some(url) = Self::stored_url(db, id) {
            if self.heal_blank_tab(id, &url, true) == BlankHeal::Healed {
                return Ok(());
            }
        }
        if self.nav_reload().is_ok() {
            return Ok(());
        }
        tracing::warn!("nav_reload: active tab {} has no live view — re-activating", id);
        self.activate_tab(db, id, app)
    }

    /// Return tab `id` to the new-tab page: its view is retired (parked if still
    /// downloading) and its stored URL cleared, so neither activation nor a
    /// session restore reloads what it was showing.
    pub fn reset_to_new_tab(
        &mut self,
        db: &crate::db::DbState,
        app: &AppHandle,
        id: i32,
    ) -> Result<(), String> {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
            self.retire(view);
        }
        self.tabs.insert(id, TabState::Cold);
        if id < 0 {
            crate::incognito::update_incognito_tab(id, "about:blank".to_string(), None, 0.0);
        } else {
            let (tx, rx) = std::sync::mpsc::channel();
            db.execute(move |conn| {
                if let Ok(Some(mut db_tab)) = tabs_repo::get_tab(conn, id) {
                    db_tab.url = "about:blank".to_string();
                    db_tab.title = None;
                    db_tab.scroll_y = 0.0;
                    let _ = tabs_repo::update_tab(conn, &db_tab);
                }
                let _ = tx.send(());
            });
            let _ = rx.recv();
        }
        self.activate_tab(db, id, app)
    }

    pub fn nav_stop(&self) -> Result<(), String> {
        self.with_active_view(|v| v.stop())
    }

    pub fn navigate_tab(
        &mut self,
        db: &crate::db::DbState,
        app: &AppHandle,
        id: i32,
        url: &str,
    ) -> Result<(), String> {
        let is_blank = url.is_empty() || url == "about:blank";
        
        let state_is_live = match self.tabs.get(&id) {
            Some(TabState::Live { .. } | TabState::Suspended { .. }) => true,
            Some(TabState::Cold) => false,
            None => return Err("Tab not found".to_string()),
        };

        if state_is_live {
            if is_blank {
                self.reset_to_new_tab(db, app, id)
            } else {
                if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.get(&id) {
                    view.navigate(url);
                    // Address-bar Enter lands here: hand keyboard focus to the
                    // page so scrolling/shortcuts work without a click.
                    view.focus();
                }
                Ok(())
            }
        } else {
            if id < 0 {
                crate::incognito::update_incognito_tab(id, url.to_string(), None, 0.0);
            } else {
                let (tx, rx) = std::sync::mpsc::channel();
                let db_clone = db.clone();
                let url_str = url.to_string();
                db_clone.execute(move |conn| {
                    if let Ok(Some(mut db_tab)) = tabs_repo::get_tab(conn, id) {
                        db_tab.url = url_str;
                        let _ = tabs_repo::update_tab(conn, &db_tab);
                    }
                    let _ = tx.send(());
                });
                let _ = rx.recv();
            }
            self.activate_tab(db, id, app)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Rect, TabRuntimeState};

    struct FakeView;
    impl ContentView for FakeView {
        fn id(&self) -> ViewId { ViewId(0) }
        fn navigate(&self, _url: &str) {}
        fn back(&self) {}
        fn forward(&self) {}
        fn reload(&self) {}
        fn stop(&self) {}
        fn set_bounds(&self, _rect: Rect) {}
        fn set_visible(&self, _v: bool) {}
        fn try_suspend(&self, done: Box<dyn FnOnce(bool) + Send>) { done(false) }
        fn resume(&self) {}
        fn snapshot(&self) -> TabRuntimeState {
            TabRuntimeState {
                url: String::new(), title: String::new(), favicon_url: String::new(),
                scroll_y: 0.0, can_go_back: false, can_go_forward: false,
            }
        }
        fn mute(&self, _m: bool) {}
        fn find(&self, _text: &str, _forward: bool) {}
        fn zoom(&self, _factor: f64) {}
        fn close(self: Box<Self>) {}
    }

    fn live() -> TabState {
        TabState::Live { view: Box::new(FakeView), last_active: Instant::now() }
    }

    #[test]
    fn closing_lands_on_most_recent_loaded_tab_never_a_cold_one() {
        let mut p = TabPool::new();
        p.tabs.insert(1, TabState::Cold);
        p.tabs.insert(2, live());
        p.tabs.insert(4, live());
        p.tabs.insert(5, TabState::Cold);
        // Closing 3; most recent before it was 1 (cold), then 4, then 2.
        p.mru = vec![3, 1, 4, 2];
        assert_eq!(p.next_loaded_tab(3, &[1, 2, 4, 5]), Some(4));

        // A loaded tab that was never activated (opened in the background) is
        // still preferred over a cold neighbour.
        p.mru = vec![3, 1];
        assert_eq!(p.next_loaded_tab(3, &[1, 2, 4, 5]), Some(2));

        // Only cold tabs left → None, i.e. the new-tab page.
        assert_eq!(p.next_loaded_tab(3, &[1, 5]), None);
    }
}
