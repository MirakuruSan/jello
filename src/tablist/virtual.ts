// Virtualized tab list renderer (~80 lines, per SPEC §E)
// Fixed 36px rows, renders only visible + buffer rows

import { invoke } from "@tauri-apps/api/core";
import type { Tab } from "../types";
import * as icons from "../icons";

const ROW_HEIGHT = 36;
const BUFFER = 20;

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, "");
  } catch {
    return url;
  }
}
function faviconLetter(url: string): string {
  const h = hostOf(url);
  return (h[0] || "?").toUpperCase();
}
function faviconColor(url: string): string {
  const h = hostOf(url);
  let hash = 0;
  for (let i = 0; i < h.length; i++) hash = (hash * 31 + h.charCodeAt(i)) & 0xffffff;
  return `hsl(${hash % 360}, 45%, 45%)`;
}

export interface VirtualTabListOptions {
  viewport: HTMLElement;
  spacer: HTMLElement;
  onActivate: (id: number) => void;
  onClose: (id: number) => void;
  onMuteToggle: (id: number) => void;
  onContextMenu?: (tab: Tab, x: number, y: number) => void;
}

export class VirtualTabList {
  private tabs: Tab[] = [];
  private opts: VirtualTabListOptions;
  private activeId = -1;

  constructor(opts: VirtualTabListOptions) {
    this.opts = opts;
    this.opts.viewport.addEventListener("scroll", () => this.render());
  }

  setTabs(tabs: Tab[], activeId: number): void {
    this.tabs = tabs;
    this.activeId = activeId;
    this.opts.spacer.style.height = `${tabs.length * ROW_HEIGHT}px`;
    this.render();
  }

  render(): void {
    const vp = this.opts.viewport;
    const scrollTop = vp.scrollTop;
    const vpHeight = vp.clientHeight;

    const start = Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - BUFFER);
    const end = Math.min(this.tabs.length, Math.ceil((scrollTop + vpHeight) / ROW_HEIGHT) + BUFFER);

    // Clear existing rows (they are absolute-positioned inside spacer)
    const spacer = this.opts.spacer;
    while (spacer.firstChild) spacer.removeChild(spacer.firstChild);

    for (let i = start; i < end; i++) {
      const tab = this.tabs[i];
      const row = document.createElement("div");
      row.className = "tab-row" + (tab.id === this.activeId ? " active" : "");
      row.style.top = `${i * ROW_HEIGHT}px`;
      row.setAttribute("data-tab-id", String(tab.id));
      row.draggable = true;

      // Favicon: privacy-preserving letter avatar derived from the host (no
      // network fetch). Real favicon-image caching is a documented deferral.
      const favicon = document.createElement("div");
      favicon.className = "favicon";
      favicon.textContent = faviconLetter(tab.url);
      favicon.style.background = faviconColor(tab.url);
      row.appendChild(favicon);

      // Title
      const title = document.createElement("span");
      title.className = "tab-title";
      title.textContent = tab.title || tab.url;
      row.appendChild(title);

      // State dot
      const dot = document.createElement("span");
      dot.className = "state-dot cold"; // Default cold; updated by caller
      row.appendChild(dot);

      // Audio icon (if not muted)
      if (tab.muted) {
        const audio = document.createElement("span");
        audio.className = "audio-icon";
        audio.innerHTML = icons.muted;
        audio.addEventListener("click", (e) => {
          e.stopPropagation();
          this.opts.onMuteToggle(tab.id);
        });
        row.appendChild(audio);
      }

      // Close button
      const close = document.createElement("span");
      close.className = "close-x";
      close.textContent = "×";
      close.addEventListener("click", (e) => {
        e.stopPropagation();
        this.opts.onClose(tab.id);
      });
      row.appendChild(close);

      // Click to activate
      row.addEventListener("click", () => this.opts.onActivate(tab.id));

      // Right-click → context menu (Close, Duplicate, Pin, Mute, Close others)
      row.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        e.stopPropagation();
        this.opts.onContextMenu?.(tab, e.clientX, e.clientY);
      });

      // Drag start
      row.addEventListener("dragstart", (e) => {
        e.dataTransfer?.setData("text/plain", String(tab.id));
      });

      // Drop target
      row.addEventListener("dragover", (e) => e.preventDefault());
      row.addEventListener("drop", (e) => {
        e.preventDefault();
        const dragId = Number(e.dataTransfer?.getData("text/plain"));
        if (dragId && dragId !== tab.id) {
          invoke("tabs_reorder", { id: dragId, beforeId: tab.id }).catch(console.error);
        }
      });

      spacer.appendChild(row);
    }
  }
}
