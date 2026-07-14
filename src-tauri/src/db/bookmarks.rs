use rusqlite::{Connection, Result};

pub struct BookmarkDbEntry {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub folder_id: Option<i32>,
    pub tags: Option<String>,
    pub position: i32,
    pub created_at: i64,
}

pub fn add_bookmark(conn: &Connection, url: &str, title: &str) -> Result<i32> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // position = append to end
    let next_pos: i32 = conn
        .query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM bookmarks", [], |r| r.get(0))
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO bookmarks (url, title, folder_id, tags, position, created_at)
         VALUES (?1, ?2, NULL, NULL, ?3, ?4)",
        rusqlite::params![url, title, next_pos, created_at],
    )?;
    Ok(conn.last_insert_rowid() as i32)
}

pub fn remove_bookmark(conn: &Connection, id: i32) -> Result<()> {
    conn.execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
    Ok(())
}

pub fn update_bookmark(conn: &Connection, id: i32, url: &str, title: &str) -> Result<()> {
    conn.execute(
        "UPDATE bookmarks SET url = ?1, title = ?2 WHERE id = ?3",
        rusqlite::params![url, title, id],
    )?;
    Ok(())
}

pub fn list_bookmarks(conn: &Connection) -> Result<Vec<BookmarkDbEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, COALESCE(title, '') as title, folder_id, tags, position, created_at
         FROM bookmarks
         ORDER BY position ASC, id ASC"
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(BookmarkDbEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            folder_id: row.get(3)?,
            tags: row.get(4)?,
            position: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}
