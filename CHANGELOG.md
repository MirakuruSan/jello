# Changelog

All notable changes to Jello are documented here. This project adheres to
[Semantic Versioning](https://semver.org/) and the format is based on
[Keep a Changelog](https://keepachangelog.com/).

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
