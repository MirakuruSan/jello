//! Fingerprinting protection for content tabs: on by default
//! (`fingerprintProtection`), with per-site exceptions for pages it breaks
//! (`fingerprintAllowedSites`, registrable domains).
//!
//! `farble.js` runs before page scripts in every document, as part of the
//! document-start script every content tab registers before its first
//! navigation (content_blocker/page.rs), and in every worker, which the
//! DevTools protocol holds at start until the script has run there (`attach`).
//! Workers and frames are also told to report the Edge browser Jello is built
//! on: client hints (`Sec-CH-UA`, `navigator.userAgentData`) without the
//! WebView2 brand, and one language in `Accept-Language`.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};

const FARBLE_JS: &str = include_str!("farble.js");
/// https://publicsuffix.org/list/public_suffix_list.dat (MPL-2.0).
const SUFFIX_LIST: &str = include_str!("public_suffix_list.dat");

/// Random for each run of Jello: farbled values change between sessions.
fn secret() -> &'static str {
    static SECRET: OnceLock<String> = OnceLock::new();
    SECRET.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut out = String::new();
        for i in 0..4u64 {
            // RandomState keys come from the operating system's generator.
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            out.push_str(&format!("{:016x}", h.finish()));
        }
        out
    })
}

/// Public suffix rules as farble.js reads them: one ASCII rule per line,
/// with a line break at both ends.
fn suffixes() -> &'static str {
    static RULES: OnceLock<String> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut out = String::from("\n");
        for line in SUFFIX_LIST.lines() {
            let Some(rule) = line.split_whitespace().next() else { continue };
            if rule.starts_with("//") {
                continue;
            }
            let (prefix, name) = match rule.strip_prefix("*.") {
                Some(rest) => ("*.", rest),
                None => match rule.strip_prefix('!') {
                    Some(rest) => ("!", rest),
                    None => ("", rule),
                },
            };
            if let Some(ascii) = ascii_host(name) {
                out.push_str(prefix);
                out.push_str(&ascii);
                out.push('\n');
            }
        }
        out
    })
}

/// A hostname as it appears in URLs (punycode, lowercase).
fn ascii_host(name: &str) -> Option<String> {
    let url = tauri::Url::parse(&format!("http://{name}/")).ok()?;
    url.host_str().map(str::to_string)
}

/// Registrable domain of `host` (the public suffix plus one label), the same
/// rule farble.js applies; per-site exceptions are stored in this form.
pub fn registrable(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.contains(':') || host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return host;
    }
    let rules = suffixes();
    let has = |rule: &str| rules.contains(&format!("\n{rule}\n"));
    let labels: Vec<&str> = host.split('.').collect();
    let mut suffix = 1;
    for i in 0..labels.len() {
        let name = labels[i..].join(".");
        if has(&format!("!{name}")) {
            suffix = labels.len() - i - 1;
            break;
        }
        if has(&name) || (i + 1 < labels.len() && has(&format!("*.{}", labels[i + 1..].join(".")))) {
            suffix = labels.len() - i;
            break;
        }
    }
    if suffix >= labels.len() {
        host
    } else {
        labels[labels.len() - suffix - 1..].join(".")
    }
}

fn config(top: Option<&str>) -> String {
    let mut cfg = serde_json::json!({
        "secret": secret(),
        "suffixes": suffixes(),
        "allow": crate::prefs::fingerprint_allowed_sites(),
    });
    match top {
        Some(top) => cfg["top"] = serde_json::Value::String(top.to_string()),
        None => cfg["marker"] = serde_json::Value::String(shared_worker_marker().to_string()),
    }
    cfg.to_string()
}

/// Query flag farble.js adds to shared worker URLs (random per session; the
/// worker can read its own URL, so it is independent of the secret).
pub fn shared_worker_marker() -> &'static str {
    static MARKER: OnceLock<String> = OnceLock::new();
    MARKER.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(u64::MAX);
        format!("_{:016x}", h.finish())
    })
}

