//! uBlock Origin's scriptlets and redirect resources for the engine.
//!
//! They are published as ES modules, so a hidden window loads them and
//! serializes them (`resources.js`). The source is the jsDelivr mirror of the
//! latest uBlock Origin release, or, when that fails, the release's own zip
//! from GitHub served to the window from disk. They are downloaded rather
//! than shipped with Jello because uBlock Origin is GPL-licensed. This runs
//! when the cached copy is a week old; a failed run keeps the last good copy.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;

const ASSEMBLE_JS: &str = include_str!("resources.js");
const WINDOW_LABEL: &str = "content_blocker_resources";
/// Virtual host that serves an extracted release to the hidden window.
const LOCAL_HOST: &str = "ublock-origin.example";

struct Release {
    tag: String,
    zip_url: Option<String>,
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("Jello/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

/// The latest uBlock Origin release: its tag (e.g. "1.75.0") and the URL of
/// its Chromium zip.
fn latest_release() -> Result<Release, String> {
    tauri::async_runtime::block_on(async {
        let body = http_client()?
            .get("https://api.github.com/repos/gorhill/uBlock/releases/latest")
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let release: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let tag = release["tag_name"]
            .as_str()
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .map(str::to_string)
            .ok_or_else(|| "release has no tag".to_string())?;
        let zip_url = release["assets"].as_array().and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a["name"].as_str()?;
                name.ends_with(".chromium.zip").then(|| a["browser_download_url"].as_str().map(str::to_string))?
            })
        });
        Ok(Release { tag, zip_url })
    })
}

/// Returns (uBlock Origin version, resources JSON).
pub fn assemble(app: &AppHandle) -> Result<(String, String), String> {
    let release = latest_release()?;
    let cdn = format!("https://cdn.jsdelivr.net/gh/gorhill/uBlock@{}/src/", release.tag);
    let json = match run_in_window(app, &cdn, None) {
        Ok(json) => json,
        Err(cdn_err) => {
            tracing::warn!("content blocker: jsDelivr resources failed ({cdn_err}); using the release zip");
            let zip_url = release.zip_url.as_deref().ok_or_else(|| format!("{cdn_err}; release has no zip"))?;
            let folder = super::cache_dir(app).join("ublock-origin-source");
            extract_release(zip_url, &folder)?;
            run_in_window(app, &format!("https://{LOCAL_HOST}/"), Some(&folder))?
        }
    };
    let resources: Vec<adblock::resources::Resource> =
        serde_json::from_str(&json).map_err(|e| format!("bad resources: {e}"))?;
    if resources.len() < 50 {
        return Err(format!("only {} resources", resources.len()));
    }
    tracing::info!("content blocker: {} resources from uBlock Origin {}", resources.len(), release.tag);
    Ok((release.tag, json))
}

