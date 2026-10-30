//! Hooks one content webview: its network requests are checked against the
//! built-in blocker's engine, each document gets its cosmetic filters and
//! scriptlets injected when it is created, and every document starts with
//! the fingerprinting protection script (see crate::fingerprint).
//!
//! Scripts registered with `AddScriptToExecuteOnDocumentCreated` only reach
//! documents created after the registration completes, and WebView2 holds a
//! registration back until a pending navigation commits. So a top-level
//! navigation whose scripts are not in place yet is cancelled, the scripts are
//! registered, and the same navigation is started again: a new document or a
//! redirect with its original request headers, a back/forward step as the same
//! history entry, a reload as a reload, and a form post with the body it sent.
//! The same hold keeps pages from loading unfiltered while the engine itself
//! is still loading.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

const PAGE_JS: &str = include_str!("page.js");
const PICKER_JS: &str = include_str!("picker.js");
const BOOTSTRAP_JS: &str = include_str!("bootstrap.js");
/// Blocked requests kept per page for the request log.
const LOG_LIMIT: usize = 200;
/// The longest a navigation is held for its scripts or the engine.
const HOLD_LIMIT: Duration = Duration::from_secs(5);
/// Form posts with a larger body are not held (they load unchanged).
const POST_BODY_LIMIT: usize = 32 << 20;
/// Cross-site frame hosts per page whose scripts are kept for their frames.
const CROSS_FRAME_LIMIT: usize = 8;

/// Random per-session name for the page channel, the hand-over property the
/// document-start script uses, and the path redirect resources are served
/// under, so pages cannot rely on a fixed name to detect or disable them.
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
    /// Counts top-level navigations (a redirect continues its navigation).
    generation: u64,
    /// Registrations made for `generation` that have not completed yet.
    inflight: u32,
    /// The document-start script every document gets is registered.
    ready: bool,
    /// Id and version (see `document_start_version`) of the registered
    /// document-start script.
    document_start: Option<String>,
    document_start_version: u64,
    /// A cancelled navigation to start again once its scripts are in place.
    held: Option<Held>,
    /// A form post whose body is being kept to post it again.
    capture: Option<Capture>,
    /// URL of a navigation started again after a hold; it is never held twice.
    replaying: Option<String>,
    /// The current page's scripts may have been registered too late (its
    /// navigation could not be held), so they also run at ContentLoading.
    late: bool,
    /// The top-level URL and engine version the `$csp` script was made for.
    csp_url: String,
    csp_version: u64,
    /// Ids of the scripts injected for `top_host` and its frames.
    scripts: Vec<String>,
    /// Id of the `$csp` script of the current top-level URL.
    csp_script: Option<String>,
    /// Frame hostnames that already have their script on this site.
    frame_hosts: HashSet<String>,
    /// Scripts for the cross-site frame hosts of this page, newest last. Such
    /// frames run in their own process; each one gets these registered on its
    /// own DevTools session while it is held at start.
    cross_frame_scripts: VecDeque<(String, String)>,
    /// Requests blocked since the current page started loading, newest last.
    blocked: u32,
    log: VecDeque<BlockedRequest>,
    /// The element picker is open on this page.
    picking: bool,
}

struct Held {
    generation: u64,
    replay: Replay,
}

/// How a held navigation is started again.
enum Replay {
    /// A new document or a redirect: the same request with its headers.
    Get { url: String, headers: String },
    /// A form post, with the body captured on its way out.
    Post { url: String, headers: String, body: Vec<u8> },
    /// Back or forward: the same history entry.
    History { url: String },
    Reload { url: String },
}

impl Replay {
    fn url(&self) -> &str {
        match self {
            Replay::Get { url, .. } | Replay::Post { url, .. } | Replay::History { url } | Replay::Reload { url } => url,
        }
    }
}

/// A form post being held. Its request is answered with a redirect to the same
/// URL and the body is kept; the navigation is cancelled at that redirect and
/// posted again once the page's scripts are registered.
struct Capture {
    generation: u64,
    url: String,
    body: Option<Vec<u8>>,
}

/// What a document-creation script is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    /// Hands the page channel to Jello's scripts; registered once per webview.
    Bootstrap,
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

/// A new engine is in place: start navigations that were waiting for one.
pub(super) fn engine_published() {
    let live: Vec<_> = {
        let pages = PAGES.lock().unwrap();
        pages.iter().filter_map(|p| Some((p.state.upgrade()?, p.webview.clone()))).collect()
    };
    for (state, webview) in live {
        resume(&state, &webview, None);
    }
}

