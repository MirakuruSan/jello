// Jello overlay main entry point
// State management, hit-rect tracking, hover-fade, shortcut dispatch, Tauri event listeners

import { invoke, convertFileSrc } from "@tauri-apps/api/core";

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Tab, HitRect } from "./types";
import { TabPanelController } from "./tablist/panel";
import { PaletteController } from "./palette/controller";
import { ViewsController, type DownloadItem } from "./views/views";
import { Wizard, applyTheme } from "./views/wizard";
import { showContextMenu, showMenuAt, closeContextMenu, isMenuOpen, type MenuEntry } from "./contextMenu";
import * as icons from "./icons";

// ===== State =====
let tabs: Tab[] = [];
let activeTabId = -1;
let tabPanel: TabPanelController | null = null;
let findBarOpen = false;
let pinnedOnTop = false;
let topBarFadeTimer: ReturnType<typeof setTimeout> | null = null;

// ===== Hit-rect tracking =====
// Last rect set sent to the backend: the 250ms layout tick only crosses IPC
// (and re-applies the window region) when something actually moved.
let lastHitRectsKey = "";
function updateHitRects(): void {
  // The backend turns these rects into the overlay's window REGION — anything
  // not listed is click-through to the page AND not painted. So: skip faded
  // chrome (it must not eat clicks while invisible), and include transient
  // visuals (toasts, chord HUD) so they stay visible over content.
  // Open panels contribute their whole rect: the backend no longer forces a
  // full-window region while a panel is open (that ate every page click),
  // so the panel itself must claim its area to stay clickable.
  const elements = document.querySelectorAll<HTMLElement>(
    ".interactive, .region-visible, .tab-panel.open, .views-panel.open, .find-bar.open"
  );
  const rects: HitRect[] = [];
  for (const el of Array.from(elements)) {
    if (el.closest(".faded")) continue;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0) continue;
    rects.push({
      x: Math.round(r.left),
      y: Math.round(r.top),
      width: Math.round(r.width),
      height: Math.round(r.height),
    });
  }
  const key = JSON.stringify(rects);
  if (key === lastHitRectsKey) return;
  lastHitRectsKey = key;
  invoke("overlay_set_hit_rects", { rects }).catch(() => { lastHitRectsKey = ""; });
}

// ===== Hover-fade system =====
function resetTopBarFade(): void {
  const topBar = document.getElementById("top-bar");
  if (!topBar) return;
  topBar.classList.remove("faded");
  if (topBarFadeTimer) clearTimeout(topBarFadeTimer);
  topBarFadeTimer = setTimeout(() => {
    // Don't fade away the address bar while it's open/focused — it hid what the
    // user was typing. Reschedule instead so it fades once editing ends.
    if (document.getElementById("domain-pill")?.classList.contains("editing")) {
      resetTopBarFade();
      return;
    }
    topBar.classList.add("faded");
    updateHitRects();
  }, 2000);
}

function handleMouseMove(e: MouseEvent): void {
  if (e.clientY < 56) {
    resetTopBarFade();
  }
}

// Tell the backend whether any overlay panel is open, so the accelerator
// handler only swallows Esc while a panel is showing.
function syncPanelOpen(): void {
  const panelOpen = (tabPanel?.isOpen() ?? false) || (views?.isOpen() ?? false);
  invoke("overlay_set_panel_open", { open: panelOpen || findBarOpen }).catch(() => {});
}

// ===== Chrome-UI visibility toggle (Ctrl+Shift+U) =====
// Hides all floating chrome so page elements underneath the pills are
// reachable. Hit rects vanish with it, so the overlay becomes fully
// click-through until toggled back.
function toggleChromeUi(): void {
  const hidden = document.body.classList.toggle("chrome-hidden");
  if (hidden) {
    invoke("overlay_set_panel_open", { open: false }).catch(() => {});
  }
  updateHitRects();
  showToast(hidden ? "UI hidden — Ctrl+Shift+U to restore" : "UI restored");
}

// ===== Tab panel =====
// TabPanelController is the single authority over the panel; it calls back
// into syncPanelOpen/updateHitRects via its onOpenChanged hook.
function toggleTabPanel(): void {
  tabPanel?.toggle();
}

// ===== Find bar =====
function toggleFindBar(): void {
  findBarOpen = !findBarOpen;
  const bar = document.getElementById("find-bar");
  if (bar) {
    bar.classList.toggle("open", findBarOpen);
    if (findBarOpen) {
      const input = document.getElementById("find-input") as HTMLInputElement | null;
      input?.focus();
    }
  }
  syncPanelOpen();
  updateHitRects();
}

// ===== New-tab clock =====
function updateClock(): void {
  const el = document.getElementById("newtab-clock");
  if (!el) return;
  const now = new Date();
  el.textContent = now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
}

// ===== Toast =====
function showToast(message: string): void {
  const container = document.getElementById("toast-container");
  if (!container) return;
  const toast = document.createElement("div");
  toast.className = "toast glass";
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.remove();
  }, 3000);
}

// Web Store install banner. A glass bar with .interactive so clicks
// land (hit-rect region); dispatches overlay:layout so the region updates.
function showWebstoreBanner(url: string): void {
  document.getElementById("webstore-banner")?.remove();
  const banner = document.createElement("div");
  banner.id = "webstore-banner";
  banner.className = "webstore-banner glass interactive";
  const text = document.createElement("span");
  text.textContent = "Install this extension into Jello?";
  text.style.cssText = "flex:1;";
  const install = document.createElement("button");
  install.className = "views-btn interactive";
  install.textContent = "Install";
  const dismiss = document.createElement("button");
  dismiss.className = "views-btn interactive";
  dismiss.textContent = "Dismiss";
  const close = () => {
    banner.remove();
    window.dispatchEvent(new Event("overlay:layout"));
  };
  install.addEventListener("click", async () => {
    install.disabled = true;
    install.textContent = "Installing…";
    try {
      await invoke("extensions_install", { crxIdOrUrl: url });
      showToast("Extension installed — restart Jello to apply");
    } catch (e) {
      showToast("Extension install failed");
      console.error(e);
    }
    close();
  });
  dismiss.addEventListener("click", close);
  banner.append(text, install, dismiss);
  document.body.appendChild(banner);
  window.dispatchEvent(new Event("overlay:layout"));
  setTimeout(close, 15000); // auto-dismiss
}