/// Download the release zip and keep the files resources.js reads: `js/` and
/// `web_accessible_resources/`, without the zip's top-level folder.
fn extract_release(zip_url: &str, folder: &Path) -> Result<(), String> {
    let bytes = tauri::async_runtime::block_on(async {
        http_client()?
            .get(zip_url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .bytes()
            .await
            .map_err(|e| e.to_string())
    })?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(folder);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(path) = entry.enclosed_name() else { continue };
        // Drop the top-level folder (uBlock0.chromium/).
        let relative: std::path::PathBuf = path.components().skip(1).collect();
        let keep = relative.starts_with("js") || relative.starts_with("web_accessible_resources");
        if !keep || entry.is_dir() {
            continue;
        }
        let target = folder.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Assemble the resources at `base` in a hidden window. With `folder`, the
/// window first maps it to `LOCAL_HOST` and opens a page there, so the
/// modules load from the same origin.
fn run_in_window(app: &AppHandle, base: &str, folder: Option<&Path>) -> Result<String, String> {
    let expression = format!("{}({})", ASSEMBLE_JS, serde_json::to_string(base).unwrap_or_default());
    let (tx, rx) = mpsc::channel();
    let app_m = app.clone();
    app.run_on_main_thread(move || {
        let url = "about:blank".parse().expect("valid URL");
        // browser_extensions_enabled must match every other webview: they
        // share the WebView2 environment, which rejects mismatched options.
        let res = tauri::WebviewWindowBuilder::new(&app_m, WINDOW_LABEL, tauri::WebviewUrl::External(url))
            .visible(false)
            .skip_taskbar(true)
            .browser_extensions_enabled(true)
            .build();
        let _ = tx.send(res.map_err(|e| e.to_string()));
    })
    .map_err(|e| e.to_string())?;
    let window = rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| "window creation timed out".to_string())??;
    let result = match folder {
        Some(folder) => open_local(&window, folder).and_then(|_| evaluate(&window, &expression)),
        None => evaluate(&window, &expression),
    };
    let _ = window.close();
    result
}

/// Map `folder` to `LOCAL_HOST` and navigate there, returning once the
/// navigation completed.
fn open_local(window: &tauri::WebviewWindow, folder: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let folder = folder.to_string_lossy().to_string();
        let (tx, rx) = mpsc::channel();
        window
            .with_webview(move |w| {
                use webview2_com::Microsoft::Web::WebView2::Win32::*;
                use webview2_com::NavigationCompletedEventHandler;
                use windows::core::{Interface, HSTRING};
                unsafe {
                    let Ok(core) = w.controller().CoreWebView2() else {
                        let _ = tx.send(false);
                        return;
                    };
                    let Ok(core3) = core.cast::<ICoreWebView2_3>() else {
                        let _ = tx.send(false);
                        return;
                    };
                    let mapped = core3.SetVirtualHostNameToFolderMapping(
                        &HSTRING::from(LOCAL_HOST),
                        &HSTRING::from(folder),
                        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
                    );
                    if mapped.is_err() {
                        let _ = tx.send(false);
                        return;
                    }
                    let done = tx.clone();
                    let handler = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                        let mut ok = windows::core::BOOL::default();
                        if let Some(args) = args {
                            let _ = args.IsSuccess(&mut ok);
                        }
                        let _ = done.send(ok.as_bool());
                        Ok(())
                    }));
                    let mut token: i64 = 0;
                    let _ = core.add_NavigationCompleted(&handler, &mut token);
                    let page = format!("https://{LOCAL_HOST}/js/redirect-resources.js");
                    if core.Navigate(&HSTRING::from(page)).is_err() {
                        let _ = tx.send(false);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(true) => Ok(()),
            Ok(false) => Err("could not open the extracted release".to_string()),
            Err(_) => Err("timed out opening the extracted release".to_string()),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, folder);
        Err("unsupported platform".to_string())
    }
}

/// Run an async expression in the window and return its string result.
fn evaluate(window: &tauri::WebviewWindow, expression: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let params = serde_json::json!({
            "expression": expression,
            "awaitPromise": true,
            "returnByValue": true,
        })
        .to_string();
        let (tx, rx) = mpsc::channel();
        window
            .with_webview(move |w| {
                use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
                use windows::core::HSTRING;
                unsafe {
                    let Ok(core) = w.controller().CoreWebView2() else {
                        let _ = tx.send(Err("no webview".to_string()));
                        return;
                    };
                    let done = tx.clone();
                    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |_hr, json: String| {
                        let reply: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                        let out = match reply.pointer("/result/value").and_then(|v| v.as_str()) {
                            Some(v) => Ok(v.to_string()),
                            None => Err(reply
                                .pointer("/exceptionDetails/exception/description")
                                .and_then(|v| v.as_str())
                                .unwrap_or("evaluation failed")
                                .to_string()),
                        };
                        let _ = done.send(out);
                        Ok(())
                    }));
                    if core
                        .CallDevToolsProtocolMethod(&HSTRING::from("Runtime.evaluate"), &HSTRING::from(params.as_str()), &handler)
                        .is_err()
                    {
                        let _ = tx.send(Err("DevTools call failed".to_string()));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(90))
            .map_err(|_| "timed out loading uBlock Origin resources".to_string())?
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, expression);
        Err("unsupported platform".to_string())
    }
}
