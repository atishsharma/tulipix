//! YouTube's own query suggestions, as the search box shows them under the
//! library's matches.
//!
//! The endpoint is Google's unofficial autocomplete with `client=firefox`,
//! which answers plain UTF-8 JSON rather than a JSONP callback:
//! `["lofi ra",["lofi rap song","lofi rain",…],[],{…}]`.

/// The endpoint, without the query.
pub const SUGGEST_URL: &str = "https://suggestqueries-clients6.youtube.com/complete/search";

/// Up to `limit` suggestions, leaving out one identical to the query itself.
/// Anything that is not the expected shape is no suggestions, not an error:
/// this is a nicety on top of a search that works without it.
pub fn parse_suggest(body: &str, limit: usize) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let asked = v[0].as_str().unwrap_or("").trim().to_lowercase();
    v[1].as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| s.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.to_lowercase() != asked)
                .take(limit)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As served on 2026-09-16 for `q=lofi ra`, trimmed.
    const BODY: &str = r#"["lofi ra",["lofi rap song","lofi rain","lofi rap","lofi radio"],[],{"google:suggestsubtypes":[[512],[512,433],[512,433],[512]]}]"#;

    #[test]
    fn reads_the_second_element() {
        assert_eq!(parse_suggest(BODY, 10), ["lofi rap song", "lofi rain", "lofi rap", "lofi radio"]);
        assert_eq!(parse_suggest(BODY, 2), ["lofi rap song", "lofi rain"]);
    }

    #[test]
    fn drops_the_query_itself_and_keeps_unicode() {
        let body = r#"["café mú",["Café mú","café música","café música do café"]]"#;
        assert_eq!(parse_suggest(body, 10), ["café música", "café música do café"]);
    }

    #[test]
    fn anything_else_is_empty() {
        assert!(parse_suggest("", 10).is_empty());
        assert!(parse_suggest("window.google.ac.h([\"x\",[]])", 10).is_empty());
        assert!(parse_suggest(r#"{"error":"quota"}"#, 10).is_empty());
        assert!(parse_suggest(r#"["x"]"#, 10).is_empty());
    }
}
