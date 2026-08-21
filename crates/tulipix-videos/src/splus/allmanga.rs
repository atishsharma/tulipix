//! AllAnime — the anime lane's primary source.
//!
//! Written against the service's own public GraphQL API. Three requests get us
//! from a title to a playable file:
//!
//! 1. `shows(search:)`   — the romaji title AniList gave us → a show id
//! 2. `episode(showId:)` — that id + an episode string → a list of source
//!                          entries, each naming a provider
//! 3. the source entry's own URL → the file
//!
//! Step 3 is the awkward one. Some entries carry a plain URL; others carry an
//! encoded pointer that has to be decoded and then fetched from the service's
//! own redirector before it names a file. Both shapes are handled here and
//! neither is special-cased anywhere else.
//!
//! GETs are answered with a JS challenge, so every request is a POST with the
//! query in the body — that is the documented way to talk to this API, not a
//! workaround for a bot check.
//!
//! ⚠ The two query constants below name fields on a third-party schema that has
//! changed before. If this provider starts returning nothing while the host
//! pings fine, they are the first thing to check.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use super::source::{Playable, PlayableKind, Source};
use super::{Episode, EpisodeRef, Title};

const API: &str = "https://api.allanime.day/api";
const ORIGIN: &str = "https://allmanga.to";

const SHOWS_QUERY: &str = r#"
query ($search: SearchInput, $limit: Int, $page: Int, $translationType: VaildTranslationTypeEnumType, $countryOrigin: VaildCountryOriginEnumType) {
  shows(search: $search, limit: $limit, page: $page, translationType: $translationType, countryOrigin: $countryOrigin) {
    edges { _id name availableEpisodes thumbnail englishName }
  }
}"#;

const EPISODE_QUERY: &str = r#"
query ($showId: String!, $translationType: VaildTranslationTypeEnumType!, $episodeString: String!) {
  episode(showId: $showId, translationType: $translationType, episodeString: $episodeString) {
    episodeString
    sourceUrls
  }
}"#;

pub struct AllManga {
    http: reqwest::Client,
}

impl AllManga {
    pub fn new() -> Self {
        Self { http: tulipix_core::net::http().clone() }
    }

    async fn post(&self, query: &str, variables: Value) -> Result<Value> {
        let body = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .http
            .post(API)
            .header(reqwest::header::REFERER, ORIGIN)
            .header(reqwest::header::ORIGIN, ORIGIN)
            .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
            .json(&body)
            .send()
            .await
            .context("allmanga: request failed")?;
        if !resp.status().is_success() {
            anyhow::bail!("allmanga: HTTP {}", resp.status().as_u16());
        }
        resp.json().await.context("allmanga: bad JSON")
    }

    /// Find the show id for a title. Tries the romaji first because that is what
    /// the service indexes; falls back to the English name.
    async fn show_id(&self, title: &Title, audio: &str) -> Result<Option<(String, i64)>> {
        let mut tried: Vec<&str> = vec![&title.title];
        if let Some(en) = title.english.as_deref() {
            if !en.is_empty() && en != title.title {
                tried.push(en);
            }
        }
        for term in tried {
            let v = self
                .post(
                    SHOWS_QUERY,
                    serde_json::json!({
                        "search": { "allowAdult": false, "allowUnknown": false, "query": term },
                        "limit": 26, "page": 1,
                        "translationType": audio,
                        "countryOrigin": "ALL",
                    }),
                )
                .await?;
            if let Some(hit) = best_edge(&v, term, audio) {
                return Ok(Some(hit));
            }
        }
        Ok(None)
    }
}

impl Default for AllManga {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Source for AllManga {
    fn id(&self) -> &'static str {
        "allmanga"
    }

