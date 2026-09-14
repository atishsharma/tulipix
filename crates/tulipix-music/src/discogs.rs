//! Discogs — the second music metadata source, for what MusicBrainz has no
//! match for. Discogs is strongest exactly where MusicBrainz is weakest:
//! self-released records, reissues, and anything pressed before the web.
//!
//! Authentication is a personal access token, taken from the account page and
//! sent in the `Authorization` header. Discogs is firm about two things: a
//! descriptive `User-Agent` (a default one is answered with `403`), and 60
//! authenticated requests a minute.

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const DISCOGS_BASE: &str = "https://api.discogs.com";
pub const DISCOGS_USER_AGENT: &str =
    concat!("Tulipix/", env!("CARGO_PKG_VERSION"), " +https://github.com/atishsharma/tulipix");

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiscogsArtist {
    pub id: i64,
    pub name: String,
    /// Their biography, as typed by the contributor. Discogs marks up links as
    /// `[a=Name]` and `[l=123]`; `clean_profile` takes those out.
    pub profile: String,
    pub image_url: Option<String>,
    /// "Detroit, USA", when the page says.
    pub area: Option<String>,
    pub members: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiscogsRelease {
    pub id: i64,
    pub title: String,
    pub year: Option<i64>,
    pub genres: Vec<String>,
    pub styles: Vec<String>,
    pub label: Option<String>,
    pub cover_url: Option<String>,
}

pub struct DiscogsClient {
    token: String,
    http: reqwest::Client,
}

impl DiscogsClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self { token: token.into(), http: tulipix_core::net::http().clone() }
    }

    fn get(&self, url: String) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header(reqwest::header::USER_AGENT, DISCOGS_USER_AGENT)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Discogs token={}", self.token),
            )
    }

    /// The artist page for a name. `None` when Discogs knows no such artist —
    /// which is the ordinary answer for a local recording, and not an error.
    pub async fn artist(&self, name: &str) -> Result<Option<DiscogsArtist>> {
        let search: serde_json::Value = self
            .get(format!("{DISCOGS_BASE}/database/search"))
            .query(&[("type", "artist"), ("q", name), ("per_page", "5")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let Some(id) = best_artist_id(&search, name) else { return Ok(None) };
        let v: serde_json::Value = self
            .get(format!("{DISCOGS_BASE}/artists/{id}"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(Some(parse_artist(&v)))
    }

    /// The release that best matches an artist and album pair.
    pub async fn release(&self, artist: &str, album: &str) -> Result<Option<DiscogsRelease>> {
        let v: serde_json::Value = self
            .get(format!("{DISCOGS_BASE}/database/search"))
            .query(&[
                ("type", "release"),
                ("artist", artist),
                ("release_title", album),
                ("per_page", "5"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(v["results"].as_array().and_then(|a| a.first()).map(parse_release))
    }

    /// One cheap authenticated call, for Settings' Test button.
    pub async fn check(&self) -> Result<bool> {
        let resp = self
            .get(format!("{DISCOGS_BASE}/database/search"))
            .query(&[("q", "a"), ("type", "artist"), ("per_page", "1")])
            .send()
            .await?;
        Ok(resp.status().is_success())
    }
}

/// The first hit whose name matches exactly, else the first hit. Discogs
/// ranks by its own popularity, which puts a tribute band above the band.
fn best_artist_id(search: &serde_json::Value, want: &str) -> Option<i64> {
    let results = search["results"].as_array()?;
    let exact = results.iter().find(|r| {
        r["title"]
            .as_str()
            .is_some_and(|t| t.trim().eq_ignore_ascii_case(want.trim()))
    });
    exact
        .or_else(|| results.first())
        .and_then(|r| r["id"].as_i64())
}

fn parse_artist(v: &serde_json::Value) -> DiscogsArtist {
    DiscogsArtist {
        id: v["id"].as_i64().unwrap_or(0),
        name: v["name"].as_str().unwrap_or_default().to_string(),
        profile: clean_profile(v["profile"].as_str().unwrap_or_default()),
        image_url: v["images"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|i| i["uri"].as_str())
            .map(str::to_string),
        area: v["profile"].as_str().and_then(area_from_profile),
        members: v["members"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["name"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        url: v["uri"].as_str().unwrap_or_default().to_string(),
    }
}

fn parse_release(v: &serde_json::Value) -> DiscogsRelease {
    let strings = |key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str()).map(str::to_string).collect())
            .unwrap_or_default()
    };
    DiscogsRelease {
        id: v["id"].as_i64().unwrap_or(0),
        title: v["title"].as_str().unwrap_or_default().to_string(),
        // Search results carry the year as a string often enough to be worth
        // taking both shapes.
        year: v["year"]
            .as_i64()
            .or_else(|| v["year"].as_str().and_then(|s| s.parse().ok())),
        genres: strings("genre"),
        styles: strings("style"),
        label: v["label"].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).map(str::to_string),
        cover_url: v["cover_image"].as_str().map(str::to_string).filter(|u| !u.is_empty()),
    }
}

/// Discogs' own markup out of a biography: `[a=Kraftwerk]` is a link to an
/// artist, `[l=Motown]` to a label, `[r=123]` to a release, and `[url=…]…[/url]`
/// is a plain link. The names inside stay; the brackets go.
pub fn clean_profile(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find(']') else {
            out.push_str(&rest[open..]);
            return out.trim().to_string();
        };
        let tag = &rest[open + 1..open + close];
        // `[a=Name]` / `[l=Name]` / `[m=Name]` keep the name; `[a123]`,
        // `[/url]` and the rest are dropped whole.
        if let Some((kind, name)) = tag.split_once('=') {
            // `[url=…]` is dropped whole -- the text after it is the label,
            // and it is already in `rest`.
            if matches!(kind, "a" | "l" | "m" | "r") {
                out.push_str(name);
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Discogs has no country field on an artist; contributors write the place in
/// the first line of the profile often enough that it is worth reading, and
/// never worth guessing at. Only a first line that is short and holds a comma
/// is taken for a place.
fn area_from_profile(profile: &str) -> Option<String> {
    let first = profile.lines().next()?.trim();
    (first.len() <= 48 && first.contains(',') && !first.ends_with('.'))
        .then(|| clean_profile(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_name_beats_a_higher_ranked_tribute_band() {
        let v = serde_json::json!({"results":[
            {"id": 1, "title": "Kraftwerk Tribute"},
            {"id": 2, "title": "Kraftwerk"}
        ]});
        assert_eq!(best_artist_id(&v, "kraftwerk"), Some(2));
        // Nothing matches exactly: the top hit stands.
        assert_eq!(best_artist_id(&v, "nobody"), Some(1));
        assert_eq!(best_artist_id(&serde_json::json!({"results":[]}), "x"), None);
    }

    #[test]
    fn discogs_markup_leaves_the_names_behind() {
        assert_eq!(clean_profile("Founded by [a=Ralf] and [a=Florian]."), "Founded by Ralf and Florian.");
        assert_eq!(clean_profile("Signed to [l=Motown]"), "Signed to Motown");
        assert_eq!(clean_profile("See [url=http://x]here[/url]"), "See here");
        // An unclosed bracket is left as typed rather than eating the rest.
        assert_eq!(clean_profile("half [a=open"), "half [a=open");
    }

    #[test]
    fn an_artist_payload_reads_whole() {
        let v = serde_json::json!({
            "id": 3, "name": "Kraftwerk", "uri": "https://www.discogs.com/artist/3",
            "profile": "Düsseldorf, Germany\nFounded by [a=Ralf].",
            "images": [{"uri": "https://img/1.jpg"}],
            "members": [{"name": "Ralf Hütter"}, {"name": "Florian Schneider"}]
        });
        let a = parse_artist(&v);
        assert_eq!(a.id, 3);
        assert_eq!(a.members.len(), 2);
        assert_eq!(a.area.as_deref(), Some("Düsseldorf, Germany"));
        assert!(a.profile.contains("Founded by Ralf."));
        assert_eq!(a.image_url.as_deref(), Some("https://img/1.jpg"));
    }

    #[test]
    fn a_release_takes_the_year_as_a_number_or_a_string() {
        let a = parse_release(&serde_json::json!({"id":1,"title":"X","year":1978,"genre":["Electronic"]}));
        assert_eq!(a.year, Some(1978));
        let b = parse_release(&serde_json::json!({"id":2,"title":"Y","year":"1981","cover_image":""}));
        assert_eq!(b.year, Some(1981));
        assert_eq!(b.cover_url, None);
    }
}