/// Bumped when the document-start script changes (fingerprinting protection
/// switched, a site exempted); pages register the new one at their next
/// navigation.
static DOCUMENT_START_VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn document_start_changed() {
    DOCUMENT_START_VERSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

fn document_start_version() -> u64 {
    DOCUMENT_START_VERSION.load(std::sync::atomic::Ordering::SeqCst)
}

/// The script registered for every document: fingerprinting protection, then
/// the page channel hand-over.
fn document_start_script() -> String {
    let key = serde_json::to_string(channel_key()).unwrap_or_default();
    let mut js = crate::fingerprint::document_script().unwrap_or_default();
    js.push_str(&format!("\n{BOOTSTRAP_JS}({key});"));
    js
}

/// The browser's client hints are known (or could not be read): report them
/// on every live page, then start navigations that waited for them.
pub fn client_hints_learned() {
    let live: Vec<_> = {
        let pages = PAGES.lock().unwrap();
        pages.iter().filter_map(|p| Some((p.state.upgrade()?, p.webview.clone()))).collect()
    };
    for (state, webview) in live {
        #[cfg(target_os = "windows")]
        let _ = webview.with_webview(|w| unsafe {
            if let Ok(core) = w.controller().CoreWebView2() {
                crate::fingerprint::apply_client_hints(&core);
            }
        });
        resume(&state, &webview, None);
    }
}

/// Start the held navigation again if nothing holds it any more: its scripts
/// are registered, the document-start script is in place and the engine is
/// loaded. `expired` names a hold generation whose time limit ran out.
fn resume(state: &Arc<Mutex<PageState>>, webview: &tauri::Webview<tauri::Wry>, expired: Option<u64>) {
    let replay = {
        let Ok(mut s) = state.lock() else { return };
        let Some(held) = s.held.as_ref() else { return };
        let expired = expired == Some(held.generation);
        let wait_engine = crate::prefs::builtin_adblock()
            && !crate::prefs::ad_blocking_paused(&s.top_host)
            && super::engine().is_none();
        let wait_hints = crate::fingerprint::client_hints_pending();
        if !expired && (s.inflight > 0 || !s.ready || wait_engine || wait_hints) {
            return;
        }
        let Some(held) = s.held.take() else { return };
        s.replaying = Some(held.replay.url().to_string());
        held.replay
    };
    #[cfg(target_os = "windows")]
    let _ = webview.with_webview(move |w| unsafe {
        if let Ok(core) = w.controller().CoreWebView2() {
            start_again(&core, replay);
        }
    });
    #[cfg(not(target_os = "windows"))]
    let _ = (webview, replay);
}

/// What the built-in blocker did on a tab's current page, and whether
/// fingerprinting protection covers it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabStats {
    host: String,
    /// The built-in blocker is the selected ad blocker.
    blocking: bool,
    paused: bool,
    blocked: u32,
    log: Vec<BlockedRequest>,
    /// The site fingerprinting exceptions apply to (registrable domain).
    site: String,
    /// Fingerprinting protection is on.
    fingerprint_protection: bool,
    /// The site is exempted from it.
    fingerprint_exempt: bool,
}

pub fn tab_stats(tab: i32) -> Option<TabStats> {
    let (state, _) = page(tab)?;
    let s = state.lock().ok()?;
    let site = crate::fingerprint::registrable(&s.top_host);
    Some(TabStats {
        host: s.top_host.clone(),
        blocking: crate::prefs::builtin_adblock(),
        paused: crate::prefs::ad_blocking_paused(&s.top_host),
        blocked: s.blocked,
        log: s.log.iter().cloned().collect(),
        fingerprint_protection: crate::prefs::fingerprint_protection(),
        fingerprint_exempt: crate::prefs::fingerprint_allowed_sites().contains(&site),
        site,
    })
}