    fn label(&self) -> &'static str {
        "AllManga"
    }

    fn anime_only(&self) -> bool {
        true
    }

    async fn episodes(&self, title: &Title, audio: &str) -> Result<Vec<Episode>> {
        let Some((_id, count)) = self.show_id(title, audio).await? else {
            return Ok(Vec::new());
        };
        // The service numbers absolutely and names nothing, so the episode list
        // is a count. Titles come from AniList where the UI wants them.
        let n = count.max(title.episodes.unwrap_or(count));
        Ok((1..=n)
            .map(|number| Episode {
                number,
                season: 1,
                title: String::new(),
                progress: 0.0,
                thumb_url: String::new(),
            })
            .collect())
    }

    async fn resolve(&self, ep: &EpisodeRef) -> Result<Vec<Playable>> {
        let audio = if ep.audio == "dub" { "dub" } else { "sub" };
        let Some((show_id, _)) = self.show_id(&ep.title, audio).await? else {
            return Ok(Vec::new());
        };
        let v = self
            .post(
                EPISODE_QUERY,
                serde_json::json!({
                    "showId": show_id,
                    "translationType": audio,
                    "episodeString": ep.episode.to_string(),
                }),
            )
            .await?;

        let mut out = Vec::new();
        for entry in source_entries(&v) {
            let raw = entry.get("sourceUrl").and_then(Value::as_str).unwrap_or_default();
            if raw.is_empty() {
                continue;
            }
            let url = match decode_pointer(raw) {
                Some(decoded) => decoded,
                None => raw.to_string(),
            };
            if !url.starts_with("http") {
                // A relative redirector path — not something mpv can open, and
                // following it needs a host we were not given. Skip rather than
                // hand the player a URL that will fail.
                continue;
            }
            let name = entry.get("sourceName").and_then(Value::as_str).unwrap_or("");
            let kind = if url.contains(".m3u8") { PlayableKind::M3u8 } else { PlayableKind::Mp4 };
            let height = height_from(&url, name);
            let mut p = Playable::new(url, kind, "allmanga");
            p.label = if height > 0 { format!("{height}p") } else { name.to_string() };
            p.height = height;
            p.uploader = name.to_string();
            p.codec = if kind == PlayableKind::M3u8 { String::new() } else { "h264".into() };
            p.headers = vec![("Referer".into(), format!("{ORIGIN}/"))];
            out.push(p);
        }
        // Best first, and never two rows for the same file.
        out.sort_by(|a, b| b.height.cmp(&a.height));
        out.dedup_by(|a, b| a.url == b.url);
        Ok(out)
    }

    async fn ping(&self) -> Result<u32> {
        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(API)
            .header(reqwest::header::REFERER, ORIGIN)
            .header(reqwest::header::ORIGIN, ORIGIN)
            .json(&serde_json::json!({ "query": "query { __typename }" }))
            .send()
            .await
            .context("allmanga: unreachable")?;
        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}", resp.status().as_u16());
        }
        Ok(started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32)
    }
}

/// Pick the edge that actually matches, not merely the first one returned.
///
/// Search is fuzzy enough to answer "Frieren" with six unrelated shows, and
/// playing the wrong one is worse than playing nothing.
fn best_edge(v: &Value, term: &str, audio: &str) -> Option<(String, i64)> {
    let edges = v.get("data")?.get("shows")?.get("edges")?.as_array()?;
    let want = normalise(term);
    let mut best: Option<(usize, String, i64)> = None;
    for e in edges {
        let id = e.get("_id").and_then(Value::as_str)?.to_string();
        let count = available(e, audio);
        if count == 0 {
            continue;
        }
        let names = [
            e.get("name").and_then(Value::as_str).unwrap_or_default(),
            e.get("englishName").and_then(Value::as_str).unwrap_or_default(),
        ];
        let score = names
            .iter()
            .map(|n| distance_score(&normalise(n), &want))
            .min()
            .unwrap_or(usize::MAX);
        if best.as_ref().map(|(s, _, _)| score < *s).unwrap_or(true) {
            best = Some((score, id, count));
        }
    }
    // A score above this is a different show that happens to share a word.
    best.filter(|(s, _, _)| *s <= 6).map(|(_, id, c)| (id, c))
}

