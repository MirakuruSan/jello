//! Hooks the built-in ad blocker into one content webview: its network
//! requests are checked against the engine, and each document gets its
//! cosmetic filters and scriptlets injected when it is created.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const PAGE_JS: &str = include_str!("page.js");
const PICKER_JS: &str = include_str!("picker.js");
/// Blocked requests kept per page for the request log.
const LOG_LIMIT: usize = 200;

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

/// One entry of a tab's request log.
#[derive(Clone, serde::Serialize)]
pub struct BlockedRequest {
    pub url: String,
    /// adblock request type: "script", "image", "subdocument", …
    pub kind: &'static str,
}

#[derive(Default)]
struct PageState {
    /// The top-level document being loaded or shown.
    top_url: String,
    /// Hostname of the top-level page the injected scripts belong to.
    top_host: String,
    /// The top-level script for `top_host` was requested.
    host_script: bool,
    /// Engine version the host script was built from.
    script_version: u64,
    /// Counts top-level navigations; registrations belong to one of them.
    generation: u64,
    /// A navigation held back until its scripts are registered.
    restart: Option<Restart>,
    /// The top-level URL and engine version the `$csp` script was made for.
    csp_url: String,
    csp_version: u64,
    /// Ids of the scripts injected for `top_host` and its frames.
    scripts: Vec<String>,
    /// Id of the `$csp` script of the current top-level URL.
    csp_script: Option<String>,
    /// Frame hostnames that already have their script on this site.
    frame_hosts: HashSet<String>,
    /// Requests blocked since the current page started loading, newest last.
    blocked: u32,
    log: VecDeque<BlockedRequest>,
}

/// A cancelled top-level navigation to start again once `pending`
/// registrations of navigation `generation` have completed.
struct Restart {
    generation: u64,
    url: String,
    /// The original request headers, "Name: value" lines.
    headers: String,
    pending: u32,
}

/// What a document-creation script is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    /// Cosmetics and scriptlets for the top-level page's host.
    Host,
    /// `$csp` policy for one top-level URL.
    Csp,
    /// Cosmetics and scriptlets for one frame host.
    Frame,
}

impl PageState {
    fn record(&mut self, url: &str, kind: &'static str) {
        self.blocked += 1;
        if self.log.len() == LOG_LIMIT {
            self.log.pop_front();
        }
        self.log.push_back(BlockedRequest { url: url.to_string(), kind });
    }
}

/// Live pages by tab id: the ⋯ menu's blocked count, the request log and the
/// element picker look pages up here.
struct PageEntry {
    tab: i32,
    state: Weak<Mutex<PageState>>,
    webview: tauri::Webview<tauri::Wry>,
}

static PAGES: Mutex<Vec<PageEntry>> = Mutex::new(Vec::new());

fn register(tab: i32, state: &Arc<Mutex<PageState>>, webview: &tauri::Webview<tauri::Wry>) {
    let mut pages = PAGES.lock().unwrap();
    pages.retain(|p| p.tab != tab && p.state.strong_count() > 0);
    pages.push(PageEntry { tab, state: Arc::downgrade(state), webview: webview.clone() });
}

fn page(tab: i32) -> Option<(Arc<Mutex<PageState>>, tauri::Webview<tauri::Wry>)> {
    let pages = PAGES.lock().unwrap();
    pages.iter().rev().find(|p| p.tab == tab).and_then(|p| Some((p.state.upgrade()?, p.webview.clone())))
}

/// What the built-in blocker did on a tab's current page.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabStats {
    host: String,
    paused: bool,
    blocked: u32,
    log: Vec<BlockedRequest>,
}

pub fn tab_stats(tab: i32) -> Option<TabStats> {
    let (state, _) = page(tab)?;
    let s = state.lock().ok()?;
    Some(TabStats {
        host: s.top_host.clone(),
        paused: crate::prefs::ad_blocking_paused(&s.top_host),
        blocked: s.blocked,
        log: s.log.iter().cloned().collect(),
    })
}

