//! Anime scraper — AniDB + AniList. The pair covers what TMDB gets wrong
//! about anime: absolute episode numbering vs split-cour seasons, alternate
//! romaji titles, OVAs/movies bundled into a single series, etc.
//!
//! Both providers go through the same `AnimeProvider` trait so the user can
//! pick per-source in Settings → Sources without code changes.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tulipix_core::util::unix_secs_i64 as now;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimeMeta {
    pub anidb_id: Option<i64>,
    pub anilist_id: Option<i64>,
    pub title_romaji: String,
    pub title_native: Option<String>,
    pub title_english: Option<String>,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub episode_count: Option<i64>,
    pub format: Option<String>, // "TV", "Movie", "OVA", "Special"
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimeEpisode {
    pub absolute: i64,
    pub season: Option<i64>,
    pub episode: i64,
    pub title: Option<String>,
    pub air_date: Option<String>,
    pub overview: Option<String>,
}

#[async_trait]
pub trait AnimeProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, title: &str) -> Result<Option<AnimeMeta>>;
    async fn episodes(&self, series_id: i64) -> Result<Vec<AnimeEpisode>>;
}

pub struct AniListProvider {
    pub base: String,
    http: reqwest::Client,
}

impl AniListProvider {
    pub fn new() -> Self {
        Self {
            base: "https://graphql.anilist.co".into(),
            http: tulipix_core::net::http().clone(),
        }
    }
}

impl Default for AniListProvider {
    fn default() -> Self {
        Self::new()
    }
}

const ANILIST_QUERY: &str = r#"
query ($search: String) {
  Media(search: $search, type: ANIME) {
    id
    title { romaji english native }
    description(asHtml: false)
    coverImage { large }
    episodes
    format
    startDate { year }
  }
}"#;

#[async_trait]
impl AnimeProvider for AniListProvider {
    fn name(&self) -> &'static str {
        "anilist"
    }

    async fn search(&self, title: &str) -> Result<Option<AnimeMeta>> {
        let body = serde_json::json!({
            "query": ANILIST_QUERY,
            "variables": { "search": title },
        });
        let resp = self.http.post(&self.base).json(&body).send().await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(parse_anilist(&v))
    }

    async fn episodes(&self, _series_id: i64) -> Result<Vec<AnimeEpisode>> {
        // AniList exposes only the count, not per-episode metadata. Caller
        // falls through to AniDB or the filename parser for per-episode data.
        Ok(Vec::new())
    }
}