/// Start the element picker on a tab's page. It runs as a DevTools evaluation
/// that resolves with the picked selector, which is saved to My filters.
pub fn pick_element(tab: i32) -> Result<(), String> {
    let (state, webview) = page(tab).ok_or_else(|| "ad blocking is not active on this tab".to_string())?;
    {
        let mut s = state.lock().map_err(|e| e.to_string())?;
        if s.picking {
            return Ok(());
        }
        s.picking = true;
    }
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        let app = webview.app_handle().clone();
        let params = serde_json::json!({
            "expression": PICKER_JS,
            "awaitPromise": true,
            "returnByValue": true,
        })
        .to_string();
        let failed = state.clone();
        let res = webview.with_webview(move |w| unsafe {
            use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
            use windows::core::HSTRING;
            let Ok(core) = w.controller().CoreWebView2() else {
                if let Ok(mut s) = state.lock() {
                    s.picking = false;
                }
                return;
            };
            let done_state = state.clone();
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |_, json: String| {
                let host = match done_state.lock() {
                    Ok(mut s) => {
                        s.picking = false;
                        s.top_host.trim_start_matches("www.").to_string()
                    }
                    Err(_) => return Ok(()),
                };
                let reply: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                let filter = reply.pointer("/result/value").and_then(|v| v.as_str()).and_then(|s| picked_filter(&host, s));
                if let Some(filter) = filter {
                    super::add_user_filter(&app, &filter);
                    use tauri::Emitter;
                    let _ = app.emit("toast:show", format!("Hidden on {host}. Undo in Settings › Ad blocking › My filters"));
                }
                Ok(())
            }));
            if core
                .CallDevToolsProtocolMethod(&HSTRING::from("Runtime.evaluate"), &HSTRING::from(params), &handler)
                .is_err()
            {
                if let Ok(mut s) = state.lock() {
                    s.picking = false;
                }
            }
        });
        if res.is_err() {
            if let Ok(mut s) = failed.lock() {
                s.picking = false;
            }
        }
        res.map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = webview;
        Err("unsupported platform".to_string())
    }
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

/// Hides the elements whose request was blocked (images, frames, embeds), so
/// they leave no empty box behind, like uBlock Origin's "collapse". The
/// preload scanner can request an image before its element exists, so a
/// document still loading is checked again once parsed.
const COLLAPSE_JS: &str = r#"(function (url) {
  const collapse = () => {
    for (const el of document.querySelectorAll("img[src],iframe[src],frame[src],embed[src],object[data],video[src],input[type=image][src]")) {
      if (el.src === url || el.data === url) el.style.setProperty("display", "none", "important");
    }
  };
  collapse();
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", collapse, { once: true });
})"#;

/// Path prefix scriptlets fetch redirect resources from (`war:` directives),
/// on the page's own origin; Jello answers these requests itself.
fn war_prefix() -> String {
    format!("/__{}/war/", channel_key())
}

/// The resource name of a scriptlet's redirect-resource request, if `url` is one.
fn war_request(url: &str) -> Option<String> {
    if !url.contains(channel_key()) {
        return None;
    }
    let parsed = tauri::Url::parse(url).ok()?;
    let name = parsed.path().strip_prefix(&war_prefix())?;
    (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
}

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
        // Scriptlets run in the page's world, before its own scripts.
        // uBlock Origin's scriptlets read a `scriptletGlobals` object that its
        // injector defines (without it they throw and do nothing); its
        // `warOrigin` is where they fetch redirect resources from.
        let war = serde_json::to_string(&war_prefix().trim_end_matches('/')).ok()?;
        js.push_str(&format!(
            "(function(){{if(location.hostname!=={host_json}||(window===window.top)!=={top})return;\
             const scriptletGlobals=/^https?:$/.test(location.protocol)?{{warOrigin:location.origin+{war}}}:{{}};\n{}\n}})();\n",
            res.injected_script
        ));
    }
    js.push_str(PAGE_JS);
    js.push_str(&format!("({cfg});"));
    Some(js)
}

/// The channel's answer for a document at `document_url`: the generic
/// selectors for the classes and ids it reported (`{"c": [...], "i": [...]}`),
/// as a JSON array.
fn generic_reply(state: &Weak<Mutex<PageState>>, document_url: &str, data: &str) -> String {
    const NONE: &str = "[]";
    if !crate::prefs::builtin_adblock() {
        return NONE.to_string();
    }
    let paused = state
        .upgrade()
        .and_then(|s| s.lock().ok().map(|s| crate::prefs::ad_blocking_paused(&s.top_host)))
        .unwrap_or(true);
    if paused {
        return NONE.to_string();
    }
    let Some(engine) = super::engine() else { return NONE.to_string() };
    let res = engine.url_cosmetic_resources(document_url);
    if res.generichide {
        return NONE.to_string();
    }
    let data: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
    let take = |field: &str| -> Vec<String> {
        data[field]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).take(1000).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let selectors = engine.hidden_class_id_selectors(take("c"), take("i"), &res.exceptions);
    serde_json::to_string(&selectors).unwrap_or_else(|_| NONE.to_string())
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

/// The id of the history entry for `url` nearest to the current entry, from a
/// `Page.getNavigationHistory` reply.
fn history_entry(reply: &str, url: &str) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_str(reply).ok()?;
    let current = v["currentIndex"].as_i64()?;
    v["entries"]
        .as_array()?
        .iter()
        .enumerate()
        .filter(|(i, e)| *i as i64 != current && e["url"].as_str() == Some(url))
        .min_by_key(|(i, _)| (*i as i64 - current).abs())
        .and_then(|(_, e)| e["id"].as_i64())
}