/// Start the element picker on a tab's page.
pub fn pick_element(tab: i32) -> Result<(), String> {
    let (_, webview) = page(tab).ok_or_else(|| "ad blocking is not active on this tab".to_string())?;
    let key = serde_json::to_string(channel_key()).unwrap_or_default();
    webview.eval(format!("{PICKER_JS}({key})")).map_err(|e| e.to_string())
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

/// `$csp` filters for a top-level URL: a script that adds their policy as a
/// meta tag before the page's own scripts run, or None. It is tied to the
/// exact URL because CSP filters can match on the path.
fn csp_script(url: &str) -> Option<String> {
    let engine = super::engine()?;
    let request = adblock::request::Request::new(url, url, "document", "GET").ok()?;
    let policy = engine.get_csp_directives(&request)?;
    let url = serde_json::to_string(url).ok()?;
    let policy = serde_json::to_string(&policy).ok()?;
    Some(format!("{CSP_JS}({url},{policy});"))
}

const CSP_JS: &str = r#"(function (url, policy) {
  if (location.href !== url) return;
  const add = () => {
    if (!document.head) return false;
    for (const m of document.head.querySelectorAll('meta[http-equiv="Content-Security-Policy"]')) {
      if (m.content === policy) return true;
    }
    const meta = document.createElement("meta");
    meta.httpEquiv = "Content-Security-Policy";
    meta.content = policy;
    document.head.prepend(meta);
    return true;
  };
  if (!add()) {
    const wait = new MutationObserver(() => add() && wait.disconnect());
    wait.observe(document, { childList: true, subtree: true });
  }
})"#;

/// The script that applies a document's scriptlets and cosmetic filters, or
/// None when nothing applies.
fn document_script(url: &str, top: bool) -> Option<String> {
    let engine = super::engine()?;
    let host = hostname(url)?;
    let res = engine.url_cosmetic_resources(url);
    let generic = !res.generichide;
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
        // uBlock Origin's scriptlets read a `scriptletGlobals` object that its
        // injector defines; without it they throw and do nothing.
        let once = serde_json::to_string(&format!("{}s", channel_key())).ok()?;
        js.push_str(&format!(
            "(function(){{if(location.hostname!=={host_json}||(window===window.top)!=={top}||window[{once}])return;\
             Object.defineProperty(window,{once},{{value:1}});\nconst scriptletGlobals={{}};\n{}\n}})();\n",
            res.injected_script
        ));
    }
    js.push_str(PAGE_JS);
    js.push_str(&format!("({cfg});"));
    Some(js)
}

/// A request on the page channel (`<origin>/__<key>/<what>?d=<json>`, see
/// page.js): what is asked, and its data. None for any other URL.
fn channel_request(url: &str, key: &str) -> Option<(String, serde_json::Value)> {
    if !url.contains(key) {
        return None;
    }
    let parsed = tauri::Url::parse(url).ok()?;
    let what = parsed.path().strip_prefix(&format!("/__{key}/"))?.to_string();
    let data = parsed
        .query_pairs()
        .find(|(k, _)| k == "d")
        .and_then(|(_, v)| serde_json::from_str(&v).ok())
        .unwrap_or(serde_json::Value::Null);
    Some((what, data))
}

/// The answer to a channel request from the document at `document_url`:
/// generic selectors for the classes and ids it reported, or the filter it
/// picked saved to My filters. Returns the JSON response body.
fn channel_reply(what: &str, data: &serde_json::Value, document_url: &str, app: &tauri::AppHandle) -> String {
    match what {
        "generic" => {
            let Some(engine) = super::engine() else { return "[]".to_string() };
            let res = engine.url_cosmetic_resources(document_url);
            if res.generichide {
                return "[]".to_string();
            }
            let take = |field: &str| -> Vec<String> {
                data[field]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str()).take(1000).map(str::to_string).collect())
                    .unwrap_or_default()
            };
            let selectors = engine.hidden_class_id_selectors(take("c"), take("i"), &res.exceptions);
            serde_json::to_string(&selectors).unwrap_or_else(|_| "[]".to_string())
        }
        "pick" => {
            let host = hostname(document_url).unwrap_or_default();
            let host = host.trim_start_matches("www.");
            if let Some(filter) = data["s"].as_str().and_then(|s| picked_filter(host, s)) {
                super::add_user_filter(app, &filter);
                use tauri::Emitter;
                let _ = app.emit("toast:show", format!("Hidden on {host}. Undo in Settings › Ad blocking › My filters"));
            }
            "[]".to_string()
        }
        _ => "[]".to_string(),
    }
}

