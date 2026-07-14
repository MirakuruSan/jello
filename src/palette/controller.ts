import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { PaletteItem, PaletteResults } from "../types";
import * as icons from "../icons";

export class PaletteController {
  private searchInput: HTMLInputElement;
  private resultsContainer: HTMLElement;
  
  private items: PaletteItem[] = [];
  private selectedIndex = 0;
  private currentQuery = "";
  private mode = "search";

  constructor() {
    this.searchInput = document.getElementById("palette-search") as HTMLInputElement;
    this.resultsContainer = document.getElementById("palette-results") as HTMLElement;

    this.setupEventListeners();

    // Backend tells us the mode + prefill each time the palette opens.
    listen<{ mode: string; prefill: string }>("palette:open", (e) => {
      this.mode = e.payload.mode || "search";
      this.currentQuery = e.payload.prefill || "";
      this.searchInput.value = this.currentQuery;
      this.searchInput.focus();
      this.searchInput.select();
      this.selectedIndex = 0;
      this.queryPalette();
    }).catch(console.error);

    this.queryPalette();
  }

  private setupEventListeners(): void {
    // Input typing
    this.searchInput.addEventListener("input", (e) => {
      this.currentQuery = (e.target as HTMLInputElement).value;
      this.selectedIndex = 0;
      this.queryPalette();
    });

    // Keyboard navigation
    this.searchInput.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        this.navigate(1);
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        this.navigate(-1);
      } else if (e.key === "Escape") {
        e.preventDefault();
        invoke("palette_hide").catch(console.error);
      } else if (e.key === "Enter") {
        e.preventDefault();
        
        let disposition = this.mode === "newtab" ? "new-tab-foreground" : "current-tab";
        let targetUrl = "";
        
        if (e.ctrlKey && !e.shiftKey && !e.altKey) {
          // "Wrap as .com" is a shortcut for a bare word only (e.g. "github"
          // → github.com). For anything with a dot/space/scheme, or a real
          // query the user is selecting a result for, fall through to plain
          // Enter (open the highlighted item) instead of hijacking it.
          const q = this.currentQuery.trim();
          if (/^[a-z0-9-]+$/i.test(q)) {
            targetUrl = `https://www.${q}.com`;
            disposition = "current-tab";
          }
        } else if (e.altKey) {
          disposition = "new-tab-foreground";
        } else if (e.ctrlKey && e.shiftKey) {
          disposition = "new-tab-background";
        } else if (e.shiftKey) {
          disposition = "new-window";
        }

        this.openSelected(disposition, targetUrl);
      }
    });

    // Auto-focus when shown (we listen to window focus event)
    window.addEventListener("focus", () => {
      this.searchInput.focus();
      this.searchInput.select();
      this.queryPalette();
    });

    // Dismiss when the palette loses focus (click-away), like other quick bars.
    window.addEventListener("blur", () => {
      invoke("palette_hide").catch(() => {});
    });
  }

  /** Resize the palette window to fit the input + rendered rows. */
  private syncHeight(): void {
    const INPUT_H = 60;
    const rowsH = this.resultsContainer.scrollHeight;
    const desired = rowsH > 4 ? INPUT_H + Math.min(rowsH + 8, 420) : INPUT_H;
    invoke("palette_resize", { height: desired }).catch(() => {});
  }

  private async queryPalette(): Promise<void> {
    // Empty query → stay a clean single-line pill (no default lists).
    if (this.currentQuery.trim().length === 0) {
      this.items = [];
      this.resultsContainer.innerHTML = "";
      this.syncHeight();
      return;
    }
    try {
      const results = await invoke<PaletteResults>("palette_query", {
        text: this.currentQuery,
        scope: "all",
      });

      // Flatten results for navigation list
      this.items = [];
      
      // Add open tabs
      this.items.push(...results.openTabs);
      
      // Add bookmarks
      this.items.push(...results.bookmarks);

      // Add history
      this.items.push(...results.history);

      // Add search engine fallback if query is not empty
      if (this.currentQuery.trim().length > 0) {
        this.items.push({
          id: "search:duckduckgo",
          itemType: "search",
          title: `Search DuckDuckGo for "${this.currentQuery.trim()}"`,
          url: this.currentQuery.trim(),
          matchedRanges: [],
        });
      }

      // Constrain selectedIndex
      if (this.selectedIndex >= this.items.length) {
        this.selectedIndex = Math.max(0, this.items.length - 1);
      }

      this.render(results);
    } catch (e) {
      console.error("Failed to query palette", e);
    }
  }

  private navigate(direction: number): void {
    if (this.items.length === 0) return;
    this.selectedIndex = (this.selectedIndex + direction + this.items.length) % this.items.length;
    this.updateActiveItem();
  }

  private updateActiveItem(): void {
    const rows = this.resultsContainer.querySelectorAll(".palette-row");
    rows.forEach((row, idx) => {
      if (idx === this.selectedIndex) {
        row.classList.add("active");
        row.scrollIntoView({ block: "nearest" });
      } else {
        row.classList.remove("active");
      }
    });
  }

  private render(results: PaletteResults): void {
    this.resultsContainer.innerHTML = "";

    let flatIndex = 0;

    const renderSection = (title: string, sectionItems: PaletteItem[]) => {
      if (sectionItems.length === 0) return;

      const header = document.createElement("div");
      header.className = "palette-section-header";
      header.textContent = title;
      this.resultsContainer.appendChild(header);

      sectionItems.forEach((item) => {
        const rowIdx = flatIndex;
        flatIndex++;

        const row = document.createElement("div");
        row.className = "palette-row interactive" + (rowIdx === this.selectedIndex ? " active" : "");
        row.setAttribute("data-index", String(rowIdx));

        const icon = document.createElement("span");
        icon.className = "palette-row-icon";
        icon.innerHTML = item.itemType === "tab" ? icons.tab : item.itemType === "bookmark" ? icons.bookmark : icons.history;
        row.appendChild(icon);

        const details = document.createElement("div");
        details.className = "palette-row-details";

        const itemTitle = document.createElement("div");
        itemTitle.className = "palette-row-title";
        
        // Render title with highlights if any
        if (item.matchedRanges && item.matchedRanges.length > 0) {
          itemTitle.innerHTML = this.highlightText(item.title, item.matchedRanges);
        } else {
          itemTitle.textContent = item.title;
        }
        details.appendChild(itemTitle);

        const itemUrl = document.createElement("div");
        itemUrl.className = "palette-row-url";
        itemUrl.textContent = item.url;
        details.appendChild(itemUrl);

        row.appendChild(details);

        row.addEventListener("click", () => {
          this.selectedIndex = rowIdx;
          this.openSelected("current-tab");
        });

        this.resultsContainer.appendChild(row);
      });
    };

    renderSection("Open Tabs", results.openTabs);
    renderSection("Bookmarks", results.bookmarks);
    renderSection("History", results.history);

    // Render search fallback if query is not empty
    if (this.currentQuery.trim().length > 0) {
      const rowIdx = flatIndex;
      
      const row = document.createElement("div");
      row.className = "palette-row search-fallback interactive" + (rowIdx === this.selectedIndex ? " active" : "");
      row.setAttribute("data-index", String(rowIdx));

      const icon = document.createElement("span");
      icon.className = "palette-row-icon";
      icon.innerHTML = icons.search;
      row.appendChild(icon);

      const details = document.createElement("div");
      details.className = "palette-row-details";

      const itemTitle = document.createElement("div");
      itemTitle.className = "palette-row-title";
      itemTitle.textContent = `Search DuckDuckGo for "${this.currentQuery.trim()}"`;
      details.appendChild(itemTitle);

      row.appendChild(details);

      row.addEventListener("click", () => {
        this.selectedIndex = rowIdx;
        this.openSelected("current-tab");
      });

      this.resultsContainer.appendChild(row);
    }

    this.syncHeight();
  }

  private highlightText(text: string, ranges: [number, number][]): string {
    let html = "";
    let lastIdx = 0;
    
    // Sort ranges by start index just in case
    const sortedRanges = [...ranges].sort((a, b) => a[0] - b[0]);

    for (const [start, end] of sortedRanges) {
      if (start > lastIdx) {
        html += this.escapeHtml(text.slice(lastIdx, start));
      }
      html += `<mark class="palette-highlight">${this.escapeHtml(text.slice(start, end))}</mark>`;
      lastIdx = end;
    }
    if (lastIdx < text.length) {
      html += this.escapeHtml(text.slice(lastIdx));
    }
    return html;
  }

  private escapeHtml(text: string): string {
    const map: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#039;",
    };
    return text.replace(/[&<>"']/g, (m) => map[m]);
  }

  private openSelected(disposition: string, overrideUrl?: string): void {
    const item = this.items[this.selectedIndex];
    if (!item) return;

    const finalUrl = overrideUrl || item.url;

    invoke("palette_open", {
      id: item.id,
      url: finalUrl,
      disposition,
    }).catch(console.error);
  }
}
