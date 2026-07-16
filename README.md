# Jello

A fast, chromeless web browser for Windows built on [Tauri](https://tauri.app/)
and WebView2. Jello puts the page first — the entire window is the web page, and
all browser chrome (tabs, address bar, controls) floats above it as translucent
glass and gets out of your way, uses global hotkeys and shortcuts to summun and navigate it quickly. It has Zero telemetry, zero bloat, no nonsense, built for privacy.

I mainly build this for my own use, I wanted something lightweight that I can summon instantly using a hotkey from anywhere everytime I wanted to look up or research something. I was annoyed by the time it took to launch other browsers and the memory resources they were using.

> Status: **v0.4.7** — active development.

## Features

- **Chromeless, page-first UI.** The web page fills the whole window; floating
  glass pills provide tabs, navigation, and the address bar on demand.
- **Quick palette** (Alt+Space style) for search, history, bookmarks, and tabs,
  plus an in-window address bar (Ctrl+L).
- **Browser extensions that actually run.** Install Chrome extensions (e.g.
  uBlock Origin) by Web Store or Edge Add-ons URL, from a local .crx/.zip file,
  or by drag-and-drop — they load into content webviews and block ads/trackers.
  Manage them in Settings; a banner offers one-click install on store pages.
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
