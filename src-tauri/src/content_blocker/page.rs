//! Hooks the built-in ad blocker into one content webview: its network
//! requests are checked against the engine, and each document gets its
//! cosmetic filters and scriptlets injected when it is created.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

const PAGE_JS: &str = include_str!("page.js");

/// Random per-session name for the page <-> host cosmetic channel, so pages
/// cannot rely on a fixed global to detect or disable it.
fn channel_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        format!("_{:016x}", h.finish())
    })
}

#[derive(Default)]
struct PageState {
    /// The top-level document being loaded or shown.
    top_url: String,
    /// Hostname of the top-level page the injected scripts belong to.
    top_host: String,
    /// Class/id selectors exempt from generic rules on the top-level document.
    exceptions: HashSet<String>,
    /// Ids of the scripts injected for `top_host` and its frames.
    scripts: Vec<String>,
    /// Frame hostnames that already have their script on this site.
    frame_hosts: HashSet<String>,
}

fn hostname(url: &str) -> Option<String> {
    tauri::Url::parse(url).ok()?.host_str().map(str::to_string)
}

/// adblock request type for a WebView2 resource context.
#[cfg(target_os = "windows")]
fn request_type(
    ctx: webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT,
) -> &'static str {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "subdocument",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "csp_report",
        _ => "other",
    }
}

/// The script that applies a document's scriptlets and cosmetic filters, or
/// None when nothing applies. Returns the generic-rule exceptions too.
fn document_script(url: &str, top: bool) -> Option<(String, HashSet<String>)> {
    let engine = super::engine()?;
    let host = hostname(url)?;
    let res = engine.url_cosmetic_resources(url);
    let generic = top && !res.generichide;
    if res.hide_selectors.is_empty() && res.procedural_actions.is_empty() && res.injected_script.is_empty() && !generic {
        return None;
    }
    let host_json = serde_json::to_string(&host).ok()?;
    let cfg = serde_json::json!({
        "host": host,
        "top": top,
        "key": channel_key(),
        "css": res.hide_selectors,
        "procedural": res.procedural_actions,
        "generic": generic,
    });
    let mut js = String::with_capacity(PAGE_JS.len() + res.injected_script.len() + 256);
    if !res.injected_script.is_empty() {
        // Scriptlets must run before the page's own scripts, in its world,
        // and once per document even if a stale copy of this script remains.
        let once = serde_json::to_string(&format!("{}s", channel_key())).ok()?;
        js.push_str(&format!(
            "(function(){{if(location.hostname!=={host_json}||(window===window.top)!=={top}||window[{once}])return;\
             Object.defineProperty(window,{once},{{value:1}});\n{}\n}})();\n",
            res.injected_script
        ));
    }
    js.push_str(PAGE_JS);
    js.push_str(&format!("({cfg});"));
    Some((js, res.exceptions))
}

