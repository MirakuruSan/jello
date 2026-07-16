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
            view.close();
        }
        if self.active_tab_id == Some(id) {
            self.active_tab_id = None;
        }
        self.mru.retain(|&x| x != id);
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
                    if view.is_audio_playing() {
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
            if let Some(state) = self.tabs.remove(&id) {
                match state {
                    TabState::Live { view, .. } | TabState::Suspended { view, .. } => {
                        let snapshot = view.snapshot();
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
                        view.close();
                    }
                    _ => {}
                }
                self.tabs.insert(id, TabState::Cold);
                return true;
            }
        }
        false
    }

    pub fn activate_tab(&mut self, db: &crate::db::DbState, id: i32, app: &AppHandle) -> Result<(), String> {
        let prev_active = self.active_tab_id;
        self.active_tab_id = Some(id);
        self.touch_mru(id);

        // Hide all other views. Titles/URLs are now event-driven (COM handlers),
        // so only the tab we're switching *away from* needs a scroll-position
        // capture — not every live tab. This removes the N-async-eval fan-out
        // that ran under the pool lock on every switch (Phase 3 new-tab perf).
        for (&other_id, other_state) in &self.tabs {
            if other_id != id {
                match other_state {
                    TabState::Live { view, .. } | TabState::Suspended { view, .. } => {
                        if Some(other_id) == prev_active {
                            view.refresh_snapshot();
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

        // It is Cold. Evict LRU if active count >= 5
        while self.get_active_count() >= 5 {
            if !self.evict_lru(db) {
                break;
            }
        }

        let label = format!("content_tab_{}", id);
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
            let app_for_ext = app.clone();
            app.run_on_main_thread(move || {
                let webview_url = WebviewUrl::External(
                    url.parse().unwrap_or_else(|_| "about:blank".parse().unwrap())
                );
                let mut webview_builder = WebviewBuilder::new(&label, webview_url);
                if is_incognito {
                    webview_builder = webview_builder.incognito(true);
                } else {
                    // Enable extensions on the shared profile; the actual loading
                    // is done per-extension via explicit AddBrowserExtension COM
                    // calls after add_child (P1.1) — NOT the destructive
                    // extensions_path staging that corrupted under a live session.
                    webview_builder = webview_builder.browser_extensions_enabled(true);
                }
                let res = window.add_child(webview_builder, pos, size).map_err(|e| e.to_string());
                if let Ok(ref wv) = res {
                    if !is_incognito {
                        crate::extensions::load_all_enabled(&app_for_ext, wv);
                    }
                }
                let _ = tx.send(res);
            }).map_err(|e| e.to_string())?;
        }
        // Bounded wait: if the main thread is wedged, fail the activation
        // instead of holding the pool lock forever (past freeze root cause).
        let webview = rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| "webview creation timed out on main thread".to_string())??;

        let webview_clone = webview.clone();

        // Create ContentView wrapper. Its internal bounds default to 800x600 and
        // set_visible re-applies that default — which shrank a webview that
        // add_child had just created at a larger (e.g. maximized) size. Seed the
        // real creation rect FIRST so a page opened while the window is already
        // maximized fills it (no Resized event fires afterward to correct it).
        let view = Box::new(WebView2ContentView::new(ViewId(id as u32), webview, is_incognito));
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

    pub fn suspend_idle(&mut self, db: &crate::db::DbState) {
        let now = Instant::now();
        let mut keys_to_suspend = Vec::new();

        for (&id, state) in &self.tabs {
            if Some(id) == self.active_tab_id {
                continue;
            }

            if let TabState::Live { view, last_active } = state {
                if view.is_audio_playing() {
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

                let idle_threshold = if is_pinned {
                    Duration::from_secs(30 * 60) // Pinned: 30 mins
                } else {
                    Duration::from_secs(5 * 60) // Unpinned: 5 mins
                };

                if now.duration_since(*last_active) > idle_threshold {
                    keys_to_suspend.push(id);
                }
            }
        }

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
                if view.is_audio_playing() {
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
    /// accelerators keep working without a click — #3).
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
    /// pausing media; NORMAL restores full performance on show.
    pub fn set_memory_target_all(&self, low: bool) {
        for state in self.tabs.values() {
            if let TabState::Live { view, .. } | TabState::Suspended { view, .. } = state {
                view.set_memory_target(low);
            }
        }
    }

    /// Discard a tab's live webview (free memory) while keeping the tab row, and
    /// clear it as the active tab. The tab goes Cold and reloads on next
    /// activation (#10 "unload tab").
    pub fn discard_tab(&mut self, id: i32) {
        if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
            view.close();
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
                if let Some(TabState::Live { view, .. } | TabState::Suspended { view, .. }) = self.tabs.remove(&id) {
                    view.close();
                }
                self.tabs.insert(id, TabState::Cold);
                self.activate_tab(db, id, app)
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