fn parse_anilist(v: &serde_json::Value) -> Option<AnimeMeta> {
    let m = v.get("data")?.get("Media")?;
    if m.is_null() {
        return None;
    }
    Some(AnimeMeta {
        anidb_id: None,
        anilist_id: m.get("id").and_then(|x| x.as_i64()),
        title_romaji: m
            .get("title")
            .and_then(|t| t.get("romaji"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        title_english: m
            .get("title")
            .and_then(|t| t.get("english"))
            .and_then(|x| x.as_str())
            .map(str::to_string),
        title_native: m
            .get("title")
            .and_then(|t| t.get("native"))
            .and_then(|x| x.as_str())
            .map(str::to_string),
        year: m.get("startDate").and_then(|d| d.get("year")).and_then(|x| x.as_i64()),
        overview: m.get("description").and_then(|x| x.as_str()).map(str::to_string),
        poster_url: m
            .get("coverImage")
            .and_then(|c| c.get("large"))
            .and_then(|x| x.as_str())
            .map(str::to_string),
        episode_count: m.get("episodes").and_then(|x| x.as_i64()),
        format: m.get("format").and_then(|x| x.as_str()).map(str::to_string),
    })
}

/// AniDB. Two endpoints, both gzipped:
///
///   * the daily titles dump, which is the only way to turn a title into an
///     `aid` -- AniDB has no search endpoint for registered clients;
///   * `httpapi?request=anime&aid=…`, which returns the series and every
///     episode as XML.
///
/// AniDB requires a client registered on their site: the name and version go
/// in every request, and an unregistered one is answered with an error
/// document rather than a 4xx. The name is what Settings stores; the version
/// is ours.
///
/// Their terms are firm about the dump: fetch it at most once a day. It is
/// cached on disk and re-read until it is a day old.
pub struct AniDbProvider {
    pub client_name: String,
    pub client_version: i64,
    /// Where the titles dump is kept between runs.
    pub cache_dir: std::path::PathBuf,
    http: reqwest::Client,
}

pub const ANIDB_TITLES_URL: &str = "https://anidb.net/api/anime-titles.dat.gz";
/// AniDB asks for one dump a day at most, and answers a banned client with a
/// 403 for a day. One a day, from disk in between.
pub const ANIDB_TITLES_MAX_AGE: i64 = 86_400;

impl AniDbProvider {
    pub fn new(client_name: impl Into<String>, cache_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            client_name: client_name.into(),
            client_version: 1,
            cache_dir: cache_dir.into(),
            http: tulipix_core::net::http().clone(),
        }
    }

    pub fn anime_url(&self, anidb_id: i64) -> String {
        format!(
            "http://api.anidb.net:9001/httpapi?request=anime&client={}&clientver={}&protover=1&aid={}",
            self.client_name, self.client_version, anidb_id
        )
    }

    fn titles_path(&self) -> std::path::PathBuf {
        self.cache_dir.join("anidb-titles.dat")
    }

    /// The titles dump as text, from disk when it is less than a day old and
    /// from AniDB otherwise. A fetch that fails falls back to whatever copy is
    /// on disk, however old: a stale index beats no index.
    pub async fn titles(&self) -> Result<String> {
        let path = self.titles_path();
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() < ANIDB_TITLES_MAX_AGE as u64);
        if fresh {
            if let Ok(s) = std::fs::read_to_string(&path) {
                return Ok(s);
            }
        }
        match self.fetch_titles().await {
            Ok(text) => {
                std::fs::create_dir_all(&self.cache_dir).ok();
                std::fs::write(&path, text.as_bytes()).ok();
                Ok(text)
            }
            Err(e) => match std::fs::read_to_string(&path) {
                Ok(s) => {
                    tracing::warn!(error = %e, "anidb: titles dump not refreshed, using the copy on disk");
                    Ok(s)
                }
                Err(_) => Err(e),
            },
        }
    }

    async fn fetch_titles(&self) -> Result<String> {
        let bytes = self
            .http
            .get(ANIDB_TITLES_URL)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(String::from_utf8_lossy(&gunzip(&bytes)?).into_owned())
    }

    /// The `aid` whose title matches best. Exact match first, then a title
    /// that starts with the query -- AniDB titles carry season suffixes
    /// ("Steins;Gate 0") that a contains-match would rank above the original.
    pub async fn find_aid(&self, title: &str) -> Result<Option<i64>> {
        Ok(best_aid(&self.titles().await?, title))
    }

    /// The series and its episodes, straight from httpapi.
    pub async fn anime(&self, aid: i64) -> Result<Option<(AnimeMeta, Vec<AnimeEpisode>)>> {
        let bytes = self
            .http
            .get(self.anime_url(aid))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let xml = String::from_utf8_lossy(&gunzip(&bytes)?).into_owned();
        parse_anidb(&xml, aid)
    }
}

/// gzip, or the bytes as they arrived. reqwest is built here without the
/// `gzip` feature, so nothing decompresses this on the way in; AniDB sends
/// both endpoints gzipped, and the magic number says so.
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        return Ok(bytes.to_vec());
    }
    use std::io::Read as _;
    let mut out = Vec::with_capacity(bytes.len() * 4);
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out)?;
    Ok(out)
}

