//! TMDB (movies) + TVDB (shows) scraper.
//!
//! The HTTP client is wrapped behind a `MetadataProvider` trait so tests can
//! inject a fake provider, the rest of the app can ignore which service
//! answered, and a follow-up agent chain (TMDB → OMDb → filename parser) can
//! slot in without touching call sites.
//!
//! Poster + backdrop URLs are cached to disk under
//! `<cache>/videos/posters/<hash>.jpg` with the bundled HTTP client.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieMeta {
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub runtime_min: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShowMeta {
    pub tmdb_id: Option<i64>,
    pub tvdb_id: Option<i64>,
    pub title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeMeta {
    pub season: i64,
    pub episode: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub air_date_unix: Option<i64>,
    pub still_path: Option<String>,
    pub runtime_min: Option<i64>,
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn search_movie(&self, title: &str, year: Option<i64>) -> Result<Option<MovieMeta>>;
    async fn search_show(&self, title: &str) -> Result<Option<ShowMeta>>;
    async fn episode(&self, show_tmdb_id: i64, season: i64, episode: i64) -> Result<Option<EpisodeMeta>>;
}

pub struct TmdbClient {
    pub api_key: String,
    pub base: String,
    pub image_base: String,
    http: reqwest::Client,
}

impl TmdbClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: "https://api.themoviedb.org/3".into(),
            image_base: "https://image.tmdb.org/t/p/w500".into(),
            http: reqwest::Client::new(),
        }
    }

    /// Resolve a TMDB `poster_path` (e.g. `/abc.jpg`) to a local cached file.
    /// Returns the cached file path; downloads on first call.
    pub async fn cache_poster(&self, poster_path: &str, cache_dir: &Path) -> Result<PathBuf> {
        let url = format!("{}{}", self.image_base, poster_path);
        cache_image(&self.http, &url, cache_dir).await
    }
}

#[async_trait]
impl MetadataProvider for TmdbClient {
    async fn search_movie(&self, title: &str, year: Option<i64>) -> Result<Option<MovieMeta>> {
        let mut req = self.http.get(format!("{}/search/movie", self.base))
            .query(&[("api_key", self.api_key.as_str()), ("query", title)]);
        if let Some(y) = year { req = req.query(&[("year", y.to_string())]); }
        let v: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        let Some(first) = v.get("results").and_then(|x| x.as_array()).and_then(|a| a.first()) else {
            return Ok(None);
        };
        Ok(Some(MovieMeta {
            tmdb_id: first.get("id").and_then(|x| x.as_i64()),
            title: first.get("title").and_then(|x| x.as_str()).unwrap_or(title).to_string(),
            year: first.get("release_date").and_then(|x| x.as_str())
                .and_then(|s| s.get(..4)).and_then(|s| s.parse().ok()),
            overview: first.get("overview").and_then(|x| x.as_str()).map(str::to_string),
            poster_path: first.get("poster_path").and_then(|x| x.as_str()).map(str::to_string),
            backdrop_path: first.get("backdrop_path").and_then(|x| x.as_str()).map(str::to_string),
            runtime_min: None, // search payload omits runtime; details endpoint fills it.
        }))
    }

    async fn search_show(&self, title: &str) -> Result<Option<ShowMeta>> {
        let req = self.http.get(format!("{}/search/tv", self.base))
            .query(&[("api_key", self.api_key.as_str()), ("query", title)]);
        let v: serde_json::Value = req.send().await?.error_for_status()?.json().await?;
        let Some(first) = v.get("results").and_then(|x| x.as_array()).and_then(|a| a.first()) else {
            return Ok(None);
        };
        Ok(Some(ShowMeta {
            tmdb_id: first.get("id").and_then(|x| x.as_i64()),
            tvdb_id: None,
            title: first.get("name").and_then(|x| x.as_str()).unwrap_or(title).to_string(),
            year: first.get("first_air_date").and_then(|x| x.as_str())
                .and_then(|s| s.get(..4)).and_then(|s| s.parse().ok()),
            overview: first.get("overview").and_then(|x| x.as_str()).map(str::to_string),
            poster_path: first.get("poster_path").and_then(|x| x.as_str()).map(str::to_string),
            backdrop_path: first.get("backdrop_path").and_then(|x| x.as_str()).map(str::to_string),
        }))
    }

    async fn episode(&self, show_tmdb_id: i64, season: i64, episode: i64) -> Result<Option<EpisodeMeta>> {
        let url = format!("{}/tv/{show_tmdb_id}/season/{season}/episode/{episode}", self.base);
        let resp = self.http.get(url)
            .query(&[("api_key", self.api_key.as_str())]).send().await?;
        if !resp.status().is_success() { return Ok(None); }
        let v: serde_json::Value = resp.json().await?;
        Ok(Some(EpisodeMeta {
            season, episode,
            title: v.get("name").and_then(|x| x.as_str()).map(str::to_string),
            overview: v.get("overview").and_then(|x| x.as_str()).map(str::to_string),
            air_date_unix: v.get("air_date").and_then(|x| x.as_str())
                .and_then(parse_yyyy_mm_dd),
            still_path: v.get("still_path").and_then(|x| x.as_str()).map(str::to_string),
            runtime_min: v.get("runtime").and_then(|x| x.as_i64()),
        }))
    }
}

