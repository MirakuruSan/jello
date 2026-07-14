use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::thread;

pub mod migrations;
pub mod tabs_repo;
pub mod history;
pub mod bookmarks;
pub mod quick_launch;

pub enum DbMessage {
    Execute(Box<dyn FnOnce(&mut Connection) + Send>),
    Shutdown,
}

#[derive(Clone)]
pub struct DbState {
    pub sender: Sender<DbMessage>,
}

impl DbState {
    pub fn execute<F>(&self, f: F)
    where
        F: FnOnce(&mut Connection) + Send + 'static,
    {
        let _ = self.sender.send(DbMessage::Execute(Box::new(f)));
    }
}

pub fn init_db() -> Result<Sender<DbMessage>, crate::error::JelloError> {
    let app_data = std::env::var("APPDATA")
        .map(PathBuf::from)
        .map_err(|_| crate::error::JelloError::General("APPDATA environment variable not found".to_string()))?;
    
    let jello_dir = app_data.join("Jello");
    std::fs::create_dir_all(&jello_dir)?;
    let db_path = jello_dir.join("jello.db");
    
    let (tx, rx) = channel::<DbMessage>();
    
    thread::spawn(move || {
        let mut conn = match Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to open database: {}", e);
                return;
            }
        };
        
        // Enable WAL mode
        if let Err(e) = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;") {
            tracing::error!("Failed to configure database WAL mode: {}", e);
            return;
        }
        
        // Run migrations
        if let Err(e) = migrations::run_migrations(&mut conn) {
            tracing::error!("Failed to run database migrations: {}", e);
            return;
        }
        
        tracing::info!("Database initialized successfully at {:?}", db_path);
        
        while let Ok(msg) = rx.recv() {
            match msg {
                DbMessage::Execute(f) => {
                    f(&mut conn);
                }
                DbMessage::Shutdown => {
                    break;
                }
            }
        }
        
        tracing::info!("Database thread shutting down");
    });
    
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_migrations_and_queries() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        // Test inserting and retrieving a setting
        conn.execute(
            "INSERT INTO settings (key, value_json) VALUES (?1, ?2)",
            ("theme", "\"dark\""),
        )
        .unwrap();

        let val: String = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                ["theme"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(val, "\"dark\"");

        // Test history search
        conn.execute(
            "INSERT INTO history (url, title, last_visit) VALUES (?1, ?2, ?3)",
            ("https://example.com", "Example Domain", 1600000000i64),
        )
        .unwrap();

        let history_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(history_count, 1);
    }

    #[test]
    fn test_tabs_repo() {
        use crate::ipc_types::Tab;
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        let tab = Tab {
            id: 0,
            window_id: 1,
            url: "https://example.com".to_string(),
            title: Some("Example".to_string()),
            favicon_id: None,
            pinned: false,
            muted: false,
            order_key: "a0".to_string(),
            scroll_y: 0.0,
            last_active: None,
            created_at: 1234567890,
        };

        let id = tabs_repo::insert_tab(&conn, &tab).unwrap();
        assert_eq!(id, 1);

        let mut retrieved = tabs_repo::get_tab(&conn, id).unwrap().unwrap();
        assert_eq!(retrieved.url, "https://example.com");
        assert_eq!(retrieved.title, Some("Example".to_string()));
        assert!(!retrieved.pinned);

        retrieved.pinned = true;
        tabs_repo::update_tab(&conn, &retrieved).unwrap();

        let updated = tabs_repo::get_tab(&conn, id).unwrap().unwrap();
        assert!(updated.pinned);

        let list = tabs_repo::list_tabs(&conn, 1).unwrap();
        assert_eq!(list.len(), 1);

        tabs_repo::delete_tab(&conn, id).unwrap();
        assert!(tabs_repo::get_tab(&conn, id).unwrap().is_none());
    }

    #[test]
    fn test_history_record_visit() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        history::record_visit(&conn, "https://a.test/", Some("A"), false).unwrap();
        history::record_visit(&conn, "https://a.test/", Some("A2"), true).unwrap();

        let (vc, tc, title): (i32, i32, String) = conn
            .query_row(
                "SELECT visit_count, typed_count, title FROM history WHERE url = 'https://a.test/'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(vc, 2);
        assert_eq!(tc, 1);
        assert_eq!(title, "A2");

        // FTS finds it (url tokenizes to https / a / test).
        let hits: i32 = conn
            .query_row("SELECT COUNT(*) FROM history_fts WHERE history_fts MATCH 'test'", [], |r| r.get(0))
            .unwrap();
        assert!(hits >= 1);
    }

    #[test]
    fn test_closed_tab_undo_roundtrip() {
        use crate::ipc_types::Tab;
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        let tab = Tab {
            id: 42,
            window_id: 1,
            url: "https://reopen.test/path".to_string(),
            title: Some("Reopen".to_string()),
            favicon_id: None,
            pinned: true,
            muted: false,
            order_key: "a5".to_string(),
            scroll_y: 12.0,
            last_active: Some(111),
            created_at: 222,
        };

        // No closed tabs yet.
        assert!(tabs_repo::pop_closed_tab(&conn, 1).unwrap().is_none());

        tabs_repo::push_closed_tab(&conn, &tab).unwrap();
        let restored = tabs_repo::pop_closed_tab(&conn, 1).unwrap().unwrap();
        assert_eq!(restored.url, "https://reopen.test/path");
        assert_eq!(restored.order_key, "a5");
        assert!(restored.pinned);

        // Stack is now empty again (popped).
        assert!(tabs_repo::pop_closed_tab(&conn, 1).unwrap().is_none());

        // Cap at 25 per window.
        for i in 0..30 {
            let mut t = tab.clone();
            t.url = format!("https://n/{}", i);
            tabs_repo::push_closed_tab(&conn, &t).unwrap();
        }
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM closed_tabs WHERE window_id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 25);
    }

    #[test]
    fn test_500_tabs_performance() {
        use crate::ipc_types::Tab;
        use std::time::Instant;

        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        let start = Instant::now();

        let tx = conn.transaction().unwrap();
        for i in 0..500 {
            let tab = Tab {
                id: 0,
                window_id: 1,
                url: format!("https://example.com/{}", i),
                title: Some(format!("Tab {}", i)),
                favicon_id: None,
                pinned: false,
                muted: false,
                order_key: format!("a{:03}", i),
                scroll_y: 0.0,
                last_active: None,
                created_at: 1234567890 + i as i64,
            };
            tabs_repo::insert_tab(&tx, &tab).unwrap();
        }
        tx.commit().unwrap();

        let duration = start.elapsed();
        println!("Inserted 500 fake tabs in: {:?}", duration);
        assert!(duration.as_millis() < 1000, "Should insert 500 tabs in less than 1 second, took {:?}", duration);

        let list = tabs_repo::list_tabs(&conn, 1).unwrap();
        assert_eq!(list.len(), 500);
    }

    #[test]
    fn test_incognito_zero_db_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&mut conn).unwrap();

        let db_tabs_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM tabs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(db_tabs_count, 0);

        crate::incognito::clear_all_incognito_tabs();
        let incognito_win_id = 999;
        
        let tab1 = crate::incognito::create_incognito_tab(incognito_win_id, "https://example.com/1".to_string(), "a".to_string());
        let tab2 = crate::incognito::create_incognito_tab(incognito_win_id, "https://example.com/2".to_string(), "b".to_string());
        
        assert_eq!(crate::incognito::list_incognito_tabs(incognito_win_id).len(), 2);
        assert!(tab1.id < 0);
        assert!(tab2.id < 0);

        crate::incognito::update_incognito_tab(tab1.id, "https://example.com/updated".to_string(), Some("Updated".to_string()), 100.0);
        crate::incognito::set_incognito_pinned(tab1.id, true);
        crate::incognito::set_incognito_muted(tab2.id, true);

        let t1_updated = crate::incognito::get_incognito_tab(tab1.id).unwrap();
        assert_eq!(t1_updated.url, "https://example.com/updated");
        assert!(t1_updated.pinned);

        let t2_updated = crate::incognito::get_incognito_tab(tab2.id).unwrap();
        assert!(t2_updated.muted);

        let db_tabs_count_after: i32 = conn
            .query_row("SELECT COUNT(*) FROM tabs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(db_tabs_count_after, 0);

        crate::incognito::delete_incognito_tab(tab1.id);
        assert_eq!(crate::incognito::list_incognito_tabs(incognito_win_id).len(), 1);

        crate::incognito::clear_all_incognito_tabs();
        assert_eq!(crate::incognito::list_incognito_tabs(incognito_win_id).len(), 0);
    }
}
