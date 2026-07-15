use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputClassification {
    Url(String),
    SearchQuery(String),
}

pub struct SearchEngine {
    pub name: String,
    pub keyword: String,
    pub url_template: String,
}

pub fn classify_input(input: &str) -> InputClassification {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return InputClassification::SearchQuery(String::new());
    }

    // 1. Ends explicitly with '/'
    if trimmed.ends_with('/') {
        return InputClassification::Url(normalize_url(trimmed));
    }

    // 2. Has scheme in {http, https, file, jello}
    if let Some(pos) = trimmed.find("://") {
        let scheme = &trimmed[..pos].to_lowercase();
        if scheme == "http" || scheme == "https" || scheme == "file" || scheme == "jello" {
            return InputClassification::Url(trimmed.to_string());
        }
    }

    // Split into host/port and path for further checks
    let first_slash = trimmed.find('/').unwrap_or(trimmed.len());
    let host_and_port = &trimmed[..first_slash];

    let host = if let Some(colon_pos) = host_and_port.rfind(':') {
        if host_and_port.contains(']') && host_and_port[colon_pos..].contains(']') {
            host_and_port
        } else {
            &host_and_port[..colon_pos]
        }
    } else {
        host_and_port
    };

    // 3. Is localhost
    if host.to_lowercase() == "localhost" {
        return InputClassification::Url(normalize_url(trimmed));
    }

    // 4. Is IPv4 / IPv6 literal
    if is_ip_literal(host) {
        return InputClassification::Url(normalize_url(trimmed));
    }

    // 5. Contains no spaces AND has a dot AND last label of host is alphabetic >= 2 chars
    if !trimmed.contains(' ') && host.contains('.') {
        let clean_host = host.trim_start_matches('[').trim_end_matches(']');
        let labels: Vec<&str> = clean_host.split('.').collect();
        if let Some(last_label) = labels.last() {
            if last_label.len() >= 2 && last_label.chars().all(|c| c.is_ascii_alphabetic()) {
                return InputClassification::Url(normalize_url(trimmed));
            }
        }
    }

    InputClassification::SearchQuery(trimmed.to_string())
}

/// Scheme-less input can't be loaded by WebView2 (Url::parse fails and the
/// content view silently falls back to about:blank), so every Url
/// classification must come out absolute. localhost/IPs get http (dev
/// servers rarely have TLS); everything else defaults to https.
pub fn normalize_url(input: &str) -> String {
    if input.contains("://") || input.starts_with("about:") {
        return input.to_string();
    }
    let host = input.split('/').next().unwrap_or(input);
    let bare_host = host.split(':').next().unwrap_or(host);
    if bare_host.eq_ignore_ascii_case("localhost") || is_ip_literal(host) || is_ip_literal(bare_host) {
        format!("http://{}", input)
    } else {
        format!("https://{}", input)
    }
}

fn is_ip_literal(s: &str) -> bool {
    let clean = if s.starts_with('[') && s.ends_with(']') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    clean.parse::<std::net::IpAddr>().is_ok()
}

/// The default search template (a URL containing `%s`), from the `searchEngine`
/// setting, falling back to DuckDuckGo. Lets the user pick their engine (#12).
pub fn default_search_url(app: &tauri::AppHandle) -> String {
    // `defaultSearch` is the key the setup wizard writes (a URL template with
    // `%s`). It was previously never read, so choosing an engine had no effect
    // (#12). Fall back to DuckDuckGo.
    crate::capture::screenshot::get_setting(app, "defaultSearch")
        .filter(|s| s.contains("%s"))
        .unwrap_or_else(|| "https://duckduckgo.com/?q=%s".to_string())
}

pub fn get_search_engines(conn: &Connection) -> rusqlite::Result<Vec<SearchEngine>> {
    let mut stmt = conn.prepare("SELECT name, keyword, url_template FROM search_engines")?;
    let rows = stmt.query_map([], |row| {
        Ok(SearchEngine {
            name: row.get(0)?,
            keyword: row.get(1)?,
            url_template: row.get(2)?,
        })
    })?;

    let mut list = Vec::new();
    for r in rows {
        list.push(r?);
    }
    Ok(list)
}

pub fn percent_encode(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

pub fn route_query(input: &str, engines: &[SearchEngine], default_template: &str) -> String {
    let trimmed = input.trim();
    
    // Check if input starts with "keyword "
    if let Some(space_pos) = trimmed.find(' ') {
        let keyword = &trimmed[..space_pos];
        let rest = trimmed[space_pos + 1..].trim();
        
        if let Some(engine) = engines.iter().find(|e| e.keyword == keyword) {
            let encoded = percent_encode(rest);
            return engine.url_template.replace("%s", &encoded);
        }
    }

    let encoded = percent_encode(trimmed);
    default_template.replace("%s", &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifier_url() {
        assert_eq!(classify_input("http://google.com"), InputClassification::Url("http://google.com".to_string()));
        assert_eq!(classify_input("https://localhost:8080"), InputClassification::Url("https://localhost:8080".to_string()));
        assert_eq!(classify_input("localhost"), InputClassification::Url("http://localhost".to_string()));
        assert_eq!(classify_input("localhost:3000"), InputClassification::Url("http://localhost:3000".to_string()));
        assert_eq!(classify_input("127.0.0.1"), InputClassification::Url("http://127.0.0.1".to_string()));
        assert_eq!(classify_input("[::1]"), InputClassification::Url("http://[::1]".to_string()));
        assert_eq!(classify_input("google.com"), InputClassification::Url("https://google.com".to_string()));
        assert_eq!(classify_input("google.com/"), InputClassification::Url("https://google.com/".to_string()));
        assert_eq!(classify_input("google.com/path"), InputClassification::Url("https://google.com/path".to_string()));
        assert_eq!(classify_input("my-site.local"), InputClassification::Url("https://my-site.local".to_string()));
        assert_eq!(classify_input("jello://settings"), InputClassification::Url("jello://settings".to_string()));
    }

    #[test]
    fn test_classifier_query() {
        assert_eq!(classify_input("google"), InputClassification::SearchQuery("google".to_string()));
        assert_eq!(classify_input("google."), InputClassification::SearchQuery("google.".to_string()));
        assert_eq!(classify_input("google.c"), InputClassification::SearchQuery("google.c".to_string()));
        assert_eq!(classify_input("google.12"), InputClassification::SearchQuery("google.12".to_string()));
        assert_eq!(classify_input("search query"), InputClassification::SearchQuery("search query".to_string()));
        assert_eq!(classify_input("http ://invalid"), InputClassification::SearchQuery("http ://invalid".to_string()));
    }

    #[test]
    fn test_nucleo_api() {
        use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};
        use nucleo_matcher::{Matcher, Config, Utf32String, Utf32Str};
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse("ust", CaseMatching::Ignore, Normalization::Smart);
        
        let mut indices = Vec::new();
        let haystack = Utf32String::from("rust");
        let slice = match &haystack {
            Utf32String::Ascii(v) => Utf32Str::Ascii(v.as_bytes()),
            Utf32String::Unicode(v) => Utf32Str::Unicode(v),
        };
        let score = pattern.indices(slice, &mut matcher, &mut indices);
        assert!(score.is_some());
        assert_eq!(indices, vec![1, 2, 3]);
    }
}