/// The original URL of a marked shared worker script request, or None.
pub fn shared_worker_original(url: &str) -> Option<String> {
    let marker = shared_worker_marker();
    url.strip_suffix(&format!("?{marker}"))
        .or_else(|| url.strip_suffix(&format!("&{marker}")))
        .map(str::to_string)
}

/// The protection's part of the document-start script, or None when it is off.
pub fn document_script() -> Option<String> {
    crate::prefs::fingerprint_protection().then(|| format!("{FARBLE_JS}({});", config(None)))
}

/// The script run in a worker started under the top-level `origin`.
fn worker_script(origin: &str) -> Option<String> {
    crate::prefs::fingerprint_protection().then(|| format!("{FARBLE_JS}({});", config(Some(origin))))
}

/// Shared workers belong to no page, and WebView2 does not let Jello hold
/// them at start through the DevTools protocol. Their main script request is
/// answered with this instead: the protection, then the original script
/// loaded from the same URL (as a classic script, or as a module for module
/// workers, whose connections are held until the module has loaded).
pub fn shared_worker_script(url: &str) -> Option<String> {
    let protection = worker_script(&origin_of(url))?;
    let url = serde_json::to_string(url).ok()?;
    // Top-level `this` is the global object in a classic script and
    // undefined in a module.
    Some(format!(
        r##"{protection}
if (this !== undefined) {{
  importScripts({url});
}} else {{
  const held = [];
  const hold = (e) => held.push(e);
  self.addEventListener("connect", hold);
  import({url} + "#module").then(() => {{
    self.removeEventListener("connect", hold);
    for (const e of held) self.dispatchEvent(new MessageEvent("connect", {{ ports: e.ports, data: e.data }}));
  }});
}}"##
    ))
}

/// The browser's real user-agent client hints, learned from Jello's own
/// window (a secure context) at startup; the WebView2 brand is removed when
/// they are reported to sites.
static UA_METADATA: RwLock<Option<serde_json::Value>> = RwLock::new(None);
/// 0 while the hints are being learned, 1 once known, 2 when they could not
/// be read (sites then see WebView2's own hints).
static HINTS: AtomicU8 = AtomicU8::new(0);

/// Navigations wait for the client hints while they are being learned, so
/// the first pages do not go out with the WebView2 brand.
pub fn client_hints_pending() -> bool {
    crate::prefs::fingerprint_protection() && HINTS.load(Ordering::SeqCst) == 0
}

/// Learn the client hints at startup, retrying while Jello's window loads.
pub fn start(app: &tauri::AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..20 {
            if HINTS.load(Ordering::SeqCst) == 1 {
                return;
            }
            learn_client_hints(&app);
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if HINTS.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            tracing::warn!("fingerprinting protection: client hints unavailable");
            crate::content_blocker::page::client_hints_learned();
        }
    });
}

/// Store one setting (database and in-memory copy).
fn store(db: &crate::db::DbState, key: &'static str, value: serde_json::Value) -> Result<(), String> {
    crate::prefs::apply(key, &value);
    let stored = value.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    db.execute(move |conn| {
        let res = conn.execute("INSERT OR REPLACE INTO settings (key, value_json) VALUES (?1, ?2)", [key, stored.as_str()]);
        let _ = tx.send(res.map(|_| ()).map_err(|e| e.to_string()));
    });
    rx.recv().map_err(|e| e.to_string())?
}

/// Settings > Privacy: switch fingerprinting protection. Pages pick the change
/// up at their next navigation.
#[tauri::command]
pub async fn fingerprint_protection_set(enabled: bool, db: tauri::State<'_, crate::db::DbState>) -> Result<(), String> {
    store(&db, "fingerprintProtection", serde_json::Value::Bool(enabled))?;
    crate::content_blocker::page::document_start_changed();
    Ok(())
}

/// Exempt a site from fingerprinting protection, or protect it again. Returns
/// whether the site is protected now.
#[tauri::command]
pub async fn fingerprint_site_toggle(host: String, db: tauri::State<'_, crate::db::DbState>) -> Result<bool, String> {
    let site = registrable(&host);
    if site.is_empty() {
        return Err("no site".to_string());
    }
    let mut sites = crate::prefs::fingerprint_allowed_sites();
    let protected = if sites.contains(&site) {
        sites.retain(|s| *s != site);
        true
    } else {
        sites.push(site);
        false
    };
    store(&db, "fingerprintAllowedSites", serde_json::json!(sites))?;
    crate::content_blocker::page::document_start_changed();
    Ok(protected)
}

