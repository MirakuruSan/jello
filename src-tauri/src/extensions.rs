use tauri::{command, AppHandle, State, Manager};
use std::fs;
use std::path::{Path, PathBuf};
use std::io::Cursor;
use std::sync::Mutex;
use crate::db::DbState;
use crate::ipc_types::Extension;

// ── Extension load caches (P1.1) ────────────────────────────────────────────
// The old mechanism mutated the staging dir with remove_dir_all + copy at
// RUNTIME while Chromium held those very files open (§2 finding #8) → corrupt
// staging → "Extension isn't loaded". The rebuild now runs ONLY at startup, and
// extensions are loaded into each content webview's shared profile via explicit
// per-extension AddBrowserExtension COM calls that yield the REAL runtime id.

/// Enabled extensions, cached so the content-webview creation path (main thread)
/// can load them WITHOUT a DB round trip. Refreshed at startup and on
/// install/enable/disable/uninstall.
static ENABLED_EXTS: Mutex<Vec<Extension>> = Mutex::new(Vec::new());
/// store_ids already AddBrowserExtension'd into the shared profile this session
/// (the profile is shared across all non-incognito webviews, so one load covers
/// the session). Optimistically inserted at issue time; removed on failure.
static LOADED_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// store_id → real runtime chrome-extension:// id, captured from
/// AddBrowserExtension's completed handler. Replaces path-hash guessing.
static RUNTIME_IDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Replace the enabled-extensions cache (called whenever the enabled set changes).
pub fn set_enabled_cache(exts: Vec<Extension>) {
    *ENABLED_EXTS.lock().unwrap() = exts;
}

fn refresh_enabled_cache(db: &DbState) {
    if let Ok(all) = db_list_extensions(db) {
        set_enabled_cache(all.into_iter().filter(|e| e.enabled).collect());
    }
}

/// Look up the real runtime id for a store id (empty until the extension has been
/// loaded into a live profile at least once).
pub fn runtime_id_for(store_id: &str) -> Option<String> {
    RUNTIME_IDS
        .lock()
        .unwrap()
        .iter()
        .find(|(s, _)| s == store_id)
        .map(|(_, r)| r.clone())
}

