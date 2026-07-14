use tauri::{command, AppHandle, State, Manager};
use std::fs;
use std::path::{Path, PathBuf};
use std::io::Cursor;
use crate::db::DbState;
use crate::ipc_types::Extension;

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

fn resolve_ext_id(crx_id_or_url: &str) -> Result<String, String> {
    if crx_id_or_url.len() == 32 && crx_id_or_url.chars().all(|c| c.is_ascii_lowercase()) {
        Ok(crx_id_or_url.to_string())
    } else if let Some(pos) = crx_id_or_url.find("/detail/") {
        let suffix = &crx_id_or_url[pos + 8..];
        let id_part = suffix.split('/').next().unwrap_or("");
        if id_part.len() == 32 {
            Ok(id_part.to_string())
        } else {
            Err("Could not extract extension ID from URL".to_string())
        }
    } else {
        Err("Invalid extension ID or Web Store URL".to_string())
    }
}

/// Download the CRX bytes over HTTPS with a Chrome-like User-Agent so Google's
/// update service serves the file (it 403s requests without one). Async: runs on
/// the tokio pool, never the IPC handler thread.
async fn download_crx(ext_id: &str) -> Result<Vec<u8>, String> {
    // NOTE: an old/rounded prodversion (e.g. 120.0.0.0) makes this endpoint
    // return "204 No Content" — the real cause of past download failures. A
    // realistic recent version plus installsource=ondemand yields the CRX.
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&acceptformat=crx2,crx3&prodversion=131.0.6778.86&x=id%3D{}%26installsource%3Dondemand%26uc",
        ext_id
    );
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("HTTP client init failed: {e}"))?;
    let resp = client.get(&url).send().await
        .map_err(|e| format!("Download request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await
        .map_err(|e| format!("Download interrupted: {e}"))?;
    if bytes.len() < 16 {
        return Err("Downloaded CRX is empty or truncated".to_string());
    }
    Ok(bytes.to_vec())
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
    let manifest_path = dest.join("manifest.json");
    if !manifest_path.exists() {
        return Err("manifest.json not found in extension".to_string());
    }
    let manifest_str = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
    serde_json::from_str(&manifest_str).map_err(|e| format!("Bad manifest: {e}"))
}

/// Core install. `prompt` = show a consent dialog (false for the wizard's
/// auto-install, where the checkbox already gave consent). Async per threading
/// rule #1 — this creates dirs, hits the network, and may show a dialog.
async fn install_ext(
    ext_id: String,
    prompt: bool,
    db: &DbState,
    app: &AppHandle,
) -> Result<Extension, String> {
    let crx_bytes = download_crx(&ext_id).await?;
    let zip_bytes = strip_crx3_header(&crx_bytes)?;

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
    let ext = install_ext(ext_id, true, &db, &app).await?;
    rebuild_active_extensions(&app, &db);
    Ok(ext)
}

#[command]
pub async fn extensions_set_enabled(
    id: String,
    enabled: bool,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    db_set_extension_enabled(&db, id, enabled)?;
    rebuild_active_extensions(&app, &db);
    Ok(())
}

#[command]
pub async fn extensions_install_ubol(
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<Extension, String> {
    // uBlock Origin Lite Web Store ID: ddkjiahejlhfcafbddmgiahcphecmpfh.
    // No prompt: the first-run wizard checkbox is the user's consent.
    let ext = install_ext("ddkjiahejlhfcafbddmgiahcphecmpfh".to_string(), false, &db, &app).await?;
    rebuild_active_extensions(&app, &db);
    Ok(ext)
}

#[command]
pub async fn extensions_uninstall(
    id: String,
    db: State<'_, DbState>,
    app: AppHandle,
) -> Result<(), String> {
    // Remove files then the DB row, then rebuild the active staging dir.
    let _ = fs::remove_dir_all(get_extensions_dir(&app).join(&id));
    let (tx, rx) = std::sync::mpsc::channel();
    let id_clone = id.clone();
    db.execute(move |conn| {
        let res = conn.execute("DELETE FROM extensions WHERE id = ?1", rusqlite::params![id_clone]);
        let _ = tx.send(res);
    });
    rx.recv().unwrap_or(Ok(0)).map(|_| ()).map_err(|e| e.to_string())?;
    rebuild_active_extensions(&app, &db);
    Ok(())
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

/// Open an installed extension's options/settings page in a new tab. Computes
/// the runtime chrome-extension:// id from the staging path (see
/// `chromium_unpacked_id`) so the page loads from the loaded extension.
#[command]
pub async fn extensions_open_options(
    id: String,
    db: State<'_, DbState>,
    pool: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::engine::pool::TabPool>>>,
    app: AppHandle,
) -> Result<(), String> {
    let staging = active_extensions_dir(&app).join(&id);
    if !staging.join("manifest.json").exists() {
        return Err("Extension isn't loaded — enable it and restart Jello first.".to_string());
    }
    let options = read_options_page(&staging)
        .ok_or_else(|| "This extension has no options page.".to_string())?;
    let runtime_id = chromium_unpacked_id(&staging);
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
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(parsed))
        .title("Extension settings")
        .inner_size(920.0, 720.0)
        .browser_extensions_enabled(true)
        // Load extensions here too, so the page resolves even if no content tab
        // has loaded this extension into the profile yet.
        .extensions_path(active_extensions_dir(&app))
        .build()
        .map_err(|e| e.to_string())?;
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
    staged
}
