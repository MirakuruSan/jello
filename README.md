# Jello

A fast, chromeless web browser for Windows built on [Tauri](https://tauri.app/)
and WebView2. Jello puts the page first: the whole window is the web page, and
the browser chrome (tabs, address bar, controls) floats above it as translucent
glass that stays out of the way. Global hotkeys summon it from anywhere, so it
works as an instant research tool rather than a heavyweight browser.

No telemetry. Screenshot search and OCR text capture are built in, and Chrome
extensions are supported.

> Status: **v0.4.10** — active development.

## Features

- **Chromeless, page-first UI.** The web page fills the whole window; floating
  glass pills provide tabs, navigation, and the address bar on demand.
- **Liquid-glass look** with a live Windows Acrylic blur behind the new-tab page
  and palette (or Mica / Solid), in dark and light themes.
- **Quick palette** (Alt+Space style) for search, history, bookmarks, and tabs,
  with one-step "Ask Google AI Mode" / "Ask Duck.ai" rows, plus an in-window
  address bar (Ctrl+L).
- **Browser extensions that actually run.** Install Chrome extensions (e.g.
  uBlock Origin) by Web Store or Edge Add-ons URL, from a local .crx/.zip file,
  or by drag-and-drop. Manage them in Settings; a banner offers one-click
  install on store pages.
- **Picture-in-picture.** Alt+P floats a video; switching tabs or minimizing
  floats a playing video automatically.
- **Low memory use.** Configurable limit on loaded tabs, background tabs that
  freeze and trim memory when idle, and a memory saver that unloads tabs you
  haven't used in a while. Tabs playing audio, downloading, or in
  picture-in-picture are left alone.
- **Downloads** with pause / resume / retry, progress in the top bar, and
  downloads that keep going after their tab is closed.
- **Windows integration.** Registers as a web browser and as an "Open with"
  handler for the file types chosen in Settings (web pages and PDFs by default).
- **Native context menus** on pages, tabs, and the address bar.
- **Screenshot & OCR capture** with copy / save / search / Ask-AI actions.
- **Tab management** — virtualized tab panel, MRU switching (Ctrl+Tab),
  reopen-closed (Ctrl+Shift+T), pin, mute, drag-reorder, unload.
- **Per-host zoom** (Ctrl+scroll and Ctrl +/-/0), **always-on-top pin**, and
  **minimize-to-tray**.
- **Global hotkeys** — summon, palette, address bar, screenshot, OCR — all
  rebindable in Settings.
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

Development builds don't register Jello with Windows (protocol handler,
"Open with", Default Apps); set `JELLO_SHELL_REGISTER=1` to test that path.

## Releases & updates

Tagged `v*` commits trigger the GitHub Actions workflow, which builds, signs, and
publishes an installer plus `latest.json` to GitHub Releases. The in-app updater
(Settings → "Check for updates") verifies the minisign signature before applying.

## License

See [LICENSE](LICENSE).