// Keep the top-bar tab-count pill in sync with the in-memory tab list, so it's
// correct on launch instead of showing the hardcoded "1" until the panel opens.
function updateTabCountBadge(): void {
  const badge = document.getElementById("btn-tab-count");
  if (badge) badge.textContent = String(tabs.length);
}

function getWindowId(): number {
  const label = getCurrentWebviewWindow().label;
  if (label === "main") return 1;
  const match = label.match(/_(.+)$/);
  if (match) {
    const id = parseInt(match[1], 10);
    if (!isNaN(id)) return id;
  }
  return 1;
}

// Navigate from THIS window: pass our own active tab + window id so the backend
// targets the calling window, not the pool's single global active tab.
function navTo(input: string): void {
  invoke("nav_to", { input, tabId: activeTabId, windowId: getWindowId() }).catch(console.error);
}

// ===== Active-tab helpers =====
function activeTab(): Tab | undefined {
  return tabs.find((t) => t.id === activeTabId);
}
// Show the new-tab page (and its search pill) only for a blank tab. Must run on
// navigation too (tab:updated), not just tab switches — otherwise the search
// pill lingers over a loaded site after navigating away from about:blank.
function updateNewtabVisibility(tab: Tab | undefined): void {
  const newtab = document.getElementById("newtab-page");
  if (!newtab) return;
  const show = tab?.url === "about:blank" || !tab;
  const wasVisible = newtab.classList.contains("visible");
  newtab.classList.toggle("visible", show);
  // Auto-focus the search/URL pill when a blank tab appears (e.g. Ctrl+T) so
  // typing works immediately without a click.
  if (show && !wasVisible) {
    const s = document.getElementById("newtab-search") as HTMLInputElement | null;
    requestAnimationFrame(() => { s?.focus(); });
    renderTopSites();
  }
}

// Most-visited site tiles on the new-tab page (backend: top_sites).
function renderTopSites(): void {
  const grid = document.getElementById("top-sites");
  if (!grid) return;
  invoke<{ url: string; title: string }[]>("top_sites", { limit: 10 })
    .then((sites) => {
      grid.replaceChildren(
        ...sites.map((site) => {
          const tile = document.createElement("button");
          tile.className = "top-site interactive";
          tile.title = site.url;
          const icon = document.createElement("span");
          icon.className = "site-icon";
          icon.textContent = site.title.charAt(0).toUpperCase();
          const label = document.createElement("span");
          label.className = "site-label";
          label.textContent = site.title;
          tile.append(icon, label);
          tile.addEventListener("click", () => navTo(site.url));
          return tile;
        })
      );
    })
    .catch(() => {});
}
function activeHost(): string {
  try {
    return new URL(activeTab()?.url || "").hostname;
  } catch {
    return "";
  }
}
let views: ViewsController | null = null;
let zoomFactor = 1.0;
// Shows/hides the picture-in-picture pill for the active tab; wired in init.
let syncPipButton: () => void = () => {};
// Bookmark star: maps the current bookmark set (url -> id) so the star button
// can toggle and reflect whether the active page is bookmarked. Wired in init.
let bookmarkMap = new Map<string, number>();
let refreshBookmarkStar: () => void = () => {};
function applyZoom(f: number): void {
  zoomFactor = Math.min(5, Math.max(0.25, f));
  invoke("zoom_set", { factor: zoomFactor, host: activeHost() }).catch(console.error);
  showZoomHud(zoomFactor);
}

// Zoom chip in the address pill: visible only when the page isn't at 100%.
function updateZoomChip(): void {
  const chip = document.getElementById("btn-zoom-reset");
  if (!chip) return;
  const pct = Math.round(zoomFactor * 100);
  const hidden = Math.abs(zoomFactor - 1) < 0.01;
  if (chip.hidden === hidden && chip.textContent === `${pct}%`) return;
  chip.hidden = hidden;
  chip.textContent = `${pct}%`;
  updateHitRects();
}

// Transient zoom pill: shows e.g. "120%" for ~1.2s while adjusting.
let zoomHudTimer = 0;
function showZoomHud(f: number): void {
  updateZoomChip();
  let hud = document.getElementById("zoom-hud");
  if (!hud) {
    hud = document.createElement("div");
    hud.id = "zoom-hud";
    hud.className = "glass";
    hud.style.cssText =
      "position:fixed;bottom:18px;left:50%;transform:translateX(-50%);padding:6px 14px;" +
      "border-radius:999px;font-size:0.8rem;font-weight:600;z-index:99999;pointer-events:none;" +
      "transition:opacity 0.15s ease;";
    document.body.appendChild(hud);
  }
  hud.textContent = `${Math.round((f * 100) / 5) * 5}%`;
  hud.style.opacity = "1";
  clearTimeout(zoomHudTimer);
  zoomHudTimer = window.setTimeout(() => {
    const el = document.getElementById("zoom-hud");
    if (el) el.style.opacity = "0";
  }, 1200);
}

// Per-site zoom restore: when the active host changes, apply its saved
// zoom (or 100% if none). Guarded by lastZoomHost so same-host navigations and
// repeated tab:updated events don't re-apply.
let lastZoomHost = "";
async function maybeRestoreZoom(host: string): Promise<void> {
  if (!host || host === lastZoomHost) return;
  lastZoomHost = host;
  let saved: number | null = null;
  try {
    saved = await invoke<number | null>("zoom_get", { host });
  } catch {
    saved = null;
  }
  const f = typeof saved === "number" ? saved : 1.0;
  zoomFactor = f;
  updateZoomChip();
  // Apply to the active view. Only pass the host (which saves) when we actually
  // have a stored value, so we never pollute settings with 100% for every host.
  invoke("zoom_set", { factor: f, host: typeof saved === "number" ? host : "" }).catch(() => {});
}

// Update the domain pill: hostname text + a theme-aware padlock (closed/green
// for https, open/red for http, hidden for blank/internal pages).
function updateDomainPill(tab: Tab | undefined): void {
  const domainText = document.getElementById("domain-text");
  const lock = document.querySelector<HTMLElement>("#domain-pill .lock-icon");
  if (!domainText) return;
  refreshBookmarkStar();
  if (!tab || tab.url === "about:blank" || !tab.url) {
    domainText.textContent = "New Tab";
    if (lock) lock.innerHTML = "";
    return;
  }
  try {
    const url = new URL(tab.url);
    domainText.textContent = url.hostname || tab.title || "New Tab";
    if (lock) {
      if (url.protocol === "https:") {
        lock.innerHTML = icons.lockClosed;
        lock.className = "lock-icon secure";
      } else if (url.protocol === "http:") {
        lock.innerHTML = icons.lockOpen;
        lock.className = "lock-icon insecure";
      } else {
        lock.innerHTML = "";
        lock.className = "lock-icon";
      }
    }
  } catch {
    domainText.textContent = tab.title || "New Tab";
    if (lock) lock.innerHTML = "";
  }
}