/// Whether "Name: value" header lines include `name`.
fn has_header(headers: &str, name: &str) -> bool {
    headers
        .lines()
        .any(|l| l.split_once(':').is_some_and(|(n, _)| n.trim().eq_ignore_ascii_case(name)))
}

/// Hook one content webview (tab `tab`), before its first navigation.
pub fn attach(webview: &tauri::Webview<tauri::Wry>, tab: i32) {
    #[cfg(target_os = "windows")]
    {
        let state = Arc::new(Mutex::new(PageState::default()));
        register(tab, &state, webview);
        let handle = webview.clone();
        let _ = webview.with_webview(move |w| unsafe { hook(w, state, handle) });
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (webview, tab);
}

#[cfg(target_os = "windows")]
unsafe fn hook(w: tauri::webview::PlatformWebview, state: Arc<Mutex<PageState>>, webview: tauri::Webview<tauri::Wry>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler, ContentLoadingEventHandler, ExecuteScriptCompletedHandler,
        NavigationStartingEventHandler, WebResourceRequestedEventHandler,
    };
    use windows::core::{Interface, HSTRING};

    let Ok(core) = w.controller().CoreWebView2() else { return };
    let Some(env) = core.cast::<ICoreWebView2_2>().ok().and_then(|c| c.Environment().ok()) else { return };
    let Ok(core22) = core.cast::<ICoreWebView2_22>() else {
        tracing::warn!("content blocker: WebView2 runtime too old for content tab hooks");
        return;
    };
    // Tabs opened while the built-in blocker is selected route their requests
    // through it; other tabs only intercept marked shared worker scripts
    // (fingerprinting protection). The source-kinds filter
    // also covers requests made inside cross-site iframes and by workers; the
    // plain filter only sees the top document's.
    let kinds = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT
        | COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SHARED_WORKER
        | COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER;
    let pattern = if crate::prefs::builtin_adblock() {
        "*".to_string()
    } else {
        // Only shared worker scripts marked by fingerprinting protection.
        format!("*{}", crate::fingerprint::shared_worker_marker())
    };
    let _ = core22.AddWebResourceRequestedFilterWithRequestSourceKinds(
        &HSTRING::from(pattern),
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        kinds,
    );

    let run_script = |core: &ICoreWebView2, js: String| {
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
        let _ = core.ExecuteScript(&HSTRING::from(js), &done);
    };
    let collapse = move |core: &ICoreWebView2, url: &str| {
        let url = serde_json::to_string(url).unwrap_or_default();
        run_script(core, format!("{COLLAPSE_JS}({url})"));
    };

    remove_framework_scripts(&core);

    // The page channel (channel.rs), reachable only by Jello's own scripts.
    // Host objects reach the top-level document; frames get theirs as they
    // are created, nested frames included.
    let reply_state = Arc::downgrade(&state);
    let channel = super::channel::new(Box::new(move |url, data| generic_reply(&reply_state, url, data)));
    let mut object = super::channel::variant(&channel);
    if core.AddHostObjectToScript(&HSTRING::from(channel_key()), &mut object).is_err() {
        tracing::warn!("content blocker: page channel unavailable");
    }
    let _ = windows::Win32::System::Variant::VariantClear(&mut object);
    if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
        let handler = webview2_com::FrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(frame) = args.and_then(|a| a.Frame().ok()) {
                expose_channel(&frame, &channel);
            }
            Ok(())
        }));
        let mut token: i64 = 0;
        let _ = core4.add_FrameCreated(&handler, &mut token);
    }

    // Network requests, and redirect resources requested by scriptlets.
    let req_state = state.clone();
    let req_env = env.clone();
    let on_request = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
        let Some(args) = args else { return Ok(()) };
        let Ok(req) = args.Request() else { return Ok(()) };
        let url = take_string(|p| req.Uri(p));
        let header = |name: &str| {
            req.Headers()
                .map(|h| take_string(|p| h.GetHeader(&HSTRING::from(name), p)))
                .unwrap_or_default()
        };
        if let Some(name) = war_request(&url) {
            // Answered here so it never reaches the site.
            let response = match super::war_resource(&name) {
                Some((mime, bytes)) => {
                    let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(&bytes));
                    let headers = format!("Content-Type: {mime}\r\nAccess-Control-Allow-Origin: *\r\nCache-Control: no-store");
                    req_env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
                }
                None => req_env.CreateWebResourceResponse(None, 404, &HSTRING::from("Not Found"), &HSTRING::from("")),
            };
            if let Ok(response) = response {
                let _ = args.SetResponse(&response);
            }
            return Ok(());
        }
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("wss://") || url.starts_with("ws://")) {
            return Ok(());
        }
        let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        let _ = args.ResourceContext(&mut ctx);
        let method = take_string(|p| req.Method(p));
        // A shared worker's main script gets fingerprinting protection first
        // (see fingerprint::shared_worker_script).
        if let Some(original) = crate::fingerprint::shared_worker_original(&url) {
            if let Some(js) = crate::fingerprint::shared_worker_script(&original) {
                let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(js.as_bytes()));
                let headers = "Content-Type: text/javascript\r\nCache-Control: no-store";
                if let Ok(response) =
                    req_env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
                {
                    let _ = args.SetResponse(&response);
                }
                return Ok(());
            }
        }
        // Documents are decided by the navigation events below: top-level pages
        // always load, frames are cancelled at FrameNavigationStarting. A form
        // post being held has its body kept here and is redirected to its own
        // URL, where the navigation handler cancels it (see Capture).
        if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
            if method.eq_ignore_ascii_case("POST") {
                if let Ok(mut s) = req_state.lock() {
                    let generation = s.generation;
                    let wanted = s
                        .capture
                        .as_ref()
                        .is_some_and(|c| c.generation == generation && c.url == url && c.body.is_none());
                    if wanted {
                        match req.Content().ok().and_then(|stream| read_stream(&stream, POST_BODY_LIMIT)) {
                            Some(body) => {
                                if let Some(c) = s.capture.as_mut() {
                                    c.body = Some(body);
                                }
                                let headers = format!("Location: {url}\r\nCache-Control: no-store");
                                if let Ok(response) = req_env.CreateWebResourceResponse(
                                    None,
                                    307,
                                    &HSTRING::from("Temporary Redirect"),
                                    &HSTRING::from(headers),
                                ) {
                                    let _ = args.SetResponse(&response);
                                }
                            }
                            None => s.capture = None,
                        }
                    }
                }
            }
            return Ok(());
        }
        if !crate::prefs::builtin_adblock() {
            return Ok(());
        }
        let (top_url, top_host) = req_state.lock().map(|s| (s.top_url.clone(), s.top_host.clone())).unwrap_or_default();
        if crate::prefs::ad_blocking_paused(&top_host) {
            return Ok(());
        }
        let Some(engine) = super::engine() else { return Ok(()) };
        // The requesting document (or worker) is the filter context; a page
        // with no-referrer policy falls back to the top-level document.
        let referer = header("Referer");
        let source = if referer.is_empty() { top_url } else { referer };
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
                    req_env.CreateWebResourceResponse(stream.as_ref(), 200, &HSTRING::from("OK"), &HSTRING::from(headers))
                }
                // A redirect to a data: URL is refused by the browser, so the
                // page sees a network error, as with an extension's block.
                None => req_env.CreateWebResourceResponse(
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
            req_env.CreateWebResourceResponse(None, 307, &HSTRING::from("Temporary Redirect"), &HSTRING::from(headers))
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
    // `$csp` script when the top-level URL changes. The last registration of
    // a held navigation's generation starts it again.
    let add_script = {
        let state = state.clone();
        let webview = webview.clone();
        move |core: &ICoreWebView2, js: String, kind: ScriptKind, generation: u64| {
            let done_state = state.clone();
            let done_webview = webview.clone();
            let finish = move |id: Option<String>| {
                if let Ok(mut s) = done_state.lock() {
                    match (kind, id) {
                        (ScriptKind::Bootstrap, id) => {
                            s.ready = true;
                            s.document_start = id;
                        }
                        (ScriptKind::Csp, Some(id)) => s.csp_script = Some(id),
                        (_, Some(id)) => s.scripts.push(id),
                        _ => {}
                    }
                    if kind != ScriptKind::Bootstrap && kind != ScriptKind::Frame && generation == s.generation {
                        s.inflight = s.inflight.saturating_sub(1);
                    }
                }
                resume(&done_state, &done_webview, None);
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

    // The document-start script runs first in every document from now on.
    if let Ok(mut s) = state.lock() {
        s.document_start_version = document_start_version();
    }
    add_script(&core, document_start_script(), ScriptKind::Bootstrap, 0);

    // Workers and cross-site frames: held at start and prepared through the
    // DevTools protocol (fingerprinting protection, client hints, and the
    // scriptlets of cross-site frames, which run in their own process).
    let origin_state = state.clone();
    let frames_state = state.clone();
    crate::fingerprint::attach(
        &core,
        move || {
            let url = origin_state.lock().map(|s| s.top_url.clone()).unwrap_or_default();
            tauri::Url::parse(&url).map(|u| u.origin().ascii_serialization()).unwrap_or_else(|_| "null".to_string())
        },
        move || {
            frames_state
                .lock()
                .map(|s| s.cross_frame_scripts.iter().map(|(_, js)| js.clone()).collect())
                .unwrap_or_default()
        },
    );

    // Top-level navigations. A host's scripts stay registered while the page
    // stays on that host, so reloads and in-site navigation reuse them; moving
    // to another host replaces them. The `$csp` script depends on the full URL.
    // Navigations without a host (about:blank) leave everything alone.
    let nav_state = state.clone();
    let nav_add = add_script.clone();
    let nav_webview = webview.clone();
    let on_navigation = NavigationStartingEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else { return Ok(()) };
        let url = take_string(|p| args.Uri(p));
        let host = hostname(&url).filter(|_| url.starts_with("http"));
        let enabled = crate::prefs::builtin_adblock();
        let paused = host.as_deref().is_some_and(crate::prefs::ad_blocking_paused);
        let active = enabled && !paused;
        let headers = request_headers(&args);
        let posted = headers.as_deref().is_some_and(|h| has_header(h, "Content-Type"));
        if active && !posted {
            if let Some(clean) = removeparam_target(&url) {
                let _ = args.SetCancel(true);
                let _ = core.Navigate(&HSTRING::from(clean));
                return Ok(());
            }
        }
        let mut redirected = windows::core::BOOL::default();
        let _ = args.IsRedirected(&mut redirected);
        let redirected = redirected.as_bool();
        let mut kind = COREWEBVIEW2_NAVIGATION_KIND::default();
        if let Ok(args3) = args.cast::<ICoreWebView2NavigationStartingEventArgs3>() {
            let _ = args3.NavigationKind(&mut kind);
        }

        let Ok(mut s) = nav_state.lock() else { return Ok(()) };
        s.top_url = url.clone();
        let Some(host) = host else { return Ok(()) };
        let replayed = s.replaying.take().is_some_and(|u| u == url);
        if !redirected {
            s.generation += 1;
            s.inflight = 0;
            s.held = None;
            s.blocked = 0;
            s.log.clear();
            if !posted {
                s.capture = None;
            }
        }
        let generation = s.generation;
        s.late = false;
        // A changed document-start script (protection settings) replaces the
        // registered one; the navigation waits for it like for host scripts.
        let mut stale_document_start = None;
        let renew_document_start = s.document_start_version != document_start_version();
        if renew_document_start {
            stale_document_start = s.document_start.take();
            s.ready = false;
            s.document_start_version = document_start_version();
        }
        let keep_csp = active && s.csp_url == url && s.csp_version == super::engine_version();
        let mut old = Vec::new();
        if !keep_csp {
            old.extend(s.csp_script.take());
            s.csp_url.clear();
        }
        let in_place = active && s.top_host == host && s.host_script && s.script_version == super::engine_version();
        if !in_place {
            s.top_host = host;
            s.host_script = false;
            s.frame_hosts.clear();
            s.cross_frame_scripts.clear();
            old.append(&mut s.scripts);
        }

        let host_js = if active && !in_place { document_script(&url, true) } else { None };
        let csp_js = if active && !keep_csp { csp_script(&url) } else { None };
        let needs_scripts = host_js.is_some() || csp_js.is_some();
        let wait_engine = active && super::engine().is_none();
        let wait_hints = crate::fingerprint::client_hints_pending();
        // A redirect continues a navigation whose registrations may still be
        // pending (a held form post reaches here through its redirect).
        if (needs_scripts || s.inflight > 0 || wait_engine || wait_hints || !s.ready) && !replayed {
            let replay = if posted {
                match s.capture.take() {
                    Some(Capture { generation: g, url: u, body: Some(body) }) if redirected && g == generation && u == url => {
                        headers.map(|headers| Replay::Post { url: url.clone(), headers, body })
                    }
                    _ => {
                        if !redirected {
                            // Held at the redirect the request handler answers with.
                            s.capture = Some(Capture { generation, url: url.clone(), body: None });
                        }
                        None
                    }
                }
            } else if kind == COREWEBVIEW2_NAVIGATION_KIND_BACK_OR_FORWARD {
                Some(Replay::History { url: url.clone() })
            } else if kind == COREWEBVIEW2_NAVIGATION_KIND_RELOAD {
                Some(Replay::Reload { url: url.clone() })
            } else {
                headers.map(|headers| Replay::Get { url: url.clone(), headers })
            };
            match replay {
                Some(replay) => {
                    let _ = args.SetCancel(true);
                    s.held = Some(Held { generation, replay });
                    let timer_state = nav_state.clone();
                    let timer_webview = nav_webview.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(HOLD_LIMIT);
                        resume(&timer_state, &timer_webview, Some(generation));
                    });
                }
                None => s.late = needs_scripts,
            }
        } else if needs_scripts {
            s.late = true;
        }
        if host_js.is_some() {
            s.host_script = true;
            s.script_version = super::engine_version();
            s.inflight += 1;
        }
        if csp_js.is_some() {
            s.csp_url = url.clone();
            s.csp_version = super::engine_version();
            s.inflight += 1;
        }
        let held = s.held.is_some();
        drop(s);

        for id in old.into_iter().chain(stale_document_start) {
            let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
        }
        if renew_document_start {
            nav_add(&core, document_start_script(), ScriptKind::Bootstrap, generation);
        }
        if let Some(js) = host_js {
            nav_add(&core, js, ScriptKind::Host, generation);
        }
        if let Some(js) = csp_js {
            nav_add(&core, js, ScriptKind::Csp, generation);
        }
        if held {
            // Nothing left to register: it may only be waiting for the engine.
            resume(&nav_state, &nav_webview, None);
        }
        Ok(())
    }));
    let _ = core.add_NavigationStarting(&on_navigation, &mut token);

    // A navigation that could not be held (a form post with no readable body,
    // a redirected post) gets its scripts through ExecuteScript once its
    // document exists, which may be after the page's first scripts.
    let content_state = state.clone();
    let on_content = ContentLoadingEventHandler::create(Box::new(move |sender, _args| {
        let Some(core) = sender else { return Ok(()) };
        let late = content_state.lock().map(|mut s| std::mem::take(&mut s.late)).unwrap_or(false);
        if !late || !crate::prefs::builtin_adblock() {
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
        if !frame_state.lock().map(|mut s| s.frame_hosts.insert(host.clone())).unwrap_or(false) {
            return Ok(());
        }
        let Some(js) = document_script(&url, false) else { return Ok(()) };
        // A frame from another site runs in a process of its own, which a
        // registration made now can miss; it gets the script on its own
        // DevTools session while held at start (fingerprint::attach).
        let cross_site = crate::fingerprint::registrable(&host) != crate::fingerprint::registrable(&top_host);
        if cross_site {
            if let Ok(mut s) = frame_state.lock() {
                if s.cross_frame_scripts.len() == CROSS_FRAME_LIMIT {
                    s.cross_frame_scripts.pop_front();
                }
                s.cross_frame_scripts.push_back((host, js));
            }
        } else {
            frame_add(&core, js, ScriptKind::Frame, 0);
        }
        Ok(())
    }));
    let _ = core.add_FrameNavigationStarting(&on_frame, &mut token);
}

