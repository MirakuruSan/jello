use tauri::{command, AppHandle, Manager, State, Emitter};
use std::sync::{Arc, Mutex};
use crate::db::DbState;
use crate::engine::pool::TabPool;
use crate::ipc_types::{PaletteItem, PaletteResults};
use crate::search::{classify_input, InputClassification, get_search_engines, route_query};

/// A tab's display title, falling back to its URL host (not "Untitled") when the
/// title is empty — after Phase 3 titles are usually fresh, but new/blank tabs
/// still benefit from showing the host (Phase 8 item 4).
fn tab_display_title(title: &Option<String>, url: &str) -> String {
    if let Some(t) = title {
        if !t.trim().is_empty() {
            return t.clone();
        }
    }
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(|h| h.trim_start_matches("www.").to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| url.to_string())
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct PaletteOpenPayload {
    mode: String,
    prefill: String,
}

/// Show the palette in a given mode ("search" | "newtab" | "addressbar") with an
/// optional prefill string, emitting palette:open so the controller configures
/// its input.
pub fn show_palette(app: &AppHandle, mode: &str, prefill: &str) {
    let window = match app.get_webview_window("palette") {
        Some(w) => w,
        None => match crate::windows::create_palette_window(app) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("Failed to create palette window: {}", e);
                return;
            }
        }
    };

    // Anchor near the upper third, horizontally centered (Alt+Space style).
    // Reset to the compact height on each open so it doesn't reopen tall.
    if let Ok(Some(monitor)) = window.current_monitor() {
        let m_size = monitor.size();
        let m_pos = monitor.position();
        let scale_factor = monitor.scale_factor();
        let w_width = 680.0;

        let x = m_pos.x as f64 + (m_size.width as f64 - w_width * scale_factor) / 2.0;
        let y = m_pos.y as f64 + (m_size.height as f64) * 0.18;

        let _ = window.set_size(tauri::LogicalSize::new(680.0, 60.0));
        let _ = window.set_position(tauri::PhysicalPosition::new(x as i32, y as i32));
    }
    let _ = window.show();
    let _ = window.set_focus();

    let _ = app.emit("palette:open", PaletteOpenPayload {
        mode: mode.to_string(),
        prefill: prefill.to_string(),
    });
}

/// Async: may lazily CREATE the palette window — must not run inside the
/// WebView2 IPC handler on the main thread (deadlock; see tabs.rs).
#[command]
pub async fn palette_show(app: AppHandle, mode: Option<String>, prefill: Option<String>) -> Result<(), String> {
    show_palette(&app, mode.as_deref().unwrap_or("search"), prefill.as_deref().unwrap_or(""));
    Ok(())
}

#[command]
pub fn palette_hide(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_window("palette") {
        let _ = window.hide();
    }
    Ok(())
}

