# Jello

A fast, chromeless web browser for Windows built on [Tauri](https://tauri.app/)
and WebView2. Jello puts the page first — the entire window is the web page, and
all browser chrome (tabs, address bar, controls) floats above it as translucent
glass and gets out of your way.

> Status: **v0.4.0** — active development.

## Features

- **Chromeless, page-first UI.** The web page fills the whole window; floating
  glass pills provide tabs, navigation, and the address bar on demand.
- **Quick palette** (Alt+Space style) for search, history, bookmarks, and tabs,
  plus an in-window address bar (Ctrl+L).
- **Browser extensions that actually run.** Install Chrome extensions (e.g.
  uBlock Origin Lite) by Web Store URL or ID — they load into content webviews
  and block ads/trackers. Manage them in Settings; a banner offers one-click
  install when you visit the Chrome Web Store.
- **Native context menus** on pages, tabs, and the address bar.
- **Screenshot & OCR capture** with copy / save / search / Ask-AI actions.
- **Tab management** — virtualized tab panel, MRU switching (Ctrl+Tab),
  reopen-closed (Ctrl+Shift+T), pin, mute, drag-reorder, suspend-on-idle.
- **Incognito windows** with an isolated, non-recording session.
- **Per-host zoom** (Ctrl+scroll and Ctrl +/-/0), **always-on-top pin**, and
  **minimize-to-tray**.
- **Global hotkeys** — summon, palette, address bar, screenshot, OCR, incognito
  — all rebindable in Settings.
- **Secure auto-updater** via signed GitHub Releases.

## Tech stack

- **Shell:** Tauri 2 + wry (WebView2 on Windows)
- **Backend:** Rust — tab pool, WebView2 COM event handlers, SQLite (rusqlite)
- **Frontend:** TypeScript + Vite (no framework), a small overlay that renders
  the floating chrome and reports hit-rects to the shell for click pass-through

## Building from source

Prerequisites: [Rust](https://rustup.rs/), [Node.js](https://nodejs.org/) 20+,
and the WebView2 runtime (preinstalled on Windows 11).

```bash
npm install
npm run tauri dev      # run in development
npm run tauri build    # produce an NSIS installer in src-tauri/target/release/bundle
```

## Releases & updates

Tagged `v*` commits trigger the GitHub Actions workflow, which builds, signs, and
publishes an installer plus `latest.json` to GitHub Releases. The in-app updater
(Settings → "Check for updates") verifies the minisign signature before applying.

## License

See [LICENSE](LICENSE).