/// A picked element's selector as a filter for `host`, or None when the
/// selector cannot be stored as one line of filter text.
fn picked_filter(host: &str, selector: &str) -> Option<String> {
    let selector = selector.trim();
    let valid = !host.is_empty()
        && !selector.is_empty()
        && selector.len() <= 500
        && !selector.contains(['\n', '\r'])
        && !selector.contains("##")
        && !selector.contains("#@#");
    valid.then(|| format!("{host}##{selector}"))
}

/// Route one content webview (tab `tab`) through the built-in blocker.
pub fn attach(webview: &tauri::Webview<tauri::Wry>, tab: i32) {
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        let state = Arc::new(Mutex::new(PageState::default()));
        register(tab, &state, webview);
        let app = webview.app_handle().clone();
        let _ = webview.with_webview(move |w| unsafe { hook(w, state, app) });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (webview, tab);
}

#[cfg(target_os = "windows")]
unsafe fn hook(w: tauri::webview::PlatformWebview, state: Arc<Mutex<PageState>>, app: tauri::AppHandle) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler, ContentLoadingEventHandler, ExecuteScriptCompletedHandler,
        NavigationStartingEventHandler, WebResourceRequestedEventHandler,
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

    let run_script = |core: &ICoreWebView2, js: String| {
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
        let _ = core.ExecuteScript(&HSTRING::from(js), &done);
    };
    let collapse = move |core: &ICoreWebView2, url: &str| {
        let key = serde_json::to_string(channel_key()).unwrap_or_default();
        let url = serde_json::to_string(url).unwrap_or_default();
        run_script(core, format!("window[{key}]&&window[{key}].collapse({url})"));
    };

    // Restarted navigations are created from the environment too.
    let script_env = env.clone();

    // Network requests, and the page channel (see channel_request).
    let req_state = state.clone();
    let req_app = app.clone();
    let on_request = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
        let Some(args) = args else { return Ok(()) };
        let Ok(req) = args.Request() else { return Ok(()) };
        let url = take_string(|p| req.Uri(p));
        let header = |name: &str| {
            req.Headers()
                .map(|h| take_string(|p| h.GetHeader(&HSTRING::from(name), p)))
                .unwrap_or_default()
        };
        if let Some((what, data)) = channel_request(&url, channel_key()) {
            // Answered here so it never reaches the site, even while the
            // blocker is switched off.
            let referer = header("Referer");
            let body = channel_reply(&what, &data, if referer.is_empty() { &url } else { &referer }, &req_app);
            let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(body.as_bytes()));
            let headers = "Content-Type: application/json\r\nCache-Control: no-store";
            if let Ok(response) =
                env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
            {
                let _ = args.SetResponse(&response);
            }
            return Ok(());
        }
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("wss://") || url.starts_with("ws://")) {
            return Ok(());
        }
        let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        let _ = args.ResourceContext(&mut ctx);
        let (top_url, top_host) = req_state.lock().map(|s| (s.top_url.clone(), s.top_host.clone())).unwrap_or_default();
        // Documents are decided by the navigation events below: top-level pages
        // always load, frames are cancelled at FrameNavigationStarting.
        if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
            return Ok(());
        }
        if crate::prefs::ad_blocking_paused(&top_host) {
            return Ok(());
        }
        let Some(engine) = super::engine() else { return Ok(()) };
        // The requesting document (or worker) is the filter context; a page
        // with no-referrer policy falls back to the top-level document.
        let referer = header("Referer");
        let source = if referer.is_empty() { top_url } else { referer };
        let method = take_string(|p| req.Method(p));
        let kind = request_type(ctx);
        let Ok(request) = adblock::request::Request::new(&url, &source, kind, &method) else {
            return Ok(());
        };
        let result = engine.check_network_request(&request);
        let response = if result.should_block() {
            super::count_blocked();
            if let Ok(mut s) = req_state.lock() {
                s.record(&url, kind);
            }
            if matches!(kind, "image" | "media" | "other") {
                if let Some(core) = &sender {
                    collapse(core, &url);
                }
            }
            match result.redirect.as_deref().and_then(decode_data_url) {
                Some((mime, body)) => {
                    let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&body));
                    let headers = format!("Content-Type: {mime}\r\nAccess-Control-Allow-Origin: *");
                    env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
                }
                // A redirect to a data: URL is refused by the browser, so the
                // page sees a network error, as with an extension's block.
                None => env.CreateWebResourceResponse(
                    None,
                    307,
                    &HSTRING::from("Blocked"),
                    &HSTRING::from(BLOCKED_HEADERS),
                ),
            }
        } else if let Some(clean) = result.rewritten_url.filter(|u| *u != url) {
            // `$removeparam`: send the request on to the cleaned URL.
            let origin = header("Origin");
            let allow = if origin.is_empty() { "*".to_string() } else { origin };
            let headers = format!(
                "Location: {clean}\r\nAccess-Control-Allow-Origin: {allow}\r\nAccess-Control-Allow-Credentials: true"
            );
            env.CreateWebResourceResponse(None, 307, &HSTRING::from("Temporary Redirect"), &HSTRING::from(headers))
        } else {
            return Ok(());
        };
        if let Ok(response) = response {
            let _ = args.SetResponse(&response);
        }
        Ok(())
    }));
    let mut token: i64 = 0;
    let _ = core.add_WebResourceRequested(&on_request, &mut token);

    // Adds a document-creation script and records its id: frame and host
    // scripts are removed when the top-level page moves to another host, the
    // `$csp` script when the top-level URL changes. When the navigation of
    // `generation` was held back for this registration (see below), the last
    // completion starts it again.
    let add_script = {
        let state = state.clone();
        let env = script_env;
        move |core: &ICoreWebView2, js: String, kind: ScriptKind, generation: u64| {
            let done_state = state.clone();
            let done_core = core.clone();
            let done_env = env.clone();
            let finish = move |id: Option<String>| {
                let mut resume = None;
                if let Ok(mut s) = done_state.lock() {
                    if let Some(id) = id {
                        match kind {
                            ScriptKind::Csp => s.csp_script = Some(id),
                            _ => s.scripts.push(id),
                        }
                    }
                    if let Some(restart) = s.restart.as_mut().filter(|r| r.generation == generation) {
                        restart.pending = restart.pending.saturating_sub(1);
                        if restart.pending == 0 {
                            resume = s.restart.take();
                        }
                    }
                }
                if let Some(restart) = resume {
                    let request = done_env.cast::<ICoreWebView2Environment2>().and_then(|env2| {
                        env2.CreateWebResourceRequest(
                            &HSTRING::from(restart.url.as_str()),
                            &HSTRING::from("GET"),
                            None,
                            &HSTRING::from(restart.headers.as_str()),
                        )
                    });
                    let resumed = request
                        .and_then(|r| done_core.cast::<ICoreWebView2_2>().and_then(|c| c.NavigateWithWebResourceRequest(&r)));
                    if resumed.is_err() {
                        let _ = done_core.Navigate(&HSTRING::from(restart.url.as_str()));
                    }
                }
            };
            let on_done = finish.clone();
            let done = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |hr, id: String| {
                on_done(hr.is_ok().then_some(id));
                Ok(())
            }));
            if core.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(js), &done).is_err() {
                // The completion will never run; a held navigation must resume.
                finish(None);
            }
        }
    };

    // Top-level navigations. A host's scripts stay registered while the page
    // stays on that host, so reloads and in-site navigation reuse them; moving
    // to another host replaces them. The `$csp` script depends on the full URL.
    // Navigations without a host (about:blank) leave everything alone.
    //
    // A script registered here would arrive too late for the document this
    // navigation creates, so a new-document GET navigation that needs one is
    // cancelled and started again, with the same request headers, once the
    // registration has completed; the second pass finds everything in place.
    // Back/forward, redirects and form posts cannot be replayed that way; they
    // rely on the copy run at ContentLoading.
    let nav_state = state.clone();
    let nav_add = add_script.clone();
    let on_navigation = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        let url = take_string(|p| args.Uri(p));
        let host = hostname(&url).filter(|_| url.starts_with("http"));
        let enabled = crate::prefs::builtin_adblock();
        let paused = host.as_deref().is_some_and(crate::prefs::ad_blocking_paused);
        let active = enabled && !paused;
        if active {
            if let Some(clean) = removeparam_target(&url, &args) {
                let _ = args.SetCancel(true);
                let _ = core.Navigate(&HSTRING::from(clean));
                return Ok(());
            }
        }
        let (in_place, keep_csp, generation, old) = match nav_state.lock() {
            Ok(mut s) => {
                s.top_url = url.clone();
                let Some(host) = host.clone() else { return Ok(()) };
                s.generation += 1;
                s.restart = None;
                s.blocked = 0;
                s.log.clear();
                let keep_csp = active && s.csp_url == url && s.csp_version == super::engine_version();
                let mut old = Vec::new();
                if !keep_csp {
                    old.extend(s.csp_script.take());
                    s.csp_url.clear();
                }
                let in_place =
                    active && s.top_host == host && s.host_script && s.script_version == super::engine_version();
                if !in_place {
                    s.top_host = host;
                    s.host_script = false;
                    s.frame_hosts.clear();
                    old.append(&mut s.scripts);
                }
                (in_place, keep_csp, s.generation, old)
            }
            Err(_) => return Ok(()),
        };
        for id in old {
            let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
        }
        if !active {
            return Ok(());
        }
        let host_js = if in_place { None } else { document_script(&url, true) };
        let csp_js = if keep_csp { None } else { csp_script(&url) };
        let pending = host_js.is_some() as u32 + csp_js.is_some() as u32;
        if pending > 0 {
            if let Some(headers) = replayable_headers(&args) {
                let _ = args.SetCancel(true);
                if let Ok(mut s) = nav_state.lock() {
                    s.restart = Some(Restart { generation, url: url.clone(), headers, pending });
                }
            }
        }
        if let Some(js) = host_js {
            if let Ok(mut s) = nav_state.lock() {
                s.host_script = true;
                s.script_version = super::engine_version();
            }
            nav_add(&core, js, ScriptKind::Host, generation);
        }
        if let Some(js) = csp_js {
            if let Ok(mut s) = nav_state.lock() {
                s.csp_url = url.clone();
                s.csp_version = super::engine_version();
            }
            nav_add(&core, js, ScriptKind::Csp, generation);
        }
        Ok(())
    }));
    let _ = core.add_NavigationStarting(&on_navigation, &mut token);

    // A cross-host redirect lands on a host whose script was not registered
    // in time, and a navigation can start before the engine is loaded, so the
    // committed document also gets its script through ExecuteScript. The
    // script's own guards make this copy a no-op when the registered one ran.
    let on_content = ContentLoadingEventHandler::create(Box::new(move |sender, _args| {
        let Some(core) = sender else { return Ok(()) };
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let url = take_string(|p| core.Source(p));
        if hostname(&url).is_some_and(|h| crate::prefs::ad_blocking_paused(&h)) {
            return Ok(());
        }
        if let Some(js) = csp_script(&url) {
            run_script(&core, js);
        }
        if let Some(js) = document_script(&url, true) {
            run_script(&core, js);
        }
        Ok(())
    }));
    let _ = core.add_ContentLoading(&on_content, &mut token);

    // Frame navigations: cancel blocked frames (ad iframes), and give the
    // rest one script per frame hostname on the page.
    let frame_state = state.clone();
    let frame_add = add_script.clone();
    let on_frame = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let (top_url, top_host) = frame_state.lock().map(|s| (s.top_url.clone(), s.top_host.clone())).unwrap_or_default();
        if crate::prefs::ad_blocking_paused(&top_host) {
            return Ok(());
        }
        let url = take_string(|p| args.Uri(p));
        if let Some(engine) = super::engine() {
            let blocked = adblock::request::Request::new(&url, &top_url, "subdocument", "GET")
                .map(|r| engine.check_network_request(&r).should_block())
                .unwrap_or(false);
            if blocked {
                super::count_blocked();
                if let Ok(mut s) = frame_state.lock() {
                    s.record(&url, "subdocument");
                }
                let _ = args.SetCancel(true);
                collapse(&core, &url);
                return Ok(());
            }
        }
        let Some(host) = hostname(&url) else { return Ok(()) };
        if !frame_state.lock().map(|mut s| s.frame_hosts.insert(host)).unwrap_or(false) {
            return Ok(());
        }
        if let Some(js) = document_script(&url, false) {
            frame_add(&core, js, ScriptKind::Frame, 0);
        }
        Ok(())
    }));
    let _ = core.add_FrameNavigationStarting(&on_frame, &mut token);
}

