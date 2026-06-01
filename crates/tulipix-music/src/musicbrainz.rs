//! `np.p4.music.musicbrainz` — MusicBrainz lookup + Cover Art Archive.
//!
//! Builds the query URLs and parses the JSON shape MB returns. Network IO is a
//! thin `reqwest` call; the testable surface is URL construction (MB demands a
//! descriptive User-Agent and a Lucene-escaped query) and best-match scoring.

use anyhow::Result;
use serde::Deserialize;

pub const MB_BASE: &str = "https://musicbrainz.org/ws/2";
pub const CAA_BASE: &str = "https://coverartarchive.org";
pub const USER_AGENT: &str = "Tulipix/0.1 (https://github.com/atishsharma/tulipix)";

/// Escape Lucene special chars so a track titled `AC/DC: Live!` doesn't break
/// the query parser.
pub fn lucene_escape(s: &str) -> String {
    const SPECIAL: &[char] = &['+', '-', '&', '|', '!', '(', ')', '{', '}', '[', ']', '^', '"', '~', '*', '?', ':', '\\', '/'];
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if SPECIAL.contains(&c) { out.push('\\'); }
        out.push(c);
    }
    out
}

/// Recording search URL for an artist + title pair.
pub fn recording_search_url(artist: &str, title: &str) -> String {
    let q = format!("artist:{} AND recording:{}", lucene_escape(artist), lucene_escape(title));
    format!("{MB_BASE}/recording?fmt=json&limit=5&query={}", urlencode(&q))
}