/// Remove the scripts the window framework registered while the webview was
/// created: content pages never use them, and they would show every site the
/// framework's globals. Script ids count up per webview from "1". WebView2
/// registers its own `chrome.webview` setup first, as id 1, when the webview
/// is created; the framework's scripts follow, and a marker script
/// registered now gets the id after their last one. Ids 2 up to the marker
/// are removed.
#[cfg(target_os = "windows")]
unsafe fn remove_framework_scripts(core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2) {
    use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
    use windows::core::HSTRING;
    let marker_core = core.clone();
    let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |hr, id: String| {
        if let Some(marker) = hr.ok().and_then(|_| id.parse::<u32>().ok()) {
            for older in 2..=marker {
                let _ = marker_core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(older.to_string()));
            }
        }
        Ok(())
    }));
    let _ = core.AddScriptToExecuteOnDocumentCreated(&HSTRING::from(";"), &handler);
}

/// Add the page channel to a frame, and to the frames it creates in turn.
#[cfg(target_os = "windows")]
unsafe fn expose_channel(
    frame: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Frame,
    channel: &windows::Win32::System::Com::IDispatch,
) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use windows::core::{Interface, HSTRING, PCWSTR};
    let mut object = super::channel::variant(channel);
    let any = HSTRING::from("*");
    let origins = [PCWSTR(any.as_ptr())];
    let _ = frame.AddHostObjectToScriptWithOrigins(&HSTRING::from(channel_key()), &mut object, 1, origins.as_ptr());
    let _ = windows::Win32::System::Variant::VariantClear(&mut object);
    if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
        let channel = channel.clone();
        let handler = webview2_com::FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(child) = args.and_then(|a| a.Frame().ok()) {
                expose_channel(&child, &channel);
            }
            Ok(())
        }));
        let mut token: i64 = 0;
        let _ = frame7.add_FrameCreated(&handler, &mut token);
    }
}