/// The request headers of a navigation that can be cancelled and started
/// again unchanged: a new document (not back/forward or reload), not a
/// redirect, and not a form post (whose body cannot be replayed).
#[cfg(target_os = "windows")]
unsafe fn replayable_headers(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
) -> Option<String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use windows::core::Interface;
    let mut kind = COREWEBVIEW2_NAVIGATION_KIND::default();
    args.cast::<ICoreWebView2NavigationStartingEventArgs3>().ok()?.NavigationKind(&mut kind).ok()?;
    let mut redirected = windows::core::BOOL::default();
    let _ = args.IsRedirected(&mut redirected);
    if kind != COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT || redirected.as_bool() {
        return None;
    }
    let headers = args.RequestHeaders().ok()?;
    let iter = headers.GetIterator().ok()?;
    let mut out = String::new();
    let mut has = windows::core::BOOL::default();
    let _ = iter.HasCurrentHeader(&mut has);
    while has.as_bool() {
        let mut name = windows::core::PWSTR::null();
        let mut value = windows::core::PWSTR::null();
        if iter.GetCurrentHeader(&mut name, &mut value).is_ok() {
            let name = take_string(|p| {
                *p = name;
                Ok(())
            });
            let value = take_string(|p| {
                *p = value;
                Ok(())
            });
            if name.eq_ignore_ascii_case("content-type") {
                return None;
            }
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        let mut next = windows::core::BOOL::default();
        if iter.MoveNext(&mut next).is_err() {
            break;
        }
        has = next;
    }
    Some(out)
}

