// Query/mutation commands backing the history, bookmarks, and settings views.
use tauri::{command, State};
use crate::db::DbState;
use crate::ipc_types::{HistoryEntry, Bookmark};

fn db_call<T: Send + 'static>(
    db: &DbState,
    f: impl FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<T> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let _ = tx.send(f(conn));
    });
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[command]
pub async fn history_search(query: String, limit: Option<i64>, db: State<'_, DbState>) -> Result<Vec<HistoryEntry>, String> {
    let lim = limit.unwrap_or(200);
    db_call(&db, move |conn| crate::db::history::search_history(conn, &query, lim))
}

#[command]
pub async fn history_delete(mode: String, ids: Option<Vec<i32>>, db: State<'_, DbState>) -> Result<(), String> {
    db_call(&db, move |conn| {
        match mode.as_str() {
            "all" => crate::db::history::delete_history_all(conn),
            "ids" => crate::db::history::delete_history_ids(conn, &ids.unwrap_or_default()),
            _ => Ok(()),
        }
    })
}

#[derive(serde::Serialize)]
pub struct TopSite {
    url: String,
    title: String,
}

/// Short tile label for a host: "youtube.com" → "youtube", "music.youtube.com"
/// → "music.youtube"; IP addresses and single-label hosts stay as they are.
fn site_label(host: &str) -> String {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_string();
    }
    match host.rsplit_once('.') {
        Some((rest, _tld)) if !rest.is_empty() => rest.to_string(),
        _ => host.to_string(),
    }
}

/// Most-visited sites for the new-tab page: the highest-frecency history
/// collapsed to one tile per host, each opening the site's front page (the
/// top entry for a host is often a search or a deep link).
#[command]
pub async fn top_sites(limit: Option<usize>, db: State<'_, DbState>) -> Result<Vec<TopSite>, String> {
    let limit = limit.unwrap_or(10).min(20);
    let entries = db_call(&db, |conn| crate::db::history::get_top_history_by_frecency(conn, 300))?;
    let mut seen = std::collections::HashSet::new();
    let mut sites = Vec::new();
    for entry in entries {
        let Ok(url) = tauri::Url::parse(&entry.url) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        let Some(host) = url.host_str() else { continue };
        let site = host.trim_start_matches("www.").to_string();
        if !seen.insert(site.clone()) {
            continue;
        }
        sites.push(TopSite { url: format!("{}://{}/", url.scheme(), host), title: site_label(&site) });
        if sites.len() == limit {
            break;
        }
    }
    Ok(sites)
}

#[command]
pub async fn bookmarks_list(db: State<'_, DbState>) -> Result<Vec<Bookmark>, String> {
    db_call(&db, |conn| {
        let entries = crate::db::bookmarks::list_bookmarks(conn)?;
        Ok(entries.into_iter().map(|b| Bookmark {
            id: b.id, url: b.url, title: b.title, position: b.position,
        }).collect())
    })
}

#[command]
pub async fn bookmarks_add(url: String, title: String, db: State<'_, DbState>) -> Result<i32, String> {
    db_call(&db, move |conn| crate::db::bookmarks::add_bookmark(conn, &url, &title))
}

#[command]
pub async fn bookmarks_update(id: i32, url: String, title: String, db: State<'_, DbState>) -> Result<(), String> {
    db_call(&db, move |conn| crate::db::bookmarks::update_bookmark(conn, id, &url, &title))
}

#[command]
pub async fn bookmarks_remove(id: i32, db: State<'_, DbState>) -> Result<(), String> {
    db_call(&db, move |conn| crate::db::bookmarks::remove_bookmark(conn, id))
}

/// Return all settings as a JSON object { key: value } (values are raw strings).
#[command]
pub async fn settings_get(db: State<'_, DbState>) -> Result<serde_json::Value, String> {
    db_call(&db, |conn| {
        let mut stmt = conn.prepare("SELECT key, value_json FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map = serde_json::Map::new();
        for r in rows {
            let (k, v) = r?;
            // Try to parse the stored value as JSON, else keep it as a string.
            let parsed = serde_json::from_str(&v).unwrap_or(serde_json::Value::String(v));
            map.insert(k, parsed);
        }
        Ok(serde_json::Value::Object(map))
    })
}

/// Upsert a batch of settings from a JSON object { key: value }.
#[command]
pub async fn settings_set(
    patch: serde_json::Value,
    db: State<'_, DbState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let obj = patch.as_object().cloned().ok_or_else(|| "patch must be an object".to_string())?;
    for (k, v) in &obj {
        crate::prefs::apply(k, v);
    }
    if obj.contains_key("builtinAdblock") && crate::prefs::builtin_adblock() {
        crate::content_blocker::start(&app);
    }
    if obj.contains_key("youtubeAmbientMode") {
        crate::content_blocker::page::document_start_changed();
    }
    db_call(&db, move |conn| {
        let tx = conn.transaction()?;
        for (k, v) in obj {
            let stored = match &v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            tx.execute(
                "INSERT OR REPLACE INTO settings (key, value_json) VALUES (?1, ?2)",
                rusqlite::params![k, stored],
            )?;
        }
        tx.commit()?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::site_label;

    #[test]
    fn site_labels_drop_the_tld_only() {
        assert_eq!(site_label("youtube.com"), "youtube");
        assert_eq!(site_label("music.youtube.com"), "music.youtube");
        assert_eq!(site_label("127.0.0.1"), "127.0.0.1");
        assert_eq!(site_label("localhost"), "localhost");
    }
}
