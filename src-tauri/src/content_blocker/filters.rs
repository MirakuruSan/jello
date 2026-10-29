//! Filter lists for the built-in ad blocker: where they come from, how uBlock
//! Origin's `!#if` directives are resolved, and how they compile into an
//! `adblock::Engine`. Downloads and on-disk caching live in `mod.rs`.

use adblock::lists::{FilterFormat, FilterSet, ParseOptions};
use adblock::resources::PermissionMask;
use adblock::Engine;

/// One subscribed filter list.
pub struct ListSource {
    /// File name in the cache directory, and the key in the update state.
    pub id: &'static str,
    /// Tried in order until one answers.
    pub urls: &'static [&'static str],
    pub format: FilterFormat,
    /// uBlock Origin's own lists may use its trusted scriptlets
    /// (`trusted-set-cookie`, `trusted-replace-xhr-response`, …).
    pub trusted: bool,
}

/// uBlock Origin's default selection: its own lists, EasyList, EasyPrivacy and
/// Peter Lowe's ad server list.
pub const LISTS: &[ListSource] = &[
    ListSource {
        id: "ublock-filters",
        urls: &[
            "https://ublockorigin.github.io/uAssets/filters/filters.txt",
            "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssets@latest/filters/filters.txt",
        ],
        format: FilterFormat::Standard,
        trusted: true,
    },
    ListSource {
        id: "ublock-badware",
        urls: &[
            "https://ublockorigin.github.io/uAssets/filters/badware.txt",
            "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssets@latest/filters/badware.txt",
        ],
        format: FilterFormat::Standard,
        trusted: true,
    },
    ListSource {
        id: "ublock-privacy",
        urls: &[
            "https://ublockorigin.github.io/uAssets/filters/privacy.txt",
            "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssets@latest/filters/privacy.txt",
        ],
        format: FilterFormat::Standard,
        trusted: true,
    },
    ListSource {
        id: "ublock-quick-fixes",
        urls: &[
            "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt",
            "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssets@latest/filters/quick-fixes.txt",
        ],
        format: FilterFormat::Standard,
        trusted: true,
    },
    ListSource {
        id: "ublock-unbreak",
        urls: &[
            "https://ublockorigin.github.io/uAssets/filters/unbreak.txt",
            "https://cdn.jsdelivr.net/gh/uBlockOrigin/uAssets@latest/filters/unbreak.txt",
        ],
        format: FilterFormat::Standard,
        trusted: true,
    },
    ListSource {
        id: "easylist",
        urls: &[
            "https://easylist.to/easylist/easylist.txt",
            "https://ublockorigin.github.io/uAssets/thirdparties/easylist.txt",
        ],
        format: FilterFormat::Standard,
        trusted: false,
    },
    ListSource {
        id: "easyprivacy",
        urls: &[
            "https://easylist.to/easylist/easyprivacy.txt",
            "https://ublockorigin.github.io/uAssets/thirdparties/easyprivacy.txt",
        ],
        format: FilterFormat::Standard,
        trusted: false,
    },
    ListSource {
        id: "peter-lowe",
        urls: &[
            "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=hosts&showintro=1&mimetype=plaintext",
            "https://ublockorigin.github.io/uAssets/thirdparties/pgl.yoyo.org/as/serverlist.txt",
        ],
        format: FilterFormat::Hosts,
        trusted: false,
    },
];

/// Exceptions Jello always applies on top of the lists. The default lists
/// block Google's AI Overview request, which leaves results pages saying
/// "Can't generate an AI overview right now".
const JELLO_RULES: &str = "@@||www.google.com/async/folsrch^\n";

/// Permission bit that unlocks uBlock Origin's trusted scriptlets; resources
/// marked `requiresTrust` carry it.
pub const TRUSTED_SCRIPTLETS: PermissionMask = PermissionMask::from_bits(1);

/// Truth value of a `!#if` token for Jello: a Chromium (Edge WebView2) desktop
/// browser running uBlock Origin's scriptlets, without HTML filtering.
fn directive_token(token: &str) -> bool {
    matches!(token, "env_chromium" | "env_edge" | "ext_ublock")
}

