// History / Bookmarks / Downloads / Settings panel.
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import * as icons from "../icons";

interface HistoryEntry { id: number; url: string; title: string; visitCount: number; lastVisit: number; }
interface Bookmark { id: number; url: string; title: string; position: number; }
export interface DownloadItem {
  id: string;
  fileName: string;
  url: string;
  state: string;
  received?: number;
  total?: number;
  path?: string;
  /// WebView2 interrupt reason for failed/interrupted downloads.
  reason?: string;
}

/// Download states that are over (no further progress is possible).
const DOWNLOAD_ENDED = new Set(["completed", "cancelled", "failed"]);
const DOWNLOAD_LABEL: Record<string, string> = {
  inprogress: "Downloading",
  paused: "Paused",
  interrupted: "Interrupted",
  completed: "Done",
  cancelled: "Cancelled",
  failed: "Failed",
};

type ViewName = "history" | "bookmarks" | "downloads" | "settings";

export class ViewsController {
  private panel: HTMLElement;
  private content: HTMLElement;
  private current: ViewName = "history";
  private downloads: DownloadItem[] = [];
  /// Scroll handler of the settings section index; replaced on each render.
  private navSync: (() => void) | null = null;
  private onNavigate: (url: string) => void;
  private onOpenChanged: (open: boolean) => void;

  constructor(onNavigate: (url: string) => void, onOpenChanged: (open: boolean) => void = () => {}) {
    this.onNavigate = onNavigate;
    this.onOpenChanged = onOpenChanged;
    this.panel = document.getElementById("views-panel")!;
    this.content = document.getElementById("views-content")!;

    document.querySelectorAll<HTMLButtonElement>(".views-tab").forEach((tab) => {
      tab.addEventListener("click", () => this.open(tab.dataset.view as ViewName));
    });
    document.getElementById("views-close")?.addEventListener("click", () => this.close());
  }

  isOpen(): boolean {
    return this.panel.classList.contains("open");
  }

  open(view: ViewName): void {
    this.current = view;
    this.panel.classList.add("open");
    this.onOpenChanged(true);
    document.querySelectorAll<HTMLButtonElement>(".views-tab").forEach((t) =>
      t.classList.toggle("active", t.dataset.view === view)
    );
    this.render();
  }

  close(): void {
    this.panel.classList.remove("open");
    this.onOpenChanged(false);
  }

  addDownload(item: DownloadItem): void {
    const existing = this.downloads.find((d) => d.id === item.id);
    if (existing) Object.assign(existing, item);
    else this.downloads.unshift(item);
    if (this.isOpen() && this.current === "downloads") this.render();
  }

  /// Merge a progress/done update into an existing download. Re-renders
  /// only the affected row's dynamic bits to avoid a full rebuild storm.
  updateDownload(id: string, patch: Partial<DownloadItem>): void {
    const d = this.downloads.find((x) => x.id === id);
    if (!d) return;
    Object.assign(d, patch);
    if (this.isOpen() && this.current === "downloads") this.render();
  }

  private async render(): Promise<void> {
    if (this.navSync) {
      this.content.removeEventListener("scroll", this.navSync);
      this.navSync = null;
    }
    this.content.innerHTML = "<div class='views-loading'>Loading…</div>";
    switch (this.current) {
      case "history": return this.renderHistory();
      case "bookmarks": return this.renderBookmarks();
      case "downloads": return this.renderDownloads();
      case "settings": return this.renderSettings();
    }
  }

  private rowHtml(title: string, url: string): HTMLElement {
    const row = document.createElement("div");
    row.className = "views-row";
    const t = document.createElement("div");
    t.className = "views-row-title";
    t.textContent = title || url;
    const u = document.createElement("div");
    u.className = "views-row-url";
    u.textContent = url;
    row.appendChild(t);
    row.appendChild(u);
    return row;
  }

