//! The browse feed — what the Stream landing screen shows before a search.
//!
//! `tab-operating` is the catalogue's own home screen: rows of subjects under
//! editorial headings ("Trending", "New releases"). The client already calls it
//! once on [`crate::stream::StreamClient::init`] to pick up a session token, and
//! until now threw the payload away.
//!
//! The exact envelope is not documented and has changed between app versions, so
//! this does not hard-code a path. It walks the payload looking for arrays of
//! things that *are* subjects — objects carrying a `subjectId` — and labels each
//! row from the nearest heading-ish field on the object that owns the array.
//! An unrecognised payload yields no rows, which the landing screen renders as
//! "nothing to browse", never as an error.

use serde_json::Value;

use super::{hit_of, SearchHit};

/// Rows longer than this are trimmed — a landing row is a strip, not a grid.
const MAX_ITEMS: usize = 20;
/// Rows shorter than this are not worth a heading.
const MIN_ITEMS: usize = 3;
/// How many rows the landing screen will show.
const MAX_ROWS: usize = 8;
/// Guards against a pathological payload turning the walk into a long crawl.
const MAX_DEPTH: usize = 6;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeedRow {
    pub title: String,
    pub items: Vec<SearchHit>,
}

/// Every subject row in a browse payload, in the order the server listed them.
pub fn parse_feed(payload: &Value) -> Vec<FeedRow> {
    let mut rows: Vec<FeedRow> = Vec::new();
    walk(payload, "", 0, &mut rows);

    // The same subject often appears in two rows ("Trending" and "For you").
    // Dropping the later duplicate keeps the screen from looking like a loop.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for row in rows.iter_mut() {
        row.items.retain(|h| seen.insert(h.id.clone()));
    }
    rows.retain(|r| r.items.len() >= MIN_ITEMS);
    rows.truncate(MAX_ROWS);
    rows
}

