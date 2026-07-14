use tauri::{command, AppHandle, Manager, State, Emitter};
use std::sync::{Arc, Mutex};
use crate::db::{DbState, tabs_repo};
use crate::ipc_types::Tab;
use crate::engine::pool::TabPool;
use crate::engine::fractional_index::generate_key_between;

// Sync implementation, callable from any non-main thread (chords, deeplink,
// capture). NEVER call from a sync #[command]: webview creation (add_child)
// cannot complete inside the WebView2 IPC event handler on the main thread —
// it deadlocks the app. Commands below are async so they run on the runtime
// thread pool instead.
pub fn tabs_activate_impl(
    id: i32,
    db: &DbState,
    pool: &Arc<Mutex<TabPool>>,
    app: &AppHandle,
) -> Result<(), String> {
    let mut pool_guard = pool.lock().unwrap();
    pool_guard.activate_tab(db, id, app)?;

    // Update last_active
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    if id < 0 {
        crate::incognito::update_incognito_tab_last_active(id, now);
        if let Some(tab) = crate::incognito::get_incognito_tab(id) {
            let _ = app.emit("tab:activated", id);
            let _ = app.emit("tab:updated", &tab);
        }
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = (|| -> rusqlite::Result<Option<Tab>> {
                if let Some(mut tab) = tabs_repo::get_tab(conn, id)? {
                    tab.last_active = Some(now);
                    tabs_repo::update_tab(conn, &tab)?;
                    Ok(Some(tab))
                } else {
                    Ok(None)
                }
            })();
            let _ = tx.send(res);
        });

        let updated_tab = rx.recv().unwrap_or(Ok(None))
            .map_err(|e| e.to_string())?;

        if let Some(tab) = updated_tab {
            let _ = app.emit("tab:activated", id);
            let _ = app.emit("tab:updated", &tab);
        }
    }

    Ok(())
}

pub fn tabs_list_impl(window_id: i32, db: &DbState) -> Result<Vec<Tab>, String> {
    if crate::incognito::is_incognito_window(window_id) {
        return Ok(crate::incognito::list_incognito_tabs(window_id));
    }

    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = tabs_repo::list_tabs(conn, window_id);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
        .map_err(|e| e.to_string())
}

#[command]
pub async fn tabs_list(window_id: i32, db: State<'_, DbState>) -> Result<Vec<Tab>, String> {
    tabs_list_impl(window_id, &db)
}

pub fn tabs_create_impl(
    url: Option<String>,
    background: Option<bool>,
    window_id: Option<i32>,
    db: &DbState,
    pool: &Arc<Mutex<TabPool>>,
    app: &AppHandle,
) -> Result<Tab, String> {
    let win_id = window_id.unwrap_or(1);
    let url_val = url.unwrap_or_else(|| "about:blank".to_string());
    let bg = background.unwrap_or(false);

    // Incognito windows keep tabs in an in-memory store — compute the order key
    // synchronously there (no DB thread involved).
    if crate::incognito::is_incognito_window(win_id) {
        let tabs = tabs_list_impl(win_id, db)?;
        let last_key = tabs.last().map(|t| t.order_key.as_str());
        let order_key = generate_key_between(last_key, None);
        let new_tab = crate::incognito::create_incognito_tab(win_id, url_val, order_key);
        let _ = app.emit("tab:created", &new_tab);
        if !bg {
            tabs_activate_impl(new_tab.id, db, pool, app)?;
        }
        return Ok(new_tab);
    }

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Single DB round-trip for the new-tab fast path: compute the order key,
    // insert the row, and read it back in one message to the DB thread. This
    // was previously a separate `tabs_list` round-trip followed by insert+get
    // (Phase 3 — "new tab slow/sluggish").
    let (tx, rx) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let res = (|| -> rusqlite::Result<Tab> {
            let existing = tabs_repo::list_tabs(conn, win_id)?;
            let last_key = existing.last().map(|t| t.order_key.clone());
            let order_key = generate_key_between(last_key.as_deref(), None);
            let dummy_tab = Tab {
                id: 0,
                window_id: win_id,
                url: url_val,
                title: None,
                favicon_id: None,
                pinned: false,
                muted: false,
                order_key,
                scroll_y: 0.0,
                last_active: None,
                created_at,
            };
            let id = tabs_repo::insert_tab(conn, &dummy_tab)?;
            let tab = tabs_repo::get_tab(conn, id)?.unwrap();
            Ok(tab)
        })();
        let _ = tx.send(res);
    });

    let new_tab = rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
        .map_err(|e| e.to_string())?;

    let _ = app.emit("tab:created", &new_tab);

    if !bg {
        tabs_activate_impl(new_tab.id, db, pool, app)?;
    }

    Ok(new_tab)
}

