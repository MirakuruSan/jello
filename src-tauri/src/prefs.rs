//! In-memory copies of the settings read on hot paths (tab activation, the 30s
//! suspend sweep). Reading them from SQLite there would put a blocking DB
//! round-trip into every tab switch, so they are loaded once at startup and
//! refreshed by `settings_set`.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

static MAX_LOADED_TABS: AtomicU32 = AtomicU32::new(5);
static SUSPEND_AFTER_MINS: AtomicU32 = AtomicU32::new(5);
static DISCARD_AFTER_MINS: AtomicU32 = AtomicU32::new(30);
static AUTO_PIP: AtomicBool = AtomicBool::new(true);
static PASSWORD_AUTOSAVE: AtomicBool = AtomicBool::new(true);
static GENERAL_AUTOFILL: AtomicBool = AtomicBool::new(true);
static CLEAR_HISTORY_ON_EXIT: AtomicBool = AtomicBool::new(false);
static BUILTIN_ADBLOCK: AtomicBool = AtomicBool::new(false);

/// Webviews kept alive at once; the least-recently-used one is unloaded when a
/// new tab would exceed it. 0 in the setting means unlimited.
pub fn max_loaded_tabs() -> usize {
    match MAX_LOADED_TABS.load(Ordering::Relaxed) {
        0 => usize::MAX,
        n => n as usize,
    }
}

/// Idle time after which a background tab is frozen (TrySuspend). None = never.
pub fn suspend_after() -> Option<Duration> {
    minutes(SUSPEND_AFTER_MINS.load(Ordering::Relaxed))
}

/// Idle time after which a background tab's webview is closed outright (memory
/// saver) — the only thing that returns a renderer's memory to the OS.
pub fn discard_after() -> Option<Duration> {
    minutes(DISCARD_AFTER_MINS.load(Ordering::Relaxed))
}

/// Move a playing video into picture-in-picture when its tab is left or the
/// window is minimized/hidden.
pub fn auto_pip() -> bool {
    AUTO_PIP.load(Ordering::Relaxed)
}

/// WebView2's built-in password saving, applied to each tab as it loads.
pub fn password_autosave() -> bool {
    PASSWORD_AUTOSAVE.load(Ordering::Relaxed)
}

/// WebView2's address/form autofill, applied to each tab as it loads.
pub fn general_autofill() -> bool {
    GENERAL_AUTOFILL.load(Ordering::Relaxed)
}

pub fn clear_history_on_exit() -> bool {
    CLEAR_HISTORY_ON_EXIT.load(Ordering::Relaxed)
}

/// Jello's built-in ad blocker (content_blocker). Tabs opened while it is on
/// route their requests through it; switching it off stops blocking at once.
pub fn builtin_adblock() -> bool {
    BUILTIN_ADBLOCK.load(Ordering::Relaxed)
}

fn minutes(m: u32) -> Option<Duration> {
    (m > 0).then(|| Duration::from_secs(m as u64 * 60))
}

fn as_u32(v: &serde_json::Value) -> Option<u32> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().map(|n| n.min(u32::MAX as u64) as u32),
        serde_json::Value::String(s) => s.trim().trim_matches('"').parse().ok(),
        _ => None,
    }
}

fn as_bool(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim().trim_matches('"') {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Apply one setting to the cache. Keys this module doesn't own are ignored.
pub fn apply(key: &str, value: &serde_json::Value) {
    match key {
        "maxLoadedTabs" => {
            if let Some(n) = as_u32(value) {
                MAX_LOADED_TABS.store(if n == 0 { 0 } else { n.max(1) }, Ordering::Relaxed);
            }
        }
        "suspendAfterMins" => {
            if let Some(n) = as_u32(value) {
                SUSPEND_AFTER_MINS.store(n, Ordering::Relaxed);
            }
        }
        "discardAfterMins" => {
            if let Some(n) = as_u32(value) {
                DISCARD_AFTER_MINS.store(n, Ordering::Relaxed);
            }
        }
        "autoPip" | "passwordAutosave" | "generalAutofill" | "clearHistoryOnExit" | "builtinAdblock" => {
            let flag = match key {
                "autoPip" => &AUTO_PIP,
                "passwordAutosave" => &PASSWORD_AUTOSAVE,
                "generalAutofill" => &GENERAL_AUTOFILL,
                "builtinAdblock" => &BUILTIN_ADBLOCK,
                _ => &CLEAR_HISTORY_ON_EXIT,
            };
            if let Some(b) = as_bool(value) {
                flag.store(b, Ordering::Relaxed);
            }
        }
        _ => {}
    }
}

/// Load every cached setting from the DB (startup).
pub fn load(db: &crate::db::DbState) {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT key, value_json FROM settings")
            .and_then(|mut stmt| {
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        let _ = tx.send(rows);
    });
    for (k, raw) in rx.recv().unwrap_or_default() {
        let v = serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
        apply(&k, &v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_numbers_bools_and_stored_strings() {
        apply("maxLoadedTabs", &json!(8));
        assert_eq!(max_loaded_tabs(), 8);
        apply("maxLoadedTabs", &json!("0"));
        assert_eq!(max_loaded_tabs(), usize::MAX);
        apply("maxLoadedTabs", &json!(5));

        apply("suspendAfterMins", &json!("0"));
        assert_eq!(suspend_after(), None);
        apply("suspendAfterMins", &json!(2));
        assert_eq!(suspend_after(), Some(Duration::from_secs(120)));
        apply("suspendAfterMins", &json!(5));

        apply("autoPip", &json!("false"));
        assert!(!auto_pip());
        apply("autoPip", &json!(true));
        assert!(auto_pip());

        apply("clearHistoryOnExit", &json!("true"));
        assert!(clear_history_on_exit());
        apply("clearHistoryOnExit", &json!(false));
        assert!(!clear_history_on_exit());

        assert!(!builtin_adblock());
        apply("builtinAdblock", &json!("true"));
        assert!(builtin_adblock());
        apply("builtinAdblock", &json!(false));
        assert!(!builtin_adblock());

        apply("unrelatedKey", &json!("x"));
    }
}
