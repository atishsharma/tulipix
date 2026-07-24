//! Stream — remote catalogue search and playback for the Videos section.
//!
//! Backend ported from MovieBox-Tui (MIT OR Apache-2.0,
//! https://github.com/mesamirh/MovieBox-Tui): `client` carries the signed HTTP
//! transport and `crypto` the request signing. This module is the part that did
//! not exist upstream — the endpoint calls return `serde_json::Value` blobs, so
//! everything here turns those into typed structs the UI can bind to.
//!
//! Parsing is split from the network calls (same shape as `music::youtube::piped`)
//! so the field plumbing is unit-tested offline against captured payloads.
//!
//! The host pool is user-editable: see [`hosts`].

pub mod bookmarks;
pub mod cache;
pub mod client;
pub mod crypto;
pub mod progress;

pub use client::{StreamClient, StreamError, DEFAULT_HOSTS};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---- types ----

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchHit {
    /// Subject to open. For a show split across per-season subjects this is the
    /// lowest season's subject.
    pub id: String,
    pub title: String,
    pub year: String,
    pub cover: String,
    pub is_series: bool,
    /// `(season number, subject id)` for shows the catalogue splits into one
    /// subject per season — "Person of Interest S1" … "S5" arrive as five
    /// separate results and are folded into a single card here. Empty when the
    /// show is a single subject that carries its own `seasons` array.
    pub season_subjects: Vec<(i64, String)>,
}

/// One alternate-language cut of the same title. Each is its own subject id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dub {
    pub subject_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Season {
    pub number: i64,
    pub max_ep: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Details {
    pub id: String,
    pub title: String,
    pub overview: String,
    pub cover: String,
    pub year: String,
    pub rating: f64,
    pub genre: String,
    pub country: String,
    pub content_rating: String,
    pub duration: String,
    pub is_series: bool,
    pub dubs: Vec<Dub>,
    pub seasons: Vec<Season>,
}

/// One playable file. `url` goes straight to mpv.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamFile {
    pub url: String,
    pub resource_id: String,
    pub resolution: i64,
    pub codec: String,
    /// Human-readable ("2.1 GB"). The wire format is a byte count in a string.
    pub size: String,
    pub uploader: String,
    /// Subtitle tracks carried inline on the resource entry — no second request
    /// needed for the common case.
    pub captions: Vec<Caption>,
}

impl StreamFile {
    /// Two entries the user could not tell apart: same quality, size, codec and
    /// uploader. Identical URLs count too, for the case where every other field
    /// is blank.
    pub fn is_same_offer(&self, other: &StreamFile) -> bool {
        self.url == other.url
            || (self.resolution == other.resolution
                && self.size == other.size
                && self.codec == other.codec
                && self.uploader == other.uploader)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Caption {
    pub url: String,
    pub lang: String,
    pub ext: String,
}

// ---- shared field helpers ----

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or_default().to_string()
}

fn i(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(|x| x.as_i64()).unwrap_or(0)
}

/// `cover` is an object (`{"url": …}`) on most endpoints but a bare string on
/// some. Accept either rather than losing the artwork.
fn cover_url(v: &Value) -> String {
    match v.get("cover") {
        Some(Value::String(u)) => u.clone(),
        Some(obj) => obj.get("url").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
        None => String::new(),
    }
}

/// Titles arrive decorated — `"The Movie [HD][Hindi]"`. Keep the name only.
fn clean_title(raw: &str) -> String {
    raw.split('[').next().unwrap_or(raw).trim().to_string()
}

/// `"2024-06-14"` → `"2024"`. Anything without a leading 4-digit year is dropped.
fn year_of(release_date: &str) -> String {
    let head: String = release_date.chars().take(4).collect();
    if head.len() == 4 && head.chars().all(|c| c.is_ascii_digit()) {
        head
    } else {
        String::new()
    }
}

/// `subjectType`/`stype` 2 means series; everything else is treated as a movie.
fn is_series(v: &Value) -> bool {
    v.get("subjectType")
        .or_else(|| v.get("stype"))
        .and_then(|t| t.as_i64())
        .unwrap_or(1)
        == 2
}

// ---- parsers ----

/// Search payload → hits. Shape: `results[0].subjects[]`.
///
/// Duplicates (same title + year + kind under different subject ids) are
/// collapsed, and series sort above movies, newest first — the raw order is
/// close to random.
pub fn parse_search(payload: &Value, query: &str) -> Vec<SearchHit> {
    let subjects = payload
        .get("results")
        .and_then(|r| r.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("subjects"))
        .and_then(|s| s.as_array());
    let Some(subjects) = subjects else {
        return Vec::new();
    };

    // Pass 1 — clean, split the season suffix off, drop the unrelated.
    struct Raw {
        related: bool,
        id: String,
        base: String,
        season: Option<i64>,
        year: String,
        cover: String,
        is_series: bool,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for item in subjects {
        let id = s(item, "subjectId");
        if id.is_empty() {
            continue;
        }
        let (base, season) = split_season_suffix(&clean_title(&s(item, "title")));
        let related = matches_query(&base, query);
        raws.push(Raw {
            related,
            id,
            base,
            season,
            year: year_of(&s(item, "releaseDate")),
            cover: cover_url(item),
            is_series: is_series(item),
        });
    }

    // Drop the loosely-related noise — but fail open. If the filter would empty
    // the screen, the query simply did not resemble the catalogue's naming
    // (an abbreviation, a translated title), and showing everything beats
    // claiming there were no results.
    if raws.iter().any(|r| r.related) {
        raws.retain(|r| r.related);
    }

    // Pass 2 — fold per-season subjects of the same show into one hit, keeping
    // every season's subject id so the detail pane can switch between them.
    let mut out: Vec<SearchHit> = Vec::new();
    for raw in raws {
        let key = raw.base.to_lowercase();
        let mergeable = raw.is_series && raw.season.is_some();
        if mergeable {
            if let Some(existing) = out
                .iter_mut()
                .find(|o| o.is_series && o.title.to_lowercase() == key && !o.season_subjects.is_empty())
            {
                let season = raw.season.unwrap_or(1);
                if !existing.season_subjects.iter().any(|(n, _)| *n == season) {
                    existing.season_subjects.push((season, raw.id));
                }
                continue;
            }
        }
        // Plain duplicate (same show under a second subject id, different dub).
        if !mergeable
            && out.iter().any(|o| {
                o.title.to_lowercase() == key && o.year == raw.year && o.is_series == raw.is_series
            })
        {
            continue;
        }
        out.push(SearchHit {
            id: raw.id.clone(),
            title: raw.base,
            year: raw.year,
            cover: raw.cover,
            is_series: raw.is_series,
            season_subjects: match raw.season {
                Some(n) if raw.is_series => vec![(n, raw.id)],
                _ => Vec::new(),
            },
        });
    }

    // Pass 3 — a merged card opens on, and is labelled by, its earliest season.
    for hit in out.iter_mut() {
        if hit.season_subjects.len() < 2 {
            // A lone "S1" is not really a split show; drop the season list so it
            // falls back to whatever seasons its own payload declares.
            hit.season_subjects.clear();
            continue;
        }
        hit.season_subjects.sort_by_key(|(n, _)| *n);
        if let Some((_, id)) = hit.season_subjects.first() {
            hit.id = id.clone();
        }
    }

    out.sort_by(|a, b| b.is_series.cmp(&a.is_series).then_with(|| b.year.cmp(&a.year)));
    out
}

/// Split a trailing season marker off a title: `"Person of Interest S5"` →
/// `("Person of Interest", Some(5))`. Handles `S5` and `Season 5`; anything else
/// comes back untouched.
pub fn split_season_suffix(title: &str) -> (String, Option<i64>) {
    let t = title.trim().trim_end_matches(|c: char| c == '-' || c == ':' || c == '.').trim();

    // "… Season 5" / "… Season Two" / "… Part 2" / "… Part III"
    for word in [" season ", " part ", " series "] {
        if let Some(pos) = rfind_ci(t, word) {
            let tail = t[pos + word.len()..].trim();
            if let Some(n) = parse_number(tail) {
                return (trim_tail(&t[..pos]), Some(n));
            }
        }
    }

    // "… S5"
    if let Some((head, tail)) = t.rsplit_once(' ') {
        let mut chars = tail.chars();
        if matches!(chars.next(), Some('S') | Some('s')) {
            let digits: String = chars.collect();
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                if let Ok(n) = digits.parse::<i64>() {
                    return (trim_tail(head), Some(n));
                }
            }
        }
    }

    (t.to_string(), None)
}

/// Byte offset of the last case-insensitive occurrence of an ASCII `needle`.
///
/// Searching `t.to_lowercase()` and then slicing `t` with the offsets it returns
/// is not the same string: `'İ'` lowercases to two chars (three bytes) where the
/// original is two, so a title like `"İİ Season 2"` produced an offset past the
/// end of `t` — an out-of-bounds slice, i.e. a panic on a catalogue title we do
/// not control. Titles are attacker/server-supplied, so match on `t` itself.
fn rfind_ci(hay: &str, needle: &str) -> Option<usize> {
    hay.char_indices().rev().map(|(i, _)| i).find(|&i| {
        // `get` yields None for a window that runs off the end or splits a char.
        hay.get(i..i + needle.len()).is_some_and(|w| w.eq_ignore_ascii_case(needle))
    })
}

fn trim_tail(s: &str) -> String {
    s.trim().trim_end_matches(|c: char| c == '-' || c == ':' || c == ',').trim().to_string()
}

/// A season number written as digits, a Roman numeral, or an English word.
/// Returns `None` for anything else, so "Part of the Deal" is not a season.
fn parse_number(raw: &str) -> Option<i64> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c.is_ascii_digit()) {
        return t.parse().ok().filter(|n| (1..=200).contains(n));
    }
    let lower = t.to_lowercase();
    const WORDS: &[&str] = &[
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen",
        "eighteen", "nineteen", "twenty",
    ];
    if let Some(idx) = WORDS.iter().position(|w| *w == lower) {
        return Some(idx as i64 + 1);
    }
    roman(&lower)
}