#[command]
pub async fn tabs_create(
    url: Option<String>,
    background: Option<bool>,
    window_id: Option<i32>,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<Tab, String> {
    tabs_create_impl(url, background, window_id, &db, &pool, &app)
}

#[command]
pub async fn tabs_activate(
    id: i32,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    tabs_activate_impl(id, &db, &pool, &app)
}

#[command]
pub async fn tabs_close(
    id: i32,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    // Check if the tab is active
    let is_active = {
        let pool_guard = pool.lock().unwrap();
        pool_guard.get_active_tab_id() == Some(id)
    };

    // Get tab's window_id before delete
    let win_id = if id < 0 {
        crate::incognito::get_incognito_tab(id)
            .map(|t| t.window_id)
            .unwrap_or(0)
    } else {
        let (tx_win, rx_win) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let win = tabs_repo::get_tab(conn, id)
                .map(|opt| opt.map(|t| t.window_id).unwrap_or(0))
                .unwrap_or(0);
            let _ = tx_win.send(win);
        });
        rx_win.recv().unwrap_or(0)
    };

    // Snapshot the tab for the undo stack before removing it.
    let closing_tab = if id < 0 {
        crate::incognito::get_incognito_tab(id)
    } else {
        let (tx_s, rx_s) = std::sync::mpsc::channel();
        let db_clone_s = db.clone();
        db_clone_s.execute(move |conn| {
            let _ = tx_s.send(tabs_repo::get_tab(conn, id).ok().flatten());
        });
        rx_s.recv().unwrap_or(None)
    };

    // Remove from pool (closes webview)
    {
        let mut pool_guard = pool.lock().unwrap();
        pool_guard.remove_tab(id);
    }

    if id < 0 {
        if let Some(t) = closing_tab {
            crate::incognito::push_closed_incognito(t);
        }
        crate::incognito::delete_incognito_tab(id);
    } else {
        // Push onto the persistent undo stack, then delete from DB.
        if let Some(t) = closing_tab {
            let db_push = db.clone();
            db_push.execute(move |conn| {
                let _ = tabs_repo::push_closed_tab(conn, &t);
            });
        }
        let (tx_del, rx_del) = std::sync::mpsc::channel();
        let db_clone2 = db.clone();
        db_clone2.execute(move |conn| {
            let res = tabs_repo::delete_tab(conn, id);
            let _ = tx_del.send(res);
        });
        rx_del.recv().unwrap_or(Ok(()))
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit("tab:closed", id);

    // If it was the active tab, activate the next available tab in the window
    if is_active && win_id != 0 {
        let remaining = tabs_list_impl(win_id, &db)?;
        if !remaining.is_empty() {
            // Find closest index or just activate the last tab
            let next_id = remaining.last().unwrap().id;
            tabs_activate_impl(next_id, &db, &pool, &app)?;
        } else {
            // Reset active tab in pool; overlay takes input again (new-tab page).
            let mut pool_guard = pool.lock().unwrap();
            pool_guard.set_active_tab_id(None);
            drop(pool_guard);
            let label = if win_id == 1 {
                "main".to_string()
            } else if app.get_window(&format!("main_{}", win_id)).is_some() {
                format!("main_{}", win_id)
            } else {
                format!("incognito_{}", win_id)
            };
            crate::app::overlay_mark_content(&app, &label, false);
        }
    }

    Ok(())
}

