// Runs in every document of a content tab, before the page's own scripts
// (after fingerprinting protection, when that is on).
//
// `chrome.webview` is removed, with its interface object, so pages cannot
// tell they are running in an embedded WebView2 rather than in Edge.
//
// The host channel (see channel.rs) lives in `chrome.webview.hostObjects`,
// and WebView2 looks `chrome.webview` up when it is called. So the channel is
// left, under the per-session key, as a function that puts `chrome.webview`
// back while it issues each call; no page script can run in between.
// Jello's filtering script runs next and takes it. Whatever is left is
// removed before the first page script runs: the parser performs a
// microtask checkpoint, delivering mutation records, before it executes a
// script element.
(function (key) {
  const chrome = window.chrome;
  if (!chrome) return;
  const webview = Object.getOwnPropertyDescriptor(chrome, "webview");
  const iface = Object.getOwnPropertyDescriptor(window, "EmbeddedBrowserWebView");
  let channel;
  try {
    channel = chrome.webview.hostObjects[key];
  } catch (e) {}
  try {
    delete chrome.webview;
    delete window.EmbeddedBrowserWebView;
  } catch (e) {}
  if (!channel || !webview || (location.protocol !== "https:" && location.protocol !== "http:")) return;
  const define = Object.defineProperty;
  const generic = (url, data) => {
    define(chrome, "webview", webview);
    if (iface) define(window, "EmbeddedBrowserWebView", iface);
    try {
      return channel.generic(url, data);
    } finally {
      delete chrome.webview;
      delete window.EmbeddedBrowserWebView;
    }
  };
  define(window, key, { value: { generic }, configurable: true });
  const drop = new MutationObserver(() => {
    delete window[key];
    drop.disconnect();
  });
  drop.observe(document, { childList: true, subtree: true });
})