// In-app address bar: reveal the inline input inside the domain pill, prefilled
// with the current URL. On Enter it navigates; Escape/blur restores the pill.
function openAddressBar(): void {
  const pill = document.getElementById("domain-pill");
  const input = document.getElementById("address-input") as HTMLInputElement | null;
  if (!pill || !input) return;
  input.value = activeTab()?.url || "";
  pill.classList.add("editing");
  resetTopBarFade(); // ensure the bar is un-faded and stays put while editing
  updateHitRects();
  requestAnimationFrame(() => {
    input.focus();
    input.select();
  });
}
function closeAddressBar(): void {
  const pill = document.getElementById("domain-pill");
  if (!pill) return;
  pill.classList.remove("editing");
  updateHitRects();
}

// Switch to the previous/next tab in the tab strip, wrapping around. Used by
// Alt+Left / Alt+Right (distinct from history back/forward on the browser keys).
function switchAdjacentTab(dir: -1 | 1): void {
  if (tabs.length < 2) return;
  const idx = tabs.findIndex((t) => t.id === activeTabId);
  const from = idx === -1 ? 0 : idx;
  const next = (from + dir + tabs.length) % tabs.length;
  const target = tabs[next];
  if (target) invoke("tabs_activate", { id: target.id }).catch(console.error);
}

// Map a keyboard event to a shortcut action string (mirrors the native
// WebView2 accelerator key map in webview2.rs) so the overlay/chrome can
// dispatch the same shortcuts the content webviews do. Uses e.code so it is
// layout- and case-independent.
function keyEventToAction(e: KeyboardEvent): string | null {
  const c = e.ctrlKey, s = e.shiftKey, a = e.altKey;
  const code = e.code;
  if (e.key === "BrowserBack") return "Nav+Back";
  if (e.key === "BrowserForward") return "Nav+Forward";
  if (a && !c && !s) {
    if (code === "ArrowLeft") return "Tab+Prev";
    if (code === "ArrowRight") return "Tab+Next";
    if (code === "KeyP") return "Alt+P";
    return null;
  }
  if (!c && !s && !a) {
    if (code === "F5") return "F5";
    if (code === "F6") return "F6";
    if (code === "Escape") return "Esc";
    return null;
  }
  if (c && !s && !a) {
    switch (code) {
      case "KeyT": return "Ctrl+T";
      case "KeyW": return "Ctrl+W";
      case "KeyN": return "Ctrl+N";
      case "KeyL": return "Ctrl+L";
      case "KeyF": return "Ctrl+F";
      case "KeyR": return "Ctrl+R";
      case "KeyJ": return "Ctrl+J";
      case "KeyD": return "Ctrl+D";
      case "KeyH": return "Ctrl+H";
      case "KeyM": return "Ctrl+M";
      case "KeyQ": return "Ctrl+Q";
      case "Tab": return "Ctrl+Tab";
      case "Equal": case "NumpadAdd": return "Ctrl+ZoomIn";
      case "Minus": case "NumpadSubtract": return "Ctrl+ZoomOut";
      case "Digit0": case "Numpad0": return "Ctrl+ZoomReset";
    }
    if (/^Digit[1-9]$/.test(code)) return "Ctrl+" + code.slice(5);
    return null;
  }
  if (c && s && !a) {
    switch (code) {
      case "KeyT": return "Ctrl+Shift+T";
      case "KeyW": return "Ctrl+Shift+W";
      case "KeyN": return "Ctrl+Shift+N";
      case "KeyE": return "Ctrl+Shift+E";
      case "KeyU": return "Ctrl+Shift+U";
      case "KeyR": return "Ctrl+Shift+R";
      case "KeyO": return "Ctrl+Shift+O";
      case "KeyC": return "Ctrl+Shift+C";
      case "KeyV": return "Ctrl+Shift+V";
      case "Tab": return "Ctrl+Shift+Tab";
    }
    return null;
  }
  return null;
}

// Add or remove the active page from bookmarks, based on current state.
async function toggleCurrentBookmark(): Promise<void> {
  const t = activeTab();
  if (!t || !t.url || t.url === "about:blank") {
    showToast("Nothing to bookmark");
    return;
  }
  if (bookmarkMap.has(t.url)) {
    await invoke("bookmarks_remove", { id: bookmarkMap.get(t.url) }).catch(() => {});
    bookmarkMap.delete(t.url);
    showToast("Bookmark removed");
  } else {
    await invoke("bookmark_current_tab", { url: t.url, title: t.title || t.url }).catch(console.error);
    showToast("Bookmarked");
  }
  refreshBookmarkStar();
}