/// One line of the dump is `<aid>|<type>|<language>|<title>`. Lines opening
/// with `#` are its header.
fn best_aid(dump: &str, want: &str) -> Option<i64> {
    let want_n = normalise_title(want);
    if want_n.is_empty() {
        return None;
    }
    let mut prefix: Option<(usize, i64)> = None;
    for line in dump.lines() {
        if line.starts_with('#') {
            continue;
        }
        let mut cols = line.split('|');
        let (Some(aid), Some(_kind), Some(_lang), Some(title)) =
            (cols.next(), cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let Ok(aid) = aid.parse::<i64>() else { continue };
        let t = normalise_title(title);
        if t == want_n {
            return Some(aid);
        }
        // Shortest title that still opens with the query: "Steins;Gate" beats
        // "Steins;Gate 0" for a file called "Steins Gate 01".
        if t.starts_with(&want_n) && prefix.is_none_or(|(len, _)| t.len() < len) {
            prefix = Some((t.len(), aid));
        }
    }
    prefix.map(|(_, aid)| aid)
}

/// Case, punctuation and runs of spaces out, so "Steins;Gate" and
/// "steins gate" are the same title.
fn normalise_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

/// httpapi's `<anime>` document. AniDB answers a bad client with
/// `<error>…</error>` and HTTP 200, so that is checked before anything else.
fn parse_anidb(xml: &str, aid: i64) -> Result<Option<(AnimeMeta, Vec<AnimeEpisode>)>> {
    let doc = roxmltree::Document::parse(xml)?;
    let root = doc.root_element();
    if root.has_tag_name("error") {
        anyhow::bail!("anidb: {}", root.text().unwrap_or("refused the request").trim());
    }
    if !root.has_tag_name("anime") {
        return Ok(None);
    }
    let child = |name: &str| root.children().find(|n| n.has_tag_name(name));
    let text = |name: &str| child(name).and_then(|n| n.text()).map(str::to_string);

    let titles: Vec<(String, String, String)> = child("titles")
        .into_iter()
        .flat_map(|t| t.children())
        .filter(|n| n.has_tag_name("title"))
        .map(|n| {
            (
                n.attribute("type").unwrap_or("").to_string(),
                n.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                    .or_else(|| n.attribute("lang"))
                    .unwrap_or("")
                    .to_string(),
                n.text().unwrap_or("").to_string(),
            )
        })
        .collect();
    let pick = |kind: &str, lang: &str| -> Option<String> {
        titles
            .iter()
            .find(|(k, l, _)| k == kind && l == lang)
            .map(|(_, _, t)| t.clone())
    };

    let meta = AnimeMeta {
        anidb_id: Some(aid),
        anilist_id: None,
        title_romaji: pick("main", "x-jat")
            .or_else(|| pick("official", "x-jat"))
            .or_else(|| titles.first().map(|(_, _, t)| t.clone()))
            .unwrap_or_default(),
        title_english: pick("official", "en").or_else(|| pick("synonym", "en")),
        title_native: pick("official", "ja"),
        year: text("startdate")
            .as_deref()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse().ok()),
        overview: text("description"),
        poster_url: text("picture").map(|p| format!("https://cdn.anidb.net/images/main/{p}")),
        episode_count: text("episodecount").and_then(|n| n.trim().parse().ok()),
        format: text("type"),
    };

    let mut episodes: Vec<AnimeEpisode> = child("episodes")
        .into_iter()
        .flat_map(|e| e.children())
        .filter(|n| n.has_tag_name("episode"))
        .filter_map(|n| {
            let epno = n.children().find(|c| c.has_tag_name("epno"))?;
            // type 1 is a numbered episode; 2 is a special, 3 an opening, and
            // so on. Only the first is an episode of the series.
            if epno.attribute("type") != Some("1") {
                return None;
            }
            let number: i64 = epno.text()?.trim().parse().ok()?;
            Some(AnimeEpisode {
                absolute: number,
                season: Some(1),
                episode: number,
                title: n
                    .children()
                    .filter(|c| c.has_tag_name("title"))
                    .find(|c| {
                        c.attribute(("http://www.w3.org/XML/1998/namespace", "lang")) == Some("en")
                            || c.attribute("lang") == Some("en")
                    })
                    .or_else(|| n.children().find(|c| c.has_tag_name("title")))
                    .and_then(|c| c.text())
                    .map(str::to_string),
                air_date: n
                    .children()
                    .find(|c| c.has_tag_name("airdate"))
                    .and_then(|c| c.text())
                    .map(str::to_string),
                overview: n
                    .children()
                    .find(|c| c.has_tag_name("summary"))
                    .and_then(|c| c.text())
                    .map(str::to_string),
            })
        })
        .collect();
    episodes.sort_by_key(|e| e.absolute);
    Ok(Some((meta, episodes)))
}

