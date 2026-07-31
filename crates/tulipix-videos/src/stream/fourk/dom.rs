//! A small, forgiving HTML scanner — enough to read one scraped catalogue page.
//!
//! Upstream MovieBox-Tui reaches for `scraper` here. That pulls html5ever,
//! selectors, cssparser and about twenty transitive crates into a dependency
//! tree this workspace has spent real effort keeping small, to buy four
//! operations: find elements by tag and class, read an attribute, read the text
//! inside, and search within what was found. That is what this is.
//!
//! It is not a compliant parser and does not try to be. It never allocates a
//! tree, tolerates unclosed tags, and treats an element as running to its
//! matching close or to the next sibling of the same name — scraped pages
//! routinely omit closes, and dropping half a page because one `</div>` is
//! missing is worse than being slightly wrong about nesting.

/// One element: its attribute text and the HTML between its tags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct El<'a> {
    pub tag: &'a str,
    pub attrs: &'a str,
    pub inner: &'a str,
}

/// Elements with no closing tag — depth tracking must not wait for one.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose content is raw text and may contain `<`. Skipped wholesale
/// rather than tokenised into.
const RAW_TEXT: &[&str] = &["script", "style"];

/// Every `<tag>` carrying `class`, in document order.
///
/// `tag` of `"*"` matches any element, which is how a class-only selector is
/// spelled. `class` of `None` matches on the tag alone. Class matching is
/// whole-token: `.badge` does not match `class="badge-size"`.
pub fn find_all<'a>(html: &'a str, tag: &str, class: Option<&str>) -> Vec<El<'a>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some((_, name, attrs, after)) = next_open(html, cursor) {
        if RAW_TEXT.contains(&name) {
            cursor = skip_raw(html, name, after);
            continue;
        }
        let (inner_s, inner_e, next) = element_body(html, name, after);
        let matches_tag = tag == "*" || tag.eq_ignore_ascii_case(name);
        let matches_class = class.is_none_or(|c| has_class(attrs, c));
        if matches_tag && matches_class {
            out.push(El { tag: name, attrs, inner: &html[inner_s..inner_e] });
            // Do not descend into a match: an element and its identically
            // classed child are one hit, not two, and a caller that wants the
            // child asks for it inside `inner`.
            cursor = next;
            continue;
        }
        cursor = after;
    }
    out
}

/// First match, or `None`.
pub fn find<'a>(html: &'a str, tag: &str, class: Option<&str>) -> Option<El<'a>> {
    find_all(html, tag, class).into_iter().next()
}

/// Is `class` one of the whitespace-separated tokens in this element's
/// `class` attribute?
pub fn has_class(attrs: &str, class: &str) -> bool {
    attr(attrs, "class")
        .map(|list| list.split_whitespace().any(|t| t == class))
        .unwrap_or(false)
}

/// Read one attribute out of an element's attribute text.
///
/// Tolerates unquoted values and single quotes. Matching is
/// case-insensitive on the name and whole-token, so `href` never matches
/// inside `data-href`.
pub fn attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let want = name.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find(&want) {
        let at = from + pos;
        from = at + want.len();
        let boundary_before = at == 0
            || attrs[..at].chars().next_back().map(char::is_whitespace).unwrap_or(false);
        if !boundary_before {
            continue;
        }
        let rest = attrs[at + want.len()..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        let value = if let Some(r) = rest.strip_prefix('"') {
            r.split('"').next().unwrap_or("")
        } else if let Some(r) = rest.strip_prefix('\'') {
            r.split('\'').next().unwrap_or("")
        } else {
            rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or("")
        };
        return Some(decode_entities(value));
    }
    None
}

/// Visible text of a fragment: tags dropped, entities decoded, runs of
/// whitespace collapsed to one space.
pub fn text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut cursor = 0usize;
    while let Some(lt) = html[cursor..].find('<') {
        out.push_str(&html[cursor..cursor + lt]);
        out.push(' ');
        let from = cursor + lt;
        // A raw-text element's content is code, not prose.
        if let Some((_, name, _, after)) = next_open(html, from)
            && RAW_TEXT.contains(&name)
        {
            cursor = skip_raw(html, name, after);
            continue;
        }
        match html[from..].find('>') {
            Some(gt) => cursor = from + gt + 1,
            None => {
                cursor = html.len();
                break;
            }
        }
    }
    out.push_str(&html[cursor..]);
    decode_entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- scanning ----

/// The next opening tag at or after `from`, as
/// `(tag_start, name, attrs, offset_just_past_the_tag)`. Comments, doctypes
/// and closing tags are skipped.
fn next_open(html: &str, from: usize) -> Option<(usize, &str, &str, usize)> {
    let mut cursor = from;
    loop {
        let lt = cursor + html[cursor..].find('<')?;
        let rest = &html[lt..];
        if rest.starts_with("<!--") {
            cursor = lt + rest.find("-->").map(|e| e + 3).unwrap_or(rest.len());
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") || rest.starts_with("</") {
            cursor = lt + rest.find('>').map(|e| e + 1).unwrap_or(rest.len());
            continue;
        }
        let gt = lt + rest.find('>')?;
        let body = &html[lt + 1..gt];
        let name_end = body
            .find(|c: char| c.is_whitespace() || c == '/')
            .unwrap_or(body.len());
        let name = &body[..name_end];
        if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            cursor = gt + 1;
            continue;
        }
        return Some((lt, name, &body[name_end..], gt + 1));
    }
}

