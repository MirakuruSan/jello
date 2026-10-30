# Third-party notices

Jello is licensed under the Apache License 2.0 (see `LICENSE`). It includes or ships
with the third-party material below, each under its own license.

## Filter data shipped alongside Jello

The installer includes a snapshot of filter lists and uBlock Origin resource files in
`resources/content-blocker/` (source tree: `src-tauri/resources/content-blocker/`). They
are separate data files that Jello's built-in ad blocker reads at runtime. They are not
compiled into Jello and are not covered by Jello's license; they remain under their own
licenses, unmodified. The folder's `README.md` lists each file with its source, and
`LICENSE-GPL-3.0.txt` holds the license text.

- **uBlock Origin filter lists** (uAssets): GPL-3.0. https://github.com/uBlockOrigin/uAssets
- **EasyList and EasyPrivacy**: dual-licensed GPL-3.0 or CC BY-SA 3.0, distributed here
  under GPL-3.0. https://easylist.to/
- **uBlock Origin scriptlets and redirect resources**: GPL-3.0. https://github.com/gorhill/uBlock

Jello also downloads these lists, and Peter Lowe's ad server list
(https://pgl.yoyo.org/adservers/), at runtime to keep them current.

## Libraries and data compiled into Jello

- **adblock-rust** (`adblock` crate) by Brave Software: MPL-2.0.
  https://github.com/brave/adblock-rust
- **Public Suffix List** by Mozilla, used by fingerprinting protection to tell
  sites apart: MPL-2.0. The unmodified list is at
  `src-tauri/src/fingerprint/public_suffix_list.dat`. https://publicsuffix.org/

Other Rust and npm dependencies are listed in `src-tauri/Cargo.lock` and
`package-lock.json` with their licenses.