/// Cover Art Archive front-cover URL for a release MBID.
pub fn cover_front_url(release_mbid: &str) -> String {
    format!("{CAA_BASE}/release/{release_mbid}/front")
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct RecordingSearch {
    #[serde(default)]
    pub recordings: Vec<Recording>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Recording {
    pub id: String,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<ArtistCredit>,
    #[serde(default)]
    pub releases: Vec<Release>,
    #[serde(default)]
    pub tags: Vec<Tag>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ArtistCredit {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Tag {
    #[serde(default)]
    pub count: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Release {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
}

/// Rich metadata distilled from a recording search, ready to write to the DB.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FetchedMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<i64>,
    pub release_date: Option<String>,
    pub genre: Option<String>,
    pub credits: String,
    pub cover_url: Option<String>,
}

/// Distil the best recording match into a `FetchedMeta` (title / artist / album
/// / year / genre / cover). Returns `None` when the search yielded nothing.
pub fn best_metadata(search: &RecordingSearch) -> Option<FetchedMeta> {
    let r = best_match(search)?;
    let artist = {
        let names: Vec<&str> = r.artist_credit.iter().map(|a| a.name.as_str()).filter(|n| !n.is_empty()).collect();
        names.join(", ")
    };
    let rel = r.releases.first();
    let album = rel.map(|x| x.title.clone()).unwrap_or_default();
    let release_date = rel.and_then(|x| x.date.clone()).filter(|d| !d.is_empty());
    let year = release_date.as_deref()
        .and_then(|d| d.get(0..4)).and_then(|y| y.parse::<i64>().ok())
        .filter(|y| *y > 0);
    let genre = r.tags.iter().max_by_key(|t| t.count)
        .map(|t| t.name.clone()).filter(|g| !g.is_empty());
    // Credits — the recording's artist-credit names (performers), joined.
    let credits = r.artist_credit.iter().map(|a| a.name.as_str())
        .filter(|n| !n.is_empty()).collect::<Vec<_>>().join(", ");
    let cover_url = rel.map(|x| cover_front_url(&x.id));
    Some(FetchedMeta { title: r.title.clone(), artist, album, year, release_date, genre, credits, cover_url })
}

/// Highest-scoring recording, if any.
pub fn best_match(search: &RecordingSearch) -> Option<&Recording> {
    search.recordings.iter().max_by_key(|r| r.score)
}

/// Cover Art Archive front-cover URL for the best recording's first release,
/// if the search yielded one (np.p5.music.art-bio cover fetch).
pub fn best_cover_url(search: &RecordingSearch) -> Option<String> {
    best_match(search)
        .and_then(|r| r.releases.first())
        .map(|rel| cover_front_url(&rel.id))
}

/// Live lookup. Sends the required User-Agent; returns parsed search results.
pub async fn lookup_recording(client: &reqwest::Client, artist: &str, title: &str) -> Result<RecordingSearch> {
    let url = recording_search_url(artist, title);
    let res = client.get(&url).header(reqwest::header::USER_AGENT, USER_AGENT).send().await?;
    Ok(res.json::<RecordingSearch>().await?)
}

/// Artist search URL (np.p5.music.art-bio).
pub fn artist_search_url(artist: &str) -> String {
    let q = format!("artist:{}", lucene_escape(artist));
    format!("{MB_BASE}/artist?fmt=json&limit=3&query={}", urlencode(&q))
}

#[derive(Debug, Deserialize, Default)]
pub struct ArtistSearch {
    #[serde(default)]
    pub artists: Vec<Artist>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Artist {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub score: i64,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub disambiguation: Option<String>,
    #[serde(default, rename = "life-span")]
    pub life_span: Option<LifeSpan>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct LifeSpan {
    #[serde(default)]
    pub begin: Option<String>,
    #[serde(default)]
    pub ended: Option<bool>,
}

/// Compose a short bio/credits blurb from an artist hit.
pub fn artist_blurb(a: &Artist) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(k) = &a.kind { if !k.is_empty() { parts.push(k.clone()); } }
    if let Some(c) = &a.country { if !c.is_empty() { parts.push(c.clone()); } }
    if let Some(ls) = &a.life_span { if let Some(b) = &ls.begin { if !b.is_empty() {
        parts.push(if ls.ended == Some(true) { format!("active from {b}") } else { format!("since {b}") });
    } } }
    let mut blurb = format!("{}", a.name);
    if let Some(d) = &a.disambiguation { if !d.is_empty() { blurb.push_str(&format!(" — {d}")); } }
    if !parts.is_empty() { blurb.push_str(&format!("\n{}", parts.join(" · "))); }
    blurb
}

/// Live artist lookup for bio/credits.
pub async fn lookup_artist(client: &reqwest::Client, artist: &str) -> Result<ArtistSearch> {
    let url = artist_search_url(artist);
    let res = client.get(&url).header(reqwest::header::USER_AGENT, USER_AGENT).send().await?;
    Ok(res.json::<ArtistSearch>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_encodes() {
        let url = recording_search_url("AC/DC", "T.N.T");
        assert!(url.contains("artist%3AAC%5C%2FDC")); // '\' escaped, then ':' '\' '/' percent-encoded
        assert!(url.starts_with(MB_BASE));
        assert!(url.contains("fmt=json"));
    }

    #[test]
    fn artist_blurb_composes() {
        let a = Artist { name: "Radiohead".into(), score: 100, kind: Some("Group".into()),
            country: Some("GB".into()), disambiguation: Some("English band".into()),
            life_span: Some(LifeSpan { begin: Some("1991".into()), ended: Some(false) }) };
        let b = artist_blurb(&a);
        assert!(b.contains("Radiohead — English band"));
        assert!(b.contains("Group · GB · since 1991"));
        assert!(artist_search_url("AC/DC").contains("artist%3AAC%5C%2FDC"));
    }

    #[test]
    fn cover_url_shape() {
        assert_eq!(cover_front_url("abc-123"), "https://coverartarchive.org/release/abc-123/front");
    }

    #[test]
    fn best_match_picks_top_score() {
        let s = RecordingSearch { recordings: vec![
            Recording { id: "a".into(), score: 70, title: "x".into(), artist_credit: vec![], releases: vec![], tags: vec![] },
            Recording { id: "b".into(), score: 99, title: "y".into(), artist_credit: vec![], releases: vec![], tags: vec![] },
        ]};
        assert_eq!(best_match(&s).unwrap().id, "b");
        assert!(best_match(&RecordingSearch { recordings: vec![] }).is_none());
    }
}