/// `Emulation.setUserAgentOverride` parameters reporting Edge without the
/// WebView2 brand, from the hints `navigator.userAgentData` reported.
fn ua_override(hints: &serde_json::Value, language: &str) -> Option<serde_json::Value> {
    let ua = hints["ua"].as_str()?;
    let drop_embedder = |list: &serde_json::Value| -> serde_json::Value {
        serde_json::Value::Array(
            list.as_array()
                .map(|a| {
                    a.iter()
                        .filter(|b| !b["brand"].as_str().unwrap_or("").to_ascii_lowercase().contains("webview"))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        )
    };
    let text = |key: &str| hints[key].as_str().unwrap_or("").to_string();
    Some(serde_json::json!({
        "userAgent": ua,
        "acceptLanguage": language,
        "platform": "Win32",
        "userAgentMetadata": {
            "brands": drop_embedder(&hints["brands"]),
            "fullVersionList": drop_embedder(&hints["fullVersionList"]),
            "platform": text("platform"),
            "platformVersion": text("platformVersion"),
            "architecture": text("architecture"),
            "model": text("model"),
            "mobile": hints["mobile"].as_bool().unwrap_or(false),
            "bitness": text("bitness"),
            "wow64": hints["wow64"].as_bool().unwrap_or(false),
            "formFactors": hints["formFactors"].clone(),
        },
    }))
}

/// Script that reads the client hints, evaluated in Jello's own window.
const HINTS_JS: &str = r#"navigator.userAgentData.getHighEntropyValues(["architecture", "bitness", "model", "platformVersion", "fullVersionList", "wow64", "formFactors"]).then((v) => JSON.stringify({ ...v, ua: navigator.userAgent, language: navigator.language }))"#;

/// Read the client hints from a Jello window.
fn learn_client_hints(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        let Some(webview) = app
            .webviews()
            .into_values()
            .find(|w| w.url().is_ok_and(|u| u.host_str() == Some("tauri.localhost")))
        else {
            return;
        };
        let params = serde_json::json!({ "expression": HINTS_JS, "awaitPromise": true, "returnByValue": true }).to_string();
        let _ = webview.with_webview(move |w| unsafe {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::HSTRING;
            let Ok(core) = w.controller().CoreWebView2() else { return };
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |_, reply: String| {
                let reply: serde_json::Value = serde_json::from_str(&reply).unwrap_or_default();
                let hints = reply
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                if let Some(hints) = hints {
                    *UA_METADATA.write().unwrap() = Some(hints);
                    if HINTS.swap(1, Ordering::SeqCst) != 1 {
                        crate::content_blocker::page::client_hints_learned();
                    }
                }
                Ok(())
            }));
            let _ = core.CallDevToolsProtocolMethod(&HSTRING::from("Runtime.evaluate"), &HSTRING::from(params), &handler);
        });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = app;
}

/// The override for this browser, once the hints are known.
fn current_override() -> Option<serde_json::Value> {
    let hints = UA_METADATA.read().ok()?.clone()?;
    let language = hints["language"].as_str().unwrap_or("en-US").to_string();
    ua_override(&hints, &language)
}

