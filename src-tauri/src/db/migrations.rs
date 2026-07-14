use rusqlite::Connection;

pub const MIGRATIONS: &[&str] = &[
    // Migration 1: Initial schema
    r#"
    CREATE TABLE tabs (
      id INTEGER PRIMARY KEY, window_id INTEGER NOT NULL DEFAULT 1,
      url TEXT NOT NULL, title TEXT, favicon_id INTEGER,
      pinned INTEGER DEFAULT 0, muted INTEGER DEFAULT 0,
      order_key TEXT NOT NULL, scroll_y REAL DEFAULT 0,
      last_active INTEGER, created_at INTEGER NOT NULL
    );
    CREATE TABLE favicons (
      id INTEGER PRIMARY KEY, host TEXT UNIQUE NOT NULL,
      png BLOB, fetched_at INTEGER NOT NULL
    );
    CREATE TABLE history (
      id INTEGER PRIMARY KEY, url TEXT UNIQUE NOT NULL,
      title TEXT, visit_count INTEGER DEFAULT 1,
      last_visit INTEGER NOT NULL, typed_count INTEGER DEFAULT 0
    );
    CREATE VIRTUAL TABLE history_fts USING fts5(
      title, url, content='history', content_rowid='id'
    );
    CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
      INSERT INTO history_fts(rowid, title, url) VALUES (new.id, new.title, new.url);
    END;
    CREATE TRIGGER history_ad AFTER DELETE ON history BEGIN
      INSERT INTO history_fts(history_fts, rowid, title, url) VALUES('delete', old.id, old.title, old.url);
    END;
    CREATE TRIGGER history_au AFTER UPDATE ON history BEGIN
      INSERT INTO history_fts(history_fts, rowid, title, url) VALUES('delete', old.id, old.title, old.url);
      INSERT INTO history_fts(rowid, title, url) VALUES (new.id, new.title, new.url);
    END;
    CREATE TABLE folders (
      id INTEGER PRIMARY KEY, name TEXT NOT NULL,
      parent_id INTEGER, created_at INTEGER NOT NULL
    );
    CREATE TABLE bookmarks (
      id INTEGER PRIMARY KEY, url TEXT NOT NULL,
      title TEXT, folder_id INTEGER, tags TEXT,
      position INTEGER DEFAULT 0, created_at INTEGER NOT NULL
    );
    CREATE TABLE settings (
      key TEXT PRIMARY KEY, value_json TEXT NOT NULL
    );
    CREATE TABLE search_engines (
      id INTEGER PRIMARY KEY, name TEXT NOT NULL,
      keyword TEXT UNIQUE NOT NULL, url_template TEXT NOT NULL
    );
    CREATE TABLE quick_launch (
      id INTEGER PRIMARY KEY, target_url TEXT NOT NULL,
      title TEXT, sequence TEXT NOT NULL, disposition TEXT NOT NULL
    );
    CREATE TABLE closed_tabs (
      id INTEGER PRIMARY KEY, window_id INTEGER NOT NULL,
      tab_json TEXT NOT NULL, closed_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
      id INTEGER PRIMARY KEY, closed_at INTEGER NOT NULL,
      tabs_json TEXT NOT NULL
    );
    "#,
    r#"
    CREATE TABLE extensions (
      id TEXT PRIMARY KEY, version TEXT NOT NULL,
      name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1
    );
    "#,
    r#"
    INSERT INTO search_engines (name, keyword, url_template) VALUES
      ('DuckDuckGo', 'd', 'https://duckduckgo.com/?q=%s'),
      ('Google', 'g', 'https://google.com/search?q=%s'),
      ('Bing', 'b', 'https://www.bing.com/search?q=%s'),
      ('Brave', 'br', 'https://search.brave.com/search?q=%s'),
      ('YouTube', 'yt', 'https://www.youtube.com/results?search_query=%s'),
      ('Wikipedia', 'w', 'https://en.wikipedia.org/wiki/Special:Search?search=%s'),
      ('GitHub', 'gh', 'https://github.com/search?q=%s');
    "#,
];

pub fn run_migrations(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let mut current_version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    
    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let migration_version = (i + 1) as i32;
        if migration_version > current_version {
            let tx = conn.transaction()?;
            tx.execute_batch(migration)?;
            tx.pragma_update(None, "user_version", migration_version)?;
            tx.commit()?;
            current_version = migration_version;
        }
    }
    Ok(())
}