// ===== Shortcut dispatch =====
function handleShortcut(action: string): void {
  resetTopBarFade();
  switch (action) {
    case "Ctrl+T":
      invoke("tabs_create", { windowId: getWindowId() }).catch(console.error);
      break;
    case "Ctrl+W":
      if (activeTabId !== -1) invoke("tabs_close", { id: activeTabId }).catch(console.error);
      break;
    case "Ctrl+Shift+W":
      // Unload (discard) the current tab and move to the next one.
      if (activeTabId !== -1) invoke("tabs_unload", { id: activeTabId }).catch(console.error);
      break;
    case "Ctrl+Shift+T":
      invoke("tabs_reopen_closed", { windowId: getWindowId() }).catch(console.error);
      break;
    case "Ctrl+N":
      invoke("window_new", {}).catch(console.error);
      break;
    case "Ctrl+Shift+N":
      invoke("window_new_incognito", {}).catch(console.error);
      break;
    case "Ctrl+Shift+E":
      toggleTabPanel();
      break;
    case "Ctrl+Shift+U":
      toggleChromeUi();
      break;
    case "Ctrl+F":
      toggleFindBar();
      break;
    case "Ctrl+R":
    case "F5":
      invoke("nav_reload", {}).catch(console.error);
      break;
    case "Ctrl+Shift+R":
      invoke("nav_reload", {}).catch(console.error);
      break;
    case "Ctrl+L":
    case "F6":
      // In-app address bar (no detached window), like any other browser.
      openAddressBar();
      break;
    case "Ctrl+D":
      // Toggle (add/remove) so the star and Ctrl+D agree; reload the set so the
      // star reflects the new state.
      toggleCurrentBookmark().then(() => {
        invoke<{ id: number; url: string }[]>("bookmarks_list")
          .then((marks) => { bookmarkMap = new Map(marks.map((m) => [m.url, m.id])); refreshBookmarkStar(); })
          .catch(() => {});
      });
      break;
    case "Ctrl+Shift+C": {
      const t = activeTab();
      if (t?.url) {
        navigator.clipboard.writeText(t.url).then(() => showToast("URL copied")).catch(() => {});
      }
      break;
    }
    case "Ctrl+Shift+V":
      navigator.clipboard.readText().then((text) => {
        if (text.trim()) navTo(text.trim());
      }).catch(() => {});
      break;
    case "Ctrl+ZoomIn":
      applyZoom(zoomFactor + 0.1);
      break;
    case "Ctrl+ZoomOut":
      applyZoom(zoomFactor - 0.1);
      break;
    case "Ctrl+ZoomReset":
      applyZoom(1.0);
      break;
    case "Ctrl+Tab":
      invoke("tabs_mru_switch", { forward: true }).catch(console.error);
      break;
    case "Ctrl+Shift+Tab":
      invoke("tabs_mru_switch", { forward: false }).catch(console.error);
      break;
    case "Ctrl+H":
      views?.open("history");
      break;
    case "Ctrl+J":
      views?.open("downloads");
      break;
    case "Ctrl+Shift+O":
      views?.open("bookmarks");
      break;
    case "Nav+Back":
      invoke("nav_back", {}).catch(console.error);
      break;
    case "Nav+Forward":
      invoke("nav_forward", {}).catch(console.error);
      break;
    case "Tab+Prev":
      switchAdjacentTab(-1);
      break;
    case "Tab+Next":
      switchAdjacentTab(1);
      break;
    case "Ctrl+M":
      if (activeTabId > 0) {
        const tab = activeTab();
        if (tab) invoke("tabs_set_muted", { id: activeTabId, muted: !tab.muted }).catch(console.error);
      }
      break;
    case "Ctrl+Q":
      invoke("window_controls", { action: "close" }).catch(console.error);
      break;
    case "Alt+P":
      invoke("pip_toggle").catch(() => showToast("No page to float"));
      break;
    case "Esc":
      if (findBarOpen) toggleFindBar();
      else if (tabPanel?.isOpen()) tabPanel.hide();
      else if (views?.isOpen()) views.close();
      break;
    default:
      // Handle Ctrl+1..9 (activate nth / last visible tab)
      if (/^Ctrl\+[1-9]$/.test(action)) {
        const n = parseInt(action[5], 10);
        const targetTab = n === 9 ? tabs[tabs.length - 1] : tabs[n - 1];
        if (targetTab) invoke("tabs_activate", { id: targetTab.id }).catch(console.error);
      }
      break;
  }
}

// ===== Capture Overlay Controller =====
function initCaptureMode(params: URLSearchParams): void {
  document.body.classList.add("capture-mode");

  const imagePath = params.get("image_path") || "";
  const scale = parseFloat(params.get("scale") || "1");
  const mode = params.get("mode") || "screenshot";

  // Set frozen screenshot as background
  const bgEl = document.getElementById("capture-bg");
  if (bgEl) {
    const fileUrl = convertFileSrc(imagePath);
    bgEl.style.backgroundImage = `url("${fileUrl}")`;
  }

  const selEl = document.getElementById("capture-selection") as HTMLDivElement | null;
  const badgeEl = document.getElementById("capture-badge");
  if (!selEl) return;

  let dragging = false;
  let startX = 0;
  let startY = 0;
  let selX = 0;
  let selY = 0;
  let selW = 0;
  let selH = 0;

  function updateSelectionRect(): void {
    if (!selEl) return;
    selEl.style.left = selX + "px";
    selEl.style.top = selY + "px";
    selEl.style.width = selW + "px";
    selEl.style.height = selH + "px";
    if (badgeEl) {
      const physW = Math.round(selW * scale);
      const physH = Math.round(selH * scale);
      badgeEl.textContent = `${physW} × ${physH}`;
    }
  }

  // Clicks on the action bar / OCR popover must reach the buttons — they must
  // never restart the selection or dismiss the panel they live in.
  function insidePanel(e: MouseEvent): boolean {
    const t = e.target as HTMLElement | null;
    return !!t?.closest("#capture-actionbar, #capture-ocr-popover");
  }

  document.addEventListener("mousedown", (e: MouseEvent) => {
    if (insidePanel(e)) return;
    dragging = true;
    startX = e.clientX;
    startY = e.clientY;
    selX = startX;
    selY = startY;
    selW = 0;
    selH = 0;
    selEl.classList.add("active");
    updateSelectionRect();
  });

  document.addEventListener("mousemove", (e: MouseEvent) => {
    if (!dragging) return;
    const cx = e.clientX;
    const cy = e.clientY;
    selX = Math.min(startX, cx);
    selY = Math.min(startY, cy);
    selW = Math.abs(cx - startX);
    selH = Math.abs(cy - startY);
    updateSelectionRect();
  });

  const isOcr = mode === "ocr";
  const actionbar = document.getElementById("capture-actionbar");
  const popover = document.getElementById("capture-ocr-popover");
  const ocrTextEl = document.getElementById("capture-ocr-text") as HTMLTextAreaElement | null;
  let lastCropPath = "";

  // Physical (image) coords of the current selection.
  function physCoords() {
    return {
      x: Math.round(selX * scale),
      y: Math.round(selY * scale),
      width: Math.round(selW * scale),
      height: Math.round(selH * scale),
    };
  }

  // Anchor a panel just below-right of the selection, clamped to the viewport.
  function anchorPanel(el: HTMLElement): void {
    const px = Math.min(selX, window.innerWidth - el.offsetWidth - 8);
    const py = Math.min(selY + selH + 8, window.innerHeight - el.offsetHeight - 8);
    el.style.left = Math.max(8, px) + "px";
    el.style.top = Math.max(8, py) + "px";
  }

  function runScreenshotAction(action: string): void {
    const p = physCoords();
    invoke("capture_crop", {
      sourcePath: imagePath, x: p.x, y: p.y, width: p.width, height: p.height,
      action, isOcr: false,
    }).catch(console.error);
  }

  async function runOcr(): Promise<void> {
    const p = physCoords();
    try {
      const res = await invoke<{ ocrText: string; imagePath: string }>("capture_crop", {
        sourcePath: imagePath, x: p.x, y: p.y, width: p.width, height: p.height,
        action: "ocr", isOcr: true,
      });
      lastCropPath = res.imagePath;
      if (ocrTextEl) ocrTextEl.value = res.ocrText || "";
      if (popover) {
        popover.classList.add("visible");
        anchorPanel(popover);
        ocrTextEl?.focus();
      }
    } catch (e) {
      console.error(e);
      invoke("capture_cancel", {}).catch(console.error);
    }
  }

  function showActionBar(): void {
    if (!isOcr) {
      if (actionbar) { actionbar.classList.add("visible"); anchorPanel(actionbar); }
    } else {
      runOcr();
    }
  }

  // Screenshot action bar buttons.
  actionbar?.querySelectorAll<HTMLButtonElement>(".capture-action-btn").forEach((btn) => {
    btn.addEventListener("click", () => runScreenshotAction(btn.dataset.action || "copy"));
  });

  // OCR popover buttons.
  popover?.querySelectorAll<HTMLButtonElement>(".capture-action-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      invoke("capture_ocr_action", {
        text: ocrTextEl?.value || "",
        action: btn.dataset.ocrAction || "copy",
        imagePath: lastCropPath,
      }).catch(console.error);
    });
  });

  document.addEventListener("mouseup", () => {
    if (!dragging) return;
    dragging = false;
    if (selW > 2 && selH > 2) showActionBar();
  });

  // A fresh mousedown outside any open panel restarts selection.
  document.addEventListener("mousedown", (e: MouseEvent) => {
    if (insidePanel(e)) return;
    actionbar?.classList.remove("visible");
    popover?.classList.remove("visible");
  });

  document.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      invoke("capture_cancel", {}).catch(console.error);
      return;
    }

    if (e.key === "Enter" && selW > 2 && selH > 2) {
      // Enter = first action (Copy for screenshot, run OCR for OCR mode).
      if (isOcr) runOcr();
      else runScreenshotAction("copy");
      e.preventDefault();
      return;
    }

    const step = e.shiftKey ? 10 : 1;
    switch (e.key) {
      case "ArrowLeft":
        selX = Math.max(0, selX - step);
        updateSelectionRect();
        e.preventDefault();
        break;
      case "ArrowRight":
        selX += step;
        updateSelectionRect();
        e.preventDefault();
        break;
      case "ArrowUp":
        selY = Math.max(0, selY - step);
        updateSelectionRect();
        e.preventDefault();
        break;
      case "ArrowDown":
        selY += step;
        updateSelectionRect();
        e.preventDefault();
        break;
    }
  });
}

