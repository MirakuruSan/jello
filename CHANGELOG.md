# Changelog

All notable changes to Jello are documented here. This project adheres to
[Semantic Versioning](https://semver.org/) and the format is based on
[Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- **Tabs come back with their history.** Each tab keeps its back and forward
  pages and where you were scrolled on each. After restarting Jello,
  unloading a tab to save memory, reopening a closed tab or duplicating one,
  Back and Forward work as before and pages open where you left them.

### Changed
- **Pages rest while you can't see them.** When Jello's window is completely
  covered by other windows, on another virtual desktop, hidden in the tray or
  behind the lock screen, its pages stop drawing and videos stop decoding
  frames, as when the window is minimized. Audio keeps playing, and pages are
  back the moment the window is in view.
- **YouTube's Ambient mode is off by default.** The glow it draws behind
  videos keeps the whole page redrawing at the display's refresh rate;
  without it, YouTube used over a third less CPU during playback in our
  tests. Allow it again in Settings > Media.

### Fixed
- Fingerprinting protection now also hides installed fonts that a page names
  in its own style sheets (`@font-face` rules with `local()`), whether the
  rules are in the page, added by a script, or in a style sheet file from the
  same site. Style sheet files from other sites can't be read by the page and
  are left as they are.

## [0.4.14] — 2026-10-30

### Added
- **Fingerprinting protection, on by default.** Sites no longer see your
  graphics card (WebGL and WebGPU report a common one, with the capabilities
  of mainstream graphics hardware), the fonts you installed beyond the ones
  Windows comes with, where your window sits on screen or your battery state.
  Canvas, WebGL, audio, text and layout measurements and hardware details
  come back slightly varied instead of exact. The variations differ for every
  site and every time Jello starts, so sites can't use your device to
  recognize you across sites or visits. This covers embedded frames and every
  kind of web worker too. If a site doesn't work right, turn the protection
  off there from the ⋯ menu; Settings > Privacy & security lists those sites
  and switches the protection.
- Pages see Jello as the Edge browser it is built on: they can no longer
  detect the embedded WebView2 or Jello's own internals, client hints no
  longer mention WebView2, and pages are told one preferred language.

### Changed
- Edge's tracking prevention runs at its strict level: known trackers are
  kept from loading and from storing data on sites they are embedded in,
  also on sites where you pause the ad blocker.
- **The built-in ad blocker works from the first launch.** Jello now ships
  with a copy of the filter lists and uBlock Origin's scriptlets and updates
  them in the background. A page that opens while the blocker is still
  starting waits for it (a few seconds at most) instead of loading unfiltered.
  The shipped filter data keeps its own licenses; see `THIRD_PARTY_NOTICES.md`.

### Fixed
- Scriptlets against video ads and anti-adblock walls now take effect before
  a page's own scripts also after going back or forward, after a redirect to
  another site, after submitting a form and after a reload, and in embedded
  frames from other sites.
- Scriptlets that answer with one of uBlock Origin's placeholder files (such as
  an empty video ad response) now receive it, so those players behave as they
  do with the uBlock Origin extension.
- Pages can no longer see the ad blocker's element-hiding lookups in their
  performance data: the lookups no longer go over the network.

## [0.4.13] — 2026-10-30

### Added
- **Built-in ad blocker, on by default.** Jello now blocks ads and trackers
  itself, without an extension, using uBlock Origin's default filter lists
  (uBlock filters, EasyList, EasyPrivacy, Peter Lowe's list), which it
  downloads and keeps up to date. It blocks network requests (including in
  cross-site frames and service workers), hides ad placeholders and empty ad
  slots, runs uBlock Origin's scriptlets against video ads and anti-adblock
  walls, strips tracking parameters (`$removeparam`) and applies `$csp`
  filters. It also works in InPrivate windows. On the tested sites it blocks
  as much as the uBlock Origin extension with no extension process running,
  using about 80–150 MB less memory and slightly less CPU.
- **Settings > Ad blocking**: choose Built-in (recommended), the uBlock Origin
  extension, or Off. Only one ad blocker runs at a time, so switching to the
  built-in one turns uBlock Origin off, and choosing uBlock Origin installs it
  if needed.
- **My filters**: your own blocking and hiding rules in uBlock Origin syntax,
  edited in Settings.
- **⋯ menu**: how many requests were blocked on the page (opens the list of
  blocked requests), **Pause ad blocking on this site**, and **Hide an
  element…**, which turns the element you click into a filter in My filters.

### Changed
- The setup wizard's "Block ads & trackers" option uses the built-in blocker
  instead of installing uBlock Origin. Existing installs switch to the built-in
  blocker and turn uBlock Origin off; choose it again in Settings > Ad blocking
  to go back.

## [0.4.12] — 2026-10-29

### Added
- **F11 full screen** for the window, also in the ⋯ menu. While full screen,
  the maximize button exits it.
- **Resize from the top edge.** The top few pixels of the window resize it,
  as in any Windows app.

### Changed
- **Moving the window.** While the toolbar shows, its empty space works as the
  title bar: drag to move, double-click to maximize. A move starts at the exact
  point you grabbed, so the window no longer slips under the cursor, including
  when dragging a maximized or snapped window away.
- **A minimized window rests.** Its page stops painting and its timers are
  throttled, and every tab trims its memory; audio and picture-in-picture keep
  playing. With a YouTube video floating in picture-in-picture, minimizing cut
  Jello's memory use by about a fifth.

### Fixed
- Jello's toolbar no longer appears over a fullscreen video, and the top edge
  of a fullscreen page is clickable.
- Going full screen from a maximized window left an empty band at the bottom of
  the screen, and leaving full screen left the window covering the taskbar.
- F11 did nothing.
- The picture-in-picture window shows in the taskbar as part of Jello, with
  Jello's icon, instead of as a separate Microsoft Edge WebView2 entry.
- The closed find bar could still be reached with Tab and by screen readers.

## [0.4.11] — 2026-10-27

### Changed
- **New app icon:** a hand-drawn, translucent red jelly.
- **Toolbar reveal.** The toolbar appears when the pointer touches the
  window's top edge and hides shortly after the pointer leaves it, so the top
  of the page stays clickable. It now fades in at once instead of stepping in.
- **Keep window on top** has its own button in the address bar, next to the
  bookmark star.
- **Redesigned screen capture and OCR.** The frozen screen dims around the
  selection, with corner handles, a size readout and an action bar (Copy,
  Save, Pin, Search, Ask AI). Recognized text opens in an editable panel with
  Copy, Search and Ask AI. Enter captures the whole screen, Ctrl+C and Ctrl+S
  copy or save the selection, arrow keys nudge it and Esc cancels.

### Fixed
- The capture overlay could appear as an empty, transparent window. It now
  shows only once the screenshot has painted, takes keyboard focus on the
  monitor under the pointer, and no longer includes Jello's own window in the
  frozen image.
- Capture overlays were the wrong size on monitors with different display
  scaling.
- A capture hotkey could hide Jello without showing the overlay. A failed
  capture now always brings Jello back.
- Picture-in-picture "Back to tab" now restores Jello when it is minimized or
  in the tray, and switches to the video's tab.
- The palette hotkey closes the palette when it is open, and the palette
  always opens with the cursor in its search field.
- Clicking ⋯ again closes the menu. In short windows the menu scrolls instead
  of covering the toolbar.
- The toolbar no longer overlaps itself in narrow windows.

## [0.4.10] — 2026-10-27

### Added
- **Liquid-glass interface.** Toolbar pills, panels, menus and the palette
  share one glass material: a translucent tint, a rim that catches light, a
  specular top edge and soft depth. The new-tab page and the palette show a
  live blur of whatever is behind the window.
- **Settings → Appearance.** Theme (dark / light) and window material —
  Acrylic (live blur), Mica (wallpaper tint, lighter on the GPU) or Solid —
  applied immediately to every Jello window.
- Drawn icons in the toolbar, panels, find bar and lists, replacing text
  symbols.

### Changed
- **Reorganized toolbar.** Navigation on the left; the address in the center
  with page actions inside it (bookmark, picture-in-picture, and a zoom chip
  that appears when the page isn't at 100%); new tab, tab list, downloads and
  the menu on the right; window controls in their own group. Reload turns
  into Stop while a page loads.
- **A real main menu.** The ⋯ button opens a menu with icons and shortcuts:
  new and reopened tabs, history, bookmarks, downloads, find, an inline zoom
  control, picture-in-picture, screenshot, keep-on-top and settings. Menus
  work from the keyboard (arrows, Home/End, Enter, Esc).
- **Settings** gained a section index that follows the scroll, switches for
  on/off options, and consistent buttons, inputs and selects.

### Fixed
- History entries were sometimes named after the previous page when a tab
  navigated to a new one.
- The palette now follows the light/dark theme.

## [0.4.9] — 2026-10-25

### Added
- **Picture-in-picture.** Alt+P (or the ⧉ button that appears while a tab plays
  media) floats the page's video in a small always-on-top window. Switching
  tabs or minimizing Jello floats a playing video automatically and returning
  brings it back; turn this off under Settings → Media.
- **"Open with" and file types.** Jello registers itself with Windows as a web
  browser (http/https) and as an "Open with" handler for the file types you
  tick under Settings → Files & default browser (web pages and PDFs by default;
  images, audio/video and text/data available). Files opened this way, and
  files dropped on the window, open in a new tab.
- **Performance settings.** Choose how many tabs stay loaded, how soon
  background tabs freeze, and when idle tabs are unloaded entirely (memory
  saver, default 30 minutes). Background tabs also trim their memory as soon as
  you leave them.
- **Ask AI from the palette.** Every palette search offers "Ask Google AI Mode"
  and "Ask Duck.ai" rows next to the normal web search.
- Downloads: a top-bar progress pill, Open / Show in folder buttons, and
  readable failure reasons.
- Most-visited sites on the new-tab page.

### Fixed
- **Downloads finish.** Closing a tab — or Jello unloading it — cancelled any
  download it had started, leaving a truncated file under the real name. Such
  tabs now keep downloading in the background until the file is done.
- **No more duplicate downloads.** The blank-tab recovery mistook a download
  for a lost page load and requested the file again; it now waits for slow
  pages and ignores downloads. Tabs that exist only to download a file close
  automatically, so they no longer sit empty or re-download on restore.
- **Pause / Resume work.** Pausing used to be treated as a failed download, so
  Resume could never find it again. Interrupted downloads can now be retried.
- **Google AI Overviews show in search results.** The built-in uBlock Origin
  blocked the request that loads them ("Can't generate an AI overview right
  now"). Jello adds a single exception for that request; ads and trackers stay
  blocked.
- **Closing a tab switches to the tab you used last that is still loaded**
  instead of loading an unloaded neighbour; with none loaded, the new-tab page
  shows.
- The palette searched DuckDuckGo regardless of the chosen search engine.
- Links handed to Jello by other apps now open in a new tab instead of
  replacing the page you were on.
- An extension settings page that closes itself no longer leaves an empty
  window behind.
- **Settings that did nothing.** "Save & autofill passwords", "Autofill
  addresses & forms" and "Clear history on exit" now take effect. The "Block
  ads" and "Search suggestions" switches had no effect and are removed; ad
  blocking is the uBlock Origin entry under Extensions.
- A tab that only downloaded a file no longer comes back (and downloads again)
  with Reopen closed tab.

## [0.4.8] — 2026-07-17

### Changed
- **New windows and incognito are temporarily disabled.** Both need per-window
  tab state to work correctly — opening a new window could blank every other
  window, and incognito was unreliable. They are gated out of stable builds
  until rebuilt: the shortcuts and settings entries are hidden, attempts show a
  notice, and anything that used to open a new window opens a tab instead.

## [0.4.7] — 2026-07-17

### Fixed
- **Restored tabs load reliably on startup.** Loading extensions during tab
  creation was cancelling the tab's own first navigation — the restored tab sat
  on a blank page that even Reload couldn't fix. Extensions are now applied to
  the browser profile once at startup (and on install/enable/disable), never
  while a page is loading; as a safety net, a tab that still ends up blank is
  detected and re-navigated automatically, and Reload recovers it too.
- **Incognito works again.** Incognito content webviews were created with a
  mismatched browser-environment setting, so they came up dead — opening any
  website in an incognito window did nothing. New incognito tabs now load
  normally (still with an isolated, extension-free, non-recording session).

## [0.4.6] — 2026-07-16

### Fixed
- **Pages load much faster.** Extensions removed or disabled in the past kept
  silently running in the browser profile — two copies of uBlock Origin plus
  several stale extensions were filtering every single request. The profile is
  now kept in sync with your extension list, at startup and immediately on
  disable/uninstall.
- **"Restore tabs on startup" no longer lands on a stuck blank page.** A failed
  restore is retried automatically, and Reload now recovers a tab whose page
  never loaded (it previously did nothing on such a tab).
- **Jello recovers after PC sleep.** If Windows kills the browser engine while
  the PC sleeps (or it dies at any other time), Jello detects it and restarts
  itself automatically — no more dead, invisible window until a manual restart.
  Global hotkeys are also re-registered immediately on resume.

## [0.4.5] — 2026-07-16

### Fixed
- **Pages stay interactive while a panel is open.** The tab list / settings
  panel no longer blocks clicking the page around it.
- **Faster browsing.** Redundant stacked ad blockers (uBO Lite, AdGuard) are
  disabled automatically when the full uBlock Origin is active — every request
  was paying multiple filter passes. Also fixed a case where the hidden-mode
  memory trim could stay active after summoning, leaving everything sluggish.
- **"Restore tabs on startup" works.** With it enabled, Jello reopens on your
  most recent tab instead of an empty home screen.
- **Global-hotkeys settings section actually shows the hotkeys** (rows were
  being appended to the bottom of the settings form).
- **Tab search field fits the panel.**
- Unloading the last loaded tab now lands on the home screen instead of waking
  an unloaded tab.

### Added
- **Tab state dots**: green = loaded, amber = suspended, grey = unloaded; the
  right-click "Unload" entry is greyed out for tabs that are already unloaded.
- **"Get extensions" button** (Settings → Extensions) opens the Chrome Web
  Store, where the install banner takes it from there.

### Known limitation
- Per-tab back/forward history cannot be carried across a full restart
  (WebView2 offers no API to save or restore a tab's navigation stack). Your
  global History (Ctrl+H) and the tabs themselves do persist.

## [0.4.4] — 2026-07-15

### Fixed
- **Keyboard focus follows you everywhere.** Summoning the window, Ctrl+Alt+L,
  Ctrl+T, address-bar navigation, and tab switching now put the keyboard where
  you expect (page or search pill) — no more clicking first before you can type.
- **Settings and tab panels are readable.** Panels get a solid background so
  page text no longer bleeds through the glass.

### Changed
- **Lighter when hidden.** Hiding Jello immediately trims Chromium memory (media
  and downloads keep running), and after a minute hidden, background tabs are
  suspended — playing audio and the active tab are never touched. Memory returns
  to full performance on summon.
- Removed unused code and an unused dependency.
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
- Extension download (HTTPS with a Chrome User-Agent) and zip extraction run
  in-process in Rust instead of shelling out to PowerShell.

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