/// Load every enabled extension into this webview's (shared) profile via explicit
/// per-extension `AddBrowserExtension`. Failures are per-extension and never
/// affect the others. Reads only the cache + filesystem — no DB. Skips ids
/// already loaded this session.
pub fn load_all_enabled(app: &AppHandle, webview: &tauri::Webview<tauri::Wry>) {
    #[cfg(target_os = "windows")]
    {
        let active = active_extensions_dir(app);
        let exts = ENABLED_EXTS.lock().unwrap().clone();
        for ext in exts {
            {
                let mut loaded = LOADED_IDS.lock().unwrap();
                if loaded.iter().any(|s| s == &ext.id) {
                    continue;
                }
                loaded.push(ext.id.clone()); // optimistic — removed on failure below
            }
            let dir = active.join(&ext.id);
            if !dir.join("manifest.json").exists() {
                LOADED_IDS.lock().unwrap().retain(|s| s != &ext.id);
                continue;
            }
            add_browser_extension(webview, ext.id.clone(), dir);
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, webview);
}

/// Issue a single `AddBrowserExtension` on the webview's profile and record the
/// real runtime id when it completes. Runs the COM work on the webview UI thread
/// via with_webview (marshals correctly).
#[cfg(target_os = "windows")]
fn add_browser_extension(webview: &tauri::Webview<tauri::Wry>, store_id: String, dir: PathBuf) {
    let path_str = dir.to_string_lossy().to_string();
    let _ = webview.with_webview(move |w| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2BrowserExtension, ICoreWebView2Profile7, ICoreWebView2_13,
        };
        use webview2_com::ProfileAddBrowserExtensionCompletedHandler;
        use windows::core::{Interface, HSTRING, PCWSTR, PWSTR};

        let core = match w.controller().CoreWebView2() {
            Ok(c) => c,
            Err(_) => {
                LOADED_IDS.lock().unwrap().retain(|s| s != &store_id);
                return;
            }
        };
        let profile = match core.cast::<ICoreWebView2_13>().and_then(|c| c.Profile()) {
            Ok(p) => p,
            Err(_) => {
                LOADED_IDS.lock().unwrap().retain(|s| s != &store_id);
                return;
            }
        };
        let profile7 = match profile.cast::<ICoreWebView2Profile7>() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("extensions: ICoreWebView2Profile7 unavailable: {:?}", e);
                LOADED_IDS.lock().unwrap().retain(|s| s != &store_id);
                return;
            }
        };

        let store_cb = store_id.clone();
        let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(
            move |result: windows::core::Result<()>, ext: Option<ICoreWebView2BrowserExtension>| {
                if result.is_ok() {
                    if let Some(ext) = ext {
                        let mut idp = PWSTR::null();
                        if ext.Id(&mut idp).is_ok() && !idp.is_null() {
                            let rid = idp.to_string().unwrap_or_default();
                            if !rid.is_empty() {
                                let mut map = RUNTIME_IDS.lock().unwrap();
                                match map.iter_mut().find(|(s, _)| s == &store_cb) {
                                    Some(e) => e.1 = rid,
                                    None => map.push((store_cb.clone(), rid)),
                                }
                            }
                        }
                    }
                    tracing::info!("extensions: AddBrowserExtension ok for {}", store_cb);
                } else {
                    // The common failure is the shared profile already having this
                    // extension registered (0x80004005) — the extension still
                    // works and options resolves via the path-hash fallback. Keep
                    // it marked loaded (don't retry on every new tab → no error
                    // spam) and log at warn, not error.
                    tracing::warn!("extensions: AddBrowserExtension for {} returned {:?} (likely already loaded)", store_cb, result.err());
                }
                Ok(())
            },
        ));

        let folder = HSTRING::from(path_str.as_str());
        if let Err(e) = profile7.AddBrowserExtension(PCWSTR(folder.as_ptr()), &handler) {
            tracing::error!("extensions: AddBrowserExtension call failed for {}: {:?}", store_id, e);
            LOADED_IDS.lock().unwrap().retain(|s| s != &store_id);
        }
    });
}

/// Copy ONE extension's versioned source into the active staging dir without
/// wiping the whole root (the runtime-safe replacement for the old destructive
/// rebuild). Used by install/enable at runtime.
fn stage_one_additive(app: &AppHandle, ext: &Extension) -> bool {
    let versioned = get_extensions_dir(app).join(&ext.id).join(&ext.version);
    if !versioned.join("manifest.json").exists() {
        return false;
    }
    let target = active_extensions_dir(app).join(&ext.id);
    let _ = fs::remove_dir_all(&target); // best-effort; ignore if locked
    copy_dir_recursive(&versioned, &target).is_ok()
}

fn get_extensions_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("")).join("extensions")
}

// Database query helpers
fn db_list_extensions(db: &DbState) -> Result<Vec<Extension>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = (|| -> rusqlite::Result<Vec<Extension>> {
            let mut stmt = conn.prepare("SELECT id, version, name, enabled FROM extensions")?;
            let mut rows = stmt.query([])?;
            let mut list = Vec::new();
            while let Some(row) = rows.next()? {
                list.push(Extension {
                    id: row.get(0)?,
                    version: row.get(1)?,
                    name: row.get(2)?,
                    enabled: row.get::<_, i32>(3)? != 0,
                });
            }
            Ok(list)
        })();
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Err(rusqlite::Error::QueryReturnedNoRows))
        .map_err(|e| e.to_string())
}