// ===== Pin Image Viewer =====
function initPinMode(params: URLSearchParams): void {
  document.body.classList.add("pin-mode");

  const imagePath = params.get("pin_image") || "";
  const imgEl = document.getElementById("pin-image") as HTMLImageElement | null;
  if (imgEl) {
    const fileUrl = convertFileSrc(imagePath);
    imgEl.src = fileUrl;
  }

  // Close on Esc or close button
  const closeBtn = document.getElementById("pin-close-btn");
  closeBtn?.addEventListener("click", () => {
    getCurrentWebviewWindow().close();
  });

  document.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      getCurrentWebviewWindow().close();
    }
  });
}

// Drawn icons for the icon-only chrome buttons (the markup leaves them empty).
const CHROME_ICONS: Record<string, string> = {
  "btn-back": icons.back,
  "btn-forward": icons.forward,
  "btn-reload": icons.reload,
  "btn-pip": icons.pip,
  "btn-new-tab": icons.plus,
  "btn-menu": icons.more,
  "btn-min": icons.minimize,
  "btn-max": icons.maximize,
  "btn-close": icons.close,
  "btn-close-panel": icons.close,
  "find-prev": icons.chevronUp,
  "find-next": icons.chevronDown,
  "find-close": icons.close,
  "views-close": icons.close,
  "pin-close-btn": icons.close,
};
// Reload turns into Stop while the active page loads.
let pageLoading = false;
function setPageLoading(loading: boolean): void {
  pageLoading = loading;
  const btn = document.getElementById("btn-reload");
  if (!btn) return;
  btn.innerHTML = loading ? icons.close : icons.reload;
  btn.title = loading ? "Stop loading (Esc)" : "Reload (Ctrl+R)";
  btn.setAttribute("aria-label", btn.title);
}

function setPinnedOnTop(pinned: boolean): void {
  pinnedOnTop = pinned;
  invoke("window_set_pinned", { pinned }).catch(console.error);
  showToast(pinned ? "Window stays on top" : "Window no longer stays on top");
}

// Inline zoom stepper row for the main menu: − 110% +, without closing it.
function zoomMenuRow(): HTMLElement {
  const row = document.createElement("div");
  row.className = "menu-zoom-row";
  const icon = document.createElement("span");
  icon.className = "menu-icon";
  icon.innerHTML = icons.search;
  const label = document.createElement("span");
  label.className = "menu-label";
  label.textContent = "Zoom";
  const value = document.createElement("button");
  value.className = "menu-zoom-value";
  value.title = "Reset to 100% (Ctrl+0)";
  const render = () => { value.textContent = `${Math.round(zoomFactor * 100)}%`; };
  const step = (svg: string, title: string, delta: number) => {
    const b = document.createElement("button");
    b.className = "menu-zoom-btn";
    b.title = title;
    b.setAttribute("aria-label", title);
    b.innerHTML = svg;
    b.addEventListener("click", (e) => { e.stopPropagation(); applyZoom(zoomFactor + delta); render(); });
    return b;
  };
  value.addEventListener("click", (e) => { e.stopPropagation(); applyZoom(1.0); render(); });
  render();
  row.append(icon, label, step(icons.minimize, "Zoom out (Ctrl+−)", -0.1), value, step(icons.plus, "Zoom in (Ctrl+=)", 0.1));
  return row;
}