fn parse_yyyy_mm_dd(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 10 { return None; }
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let m: i64 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: i64 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    Some(days_from_civil(y, m, d) * 86_400)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn upsert_movie(pool: &SqlitePool, item_id: i64, m: &MovieMeta) -> Result<()> {
    sqlx::query(
        "INSERT INTO movies (item_id, tmdb_id, title, year, overview, poster_path, backdrop_path, runtime_min, updated)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            tmdb_id = excluded.tmdb_id, title = excluded.title, year = excluded.year,
            overview = excluded.overview, poster_path = excluded.poster_path,
            backdrop_path = excluded.backdrop_path, runtime_min = excluded.runtime_min,
            updated = excluded.updated",
    )
    .bind(item_id).bind(m.tmdb_id).bind(&m.title).bind(m.year)
    .bind(&m.overview).bind(&m.poster_path).bind(&m.backdrop_path)
    .bind(m.runtime_min).bind(now())
    .execute(pool).await?;
    Ok(())
}

pub async fn upsert_show(pool: &SqlitePool, s: &ShowMeta) -> Result<i64> {
    sqlx::query(
        "INSERT INTO shows (tmdb_id, tvdb_id, title, year, overview, poster_path, backdrop_path, updated)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(tmdb_id) DO UPDATE SET
            title = excluded.title, year = excluded.year, overview = excluded.overview,
            poster_path = excluded.poster_path, backdrop_path = excluded.backdrop_path,
            updated = excluded.updated",
    )
    .bind(s.tmdb_id).bind(s.tvdb_id).bind(&s.title).bind(s.year)
    .bind(&s.overview).bind(&s.poster_path).bind(&s.backdrop_path).bind(now())
    .execute(pool).await?;
    let id: i64 = if let Some(tmdb) = s.tmdb_id {
        sqlx::query_scalar("SELECT id FROM shows WHERE tmdb_id = ?").bind(tmdb).fetch_one(pool).await?
    } else {
        sqlx::query_scalar("SELECT id FROM shows WHERE title = ? ORDER BY updated DESC LIMIT 1")
            .bind(&s.title).fetch_one(pool).await?
    };
    Ok(id)
}

/// Download a poster/backdrop into the local cache, return the cached path.
pub async fn cache_image(http: &reqwest::Client, url: &str, cache_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(cache_dir).ok();
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let hash = h.finalize();
    let name = format!("{:x}.jpg", &hash);
    let path = cache_dir.join(name);
    if path.exists() { return Ok(path); }
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn yyyy_mm_dd_round_trip() {
        // 2024-02-29 → unix 1709164800 (midnight UTC)
        assert_eq!(parse_yyyy_mm_dd("2024-02-29"), Some(1_709_164_800));
        assert_eq!(parse_yyyy_mm_dd("1970-01-01"), Some(0));
    }

    #[tokio::test]
    async fn upsert_movie_then_show() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/m.mkv', 0, 1, 0, 'videos', 0, 0)")
            .execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/m.mkv'").fetch_one(&pool).await.unwrap();
        let m = MovieMeta {
            tmdb_id: Some(550), title: "Fight Club".into(), year: Some(1999),
            overview: Some("...".into()), poster_path: Some("/poster.jpg".into()),
            backdrop_path: None, runtime_min: Some(139),
        };
        upsert_movie(&pool, id, &m).await.unwrap();
        let title: String = sqlx::query_scalar("SELECT title FROM movies WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(title, "Fight Club");

        let s = ShowMeta { tmdb_id: Some(1396), title: "Breaking Bad".into(), year: Some(2008), ..ShowMeta::default() };
        let show_id = upsert_show(&pool, &s).await.unwrap();
        let title: String = sqlx::query_scalar("SELECT title FROM shows WHERE id = ?").bind(show_id).fetch_one(&pool).await.unwrap();
        assert_eq!(title, "Breaking Bad");
    }

    #[tokio::test]
    async fn upsert_show_idempotent_on_tmdb_id() {
        let (_t, pool) = open_pool().await;
        let s1 = ShowMeta { tmdb_id: Some(1), title: "A".into(), ..ShowMeta::default() };
        let s2 = ShowMeta { tmdb_id: Some(1), title: "A revised".into(), ..ShowMeta::default() };
        let id1 = upsert_show(&pool, &s1).await.unwrap();
        let id2 = upsert_show(&pool, &s2).await.unwrap();
        assert_eq!(id1, id2);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shows").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1);
    }
}
