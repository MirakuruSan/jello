# Changelog

All notable changes to Jello are documented here. This project adheres to
[Semantic Versioning](https://semver.org/) and the format is based on
[Keep a Changelog](https://keepachangelog.com/).

## [0.4.4] — 2026-07-15

### Fixed
- **The main window can no longer get permanently stuck hidden.** Summon, tray
  click, and relaunch now verify visibility at the OS level and fall back to a
  raw Win32 show/hide, so a degraded runtime state (where the window plumbing
  silently no-ops) self-heals instead of leaving the window unreachable.
- **Global hotkeys are reliable and self-healing.** The keypress path no longer
  does a database round trip; re-registration is serialized so rebinding one
  hotkey can't drop the others, and a background watchdog re-registers any
  hotkey the OS silently dropped within a minute. Failed registrations now
  surface a toast instead of failing silently.
- **The leader/chord key hook no longer eats keystrokes system-wide.** It caches
  its shortcuts (no database access inside the low-level keyboard hook), installs
  on a thread that pumps messages, and passes modifiers and unrelated keys
  through instead of swallowing them.
- **Extensions load reliably and the options page opens.** Extension loading was
  rebuilt on explicit per-extension WebView2 calls that capture each extension's
  real runtime id, and the staging folder is no longer rewritten underneath a
  live session (the cause of "Extension isn't loaded" and the options window
  landing on an error page). A real **Restart Jello** button was added.
- **Maximizing fills the window on every path.** Content now resizes correctly
  whether you maximize with the button, Win+Up, or by dragging the title to the
  top of the screen.
- **Screenshots no longer leave a Jello ghost or a stuck-hidden window.** The
  hide/restore list is merged instead of overwritten so a rapid double-trigger
  can't lose it, with a raw fallback for the main window.

### Added
- **Install extensions from a file or by drag-and-drop.** Drop a `.crx`/`.zip`
  onto the window, or use the new "From file…" button (Settings → Extensions).
  Single-root release zips (like uBlock Origin's) are handled.
- **Microsoft Edge Add-ons store support.** Installing from a
  `microsoftedge.microsoft.com/addons` URL works, and the install banner appears
  on those pages too.
- **Per-site zoom that persists.** Your zoom level is remembered per website and
  re-applied automatically on return and after restarting, with a transient
  percentage indicator while you adjust.
- **Downloads you can control.** Pause, resume, and cancel downloads with a live
  progress bar, and open the containing folder when they finish.
- **A diagnostic log** at `%APPDATA%\Jello\logs\jello.log` so field issues are
  traceable.

## [0.4.3] — 2026-07-15

### Fixed
- **Window reopens from the tray again.** Clicking the tray icon (or "Show
  Jello") after closing to tray now reliably brings the window back — the show
  was running off the main thread and silently failing.
- **Hotkeys no longer "stop working" mid-session.** Tab/window shortcuts
  (Ctrl+T, Ctrl+W, Ctrl+N, …) now work whether the page or the chrome/new-tab
  page has focus, instead of only when a web page was focused. Ctrl+W closes the
  tab.
- **Extension Options page opens** instead of showing ERR_BLOCKED_BY_CLIENT: the
  window no longer re-navigates to the extension page (which Chromium blocks)
  once the initial load has succeeded.
- **Installing extensions by Web Store URL works.** The 32-character extension
  ID is now extracted correctly from modern `/detail/<slug>/<id>` URLs (it was
  grabbing the slug and failing).
- **Screenshots no longer capture a ghost Jello window.** Jello's own windows
  are hidden before the screen is grabbed and restored afterward.
- **Maximizing resizes page content** to fill the window instead of leaving
  pages at the pre-maximize size.
- **The new-tab search pill no longer lingers** over a loaded website after
  navigating away from the new-tab page.
- **Global hotkey rows are never blank** — the list always falls back to the
  full set of defaults.

### Changed
- The Settings "Run first-time setup again" button is now **"Configure"**, and
  the redundant "Wipe all history" was removed from Settings (it lives in the
  History tab as "Clear all").

### Added
- **Bookmark star** in the top bar to bookmark/un-bookmark the current page
  (Ctrl+D also toggles); the star reflects whether the page is bookmarked.

## [0.4.2] — 2026-07-15

### Fixed
- **Close button now minimizes to tray (or quits).** Clicking close, or pressing
  Ctrl+Q, reliably hides the window to the system tray (default) or exits, instead
  of doing nothing. Window-control commands now run asynchronously so they can't
  deadlock the WebView2 IPC handler.
- **Show/hide hotkey summons the window back.** Toggling with the global hotkey now
  brings a hidden or minimized window back and focuses it, instead of hiding it and
  leaving the app unresponsive until killed from Task Manager.
- **Address-bar hotkey opens the in-window address bar** (Ctrl+L / F6) rather than
  the command palette.
- **"Open in new window" opens a real new window.** New and incognito windows are
  built on the main thread and force-resized so they appear at the correct size and
  position instead of coming up hidden or 0×0.
- **Extension Options window opens reliably.** The dashboard now navigates to the
  extension page after the extension has finished loading, fixing a race where the
  window opened blank.
- **Tab count is correct on launch.** The tab-count badge reflects the real number
  of tabs immediately, instead of showing "1" until the tab panel is first opened.

## [0.4.1] — 2026-07-14

### Fixed
- **Screenshot / OCR overlay not appearing.** Enabling browser extensions on the
  main window in 0.4.0 left the capture and pinned-image windows with a mismatched
  setting, so WebView2 refused to create their webview and no overlay showed. All
  windows now agree on the setting.

### Changed
- **Settings reorganized** into clear sections (Browsing, Privacy & security,
  Startup & window, Updates, Extensions, hotkeys, Setup & data) with headers,
  instead of one long flat list.

### Added
- **Extension settings in their own window.** The Extensions panel now has an
  "Options" button that opens an installed extension's dashboard/settings page
  (e.g. uBlock Origin Lite's dashboard) in a dedicated window. The runtime
  extension id is derived deterministically from its load path, and the page is
  hosted in a top-level window (content tabs can't host non-web-accessible
  extension pages).

## [0.4.0] — 2026-07-14

Major polish release closing the remaining post-Gemini bug reports. Highlights:
browser extensions now actually run and block ads, tab titles/URLs stay live,
and right-click / zoom / pin all work.

### Added
- **Browser extensions load and run.** Installed extensions (uBlock Origin Lite,
  etc.) are now loaded into content webviews and actively filter — verified live
  that ad-serving requests are blocked. New **Settings → Extensions** panel:
  install by Web Store URL/ID, enable/disable, remove. Navigating to a Chrome
  Web Store page shows an "Install into Jello?" banner (the store's own button
  can't work in WebView2).
- **Native context menus.** Right-click on a page gives the full default menu
  (Back/Reload/Copy/Save image/Inspect); right-click a tab row → Duplicate / Pin
  / Mute / Close others / Close; right-click the domain pill → Copy URL / Paste
  and go.
- **Ctrl+scroll zoom** on pages, synced with the per-host zoom store.
- **Always-on-top pin** button in the top bar.
- **Top loading bar** that tracks page navigation.
- **Minimize-to-tray**: closing the window keeps Jello in the tray (toggle in
  Settings); tray left-click restores the window with tabs intact.

### Changed
- **Live tab titles & URLs.** Titles/URLs now update from WebView2 events instead
  of only on evict/suspend, so the tab panel and palette always show reality.
- **Faster, smoother tabs.** Replaced the 250ms per-tab poller (which locked the
  tab pool N times/sec) with event-driven updates + a light scroll-only poller;
  collapsed the new-tab DB round-trips; debounced tab-list reloads.
- **Quick palette** redesigned as a single composer pill and now always fronts
  the main window when opening a result; empty titles fall back to the host.

### Fixed
- Screenshot/OCR "search" and "Ask AI" actions now front the main window so the
  results tab is visible.
- Palette "new window" used an inconsistent window id and skipped overlay
  plumbing; find-bar Enter/Shift+Enter now navigate matches.

## [0.3.3] — 2026-07-14

First tracked release. Focus: polish, correctness, and closing gaps from the
post-Gemini bug reports.

### Added
- **In-app address bar.** Ctrl+L / F6 / clicking the domain pill now opens an
  inline URL editor inside the window (like any other browser) instead of a
  detached palette window.
- **Tab-switching hotkeys.** Alt+← / Alt+→ move between tabs (prev/next, with
  wrap-around).
- **Zoom hotkeys.** Ctrl+= / Ctrl+− / Ctrl+0 (including numpad +, −, 0) zoom the
  active page; also handled when the overlay has focus.
- **In-window shortcut reference** in Settings listing every active shortcut.
- Robust in-Rust extension download (HTTPS with a Chrome User-Agent) and
  in-process zip extraction — no more PowerShell shell-outs.

### Changed
- Dedicated browser Back/Forward keys (mouse thumb buttons, media keys) are now
  reserved for **history** navigation, matching muscle memory; Alt+Arrow is
  tab-switching instead.
- uBlock Origin Lite auto-installs silently during first-run setup when the
  ad-blocking checkbox is left on (the checkbox is the consent — no extra
  dialog).

### Fixed
- **Extensions failed to download.** Replaced the fragile PowerShell
  `Invoke-WebRequest` / `Expand-Archive` pipeline with a reliable async HTTP
  download + in-process unzip; the CRX endpoint now receives the User-Agent it
  requires.
- **uBlock Origin not installed during setup.** The wizard's auto-install path
  no longer blocks on a confirmation dialog and no longer risks deadlocking.
- **Random freeze after extended use.** Extension install commands are now async
  (they no longer run blocking work inside the WebView2 IPC handler), and
  webview creation on the main thread now has a bounded 15s timeout so a wedged
  main thread can never hold the tab-pool lock forever.

### Notes
- Rebinding in-window shortcuts (beyond the global hotkeys already in Settings)
  is planned for a future release via a configurable keymap.