/// Evaluate a `!#if` expression: tokens, `!`, `&&`, `||` and parentheses.
/// Unknown tokens are false, as in uBlock Origin.
fn eval_directive(expr: &str) -> bool {
    struct Parser<'a> {
        tokens: Vec<&'a str>,
        pos: usize,
    }
    impl<'a> Parser<'a> {
        fn peek(&self) -> Option<&'a str> {
            self.tokens.get(self.pos).copied()
        }
        fn next(&mut self) -> Option<&'a str> {
            let t = self.peek();
            self.pos += 1;
            t
        }
        fn or(&mut self) -> bool {
            let mut v = self.and();
            while self.peek() == Some("||") {
                self.next();
                let rhs = self.and();
                v = v || rhs;
            }
            v
        }
        fn and(&mut self) -> bool {
            let mut v = self.unary();
            while self.peek() == Some("&&") {
                self.next();
                let rhs = self.unary();
                v = v && rhs;
            }
            v
        }
        fn unary(&mut self) -> bool {
            match self.next() {
                Some("!") => !self.unary(),
                Some("(") => {
                    let v = self.or();
                    if self.peek() == Some(")") {
                        self.next();
                    }
                    v
                }
                Some(t) => directive_token(t),
                None => false,
            }
        }
    }
    let mut tokens = Vec::new();
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' => i += 1,
            b'!' | b'(' | b')' => {
                tokens.push(&expr[i..i + 1]);
                i += 1;
            }
            b'&' | b'|' if i + 1 < bytes.len() && bytes[i + 1] == bytes[i] => {
                tokens.push(&expr[i..i + 2]);
                i += 2;
            }
            _ => {
                let start = i;
                while i < bytes.len() && !b" \t!()&|".contains(&bytes[i]) {
                    i += 1;
                }
                if i == start {
                    i += 1;
                } else {
                    tokens.push(&expr[start..i]);
                }
            }
        }
    }
    Parser { tokens, pos: 0 }.or()
}

