//! Built-in ad blocker: the `adblock` engine running inside Jello with uBlock
//! Origin's default filter lists. Off by default (`builtinAdblock`).
//!
//! The engine lives in the Jello process; content webviews route their
//! requests through it (`page.rs`). Lists are cached on disk, compiled once
//! into `engine.dat` (reloaded at startup without reparsing) and refreshed in
//! the background when their `! Expires:` interval runs out.

mod filters;
pub mod page;
mod resources;

use adblock::lists::{read_list_metadata, ExpiresInterval};
use adblock::resources::Resource;
use adblock::Engine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Bumped when the adblock crate or the way lists are compiled changes, so a
/// cached `engine.dat` from an older build is rebuilt instead of loaded.
const ENGINE_FORMAT: u32 = 1;
/// Scriptlet and redirect resources change with uBlock Origin releases.
const RESOURCES_MAX_AGE: u64 = 7 * 24 * 3600;
/// Bounds for a list's own `! Expires:` header (default four days).
const LIST_MIN_AGE: u64 = 6 * 3600;
const LIST_MAX_AGE: u64 = 7 * 24 * 3600;
const LIST_DEFAULT_AGE: u64 = 4 * 24 * 3600;

static ENGINE: RwLock<Option<Arc<Engine>>> = RwLock::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);
static BLOCKED: AtomicU64 = AtomicU64::new(0);
/// Serializes refreshes (the scheduled one and "Update now").
static REFRESH: Mutex<()> = Mutex::new(());

/// The engine, once lists are loaded.
pub fn engine() -> Option<Arc<Engine>> {
    ENGINE.read().ok().and_then(|e| e.clone())
}

