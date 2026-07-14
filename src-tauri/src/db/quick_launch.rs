use rusqlite::{params, Connection, Result};
use crate::ipc_types::QuickLaunchItem;

pub fn list_quick_launch(conn: &Connection) -> Result<Vec<QuickLaunchItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, target_url, title, sequence, disposition FROM quick_launch ORDER BY id"
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(QuickLaunchItem {
            id: row.get(0)?,
            target_url: row.get(1)?,
            title: row.get(2)?,
            sequence: row.get(3)?,
            disposition: row.get(4)?,
        })
    })?;

    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn set_quick_launch(conn: &Connection, item: &QuickLaunchItem) -> Result<()> {
    if item.id == 0 {
        conn.execute(
            "INSERT INTO quick_launch (target_url, title, sequence, disposition) VALUES (?, ?, ?, ?)",
            params![item.target_url, item.title, item.sequence, item.disposition],
        )?;
    } else {
        conn.execute(
            "INSERT OR REPLACE INTO quick_launch (id, target_url, title, sequence, disposition) VALUES (?, ?, ?, ?, ?)",
            params![item.id, item.target_url, item.title, item.sequence, item.disposition],
        )?;
    }
    Ok(())
}

pub fn remove_quick_launch(conn: &Connection, id: i32) -> Result<()> {
    conn.execute("DELETE FROM quick_launch WHERE id = ?", params![id])?;
    Ok(())
}
