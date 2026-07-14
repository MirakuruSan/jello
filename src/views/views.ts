// M6: History / Bookmarks / Downloads / Settings panel.
import { invoke } from "@tauri-apps/api/core";

interface HistoryEntry { id: number; url: string; title: string; visitCount: number; lastVisit: number; }
interface Bookmark { id: number; url: string; title: string; position: number; }
export interface DownloadItem { id: string; fileName: string; url: string; state: string; }

type ViewName = "history" | "bookmarks" | "downloads" | "settings";

export class ViewsController {
  private panel: HTMLElement;
  private content: HTMLElement;
  private current: ViewName = "history";
  private downloads: DownloadItem[] = [];
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

  private async render(): Promise<void> {
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
        del.textContent = "×";
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
      del.textContent = "×";
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
    for (const d of this.downloads) {
      const row = this.rowHtml(d.fileName, d.url);
      const state = document.createElement("span");
      state.className = "views-dl-state";
      state.textContent = d.state;
      row.appendChild(state);
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

    addToggle("passwordAutosave", "Save & autofill passwords", true);
    addToggle("generalAutofill", "Autofill addresses & forms", true);
    addToggle("adblock", "Block ads & trackers (uBlock Origin Lite)", true);
    addToggle("searchSuggestions", "Search suggestions (sends keystrokes to engine)", false);
    addToggle("clearHistoryOnExit", "Clear history on exit", false);
    addToggle("restoreSession", "Restore tabs on startup", false);
    addToggle("allowFileUrls", "Allow local file:// URLs", false);

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

    const updaterEnabled = await invoke<boolean>("updater_enabled").catch(() => false);
    if (updaterEnabled) {
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

    // --- Extensions ---
    this.renderExtensionsSection(form);

    // --- Global hotkeys (rebindable) ---
    const hkHeader = document.createElement("div");
    hkHeader.textContent = "Global hotkeys";
    hkHeader.style.cssText = "margin-top:14px;font-weight:600;font-size:0.8125rem;";
    form.appendChild(hkHeader);
    const hkNote = document.createElement("div");
    hkNote.textContent = "Format: Ctrl+Alt+K, Ctrl+Shift+Space, … Press Apply to save; conflicts are rejected.";
    hkNote.style.cssText = "color:var(--text-dim);font-size:0.6875rem;margin-bottom:4px;";
    form.appendChild(hkNote);

    const hkLabels: Record<string, string> = {
      summon: "Summon / hide Jello",
      palette: "Open quick palette",
      addressbar: "Open address bar",
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
          form.appendChild(row);
        }
      })
      .catch(() => {});

    // --- In-window shortcuts (reference) ---
    const swHeader = document.createElement("div");
    swHeader.textContent = "In-window shortcuts";
    swHeader.style.cssText = "margin-top:14px;font-weight:600;font-size:0.8125rem;";
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
      ["New window", "Ctrl+N"],
      ["New incognito window", "Ctrl+Shift+N"],
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

    const wipe = document.createElement("button");
    wipe.className = "views-btn views-btn-danger interactive";
    wipe.textContent = "Wipe all history";
    wipe.addEventListener("click", async () => {
      await invoke("history_delete", { mode: "all" }).catch(() => {});
    });
    form.appendChild(wipe);

    const rerun = document.createElement("button");
    rerun.className = "views-btn interactive";
    rerun.textContent = "Run first-time setup again";
    rerun.addEventListener("click", () => {
      invoke("settings_set", { patch: { wizardComplete: false } }).catch(() => {});
      window.dispatchEvent(new CustomEvent("jello:run-wizard"));
    });
    form.appendChild(rerun);

    this.content.appendChild(form);
  }

  private renderExtensionsSection(form: HTMLElement): void {
    const header = document.createElement("div");
    header.textContent = "Extensions";
    header.style.cssText = "margin-top:14px;font-weight:600;font-size:0.8125rem;";
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
    const showRestartNote = () => {
      restartNote.style.display = "block";
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
    const installStatus = document.createElement("span");
    installStatus.style.cssText = "font-size:0.6875rem;min-width:70px;";
    installRow.append(input, installBtn, installStatus);
    form.appendChild(installRow);

    const listWrap = document.createElement("div");
    form.appendChild(listWrap);
    form.appendChild(restartNote);

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
        const remove = document.createElement("button");
        remove.className = "views-btn views-btn-danger interactive";
        remove.textContent = "Remove";
        remove.addEventListener("click", async () => {
          await invoke("extensions_uninstall", { id: ext.id }).catch(() => {});
          showRestartNote();
          refresh();
        });
        row.append(toggle, name, ver, remove);
        listWrap.appendChild(row);
      }
    };

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
