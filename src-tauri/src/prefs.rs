//! In-memory copies of the settings read on hot paths (tab activation, the 30s
//! suspend sweep). Reading them from SQLite there would put a blocking DB
//! round-trip into every tab switch, so they are loaded once at startup and
//! refreshed by `settings_set`.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::RwLock;
use std::time::Duration;

static MAX_LOADED_TABS: AtomicU32 = AtomicU32::new(5);
static SUSPEND_AFTER_MINS: AtomicU32 = AtomicU32::new(5);
static DISCARD_AFTER_MINS: AtomicU32 = AtomicU32::new(30);
static AUTO_PIP: AtomicBool = AtomicBool::new(true);
static PASSWORD_AUTOSAVE: AtomicBool = AtomicBool::new(true);
static GENERAL_AUTOFILL: AtomicBool = AtomicBool::new(true);
static CLEAR_HISTORY_ON_EXIT: AtomicBool = AtomicBool::new(false);
static AD_BLOCKER: AtomicU8 = AtomicU8::new(AdBlocker::Builtin as u8);
static AD_BLOCKING_PAUSED: RwLock<Vec<String>> = RwLock::new(Vec::new());
static FINGERPRINT_PROTECTION: AtomicBool = AtomicBool::new(true);
static FINGERPRINT_ALLOWED: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Which ad blocker filters pages (`adBlocker` setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdBlocker {
    /// The built-in engine (content_blocker); the default.
    Builtin = 0,
    /// The uBlock Origin extension.
    UblockOrigin = 1,
    Off = 2,
}

impl AdBlocker {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "builtin" => Some(Self::Builtin),
            "ublock" => Some(Self::UblockOrigin),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::UblockOrigin => "ublock",
            Self::Off => "off",
        }
    }
}

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

pub fn ad_blocker() -> AdBlocker {
    match AD_BLOCKER.load(Ordering::Relaxed) {
        1 => AdBlocker::UblockOrigin,
        2 => AdBlocker::Off,
        _ => AdBlocker::Builtin,
    }
}

/// The built-in ad blocker is the selected one. Tabs opened while it is
/// selected route their requests through it; choosing another stops it
/// filtering at once.
pub fn builtin_adblock() -> bool {
    ad_blocker() == AdBlocker::Builtin
}

/// Ad blocking is paused on this host (or a parent domain of it) through the
/// `adblockPausedSites` list.
pub fn ad_blocking_paused(host: &str) -> bool {
    let sites = AD_BLOCKING_PAUSED.read().unwrap();
    sites.iter().any(|site| host == site || host.strip_suffix(site.as_str()).map_or(false, |rest| rest.ends_with('.')))
}

/// The `adblockPausedSites` list as stored.
pub fn ad_blocking_paused_sites() -> Vec<String> {
    AD_BLOCKING_PAUSED.read().unwrap().clone()
}

/// Fingerprinting protection is on (the default).
pub fn fingerprint_protection() -> bool {
    FINGERPRINT_PROTECTION.load(Ordering::Relaxed)
}

/// Sites (registrable domains) exempted from fingerprinting protection, the
/// `fingerprintAllowedSites` list.
pub fn fingerprint_allowed_sites() -> Vec<String> {
    FINGERPRINT_ALLOWED.read().unwrap().clone()
}

fn site_list(value: &serde_json::Value) -> Option<Vec<String>> {
    let parsed = match value {
        serde_json::Value::String(s) => serde_json::from_str::<Vec<String>>(s).ok(),
        other => serde_json::from_value::<Vec<String>>(other.clone()).ok(),
    }?;
    Some(parsed.into_iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect())
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
        "adBlocker" => {
            if let Some(mode) = value.as_str().map(|s| s.trim().trim_matches('"')).and_then(AdBlocker::parse) {
                AD_BLOCKER.store(mode as u8, Ordering::Relaxed);
            }
        }
        "adblockPausedSites" => {
            if let Some(sites) = site_list(value) {
                *AD_BLOCKING_PAUSED.write().unwrap() = sites;
            }
        }
        "fingerprintAllowedSites" => {
            if let Some(sites) = site_list(value) {
                *FINGERPRINT_ALLOWED.write().unwrap() = sites;
            }
        }
        "autoPip" | "passwordAutosave" | "generalAutofill" | "clearHistoryOnExit" | "fingerprintProtection" => {
            let flag = match key {
                "autoPip" => &AUTO_PIP,
                "passwordAutosave" => &PASSWORD_AUTOSAVE,
                "generalAutofill" => &GENERAL_AUTOFILL,
                "fingerprintProtection" => &FINGERPRINT_PROTECTION,
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

        // The built-in blocker is the default; the stored value may be quoted.
        assert!(builtin_adblock());
        apply("adBlocker", &json!("ublock"));
        assert_eq!(ad_blocker(), AdBlocker::UblockOrigin);
        apply("adBlocker", &json!("\"off\""));
        assert_eq!(ad_blocker(), AdBlocker::Off);
        apply("adBlocker", &json!("unknown"));
        assert_eq!(ad_blocker(), AdBlocker::Off);
        apply("adBlocker", &json!("builtin"));
        assert!(builtin_adblock());

        apply("adblockPausedSites", &json!(["Example.com", " "]));
        assert!(ad_blocking_paused("example.com"));
        assert!(ad_blocking_paused("www.example.com"));
        assert!(!ad_blocking_paused("notexample.com"));
        assert!(!ad_blocking_paused("example.org"));
        apply("adblockPausedSites", &json!("[\"news.example\"]"));
        assert!(ad_blocking_paused("news.example"));
        assert!(!ad_blocking_paused("example.com"));
        apply("adblockPausedSites", &json!([]));

        // Fingerprinting protection is on unless switched off; exceptions are
        // stored as registrable domains.
        assert!(fingerprint_protection());
        apply("fingerprintProtection", &json!("false"));
        assert!(!fingerprint_protection());
        apply("fingerprintProtection", &json!(true));
        assert!(fingerprint_protection());
        apply("fingerprintAllowedSites", &json!("[\"Maps.Example\", \"\"]"));
        assert_eq!(fingerprint_allowed_sites(), vec!["maps.example".to_string()]);
        apply("fingerprintAllowedSites", &json!([]));

        apply("unrelatedKey", &json!("x"));
    }
}