/// Depth-first walk. `label` is the best heading seen on the way down, so a row
/// nested inside a titled block inherits that block's name.
fn walk(node: &Value, label: &str, depth: usize, out: &mut Vec<FeedRow>) {
    if depth > MAX_DEPTH || out.len() >= MAX_ROWS * 4 {
        return;
    }
    match node {
        Value::Array(items) => {
            // An array of subjects is a row in its own right.
            let hits: Vec<SearchHit> = items.iter().filter_map(hit_of).take(MAX_ITEMS).collect();
            if hits.len() >= MIN_ITEMS {
                out.push(FeedRow { title: label.to_string(), items: hits });
                return;
            }
            for item in items {
                walk(item, label, depth + 1, out);
            }
        }
        Value::Object(map) => {
            let here = heading(node).unwrap_or_else(|| label.to_string());
            // Only containers can hold a row; a field name like "subjects" is
            // not a heading, so the one from this object carries down instead.
            for child in map.values().filter(|v| v.is_array() || v.is_object()) {
                walk(child, &here, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// A user-facing heading on this object, if it has one.
fn heading(node: &Value) -> Option<String> {
    for key in ["tabName", "sectionTitle", "moduleName", "name", "title"] {
        let text = node.get(key).and_then(|v| v.as_str()).unwrap_or("").trim();
        // A subject's own `title` is not a heading; a subject is identified by
        // carrying an id, so anything with one is skipped here.
        if !text.is_empty() && text.len() <= 40 && node.get("subjectId").is_none() {
            return Some(text.to_string());
        }
    }
    None
}

/// Heading to show when the payload gave a row no name.
pub fn row_label(row: &FeedRow, index: usize) -> String {
    if !row.title.trim().is_empty() {
        return row.title.trim().to_string();
    }
    match index {
        0 => "Trending".to_string(),
        1 => "Popular".to_string(),
        _ => format!("Browse {}", index + 1),
    }
}

impl super::StreamClient {
    /// The catalogue's own home screen, as rows.
    pub async fn feed(&self) -> Result<Vec<FeedRow>, super::StreamError> {
        let payload = self
            .get("/wefeed-mobile-bff/tab-operating?page=1&tabId=0&version=")
            .await?;
        Ok(parse_feed(&payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn subject(id: &str, title: &str) -> Value {
        json!({
            "subjectId": id, "title": title, "subjectType": 1,
            "releaseDate": "2024-01-01", "cover": {"url": format!("https://img/{id}.jpg")}
        })
    }

    fn row(name: &str, ids: &[&str]) -> Value {
        json!({
            "title": name,
            "subjects": ids.iter().map(|i| subject(i, &format!("Title {i}"))).collect::<Vec<_>>()
        })
    }

    #[test]
    fn rows_come_back_labelled_with_their_heading() {
        let payload = json!({"data": {"items": [
            row("Trending now", &["a", "b", "c"]),
            row("New releases", &["d", "e", "f", "g"]),
        ]}});
        let rows = parse_feed(&payload);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(rows[0].title, "Trending now");
        assert_eq!(rows[0].items.len(), 3);
        assert_eq!(rows[0].items[0].id, "a");
        assert_eq!(rows[1].items.len(), 4);
    }

    #[test]
    fn a_subject_repeated_across_rows_is_shown_once() {
        let payload = json!({"items": [
            row("Trending", &["a", "b", "c"]),
            row("For you", &["a", "d", "e", "f"]),
        ]});
        let rows = parse_feed(&payload);
        assert_eq!(rows[1].items.len(), 3, "the repeat of `a` is dropped");
        assert!(!rows[1].items.iter().any(|h| h.id == "a"));
    }

    #[test]
    fn thin_rows_and_junk_payloads_yield_nothing() {
        // Two items is not a row.
        assert!(parse_feed(&json!({"items": [row("Pair", &["a", "b"])]})).is_empty());
        assert!(parse_feed(&json!({})).is_empty());
        assert!(parse_feed(&json!([])).is_empty());
        assert!(parse_feed(&json!({"items": [{"subjects": []}]})).is_empty());
        // Subjects without ids are not openable, so they do not count.
        let idless = json!({"items": [{"title": "X", "subjects": [
            {"title": "a"}, {"title": "b"}, {"title": "c"}
        ]}]});
        assert!(parse_feed(&idless).is_empty());
    }

    #[test]
    fn rows_are_capped_in_length_and_number() {
        let many: Vec<Value> = (0..30).map(|i| subject(&format!("s{i}"), "T")).collect();
        let payload = json!({"items": [{"title": "Big", "subjects": many}]});
        assert_eq!(parse_feed(&payload)[0].items.len(), MAX_ITEMS);

        let rows: Vec<Value> = (0..20)
            .map(|i| {
                let ids: Vec<String> = (0..4).map(|j| format!("r{i}c{j}")).collect();
                row(&format!("Row {i}"), &ids.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            })
            .collect();
        assert_eq!(parse_feed(&json!({"items": rows})).len(), MAX_ROWS);
    }

    #[test]
    fn cards_survive_a_feed_that_names_its_fields_differently() {
        // Same subjects, none of search's field names on them.
        let payload = json!({"items": [{"title": "Trending", "subjects": [
            {"subjectId": "a", "name": "Dune", "year": "2024",
             "image": "https://img/a.jpg", "subjectType": 1},
            {"subjectId": "b", "seriesName": "Severance", "publishDate": "2022-02-18",
             "verticalImage": "https://img/b.jpg", "subjectType": 2},
            {"subjectId": "c", "subjectName": "Loki", "releaseDate": "2021-06-09",
             "cover": {"url": "https://img/c.jpg"}, "subjectType": 2}
        ]}]});
        let items = &parse_feed(&payload)[0].items;
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].title, "Dune");
        assert_eq!(items[0].year, "2024");
        assert_eq!(items[0].cover, "https://img/a.jpg");
        assert_eq!(items[1].title, "Severance");
        assert_eq!(items[1].cover, "https://img/b.jpg");
        assert!(items[1].is_series);
        // The search-shaped fields still win when they are there.
        assert_eq!(items[2].title, "Loki");
        assert_eq!(items[2].cover, "https://img/c.jpg");
    }

    #[test]
    fn an_unnamed_row_still_gets_a_heading() {
        let payload = json!({"items": [{"subjects": [
            subject("a", "A"), subject("b", "B"), subject("c", "C")
        ]}]});
        let rows = parse_feed(&payload);
        assert_eq!(rows.len(), 1);
        assert_eq!(row_label(&rows[0], 0), "Trending");
        assert_eq!(row_label(&FeedRow { title: "  Named  ".into(), items: vec![] }, 3), "Named");
        assert_eq!(row_label(&FeedRow::default(), 5), "Browse 6");
    }
}