/// Response headers for a blocked request (see the request handler).
const BLOCKED_HEADERS: &str = "Location: data:,\r\nCache-Control: no-store";

/// `$removeparam` on the page URL itself: the cleaned URL to load instead, for
/// plain GET navigations (a form post carries a Content-Type and is left
/// alone, since navigating again would turn it into a GET).
#[cfg(target_os = "windows")]
unsafe fn removeparam_target(
    url: &str,
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
) -> Option<String> {
    let engine = super::engine()?;
    let request = adblock::request::Request::new(url, url, "document", "GET").ok()?;
    let clean = engine.check_network_request(&request).rewritten_url.filter(|u| u != url)?;
    let posted = args
        .RequestHeaders()
        .map(|h| !take_string(|p| h.GetHeader(&windows::core::HSTRING::from("Content-Type"), p)).is_empty())
        .unwrap_or(false);
    (!posted).then_some(clean)
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

    #[test]
    fn picked_selectors_become_single_line_filters() {
        assert_eq!(picked_filter("example.com", " div.ad > a ").as_deref(), Some("example.com##div.ad > a"));
        assert_eq!(picked_filter("example.com", "div\n||evil.example^"), None);
        assert_eq!(picked_filter("example.com", "a##b"), None);
        assert_eq!(picked_filter("", "div.ad"), None);
        assert_eq!(picked_filter("example.com", ""), None);
    }

    #[test]
    fn channel_requests_are_recognised_by_key_and_path() {
        let key = "_0123456789abcdef";
        let (what, data) =
            channel_request("https://news.example/___0123456789abcdef/generic?d=%7B%22c%22%3A%5B%22ad%22%5D%7D", key)
                .unwrap();
        assert_eq!(what, "generic");
        assert_eq!(data["c"][0], "ad");
        // The key elsewhere in the URL, or another path, is not the channel.
        assert!(channel_request("https://news.example/page?x=_0123456789abcdef", key).is_none());
        assert!(channel_request("https://news.example/other/_0123456789abcdef/generic", key).is_none());
        assert!(channel_request("https://news.example/generic", key).is_none());
        // Bad data still answers (with nothing), it is not passed to the site.
        let (_, data) = channel_request("https://news.example/___0123456789abcdef/pick?d=not-json", key).unwrap();
        assert!(data.is_null());
    }

    #[test]
    fn request_log_keeps_the_newest_entries() {
        let mut s = PageState::default();
        for i in 0..LOG_LIMIT + 5 {
            s.record(&format!("https://ads.example/{i}"), "script");
        }
        assert_eq!(s.blocked as usize, LOG_LIMIT + 5);
        assert_eq!(s.log.len(), LOG_LIMIT);
        assert_eq!(s.log.front().unwrap().url, "https://ads.example/5");
    }
}