fn db_insert_or_update_extension(db: &DbState, ext: &Extension) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ext_clone = ext.clone();
    db.execute(move |conn| {
        let res = conn.execute(
            "INSERT INTO extensions (id, version, name, enabled)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET version=?2, name=?3, enabled=?4",
            rusqlite::params![
                ext_clone.id,
                ext_clone.version,
                ext_clone.name,
                if ext_clone.enabled { 1 } else { 0 }
            ],
        );
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(0)).map(|_| ()).map_err(|e| e.to_string())
}

fn db_set_extension_enabled(db: &DbState, id: String, enabled: bool) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = conn.execute(
            "UPDATE extensions SET enabled = ?1 WHERE id = ?2",
            rusqlite::params![if enabled { 1 } else { 0 }, id],
        );
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(0)).map(|_| ()).map_err(|e| e.to_string())
}

fn strip_crx3_header(crx_bytes: &[u8]) -> Result<Vec<u8>, String> {
    if crx_bytes.len() < 12 {
        return Err("Invalid CRX file: too short".to_string());
    }
    if &crx_bytes[0..4] != b"Cr24" {
        return Err("Invalid CRX file: missing magic Cr24 header".to_string());
    }
    let version = u32::from_le_bytes(crx_bytes[4..8].try_into().unwrap());
    if version != 3 {
        return Err(format!("Unsupported CRX version: {}", version));
    }
    let header_len = u32::from_le_bytes(crx_bytes[8..12].try_into().unwrap()) as usize;
    if crx_bytes.len() < 12 + header_len {
        return Err("Invalid CRX file: truncated header".to_string());
    }
    Ok(crx_bytes[12 + header_len..].to_vec())
}

#[command]
pub fn extensions_list(db: State<'_, DbState>) -> Result<Vec<Extension>, String> {
    db_list_extensions(&db)
}

/// A valid Chrome extension id is exactly 32 chars in a..p.
fn is_ext_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| ('a'..='p').contains(&c))
}

fn resolve_ext_id(crx_id_or_url: &str) -> Result<String, String> {
    let trimmed = crx_id_or_url.trim();
    if is_ext_id(trimmed) {
        return Ok(trimmed.to_string());
    }
    // Web Store URLs are `.../detail/<slug>/<id>` (modern) or `.../detail/<id>`
    // (legacy). The id is the 32-char [a-p] path segment — NOT necessarily the
    // first one after /detail/, which is usually the human-readable slug. Scan
    // all path segments (strip any query/fragment) and take the id-shaped one.
    let path = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    if let Some(id) = path.split('/').map(|s| s.to_ascii_lowercase()).find(|s| is_ext_id(s)) {
        return Ok(id);
    }
    Err("Could not find a 32-character extension ID in that value or URL.".to_string())
}

fn crx_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))
}

async fn fetch_crx_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = crx_http_client()?;
    let resp = client.get(url).send().await
        .map_err(|e| format!("Download request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await
        .map_err(|e| format!("Download interrupted: {e}"))?;
    if bytes.len() < 16 {
        return Err("empty or truncated".to_string());
    }
    Ok(bytes.to_vec())
}

/// Google Chrome Web Store CRX endpoint. NOTE: an old/rounded prodversion (e.g.
/// 120.0.0.0) makes it return "204 No Content" — the real cause of past download
/// failures. A realistic recent version plus installsource=ondemand yields it.
async fn download_google(ext_id: &str) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&acceptformat=crx2,crx3&prodversion=131.0.6778.86&x=id%3D{}%26installsource%3Dondemand%26uc",
        ext_id
    );
    fetch_crx_bytes(&url).await
}

/// Microsoft Edge Add-ons CRX endpoint (P1.4). Response is CRX3 — same strip.
async fn download_edge(ext_id: &str) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://edge.microsoft.com/extensionwebstorebase/v1/crx?response=redirect&x=id%3D{}%26installsource%3Dondemand%26uc",
        ext_id
    );
    fetch_crx_bytes(&url).await
}