#[cfg(target_os = "windows")]
mod devtools {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler};
    use windows::core::{Interface, HSTRING, PWSTR};

    /// Send a DevTools command to the page (`session` None) or to an attached
    /// target, without waiting for the reply.
    pub unsafe fn send(core: &ICoreWebView2, session: Option<&str>, method: &str, params: &serde_json::Value) {
        let done = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
        let params = HSTRING::from(params.to_string());
        let method = HSTRING::from(method);
        match session {
            None => {
                let _ = core.CallDevToolsProtocolMethod(&method, &params, &done);
            }
            Some(session) => {
                if let Ok(core11) = core.cast::<ICoreWebView2_11>() {
                    let _ = core11.CallDevToolsProtocolMethodForSession(&HSTRING::from(session), &method, &params, &done);
                }
            }
        }
    }

    /// Call `on_event(params, session)` for every `event` of the page and the
    /// targets attached to it.
    pub unsafe fn subscribe(core: &ICoreWebView2, event: &str, on_event: impl Fn(serde_json::Value, String) + 'static) {
        let Ok(receiver) = core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event)) else { return };
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut raw = PWSTR::null();
            let _ = args.ParameterObjectAsJson(&mut raw);
            let params = take(raw);
            let mut raw = PWSTR::null();
            let session = match args.cast::<ICoreWebView2DevToolsProtocolEventReceivedEventArgs2>() {
                Ok(args2) if args2.SessionId(&mut raw).is_ok() => take(raw),
                _ => String::new(),
            };
            on_event(serde_json::from_str(&params).unwrap_or_default(), session);
            Ok(())
        }));
        let mut token: i64 = 0;
        let _ = receiver.add_DevToolsProtocolEventReceived(&handler, &mut token);
    }

    fn take(raw: PWSTR) -> String {
        if raw.is_null() {
            return String::new();
        }
        unsafe {
            let out = raw.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(raw.0 as *const _));
            out
        }
    }
}

/// Hold every worker and cross-site frame of a content webview at start,
/// prepare it (fingerprinting protection in workers, client hints without the
/// WebView2 brand, the frame scripts `frame_scripts` returns), then let it
/// run. `top_origin` reads the tab's top-level origin, which workers take
/// their per-site values from.
#[cfg(target_os = "windows")]
pub unsafe fn attach(
    core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
    top_origin: impl Fn() -> String + 'static,
    frame_scripts: impl Fn() -> Vec<String> + 'static,
) {
    let target_core = core.clone();
    devtools::subscribe(core, "Target.attachedToTarget", move |params, _parent| {
        let Some(session) = params["sessionId"].as_str() else { return };
        let kind = params["targetInfo"]["type"].as_str().unwrap_or("");
        let core = &target_core;
        let ua = current_override();
        match kind {
            "worker" | "shared_worker" | "service_worker" => {
                // Dedicated workers share their page's site; shared and service
                // workers serve their own origin.
                let origin = if kind == "worker" {
                    top_origin()
                } else {
                    origin_of(params["targetInfo"]["url"].as_str().unwrap_or(""))
                };
                if let Some(js) = worker_script(&origin) {
                    devtools::send(core, Some(session), "Runtime.evaluate", &serde_json::json!({ "expression": js }));
                }
                if let Some(ua) = &ua {
                    devtools::send(core, Some(session), "Network.setUserAgentOverride", ua);
                }
            }
            "iframe" => {
                if let Some(ua) = &ua {
                    devtools::send(core, Some(session), "Emulation.setUserAgentOverride", ua);
                }
                let scripts = frame_scripts();
                if !scripts.is_empty() {
                    devtools::send(core, Some(session), "Page.enable", &serde_json::json!({}));
                    for js in scripts {
                        devtools::send(core, Some(session), "Page.addScriptToEvaluateOnNewDocument", &serde_json::json!({ "source": js }));
                    }
                }
            }
            _ => {}
        }
        devtools::send(core, Some(session), "Target.setAutoAttach", &auto_attach());
        devtools::send(core, Some(session), "Runtime.runIfWaitingForDebugger", &serde_json::json!({}));
    });
    devtools::send(core, None, "Target.setAutoAttach", &auto_attach());
    apply_client_hints(core);
    set_tracking_prevention(core);
}

/// Edge's tracking prevention for the webview's profile (the shared one, or
/// an InPrivate window's): the strict level, which keeps known trackers from
/// storing anything in third-party contexts.
#[cfg(target_os = "windows")]
unsafe fn set_tracking_prevention(core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use windows::core::Interface;
    let profile = core.cast::<ICoreWebView2_13>().and_then(|c| c.Profile()).and_then(|p| p.cast::<ICoreWebView2Profile3>());
    if let Ok(profile) = profile {
        let _ = profile.SetPreferredTrackingPreventionLevel(COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT);
    }
}

