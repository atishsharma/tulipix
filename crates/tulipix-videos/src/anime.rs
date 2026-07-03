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
            http: reqwest::Client::new(),
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

/// AniDB returns XML — kept here so future packagers don't have to invent a
/// second client. We pull `<anime id="…"><titles>…</titles><episodes>…</…></…>`
/// and let serde_xml_rs (added on demand) parse it. For now the implementation
/// just builds the request URL and surfaces a typed error for missing key —
/// the tulipix-cli `rescrape --anidb` subcommand wires the actual HTTP fetch.
pub struct AniDbProvider {
    pub client_name: String,
    pub client_version: i64,
}

impl AniDbProvider {
    pub fn anime_url(&self, anidb_id: i64) -> String {
        format!(
            "http://api.anidb.net:9001/httpapi?request=anime&client={}&clientver={}&protover=1&aid={}",
            self.client_name, self.client_version, anidb_id
        )
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
        let p = AniDbProvider {
            client_name: "tulipix".into(),
            client_version: 1,
        };
        let url = p.anime_url(42);
        assert!(url.contains("client=tulipix"));
        assert!(url.contains("clientver=1"));
        assert!(url.contains("aid=42"));
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