/// Drop the lines that `!#if` / `!#else` / `!#endif` blocks exclude for this
/// browser. `adblock` parses every line it is given, so without this the
/// Firefox-, Safari- and mobile-only rules in uBlock Origin's lists would apply
/// too. `!#include` targets are not followed; the subscribed lists have none.
pub fn preprocess(text: &str) -> String {
    // One entry per open block: (this branch is active, the block's own
    // condition held).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_end();
        let enclosing = stack.last().map_or(true, |&(active, _)| active);
        let directive = trimmed.strip_prefix("!#if").filter(|rest| rest.is_empty() || rest.starts_with(' '));
        if let Some(expr) = directive {
            let cond = eval_directive(expr);
            stack.push((enclosing && cond, cond));
        } else if trimmed == "!#else" {
            if let Some((_, cond)) = stack.pop() {
                let outer = stack.last().map_or(true, |&(active, _)| active);
                stack.push((outer && !cond, cond));
            }
        } else if trimmed == "!#endif" {
            stack.pop();
        } else if enclosing {
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out
}

/// Compile downloaded list texts (paired with their sources) and the user's own
/// filters into an engine. The user's filters are trusted, like uBlock Origin's
/// "My filters", so they may use any scriptlet.
pub fn build_engine<'a>(lists: impl IntoIterator<Item = (&'a ListSource, &'a str)>, user_filters: &str) -> Engine {
    let mut set = FilterSet::new(false);
    for (source, text) in lists {
        let opts = ParseOptions {
            format: source.format,
            permissions: if source.trusted { TRUSTED_SCRIPTLETS } else { PermissionMask::default() },
            ..ParseOptions::default()
        };
        let text = match source.format {
            FilterFormat::Standard => preprocess(text),
            _ => text.to_string(),
        };
        set.add_filter_list(text, opts);
    }
    set.add_filter_list(JELLO_RULES.to_string(), ParseOptions::default());
    if !user_filters.trim().is_empty() {
        let opts = ParseOptions { permissions: TRUSTED_SCRIPTLETS, ..ParseOptions::default() };
        set.add_filter_list(preprocess(user_filters), opts);
    }
    Engine::new_with_filter_set(set)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adblock::request::Request;

    fn blocks(engine: &Engine, url: &str, source: &str, kind: &str) -> bool {
        engine
            .check_network_request(&Request::new(url, source, kind, "GET").unwrap())
            .should_block()
    }

    #[test]
    fn directives_keep_only_the_chromium_branch() {
        let list = "a\n!#if env_firefox\nfirefox\n!#else\nnot-firefox\n!#endif\n\
                    !#if !ext_ubol\nnot-lite\n!#endif\n\
                    !#if env_mobile\n!#if env_chromium\nmobile-chromium\n!#endif\n!#endif\n\
                    !#if (env_chromium || env_safari) && !cap_html_filtering\nchromium-no-html\n!#endif\nz\n";
        assert_eq!(preprocess(list), "a\nnot-firefox\nnot-lite\nchromium-no-html\nz\n");
    }

    #[test]
    fn malformed_directives_do_not_swallow_the_list() {
        // Stray !#else / !#endif outside a block are ignored.
        assert_eq!(preprocess("a\n!#endif\nb\n!#else\nc\n"), "a\nb\nc\n");
        // Unknown tokens and empty expressions are false.
        assert_eq!(preprocess("!#if made_up_token\nx\n!#endif\n!#if\ny\n!#endif\nz\n"), "z\n");
        // An unclosed block only affects the lines it covers.
        assert_eq!(preprocess("a\n!#if env_firefox\nb\n"), "a\n");
    }

    #[test]
    fn engine_blocks_ads_and_keeps_ai_overviews() {
        let ubo = &LISTS[0];
        let hosts = LISTS.iter().find(|l| l.id == "peter-lowe").unwrap();
        let engine = build_engine(
            [(ubo, "||ads.example^\n||www.google.com/async/*\n"), (hosts, "127.0.0.1 tracker.example\n")],
            "",
        );
        assert!(blocks(&engine, "https://ads.example/banner.js", "https://news.example/", "script"));
        assert!(blocks(&engine, "https://tracker.example/pixel.gif", "https://news.example/", "image"));
        assert!(!blocks(&engine, "https://cdn.example/app.js", "https://news.example/", "script"));
        assert!(blocks(&engine, "https://www.google.com/async/other", "https://www.google.com/search?q=x", "xmlhttprequest"));
        assert!(!blocks(&engine, "https://www.google.com/async/folsrch?q=x", "https://www.google.com/search?q=x", "xmlhttprequest"));
    }

    #[test]
    fn trusted_scriptlets_only_inject_from_ublock_lists() {
        use adblock::resources::{MimeType, Resource, ResourceType};
        use base64::Engine as _;
        let resource = Resource {
            name: "trusted-set-cookie.js".to_string(),
            aliases: vec![],
            kind: ResourceType::Mime(MimeType::ApplicationJavascript),
            content: base64::engine::general_purpose::STANDARD.encode("function trustedSetCookie(){}"),
            dependencies: vec![],
            permission: TRUSTED_SCRIPTLETS,
        };
        let ubo = &LISTS[0];
        let easylist = LISTS.iter().find(|l| l.id == "easylist").unwrap();
        let mut engine = build_engine(
            [(ubo, "a.example##+js(trusted-set-cookie, x, 1)\n"), (easylist, "b.example##+js(trusted-set-cookie, x, 1)\n")],
            "c.example##+js(trusted-set-cookie, x, 1)\n",
        );
        engine.use_resources([resource]);
        assert!(engine.url_cosmetic_resources("https://a.example/").injected_script.contains("trustedSetCookie"));
        assert!(!engine.url_cosmetic_resources("https://b.example/").injected_script.contains("trustedSetCookie"));
        assert!(engine.url_cosmetic_resources("https://c.example/").injected_script.contains("trustedSetCookie"));
    }

    #[test]
    fn procedural_filters_arrive_as_steps() {
        let ubo = &LISTS[0];
        let engine = build_engine(
            [(ubo, "news.example##div.card:has-text(/Sponsored/i):upward(2)\nnews.example##.promo:style(height: 0)\nnews.example##.x:remove()\n")],
            "",
        );
        let res = engine.url_cosmetic_resources("https://news.example/");
        let actions: Vec<serde_json::Value> =
            res.procedural_actions.iter().map(|a| serde_json::from_str(a).unwrap()).collect();
        let has_text = actions.iter().find(|a| a["selector"].as_array().map_or(false, |s| s.len() == 3)).unwrap();
        assert_eq!(has_text["selector"][0]["type"], "css-selector");
        assert_eq!(has_text["selector"][1]["type"], "has-text");
        assert_eq!(has_text["selector"][2]["type"], "upward");
        assert!(actions.iter().any(|a| a["action"]["type"] == "style"));
        assert!(actions.iter().any(|a| a["action"]["type"] == "remove"));
        // A selector that is not valid CSS is dropped instead of reaching pages.
        let bad = build_engine([(ubo, "news.example##div[[broken\n")], "");
        assert!(bad.url_cosmetic_resources("https://news.example/").hide_selectors.is_empty());
    }

    #[test]
    fn user_filters_block_hide_and_allow() {
        let easylist = LISTS.iter().find(|l| l.id == "easylist").unwrap();
        let user = "||cdn.example/promo.js\nnews.example##.sponsor\n@@||ads.example/needed.js\n! a comment\n";
        let engine = build_engine([(easylist, "||ads.example^\n")], user);
        assert!(blocks(&engine, "https://cdn.example/promo.js", "https://news.example/", "script"));
        assert!(!blocks(&engine, "https://ads.example/needed.js", "https://news.example/", "script"));
        assert!(blocks(&engine, "https://ads.example/other.js", "https://news.example/", "script"));
        assert!(engine.url_cosmetic_resources("https://news.example/").hide_selectors.contains(".sponsor"));
    }
}