/// Source-aware CRX download (P1.4): Google first for Chrome Web Store ids; Edge
/// first for Edge Add-ons URLs. Falls back to the other store so an id that only
/// exists in one store still resolves. Async — never on the IPC handler thread.
async fn download_crx(ext_id: &str, prefer_edge: bool) -> Result<Vec<u8>, String> {
    if prefer_edge {
        match download_edge(ext_id).await {
            Ok(b) => Ok(b),
            Err(e_edge) => download_google(ext_id).await
                .map_err(|e_g| format!("Download failed (edge: {e_edge}; google: {e_g})")),
        }
    } else {
        match download_google(ext_id).await {
            Ok(b) => Ok(b),
            Err(e_g) => download_edge(ext_id).await
                .map_err(|e_edge| format!("Download failed (google: {e_g}; edge: {e_edge})")),
        }
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    name: String,
    version: String,
    permissions: Option<Vec<String>>,
    default_locale: Option<String>,
}

/// Chrome extensions localize their name as `__MSG_key__`; the real string lives
/// in `_locales/<default_locale>/messages.json` under `key.message`. Resolve it
/// so the UI shows "uBlock Origin Lite" instead of the raw id (Phase 4.2.4).
fn resolve_localized_name(dir: &Path, raw_name: &str, default_locale: &Option<String>) -> Option<String> {
    let key = raw_name.strip_prefix("__MSG_")?.strip_suffix("__")?;
    let locale = default_locale.as_deref()?;
    let messages_path = dir.join("_locales").join(locale).join("messages.json");
    let text = fs::read_to_string(messages_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    // messages.json keys are case-insensitive in Chrome; try exact then ci.
    let msg = json.get(key)
        .or_else(|| json.as_object().and_then(|o| o.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)));
    msg?.get("message")?.as_str().map(|s| s.to_string())
}

/// Unzip the stripped CRX (a plain zip) directly into `dest`, in-process — no
/// PowerShell. Returns the parsed manifest.
fn extract_and_read_manifest(zip_bytes: &[u8], dest: &Path) -> Result<Manifest, String> {
    let _ = fs::remove_dir_all(dest);
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| format!("Invalid extension archive: {e}"))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let rel = match file.enclosed_name() {
            Some(p) => p,
            None => continue, // skip path-traversal entries
        };
        let outpath = dest.join(rel);
        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = outpath.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut out = fs::File::create(&outpath).map_err(|e| e.to_string())?;
            std::io::copy(&mut file, &mut out).map_err(|e| e.to_string())?;
        }
    }
    // GitHub release zips (e.g. uBlock Origin's uBlock0_x.chromium.zip) nest
    // everything under a single root dir. Flatten it so manifest.json is at the
    // top (P1.3.3).
    flatten_single_root(dest);
    let manifest_path = dest.join("manifest.json");
    if !manifest_path.exists() {
        return Err("manifest.json not found in extension".to_string());
    }
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
    serde_json::from_str(&manifest_str).map_err(|e| format!("Bad manifest: {e}"))
}

/// If `dest` has no manifest.json but contains exactly one subdirectory that
/// does, move that subdirectory's contents up to `dest` (handles single-root-dir
/// release zips).
fn flatten_single_root(dest: &Path) {
    if dest.join("manifest.json").exists() {
        return;
    }
    let entries: Vec<_> = match fs::read_dir(dest) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };
    if entries.len() != 1 || !entries[0].path().is_dir() {
        return;
    }
    let sub = entries[0].path();
    if !sub.join("manifest.json").exists() {
        return;
    }
    if let Ok(rd) = fs::read_dir(&sub) {
        for e in rd.filter_map(|e| e.ok()) {
            let _ = fs::rename(e.path(), dest.join(e.file_name()));
        }
    }
    let _ = fs::remove_dir_all(&sub);
}