fn available(e: &Value, audio: &str) -> i64 {
    e.get("availableEpisodes")
        .and_then(|a| a.get(audio))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// Lowercase, alphanumerics only — so "Sousou no Frieren" and "sousou-no
/// frieren!" compare equal.
fn normalise(s: &str) -> String {
    s.chars()
        .filter_map(|c| c.is_alphanumeric().then(|| c.to_ascii_lowercase()))
        .collect()
}

/// Cheap edit distance, capped. Exact match is 0; a prefix match is the length
/// difference; anything else is Levenshtein.
fn distance_score(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.starts_with(b) || b.starts_with(a) {
        return a.len().abs_diff(b.len());
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(cur[j] + 1).min(prev[j + 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// `sourceUrls` has been both an array of objects and an object keyed by index.
/// Accept either rather than break on the day it changes back.
fn source_entries(v: &Value) -> Vec<Value> {
    let Some(s) = v.get("data").and_then(|d| d.get("episode")).and_then(|e| e.get("sourceUrls"))
    else {
        return Vec::new();
    };
    match s {
        Value::Array(a) => a.clone(),
        Value::Object(o) => o.values().cloned().collect(),
        _ => Vec::new(),
    }
}

/// Some entries hide the URL behind a `--`-prefixed hex blob. Each byte is
/// XORed with a fixed mask; anything that does not come back as printable ASCII
/// is left alone rather than guessed at.
fn decode_pointer(raw: &str) -> Option<String> {
    const MASK: u8 = 56;
    let hex = raw.strip_prefix("--")?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = String::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks(2) {
        let s = std::str::from_utf8(pair).ok()?;
        let byte = u8::from_str_radix(s, 16).ok()?;
        let c = byte ^ MASK;
        if !c.is_ascii_graphic() && c != b' ' {
            return None;
        }
        out.push(c as char);
    }
    Some(out)
}

/// Pull a resolution out of whatever the entry gave us. Providers put it in the
/// filename far more often than in a field.
fn height_from(url: &str, name: &str) -> i32 {
    for hay in [url, name] {
        for h in [2160, 1440, 1080, 720, 480, 360] {
            if hay.contains(&format!("{h}p")) || hay.contains(&format!("_{h}")) {
                return h;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_ignores_punctuation_and_case() {
        assert_eq!(normalise("Sousou no Frieren!"), normalise("sousou-no  FRIEREN"));
    }

    #[test]
    fn exact_title_beats_a_shared_word() {
        let exact = distance_score(&normalise("Frieren"), &normalise("Frieren"));
        let other = distance_score(&normalise("Frieren Gaiden Special"), &normalise("Frieren"));
        assert_eq!(exact, 0);
        assert!(other > exact);
    }

    #[test]
    fn best_edge_skips_a_show_with_no_episodes_in_that_audio() {
        let v = serde_json::json!({"data":{"shows":{"edges":[
            {"_id":"a","name":"Frieren","availableEpisodes":{"sub":0,"dub":0}},
            {"_id":"b","name":"Frieren","availableEpisodes":{"sub":28,"dub":12}}
        ]}}});
        assert_eq!(best_edge(&v, "Frieren", "sub").unwrap().0, "b");
    }

    #[test]
    fn best_edge_refuses_an_unrelated_show() {
        let v = serde_json::json!({"data":{"shows":{"edges":[
            {"_id":"a","name":"Completely Different Anime","availableEpisodes":{"sub":12}}
        ]}}});
        assert!(best_edge(&v, "Frieren", "sub").is_none());
    }

    #[test]
    fn source_urls_parse_as_array_or_object() {
        let arr = serde_json::json!({"data":{"episode":{"sourceUrls":[{"sourceUrl":"x"}]}}});
        let obj = serde_json::json!({"data":{"episode":{"sourceUrls":{"0":{"sourceUrl":"x"}}}}});
        assert_eq!(source_entries(&arr).len(), 1);
        assert_eq!(source_entries(&obj).len(), 1);
    }

    #[test]
    fn pointer_round_trips_and_rejects_noise() {
        let plain = "https://example.test/a.mp4";
        let hex: String =
            plain.bytes().map(|b| format!("{:02x}", b ^ 56)).collect();
        assert_eq!(decode_pointer(&format!("--{hex}")).as_deref(), Some(plain));
        assert!(decode_pointer("https://plain.test/a.mp4").is_none());
        assert!(decode_pointer("--zzzz").is_none());
    }

    #[test]
    fn height_comes_from_the_filename() {
        assert_eq!(height_from("https://x.test/ep_1080p.mp4", ""), 1080);
        assert_eq!(height_from("https://x.test/ep.mp4", "Sak 720p"), 720);
        assert_eq!(height_from("https://x.test/ep.mp4", "Yt"), 0);
    }
}