#[async_trait]
impl AnimeProvider for AniDbProvider {
    fn name(&self) -> &'static str {
        "anidb"
    }

    async fn search(&self, title: &str) -> Result<Option<AnimeMeta>> {
        let Some(aid) = self.find_aid(title).await? else { return Ok(None) };
        Ok(self.anime(aid).await?.map(|(m, _)| m))
    }

    async fn episodes(&self, series_id: i64) -> Result<Vec<AnimeEpisode>> {
        Ok(self.anime(series_id).await?.map(|(_, e)| e).unwrap_or_default())
    }
}

pub const ANIME_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS anime_meta (
    item_id        INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    anidb_id       INTEGER,
    anilist_id     INTEGER,
    title_romaji   TEXT    NOT NULL,
    title_english  TEXT,
    title_native   TEXT,
    year           INTEGER,
    overview       TEXT,
    poster_url     TEXT,
    episode_count  INTEGER,
    format         TEXT,
    source         TEXT    NOT NULL,
    updated        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS anime_meta_anidb_idx   ON anime_meta(anidb_id);
CREATE INDEX IF NOT EXISTS anime_meta_anilist_idx ON anime_meta(anilist_id);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(ANIME_SCHEMA).execute(pool).await?;
    Ok(())
}

pub async fn upsert(pool: &SqlitePool, item_id: i64, source: &str, m: &AnimeMeta) -> Result<()> {
    sqlx::query(
        "INSERT INTO anime_meta
         (item_id, anidb_id, anilist_id, title_romaji, title_english, title_native,
          year, overview, poster_url, episode_count, format, source, updated)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            anidb_id = excluded.anidb_id, anilist_id = excluded.anilist_id,
            title_romaji = excluded.title_romaji, title_english = excluded.title_english,
            title_native = excluded.title_native, year = excluded.year,
            overview = excluded.overview, poster_url = excluded.poster_url,
            episode_count = excluded.episode_count, format = excluded.format,
            source = excluded.source, updated = excluded.updated",
    )
    .bind(item_id)
    .bind(m.anidb_id)
    .bind(m.anilist_id)
    .bind(&m.title_romaji)
    .bind(&m.title_english)
    .bind(&m.title_native)
    .bind(m.year)
    .bind(&m.overview)
    .bind(&m.poster_url)
    .bind(m.episode_count)
    .bind(&m.format)
    .bind(source)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn parse_anilist_payload() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"data":{"Media":{
                "id":21,"title":{"romaji":"One Piece","english":"One Piece","native":"ワンピース"},
                "description":"pirates","coverImage":{"large":"https://…/p.png"},
                "episodes":1100,"format":"TV","startDate":{"year":1999}
            }}}"#,
        )
        .unwrap();
        let m = parse_anilist(&v).unwrap();
        assert_eq!(m.title_romaji, "One Piece");
        assert_eq!(m.anilist_id, Some(21));
        assert_eq!(m.episode_count, Some(1100));
        assert_eq!(m.format.as_deref(), Some("TV"));
        assert_eq!(m.year, Some(1999));
    }

    #[test]
    fn parse_anilist_missing_returns_none() {
        let v: serde_json::Value = serde_json::from_str(r#"{"data":{"Media":null}}"#).unwrap();
        assert!(parse_anilist(&v).is_none());
    }

    #[test]
    fn anidb_url_includes_client_creds() {
        let p = AniDbProvider::new("tulipix", std::env::temp_dir());
        let url = p.anime_url(42);
        assert!(url.contains("client=tulipix"));
        assert!(url.contains("clientver=1"));
        assert!(url.contains("aid=42"));
    }

    const DUMP: &str = "\
# comment line
1|1|x-jat|Seikai no Monshou
21|1|x-jat|One Piece
1210|1|x-jat|Steins;Gate 0
1211|1|x-jat|Steins;Gate
";

    #[test]
    fn aid_lookup_prefers_the_exact_title_then_the_shortest_prefix() {
        assert_eq!(best_aid(DUMP, "one piece"), Some(21));
        assert_eq!(best_aid(DUMP, "One   Piece!"), Some(21));
        // "Steins Gate 01" is a filename, not a title: the shortest title it
        // opens is the series, not the sequel.
        assert_eq!(best_aid(DUMP, "Steins Gate"), Some(1211));
        assert_eq!(best_aid(DUMP, "nothing here"), None);
        assert_eq!(best_aid(DUMP, "   "), None);
    }

    #[test]
    fn gunzip_passes_plain_bytes_through() {
        assert_eq!(gunzip(b"<anime/>").unwrap(), b"<anime/>");
    }

    #[test]
    fn anidb_error_document_is_an_error_not_an_empty_result() {
        let e = parse_anidb("<error>Client Version Missing</error>", 1).unwrap_err();
        assert!(e.to_string().contains("Client Version Missing"), "{e}");
    }

    #[test]
    fn anidb_anime_document_yields_meta_and_numbered_episodes_only() {
        let xml = r#"<anime id="21">
            <type>TV Series</type>
            <episodecount>1100</episodecount>
            <startdate>1999-10-20</startdate>
            <picture>21.jpg</picture>
            <description>pirates</description>
            <titles>
              <title xml:lang="x-jat" type="main">One Piece</title>
              <title xml:lang="en" type="official">One Piece</title>
              <title xml:lang="ja" type="official">ONE PIECE</title>
            </titles>
            <episodes>
              <episode><epno type="1">2</epno><title xml:lang="en">Second</title><airdate>1999-10-27</airdate></episode>
              <episode><epno type="1">1</epno><title xml:lang="en">First</title><airdate>1999-10-20</airdate></episode>
              <episode><epno type="2">1</epno><title xml:lang="en">A special</title></episode>
            </episodes>
          </anime>"#;
        let (m, eps) = parse_anidb(xml, 21).unwrap().unwrap();
        assert_eq!(m.anidb_id, Some(21));
        assert_eq!(m.title_romaji, "One Piece");
        assert_eq!(m.title_native.as_deref(), Some("ONE PIECE"));
        assert_eq!(m.year, Some(1999));
        assert_eq!(m.episode_count, Some(1100));
        assert!(m.poster_url.as_deref().unwrap().ends_with("/21.jpg"));
        // The special is left out, and what is left is in order.
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].episode, 1);
        assert_eq!(eps[0].title.as_deref(), Some("First"));
        assert_eq!(eps[1].episode, 2);
    }

    #[tokio::test]
    async fn upsert_round_trip() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/o.mkv', 0, 1, 0, 'videos', 0, 0)")
            .execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/o.mkv'")
            .fetch_one(&pool).await.unwrap();
        let m = AnimeMeta {
            anilist_id: Some(21),
            title_romaji: "One Piece".into(),
            ..AnimeMeta::default()
        };
        upsert(&pool, id, "anilist", &m).await.unwrap();
        let title: String =
            sqlx::query_scalar("SELECT title_romaji FROM anime_meta WHERE item_id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(title, "One Piece");
    }
}
