//! Session history of content tabs. Each tab keeps its back and forward
//! entries (URL, title, scroll position), so a tab that loads again after a
//! restart, or after being unloaded to save memory, comes back with them.
//!
//! WebView2 cannot be handed a saved history, so a restored tab first loads a
//! page Jello answers itself, on a host that never reaches the network. Jello
//! turns that page into one same-document entry per saved page, moves to the
//! current one and replaces it with the real page. The other entries stay in
//! the tab's history: going back or forward to one loads the same page again,
//! which replaces itself with the page the entry stands for.
//!
//! Each entry is added with a user gesture (`build_steps`): Chromium's Back
//! button skips entries a page adds on its own, so that pages cannot trap
//! the user, and the saved entries would be unreachable.

use serde::{Deserialize, Serialize};

/// Origin of the restore page. Names under `.invalid` never resolve, and
/// Jello answers every request to it before it would be sent.
pub const RESTORE_ORIGIN: &str = "https://restore.jello.invalid";

/// Entries kept per tab: the current one and the closest around it.
const MAX_ENTRIES: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub scroll: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    pub index: usize,
    pub entries: Vec<Entry>,
}

/// Pages a tab can come back to: web pages and local files.
pub fn is_restorable(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://") || url.starts_with("file:")) && !is_restore_url(url)
}

impl History {
    /// A stored history, if it is usable.
    pub fn parse(json: &str) -> Option<History> {
        let h: History = serde_json::from_str(json).ok()?;
        (h.index < h.entries.len() && h.entries.iter().all(|e| is_restorable(&e.url))).then_some(h)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(self.index)
    }

    /// A history from the entries of a live tab (`Page.getNavigationHistory`):
    /// restore-page entries stand for the pages they replace themselves with,
    /// and entries Jello cannot load again (about:blank) are left out.
    /// `scroll` gives the scroll position recorded for an entry id.
    pub fn from_navigation(reply: &serde_json::Value, scroll: impl Fn(i64) -> Option<f64>) -> Option<History> {
        let current = reply.get("currentIndex")?.as_u64()? as usize;
        let mut history = History::default();
        let mut index = None;
        for (i, e) in reply.get("entries")?.as_array()?.iter().enumerate() {
            let url = e.get("url").and_then(|v| v.as_str()).unwrap_or_default();
            let entry = match entry_of_restore_url(url) {
                Some(entry) => Some(entry),
                None if is_restorable(url) => Some(Entry {
                    url: url.to_string(),
                    title: e.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    scroll: 0.0,
                }),
                None => None,
            };
            if let Some(mut entry) = entry {
                if let Some(y) = e.get("id").and_then(|v| v.as_i64()).and_then(&scroll) {
                    entry.scroll = y;
                }
                history.entries.push(entry);
            }
            // The current entry, or the closest kept entry before it.
            if i <= current && !history.entries.is_empty() {
                index = Some(history.entries.len() - 1);
            }
        }
        history.index = index?;
        Some(history.trimmed())
    }

    /// At most MAX_ENTRIES, dropping the entries farthest from the current one.
    pub fn trimmed(mut self) -> History {
        while self.entries.len() > MAX_ENTRIES {
            if self.index >= self.entries.len() - 1 - self.index {
                self.entries.remove(0);
                self.index -= 1;
            } else {
                self.entries.pop();
            }
        }
        self
    }

    /// The history after navigating to `url` from the current entry: forward
    /// entries are dropped and `url` becomes the current entry.
    pub fn navigated(mut self, url: &str) -> History {
        self.entries.truncate(self.index + 1);
        self.entries.push(Entry { url: url.to_string(), ..Default::default() });
        self.index = self.entries.len() - 1;
        self.trimmed()
    }
}

pub fn is_restore_url(url: &str) -> bool {
    url.starts_with(RESTORE_ORIGIN) && url[RESTORE_ORIGIN.len()..].starts_with('/')
}

/// The restore page before it holds any entry; a restored tab opens it and
/// Jello runs `build_steps` in it once it has loaded.
pub fn start_url() -> String {
    format!("{RESTORE_ORIGIN}/")
}

/// The restore page's entry for a saved page.
pub fn entry_url(entry: &Entry) -> String {
    let mut url = tauri::Url::parse(&start_url()).expect("restore origin is a valid URL");
    url.set_path("/entry");
    url.query_pairs_mut()
        .append_pair("u", &entry.url)
        .append_pair("t", &entry.title)
        .append_pair("y", &entry.scroll.to_string());
    url.into()
}

/// Scripts that make the restore page hold `history`, run one after
/// another, each with whether it needs a user gesture: one entry per saved
/// page (every added one with a gesture of its own), then a move to the
/// current entry, which replaces itself with its page.
pub fn build_steps(history: &History) -> Vec<(String, bool)> {
    let js = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut steps: Vec<(String, bool)> = history
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let call = if i == 0 { "replaceState" } else { "pushState" };
            (format!("history.{call}(null, \"\", {})", js(&entry_url(e))), i > 0)
        })
        .collect();
    let back = history.entries.len().saturating_sub(1 + history.index);
    let go = format!("location.replace({})", history.current().map(|e| js(&e.url)).unwrap_or_default());
    steps.push((
        if back > 0 {
            format!("addEventListener(\"popstate\", () => {go}, {{ once: true }}); history.go(-{back})")
        } else {
            go
        },
        false,
    ));
    steps
}

/// The page an entry of the restore page stands for: its URL, title and
/// scroll position, from the entry's own URL.
pub fn entry_of_restore_url(url: &str) -> Option<Entry> {
    if !is_restore_url(url) {
        return None;
    }
    let parsed = tauri::Url::parse(url).ok()?;
    let mut entry = Entry::default();
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "u" => entry.url = value.into_owned(),
            "t" => entry.title = value.into_owned(),
            "y" => entry.scroll = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    is_restorable(&entry.url).then_some(entry)
}

