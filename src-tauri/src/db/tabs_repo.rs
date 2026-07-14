use rusqlite::{params, Connection, Result};
use crate::ipc_types::Tab;

pub fn insert_tab(conn: &Connection, tab: &Tab) -> Result<i32> {
    conn.execute(
        "INSERT INTO tabs (window_id, url, title, favicon_id, pinned, muted, order_key, scroll_y, last_active, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            tab.window_id,
            tab.url,
            tab.title,
            tab.favicon_id,
            if tab.pinned { 1 } else { 0 },
            if tab.muted { 1 } else { 0 },
            tab.order_key,
            tab.scroll_y,
            tab.last_active,
            tab.created_at,
        ],
    )?;
    let id = conn.last_insert_rowid() as i32;
    Ok(id)
}

pub fn get_tab(conn: &Connection, id: i32) -> Result<Option<Tab>> {
    let mut stmt = conn.prepare(
        "SELECT id, window_id, url, title, favicon_id, pinned, muted, order_key, scroll_y, last_active, created_at
         FROM tabs WHERE id = ?1",
    )?;
    let mut rows = stmt.query([id])?;
    if let Some(row) = rows.next()? {
        Ok(Some(Tab {
            id: row.get(0)?,
            window_id: row.get(1)?,
            url: row.get(2)?,
            title: row.get(3)?,
            favicon_id: row.get(4)?,
            pinned: row.get::<_, i32>(5)? != 0,
            muted: row.get::<_, i32>(6)? != 0,
            order_key: row.get(7)?,
            scroll_y: row.get(8)?,
            last_active: row.get(9)?,
            created_at: row.get(10)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn list_tabs(conn: &Connection, window_id: i32) -> Result<Vec<Tab>> {
    let mut stmt = conn.prepare(
        "SELECT id, window_id, url, title, favicon_id, pinned, muted, order_key, scroll_y, last_active, created_at
         FROM tabs WHERE window_id = ?1 ORDER BY order_key ASC",
    )?;
    let mut rows = stmt.query([window_id])?;
    let mut tabs = Vec::new();
    while let Some(row) = rows.next()? {
        tabs.push(Tab {
            id: row.get(0)?,
            window_id: row.get(1)?,
            url: row.get(2)?,
            title: row.get(3)?,
            favicon_id: row.get(4)?,
            pinned: row.get::<_, i32>(5)? != 0,
            muted: row.get::<_, i32>(6)? != 0,
            order_key: row.get(7)?,
            scroll_y: row.get(8)?,
            last_active: row.get(9)?,
            created_at: row.get(10)?,
        });
    }
    Ok(tabs)
}

pub fn update_tab(conn: &Connection, tab: &Tab) -> Result<()> {
    conn.execute(
        "UPDATE tabs SET window_id = ?1, url = ?2, title = ?3, favicon_id = ?4, pinned = ?5, muted = ?6, order_key = ?7, scroll_y = ?8, last_active = ?9
         WHERE id = ?10",
        params![
            tab.window_id,
            tab.url,
            tab.title,
            tab.favicon_id,
            if tab.pinned { 1 } else { 0 },
            if tab.muted { 1 } else { 0 },
            tab.order_key,
            tab.scroll_y,
            tab.last_active,
            tab.id,
        ],
    )?;
    Ok(())
}

pub fn delete_tab(conn: &Connection, id: i32) -> Result<()> {
    conn.execute("DELETE FROM tabs WHERE id = ?1", [id])?;
    Ok(())
}

/// Push a just-closed tab onto the per-window undo stack, trimming to 25 newest.
pub fn push_closed_tab(conn: &Connection, tab: &Tab) -> Result<()> {
    let tab_json = serde_json::to_string(tab).unwrap_or_default();
    let closed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO closed_tabs (window_id, tab_json, closed_at) VALUES (?1, ?2, ?3)",
        params![tab.window_id, tab_json, closed_at],
    )?;
    // Keep only the 25 most recent for this window.
    conn.execute(
        "DELETE FROM closed_tabs WHERE window_id = ?1 AND id NOT IN (
            SELECT id FROM closed_tabs WHERE window_id = ?1 ORDER BY id DESC LIMIT 25
         )",
        params![tab.window_id],
    )?;
    Ok(())
}

/// Pop the most-recently-closed tab for a window, returning its serialized state.
pub fn pop_closed_tab(conn: &Connection, window_id: i32) -> Result<Option<Tab>> {
    let row: Option<(i32, String)> = conn
        .query_row(
            "SELECT id, tab_json FROM closed_tabs WHERE window_id = ?1 ORDER BY id DESC LIMIT 1",
            [window_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    if let Some((row_id, tab_json)) = row {
        conn.execute("DELETE FROM closed_tabs WHERE id = ?1", [row_id])?;
        Ok(serde_json::from_str::<Tab>(&tab_json).ok())
    } else {
        Ok(None)
    }
}

pub fn list_all_tabs(conn: &Connection) -> Result<Vec<Tab>> {
    let mut stmt = conn.prepare(
        "SELECT id, window_id, url, title, favicon_id, pinned, muted, order_key, scroll_y, last_active, created_at
         FROM tabs ORDER BY last_active DESC, id ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut tabs = Vec::new();
    while let Some(row) = rows.next()? {
        tabs.push(Tab {
            id: row.get(0)?,
            window_id: row.get(1)?,
            url: row.get(2)?,
            title: row.get(3)?,
            favicon_id: row.get(4)?,
            pinned: row.get::<_, i32>(5)? != 0,
            muted: row.get::<_, i32>(6)? != 0,
            order_key: row.get(7)?,
            scroll_y: row.get(8)?,
            last_active: row.get(9)?,
            created_at: row.get(10)?,
        });
    }
    Ok(tabs)
}