/// Report Edge's client hints, without the WebView2 brand, on the page.
#[cfg(target_os = "windows")]
pub unsafe fn apply_client_hints(core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2) {
    if let Some(ua) = current_override() {
        devtools::send(core, None, "Emulation.setUserAgentOverride", &ua);
    }
}

fn auto_attach() -> serde_json::Value {
    serde_json::json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true })
}

/// `scheme://host[:port]` of a URL, or "null".
fn origin_of(url: &str) -> String {
    tauri::Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_domains_follow_the_public_suffix_list() {
        assert_eq!(registrable("www.example.com"), "example.com");
        assert_eq!(registrable("example.com"), "example.com");
        assert_eq!(registrable("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable("maps.google.co.jp"), "google.co.jp");
        assert_eq!(registrable("someone.github.io"), "someone.github.io");
        assert_eq!(registrable("a.b.someone.github.io"), "someone.github.io");
        assert_eq!(registrable("com"), "com");
        assert_eq!(registrable("intranet"), "intranet");
        assert_eq!(registrable("127.0.0.1"), "127.0.0.1");
        assert_eq!(registrable("WWW.Example.COM."), "example.com");
        // Wildcard and exception rules (`*.ck`, `!www.ck`).
        assert_eq!(registrable("shop.example.ck"), "shop.example.ck");
        assert_eq!(registrable("www.ck"), "www.ck");
        // Internationalized suffixes are matched in punycode.
        assert_eq!(registrable("xn--fiqs8s.xn--fiqs8s"), "xn--fiqs8s.xn--fiqs8s");
    }

    #[test]
    fn marked_shared_worker_urls_give_back_the_original() {
        let m = shared_worker_marker();
        assert_eq!(shared_worker_original(&format!("https://a.example/w.js?{m}")).as_deref(), Some("https://a.example/w.js"));
        assert_eq!(shared_worker_original(&format!("https://a.example/w.js?v=2&{m}")).as_deref(), Some("https://a.example/w.js?v=2"));
        assert_eq!(shared_worker_original("https://a.example/w.js?v=2"), None);
        assert_eq!(shared_worker_original(&format!("https://a.example/{m}/w.js")), None);
    }

    #[test]
    fn suffix_rules_are_ascii_lines() {
        let rules = suffixes();
        assert!(rules.starts_with('\n') && rules.ends_with('\n'));
        assert!(rules.contains("\nco.uk\n"));
        assert!(rules.contains("\n*.ck\n"));
        assert!(rules.contains("\n!www.ck\n"));
        assert!(rules.is_ascii());
        assert!(!rules.contains("//"));
    }

    #[test]
    fn client_hints_drop_the_webview_brand() {
        let hints = serde_json::json!({
            "ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36 Edg/156.0.0.0",
            "brands": [{ "brand": "Chromium", "version": "156" }, { "brand": "Microsoft Edge", "version": "156" },
                       { "brand": "Not:A-Brand", "version": "8" }, { "brand": "Microsoft Edge WebView2", "version": "156" }],
            "fullVersionList": [{ "brand": "Chromium", "version": "156.0.8078.49" }, { "brand": "Microsoft Edge WebView2", "version": "156.0.4314.50" }],
            "platform": "Windows", "platformVersion": "19.0.0", "architecture": "x86", "bitness": "64",
            "model": "", "mobile": false, "wow64": false, "formFactors": ["Desktop"],
        });
        let o = ua_override(&hints, "en-US").unwrap();
        let brands: Vec<&str> = o["userAgentMetadata"]["brands"].as_array().unwrap().iter().map(|b| b["brand"].as_str().unwrap()).collect();
        assert_eq!(brands, ["Chromium", "Microsoft Edge", "Not:A-Brand"]);
        assert_eq!(o["userAgentMetadata"]["fullVersionList"].as_array().unwrap().len(), 1);
        assert_eq!(o["acceptLanguage"], "en-US");
        assert_eq!(o["userAgentMetadata"]["platformVersion"], "19.0.0");
        assert!(ua_override(&serde_json::json!({}), "en-US").is_none());
    }
}
