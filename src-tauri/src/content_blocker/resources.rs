//! uBlock Origin's scriptlets and redirect resources for the engine.
//!
//! They are published as ES modules, so a hidden window loads them from the
//! jsDelivr mirror of the latest uBlock Origin release and serializes them
//! (`resources.js`). This runs when the cached copy is a week old.

use std::sync::mpsc;
use std::time::Duration;
use tauri::AppHandle;

const ASSEMBLE_JS: &str = include_str!("resources.js");
const WINDOW_LABEL: &str = "content_blocker_resources";

/// The latest uBlock Origin release tag, e.g. "1.75.0".
fn latest_release() -> Result<String, String> {
    tauri::async_runtime::block_on(async {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Jello/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;
        let body = client
            .get("https://api.github.com/repos/gorhill/uBlock/releases/latest")
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let release: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        release["tag_name"]
            .as_str()
            .filter(|t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'))
            .map(str::to_string)
            .ok_or_else(|| "release has no tag".to_string())
    })
}

/// Returns (uBlock Origin version, resources JSON).
pub fn assemble(app: &AppHandle) -> Result<(String, String), String> {
    let version = latest_release()?;
    let base = format!("https://cdn.jsdelivr.net/gh/gorhill/uBlock@{version}/src/");
    let expression = format!("{}({})", ASSEMBLE_JS, serde_json::to_string(&base).unwrap_or_default());

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

    let result = evaluate(&window, &expression);
    let _ = window.close();
    let json = result?;
    let resources: Vec<adblock::resources::Resource> =
        serde_json::from_str(&json).map_err(|e| format!("bad resources: {e}"))?;
    if resources.len() < 50 {
        return Err(format!("only {} resources", resources.len()));
    }
    tracing::info!("content blocker: {} resources from uBlock Origin {}", resources.len(), version);
    Ok((version, json))
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