/// Grow/shrink the palette window to fit its content (input + result rows).
/// Width stays fixed; the frontend passes the desired logical height.
#[command]
pub async fn palette_resize(app: AppHandle, height: f64) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("palette") {
        let h = height.clamp(60.0, 480.0);
        window.set_size(tauri::LogicalSize::new(680.0, h)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[command]
pub async fn palette_query(
    _app: AppHandle,
    text: String,
    scope: String,
    db: State<'_, DbState>,
    _pool: State<'_, Arc<Mutex<TabPool>>>,
) -> Result<PaletteResults, String> {
    let query_trimmed = text.trim();

    // 1. Fetch Open Tabs
    let (tx_tabs, rx_tabs) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let res = crate::db::tabs_repo::list_all_tabs(conn);
        let _ = tx_tabs.send(res);
    });
    let all_tabs = rx_tabs.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;

    // 2. Fetch History (top 512 frecent)
    let (tx_hist, rx_hist) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let res = crate::db::history::get_top_history_by_frecency(conn, 512);
        let _ = tx_hist.send(res);
    });
    let history_entries = rx_hist.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;

    // 3. Fetch Bookmarks
    let (tx_bm, rx_bm) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let res = crate::db::bookmarks::list_bookmarks(conn);
        let _ = tx_bm.send(res);
    });
    let bookmark_entries = rx_bm.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;

    // If query is empty, return defaults
    if query_trimmed.is_empty() {
        let mut open_tabs_res = Vec::new();
        for t in all_tabs.iter().take(4) {
            open_tabs_res.push(PaletteItem {
                id: format!("tab:{}", t.id),
                item_type: "tab".to_string(),
                title: tab_display_title(&t.title, &t.url),
                url: t.url.clone(),
                matched_ranges: Vec::new(),
            });
        }

        let mut history_res = Vec::new();
        for h in history_entries.iter().take(6) {
            history_res.push(PaletteItem {
                id: format!("history:{}", h.id),
                item_type: "history".to_string(),
                title: if h.title.is_empty() { h.url.clone() } else { h.title.clone() },
                url: h.url.clone(),
                matched_ranges: Vec::new(),
            });
        }

        let mut bookmarks_res = Vec::new();
        for b in bookmark_entries.iter().take(4) {
            bookmarks_res.push(PaletteItem {
                id: format!("bookmark:{}", b.id),
                item_type: "bookmark".to_string(),
                title: if b.title.is_empty() { b.url.clone() } else { b.title.clone() },
                url: b.url.clone(),
                matched_ranges: Vec::new(),
            });
        }

        return Ok(PaletteResults {
            open_tabs: open_tabs_res,
            history: history_res,
            bookmarks: bookmarks_res,
        });
    }

    // Initialize nucleo matcher
    use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};
    use nucleo_matcher::{Matcher, Config, Utf32String, Utf32Str};
    
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query_trimmed, CaseMatching::Ignore, Normalization::Smart);

    // Fuzzy matching helper
    let mut match_item = |title: &str, url: &str| -> Option<(u32, Vec<(usize, usize)>)> {
        let title_utf32 = Utf32String::from(title);
        let mut title_indices = Vec::new();
        let title_slice = match &title_utf32 {
            Utf32String::Ascii(v) => Utf32Str::Ascii(v.as_bytes()),
            Utf32String::Unicode(v) => Utf32Str::Unicode(v),
        };
        let title_score = pattern.indices(title_slice, &mut matcher, &mut title_indices);

        let url_utf32 = Utf32String::from(url);
        let mut url_indices = Vec::new();
        let url_slice = match &url_utf32 {
            Utf32String::Ascii(v) => Utf32Str::Ascii(v.as_bytes()),
            Utf32String::Unicode(v) => Utf32Str::Unicode(v),
        };
        let url_score = pattern.indices(url_slice, &mut matcher, &mut url_indices);

        if title_score.is_none() && url_score.is_none() {
            return None;
        }

        let score = title_score.unwrap_or(0).max(url_score.unwrap_or(0));

        let mut matched_ranges = Vec::new();
        if !title_indices.is_empty() {
            title_indices.sort_unstable();
            let mut start = title_indices[0] as usize;
            let mut end = start + 1;
            for &idx in &title_indices[1..] {
                let idx = idx as usize;
                if idx == end {
                    end = idx + 1;
                } else {
                    matched_ranges.push((start, end));
                    start = idx;
                    end = idx + 1;
                }
            }
            matched_ranges.push((start, end));
        }

        Some((score, matched_ranges))
    };

    // Filter and score open tabs
    let mut open_tabs_matches = Vec::new();
    if scope == "all" || scope == "tabs" {
        for t in &all_tabs {
            let title = tab_display_title(&t.title, &t.url);
            if let Some((score, ranges)) = match_item(&title, &t.url) {
                open_tabs_matches.push((score, PaletteItem {
                    id: format!("tab:{}", t.id),
                    item_type: "tab".to_string(),
                    title: title.clone(),
                    url: t.url.clone(),
                    matched_ranges: ranges,
                }));
            }
        }
        open_tabs_matches.sort_by(|a, b| b.0.cmp(&a.0));
    }

    // Filter and score history
    let mut history_matches = Vec::new();
    if scope == "all" || scope == "history" {
        for h in &history_entries {
            let title = if h.title.is_empty() { &h.url } else { &h.title };
            if let Some((score, ranges)) = match_item(title, &h.url) {
                // Decay score slightly based on frecency to combine matching + frecency
                let combined_score = score + (h.visit_count as u32).min(50);
                history_matches.push((combined_score, PaletteItem {
                    id: format!("history:{}", h.id),
                    item_type: "history".to_string(),
                    title: title.to_string(),
                    url: h.url.clone(),
                    matched_ranges: ranges,
                }));
            }
        }
        history_matches.sort_by(|a, b| b.0.cmp(&a.0));
    }

    // Filter and score bookmarks
    let mut bookmarks_matches = Vec::new();
    if scope == "all" || scope == "bookmarks" {
        for b in &bookmark_entries {
            let title = if b.title.is_empty() { &b.url } else { &b.title };
            if let Some((score, ranges)) = match_item(title, &b.url) {
                bookmarks_matches.push((score, PaletteItem {
                    id: format!("bookmark:{}", b.id),
                    item_type: "bookmark".to_string(),
                    title: title.to_string(),
                    url: b.url.clone(),
                    matched_ranges: ranges,
                }));
            }
        }
        bookmarks_matches.sort_by(|a, b| b.0.cmp(&a.0));
    }

    Ok(PaletteResults {
        open_tabs: open_tabs_matches.into_iter().map(|x| x.1).take(4).collect(),
        history: history_matches.into_iter().map(|x| x.1).take(6).collect(),
        bookmarks: bookmarks_matches.into_iter().map(|x| x.1).take(4).collect(),
    })
}

