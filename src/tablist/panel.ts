import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { VirtualTabList } from "./virtual";
import { showContextMenu, type MenuEntry } from "../contextMenu";
import type { Tab } from "../types";

export class TabPanelController {
  private panel: HTMLElement;
  private viewport: HTMLElement;
  private spacer: HTMLElement;
  private searchInput: HTMLInputElement;
  private tabList: VirtualTabList;
  
  private tabs: Tab[] = [];
  private activeTabId = -1;
  private filterQuery = "";
  private windowId = 1;
  /** tab id → "live" | "suspended" (absent = unloaded). */
  private loadedStates: Record<number, string> = {};
  /** Notifies main.ts so it can sync overlay_set_panel_open + hit rects. */
  private onOpenChanged: (open: boolean) => void;

  constructor(onOpenChanged: (open: boolean) => void = () => {}) {
    this.onOpenChanged = onOpenChanged;
    const label = getCurrentWebviewWindow().label;
    if (label === "main") {
      this.windowId = 1;
    } else {
      const match = label.match(/_(.+)$/);
      if (match) {
        const id = parseInt(match[1], 10);
        if (!isNaN(id)) this.windowId = id;
      }
    }

    this.panel = document.getElementById("tab-panel") as HTMLElement;
    this.viewport = document.getElementById("tab-list-viewport") as HTMLElement;
    this.spacer = document.getElementById("tab-list-spacer") as HTMLElement;
    this.searchInput = document.getElementById("tab-search") as HTMLInputElement;

    this.tabList = new VirtualTabList({
      viewport: this.viewport,
      spacer: this.spacer,
      onActivate: (id) => this.activateTab(id),
      onClose: (id) => this.closeTab(id),
      onMuteToggle: (id) => this.toggleMute(id),
      onContextMenu: (tab, x, y) => this.showTabMenu(tab, x, y),
    });

    this.setupEventListeners();
  }

  public isOpen(): boolean {
    // "open" is the class the CSS animates on (styles.css .tab-panel.open).
    return this.panel.classList.contains("open");
  }

  public toggle(): void {
    if (this.isOpen()) {
      this.hide();
    } else {
      this.show();
    }
  }

  public show(): void {
    this.panel.classList.add("open");
    this.searchInput.focus();
    this.loadTabs();
    this.onOpenChanged(true);
  }

  public hide(): void {
    this.panel.classList.remove("open");
    this.searchInput.value = "";
    this.filterQuery = "";
    this.onOpenChanged(false);
  }

  private setupEventListeners(): void {
    // Search input filtering
    this.searchInput.addEventListener("input", (e) => {
      this.filterQuery = (e.target as HTMLInputElement).value.toLowerCase();
      this.applyFilter();
    });

    // Toggle button in UI
    const toggleBtn = document.getElementById("btn-tab-count");
    if (toggleBtn) {
      toggleBtn.addEventListener("click", () => this.toggle());
    }

    const closeBtn = document.getElementById("btn-close-panel");
    if (closeBtn) {
      closeBtn.addEventListener("click", () => this.hide());
    }

    // Tauri backend event listeners (backend emits bare ids for
    // tab:activated / tab:closed — see tabs.rs `app.emit("tab:activated", id)`)
    listen("tab:created", () => this.scheduleReload());
    listen("tab:updated", () => this.scheduleReload());
    listen("tab:closed", () => this.scheduleReload());
    listen("tab:activated", (e) => {
      this.activeTabId = e.payload as number;
      this.scheduleReload();
    });
  }

  // Coalesce bursts of tab:* events (opening a tab fires created + activated
  // back-to-back) into a single tabs_list fetch on a 50ms trailing timer, so
  // rapid tab churn doesn't trigger N redundant DB reads (Phase 3).
  private reloadTimer: number | null = null;
  private scheduleReload(): void {
    if (this.reloadTimer !== null) return;
    this.reloadTimer = window.setTimeout(() => {
      this.reloadTimer = null;
      this.loadTabs();
    }, 50);
  }

  private async loadTabs(): Promise<void> {
    try {
      const tabs = await invoke<Tab[]>("tabs_list", { windowId: this.windowId });
      this.tabs = tabs;
      this.loadedStates = await invoke<Record<number, string>>("tabs_loaded_states").catch(() => ({}));
      
      // Update Tab Count badge
      const badge = document.getElementById("btn-tab-count");
      if (badge) {
        badge.textContent = String(tabs.length);
      }

      // If activeTabId is not set, find it from the list
      // (Wait, we can assume the active tab has lastActive set or get it from activeTabId state)
      // For simplicity, find the most recently activated tab if activeTabId is -1
      if (this.activeTabId === -1 && tabs.length > 0) {
        const sorted = [...tabs].sort((a, b) => (b.lastActive || 0) - (a.lastActive || 0));
        this.activeTabId = sorted[0].id;
      }

      this.applyFilter();
    } catch (e) {
      console.error("Failed to load tabs", e);
    }
  }

  private applyFilter(): void {
    let filtered = this.tabs;
    if (this.filterQuery) {
      filtered = this.tabs.filter(
        (t) =>
          (t.title && t.title.toLowerCase().includes(this.filterQuery)) ||
          t.url.toLowerCase().includes(this.filterQuery)
      );
    }
    this.tabList.setTabs(filtered, this.activeTabId, this.loadedStates);
  }

  private activateTab(id: number): void {
    invoke("tabs_activate", { id }).catch(console.error);
    this.hide();
  }

  private closeTab(id: number): void {
    invoke("tabs_close", { id }).then(() => this.loadTabs()).catch(console.error);
  }

  private toggleMute(id: number): void {
    const tab = this.tabs.find((t) => t.id === id);
    if (tab) {
      invoke("tabs_set_muted", { id, muted: !tab.muted })
        .then(() => this.loadTabs())
        .catch(console.error);
    }
  }

  private showTabMenu(tab: Tab, x: number, y: number): void {
    const reload = () => this.loadTabs();
    const entries: MenuEntry[] = [
      { label: "Duplicate", onClick: () =>
          invoke("tabs_duplicate", { id: tab.id }).then(reload).catch(console.error) },
      { label: tab.pinned ? "Unpin" : "Pin", onClick: () =>
          invoke("tabs_set_pinned", { id: tab.id, pinned: !tab.pinned }).then(reload).catch(console.error) },
      { label: tab.muted ? "Unmute" : "Mute", onClick: () =>
          invoke("tabs_set_muted", { id: tab.id, muted: !tab.muted }).then(reload).catch(console.error) },
      // Greyed out when the tab has no webview to unload (#4).
      { label: "Unload", disabled: !(tab.id in this.loadedStates), onClick: () =>
          invoke("tabs_unload", { id: tab.id }).then(reload).catch(console.error) },
      "separator",
      { label: "Close others", disabled: this.tabs.length <= 1, onClick: () => {
          const others = this.tabs.filter((t) => t.id !== tab.id).map((t) => t.id);
          Promise.all(others.map((id) => invoke("tabs_close", { id })))
            .then(reload).catch(console.error);
        } },
      { label: "Close", danger: true, onClick: () =>
          invoke("tabs_close", { id: tab.id }).then(reload).catch(console.error) },
    ];
    showContextMenu(x, y, entries);
  }
}