function showMainMenu(anchor: HTMLElement): void {
  const hasPage = !!activeTab() && activeTab()?.url !== "about:blank";
  const entries: MenuEntry[] = [
    { label: "New tab", icon: icons.plus, shortcut: "Ctrl+T", onClick: () => handleShortcut("Ctrl+T") },
    { label: "Reopen closed tab", icon: icons.reopen, shortcut: "Ctrl+Shift+T", onClick: () => handleShortcut("Ctrl+Shift+T") },
    "separator",
    { label: "History", icon: icons.history, shortcut: "Ctrl+H", onClick: () => views?.open("history") },
    { label: "Bookmarks", icon: icons.bookmark, shortcut: "Ctrl+Shift+O", onClick: () => views?.open("bookmarks") },
    { label: "Downloads", icon: icons.download, shortcut: "Ctrl+J", onClick: () => views?.open("downloads") },
    "separator",
    { label: "Find in page", icon: icons.search, shortcut: "Ctrl+F", disabled: !hasPage, onClick: () => toggleFindBar() },
    { render: zoomMenuRow },
    { label: "Picture-in-picture", icon: icons.pip, shortcut: "Alt+P", disabled: !hasPage, onClick: () => handleShortcut("Alt+P") },
    { label: "Take screenshot", icon: icons.camera, shortcut: "Ctrl+Alt+S", onClick: () => invoke("capture_trigger", { mode: "screenshot" }).catch(console.error) },
    "separator",
    { label: "Keep window on top", icon: icons.pin, checked: pinnedOnTop, onClick: () => setPinnedOnTop(!pinnedOnTop) },
    { label: "Settings", icon: icons.settings, onClick: () => views?.open("settings") },
  ];
  showMenuAt(anchor, entries);
}

function applyChromeIcons(): void {
  for (const [id, svg] of Object.entries(CHROME_ICONS)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.innerHTML = svg;
    if (el.title) el.setAttribute("aria-label", el.title);
  }
}

// Theme + window material, for every window kind (main, palette, capture).
// The material class tells CSS whether a Windows backdrop shows through.
function applyAppearance(s: Record<string, unknown>): void {
  if (typeof s.theme === "string") applyTheme(s.theme);
  const material = typeof s.windowMaterial === "string" ? s.windowMaterial : "acrylic";
  for (const m of ["acrylic", "mica", "solid"]) {
    document.body.classList.toggle(`material-${m}`, m === material);
  }
}