#[command]
pub async fn palette_open(
    app: AppHandle,
    id: String,
    url: String,
    disposition: String,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
) -> Result<(), String> {
    // 1. Bring the main browser window forward FIRST — otherwise the tab opens
    //    into a hidden/backgrounded window and the user sees "nothing happened"
    //    while the palette lingers. (Same class of bug as the old capture paths.)
    crate::windows::ensure_main_window(&app);

    // 2. Hide the palette.
    if let Some(window) = app.get_window("palette") {
        let _ = window.hide();
    }

    // 3. Classify / route url if it's a search
    let target_url = if id.starts_with("search:") {
        // Fetch search engines
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = get_search_engines(conn);
            let _ = tx.send(res);
        });
        let engines = rx.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;
        
        let default_engine = "https://duckduckgo.com/?q=%s";
        let classification = classify_input(&url);
        
        match classification {
            InputClassification::Url(u) => u,
            InputClassification::SearchQuery(q) => route_query(&q, &engines, default_engine),
        }
    } else {
        url.clone()
    };

    // 3. Perform open operation based on disposition
    match disposition.as_str() {
        "current-tab" => {
            if let Some(stripped) = id.strip_prefix("tab:") {
                let tab_id = stripped.parse::<i32>().map_err(|e| e.to_string())?;
                crate::tabs::tabs_activate_impl(tab_id, &db, &pool, &app)?;
            } else {
                // Create tab or navigate current tab
                let tab_id = {
                    let pool_guard = pool.lock().unwrap();
                    pool_guard.get_active_tab_id()
                };
                if let Some(tid) = tab_id {
                    // Navigate active tab
                    let mut pool_guard = pool.lock().unwrap();
                    pool_guard.navigate_tab(&db, &app, tid, &target_url)?;
                } else {
                    // Create new tab
                    crate::tabs::tabs_create_impl(Some(target_url), Some(false), None, &db, &pool, &app)?;
                }
            }
        }
        "new-tab-foreground" => {
            crate::tabs::tabs_create_impl(Some(target_url), Some(false), None, &db, &pool, &app)?;
        }
        "new-tab-background" => {
            crate::tabs::tabs_create_impl(Some(target_url), Some(true), None, &db, &pool, &app)?;
        }
        "new-window" => {
            // Mask the id the same way window_new_impl/incognito do (Phase 8
            // item 2): the label id must equal the value parsed back by the
            // frontend, or the new window queries the wrong window's tabs.
            let id = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() & 0x7FFFFFFF) as i32;
            let label = format!("main_{}", id);
            let encoded_url = crate::search::percent_encode(&target_url);
            let window_url = format!("index.html?window_id={}&initial_url={}", id, encoded_url);

            let _ = tauri::WebviewWindowBuilder::new(
                &app,
                &label,
                tauri::WebviewUrl::App(window_url.into()),
            )
            .inner_size(800.0, 600.0)
            .title("Jello")
            .decorations(false)
            .transparent(true)
            // Consistent with the other app windows so the shared WebView2
            // environment agrees on extensions (Phase 4).
            .browser_extensions_enabled(true)
            .build()
            .map_err(|e| e.to_string())?;

            // Overlay pass-through + resize plumbing, same as other windows
            // (Phase 8 item 12 — palette-created windows lacked it).
            if let Some(win) = app.get_window(&label) {
                crate::app::attach_window_plumbing(&app, win);
            }
        }
        _ => return Err(format!("Unknown disposition: {}", disposition)),
    }

    Ok(())
}
