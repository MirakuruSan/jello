use rusqlite::{params, Connection, Result};

pub struct HistoryDbEntry {
    pub id: i32,
    pub url: String,
    pub title: String,
    pub visit_count: i32,
    pub last_visit: i64,
    pub typed_count: i32,
}

pub fn get_top_history_by_frecency(conn: &Connection, limit: i64) -> Result<Vec<HistoryDbEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, COALESCE(title, '') as title, visit_count, last_visit, typed_count,
           (visit_count * CASE
             WHEN (unixepoch() - last_visit) < 4 * 24 * 3600 THEN 100
             WHEN (unixepoch() - last_visit) < 14 * 24 * 3600 THEN 70
             WHEN (unixepoch() - last_visit) < 31 * 24 * 3600 THEN 50
             WHEN (unixepoch() - last_visit) < 90 * 24 * 3600 THEN 30
             ELSE 10
           END) as frecency
         FROM history
         ORDER BY frecency DESC
         LIMIT ?1"
    )?;

    let rows = stmt.query_map(params![limit], |row| {
        Ok(HistoryDbEntry {
            id: row.get(0)?,
            url: row.get(1)?,
            title: row.get(2)?,
            visit_count: row.get(3)?,
            last_visit: row.get(4)?,
            typed_count: row.get(5)?,
        })
    })?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

/// Record a page visit (upsert). When `typed` is true (navigation came from the
/// palette / address bar), also bumps typed_count.
pub fn record_visit(conn: &Connection, url: &str, title: Option<&str>, typed: bool) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let typed_inc = if typed { 1 } else { 0 };
    conn.execute(
        "INSERT INTO history (url, title, visit_count, last_visit, typed_count)
         VALUES (?1, ?2, 1, ?3, ?4)
         ON CONFLICT(url) DO UPDATE SET
           visit_count = visit_count + 1,
           last_visit = ?3,
           typed_count = typed_count + ?4,
           title = COALESCE(?2, title)",
        params![url, title, now, typed_inc],
    )?;
    Ok(())
}

/// Search history by FTS (title/url) ordered by recency. Empty query returns
/// the most recent entries.
pub fn search_history(conn: &Connection, query: &str, limit: i64) -> Result<Vec<crate::ipc_types::HistoryEntry>> {
    let q = query.trim();
    let mut out = Vec::new();
    if q.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, url, COALESCE(title,''), visit_count, last_visit
             FROM history ORDER BY last_visit DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok(crate::ipc_types::HistoryEntry {
                id: r.get(0)?, url: r.get(1)?, title: r.get(2)?,
                visit_count: r.get(3)?, last_visit: r.get(4)?,
            })
        })?;
        for r in rows { out.push(r?); }
        return Ok(out);
    }
    // FTS match: append * to the last token for prefix matching.
    let fts_query = format!("{}*", q.replace('"', " "));
    let mut stmt = conn.prepare(
        "SELECT h.id, h.url, COALESCE(h.title,''), h.visit_count, h.last_visit
         FROM history_fts f JOIN history h ON h.id = f.rowid
         WHERE history_fts MATCH ?1
         ORDER BY h.last_visit DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![fts_query, limit], |r| {
        Ok(crate::ipc_types::HistoryEntry {
            id: r.get(0)?, url: r.get(1)?, title: r.get(2)?,
            visit_count: r.get(3)?, last_visit: r.get(4)?,
        })
    })?;
    for r in rows { out.push(r?); }
    Ok(out)
}

pub fn delete_history_all(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM history", [])?;
    Ok(())
}

pub fn delete_history_ids(conn: &Connection, ids: &[i32]) -> Result<()> {
    for id in ids {
        conn.execute("DELETE FROM history WHERE id = ?1", [id])?;
    }
    Ok(())
}

pub fn add_history_entry(conn: &Connection, url: &str, title: Option<&str>) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    conn.execute(
        "INSERT INTO history (url, title, visit_count, last_visit, typed_count)
         VALUES (?1, ?2, 1, ?3, 0)
         ON CONFLICT(url) DO UPDATE SET
           visit_count = visit_count + 1,
           last_visit = ?3,
           title = COALESCE(?2, title)",
        params![url, title, now],
    )?;

    Ok(())
}