/// Byte range of an element's inner HTML, plus where scanning should resume.
///
/// Walks to the matching close, counting nested opens of the same name. With no
/// matching close the element is treated as running to the next sibling of the
/// same name, or to the end — which is what keeps a run of unclosed `<div>`s
/// splitting into separate elements instead of collapsing into one.
fn element_body(html: &str, name: &str, after_open: usize) -> (usize, usize, usize) {
    if VOID.contains(&name) || html[..after_open].trim_end().ends_with("/>") {
        return (after_open, after_open, after_open);
    }
    let mut depth = 1usize;
    let mut cursor = after_open;
    let mut first_sibling = None;
    loop {
        let Some(lt) = html[cursor..].find('<') else { break };
        let at = cursor + lt;
        let rest = &html[at..];
        if rest.starts_with("<!--") {
            cursor = at + rest.find("-->").map(|e| e + 3).unwrap_or(rest.len());
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let end = at + gt + 1;
        if let Some(body) = rest[..gt].strip_prefix("</") {
            if body.trim().eq_ignore_ascii_case(name) {
                depth -= 1;
                if depth == 0 {
                    return (after_open, at, end);
                }
            }
            cursor = end;
            continue;
        }
        let body = &rest[1..gt];
        let n_end = body.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(body.len());
        let n = &body[..n_end];
        if RAW_TEXT.contains(&n) {
            cursor = skip_raw(html, n, end);
            continue;
        }
        if n.eq_ignore_ascii_case(name) && !VOID.contains(&n) && !rest[..gt].ends_with('/') {
            if depth == 1 {
                first_sibling.get_or_insert(at);
            }
            depth += 1;
        }
        cursor = end;
    }
    let stop = first_sibling.unwrap_or(html.len());
    (after_open, stop, stop)
}

/// Skip past a raw-text element's content and closing tag.
fn skip_raw(html: &str, name: &str, after_open: usize) -> usize {
    let close = format!("</{name}");
    match html[after_open..].to_ascii_lowercase().find(&close) {
        Some(at) => {
            let from = after_open + at;
            from + html[from..].find('>').map(|e| e + 1).unwrap_or(0)
        }
        None => html.len(),
    }
}

/// The handful of entities a scraped page actually uses. Numeric forms are
/// handled generically; the rest fall through unchanged rather than being
/// mangled.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            n if n.starts_with("#x") || n.starts_with("#X") => {
                u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32)
            }
            n if n.starts_with('#') => {
                n[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_tag_and_whole_class_token() {
        let h = r#"<div class="card big"><span class="title">Dune</span></div>
                   <div class="cardholder">no</div>"#;
        let cards = find_all(h, "div", Some("card"));
        assert_eq!(cards.len(), 1, "`cardholder` is not the `card` class");
        assert_eq!(text(cards[0].inner), "Dune");
    }

    #[test]
    fn a_class_only_selector_matches_any_tag() {
        let h = r#"<a class="hit">one</a><p class="hit">two</p>"#;
        assert_eq!(find_all(h, "*", Some("hit")).len(), 2);
    }

    #[test]
    fn nesting_is_counted_not_guessed() {
        let h = "<div class=o><div>inner</div>tail</div><div>after</div>";
        let el = find(h, "div", Some("o")).unwrap();
        assert_eq!(text(el.inner), "inner tail", "stops at its own close, not the child's");
    }

    #[test]
    fn a_match_is_not_re_reported_for_its_own_descendants() {
        let h = r#"<div class="row"><div class="row">child</div></div>"#;
        let hits = find_all(h, "div", Some("row"));
        assert_eq!(hits.len(), 1, "the outer element already contains the inner one");
    }

    #[test]
    fn unclosed_siblings_still_split() {
        // Scraped pages omit closes constantly; one missing `</li>` must not
        // swallow the rest of the list.
        let h = "<li class=i>one<li class=i>two<li class=i>three";
        let items: Vec<String> = find_all(h, "li", Some("i")).iter().map(|e| text(e.inner)).collect();
        assert_eq!(items, ["one", "two", "three"]);
    }

    #[test]
    fn attributes_survive_quoting_styles_and_near_misses() {
        let a = r#" href="/a/b" data-href="/nope" title='q' rel=next"#;
        assert_eq!(attr(a, "href").as_deref(), Some("/a/b"), "data-href must not match");
        assert_eq!(attr(a, "title").as_deref(), Some("q"));
        assert_eq!(attr(a, "rel").as_deref(), Some("next"));
        assert_eq!(attr(a, "src"), None);
    }

    #[test]
    fn void_and_raw_text_elements_do_not_derail_the_scan() {
        let h = r#"<div class="w"><img src="/p.jpg"><script>if (a<b) {}</script>Text</div>"#;
        let el = find(h, "div", Some("w")).unwrap();
        assert_eq!(text(el.inner), "Text", "script content is code, not prose");
        let img = find(el.inner, "img", None).unwrap();
        assert_eq!(attr(img.attrs, "src").as_deref(), Some("/p.jpg"));
    }

    #[test]
    fn entities_decode_including_numeric_forms() {
        assert_eq!(text("<p>Tom &amp; Jerry &#39;93 &#x2014; ok</p>"), "Tom & Jerry '93 — ok");
        // An unknown entity is left alone rather than eaten.
        assert_eq!(text("<p>a &zzz; b</p>"), "a &zzz; b");
    }
}