#[command]
pub async fn tabs_reorder(
    id: i32,
    before_id: Option<i32>,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut tab_to_move = if id < 0 {
        crate::incognito::get_incognito_tab(id)
            .ok_or_else(|| "Tab not found".to_string())?
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = tabs_repo::get_tab(conn, id);
            let _ = tx.send(res);
        });
        rx.recv().unwrap_or(Ok(None))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Tab not found".to_string())?
    };

    let win_id = tab_to_move.window_id;

    // List all tabs in the window
    let all_tabs = tabs_list_impl(win_id, &db)?;

    // Filter out target tab
    let filtered_tabs: Vec<&Tab> = all_tabs.iter().filter(|t| t.id != id).collect();

    let new_key = if let Some(bid) = before_id {
        // Find index of before_id
        if let Some(idx) = filtered_tabs.iter().position(|t| t.id == bid) {
            if idx == 0 {
                generate_key_between(None, Some(filtered_tabs[0].order_key.as_str()))
            } else {
                generate_key_between(
                    Some(filtered_tabs[idx - 1].order_key.as_str()),
                    Some(filtered_tabs[idx].order_key.as_str()),
                )
            }
        } else {
            return Err("before_id tab not found in window".to_string());
        }
    } else {
        generate_key_between(filtered_tabs.last().map(|t| t.order_key.as_str()), None)
    };

    tab_to_move.order_key = new_key.clone();

    if id < 0 {
        crate::incognito::reorder_incognito_tab(id, new_key);
    } else {
        // Update DB
        let (tx_upd, rx_upd) = std::sync::mpsc::channel();
        let tab_clone = tab_to_move.clone();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = tabs_repo::update_tab(conn, &tab_clone);
            let _ = tx_upd.send(res);
        });
        rx_upd.recv().unwrap_or(Ok(()))
            .map_err(|e| e.to_string())?;
    }

    let _ = app.emit("tab:updated", &tab_to_move);

    Ok(())
}

#[command]
pub async fn tabs_set_pinned(
    id: i32,
    pinned: bool,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    let updated_tab = if id < 0 {
        crate::incognito::set_incognito_pinned(id, pinned);
        crate::incognito::get_incognito_tab(id)
            .ok_or_else(|| "Tab not found".to_string())?
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = (|| -> rusqlite::Result<Option<Tab>> {
                if let Some(mut tab) = tabs_repo::get_tab(conn, id)? {
                    tab.pinned = pinned;
                    tabs_repo::update_tab(conn, &tab)?;
                    Ok(Some(tab))
                } else {
                    Ok(None)
                }
            })();
            let _ = tx.send(res);
        });

        rx.recv().unwrap_or(Ok(None))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Tab not found".to_string())?
    };

    let _ = app.emit("tab:updated", &updated_tab);

    Ok(())
}

#[command]
pub async fn tabs_set_muted(
    id: i32,
    muted: bool,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    // Apply mute to live view in pool
    {
        let pool_guard = pool.lock().unwrap();
        pool_guard.mute_tab(id, muted);
    }

    let updated_tab = if id < 0 {
        crate::incognito::set_incognito_muted(id, muted);
        crate::incognito::get_incognito_tab(id)
            .ok_or_else(|| "Tab not found".to_string())?
    } else {
        // Update DB
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = (|| -> rusqlite::Result<Option<Tab>> {
                if let Some(mut tab) = tabs_repo::get_tab(conn, id)? {
                    tab.muted = muted;
                    tabs_repo::update_tab(conn, &tab)?;
                    Ok(Some(tab))
                } else {
                    Ok(None)
                }
            })();
            let _ = tx.send(res);
        });

        rx.recv().unwrap_or(Ok(None))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Tab not found".to_string())?
    };

    let _ = app.emit("tab:updated", &updated_tab);

    Ok(())
}