pub(crate) fn count_blocked() {
    BLOCKED.fetch_add(1, Ordering::Relaxed);
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct ListState {
    fetched: u64,
    max_age: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CacheState {
    #[serde(default)]
    engine_format: u32,
    #[serde(default)]
    lists: HashMap<String, ListState>,
    #[serde(default)]
    resources_fetched: u64,
    #[serde(default)]
    resources_version: String,
}

fn cache_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_default().join("content-blocker")
}

fn read_state(dir: &Path) -> CacheState {
    std::fs::read(dir.join("state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, state: &CacheState) {
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let _ = std::fs::write(dir.join("state.json"), json);
    }
}

fn list_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("lists").join(format!("{id}.txt"))
}

fn read_resources(dir: &Path) -> Vec<Resource> {
    std::fs::read(dir.join("resources.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn max_age(text: &str) -> u64 {
    let age = match read_list_metadata(text).expires {
        Some(ExpiresInterval::Hours(h)) => h as u64 * 3600,
        Some(ExpiresInterval::Days(d)) => d as u64 * 24 * 3600,
        None => LIST_DEFAULT_AGE,
    };
    age.clamp(LIST_MIN_AGE, LIST_MAX_AGE)
}

/// Compile every cached list into an engine and store it as `engine.dat`.
fn compile(dir: &Path) -> Option<Engine> {
    let texts: Vec<(&filters::ListSource, String)> = filters::LISTS
        .iter()
        .filter_map(|src| std::fs::read_to_string(list_path(dir, src.id)).ok().map(|t| (src, t)))
        .collect();
    if texts.is_empty() {
        return None;
    }
    let started = std::time::Instant::now();
    let engine = filters::build_engine(texts.iter().map(|(src, t)| (*src, t.as_str())));
    let _ = std::fs::write(dir.join("engine.dat"), engine.serialize());
    tracing::info!(
        "content blocker: compiled {} filter lists in {} ms",
        texts.len(),
        started.elapsed().as_millis()
    );
    Some(engine)
}

/// The cached engine, or a fresh compile when the cache is missing or stale.
fn load_engine(dir: &Path, state: &CacheState) -> Option<Engine> {
    if state.engine_format == ENGINE_FORMAT {
        if let Ok(bytes) = std::fs::read(dir.join("engine.dat")) {
            let mut engine = Engine::default();
            if engine.deserialize(&bytes).is_ok() {
                return Some(engine);
            }
        }
    }
    compile(dir)
}

fn publish(mut engine: Engine, dir: &Path) {
    engine.use_resources(read_resources(dir));
    *ENGINE.write().unwrap() = Some(Arc::new(engine));
}

/// A download is only kept when it reads like a filter list; a captive portal
/// or an error page is not one.
fn looks_like_filter_list(text: &str) -> bool {
    text.len() > 256 && !text.trim_start().starts_with('<')
}

async fn fetch_text(client: &reqwest::Client, urls: &[&str]) -> Result<String, String> {
    let mut last_err = String::new();
    for url in urls {
        match client.get(*url).send().await.and_then(|r| r.error_for_status()) {
            Ok(resp) => match resp.text().await {
                Ok(text) if looks_like_filter_list(&text) => return Ok(text),
                Ok(_) => last_err = format!("{url}: unexpected content"),
                Err(e) => last_err = format!("{url}: {e}"),
            },
            Err(e) => last_err = format!("{url}: {e}"),
        }
    }
    Err(last_err)
}

/// Download lists that are missing or past their expiry (all of them when
/// `force`), recompile if any changed, and refresh scriptlet resources when
/// they are a week old.
fn refresh(app: &AppHandle, force: bool) {
    let _guard = REFRESH.lock().unwrap();
    let dir = cache_dir(app);
    let _ = std::fs::create_dir_all(dir.join("lists"));
    let mut state = read_state(&dir);
    let now = now_secs();
    let client = match reqwest::Client::builder()
        .user_agent(concat!("Jello/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut changed = false;
    for src in filters::LISTS {
        let path = list_path(&dir, src.id);
        let entry = state.lists.get(src.id).cloned().unwrap_or_default();
        let due = force || !path.exists() || now >= entry.fetched + entry.max_age;
        if !due {
            continue;
        }
        match tauri::async_runtime::block_on(fetch_text(&client, src.urls)) {
            Ok(text) => {
                if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                    let _ = std::fs::write(&path, &text);
                    changed = true;
                }
                state.lists.insert(src.id.to_string(), ListState { fetched: now, max_age: max_age(&text) });
            }
            Err(e) => tracing::warn!("content blocker: {} not updated: {}", src.id, e),
        }
    }

    let resources_due = force
        || !dir.join("resources.json").exists()
        || now >= state.resources_fetched + RESOURCES_MAX_AGE;
    let mut resources_changed = false;
    if resources_due {
        match resources::assemble(app) {
            Ok((version, json)) => {
                let _ = std::fs::write(dir.join("resources.json"), json);
                state.resources_fetched = now;
                state.resources_version = version;
                resources_changed = true;
            }
            Err(e) => tracing::warn!("content blocker: scriptlet resources not updated: {}", e),
        }
    }

    if changed || state.engine_format != ENGINE_FORMAT || engine().is_none() {
        if let Some(engine) = compile(&dir) {
            state.engine_format = ENGINE_FORMAT;
            write_state(&dir, &state);
            publish(engine, &dir);
            return;
        }
    }
    write_state(&dir, &state);
    if resources_changed {
        if let Some(engine) = load_engine(&dir, &state) {
            publish(engine, &dir);
        }
    }
}

/// Load the engine and keep its lists fresh. Called at startup when the
/// built-in blocker is on, and when it is switched on; later calls are no-ops.
pub fn start(app: &AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let dir = cache_dir(&app);
        let state = read_state(&dir);
        if let Some(engine) = load_engine(&dir, &state) {
            publish(engine, &dir);
            tracing::info!("content blocker: engine loaded from cache");
        }
        loop {
            if crate::prefs::builtin_adblock() {
                refresh(&app, false);
            }
            std::thread::sleep(Duration::from_secs(3600));
        }
    });
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerStatus {
    enabled: bool,
    ready: bool,
    /// Unix seconds of the most recent list download (0 = never).
    updated: u64,
    blocked: u64,
    lists: usize,
    /// uBlock Origin's scriptlets are loaded; without them `+js()` filters
    /// (video ads, anti-adblock walls) do nothing.
    scriptlets: bool,
}

#[tauri::command]
pub fn content_blocker_status(app: AppHandle) -> BlockerStatus {
    let dir = cache_dir(&app);
    let state = read_state(&dir);
    BlockerStatus {
        enabled: crate::prefs::builtin_adblock(),
        ready: engine().is_some(),
        updated: state.lists.values().map(|l| l.fetched).max().unwrap_or(0),
        blocked: BLOCKED.load(Ordering::Relaxed),
        lists: state.lists.len(),
        scriptlets: !state.resources_version.is_empty() && dir.join("resources.json").exists(),
    }
}

/// "Update filters now": re-download every list and the scriptlet resources.
#[tauri::command]
pub async fn content_blocker_update(app: AppHandle) -> Result<(), String> {
    start(&app);
    tauri::async_runtime::spawn_blocking(move || refresh(&app, true))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_expiry_follows_the_header_within_bounds() {
        assert_eq!(max_age("! Title: x\n! Expires: 8 hours\n||a^\n"), 8 * 3600);
        assert_eq!(max_age("! Expires: 5 days\n"), 5 * 24 * 3600);
        assert_eq!(max_age("! Expires: 1 hour\n"), LIST_MIN_AGE);
        assert_eq!(max_age("! Expires: 14 days\n"), LIST_MAX_AGE);
        assert_eq!(max_age("||a^\n"), LIST_DEFAULT_AGE);
    }

    #[test]
    fn error_pages_are_not_filter_lists() {
        let list = format!("! Title: x\n{}", "||ads.example^\n".repeat(40));
        assert!(looks_like_filter_list(&list));
        assert!(!looks_like_filter_list("<!doctype html><html><body>Sign in to Wi-Fi</body></html>"));
        assert!(!looks_like_filter_list(&format!("  <html>{}", "x".repeat(400))));
        assert!(!looks_like_filter_list(""));
    }
}