/// Core install. `prompt` = show a consent dialog (false for the wizard's
/// auto-install, where the checkbox already gave consent). Async per threading
/// rule #1 — this creates dirs, hits the network, and may show a dialog.
async fn install_ext(
    ext_id: String,
    prompt: bool,
    prefer_edge: bool,
    db: &DbState,
    app: &AppHandle,
) -> Result<Extension, String> {
    let crx_bytes = download_crx(&ext_id, prefer_edge).await?;
    let zip_bytes = strip_crx3_header(&crx_bytes)?;
    install_from_zip_bytes(zip_bytes, ext_id, prompt, db, app).await
}

/// Shared install tail (P1.3): extract → consent → move into place → DB. Used by
/// both store installs (download_crx→strip) and file/drag installs.
async fn install_from_zip_bytes(
    zip_bytes: Vec<u8>,
    ext_id: String,
    prompt: bool,
    db: &DbState,
    app: &AppHandle,
) -> Result<Extension, String> {
    let extraction_dir = std::env::temp_dir().join(format!("jello_ext_extracted_{}", ext_id));
    let manifest = extract_and_read_manifest(&zip_bytes, &extraction_dir)?;

    let name_display = if manifest.name.starts_with("__MSG_") {
        resolve_localized_name(&extraction_dir, &manifest.name, &manifest.default_locale)
            .unwrap_or_else(|| ext_id.clone())
    } else {
        manifest.name.clone()
    };

    if prompt {
        use tauri_plugin_dialog::DialogExt;
        let perms = manifest.permissions.clone().unwrap_or_default();
        let perms_msg = if perms.is_empty() {
            "None".to_string()
        } else {
            format!("\n- {}", perms.join("\n- "))
        };
        let dialog_message = format!(
            "Do you want to install '{}' ({})?\n\nIt requires the following permissions:{}",
            name_display, manifest.version, perms_msg
        );
        let confirmed = app.dialog()
            .message(&dialog_message)
            .title("Extension Installation Request")
            .kind(tauri_plugin_dialog::MessageDialogKind::Info)
            .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancel)
            .blocking_show();
        if !confirmed {
            let _ = fs::remove_dir_all(&extraction_dir);
            return Err("Installation cancelled by user".to_string());
        }
    }

    // Move to final location: %APPDATA%\Jello\extensions\<id>\<version>\
    let final_dir = get_extensions_dir(app).join(&ext_id).join(&manifest.version);
    let _ = fs::remove_dir_all(&final_dir);
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // fs::rename fails across drives; fall back to recursive copy.
    if fs::rename(&extraction_dir, &final_dir).is_err() {
        copy_dir_recursive(&extraction_dir, &final_dir)?;
        let _ = fs::remove_dir_all(&extraction_dir);
    }

    let extension = Extension {
        id: ext_id,
        version: manifest.version,
        name: name_display,
        enabled: true,
    };
    db_insert_or_update_extension(db, &extension)?;
    Ok(extension)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[command]
