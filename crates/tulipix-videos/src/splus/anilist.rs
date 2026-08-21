//! AniList metadata for the anime lane.
//!
//! Written against AniList's public GraphQL schema (<https://anilist.gitbook.io>).
//! The app already has a one-shot AniList lookup in `crate::anime` for the
//! *scraper*; this is the tab's richer view of the same service — paged search,
//! seasonal, and the fields the scraper never needed. Both point at the same
//! base URL constant so there is one AniList address in the app.
//!
//! One query shape is reused for every call, because AniList rate-limits by
//! request and not by cost: asking for a few extra fields is free, asking twice
//! is not.

use anyhow::{Context, Result};
use serde_json::Value;

use super::Title;

/// AniList's public endpoint. Also used by [`crate::anime::AniListProvider`].
pub const BASE: &str = "https://graphql.anilist.co";

/// Everything Stream Plus reads off a Media node.
///
/// `idMal` is the AniSkip bridge — it comes back on the same request, which is
/// why the skip feature needs no id-mapping service.
const MEDIA_FIELDS: &str = r#"
    id
    idMal
    title { romaji english native }
    description(asHtml: false)
    coverImage { extraLarge large }
    bannerImage
    episodes
    format
    status
    genres
    averageScore
    isAdult
    startDate { year }
    nextAiringEpisode { episode airingAt }
"#;

fn query_search(fields: &str) -> String {
    format!(
        r#"query ($search: String, $page: Int, $perPage: Int) {{
  Page(page: $page, perPage: $perPage) {{
    pageInfo {{ hasNextPage total }}
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {{ {fields} }}
  }}
}}"#
    )
}

fn query_seasonal(fields: &str) -> String {
    format!(
        r#"query ($season: MediaSeason, $year: Int, $page: Int, $perPage: Int) {{
  Page(page: $page, perPage: $perPage) {{
    pageInfo {{ hasNextPage total }}
    media(season: $season, seasonYear: $year, type: ANIME, sort: POPULARITY_DESC) {{ {fields} }}
  }}
}}"#
    )
}

fn query_trending(fields: &str) -> String {
    format!(
        r#"query ($page: Int, $perPage: Int) {{
  Page(page: $page, perPage: $perPage) {{
    pageInfo {{ hasNextPage total }}
    media(type: ANIME, sort: TRENDING_DESC) {{ {fields} }}
  }}
}}"#
    )
}

fn query_by_id(fields: &str) -> String {
    format!("query ($id: Int) {{ Media(id: $id, type: ANIME) {{ {fields} }} }}")
}

async fn post(query: &str, variables: Value) -> Result<Value> {
    let body = serde_json::json!({ "query": query, "variables": variables });
    let resp = tulipix_core::net::http()
        .post(BASE)
        .json(&body)
        .send()
        .await
        .context("anilist: request failed")?;
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("anilist: rate limited — try again in a minute");
    }
    if !status.is_success() {
        anyhow::bail!("anilist: HTTP {}", status.as_u16());
    }
    let v: Value = resp.json().await.context("anilist: bad JSON")?;
    // GraphQL answers 200 with an `errors` array; a null `data` alongside one is
    // a failure however healthy the status line looked.
    if let Some(errs) = v.get("errors").and_then(|e| e.as_array()) {
        if v.get("data").map(Value::is_null).unwrap_or(true) {
            let first = errs
                .first()
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            anyhow::bail!("anilist: {first}");
        }
    }
    Ok(v)
}

/// One page of search results. `page` is 1-based, as AniList counts.
pub async fn search(term: &str, page: i64, per_page: i64) -> Result<(Vec<Title>, bool)> {
    if term.trim().is_empty() {
        return Ok((Vec::new(), false));
    }
    let v = post(
        &query_search(MEDIA_FIELDS),
        serde_json::json!({ "search": term, "page": page.max(1), "perPage": per_page.clamp(1, 50) }),
    )
    .await?;
    Ok(parse_page(&v))
}

/// The current season's shows, most popular first.
pub async fn seasonal(season: &str, year: i64, per_page: i64) -> Result<Vec<Title>> {
    let v = post(
        &query_seasonal(MEDIA_FIELDS),
        serde_json::json!({ "season": season, "year": year, "page": 1, "perPage": per_page.clamp(1, 50) }),
    )
    .await?;
    Ok(parse_page(&v).0)
}

pub async fn trending(per_page: i64) -> Result<Vec<Title>> {
    let v = post(
        &query_trending(MEDIA_FIELDS),
        serde_json::json!({ "page": 1, "perPage": per_page.clamp(1, 50) }),
    )
    .await?;
    Ok(parse_page(&v).0)
}