/// The restore page, for any request to RESTORE_ORIGIN. Opened at an entry's
/// URL, it replaces itself with that entry's page. It is never stored, so
/// going back to an entry always loads it again.
pub fn page(url: &str) -> String {
    let title = entry_of_restore_url(url).map(|e| e.title).unwrap_or_default();
    format!(
        r#"<!doctype html><meta charset="utf-8"><meta name="referrer" content="no-referrer"><meta name="color-scheme" content="light dark"><link rel="icon" href="data:,"><title>{}</title><script>{}</script>"#,
        escape_html(&title),
        RESTORE_JS
    )
}

const RESTORE_JS: &str = r#"(() => {
  const url = new URLSearchParams(location.search).get("u") || "";
  if (/^(https?|file):/i.test(url)) location.replace(url);
})();"#;

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> Entry {
        Entry { url: url.to_string(), ..Default::default() }
    }

    #[test]
    fn restore_entries_stand_for_their_pages() {
        let url = format!("{RESTORE_ORIGIN}/entry?u=https%3A%2F%2Fa.example%2Fx%3Fq%3D1&t=A+%26+B&y=640");
        let e = entry_of_restore_url(&url).unwrap();
        assert_eq!(e.url, "https://a.example/x?q=1");
        assert_eq!(e.title, "A & B");
        assert_eq!(e.scroll, 640.0);
        // Only pages a tab can load again.
        assert!(entry_of_restore_url(&format!("{RESTORE_ORIGIN}/entry?u=javascript%3Aalert(1)")).is_none());
        assert!(entry_of_restore_url("https://restore.jello.invalid.example/entry?u=https%3A%2F%2Fa.example").is_none());
    }

    #[test]
    fn live_history_skips_blank_entries_and_keeps_the_current_one() {
        let reply = serde_json::json!({
            "currentIndex": 3,
            "entries": [
                { "id": 1, "url": "about:blank", "title": "" },
                { "id": 2, "url": format!("{RESTORE_ORIGIN}/entry?u=https%3A%2F%2Fa.example%2F&t=A&y=10"), "title": "A" },
                { "id": 3, "url": "https://b.example/", "title": "B" },
                { "id": 4, "url": "https://c.example/", "title": "C" },
                { "id": 5, "url": "https://d.example/", "title": "D" },
            ]
        });
        let h = History::from_navigation(&reply, |id| (id == 4).then_some(300.0)).unwrap();
        assert_eq!(h.index, 2);
        let urls: Vec<_> = h.entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(urls, ["https://a.example/", "https://b.example/", "https://c.example/", "https://d.example/"]);
        assert_eq!(h.entries[0].scroll, 10.0);
        assert_eq!(h.entries[2].scroll, 300.0);

        // On about:blank before any page: nothing to keep.
        let blank = serde_json::json!({ "currentIndex": 0, "entries": [{ "id": 1, "url": "about:blank", "title": "" }] });
        assert!(History::from_navigation(&blank, |_| None).is_none());
    }

    #[test]
    fn long_histories_keep_the_entries_around_the_current_one() {
        let entries: Vec<_> = (0..80).map(|i| entry(&format!("https://e.example/{i}"))).collect();
        let h = History { index: 70, entries }.trimmed();
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(h.current().unwrap().url, "https://e.example/70");
        assert_eq!(h.entries.last().unwrap().url, "https://e.example/79");
    }

    #[test]
    fn navigating_drops_forward_entries() {
        let h = History { index: 0, entries: vec![entry("https://a.example/"), entry("https://b.example/")] };
        let h = h.navigated("https://c.example/");
        assert_eq!(h.index, 1);
        assert_eq!(h.entries[1].url, "https://c.example/");
        assert_eq!(h.entries.len(), 2);
    }

    #[test]
    fn stored_histories_round_trip_and_bad_ones_are_refused() {
        let h = History { index: 1, entries: vec![entry("https://a.example/"), entry("file:///C:/x.pdf")] };
        assert_eq!(History::parse(&h.to_json()), Some(h));
        assert!(History::parse(r#"{"index":2,"entries":[{"url":"https://a.example/"}]}"#).is_none());
        assert!(History::parse(r#"{"index":0,"entries":[{"url":"javascript:alert(1)"}]}"#).is_none());
        assert!(History::parse("not json").is_none());
    }

    #[test]
    fn entry_urls_carry_their_page() {
        let e = Entry { url: "https://a.example/é?q=1&r=2#top".into(), title: "Ünïcode & co".into(), scroll: 5.5 };
        assert_eq!(entry_of_restore_url(&entry_url(&e)), Some(e));
    }

    #[test]
    fn build_steps_add_every_entry_then_move_to_the_current_one() {
        let h = History {
            index: 1,
            entries: vec![entry("https://a.example/"), entry("https://b.example/"), entry("https://c.example/")],
        };
        let steps = build_steps(&h);
        assert_eq!(steps.len(), 4);
        assert!(steps[0].0.starts_with("history.replaceState") && !steps[0].1);
        assert!(steps[1].0.starts_with("history.pushState") && steps[1].1);
        assert!(steps[2].0.starts_with("history.pushState") && steps[2].1);
        assert!(steps[3].0.contains("history.go(-1)") && steps[3].0.contains("location.replace(\"https://b.example/\")"));
        assert!(!steps[3].1);

        let last = History { index: 0, entries: vec![entry("https://a.example/"), entry("https://b.example/")] }.navigated("https://c.example/");
        assert_eq!(build_steps(&last).last().unwrap().0, "location.replace(\"https://c.example/\")");
    }
}
