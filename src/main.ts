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
import { showContextMenu } from "./contextMenu";
import * as icons from "./icons";

// ===== State =====
let tabs: Tab[] = [];
let activeTabId = -1;
let tabPanel: TabPanelController | null = null;
let findBarOpen = false;
let pinnedOnTop = false;
let topBarFadeTimer: ReturnType<typeof setTimeout> | null = null;

// ===== Hit-rect tracking =====
function updateHitRects(): void {
  // The backend turns these rects into the overlay's window REGION — anything
  // not listed is click-through to the page AND not painted. So: skip faded
  // chrome (it must not eat clicks while invisible), and include transient
  // visuals (toasts, chord HUD) so they stay visible over content.
  const elements = document.querySelectorAll<HTMLElement>(".interactive, .region-visible");
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
  invoke("overlay_set_hit_rects", { rects }).catch(() => {});
}

// ===== Hover-fade system =====
function resetTopBarFade(): void {
  const topBar = document.getElementById("top-bar");
  if (!topBar) return;
  topBar.classList.remove("faded");
  if (topBarFadeTimer) clearTimeout(topBarFadeTimer);
  topBarFadeTimer = setTimeout(() => {
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
// handler only swallows Esc while a panel is showing (M5R.8).
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

// Web Store install banner (Phase 4). A glass bar with .interactive so clicks
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

// ===== Active-tab helpers =====
function activeTab(): Tab | undefined {
  return tabs.find((t) => t.id === activeTabId);
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
function applyZoom(f: number): void {
  zoomFactor = Math.min(5, Math.max(0.25, f));
  invoke("zoom_set", { factor: zoomFactor, host: activeHost() }).catch(console.error);
}

// Update the domain pill: hostname text + a theme-aware padlock (closed/green
// for https, open/red for http, hidden for blank/internal pages).
function updateDomainPill(tab: Tab | undefined): void {
  const domainText = document.getElementById("domain-text");
  const lock = document.querySelector<HTMLElement>("#domain-pill .lock-icon");
  if (!domainText) return;
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

// ===== Shortcut dispatch =====
function handleShortcut(action: string): void {
  resetTopBarFade();
  switch (action) {
    case "Ctrl+T":
      invoke("tabs_create", { windowId: getWindowId() }).catch(console.error);
      break;
    case "Ctrl+W":
      if (activeTabId > 0) invoke("tabs_close", { id: activeTabId }).catch(console.error);
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
    case "Ctrl+D": {
      const t = activeTab();
      if (t) {
        invoke("bookmark_current_tab", { url: t.url, title: t.title || t.url })
          .then(() => showToast("Bookmarked"))
          .catch(console.error);
      }
      break;
    }
    case "Ctrl+Shift+C": {
      const t = activeTab();
      if (t?.url) {
        navigator.clipboard.writeText(t.url).then(() => showToast("URL copied")).catch(() => {});
      }
      break;
    }
    case "Ctrl+Shift+V":
      navigator.clipboard.readText().then((text) => {
        if (text.trim()) invoke("nav_to", { input: text.trim() }).catch(console.error);
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

// ===== Init =====
window.addEventListener("DOMContentLoaded", () => {
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
    (url) => invoke("nav_to", { input: url }).catch(console.error),
    () => {
      syncPanelOpen();
      updateHitRects();
    }
  );

  const winId = getWindowId();

  // Apply saved theme, then run the first-run wizard on the main window.
  invoke<Record<string, unknown>>("settings_get").then((s) => {
    if (typeof s.theme === "string") applyTheme(s.theme);
  }).catch(() => {});
  if (winId === 1) {
    new Wizard().maybeShow();
  }

  // Load initial tabs for main state
  invoke<Tab[]>("tabs_list", { windowId: winId }).then((result) => {
    tabs = result;
    updateTabCountBadge();
    // On the main window, restore last session if enabled and no tabs exist yet.
    if (winId === 1 && tabs.length === 0) {
      invoke<Record<string, unknown>>("settings_get").then((s) => {
        if (s.restoreSession) {
          invoke<Tab[]>("session_restore_last").catch(console.error);
        }
      }).catch(() => {});
    }
  }).catch(console.error);

  // ===== Button wiring =====
  document.getElementById("btn-back")?.addEventListener("click", () => invoke("nav_back", {}).catch(console.error));
  document.getElementById("btn-forward")?.addEventListener("click", () => invoke("nav_forward", {}).catch(console.error));
  document.getElementById("btn-reload")?.addEventListener("click", () => invoke("nav_reload", {}).catch(console.error));
  document.getElementById("btn-min")?.addEventListener("click", () => invoke("window_controls", { action: "min" }).catch(console.error));
  document.getElementById("btn-max")?.addEventListener("click", () => invoke("window_controls", { action: "max" }).catch(console.error));
  document.getElementById("btn-close")?.addEventListener("click", () => invoke("window_controls", { action: "close" }).catch(console.error));
  document.getElementById("btn-menu")?.addEventListener("click", () => {
    views?.open("settings");
  });

  // Pin-on-top toggle
  const pinBtn = document.getElementById("btn-pin");
  if (pinBtn) {
    const renderPin = (p: boolean) => {
      pinBtn.innerHTML = p ? icons.pinFilled : icons.pin;
      pinBtn.classList.toggle("pinned", p);
      pinBtn.setAttribute("aria-pressed", String(p));
      pinBtn.setAttribute("title", p ? "Unpin window" : "Pin window on top");
    };
    renderPin(pinnedOnTop);
    // Reflect any persisted state from the backend.
    invoke<boolean>("window_pinned_state").then((p) => {
      pinnedOnTop = p;
      renderPin(p);
    }).catch(() => {});
    pinBtn.addEventListener("click", () => {
      pinnedOnTop = !pinnedOnTop;
      renderPin(pinnedOnTop);
      invoke("window_set_pinned", { pinned: pinnedOnTop }).catch(console.error);
      showToast(pinnedOnTop ? "Pinned on top" : "Unpinned");
    });
  }

  // New-tab search: a real input — Enter navigates/searches directly.
  const newtabSearch = document.getElementById("newtab-search") as HTMLInputElement | null;
  newtabSearch?.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.key === "Enter") {
      const text = newtabSearch.value.trim();
      if (text) {
        invoke("nav_to", { input: text }).catch(console.error);
        newtabSearch.value = "";
      }
    }
  });

  // Domain pill opens the in-app address bar (matches its Ctrl+L tooltip).
  document.getElementById("domain-pill")?.addEventListener("click", () => openAddressBar());
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
            .then((text) => { if (text.trim()) invoke("nav_to", { input: text.trim() }); })
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
      if (val) invoke("nav_to", { input: val }).catch(console.error);
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
  // Enter = find next, Shift+Enter = find previous, Escape = close (Phase 8 item 8).
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

  // Overlay-side keyboard shortcuts (the accelerator handler only covers
  // content webviews; when the overlay itself has focus we map keys here).
  document.addEventListener("keydown", (e: KeyboardEvent) => {
    const t = e.target as HTMLElement | null;
    const typing = t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
    if (e.ctrlKey && e.shiftKey && e.key.toLowerCase() === "u") {
      e.preventDefault();
      toggleChromeUi();
    } else if (!typing && e.altKey && e.key === "ArrowLeft") {
      e.preventDefault();
      switchAdjacentTab(-1);
    } else if (!typing && e.altKey && e.key === "ArrowRight") {
      e.preventDefault();
      switchAdjacentTab(1);
    } else if (!typing && e.ctrlKey && (e.key === "+" || e.key === "=")) {
      e.preventDefault();
      applyZoom(zoomFactor + 0.1);
    } else if (!typing && e.ctrlKey && e.key === "-") {
      e.preventDefault();
      applyZoom(zoomFactor - 0.1);
    } else if (!typing && e.ctrlKey && e.key === "0") {
      e.preventDefault();
      applyZoom(1.0);
    } else if (e.key === "BrowserBack") {
      invoke("nav_back", {}).catch(console.error);
    } else if (e.key === "BrowserForward") {
      invoke("nav_forward", {}).catch(console.error);
    } else if (e.key === "Escape") {
      handleShortcut("Esc");
    }
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

  // ===== Clock =====
  updateClock();
  setInterval(updateClock, 1000);

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
      if (event.payload.id === activeTabId) updateDomainPill(event.payload);
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
    updateDomainPill(tab);
    // Hide/show newtab page
    const newtab = document.getElementById("newtab-page");
    if (newtab) {
      newtab.classList.toggle("visible", tab?.url === "about:blank" || !tab);
    }
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
  });

  listen<string>("window:open-view", (event) => {
    views?.open(event.payload as any);
  });

  // Top loading bar driven by NavigationStarting/Completed (Phase 8).
  listen<{ tabId: number; loading: boolean }>("nav:loading", (event) => {
    if (event.payload.tabId !== activeTabId) return;
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

  // Chrome Web Store intercept banner (Phase 4): the store's "Add to Chrome"
  // can't work in WebView2, so offer Jello's working sideload path.
  let webstoreBannerUrl = "";
  listen<string>("webstore:detected", (event) => {
    const url = event.payload;
    if (url === webstoreBannerUrl) return; // avoid re-showing on repeat events
    webstoreBannerUrl = url;
    showWebstoreBanner(url);
  });

  listen<DownloadItem>("download:started", (event) => {
    views?.addDownload(event.payload);
    showToast(`Downloading ${event.payload.fileName}`);
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