/// One title by AniList id — used when reopening a bookmark, where the saved
/// row has the id but not the current airing state.
pub async fn by_id(id: i64) -> Result<Option<Title>> {
    let v = post(&query_by_id(MEDIA_FIELDS), serde_json::json!({ "id": id })).await?;
    Ok(v.get("data").and_then(|d| d.get("Media")).and_then(parse_media))
}

/// Northern-hemisphere season name AniList expects, for a month 1..=12.
pub fn season_of(month: u32) -> &'static str {
    match month {
        1..=3 => "WINTER",
        4..=6 => "SPRING",
        7..=9 => "SUMMER",
        _ => "FALL",
    }
}

fn parse_page(v: &Value) -> (Vec<Title>, bool) {
    let page = match v.get("data").and_then(|d| d.get("Page")) {
        Some(p) => p,
        None => return (Vec::new(), false),
    };
    let more = page
        .get("pageInfo")
        .and_then(|p| p.get("hasNextPage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let list = page
        .get("media")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(parse_media).collect())
        .unwrap_or_default();
    (list, more)
}

fn parse_media(m: &Value) -> Option<Title> {
    if m.is_null() {
        return None;
    }
    let s = |path: &[&str]| -> Option<String> {
        let mut cur = m;
        for p in path {
            cur = cur.get(p)?;
        }
        cur.as_str().map(str::to_string)
    };
    let romaji = s(&["title", "romaji"]).unwrap_or_default();
    let english = s(&["title", "english"]);
    // Something has to be searchable downstream; a Media with no romaji and no
    // english is not a title we can hand to a provider.
    if romaji.is_empty() && english.is_none() {
        return None;
    }
    let next = m.get("nextAiringEpisode");
    Some(Title {
        anilist_id: m.get("id").and_then(Value::as_i64),
        mal_id: m.get("idMal").and_then(Value::as_i64),
        tmdb_id: None,
        title: if romaji.is_empty() { english.clone().unwrap_or_default() } else { romaji },
        english,
        native: s(&["title", "native"]),
        year: m.get("startDate").and_then(|d| d.get("year")).and_then(Value::as_i64),
        overview: strip_html(&s(&["description"]).unwrap_or_default()),
        cover_url: s(&["coverImage", "extraLarge"])
            .or_else(|| s(&["coverImage", "large"]))
            .unwrap_or_default(),
        banner_url: s(&["bannerImage"]).unwrap_or_default(),
        format: m
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("TV")
            .to_string(),
        episodes: m.get("episodes").and_then(Value::as_i64),
        score: m.get("averageScore").and_then(Value::as_i64),
        genres: m
            .get("genres")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        // AniList has no certification board rating; the age gate reads
        // `isAdult` here and TMDB's `content_ratings` on the other lane.
        certification: String::new(),
        adult: m.get("isAdult").and_then(Value::as_bool).unwrap_or(false),
        next_episode: next.and_then(|n| n.get("episode")).and_then(Value::as_i64),
        next_airing_at: next.and_then(|n| n.get("airingAt")).and_then(Value::as_i64),
    })
}

/// AniList descriptions carry `<br>`, `<i>` and entities. mpv and the UI want
/// text, and pulling in an HTML parser for four tags would be silly.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_entities() {
        let got = strip_html("A <i>mage</i><br><br>who &quot;outlives&quot;  her  party.");
        assert_eq!(got, "A mage who \"outlives\" her party.");
    }

    #[test]
    fn season_names_match_anilist() {
        assert_eq!(season_of(1), "WINTER");
        assert_eq!(season_of(5), "SPRING");
        assert_eq!(season_of(8), "SUMMER");
        assert_eq!(season_of(11), "FALL");
    }

    #[test]
    fn media_without_a_usable_title_is_dropped() {
        let v = serde_json::json!({ "id": 1, "title": { "romaji": "", "english": null } });
        assert!(parse_media(&v).is_none());
    }

    #[test]
    fn english_wins_on_the_card_romaji_is_the_search_key() {
        let v = serde_json::json!({
            "id": 154587, "idMal": 52991,
            "title": { "romaji": "Sousou no Frieren", "english": "Frieren: Beyond Journey's End" },
            "episodes": 28, "format": "TV"
        });
        let t = parse_media(&v).expect("parses");
        assert_eq!(t.display_title(), "Frieren: Beyond Journey's End");
        assert_eq!(t.title, "Sousou no Frieren");
        assert_eq!(t.mal_id, Some(52991));
    }
}