/// Start a held navigation again (see `Replay`).
#[cfg(target_os = "windows")]
unsafe fn start_again(core: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, replay: Replay) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows::core::{Interface, HSTRING};

    let request = |url: &str, method: &str, headers: &str, body: Option<&[u8]>| -> windows::core::Result<()> {
        let env = core.cast::<ICoreWebView2_2>()?.Environment()?.cast::<ICoreWebView2Environment2>()?;
        let stream = body.and_then(|b| windows::Win32::UI::Shell::SHCreateMemStream(Some(b)));
        let request = env.CreateWebResourceRequest(
            &HSTRING::from(url),
            &HSTRING::from(method),
            stream.as_ref(),
            &HSTRING::from(headers),
        )?;
        core.cast::<ICoreWebView2_2>()?.NavigateWithWebResourceRequest(&request)
    };
    match replay {
        Replay::Get { url, headers } => {
            if request(&url, "GET", &headers, None).is_err() {
                let _ = core.Navigate(&HSTRING::from(url));
            }
        }
        Replay::Post { url, headers, body } => {
            let _ = request(&url, "POST", &headers, Some(&body));
        }
        Replay::Reload { .. } => {
            let _ = core.Reload();
        }
        Replay::History { url } => {
            let nav_core = core.clone();
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, reply: String| {
                let entry = hr.ok().and_then(|_| history_entry(&reply, &url));
                match entry {
                    Some(id) => {
                        let params = serde_json::json!({ "entryId": id }).to_string();
                        let done = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
                        let _ = nav_core.CallDevToolsProtocolMethod(
                            &HSTRING::from("Page.navigateToHistoryEntry"),
                            &HSTRING::from(params),
                            &done,
                        );
                    }
                    None => {
                        let _ = nav_core.Navigate(&HSTRING::from(url));
                    }
                }
                Ok(())
            }));
            let _ = core.CallDevToolsProtocolMethod(
                &HSTRING::from("Page.getNavigationHistory"),
                &HSTRING::from("{}"),
                &handler,
            );
        }
    }
}