#[command]
pub async fn tabs_duplicate(
    id: i32,
    db: State<'_, DbState>,
    _pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<Tab, String> {
    let source_tab = if id < 0 {
        crate::incognito::get_incognito_tab(id)
            .ok_or_else(|| "Source tab not found".to_string())?
    } else {
        let (tx, rx) = std::sync::mpsc::channel();
        let db_clone = db.clone();
        db_clone.execute(move |conn| {
            let res = tabs_repo::get_tab(conn, id);
            let _ = tx.send(res);
        });
        rx.recv().unwrap_or(Ok(None))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Source tab not found".to_string())?
    };

    let win_id = source_tab.window_id;

    // List all tabs in window to find insert position (immediately following source tab)
    let all_tabs = tabs_list_impl(win_id, &db)?;
    let idx = all_tabs.iter().position(|t| t.id == id).unwrap();

    let new_key = if idx == all_tabs.len() - 1 {
        generate_key_between(Some(source_tab.order_key.as_str()), None)
    } else {
        generate_key_between(
            Some(source_tab.order_key.as_str()),
            Some(all_tabs[idx + 1].order_key.as_str()),
        )
    };

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let new_tab = if id < 0 {
        let mut tab = crate::incognito::create_incognito_tab(win_id, source_tab.url.clone(), new_key);
        tab.title = source_tab.title.clone();
        tab.muted = source_tab.muted;
        tab.scroll_y = source_tab.scroll_y;
        crate::incognito::update_incognito_tab(tab.id, tab.url.clone(), tab.title.clone(), tab.scroll_y);
        crate::incognito::set_incognito_muted(tab.id, tab.muted);
        tab
    } else {
        let duplicate_tab = Tab {
            id: 0,
            window_id: win_id,
            url: source_tab.url.clone(),
            title: source_tab.title.clone(),
            favicon_id: source_tab.favicon_id,
            pinned: false, // duplicates are not pinned by default
            muted: source_tab.muted,
            order_key: new_key,
            scroll_y: source_tab.scroll_y,
            last_active: None,
            created_at,
        };

        let (tx_ins, rx_ins) = std::sync::mpsc::channel();
        let db_clone2 = db.clone();
        db_clone2.execute(move |conn| {
            let res = (|| -> rusqlite::Result<Tab> {
                let id = tabs_repo::insert_tab(conn, &duplicate_tab)?;
                let tab = tabs_repo::get_tab(conn, id)?.unwrap();
                Ok(tab)
            })();
            let _ = tx_ins.send(res);
        });

        rx_ins.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
            .map_err(|e| e.to_string())?
    };

    let _ = app.emit("tab:created", &new_tab);

    Ok(new_tab)
}

#[command]
pub async fn tabs_suspend_all(
    window_id: i32,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
) -> Result<(), String> {
    let mut pool_guard = pool.lock().unwrap();
    pool_guard.suspend_all(&db, window_id);
    Ok(())
}

#[command]
pub async fn tabs_reopen_closed(
    window_id: i32,
    db: State<'_, DbState>,
    pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<Option<Tab>, String> {
    if crate::incognito::is_incognito_window(window_id) {
        if let Some(mut t) = crate::incognito::pop_closed_incognito(window_id) {
            // Re-insert as a fresh incognito tab preserving url/order/pinned.
            let new_tab = crate::incognito::create_incognito_tab(window_id, t.url.clone(), t.order_key.clone());
            t.id = new_tab.id;
            crate::incognito::set_incognito_pinned(t.id, t.pinned);
            let _ = app.emit("tab:created", &t);
            tabs_activate_impl(t.id, &db, &pool, &app)?;
            return Ok(Some(t));
        }
        return Ok(None);
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let db_clone = db.clone();
    db_clone.execute(move |conn| {
        let _ = tx.send(tabs_repo::pop_closed_tab(conn, window_id));
    });
    let popped = rx.recv().unwrap_or(Ok(None)).map_err(|e| e.to_string())?;

    if let Some(mut t) = popped {
        // Re-insert as a new row (fresh id), preserving url/title/order/pinned.
        t.id = 0;
        t.last_active = None;
        let insert_tab = t.clone();
        let (tx2, rx2) = std::sync::mpsc::channel();
        let db_ins = db.clone();
        db_ins.execute(move |conn| {
            let res = (|| -> rusqlite::Result<Tab> {
                let id = tabs_repo::insert_tab(conn, &insert_tab)?;
                Ok(tabs_repo::get_tab(conn, id)?.unwrap())
            })();
            let _ = tx2.send(res);
        });
        let new_tab = rx2.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
            .map_err(|e| e.to_string())?;
        let _ = app.emit("tab:created", &new_tab);
        tabs_activate_impl(new_tab.id, &db, &pool, &app)?;
        Ok(Some(new_tab))
    } else {
        Ok(None)
    }
}