  private async renderHistory(): Promise<void> {
    const entries = await invoke<HistoryEntry[]>("history_search", { query: "", limit: 300 }).catch(() => []);
    this.content.innerHTML = "";
    const bar = document.createElement("div");
    bar.className = "views-toolbar";
    const search = document.createElement("input");
    search.className = "views-search interactive";
    search.placeholder = "Search history…";
    const clearBtn = document.createElement("button");
    clearBtn.className = "views-btn interactive";
    clearBtn.textContent = "Clear all";
    clearBtn.addEventListener("click", async () => {
      await invoke("history_delete", { mode: "all" }).catch(() => {});
      this.renderHistory();
    });
    bar.appendChild(search);
    bar.appendChild(clearBtn);
    this.content.appendChild(bar);

    const list = document.createElement("div");
    list.className = "views-list";
    this.content.appendChild(list);

    const paint = (items: HistoryEntry[]) => {
      list.innerHTML = "";
      for (const h of items) {
        const row = this.rowHtml(h.title, h.url);
        row.addEventListener("click", () => { this.onNavigate(h.url); this.close(); });
        const del = document.createElement("button");
        del.className = "views-row-del interactive";
        del.innerHTML = icons.close;
        del.setAttribute("aria-label", "Remove");
        del.addEventListener("click", async (e) => {
          e.stopPropagation();
          await invoke("history_delete", { mode: "ids", ids: [h.id] }).catch(() => {});
          row.remove();
        });
        row.appendChild(del);
        list.appendChild(row);
      }
    };
    paint(entries);

    let timer: ReturnType<typeof setTimeout> | null = null;
    search.addEventListener("input", () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(async () => {
        const res = await invoke<HistoryEntry[]>("history_search", { query: search.value, limit: 300 }).catch(() => []);
        paint(res);
      }, 150);
    });
  }

  private async renderBookmarks(): Promise<void> {
    const marks = await invoke<Bookmark[]>("bookmarks_list").catch(() => []);
    this.content.innerHTML = "";
    const list = document.createElement("div");
    list.className = "views-list";
    for (const b of marks) {
      const row = this.rowHtml(b.title, b.url);
      row.addEventListener("click", () => { this.onNavigate(b.url); this.close(); });
      const del = document.createElement("button");
      del.className = "views-row-del interactive";
      del.innerHTML = icons.close;
        del.setAttribute("aria-label", "Remove");
      del.addEventListener("click", async (e) => {
        e.stopPropagation();
        await invoke("bookmarks_remove", { id: b.id }).catch(() => {});
        row.remove();
      });
      row.appendChild(del);
      list.appendChild(row);
    }
    if (marks.length === 0) list.innerHTML = "<div class='views-empty'>No bookmarks yet.</div>";
    this.content.appendChild(list);
  }

  private renderDownloads(): void {
    this.content.innerHTML = "";
    const list = document.createElement("div");
    list.className = "views-list";
    if (this.downloads.length === 0) {
      list.innerHTML = "<div class='views-empty'>No downloads this session.</div>";
    }
    const fmtBytes = (n: number): string => {
      if (!n || n < 0) return "";
      const u = ["B", "KB", "MB", "GB"];
      let i = 0;
      let v = n;
      while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
      return `${v.toFixed(i === 0 ? 0 : 1)}${u[i]}`;
    };
    for (const d of this.downloads) {
      const row = this.rowHtml(d.fileName, d.url);
      const inflight = !DOWNLOAD_ENDED.has(d.state);

      // Progress bar (received/total) for in-flight downloads.
      if (inflight && (d.total ?? 0) > 0) {
        const bar = document.createElement("div");
        bar.className = "views-dl-bar";
        bar.style.cssText = "flex-basis:100%;height:4px;border-radius:2px;background:var(--surface-2,rgba(128,128,128,0.25));overflow:hidden;margin-top:4px;";
        const fill = document.createElement("div");
        const pct = Math.min(100, Math.round(((d.received ?? 0) / (d.total || 1)) * 100));
        fill.style.cssText = `height:100%;width:100%;transform-origin:left;transform:scaleX(${pct / 100});background:var(--accent,#4a9eff);transition:transform 0.2s;`;
        bar.appendChild(fill);
        row.appendChild(bar);
      }

      const state = document.createElement("span");
      state.className = "views-dl-state";
      const sizeText = (d.total ?? 0) > 0 ? ` ${fmtBytes(d.received ?? 0)}/${fmtBytes(d.total ?? 0)}` : "";
      const label = DOWNLOAD_LABEL[d.state] ?? d.state;
      state.textContent = inflight ? `${label}${sizeText}` : label;
      if (d.reason && (d.state === "failed" || d.state === "interrupted")) {
        state.textContent += ` — ${d.reason}`;
      }
      row.appendChild(state);

      const mkBtn = (label: string, cmd: string) => {
        const b = document.createElement("button");
        b.className = "views-btn interactive";
        b.textContent = label;
        b.addEventListener("click", () => {
          invoke(cmd, { id: d.id }).catch(() => {});
        });
        return b;
      };

      const mkToggle = (label: string, cmd: string, newState: string) => {
        const b = document.createElement("button");
        b.className = "views-btn interactive";
        b.textContent = label;
        b.addEventListener("click", () => {
          invoke(cmd, { id: d.id })
            .then(() => this.updateDownload(d.id, { state: newState }))
            .catch(() => {});
        });
        return b;
      };

      if (inflight) {
        if (d.state === "paused" || d.state === "interrupted") {
          row.appendChild(mkToggle(d.state === "paused" ? "Resume" : "Retry", "download_resume", "inprogress"));
        } else {
          row.appendChild(mkToggle("Pause", "download_pause", "paused"));
        }
        row.appendChild(mkBtn("Cancel", "download_cancel"));
      } else if (d.state === "completed" && d.path) {
        const mkPathBtn = (text: string, cmd: string) => {
          const b = document.createElement("button");
          b.className = "views-btn interactive";
          b.textContent = text;
          b.addEventListener("click", () => {
            invoke(cmd, { path: d.path }).catch((e) => console.error(e));
          });
          return b;
        };
        row.appendChild(mkPathBtn("Open", "download_open"));
        row.appendChild(mkPathBtn("Show in folder", "download_reveal"));
      }
      list.appendChild(row);
    }
    this.content.appendChild(list);
  }

  private async renderSettings(): Promise<void> {
    const settings = await invoke<Record<string, unknown>>("settings_get").catch(
      () => ({}) as Record<string, unknown>
    );
    this.content.innerHTML = "";
    const form = document.createElement("div");
    form.className = "views-settings";

    const addToggle = (key: string, label: string, def: boolean) => {
      const wrap = document.createElement("label");
      wrap.className = "views-setting-row";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.className = "interactive";
      cb.checked = settings[key] === undefined ? def : Boolean(settings[key]);
      cb.addEventListener("change", () => {
        invoke("settings_set", { patch: { [key]: cb.checked } }).catch(() => {});
      });
      const span = document.createElement("span");
      span.textContent = label;
      wrap.appendChild(cb);
      wrap.appendChild(span);
      form.appendChild(wrap);
    };

    // Section header + a short description, so settings read as grouped cards
    // instead of one long undifferentiated list.
    const addSection = (title: string, subtitle?: string) => {
      const h = document.createElement("div");
      h.className = "views-section-header";
      h.textContent = title;
      form.appendChild(h);
      if (subtitle) {
        const s = document.createElement("div");
        s.className = "views-section-sub";
        s.textContent = subtitle;
        form.appendChild(s);
      }
    };

    addSection("General");
    const rerun = document.createElement("button");
    rerun.className = "views-btn interactive";
    rerun.textContent = "Run setup again";
    rerun.addEventListener("click", () => {
      invoke("settings_set", { patch: { wizardComplete: false } }).catch(() => {});
      window.dispatchEvent(new CustomEvent("jello:run-wizard"));
    });
    form.appendChild(rerun);

    // Theme + window material apply live: save, re-apply the native backdrop,
    // and tell every window (palette included) to restyle.
    addSection("Appearance", "Acrylic shows a live blur of whatever is behind the window on the new-tab page and the palette; Mica tints with your wallpaper and is lighter on the GPU.");
    const addChoice = (key: string, label: string, options: [string, string][], def: string) => {
      const row = document.createElement("label");
      row.className = "views-setting-row";
      const span = document.createElement("span");
      span.textContent = label;
      span.style.cssText = "flex:1;";
      const sel = document.createElement("select");
      sel.className = "views-search interactive";
      sel.style.cssText = "flex:0 0 200px;";
      const current = typeof settings[key] === "string" ? (settings[key] as string) : def;
      for (const [value, text] of options) {
        const o = document.createElement("option");
        o.value = value;
        o.textContent = text;
        o.selected = value === current;
        sel.appendChild(o);
      }
      sel.addEventListener("change", async () => {
        await invoke("settings_set", { patch: { [key]: sel.value } }).catch(() => {});
        const all = await invoke<Record<string, unknown>>("settings_get").catch(() => ({ [key]: sel.value }));
        await emit("appearance:changed", all).catch(() => {});
        invoke("appearance_apply").catch(() => {});
      });
      row.append(span, sel);
      form.appendChild(row);
    };
    addChoice("theme", "Theme", [["dark", "Dark"], ["light", "Light"]], "dark");
    addChoice("windowMaterial", "Window material", [["acrylic", "Acrylic (live blur)"], ["mica", "Mica (wallpaper tint)"], ["solid", "Solid"]], "acrylic");



    addSection("Privacy & security", "Password and autofill changes apply to tabs loaded afterwards. Ad blocking is the uBlock Origin entry under Extensions.");
    addToggle("passwordAutosave", "Save & autofill passwords", true);
    addToggle("generalAutofill", "Autofill addresses & forms", true);
    addToggle("clearHistoryOnExit", "Clear history on exit", false);
    addToggle("allowFileUrls", "Allow local file:// URLs", false);

    addSection("Startup & window");
    addToggle("restoreSession", "Restore tabs on startup", false);

    // Autostart setting
    const autostartWrap = document.createElement("label");
    autostartWrap.className = "views-setting-row";
    const autostartCb = document.createElement("input");
    autostartCb.type = "checkbox";
    autostartCb.className = "interactive";
    autostartCb.disabled = true;
    autostartWrap.appendChild(autostartCb);
    const autostartSpan = document.createElement("span");
    autostartSpan.textContent = "Start Jello when Windows starts";
    autostartWrap.appendChild(autostartSpan);
    form.appendChild(autostartWrap);

    invoke<boolean>("autostart_status")
      .then((enabled) => {
        autostartCb.checked = enabled;
        autostartCb.disabled = false;
      })
      .catch(() => {
        autostartCb.disabled = true;
      });

    autostartCb.addEventListener("change", async () => {
      autostartCb.disabled = true;
      try {
        if (autostartCb.checked) {
          await invoke("autostart_enable");
        } else {
          await invoke("autostart_disable");
        }
      } catch (err) {
        console.error(err);
      } finally {
        autostartCb.disabled = false;
      }
    });

    addToggle("startMinimized", "Start Jello minimized in system tray", false);
    addToggle("minimizeToTray", "Closing the window minimizes to the system tray", true);

    // Numeric choice stored as a number (0 = never / unlimited).
    const addSelect = (key: string, label: string, options: [number, string][], def: number) => {
      const row = document.createElement("label");
      row.className = "views-setting-row";
      const span = document.createElement("span");
      span.textContent = label;
      span.style.cssText = "flex:1;";
      const sel = document.createElement("select");
      sel.className = "views-search interactive";
      sel.style.cssText = "flex:0 0 150px;";
      const current = Number(settings[key] ?? def);
      for (const [value, text] of options) {
        const o = document.createElement("option");
        o.value = String(value);
        o.textContent = text;
        o.selected = value === current;
        sel.appendChild(o);
      }
      sel.addEventListener("change", () => {
        invoke("settings_set", { patch: { [key]: Number(sel.value) } }).catch(() => {});
      });
      row.append(span, sel);
      form.appendChild(row);
    };

    addSection(
      "Performance",
      "Fewer loaded tabs and quicker unloading keep memory and CPU low. Tabs playing audio, downloading, or in picture-in-picture are never frozen or unloaded."
    );
    addSelect("maxLoadedTabs", "Tabs kept loaded at once",
      [[3, "3"], [5, "5"], [8, "8"], [12, "12"], [0, "Unlimited"]], 5);
    addSelect("suspendAfterMins", "Freeze background tabs after",
      [[1, "1 minute"], [2, "2 minutes"], [5, "5 minutes"], [15, "15 minutes"], [30, "30 minutes"], [0, "Never"]], 5);
    addSelect("discardAfterMins", "Unload inactive tabs after (memory saver)",
      [[10, "10 minutes"], [30, "30 minutes"], [60, "1 hour"], [120, "2 hours"], [0, "Never"]], 30);

    addSection("Media");
    addToggle("autoPip", "Float a playing video in picture-in-picture when you switch tabs or minimize (Alt+P toggles it any time)", true);

    this.renderFilesSection(form, addSection);


    // --- Extensions ---
    this.renderExtensionsSection(form);

    // --- Global hotkeys (rebindable) ---
    const hkHeader = document.createElement("div");
    hkHeader.textContent = "Global hotkeys";
    hkHeader.className = "views-section-header";
    form.appendChild(hkHeader);
    const hkNote = document.createElement("div");
    hkNote.textContent = "Format: Ctrl+Alt+K, Ctrl+Shift+Space, … Press Apply to save; conflicts are rejected.";
    hkNote.style.cssText = "color:var(--text-dim);font-size:0.6875rem;margin-bottom:4px;";
    form.appendChild(hkNote);
    // Rows arrive async — anchor them HERE. Appending straight to `form` inside
    // the .then() pushed them after every later section, leaving this section
    // visually empty.
    const hkWrap = document.createElement("div");
    form.appendChild(hkWrap);

    const hkLabels: Record<string, string> = {
      summon: "Summon / hide Jello",
      palette: "Open quick palette",
      addressbar: "Open address bar",
      aichat: "Open AI chatbot",
      screenshot: "Screenshot capture",
      ocr: "OCR text capture",
      incognito: "New incognito window",
      leader: "Leader key (chords)",
    };
    invoke<{ action: string; shortcut: string }[]>("hotkey_list")
      .then((items) => {
        for (const item of items) {
          const row = document.createElement("div");
          row.className = "views-setting-row";
          const name = document.createElement("span");
          name.textContent = hkLabels[item.action] || item.action;
          name.style.cssText = "flex:1;";
          const input = document.createElement("input");
          input.type = "text";
          input.value = item.shortcut;
          input.className = "views-search interactive";
          input.style.cssText = "flex:0 0 180px;";
          const apply = document.createElement("button");
          apply.className = "views-btn interactive";
          apply.textContent = "Apply";
          const status = document.createElement("span");
          status.style.cssText = "font-size:0.6875rem;min-width:60px;";
          apply.addEventListener("click", async () => {
            status.textContent = "…";
            try {
              await invoke("hotkey_rebind", { action: item.action, shortcut: input.value.trim() });
              status.textContent = "saved";
              status.style.color = "var(--ok)";
            } catch (err) {
              status.textContent = String(err);
              status.style.color = "var(--danger)";
            }
          });
          row.append(name, input, apply, status);
          hkWrap.appendChild(row);
        }
      })
      .catch(() => {});

    // --- In-window shortcuts (reference) ---
    const swHeader = document.createElement("div");
    swHeader.textContent = "In-window shortcuts";
    swHeader.className = "views-section-header";
    form.appendChild(swHeader);
    const swNote = document.createElement("div");
    swNote.textContent = "Active while browsing a page. Rebinding these is planned for a later release.";
    swNote.style.cssText = "color:var(--text-dim);font-size:0.6875rem;margin-bottom:4px;";
    form.appendChild(swNote);

    const inWindowShortcuts: [string, string][] = [
      ["Toggle chrome UI", "Ctrl+Shift+U"],
      ["Address bar", "Ctrl+L  /  F6"],
      ["New tab", "Ctrl+T"],
      ["Reopen closed tab", "Ctrl+Shift+T"],
      ["Close tab", "Ctrl+W"],
      // "New window" (Ctrl+N) and "New incognito window" (Ctrl+Shift+N) are
      // beta-gated — see windows.rs FEATURE_MULTIWINDOW / FEATURE_INCOGNITO.
      ["Next / previous tab (MRU)", "Ctrl+Tab  /  Ctrl+Shift+Tab"],
      ["Switch between tabs", "Alt+←  /  Alt+→"],
      ["Jump to tab 1–9", "Ctrl+1 … Ctrl+9"],
      ["History back / forward", "Browser Back / Forward keys"],
      ["Zoom in / out / reset", "Ctrl+=  /  Ctrl+−  /  Ctrl+0"],
      ["Reload / hard reload", "Ctrl+R  /  Ctrl+Shift+R"],
      ["Find in page", "Ctrl+F"],
      ["Bookmark page", "Ctrl+D"],
      ["Copy / paste-and-go URL", "Ctrl+Shift+C  /  Ctrl+Shift+V"],
      ["History / Downloads / Bookmarks", "Ctrl+H  /  Ctrl+J  /  Ctrl+Shift+O"],
      ["Tab list panel", "Ctrl+Shift+E"],
      ["Mute tab", "Ctrl+M"],
      ["Picture-in-picture video", "Alt+P"],
      ["Unload tab (free its memory)", "Ctrl+Shift+W"],
      ["Fullscreen", "F11"],
      ["Close window", "Ctrl+Q"],
    ];
    for (const [label, keys] of inWindowShortcuts) {
      const row = document.createElement("div");
      row.className = "views-setting-row";
      const name = document.createElement("span");
      name.textContent = label;
      name.style.cssText = "flex:1;";
      const kbd = document.createElement("span");
      kbd.textContent = keys;
      kbd.style.cssText = "color:var(--text-dim);font-size:0.6875rem;font-family:var(--mono,monospace);";
      row.append(name, kbd);
      form.appendChild(row);
    }


    // "Wipe all history" lives in the History tab ("Clear all"); not duplicated
    // here.

    const updaterEnabled = await invoke<boolean>("updater_enabled").catch(() => false);
    if (updaterEnabled) {
      addSection("Updates");
      addToggle("updateCheck", "Automatically check for updates", false);

      const checkUpdateBtn = document.createElement("button");
      checkUpdateBtn.className = "views-btn interactive";
      checkUpdateBtn.textContent = "Check for updates";
      checkUpdateBtn.style.marginTop = "12px";
      checkUpdateBtn.style.marginBottom = "12px";
      checkUpdateBtn.addEventListener("click", async () => {
        checkUpdateBtn.disabled = true;
        checkUpdateBtn.textContent = "Checking…";
        try {
          const updateVersion = await invoke<string | null>("updater_check");
          if (updateVersion) {
            const confirmInstall = confirm(`Update version ${updateVersion} is available! Do you want to download and install it now?`);
            if (confirmInstall) {
              checkUpdateBtn.textContent = "Installing…";
              await invoke("updater_apply");
            } else {
              checkUpdateBtn.textContent = "Check for updates";
              checkUpdateBtn.disabled = false;
            }
          } else {
            alert("Jello is up to date!");
            checkUpdateBtn.textContent = "Check for updates";
            checkUpdateBtn.disabled = false;
          }
        } catch (err) {
          alert(`Error checking for updates: ${err}`);
          checkUpdateBtn.textContent = "Check for updates";
          checkUpdateBtn.disabled = false;
        }
      });
      form.appendChild(checkUpdateBtn);
    }

    this.content.appendChild(this.withSectionNav(form));
  }

  /// Wrap the settings form with a section index: click to jump, and the
  /// section being read stays highlighted while scrolling.
  private withSectionNav(form: HTMLElement): HTMLElement {
    const layout = document.createElement("div");
    layout.className = "settings-layout";
    const nav = document.createElement("nav");
    nav.className = "settings-nav";
    nav.setAttribute("aria-label", "Settings sections");
    const headers = Array.from(form.querySelectorAll<HTMLElement>(".views-section-header"));
    const links = headers.map((h) => {
      const b = document.createElement("button");
      b.className = "interactive";
      b.textContent = h.textContent;
      b.addEventListener("click", () => h.scrollIntoView({ block: "start", behavior: "smooth" }));
      nav.appendChild(b);
      return b;
    });
    const sync = () => {
      const top = this.content.getBoundingClientRect().top + 24;
      let current = 0;
      headers.forEach((h, i) => { if (h.getBoundingClientRect().top <= top) current = i; });
      links.forEach((l, i) => l.classList.toggle("active", i === current));
    };
    this.navSync = sync;
    this.content.addEventListener("scroll", sync, { passive: true });
    requestAnimationFrame(sync);
    layout.append(nav, form);
    return layout;
  }

  /// "Open with", file types, and default-browser hookup (file_assoc.rs).
  private renderFilesSection(form: HTMLElement, addSection: (title: string, subtitle?: string) => void): void {
    addSection(
      "Default apps",
      "Jello registers with Windows as an \"Open with\" handler for the checked file types. Windows only lets you pick defaults yourself — use the button to open Default Apps."
    );
    // Rows arrive async; anchor them here so they stay inside this section.
    const wrap = document.createElement("div");
    form.appendChild(wrap);

    type Status = { groups: { key: string; label: string; enabled: boolean }[]; defaultBrowser: boolean; available: boolean };
    invoke<Status>("file_assoc_status")
      .then((st) => {
        const boxes: { key: string; cb: HTMLInputElement }[] = [];
        const status = document.createElement("div");
        status.className = "views-section-sub";
        const save = () => {
          const groups = boxes.filter((b) => b.cb.checked).map((b) => b.key);
          invoke("file_assoc_set", { groups })
            .then(() => { status.textContent = "Saved — file-type registration updated."; })
            .catch((e) => { status.textContent = String(e); });
        };
        for (const g of st.groups) {
          const row = document.createElement("label");
          row.className = "views-setting-row";
          const cb = document.createElement("input");
          cb.type = "checkbox";
          cb.className = "interactive";
          cb.checked = g.enabled;
          cb.disabled = !st.available;
          cb.addEventListener("change", save);
          const span = document.createElement("span");
          span.textContent = g.label;
          row.append(cb, span);
          wrap.appendChild(row);
          boxes.push({ key: g.key, cb });
        }
        const btn = document.createElement("button");
        btn.className = "views-btn interactive";
        btn.textContent = st.defaultBrowser ? "Jello is your default browser ✓ — manage defaults…" : "Make Jello your default browser…";
        btn.disabled = !st.available;
        btn.addEventListener("click", () => {
          invoke("file_assoc_open_defaults").catch((e) => { status.textContent = String(e); });
        });
        wrap.appendChild(btn);
        if (!st.available) status.textContent = "Shell integration is registered by installed builds of Jello.";
        wrap.appendChild(status);
      })
      .catch(() => {});
  }

  private renderExtensionsSection(form: HTMLElement): void {
    const header = document.createElement("div");
    header.textContent = "Extensions";
    header.className = "views-section-header";
    form.appendChild(header);

    const note = document.createElement("div");
    note.textContent =
      "Install Chrome extensions by Web Store URL or 32-char ID. Changes apply to newly opened tabs; restart Jello to apply everywhere.";
    note.style.cssText = "color:var(--text-dim);font-size:0.6875rem;margin-bottom:6px;";
    form.appendChild(note);

    const restartNote = document.createElement("div");
    restartNote.textContent = "↻ Restart Jello to apply extension changes everywhere.";
    restartNote.style.cssText =
      "color:var(--warn);font-size:0.6875rem;margin:4px 0;display:none;";
    const restartBtn = document.createElement("button");
    restartBtn.className = "views-btn primary interactive";
    restartBtn.textContent = "↻ Restart Jello now";
    restartBtn.style.cssText = "display:none;margin:4px 0;";
    restartBtn.addEventListener("click", () => {
      invoke("extensions_restart_app").catch(() => {});
    });
    const showRestartNote = () => {
      restartNote.style.display = "block";
      restartBtn.style.display = "inline-block";
    };

    // Install row
    const installRow = document.createElement("div");
    installRow.className = "views-setting-row";
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = "Web Store URL or extension ID";
    input.className = "views-search interactive";
    input.style.cssText = "flex:1;";
    const installBtn = document.createElement("button");
    installBtn.className = "views-btn interactive";
    installBtn.textContent = "Install";
    const fileBtn = document.createElement("button");
    fileBtn.className = "views-btn interactive";
    fileBtn.textContent = "From file…";
    fileBtn.title = "Install an extension from a local .crx or .zip file";
    const storeBtn = document.createElement("button");
    storeBtn.className = "views-btn interactive";
    storeBtn.textContent = "🧩 Get extensions";
    storeBtn.title = "Browse the Chrome Web Store — the install banner appears on extension pages";
    storeBtn.addEventListener("click", () => {
      this.onNavigate("https://chromewebstore.google.com/");
      this.close();
    });
    const installStatus = document.createElement("span");
    installStatus.style.cssText = "font-size:0.6875rem;min-width:70px;";
    installRow.append(input, installBtn, fileBtn, storeBtn, installStatus);
    form.appendChild(installRow);

    const dropHint = document.createElement("div");
    dropHint.textContent = "Tip: you can also drag a .crx/.zip file onto the window to install it.";
    dropHint.style.cssText = "color:var(--text-dim);font-size:0.6875rem;margin-bottom:6px;";
    form.appendChild(dropHint);

    const listWrap = document.createElement("div");
    form.appendChild(listWrap);
    form.appendChild(restartNote);
    form.appendChild(restartBtn);

    const refresh = async () => {
      listWrap.innerHTML = "";
      const exts = await invoke<
        { id: string; version: string; name: string; enabled: boolean }[]
      >("extensions_list").catch(() => []);
      if (!exts.length) {
        const empty = document.createElement("div");
        empty.textContent = "No extensions installed.";
        empty.style.cssText = "color:var(--text-dim);font-size:0.75rem;padding:6px 0;";
        listWrap.appendChild(empty);
        return;
      }
      for (const ext of exts) {
        const row = document.createElement("div");
        row.className = "views-setting-row";
        const toggle = document.createElement("input");
        toggle.type = "checkbox";
        toggle.className = "interactive";
        toggle.checked = ext.enabled;
        toggle.addEventListener("change", () => {
          invoke("extensions_set_enabled", { id: ext.id, enabled: toggle.checked })
            .then(showRestartNote)
            .catch(() => {});
        });
        const name = document.createElement("span");
        // Fall back to a shortened id when the manifest name is unresolved.
        name.textContent = ext.name && ext.name !== ext.id ? ext.name : `${ext.id.slice(0, 16)}…`;
        name.style.cssText = "flex:1;";
        const ver = document.createElement("span");
        ver.textContent = ext.version;
        ver.style.cssText = "color:var(--text-dim);font-size:0.6875rem;";
        const options = document.createElement("button");
        options.className = "views-btn interactive";
        options.textContent = "Options";
        options.title = "Open this extension's settings page";
        options.disabled = !ext.enabled;
        options.addEventListener("click", async () => {
          try {
            await invoke("extensions_open_options", { id: ext.id });
          } catch (err) {
            alert(String(err));
          }
        });
        const remove = document.createElement("button");
        remove.className = "views-btn views-btn-danger interactive";
        remove.textContent = "Remove";
        remove.addEventListener("click", async () => {
          await invoke("extensions_uninstall", { id: ext.id }).catch(() => {});
          showRestartNote();
          refresh();
        });
        row.append(toggle, name, ver, options, remove);
        listWrap.appendChild(row);
      }
    };

    fileBtn.addEventListener("click", async () => {
      fileBtn.disabled = true;
      installStatus.textContent = "Installing…";
      installStatus.style.color = "var(--text-dim)";
      try {
        const ext = await invoke<{ name: string } | null>("extensions_install_file_dialog");
        if (ext) {
          installStatus.textContent = "Installed";
          installStatus.style.color = "var(--ok)";
          showRestartNote();
          refresh();
        } else {
          installStatus.textContent = "";
        }
      } catch (err) {
        installStatus.textContent = "Failed";
        installStatus.style.color = "var(--danger)";
        console.error(err);
      } finally {
        fileBtn.disabled = false;
      }
    });

    installBtn.addEventListener("click", async () => {
      const val = input.value.trim();
      if (!val) return;
      installBtn.disabled = true;
      installStatus.textContent = "Installing…";
      installStatus.style.color = "var(--text-dim)";
      try {
        await invoke("extensions_install", { crxIdOrUrl: val });
        installStatus.textContent = "Installed";
        installStatus.style.color = "var(--ok)";
        input.value = "";
        showRestartNote();
        refresh();
      } catch (err) {
        installStatus.textContent = "Failed";
        installStatus.style.color = "var(--danger)";
        console.error(err);
      } finally {
        installBtn.disabled = false;
      }
    });

    refresh();
  }
}