pub async fn extensions_install(
    crx_id_or_url: String,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<Extension, String> {
    let ext_id = resolve_ext_id(&crx_id_or_url)?;
    // Edge Add-ons URLs → hit the Edge CRX endpoint first (P1.4).
    let prefer_edge = crx_id_or_url.contains("microsoftedge.microsoft.com");
    let ext = install_ext(ext_id, true, prefer_edge, &db, &app).await?;
    // Additive staging + cache refresh — NEVER the destructive runtime rebuild
    // (§2 finding #8). The extension loads into newly opened tabs immediately.
    stage_one_additive(&app, &ext);
    refresh_enabled_cache(&db);
    Ok(ext)
}

/// Slug a 32-char [a-p] stable id from arbitrary bytes (for file installs that
/// have no store id) so the DB key + staging dir name are valid and stable.
fn slug_id_from(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(seed.as_bytes());
    hash[..16]
        .iter()
        .flat_map(|b| [(b'a' + (b >> 4)) as char, (b'a' + (b & 0x0f)) as char])
        .collect()
}

/// Install an extension from a local .crx or .zip file (P1.3). Reads the bytes,
/// strips a CRX3 header if present (else treats it as a plain zip), then runs the
/// shared consent+install tail.
#[command]
pub async fn extensions_install_file(
    path: String,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<Extension, String> {
    let bytes = fs::read(&path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    let zip_bytes = if bytes.len() >= 4 && &bytes[0..4] == b"Cr24" {
        strip_crx3_header(&bytes)?
    } else {
        bytes
    };
    // Peek the manifest name to derive a stable id (no store id for file installs).
    let peek_dir = std::env::temp_dir().join("jello_ext_peek");
    let manifest = extract_and_read_manifest(&zip_bytes, &peek_dir)?;
    let id = slug_id_from(&format!("{}::{}", manifest.name, manifest.version));
    let _ = fs::remove_dir_all(&peek_dir);

    let ext = install_from_zip_bytes(zip_bytes, id, true, &db, &app).await?;
    stage_one_additive(&app, &ext);
    refresh_enabled_cache(&db);
    Ok(ext)
}

/// Open a native file picker and install the chosen .crx/.zip (P1.3.2).
#[command]
pub async fn extensions_install_file_dialog(
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<Option<Extension>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .add_filter("Extension", &["crx", "zip"])
        .pick_file(move |f| {
            let _ = tx.send(f);
        });
    let picked = rx.recv().map_err(|e| e.to_string())?;
    let Some(fp) = picked else { return Ok(None) };
    let path = fp.to_string();
    let ext = extensions_install_file(path, db, app).await?;
    Ok(Some(ext))
}

#[command]
pub async fn extensions_set_enabled(
    id: String,
    enabled: bool,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    db_set_extension_enabled(&db, id.clone(), enabled)?;
    if enabled {
        // Stage additively so newly opened tabs pick it up without the
        // destructive rebuild. (Disable takes full effect on restart — files are
        // cleaned by the startup rebuild; we never remove_dir_all a live dir.)
        if let Ok(all) = db_list_extensions(&db) {
            if let Some(ext) = all.iter().find(|e| e.id == id) {
                stage_one_additive(&app, ext);
            }
        }
    }
    refresh_enabled_cache(&db);
    Ok(())
}

/// One-time startup migration: with the full uBlock Origin enabled, ALSO running
/// uBO Lite and/or AdGuard makes every network request pay 2-3 filter passes —
/// a major "pages load slowly" cause. Disable (not uninstall — reversible in
/// Settings) the redundant ones. Returns how many were disabled.
pub fn dedupe_ad_blockers(db: &DbState) -> usize {
    let Ok(exts) = db_list_extensions(db) else { return 0 };
    let full_ubo_on = exts.iter().any(|e| e.enabled && e.name == "uBlock Origin");
    if !full_ubo_on {
        return 0;
    }
    const REDUNDANT: [&str; 2] = [
        "ddkjiahejlhfcafbddmgiahcphecmpfh", // uBlock Origin Lite
        "bgnkhhnnamicmpeenaelnjfhikgbkllg", // AdGuard AdBlocker
    ];
    let mut disabled = 0;
    for ext in exts.iter().filter(|e| e.enabled && REDUNDANT.contains(&e.id.as_str())) {
        if db_set_extension_enabled(db, ext.id.clone(), false).is_ok() {
            tracing::info!("disabled redundant ad blocker '{}' (full uBO active)", ext.name);
            disabled += 1;
        }
    }
    disabled
}

/// Install the full uBlock Origin (MV2) from its latest GitHub release (#9) —
/// the wizard/settings now offer this instead of the weaker uBO Lite.
#[command]
pub async fn extensions_install_ubo(
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<Extension, String> {
    let client = crx_http_client()?;
    let rel = client
        .get("https://api.github.com/repos/gorhill/uBlock/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("uBO release lookup failed: {e}"))?;
    let body = rel.text().await.map_err(|e| format!("release read failed: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| format!("bad release json: {e}"))?;
    let asset_url = json["assets"]
        .as_array()
        .and_then(|a| {
            a.iter().find_map(|x| {
                let name = x["name"].as_str()?;
                if name.ends_with(".chromium.zip") {
                    x["browser_download_url"].as_str()
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| "no uBO chromium.zip asset found".to_string())?
        .to_string();
    let bytes = client
        .get(&asset_url)
        .send()
        .await
        .map_err(|e| format!("uBO download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("uBO download interrupted: {e}"))?
        .to_vec();
    // Stable id so re-running the wizard updates in place.
    let id = slug_id_from("uBlock Origin (GitHub)");
    let ext = install_from_zip_bytes(bytes, id, false, &db, &app).await?;
    stage_one_additive(&app, &ext);
    refresh_enabled_cache(&db);
    Ok(ext)
}

#[command]
pub async fn extensions_uninstall(
    id: String,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    // Remove the versioned SOURCE (Chromium loads the copy under
    // extensions_active, not this, so removing source is safe) and the DB row.
    // We do NOT remove the live active/<id> dir at runtime (locked files → the
    // §2 corruption); the startup rebuild drops it once the DB row is gone.
    let _ = fs::remove_dir_all(get_extensions_dir(&app).join(&id));
    let (tx, rx) = std::sync::mpsc::channel();
    let id_clone = id.clone();
    db.execute(move |conn| {
        let res = conn.execute("DELETE FROM extensions WHERE id = ?1", rusqlite::params![id_clone]);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(0)).map(|_| ()).map_err(|e| e.to_string())?;
    refresh_enabled_cache(&db);
    Ok(())
}

/// Real restart (P1.1.4): the close button only hides to tray and a relaunch
/// hits single-instance, so the process never actually restarts and users could
/// be stuck on stale staging. This performs a true restart.
#[command]
pub fn extensions_restart_app(app: AppHandle) {
    crate::sessions::on_exit(&app);
    app.restart();
}

/// Chromium derives an unpacked extension's runtime id deterministically from
/// the absolute path it was loaded from: SHA-256 of the path encoded as UTF-16LE,
/// first 16 bytes, each nibble mapped to a..p. WebView2 loads our extensions from
/// `extensions_active/<store_id>`, so this yields the chrome-extension:// id
/// without any COM enumeration. (Verified against a live uBOL load.)
fn chromium_unpacked_id(abs_path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut bytes = Vec::new();
    for u in abs_path.to_string_lossy().encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let hash = Sha256::digest(&bytes);
    hash[..16]
        .iter()
        .flat_map(|b| [(b'a' + (b >> 4)) as char, (b'a' + (b & 0x0f)) as char])
        .collect()
}

/// Parse an extension's options page (MV2 `options_page` or MV3
/// `options_ui.page`) from its manifest, if any.
fn read_options_page(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("manifest.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if let Some(s) = v.get("options_page").and_then(|x| x.as_str()) {
        return Some(s.to_string());
    }
    v.get("options_ui")
        .and_then(|o| o.get("page"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// Open an installed extension's options/settings page in a dedicated window.
/// Uses the REAL runtime chrome-extension:// id captured from
/// `AddBrowserExtension` (P1.2); falls back to the deterministic path hash only
/// when the extension hasn't been loaded into a live profile yet.
#[command]
pub async fn extensions_open_options(
    id: String,
    db: State<'_, DbState>,
    pool: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    let staging = active_extensions_dir(&app).join(&id);
    if !staging.join("manifest.json").exists() {
        // Staging can be stale/missing (disable, a mid-session rebuild, etc.).
        // Re-stage from the installed source on demand instead of failing with
        // "Extension isn't loaded" (#11) — that error was firing far too often.
        if let Ok(exts) = db_list_extensions(&db) {
            if let Some(ext) = exts.iter().find(|e| e.id == id) {
                stage_one_additive(&app, ext);
            }
        }
    }
    if !staging.join("manifest.json").exists() {
        return Err("Extension files are missing — try reinstalling it.".to_string());
    }
    let options = read_options_page(&staging)
        .ok_or_else(|| "This extension has no options page.".to_string())?;
    // Prefer the real runtime id; fall back to the path-hash guess if the
    // extension hasn't been loaded into a profile yet this session.
    let runtime_id = runtime_id_for(&id).unwrap_or_else(|| chromium_unpacked_id(&staging));
    let url = format!("chrome-extension://{}/{}", runtime_id, options.trim_start_matches('/'));
    let _ = (&db, &pool);

    // Open in a dedicated top-level window (the user's suggestion). A separate
    // window may host the extension page where a content tab can't.
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let parsed: tauri::Url = url.parse().map_err(|e| format!("bad extension url: {e}"))?;
    let label = format!("ext_{}", &runtime_id[..8]);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return Ok(());
    }
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed.clone()))
        .title("Extension settings")
        .inner_size(920.0, 720.0)
        .browser_extensions_enabled(true)
        .build()
        .map_err(|e| e.to_string())?;

    // Load every enabled extension into this window's (shared) profile so the
    // page resolves even if no content tab has loaded it yet — per-extension COM
    // load, no destructive extensions_path staging.
    load_all_enabled(&app, win.as_ref());

    // The window's INITIAL document load can render a non-web-accessible
    // extension page (e.g. uBOL's dashboard). A later programmatic navigate()
    // to that same page, however, is treated as a web-initiated navigation and
    // BLOCKED (ERR_BLOCKED_BY_CLIENT) — which is what broke the page when the
    // extension was already loaded and the first load had succeeded. So only
    // re-navigate as a RECOVERY when the initial load did NOT land on the
    // extension page (fresh session where the extension wasn't loaded yet).
    let target_prefix = format!("chrome-extension://{}/", runtime_id);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1400));
        let on_target = win
            .url()
            .map(|u| u.as_str().starts_with(&target_prefix))
            .unwrap_or(false);
        if !on_target {
            let _ = win.navigate(parsed);
        }
    });
    Ok(())
}

/// Flat staging directory that wry's `extensions_path` expects: its immediate
/// children must each be an unpacked extension (manifest.json inside). We store
/// installs as `extensions/<id>/<version>/`, so this holds one entry per ENABLED
/// extension pointing at the versioned dir.
pub fn active_extensions_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("")).join("extensions_active")
}

/// Rebuild the staging dir from the enabled set by COPYING each enabled
/// extension's versioned folder into `extensions_active/<id>`. Real directories
/// are used deliberately: WebView2's `AddBrowserExtension` rejects reparse points
/// (junctions/symlinks), so a junction here loads nothing. Returns how many
/// extensions were staged. wry reads this path at content-webview creation, so
/// changes apply to tabs opened afterwards (hence the "restart to apply" UX).
pub fn rebuild_active_extensions(app: &AppHandle, db: &DbState) -> usize {
    let base = get_extensions_dir(app);
    let active = active_extensions_dir(app);
    let _ = fs::remove_dir_all(&active);
    if fs::create_dir_all(&active).is_err() {
        return 0;
    }
    let exts = match db_list_extensions(db) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut staged = 0;
    for ext in exts.iter().filter(|e| e.enabled) {
        let versioned = base.join(&ext.id).join(&ext.version);
        if !versioned.join("manifest.json").exists() {
            continue;
        }
        if copy_dir_recursive(&versioned, &active.join(&ext.id)).is_ok() {
            staged += 1;
        }
    }
    // Seed the enabled cache so the content-webview load path needs no DB.
    set_enabled_cache(exts.into_iter().filter(|e| e.enabled).collect());
    staged
}