// ===== Init =====
window.addEventListener("DOMContentLoaded", () => {
  applyChromeIcons();
  invoke<Record<string, unknown>>("settings_get").then(applyAppearance).catch(() => {});
  listen<Record<string, unknown>>("appearance:changed", (e) => applyAppearance(e.payload));
  const currentLabel = getCurrentWebviewWindow().label;
  const params = new URLSearchParams(window.location.search);

  if (currentLabel === "palette") {
    document.body.classList.add("palette-mode");
    new PaletteController();
    return;
  }

  // Capture overlay mode: window label starts with "capture_"
  if (currentLabel.startsWith("capture_")) {
    initCaptureMode(params);
    return;
  }

  // Pin image mode: window label starts with "pin_"
  if (currentLabel.startsWith("pin_") || params.has("pin_image")) {
    initPinMode(params);
    return;
  }

  if (currentLabel.startsWith("incognito_")) {
    document.body.classList.add("incognito-window");
  }

  // Initialize Tab Panel Controller (single authority over the tab panel)
  tabPanel = new TabPanelController(() => {
    syncPanelOpen();
    updateHitRects();
  });

  // Views panel (history / bookmarks / downloads / settings)
  views = new ViewsController(
    (url) => navTo(url),
    () => {
      syncPanelOpen();
      updateHitRects();
    }
  );

  const winId = getWindowId();

  // First-run wizard on the main window (theme/material are applied above).
  if (winId === 1) {
    new Wizard().maybeShow();
  }

  // Load initial tabs for main state. With "Restore tabs on startup" enabled,
  // also activate the most-recent tab so the user resumes where they left off.
  // Tabs persist in the DB, so the list is rarely empty at launch.
  invoke<Tab[]>("tabs_list", { windowId: winId }).then((result) => {
    tabs = result;
    updateTabCountBadge();
    if (winId !== 1) return;
    invoke<Record<string, unknown>>("settings_get").then(async (s) => {
      if (!s.restoreSession) return;
      if (tabs.length === 0) {
        const restored = await invoke<Tab[]>("session_restore_last").catch(() => [] as Tab[]);
        tabs = restored;
        updateTabCountBadge();
      }
      const candidates = tabs.filter((t) => t.url && t.url !== "about:blank");
      const recent = [...candidates].sort((a, b) => (b.lastActive || 0) - (a.lastActive || 0))[0];
      if (recent) invoke("tabs_activate", { id: recent.id }).catch(console.error);
    }).catch(() => {});
  }).catch(console.error);

  // ===== Button wiring =====
  document.getElementById("btn-back")?.addEventListener("click", () => invoke("nav_back", {}).catch(console.error));
  document.getElementById("btn-forward")?.addEventListener("click", () => invoke("nav_forward", {}).catch(console.error));
  document.getElementById("btn-reload")?.addEventListener("click", () =>
    invoke(pageLoading ? "nav_stop" : "nav_reload", {}).catch(console.error));
  document.getElementById("btn-min")?.addEventListener("click", () => invoke("window_controls", { action: "min" }).catch(console.error));
  document.getElementById("btn-max")?.addEventListener("click", () => invoke("window_controls", { action: "max" }).catch(console.error));
  document.getElementById("btn-close")?.addEventListener("click", () => invoke("window_controls", { action: "close" }).catch(console.error));
  const menuBtn = document.getElementById("btn-menu");
  menuBtn?.addEventListener("click", () => {
    if (isMenuOpen()) closeContextMenu();
    else if (menuBtn) showMainMenu(menuBtn);
  });
  document.getElementById("btn-zoom-reset")?.addEventListener("click", () => applyZoom(1.0));
  // '+' new-tab button.
  document.getElementById("btn-new-tab")?.addEventListener("click", () =>
    invoke("tabs_create", { windowId: getWindowId() }).catch(console.error));

  // Bookmark star: toggles the current page in/out of bookmarks, and reflects
  // whether the active page is already bookmarked (★ = saved, ☆ = not).
  const bookmarkBtn = document.getElementById("btn-bookmark");
  if (bookmarkBtn) {
    const loadBookmarks = async () => {
      const marks = await invoke<{ id: number; url: string }[]>("bookmarks_list").catch(() => []);
      bookmarkMap = new Map(marks.map((m) => [m.url, m.id]));
      refreshBookmarkStar();
    };
    refreshBookmarkStar = () => {
      const t = activeTab();
      const marked = !!(t && t.url && bookmarkMap.has(t.url));
      bookmarkBtn.innerHTML = marked ? icons.bookmarkFilled : icons.bookmark;
      bookmarkBtn.classList.toggle("pinned", marked);
      bookmarkBtn.setAttribute("aria-pressed", String(marked));
      bookmarkBtn.setAttribute("title", marked ? "Remove bookmark (Ctrl+D)" : "Bookmark this page (Ctrl+D)");
    };
    bookmarkBtn.addEventListener("click", async () => {
      await toggleCurrentBookmark();
      await loadBookmarks();
    });
    loadBookmarks();
  }

  // Keep-on-top lives in the main menu; reflect the persisted state.
  invoke<boolean>("window_pinned_state").then((p) => { pinnedOnTop = p; }).catch(() => {});

  // New-tab search: a real input — Enter navigates/searches directly.
  const newtabSearch = document.getElementById("newtab-search") as HTMLInputElement | null;
  newtabSearch?.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      const text = newtabSearch.value.trim();
      if (text) {
        navTo(text);
        newtabSearch.value = "";
      }
    }
  });

  // Domain pill opens the in-app address bar (matches its Ctrl+L tooltip).
  document.getElementById("domain-pill")?.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest(".page-actions")) return;
    openAddressBar();
  });
  // Right-click the domain pill → Copy URL / Paste and go.
  document.getElementById("domain-pill")?.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const tab = activeTab();
    const url = tab?.url ?? "";
    showContextMenu(e.clientX, e.clientY, [
      {
        label: "Copy URL",
        disabled: !url || url === "about:blank",
        onClick: () => { navigator.clipboard.writeText(url).catch(() => {}); },
      },
      {
        label: "Paste and go",
        onClick: () => {
          navigator.clipboard.readText()
            .then((text) => { if (text.trim()) navTo(text.trim()); })
            .catch(() => {});
        },
      },
    ]);
  });
  const addressInput = document.getElementById("address-input") as HTMLInputElement | null;
  addressInput?.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      const val = addressInput.value.trim();
      closeAddressBar();
      if (val) navTo(val);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeAddressBar();
    }
  });
  addressInput?.addEventListener("blur", () => closeAddressBar());

  // Find bar buttons
  document.getElementById("find-prev")?.addEventListener("click", () => {
    const input = document.getElementById("find-input") as HTMLInputElement | null;
    if (input?.value) invoke("find_in_page", { text: input.value, forward: false }).catch(console.error);
  });
  document.getElementById("find-next")?.addEventListener("click", () => {
    const input = document.getElementById("find-input") as HTMLInputElement | null;
    if (input?.value) invoke("find_in_page", { text: input.value, forward: true }).catch(console.error);
  });
  document.getElementById("find-close")?.addEventListener("click", () => toggleFindBar());
  // Enter = find next, Shift+Enter = find previous, Escape = close.
  document.getElementById("find-input")?.addEventListener("keydown", (e) => {
    const ev = e as KeyboardEvent;
    const input = e.target as HTMLInputElement;
    if (ev.key === "Enter") {
      ev.preventDefault();
      if (input.value) invoke("find_in_page", { text: input.value, forward: !ev.shiftKey }).catch(console.error);
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      if (findBarOpen) toggleFindBar();
    }
  });

  // ===== Hit-rect tracking =====
  updateHitRects();
  window.addEventListener("resize", updateHitRects);
  // Context menus dispatch this when they open/close so their rect enters/leaves
  // the overlay region immediately (not on the 250ms tick).
  window.addEventListener("overlay:layout", updateHitRects);
  setInterval(updateHitRects, 250);

  // Overlay-side keyboard shortcuts. The native WebView2 accelerator handler
  // only fires when a CONTENT webview has focus; when the chrome/overlay has
  // focus (new-tab page, after clicking chrome UI, address bar) those keys never
  // reach the backend, so the same shortcuts would silently stop working. Map
  // the full set here too so shortcuts work regardless of which surface is
  // focused (fixes "hotkeys work then stop then start again").
  document.addEventListener("keydown", (e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    const typing = !!(t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable));
    const action = keyEventToAction(e);
    if (!action) return;
    // While typing in a field, let the field own text-editing keys and Esc.
    if (typing && action === "Esc") return;
    e.preventDefault();
    handleShortcut(action);
  });

  // ===== Hover-fade =====
  document.addEventListener("mousemove", handleMouseMove);

  // Ctrl+scroll zoom when the overlay itself has focus (content webviews zoom
  // natively via IsZoomControlEnabled + the zoom:changed listener above).
  document.addEventListener("wheel", (e: WheelEvent) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    applyZoom(zoomFactor + (e.deltaY < 0 ? 0.1 : -0.1));
  }, { passive: false });
  resetTopBarFade();

  // ===== Clock + top sites (the new-tab page starts out visible) =====
  updateClock();
  setInterval(updateClock, 1000);
  renderTopSites();

  // ===== Tauri events =====
  listen<Tab>("tab:created", (event) => {
    if (event.payload.windowId === winId) {
      tabs.push(event.payload);
      updateTabCountBadge();
    }
  });

  listen<Tab>("tab:updated", (event) => {
    if (event.payload.windowId === winId) {
      const idx = tabs.findIndex((t) => t.id === event.payload.id);
      if (idx >= 0) tabs[idx] = event.payload;
      if (event.payload.id === activeTabId) {
        updateDomainPill(event.payload);
        updateNewtabVisibility(event.payload);
        maybeRestoreZoom(activeHost());
      }
    }
  });

  // Backend emits BARE ids for these (tabs.rs: app.emit("tab:closed", id)).
  listen<number>("tab:closed", (event) => {
    tabs = tabs.filter((t) => t.id !== event.payload);
    updateTabCountBadge();
  });

  listen<number>("tab:activated", (event) => {
    activeTabId = event.payload;
    const tab = tabs.find((t) => t.id === activeTabId);
    setPageLoading(false);
    updateDomainPill(tab);
    updateNewtabVisibility(tab);
    syncPipButton();
    // Force a re-check on activation even if the host string matches the last
    // one applied (switching tabs can land on the same host at a different zoom).
    lastZoomHost = "";
    maybeRestoreZoom(activeHost());
  });

  listen<string>("window:shortcut", (event) => {
    handleShortcut(event.payload);
  });

  listen<string>("toast:show", (event) => {
    showToast(event.payload);
  });

  // Native Ctrl+scroll zoom in the page fires this; keep the overlay's zoom
  // state in sync and persist the per-host factor.
  listen<number>("zoom:changed", (event) => {
    zoomFactor = event.payload;
    const host = activeHost();
    if (host) invoke("zoom_set", { factor: zoomFactor, host }).catch(() => {});
    showZoomHud(zoomFactor);
  });

  listen<string>("window:open-view", (event) => {
    views?.open(event.payload as any);
  });

  // Top loading bar driven by NavigationStarting/Completed.
  listen<{ tabId: number; loading: boolean }>("nav:loading", (event) => {
    if (event.payload.tabId !== activeTabId) return;
    setPageLoading(event.payload.loading);
    const bar = document.getElementById("nav-progress");
    if (!bar) return;
    if (event.payload.loading) {
      bar.classList.remove("done");
      bar.classList.add("loading");
    } else {
      bar.classList.remove("loading");
      bar.classList.add("done");
      setTimeout(() => bar.classList.remove("done"), 400);
    }
    updateHitRects();
  });

  // Chrome Web Store intercept banner: the store's "Add to Chrome"
  // can't work in WebView2, so offer Jello's working sideload path.
  let webstoreBannerUrl = "";
  listen<string>("webstore:detected", (event) => {
    const url = event.payload;
    if (url === webstoreBannerUrl) return; // avoid re-showing on repeat events
    webstoreBannerUrl = url;
    showWebstoreBanner(url);
  });

  // Downloads: the panel list (Ctrl+J) plus a top-bar pill with overall
  // progress while anything is unfinished.
  const unfinished = new Map<string, { name: string; received: number; total: number }>();
  const dlBtn = document.getElementById("btn-downloads");
  const renderDlBtn = () => {
    if (!dlBtn) return;
    dlBtn.hidden = unfinished.size === 0;
    let rec = 0, tot = 0;
    for (const d of unfinished.values()) { rec += d.received; tot += d.total; }
    const label = document.createElement("span");
    label.className = "dl-label";
    label.textContent = tot > 0 ? `${Math.floor((rec / tot) * 100)}%` : String(unfinished.size);
    dlBtn.innerHTML = icons.download;
    dlBtn.append(label);
    updateHitRects();
  };
  dlBtn?.addEventListener("click", () => views?.open("downloads"));

  listen<DownloadItem>("download:started", (event) => {
    const d = event.payload;
    views?.addDownload(d);
    unfinished.set(d.id, { name: d.fileName, received: 0, total: d.total ?? 0 });
    renderDlBtn();
    showToast(`Downloading ${d.fileName}`);
  });

  listen<{ id: string; received: number; total: number }>("download:progress", (event) => {
    const { id, received, total } = event.payload;
    views?.updateDownload(id, { received, total });
    const d = unfinished.get(id);
    if (d) { d.received = received; d.total = total; renderDlBtn(); }
  });

  // Pause and resumable errors keep a download listed as unfinished.
  listen<{ id: string; state: string; path: string; reason: string }>("download:state", (event) => {
    const { id, state, path, reason } = event.payload;
    views?.updateDownload(id, { state, path, reason });
    const name = unfinished.get(id)?.name ?? "file";
    if (state === "completed" || state === "cancelled" || state === "failed") {
      unfinished.delete(id);
      renderDlBtn();
    }
    if (state === "completed") showToast(`Downloaded ${name}`);
    else if (state === "failed") showToast(`Download failed: ${name}`);
    else if (state === "interrupted") showToast(`Download interrupted: ${name} — resume it from Downloads`);
  });

  // Picture-in-picture pill: shown while the active tab is playing media.
  const playingTabs = new Set<number>();
  const pipBtn = document.getElementById("btn-pip");
  syncPipButton = () => {
    if (!pipBtn) return;
    const show = playingTabs.has(activeTabId);
    if (pipBtn.hidden === !show) return;
    pipBtn.hidden = !show;
    updateHitRects();
  };
  pipBtn?.addEventListener("click", () => handleShortcut("Alt+P"));
  listen<{ tabId: number; playing: boolean }>("tab:audio", (event) => {
    if (event.payload.playing) playingTabs.add(event.payload.tabId);
    else playingTabs.delete(event.payload.tabId);
    syncPipButton();
  });

  // Listen to chord HUD event from Rust
  listen<{ keys: string; matchingSlots: any[] }>("hotkey:chord-hud", (event) => {
    const hud = document.getElementById("chord-hud");
    const keysEl = document.getElementById("chord-hud-keys");
    const slotsEl = document.getElementById("chord-hud-slots");
    if (!hud || !keysEl || !slotsEl) return;

    const { keys, matchingSlots } = event.payload;
    if (!keys && matchingSlots.length === 0) {
      hud.classList.remove("visible");
      return;
    }

    hud.classList.add("visible");
    keysEl.textContent = keys || "Press keys...";

    slotsEl.innerHTML = "";
    matchingSlots.forEach((slot) => {
      const row = document.createElement("div");
      row.className = "chord-hud-slot-row";

      const seqSpan = document.createElement("span");
      seqSpan.className = "chord-hud-slot-seq";
      seqSpan.textContent = slot.sequence;

      const titleSpan = document.createElement("span");
      titleSpan.className = "chord-hud-slot-title";
      titleSpan.textContent = slot.title || slot.targetUrl;

      row.appendChild(seqSpan);
      row.appendChild(titleSpan);
      slotsEl.appendChild(row);
    });
  });

  invoke("process_startup_arg").catch(console.error);

  // Show window if not started minimized
  invoke<boolean>("should_show_on_startup")
    .then((show) => {
      if (show) {
        getCurrentWebviewWindow().show().catch(console.error);
      }
    })
    .catch((err) => {
      console.error("Failed to check if should show on startup:", err);
      getCurrentWebviewWindow().show().catch(console.error);
    });
});
