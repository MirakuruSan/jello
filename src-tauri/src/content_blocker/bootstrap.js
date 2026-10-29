// Runs first in every document of a content tab, before the page's own
// scripts. It takes the host channel (see channel.rs) out of
// `chrome.webview.hostObjects` and leaves it under the per-session key for
// Jello's filtering script, which runs next and removes it. Whatever is left
// is removed before the first page script runs: the parser performs a
// microtask checkpoint, delivering mutation records, before it executes a
// script element.
(function (key) {
  if (location.protocol !== "https:" && location.protocol !== "http:") return;
  let channel;
  try {
    channel = window.chrome.webview.hostObjects[key];
  } catch (e) {
    return;
  }
  if (!channel) return;
  Object.defineProperty(window, key, { value: channel, configurable: true });
  const drop = new MutationObserver(() => {
    delete window[key];
    drop.disconnect();
  });
  drop.observe(document, { childList: true, subtree: true });
})