/// Roman numeral 1–39, lowercase. `None` if any character is not a numeral or
/// the sequence does not read as one (so "mix" or "did" are rejected).
fn roman(s: &str) -> Option<i64> {
    if s.is_empty() || !s.chars().all(|c| matches!(c, 'i' | 'v' | 'x')) {
        return None;
    }
    let val = |c: char| match c {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        _ => 0,
    };
    let chars: Vec<char> = s.chars().collect();
    let mut total = 0i64;
    for (idx, c) in chars.iter().enumerate() {
        let v = val(*c);
        let next = chars.get(idx + 1).map(|n| val(*n)).unwrap_or(0);
        if v < next {
            total -= v;
        } else {
            total += v;
        }
    }
    // Round-trip so only canonical numerals pass: "iiii" and "vx" are rejected.
    (1..=39).contains(&total).then_some(total).filter(|n| to_roman(*n) == s)
}

fn to_roman(mut n: i64) -> String {
    const TABLE: &[(i64, &str)] = &[
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut out = String::new();
    for (v, sym) in TABLE {
        while n >= *v {
            out.push_str(sym);
            n -= v;
        }
    }
    out
}

/// Is this result actually related to what was typed?
///
/// The catalogue answers a search with a lot of loosely-associated titles — a
/// search for "person of interest" comes back carrying "Personal Trainer" and
/// "Conflict of Interest". Upstream drops anything whose title does not contain
/// the query, and so do we; the reverse direction is allowed too so that typing
/// a full title still matches a shorter catalogue name.
fn matches_query(title: &str, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    let t = title.trim().to_lowercase();
    t.contains(&q) || q.contains(&t)
}

/// Suggestion payload → title strings (same envelope as search).
pub fn parse_suggest(payload: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for hit in parse_search(payload, "") {
        if !hit.title.is_empty() && !out.contains(&hit.title) {
            out.push(hit.title);
        }
    }
    out.truncate(8);
    out
}

/// Detail payload → typed details, including the season list.
///
/// Seasons live at `seasons.seasons[]`. When that is missing on a series, the
/// episode count is recovered from `resourceDetectors[0].totalEpisode` and
/// presented as a single season — same fallback the upstream TUI uses.
pub fn parse_details(payload: &Value) -> Details {
    let series = is_series(payload);

    let dubs = payload
        .get("dubs")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .map(|d| Dub { subject_id: s(d, "subjectId"), name: s(d, "lanName") })
                .filter(|d| !d.subject_id.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let seasons = match payload
        .get("seasons")
        .and_then(|s| s.get("seasons"))
        .and_then(|s| s.as_array())
    {
        Some(arr) => arr
            .iter()
            .map(|se| Season {
                number: se.get("se").and_then(|v| v.as_i64()).unwrap_or(1),
                max_ep: se.get("maxEp").and_then(|v| v.as_i64()).unwrap_or(1).max(1),
            })
            .collect(),
        None if series => {
            let max_ep = payload
                .get("resourceDetectors")
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("totalEpisode"))
                .and_then(|t| t.as_i64())
                .unwrap_or(1)
                .max(1);
            vec![Season { number: 1, max_ep }]
        }
        None => Vec::new(),
    };

    let overview = {
        let d = s(payload, "description");
        if d.is_empty() { s(payload, "intro") } else { d }
    };

    Details {
        id: s(payload, "id"),
        title: clean_title(&s(payload, "title")),
        overview,
        cover: cover_url(payload),
        year: year_of(&s(payload, "releaseDate")),
        rating: payload
            .get("imdbRatingValue")
            .and_then(|r| r.as_f64().or_else(|| r.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0),
        genre: s(payload, "genre"),
        country: s(payload, "countryName"),
        content_rating: s(payload, "contentRating"),
        duration: s(payload, "duration"),
        is_series: series,
        dubs,
        seasons,
    }
}

/// Resource payload → playable files, best resolution first.
///
/// Entries without a `resourceLink` are unplayable and dropped; duplicate URLs
/// (the same file surfaced under several resolution queries) are collapsed.
pub fn parse_resources(payload: &Value) -> Vec<StreamFile> {
    parse_resources_for(payload, 0, 0)
}

/// As [`parse_resources`], but keeps only the entries for `want_episode`
/// (and `want_season`, when > 0).
///
/// The resource endpoint returns the **whole season pack** — every episode's
/// files in one list — and ignores the `ep` query param for many subjects, so
/// se=1&ep=1 comes back carrying episodes 1..N. Each entry, though, is stamped
/// with its own episode/season, so we filter client-side. An entry that carries
/// no episode stamp is kept (movies, and packs that really did filter server
/// side), so this never hides a legitimate single-episode result.
pub fn parse_resources_for(payload: &Value, want_season: i64, want_episode: i64) -> Vec<StreamFile> {
    let Some(list) = payload.get("list").and_then(|l| l.as_array()) else {
        return Vec::new();
    };
    let mut out: Vec<StreamFile> = Vec::new();
    for file in list {
        let url = s(file, "resourceLink");
        if url.is_empty() {
            continue;
        }
        // Episode/season filter — only when the entry actually declares one.
        if want_episode > 0 {
            if let Some(ep) = entry_num(file, &["ep", "episode", "epNum", "episodeNum"]) {
                if ep != want_episode {
                    continue;
                }
            }
        }
        if want_season > 0 {
            if let Some(se) = entry_num(file, &["se", "season", "seNum", "seasonNum"]) {
                if se != want_season {
                    continue;
                }
            }
        }
        let candidate = StreamFile {
            url,
            resource_id: s(file, "resourceId"),
            resolution: i(file, "resolution"),
            codec: s(file, "codecName"),
            size: human_size(&s(file, "size")),
            uploader: s(file, "uploadBy"),
            captions: parse_captions(file),
        };
        // The catalogue returns the same file over and over under different
        // URLs — a single episode can come back as 100 rows that are all
        // "1080p · hevc · 963 MB · Alice". Collapse by what the user can
        // actually see; only genuinely different files survive.
        if out.iter().any(|f| f.is_same_offer(&candidate)) {
            continue;
        }
        out.push(candidate);
    }
    out.sort_by(|a, b| b.resolution.cmp(&a.resolution));
    out
}

/// First of `keys` present on `file` as a positive integer (numbers or numeric
/// strings). `None` when none is present — meaning "unstamped, keep it".
fn entry_num(file: &Value, keys: &[&str]) -> Option<i64> {
    for k in keys {
        if let Some(v) = file.get(*k) {
            let n = v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()));
            if let Some(n) = n {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// `"463696088"` (bytes, as a string) → `"442 MB"` / `"2.1 GB"`. Unparseable
/// input yields an empty string rather than a misleading zero.
pub fn human_size(raw: &str) -> String {
    let Ok(bytes) = raw.trim().parse::<f64>() else {
        return String::new();
    };
    if bytes <= 0.0 {
        return String::new();
    }
    let mb = bytes / 1024.0 / 1024.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

/// External-caption payload → subtitle tracks.
pub fn parse_captions(payload: &Value) -> Vec<Caption> {
    let Some(list) = payload
        .get("extCaptions")
        .and_then(|c| c.as_array())
        .or_else(|| payload.get("list").and_then(|c| c.as_array()))
    else {
        return Vec::new();
    };
    list.iter()
        .map(|c| Caption { url: s(c, "url"), lang: s(c, "lanName"), ext: s(c, "ext") })
        .filter(|c| !c.url.is_empty())
        .collect()
}

// ---- endpoint calls ----

impl StreamClient {
    pub async fn search(&self, query: &str, page: usize) -> Result<Vec<SearchHit>, StreamError> {
        let payload = json!({
            "keyword": query, "page": page, "perPage": 20,
            "subjectType": "All", "tabId": "All"
        });
        Ok(parse_search(
            &self.post("/wefeed-mobile-bff/subject-api/search/v2", &payload).await?,
            query,
        ))
    }

    pub async fn suggest(&self, query: &str) -> Result<Vec<String>, StreamError> {
        let payload = json!({
            "keyword": query, "page": 1, "perPage": 8,
            "subjectType": "All", "tabId": "All"
        });
        Ok(parse_suggest(
            &self.post("/wefeed-mobile-bff/subject-api/search/v2", &payload).await?,
        ))
    }

    pub async fn details(&self, subject_id: &str) -> Result<Details, StreamError> {
        let path = format!("/wefeed-mobile-bff/subject-api/get?subjectId={subject_id}");
        let mut payload = self.get(&path).await?;
        // A series carries its full season list on a separate endpoint; the
        // `get` payload alone yields only the 1-season resourceDetectors
        // fallback. Fetch season-info and merge it under "seasons" so
        // parse_details sees every season. (Upstream MovieBox-Tui get_details.)
        if is_series(&payload) {
            let sp = format!("/wefeed-mobile-bff/subject-api/season-info?subjectId={subject_id}");
            if let Ok(info) = self.get(&sp).await {
                if let Value::Object(map) = &mut payload {
                    map.insert("seasons".to_string(), info);
                }
            }
        }
        Ok(parse_details(&payload))
    }

    /// Files for one episode. `season`/`episode` are both 0 for a movie.
    async fn resources_at(
        &self,
        subject_id: &str,
        season: usize,
        episode: usize,
        resolution: &str,
    ) -> Result<Value, StreamError> {
        let res_param = if resolution.is_empty() {
            String::new()
        } else {
            format!("&resolution={resolution}")
        };
        let path = if season == 0 && episode == 0 {
            format!(
                "/wefeed-mobile-bff/subject-api/resource?subjectId={subject_id}&page=1&perPage=20{res_param}"
            )
        } else {
            format!(
                "/wefeed-mobile-bff/subject-api/resource?subjectId={subject_id}&se={season}&ep={episode}&page=1&perPage=20{res_param}"
            )
        };
        self.get(&path).await
    }

    /// Streams for one episode at `resolution` ("1080", "720", …).
    ///
    /// Always fans out over every quality rung and merges. The
    /// resolution-constrained endpoint (`&resolution=720`) drops the per-entry
    /// `se`/`ep` stamps, so its packs cannot be filtered to one episode — that
    /// leaked the whole season into episode 1. The all-quality rungs keep the
    /// stamps, so we fetch those, filter to the requested episode, and then
    /// narrow to the chosen rung client-side. Empty `resolution` = keep all.
    pub async fn resources(
        &self,
        subject_id: &str,
        season: usize,
        episode: usize,
        resolution: &str,
    ) -> Result<Vec<StreamFile>, StreamError> {
        let want_se = season as i64;
        let want_ep = episode as i64;
        let mut files = self.resources_merged(subject_id, season, episode, want_se, want_ep).await?;
        // Narrow to the picked rung once the episode filter has done its work on
        // the stamped, merged set.
        if let Ok(target) = resolution.trim().parse::<i64>() {
            files.retain(|f| f.resolution == target);
        }
        Ok(files)
    }

    /// Fan out over every quality rung and merge, filtering to `want_se`/`want_ep`.
    async fn resources_merged(
        &self,
        subject_id: &str,
        season: usize,
        episode: usize,
        want_se: i64,
        want_ep: i64,
    ) -> Result<Vec<StreamFile>, StreamError> {
        let mut handles = Vec::new();
        for res in ["1080", "720", "480", "360", ""] {
            let c = self.clone();
            let sid = subject_id.to_string();
            let r = res.to_string();
            handles.push(tokio::spawn(async move {
                tokio::time::timeout(
                    std::time::Duration::from_secs(4),
                    c.resources_at(&sid, season, episode, &r),
                )
                .await
                .unwrap_or(Err(StreamError::ApiStatus(408)))
            }));
        }

        let mut merged: Vec<Value> = Vec::new();
        let mut last_err = None;
        for h in handles {
            match h.await {
                Ok(Ok(res)) => {
                    if let Some(list) = res.get("list").and_then(|l| l.as_array()) {
                        merged.extend(list.clone());
                    }
                }
                Ok(Err(e)) => last_err = Some(e),
                Err(_) => {} // task panicked/cancelled — other rungs may still land
            }
        }
        if merged.is_empty() {
            return Err(last_err.unwrap_or(StreamError::ApiStatus(404)));
        }
        Ok(parse_resources_for(&json!({ "list": merged }), want_se, want_ep))
    }

    pub async fn captions(
        &self,
        subject_id: &str,
        resource_id: &str,
    ) -> Result<Vec<Caption>, StreamError> {
        let path = format!(
            "/wefeed-mobile-bff/subject-api/get-ext-captions?subjectId={subject_id}&resourceId={resource_id}"
        );
        Ok(parse_captions(&self.get(&path).await?))
    }
}

// ---- user-editable host pool ----

/// The Stream tab's Hosts editor. Stored as newline-separated entries in the
/// shared settings file so it round-trips with everything else in Settings.
pub mod hosts {
    use super::DEFAULT_HOSTS;
    use tulipix_core::settings::Settings;

    pub const KEY: &str = "stream.hosts";

    /// Built-in list, as owned strings.
    pub fn defaults() -> Vec<String> {
        DEFAULT_HOSTS.iter().map(|h| h.to_string()).collect()
    }

    /// Normalise one entry, or explain why it is unusable. Hosts come straight
    /// from a text box, so this is the trust boundary for the whole module.
    pub fn validate(raw: &str) -> Result<String, String> {
        let t = raw.trim().trim_end_matches('/');
        if t.is_empty() {
            return Err("Empty host".into());
        }
        let Ok(url) = url::Url::parse(t) else {
            return Err(format!("Not a URL: {t}"));
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("Must start with http:// or https:// — {t}"));
        }
        if url.host_str().unwrap_or_default().is_empty() {
            return Err(format!("No server name in: {t}"));
        }
        if !url.path().is_empty() && url.path() != "/" {
            return Err(format!("Server address only, drop the path: {t}"));
        }
        Ok(t.to_string())
    }

    /// Configured hosts, or the built-in list when unset/blank.
    pub fn load() -> Vec<String> {
        let s = Settings::load().unwrap_or_default();
        let stored: Vec<String> = s
            .text(KEY)
            .lines()
            .filter_map(|l| validate(l).ok())
            .collect();
        if stored.is_empty() { defaults() } else { stored }
    }

    /// Validate then persist. Returns the normalised list that was saved.
    /// Rejects a wholly empty list — no hosts means a dead tab, and the user is
    /// better served by an error than by silently reverting to the defaults.
    pub fn save(raw: &[String]) -> Result<Vec<String>, String> {
        let mut clean: Vec<String> = Vec::new();
        for entry in raw.iter().filter(|e| !e.trim().is_empty()) {
            let host = validate(entry)?;
            if !clean.contains(&host) {
                clean.push(host);
            }
        }
        if clean.is_empty() {
            return Err("Add at least one server, or press Reset for the defaults".into());
        }
        let mut s = Settings::load().unwrap_or_default();
        s.advanced.insert(KEY.to_string(), clean.join("\n"));
        s.save().map_err(|e| format!("Could not save: {e}"))?;
        Ok(clean)
    }
}

/// The Stream tab's signing-key editor. The request signature is HMAC-MD5 over
/// the canonical request keyed by a base64 secret MovieBox bakes into its APK.
/// It rotates only across MovieBox releases; when it does, every host starts
/// returning 403/406. This lets the user paste the new key without a rebuild.
pub mod sign_key {
    use super::crypto;
    use tulipix_core::settings::Settings;

    pub const KEY: &str = "stream.sign_key";

    /// Built-in default — the "Reset" value.
    pub fn default() -> String {
        crypto::default_sign_key().to_string()
    }

    /// A key must be non-empty and valid base64 that decodes to some bytes;
    /// anything else would silently break every signature. Trust boundary — the
    /// value comes straight from a text box.
    pub fn validate(raw: &str) -> Result<String, String> {
        use base64::Engine;
        let t = raw.trim();
        if t.is_empty() {
            return Err("Empty key".into());
        }
        // Same lenient padding the signer uses, then require non-empty output.
        let mut padded = t.to_string();
        padded.push_str(&"=".repeat((4 - padded.len() % 4) % 4));
        match base64::engine::general_purpose::STANDARD.decode(&padded) {
            Ok(bytes) if !bytes.is_empty() => Ok(t.to_string()),
            Ok(_) => Err("Key decodes to nothing".into()),
            Err(_) => Err("Not valid base64".into()),
        }
    }

    /// Stored key, or the built-in default when unset/blank.
    pub fn load() -> String {
        let stored = Settings::load().unwrap_or_default().text(KEY).trim().to_string();
        if stored.is_empty() { default() } else { stored }
    }

    /// Validate, persist, and install the key for the running process.
    pub fn save(raw: &str) -> Result<String, String> {
        let key = validate(raw)?;
        let mut s = Settings::load().unwrap_or_default();
        s.advanced.insert(KEY.to_string(), key.clone());
        s.save().map_err(|e| format!("Could not save: {e}"))?;
        crypto::set_sign_key(&key);
        Ok(key)
    }

    /// Push the stored key into the signer. Call once at startup so a key saved
    /// in a prior session is in force before the first request.
    pub fn apply() {
        crypto::set_sign_key(&load());
    }
}

/// Language focus for the detail pane. The catalogue offers many dubs and
/// subtitle tracks; most users want a short, familiar list. These keep the
/// picker to a few languages and **fail open** — if nothing matches, the full
/// list is returned rather than an empty one.
pub mod prefer {
    use super::{Caption, Dub};

    /// Dub languages to surface, matched case-insensitively as a substring so
    /// "Original Audio", "English", "Hindi Dub" all count.
    pub const DUBS: &[&str] = &["original", "english", "hindi"];
    /// Subtitle languages to surface.
    pub const SUBS: &[&str] = &["english", "hindi"];

    fn matches(name: &str, wanted: &[&str]) -> bool {
        let n = name.to_lowercase();
        wanted.iter().any(|w| n.contains(w))
    }

    /// Keep only preferred dubs; return all if none match.
    pub fn dubs(list: Vec<Dub>) -> Vec<Dub> {
        let kept: Vec<Dub> = list.iter().filter(|d| matches(&d.name, DUBS)).cloned().collect();
        if kept.is_empty() { list } else { kept }
    }

    /// Keep only preferred subtitle tracks; return all if none match.
    pub fn captions(list: Vec<Caption>) -> Vec<Caption> {
        let kept: Vec<Caption> = list.iter().filter(|c| matches(&c.lang, SUBS)).cloned().collect();
        if kept.is_empty() { list } else { kept }
    }
}

/// Which stream to play when the user has not picked a row.
///
/// Every episode of a show offers a different set of files, so "the one below
/// the cursor" cannot carry over. What carries over is the *rung*: the quality
/// of the last stream actually launched. Nothing launched yet means 720p — high
/// enough to look right, low enough to start quickly on a slow line.
pub mod quality {
    use super::StreamFile;
    use tulipix_core::settings::Settings;

    /// Rung used until the user's own choice supersedes it.
    pub const DEFAULT_RESOLUTION: i64 = 720;

    /// Sticky rung: the quality of the last stream the user actually played.
    pub const STICKY_KEY: &str = "stream.play_res";
    /// The quality *filter* in the detail pane ("" = every rung).
    pub const FILTER_KEY: &str = "stream.resolution";

    /// Index of the file whose rung `keep` accepts and that `better` ranks above
    /// every other. Ties go to the earlier entry — within a rung the list
    /// arrives best-offer-first.
    fn pick_where(
        files: &[StreamFile],
        keep: impl Fn(i64) -> bool,
        better: impl Fn(i64, i64) -> bool,
    ) -> Option<usize> {
        let mut best: Option<(usize, i64)> = None;
        for (idx, f) in files.iter().enumerate() {
            if keep(f.resolution) && best.is_none_or(|(_, r)| better(f.resolution, r)) {
                best = Some((idx, f.resolution));
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// Index of the file to arm, or `None` when there is nothing to play.
    ///
    /// Order: the sticky rung, then 720p, then the rung nearest 720 — preferring
    /// the one below, because defaulting *up* to 4K on a title that happens to
    /// skip 720 is not what "720p by default" means. The last fallback catches
    /// files that carry no rung at all.
    pub fn pick(files: &[StreamFile], sticky: Option<i64>) -> Option<usize> {
        let exact = |want: i64| files.iter().position(|f| f.resolution == want);
        sticky
            .filter(|r| *r > 0)
            .and_then(exact)
            .or_else(|| exact(DEFAULT_RESOLUTION))
            // highest rung below 720
            .or_else(|| pick_where(files, |r| r > 0 && r < DEFAULT_RESOLUTION, |a, b| a > b))
            // else the lowest rung above it
            .or_else(|| pick_where(files, |r| r > DEFAULT_RESOLUTION, |a, b| a < b))
            // else anything playable at all
            .or_else(|| pick_where(files, |_| true, |a, b| a > b))
    }

    /// The remembered rung, or `None` when the user has never played anything.
    pub fn sticky() -> Option<i64> {
        Settings::load()
            .unwrap_or_default()
            .text(STICKY_KEY)
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|r| *r > 0)
    }

    /// Remember the rung of a stream that was just launched. Rung 0 ("Auto")
    /// carries no information, so it is not recorded.
    pub fn remember(resolution: i64) {
        if resolution <= 0 {
            return;
        }
        let mut s = Settings::load().unwrap_or_default();
        s.advanced.insert(STICKY_KEY.to_string(), resolution.to_string());
        let _ = s.save();
    }

    /// The persisted quality filter ("" = every rung).
    pub fn filter() -> String {
        Settings::load().unwrap_or_default().text(FILTER_KEY).trim().to_string()
    }

    pub fn set_filter(res: &str) {
        let mut s = Settings::load().unwrap_or_default();
        s.advanced.insert(FILTER_KEY.to_string(), res.trim().to_string());
        let _ = s.save();
    }
}

/// Recent search terms, shown on the Stream landing screen. Same settings-file
/// storage as [`hosts`] — newline-separated, newest first.
pub mod recent {
    use tulipix_core::settings::Settings;

    pub const KEY: &str = "stream.recent";
    const MAX: usize = 5;

    pub fn load() -> Vec<String> {
        Settings::load()
            .unwrap_or_default()
            .text(KEY)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .take(MAX)
            .collect()
    }

    /// Fold a term into the list: newest first, case-insensitively
    /// de-duplicated, capped at `MAX`. A blank term leaves the list alone.
    /// Pure so the ordering rules are testable without touching the settings file.
    pub fn merge(list: &[String], term: &str) -> Vec<String> {
        let term = term.trim();
        if term.is_empty() {
            return list.to_vec();
        }
        let mut out: Vec<String> = list
            .iter()
            .filter(|t| !t.eq_ignore_ascii_case(term))
            .cloned()
            .collect();
        out.insert(0, term.to_string());
        out.truncate(MAX);
        out
    }

    /// Record a term and persist. Returns the new list.
    pub fn push(term: &str) -> Vec<String> {
        let list = merge(&load(), term);
        if term.trim().is_empty() {
            return list;
        }
        store(&list);
        list
    }

    pub fn clear() -> Vec<String> {
        store(&[]);
        Vec::new()
    }

    fn store(list: &[String]) {
        let mut s = Settings::load().unwrap_or_default();
        s.advanced.insert(KEY.to_string(), list.join("\n"));
        let _ = s.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_payload() -> Value {
        json!({"results": [{"subjects": [
            {"subjectId": "1", "title": "Dune Part Two [HD][Hindi]", "subjectType": 1,
             "releaseDate": "2024-02-27", "cover": {"url": "https://img/1.jpg"}},
            {"subjectId": "2", "title": "Severance", "subjectType": 2,
             "releaseDate": "2022-02-18", "cover": "https://img/2.jpg"},
            {"subjectId": "3", "title": "Dune Part Two", "subjectType": 1,
             "releaseDate": "2024-02-27", "cover": {"url": "https://img/3.jpg"}},
            {"subjectId": "", "title": "No id", "subjectType": 1}
        ]}]})
    }

    #[test]
    fn parses_search_and_collapses_duplicates() {
        let hits = parse_search(&search_payload(), "");
        // id-less entry dropped, "Dune Part Two" deduped against its decorated twin
        assert_eq!(hits.len(), 2);
        // series sorts first
        assert_eq!(hits[0].id, "2");
        assert!(hits[0].is_series);
        assert_eq!(hits[1].title, "Dune Part Two"); // bracket junk stripped
        assert_eq!(hits[1].year, "2024");
        assert_eq!(hits[1].cover, "https://img/1.jpg"); // object form
        assert_eq!(hits[0].cover, "https://img/2.jpg"); // bare-string form
    }

    #[test]
    fn search_survives_a_shapeless_payload() {
        assert!(parse_search(&json!({}), "").is_empty());
        assert!(parse_search(&json!({"results": []}), "").is_empty());
        assert!(parse_search(&json!({"results": [{"subjects": null}]}), "").is_empty());
    }

    #[test]
    fn suggest_dedupes_and_caps_at_eight() {
        let mut subjects = Vec::new();
        for n in 0..20 {
            subjects.push(json!({"subjectId": n.to_string(), "title": format!("T{}", n % 3)}));
        }
        let names = parse_suggest(&json!({"results": [{"subjects": subjects}]}));
        assert_eq!(names.len(), 3); // only 3 distinct titles survive the dedupe
    }

    #[test]
    fn parses_details_with_explicit_seasons() {
        let d = parse_details(&json!({
            "id": "9", "title": "Severance [4K]", "subjectType": 2,
            "description": "Work-life balance.", "releaseDate": "2022-02-18",
            "imdbRatingValue": 8.7, "genre": "Drama", "countryName": "USA",
            "contentRating": "TV-MA", "duration": "50 min",
            "cover": {"url": "https://img/s.jpg"},
            "dubs": [{"subjectId": "9", "lanName": "English"},
                     {"subjectId": "10", "lanName": "Hindi"},
                     {"subjectId": "", "lanName": "Broken"}],
            "seasons": {"seasons": [{"se": 1, "maxEp": 9}, {"se": 2, "maxEp": 10}]}
        }));
        assert_eq!(d.title, "Severance");
        assert_eq!(d.year, "2022");
        assert_eq!(d.rating, 8.7);
        assert!(d.is_series);
        assert_eq!(d.dubs.len(), 2); // id-less dub dropped
        assert_eq!(d.dubs[1].name, "Hindi");
        assert_eq!(d.seasons, vec![Season { number: 1, max_ep: 9 }, Season { number: 2, max_ep: 10 }]);
    }

    #[test]
    fn series_without_seasons_falls_back_to_total_episode() {
        let d = parse_details(&json!({
            "id": "9", "title": "Show", "stype": 2,
            "resourceDetectors": [{"totalEpisode": 12}]
        }));
        assert_eq!(d.seasons, vec![Season { number: 1, max_ep: 12 }]);
    }

    #[test]
    fn movie_has_no_seasons_and_reads_intro_as_overview() {
        let d = parse_details(&json!({
            "id": "1", "title": "Film", "subjectType": 1, "intro": "A film."
        }));
        assert!(!d.is_series);
        assert!(d.seasons.is_empty());
        assert_eq!(d.overview, "A film.");
    }

    #[test]
    fn rating_accepts_a_string() {
        let d = parse_details(&json!({"id": "1", "imdbRatingValue": "7.5"}));
        assert_eq!(d.rating, 7.5);
    }

    #[test]
    fn parses_resources_sorted_and_deduped() {
        let files = parse_resources(&json!({"list": [
            {"resourceLink": "https://v/480", "resourceId": "b", "resolution": 480,
             "codecName": "h264", "size": "402653184", "uploadBy": "Aseel"},
            {"resourceLink": "https://v/1080", "resourceId": "a", "resolution": 1080,
             "codecName": "h265", "size": "2254857830", "uploadBy": "Alice",
             "extCaptions": [{"url": "https://s/en.srt", "lanName": "English", "ext": "srt"}]},
            {"resourceLink": "https://v/1080", "resourceId": "a", "resolution": 1080},
            {"resourceId": "c", "resolution": 720}
        ]}));
        assert_eq!(files.len(), 2); // dupe URL + link-less entry dropped
        assert_eq!(files[0].resolution, 1080); // best first
        assert_eq!(files[0].codec, "h265");
        assert_eq!(files[0].size, "2.1 GB");       // bytes formatted, not raw
        assert_eq!(files[0].uploader, "Alice");
        assert_eq!(files[0].captions.len(), 1);    // read inline, no extra request
        assert_eq!(files[0].captions[0].lang, "English");
        assert_eq!(files[1].url, "https://v/480");
        assert_eq!(files[1].size, "384 MB");
        assert!(files[1].captions.is_empty());
    }

    #[test]
    fn resources_filter_to_the_requested_episode() {
        // A season pack: se=1&ep=1 came back carrying episodes 1..3.
        let pack = json!({"list": [
            {"resourceLink": "https://v/e1", "resolution": 720, "ep": 1},
            {"resourceLink": "https://v/e2", "resolution": 720, "ep": 2},
            {"resourceLink": "https://v/e3", "resolution": 720, "episode": "3"},
            {"resourceLink": "https://v/unstamped", "resolution": 720}
        ]});
        let ep1 = parse_resources_for(&pack, 1, 1);
        // episode 1 + the unstamped entry (kept), episodes 2/3 dropped
        let urls: Vec<&str> = ep1.iter().map(|f| f.url.as_str()).collect();
        assert!(urls.contains(&"https://v/e1"));
        assert!(urls.contains(&"https://v/unstamped"));
        assert!(!urls.contains(&"https://v/e2"));
        assert!(!urls.contains(&"https://v/e3"));

        let ep2 = parse_resources_for(&pack, 1, 2);
        let urls: Vec<&str> = ep2.iter().map(|f| f.url.as_str()).collect();
        assert!(urls.contains(&"https://v/e2"));
        assert!(!urls.contains(&"https://v/e1"));

        // No filter (movie) keeps everything.
        assert_eq!(parse_resources_for(&pack, 0, 0).len(), 4);
    }

    #[test]
    fn resources_filter_by_season_too() {
        let pack = json!({"list": [
            {"resourceLink": "https://v/s1e1", "resolution": 720, "se": 1, "ep": 1},
            {"resourceLink": "https://v/s2e1", "resolution": 720, "se": 2, "ep": 1}
        ]});
        let s2 = parse_resources_for(&pack, 2, 1);
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0].url, "https://v/s2e1");
    }

    #[test]
    fn parses_captions_from_either_envelope() {
        let a = parse_captions(&json!({"extCaptions": [
            {"url": "https://s/en.srt", "lanName": "English", "ext": "srt"},
            {"lanName": "Broken"}
        ]}));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].lang, "English");
        let b = parse_captions(&json!({"list": [{"url": "https://s/hi.vtt", "lanName": "Hindi"}]}));
        assert_eq!(b.len(), 1);
        assert!(parse_captions(&json!({})).is_empty());
    }

    #[test]
    fn year_extraction_rejects_junk() {
        assert_eq!(year_of("2024-02-27"), "2024");
        assert_eq!(year_of("2024"), "2024");
        assert_eq!(year_of(""), "");
        assert_eq!(year_of("N/A"), "");
        assert_eq!(year_of("Coming soon"), "");
    }

    // ---- hosts editor ----

    #[test]
    fn validate_normalises_good_hosts() {
        assert_eq!(hosts::validate("  https://a.test/  ").unwrap(), "https://a.test");
        assert_eq!(hosts::validate("http://192.168.1.5:8080").unwrap(), "http://192.168.1.5:8080");
    }

    #[test]
    fn validate_rejects_bad_hosts() {
        assert!(hosts::validate("").is_err());
        assert!(hosts::validate("   ").is_err());
        assert!(hosts::validate("a.test").is_err());          // no scheme
        assert!(hosts::validate("ftp://a.test").is_err());     // wrong scheme
        assert!(hosts::validate("https://a.test/api/v3").is_err()); // path included
    }

    #[test]
    fn save_rejects_an_all_blank_list() {
        assert!(hosts::save(&[]).is_err());
        assert!(hosts::save(&["".into(), "   ".into()]).is_err());
    }

    // ---- recent searches ----

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recent_puts_newest_first_and_dedupes_case_insensitively() {
        let list = v(&["dune", "severance"]);
        assert_eq!(recent::merge(&list, "arrival"), v(&["arrival", "dune", "severance"]));
        // re-searching an existing term moves it to the front, not duplicates it
        assert_eq!(recent::merge(&list, "severance"), v(&["severance", "dune"]));
        assert_eq!(recent::merge(&list, "  SEVERANCE  "), v(&["SEVERANCE", "dune"]));
    }

    #[test]
    fn recent_caps_the_list_and_ignores_blanks() {
        let long: Vec<String> = (0..10).map(|n| format!("t{n}")).collect();
        let merged = recent::merge(&long, "new");
        assert_eq!(merged.len(), 10);
        assert_eq!(merged[0], "new");
        assert_eq!(merged.last().unwrap(), "t8"); // oldest dropped
        assert_eq!(recent::merge(&long, "   "), long);
        assert!(recent::merge(&[], "").is_empty());
    }

    // ---- season merging + relevance (the "Person of Interest" case) ----

    fn poi_payload() -> Value {
        let mk = |id: &str, title: &str, stype: i64, date: &str| {
            json!({"subjectId": id, "title": title, "subjectType": stype, "releaseDate": date})
        };
        json!({"results": [{"subjects": [
            mk("a", "Personal Trainer", 2, "2025-01-01"),
            mk("b", "Conflict of Interest", 2, "2023-01-01"),
            mk("s5", "Person of Interest S5", 2, "2016-01-01"),
            mk("s4", "Person of Interest S4", 2, "2014-01-01"),
            mk("s3", "Person of Interest S3", 2, "2013-01-01"),
            mk("s2", "Person of Interest S2", 2, "2012-01-01"),
            mk("s1", "Person of Interest S1", 2, "2011-01-01"),
            mk("m1", "Person of Interest", 1, "2010-01-01"),
            mk("m2", "Person of Interest", 1, "2007-01-01"),
        ]}]})
    }

    #[test]
    fn per_season_subjects_fold_into_one_card() {
        let hits = parse_search(&poi_payload(), "person of interest");
        // unrelated titles filtered; five seasons become one card; two movies stay
        assert_eq!(hits.len(), 3, "{hits:#?}");

        let show = hits.iter().find(|h| h.is_series).unwrap();
        assert_eq!(show.title, "Person of Interest"); // suffix stripped
        assert_eq!(
            show.season_subjects,
            vec![
                (1, "s1".to_string()),
                (2, "s2".to_string()),
                (3, "s3".to_string()),
                (4, "s4".to_string()),
                (5, "s5".to_string()),
            ]
        );
        assert_eq!(show.id, "s1"); // opens on the earliest season

        let movies: Vec<_> = hits.iter().filter(|h| !h.is_series).collect();
        assert_eq!(movies.len(), 2);
        assert!(movies.iter().all(|m| m.season_subjects.is_empty()));
    }

    #[test]
    fn unrelated_results_are_dropped() {
        let hits = parse_search(&poi_payload(), "person of interest");
        let titles: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert!(!titles.contains(&"Personal Trainer"));
        assert!(!titles.contains(&"Conflict of Interest"));
        // an empty query filters nothing
        assert_eq!(parse_search(&poi_payload(), "").len(), 5);
    }

    #[test]
    fn a_lone_season_one_is_not_treated_as_a_split_show() {
        let p = json!({"results": [{"subjects": [
            {"subjectId": "x", "title": "Loki S1", "subjectType": 2, "releaseDate": "2021-01-01"}
        ]}]});
        let hits = parse_search(&p, "loki");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Loki");
        // no sibling seasons → defer to whatever the subject's own payload says
        assert!(hits[0].season_subjects.is_empty());
    }

    #[test]
    fn season_suffix_split_handles_every_spelling() {
        let split = |t: &str| split_season_suffix(t);
        assert_eq!(split("Person of Interest S5"), ("Person of Interest".into(), Some(5)));
        assert_eq!(split("Dark Season 3"), ("Dark".into(), Some(3)));
        assert_eq!(split("dark season 12"), ("dark".into(), Some(12)));
        assert_eq!(split("Fargo Season Two"), ("Fargo".into(), Some(2)));
        assert_eq!(split("The Crown Part 4"), ("The Crown".into(), Some(4)));
        assert_eq!(split("Rome Part III"), ("Rome".into(), Some(3)));
        assert_eq!(split("Broadchurch Series 2"), ("Broadchurch".into(), Some(2)));
        // trailing punctuation between name and marker is dropped
        assert_eq!(split("Show - Season 2"), ("Show".into(), Some(2)));
        assert_eq!(split("Show: Part IV"), ("Show".into(), Some(4)));
    }

    #[test]
    fn season_suffix_split_leaves_ordinary_titles_alone() {
        let split = |t: &str| split_season_suffix(t);
        assert_eq!(split("Friends"), ("Friends".into(), None));
        assert_eq!(split("The Sopranos"), ("The Sopranos".into(), None));
        assert_eq!(split("Se7en"), ("Se7en".into(), None));
        assert_eq!(split("Money Heist"), ("Money Heist".into(), None));
        // "Part" followed by a word that is not a number is not a season
        assert_eq!(split("Part of the Deal"), ("Part of the Deal".into(), None));
        assert_eq!(split("A Season for Love"), ("A Season for Love".into(), None));
    }

    /// A title whose lowercase form is *longer* in bytes than the original used
    /// to slice `t` with offsets taken from the lowercased copy — out of bounds,
    /// and with `panic = "abort"` in release that took the whole app down.
    #[test]
    fn season_suffix_split_survives_titles_that_grow_when_lowercased() {
        let split = |t: &str| split_season_suffix(t);
        assert_eq!(split("İİ Season 2"), ("İİ".into(), Some(2)));
        assert_eq!(split("Diriliş Ertuğrul Season 5"), ("Diriliş Ertuğrul".into(), Some(5)));
        assert_eq!(split("İstanbul"), ("İstanbul".into(), None));
        // Marker matching stays case-insensitive.
        assert_eq!(split("Dark SEASON 3"), ("Dark".into(), Some(3)));
    }

    #[test]
    fn stream_pick_prefers_sticky_then_720_then_below_then_best() {
        let f = |res: i64| StreamFile { resolution: res, url: format!("u{res}"), ..Default::default() };
        let full = vec![f(1080), f(720), f(480), f(360)];

        // Sticky wins outright.
        assert_eq!(quality::pick(&full, Some(1080)), Some(0));
        assert_eq!(quality::pick(&full, Some(480)), Some(2));
        // Nothing played yet → 720p.
        assert_eq!(quality::pick(&full, None), Some(1));
        // Sticky rung absent from this episode → fall back to 720p.
        assert_eq!(quality::pick(&full, Some(2160)), Some(1));

        // No 720p: take the best rung under it before reaching upward.
        let gap = vec![f(1080), f(480), f(360)];
        assert_eq!(quality::pick(&gap, None), Some(1), "480 beats 1080 as the 720 stand-in");
        // Nothing under 720 either → the lowest rung above it, not the highest.
        let high = vec![f(2160), f(1080)];
        assert_eq!(quality::pick(&high, None), Some(1), "1080 is nearest 720, not 2160");
        // Unlabelled rungs still yield a playable pick.
        assert_eq!(quality::pick(&[f(0), f(0)], None), Some(0));
        assert_eq!(quality::pick(&[], None), None);
    }

    #[test]
    fn roman_numerals_reject_non_canonical_and_lookalike_words() {
        assert_eq!(parse_number("IV"), Some(4));
        assert_eq!(parse_number("xiv"), Some(14));
        assert_eq!(parse_number("iiii"), None); // not canonical
        assert_eq!(parse_number("vx"), None);
        assert_eq!(parse_number("mix"), None);  // has a non-numeral letter
        assert_eq!(parse_number("0"), None);
        assert_eq!(parse_number("999"), None);  // out of plausible range
        assert_eq!(parse_number(""), None);
    }

    #[test]
    fn identical_offers_collapse_to_one_row() {
        // The real shape: one episode returning the same file 5 times under
        // different URLs, plus one genuinely different cut.
        let mut list = Vec::new();
        for n in 0..5 {
            list.push(json!({
                "resourceLink": format!("https://v/{n}"), "resourceId": "a",
                "resolution": 1080, "codecName": "hevc", "size": "1010000000",
                "uploadBy": "Alice"
            }));
        }
        list.push(json!({
            "resourceLink": "https://v/other", "resourceId": "b",
            "resolution": 1080, "codecName": "hevc", "size": "500000000",
            "uploadBy": "Aseel"
        }));
        let files = parse_resources(&json!({"list": list}));
        assert_eq!(files.len(), 2, "{files:#?}");
        assert_eq!(files[0].uploader, "Alice");
        assert_eq!(files[1].uploader, "Aseel");
    }

    #[test]
    fn relevance_filter_fails_open_rather_than_showing_nothing() {
        let p = json!({"results": [{"subjects": [
            {"subjectId": "1", "title": "Ek Deewane Ki Deewaniyat", "subjectType": 1},
            {"subjectId": "2", "title": "Deewaniyat", "subjectType": 2}
        ]}]});
        // A query matching nothing must not blank the screen.
        assert_eq!(parse_search(&p, "zzzz-no-match").len(), 2);
        // …but when something does match, the noise still goes.
        let hits = parse_search(&p, "deewaniyat");
        assert_eq!(hits.len(), 2); // both contain it
        let hits = parse_search(&p, "ek deewane");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn human_size_formats_byte_counts() {
        assert_eq!(human_size("463696088"), "442 MB");
        assert_eq!(human_size("2254857830"), "2.1 GB");
        assert_eq!(human_size("0"), "");
        assert_eq!(human_size(""), "");
        assert_eq!(human_size("not a number"), "");
    }

    #[test]
    fn prefer_dubs_keeps_the_short_list_and_fails_open() {
        let d = |n: &str| Dub { subject_id: n.into(), name: n.into() };
        let full = vec![d("Original Audio"), d("English"), d("Hindi"), d("esla dub"), d("ptbr dub")];
        let kept = prefer::dubs(full);
        assert_eq!(kept.len(), 3);
        assert!(kept.iter().all(|x| ["Original Audio", "English", "Hindi"].contains(&x.name.as_str())));
        // a Korean-only show keeps everything rather than showing nothing
        let korean = vec![d("Original"), d("Korean")];
        assert_eq!(prefer::dubs(korean).len(), 2);
        // truly nothing preferred → fail open
        assert_eq!(prefer::dubs(vec![d("French"), d("German")]).len(), 2);
    }

    #[test]
    fn prefer_captions_keeps_english_and_hindi() {
        let c = |l: &str| Caption { url: l.into(), lang: l.into(), ext: "srt".into() };
        let full = vec![c("English"), c("Hindi"), c("Spanish"), c("Arabic")];
        let kept = prefer::captions(full);
        assert_eq!(kept.len(), 2);
        assert_eq!(prefer::captions(vec![c("Spanish")]).len(), 1); // fail open
    }

    #[test]
    fn defaults_are_non_empty_and_valid() {
        let d = hosts::defaults();
        assert!(!d.is_empty());
        for h in &d {
            assert!(hosts::validate(h).is_ok(), "{h}");
        }
    }
}