/// A navigation's request headers as "Name: value" lines.
#[cfg(target_os = "windows")]
unsafe fn request_headers(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
) -> Option<String> {
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

/// The whole of a request body, or None when it cannot be read or is larger
/// than `limit`.
#[cfg(target_os = "windows")]
unsafe fn read_stream(stream: &windows::Win32::System::Com::IStream, limit: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut read = 0u32;
        let hr = stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read as *mut u32));
        if hr.is_err() {
            return None;
        }
        if read == 0 {
            return Some(out);
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > limit {
            return None;
        }
    }
}

/// Response headers for a blocked request (see the request handler).
const BLOCKED_HEADERS: &str = "Location: data:,\r\nCache-Control: no-store";

/// `$removeparam` on the page URL itself: the cleaned URL to load instead.
fn removeparam_target(url: &str) -> Option<String> {
    let engine = super::engine()?;
    let request = adblock::request::Request::new(url, url, "document", "GET").ok()?;
    engine.check_network_request(&request).rewritten_url.filter(|u| u != url)
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
    fn war_requests_are_recognised_by_key_and_path() {
        let prefix = war_prefix();
        let url = format!("https://news.example{prefix}noop-vast3.xml?secret=x");
        assert_eq!(war_request(&url).as_deref(), Some("noop-vast3.xml"));
        assert!(war_request(&format!("https://news.example{prefix}")).is_none());
        assert!(war_request(&format!("https://news.example{prefix}a/b.js")).is_none());
        assert!(war_request("https://news.example/__other/war/noop.js").is_none());
        assert!(war_request(&format!("https://news.example/page?u={prefix}noop.js")).is_none());
    }

    #[test]
    fn history_entries_are_found_nearest_the_current_one() {
        let reply = r#"{"currentIndex":3,"entries":[
            {"id":10,"url":"https://a.example/"},
            {"id":11,"url":"https://b.example/"},
            {"id":12,"url":"https://a.example/"},
            {"id":13,"url":"https://c.example/"},
            {"id":14,"url":"https://b.example/"}]}"#;
        assert_eq!(history_entry(reply, "https://a.example/"), Some(12));
        assert_eq!(history_entry(reply, "https://b.example/"), Some(14));
        assert_eq!(history_entry(reply, "https://c.example/"), None);
        assert_eq!(history_entry(reply, "https://d.example/"), None);
        assert_eq!(history_entry("not json", "https://a.example/"), None);
    }

    #[test]
    fn form_posts_are_recognised_by_content_type() {
        assert!(has_header("Referer: https://a.example/\r\ncontent-type: application/x-www-form-urlencoded\r\n", "Content-Type"));
        assert!(!has_header("Referer: https://a.example/content-type\r\nAccept: */*\r\n", "Content-Type"));
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
