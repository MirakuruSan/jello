// M6.4: session save on exit + restore-last-session on launch.
use tauri::{command, AppHandle, State, Manager, Emitter};
use std::sync::{Arc, Mutex};
use crate::db::{DbState, tabs_repo};
use crate::ipc_types::Tab;
use crate::engine::pool::TabPool;

/// Persist all current (non-incognito) tabs as the latest session, keeping the
/// 5 most recent sessions. Called on app exit.
pub fn save_session(db: &DbState) {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = (|| -> rusqlite::Result<()> {
            let tabs = tabs_repo::list_all_tabs(conn)?;
            let json = serde_json::to_string(&tabs).unwrap_or_else(|_| "[]".to_string());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            conn.execute(
                "INSERT INTO sessions (closed_at, tabs_json) VALUES (?1, ?2)",
                rusqlite::params![now, json],
            )?;
            conn.execute(
                "DELETE FROM sessions WHERE id NOT IN (SELECT id FROM sessions ORDER BY id DESC LIMIT 5)",
                [],
            )?;
            Ok(())
        })();
        let _ = tx.send(res);
    });
    let _ = rx.recv();
}

/// Restore the tabs from the most recent session into the DB as fresh rows and
/// emit tab:created for each. Does not auto-activate.
#[command]
pub async fn session_restore_last(
    db: State<'_, DbState>,
    _pool: State<'_, Arc<Mutex<TabPool>>>,
    app: AppHandle,
) -> Result<Vec<Tab>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = (|| -> rusqlite::Result<Vec<Tab>> {
            let json: Option<String> = conn
                .query_row("SELECT tabs_json FROM sessions ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
                .ok();
            let Some(json) = json else { return Ok(Vec::new()); };
            let saved: Vec<Tab> = serde_json::from_str(&json).unwrap_or_default();
            let mut restored = Vec::new();
            for mut t in saved {
                if t.window_id != 1 {
                    continue; // only restore primary-window tabs in v1
                }
                t.id = 0;
                t.last_active = None;
                let id = tabs_repo::insert_tab(conn, &t)?;
                if let Some(row) = tabs_repo::get_tab(conn, id)? {
                    restored.push(row);
                }
            }
            Ok(restored)
        })();
        let _ = tx.send(res);
    });
    let restored = rx.recv().unwrap_or(Ok(Vec::new())).map_err(|e| e.to_string())?;
    for t in &restored {
        let _ = app.emit("tab:created", t);
    }
    Ok(restored)
}

/// Save the session when the app is exiting. Wired from the RunEvent handler.
pub fn on_exit(app: &AppHandle) {
    let db = app.state::<DbState>();
    save_session(&db);
}