/// Route one content webview through the built-in blocker.
pub fn attach(webview: &tauri::Webview<tauri::Wry>) {
    #[cfg(target_os = "windows")]
    {
        let state = Arc::new(Mutex::new(PageState::default()));
        let _ = webview.with_webview(move |w| unsafe { hook(w, state) });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = webview;
}

#[cfg(target_os = "windows")]
unsafe fn hook(w: tauri::webview::PlatformWebview, state: Arc<Mutex<PageState>>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler, ContentLoadingEventHandler, ExecuteScriptCompletedHandler,
        NavigationStartingEventHandler, WebMessageReceivedEventHandler, WebResourceRequestedEventHandler,
    };
    use windows::core::{Interface, HSTRING};

    let Ok(core) = w.controller().CoreWebView2() else { return };
    let Some(env) = core.cast::<ICoreWebView2_2>().ok().and_then(|c| c.Environment().ok()) else { return };
    // The source-kinds filter also covers requests made inside cross-site
    // iframes and by workers; the plain filter only sees the top document's.
    let Ok(core22) = core.cast::<ICoreWebView2_22>() else {
        tracing::warn!("content blocker: WebView2 runtime too old for request filtering");
        return;
    };
    let kinds = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT
        | COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SHARED_WORKER
        | COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER;
    if core22
        .AddWebResourceRequestedFilterWithRequestSourceKinds(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, kinds)
        .is_err()
    {
        return;
    }

    // Network requests.
    let req_state = state.clone();
    let on_request = WebResourceRequestedEventHandler::create(Box::new(move |_s, args| {
        let Some(args) = args else { return Ok(()) };
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let Some(engine) = super::engine() else { return Ok(()) };
        let Ok(req) = args.Request() else { return Ok(()) };
        let url = take_string(|p| req.Uri(p));
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("wss://") || url.starts_with("ws://")) {
            return Ok(());
        }
        let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        let _ = args.ResourceContext(&mut ctx);
        // Documents are decided by the navigation events below: top-level
        // pages always load, frames are cancelled at FrameNavigationStarting.
        if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
            return Ok(());
        }
        // The requesting document (or worker) is the filter context; a page
        // with no-referrer policy falls back to the top-level document.
        let referer = req
            .Headers()
            .map(|h| take_string(|p| h.GetHeader(&HSTRING::from("Referer"), p)))
            .unwrap_or_default();
        let source = if referer.is_empty() {
            req_state.lock().map(|s| s.top_url.clone()).unwrap_or_default()
        } else {
            referer
        };
        let method = take_string(|p| req.Method(p));
        let Ok(request) = adblock::request::Request::new(&url, &source, request_type(ctx), &method) else {
            return Ok(());
        };
        let result = engine.check_network_request(&request);
        if !result.should_block() {
            return Ok(());
        }
        super::count_blocked();
        let response = match result.redirect.as_deref().and_then(decode_data_url) {
            Some((mime, body)) => {
                let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&body));
                let headers = format!("Content-Type: {mime}\r\nAccess-Control-Allow-Origin: *");
                env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
            }
            None => env.CreateWebResourceResponse(None, 403, &HSTRING::from("Blocked"), &HSTRING::from("")),
        };
        if let Ok(response) = response {
            let _ = args.SetResponse(&response);
        }
        Ok(())
    }));
    let mut token: i64 = 0;
    let _ = core.add_WebResourceRequested(&on_request, &mut token);

    // Adds a document-creation script and records its id for removal when the
    // top-level page moves to another host.
    let add_script = {
        let state = state.clone();
        move |core: &ICoreWebView2, js: String| {
            let state = state.clone();
            let done = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |hr, id: String| {
                if hr.is_ok() {
                    if let Ok(mut s) = state.lock() {
                        s.scripts.push(id);
                    }
                }
                Ok(())
            }));
            let _ = core.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(js), &done);
        }
    };

    // Top-level navigations. A host's scripts stay registered while the page
    // stays on that host: a script added during NavigationStarting can miss
    // the document it was added for, so reloads and in-site navigation rely
    // on the copy that is already in place. Moving to another host replaces
    // them; navigations without a host (about:blank) leave them alone.
    let nav_state = state.clone();
    let nav_add = add_script.clone();
    let on_navigation = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        let url = take_string(|p| args.Uri(p));
        let host = hostname(&url).filter(|_| url.starts_with("http"));
        let enabled = crate::prefs::builtin_adblock();
        let old = match nav_state.lock() {
            Ok(mut s) => {
                s.top_url = url.clone();
                let same_host = host.as_deref() == Some(s.top_host.as_str());
                if host.is_none() || (same_host && enabled) {
                    return Ok(());
                }
                s.top_host = host.clone().unwrap_or_default();
                s.frame_hosts.clear();
                s.exceptions.clear();
                std::mem::take(&mut s.scripts)
            }
            Err(_) => return Ok(()),
        };
        for id in old {
            let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
        }
        if !enabled {
            if let Ok(mut s) = nav_state.lock() {
                s.top_host.clear();
            }
            return Ok(());
        }
        if let Some((js, exceptions)) = document_script(&url, true) {
            if let Ok(mut s) = nav_state.lock() {
                s.exceptions = exceptions;
            }
            nav_add(&core, js);
        }
        Ok(())
    }));
    let _ = core.add_NavigationStarting(&on_navigation, &mut token);

    // The first document on a new host (and the target of a cross-host
    // redirect) can be created before its script is registered, so the
    // committed document also gets it through ExecuteScript. The script's own
    // guards make this copy a no-op when the registered one already ran.
    let load_state = state.clone();
    let on_content = ContentLoadingEventHandler::create(Box::new(move |sender, _args| {
        let Some(core) = sender else { return Ok(()) };
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let url = take_string(|p| core.Source(p));
        let Some((js, exceptions)) = document_script(&url, true) else { return Ok(()) };
        if let Ok(mut s) = load_state.lock() {
            s.exceptions = exceptions;
        }
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
        let _ = core.ExecuteScript(&HSTRING::from(js), &done);
        Ok(())
    }));
    let _ = core.add_ContentLoading(&on_content, &mut token);

    // Frame navigations: cancel blocked frames (ad iframes), and give the
    // rest one script per frame hostname on the page.
    let frame_state = state.clone();
    let on_frame = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let url = take_string(|p| args.Uri(p));
        if let Some(engine) = super::engine() {
            let top_url = frame_state.lock().map(|s| s.top_url.clone()).unwrap_or_default();
            let blocked = adblock::request::Request::new(&url, &top_url, "subdocument", "GET")
                .map(|r| engine.check_network_request(&r).should_block())
                .unwrap_or(false);
            if blocked {
                super::count_blocked();
                let _ = args.SetCancel(true);
                return Ok(());
            }
        }
        let Some(host) = hostname(&url) else { return Ok(()) };
        if !frame_state.lock().map(|mut s| s.frame_hosts.insert(host)).unwrap_or(false) {
            return Ok(());
        }
        if let Some((js, _)) = document_script(&url, false) {
            add_script(&core, js);
        }
        Ok(())
    }));
    let _ = core.add_FrameNavigationStarting(&on_frame, &mut token);

    // Generic cosmetic rules: the page reports class names and ids, the
    // engine answers with the generic selectors that match them.
    let msg_state = state;
    let on_message = WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        let raw = take_string(|p| args.WebMessageAsJson(p));
        if !raw.contains(channel_key()) {
            return Ok(());
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&raw) else { return Ok(()) };
        let Some(engine) = super::engine() else { return Ok(()) };
        let take = |key: &str| -> Vec<String> {
            msg[key]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).take(2000).map(str::to_string).collect())
                .unwrap_or_default()
        };
        let exceptions = msg_state.lock().map(|s| s.exceptions.clone()).unwrap_or_default();
        let selectors = engine.hidden_class_id_selectors(take("c"), take("i"), &exceptions);
        if selectors.is_empty() {
            return Ok(());
        }
        let key = serde_json::to_string(channel_key()).unwrap_or_default();
        let list = serde_json::to_string(&selectors).unwrap_or_default();
        let js = format!("window[{key}]&&window[{key}]({list})");
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
        let _ = core.ExecuteScript(&HSTRING::from(js), &done);
        Ok(())
    }));
    let _ = core.add_WebMessageReceived(&on_message, &mut token);
}

/// Read a string out-parameter from WebView2 and free it (the API allocates
/// it with CoTaskMemAlloc; the request handler reads several per request).
#[cfg(target_os = "windows")]
fn take_string(get: impl FnOnce(*mut windows::core::PWSTR) -> windows::core::Result<()>) -> String {
    let mut raw = windows::core::PWSTR::null();
    if get(&mut raw).is_err() || raw.is_null() {
        return String::new();
    }
    unsafe {
        let out = raw.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(raw.0 as *const _));
        out
    }
}

/// Split an adblock redirect (`data:<mime>;base64,<body>`) into MIME and bytes.
fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    let rest = url.strip_prefix("data:")?;
    let (header, body) = rest.split_once(',')?;
    let mime = header.strip_suffix(";base64")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(body).ok()?;
    Some((mime.to_string(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_data_urls_decode() {
        let (mime, body) = decode_data_url("data:application/javascript;base64,KGZ1bmN0aW9uKCl7fSkoKQ==").unwrap();
        assert_eq!(mime, "application/javascript");
        assert_eq!(body, b"(function(){})()");
        assert!(decode_data_url("https://example.com/").is_none());
    }
}
