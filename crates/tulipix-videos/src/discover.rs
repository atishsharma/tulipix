//! Discover feed — TMDB trending + upcoming for the Videos landing rail.
//!
//! Mirrors the `tmdb` module pattern: a `DiscoverProvider` trait so tests can
//! swap in a `FakeDiscoverProvider`, the rest of the app stays agnostic of
//! which service answered, and a follow-up agent chain can wedge in without
//! touching call sites.
//!
//! Fetched rows are mirrored into a small `discover_feed` cache table so the
//! UI can render the rail offline / before the next refresh tick completes.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoverKind {
    TrendingMoviesDay,
    TrendingMoviesWeek,
    UpcomingMovies,
    TrendingShowsDay,
    TrendingShowsWeek,
    OnTheAirShows,
}

impl DiscoverKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscoverKind::TrendingMoviesDay => "trending_movies_day",
            DiscoverKind::TrendingMoviesWeek => "trending_movies_week",
            DiscoverKind::UpcomingMovies => "upcoming_movies",
            DiscoverKind::TrendingShowsDay => "trending_shows_day",
            DiscoverKind::TrendingShowsWeek => "trending_shows_week",
            DiscoverKind::OnTheAirShows => "on_the_air_shows",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            DiscoverKind::TrendingMoviesDay => "/trending/movie/day",
            DiscoverKind::TrendingMoviesWeek => "/trending/movie/week",
            DiscoverKind::UpcomingMovies => "/movie/upcoming",
            DiscoverKind::TrendingShowsDay => "/trending/tv/day",
            DiscoverKind::TrendingShowsWeek => "/trending/tv/week",
            DiscoverKind::OnTheAirShows => "/tv/on_the_air",
        }
    }

    pub fn is_movie(self) -> bool {
        matches!(
            self,
            DiscoverKind::TrendingMoviesDay
                | DiscoverKind::TrendingMoviesWeek
                | DiscoverKind::UpcomingMovies
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DiscoverItem {
    pub tmdb_id: i64,
    pub is_movie: bool,
    pub title: String,
    pub year: Option<i64>,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    /// TMDB release_date / first_air_date as YYYY-MM-DD; lets the UI show
    /// "Releases Jun 14" on the upcoming rail without re-fetching.
    pub release_date: Option<String>,
    pub vote_average: Option<f64>,
}

#[async_trait]
pub trait DiscoverProvider: Send + Sync {
    async fn feed(&self, kind: DiscoverKind, page: u32) -> Result<Vec<DiscoverItem>>;
}

pub struct TmdbDiscover {
    pub api_key: String,
    pub base: String,
    http: reqwest::Client,
}

impl TmdbDiscover {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: "https://api.themoviedb.org/3".into(),
            http: reqwest::Client::new(),
        }
    }

    #[doc(hidden)]
    pub fn with_base(api_key: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: base.into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DiscoverProvider for TmdbDiscover {
    async fn feed(&self, kind: DiscoverKind, page: u32) -> Result<Vec<DiscoverItem>> {
        let url = format!("{}{}", self.base, kind.endpoint());
        let v: serde_json::Value = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str()), ("page", &page.to_string())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let is_movie = kind.is_movie();
        Ok(parse_results(&v, is_movie))
    }
}

fn parse_results(v: &serde_json::Value, is_movie: bool) -> Vec<DiscoverItem> {
    let Some(arr) = v.get("results").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|row| {
            let tmdb_id = row.get("id").and_then(|x| x.as_i64())?;
            let title = row
                .get(if is_movie { "title" } else { "name" })
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let release_date = row
                .get(if is_movie { "release_date" } else { "first_air_date" })
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let year = release_date
                .as_deref()
                .and_then(|s| s.get(..4))
                .and_then(|s| s.parse().ok());
            Some(DiscoverItem {
                tmdb_id,
                is_movie,
                title,
                year,
                overview: row.get("overview").and_then(|x| x.as_str()).map(str::to_string),
                poster_path: row.get("poster_path").and_then(|x| x.as_str()).map(str::to_string),
                backdrop_path: row.get("backdrop_path").and_then(|x| x.as_str()).map(str::to_string),
                release_date,
                vote_average: row.get("vote_average").and_then(|x| x.as_f64()),
            })
        })
        .collect()
}

pub const DISCOVER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS discover_feed (
    kind          TEXT    NOT NULL,
    position      INTEGER NOT NULL,
    tmdb_id       INTEGER NOT NULL,
    is_movie      INTEGER NOT NULL,
    title         TEXT    NOT NULL,
    year          INTEGER,
    overview      TEXT,
    poster_path   TEXT,
    backdrop_path TEXT,
    release_date  TEXT,
    vote_average  REAL,
    fetched_at    INTEGER NOT NULL,
    PRIMARY KEY (kind, position)
);
CREATE INDEX IF NOT EXISTS discover_feed_kind_idx ON discover_feed(kind, position);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(DISCOVER_SCHEMA).execute(pool).await?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Replace the cached rows for a single feed kind. Discover lists are
/// position-ordered and small (TMDB returns 20 per page), so a delete+insert
/// inside a transaction beats trying to diff individual rows.
pub async fn cache_feed(pool: &SqlitePool, kind: DiscoverKind, items: &[DiscoverItem]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM discover_feed WHERE kind = ?")
        .bind(kind.as_str())
        .execute(&mut *tx)
        .await?;
    let ts = now();
    for (i, it) in items.iter().enumerate() {
        sqlx::query(
            "INSERT INTO discover_feed
             (kind, position, tmdb_id, is_movie, title, year, overview,
              poster_path, backdrop_path, release_date, vote_average, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(kind.as_str())
        .bind(i as i64)
        .bind(it.tmdb_id)
        .bind(i64::from(it.is_movie))
        .bind(&it.title)
        .bind(it.year)
        .bind(&it.overview)
        .bind(&it.poster_path)
        .bind(&it.backdrop_path)
        .bind(&it.release_date)
        .bind(it.vote_average)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load_feed(pool: &SqlitePool, kind: DiscoverKind) -> Result<Vec<DiscoverItem>> {
    let rows = sqlx::query_as::<_, (i64, i64, String, Option<i64>, Option<String>, Option<String>, Option<String>, Option<String>, Option<f64>)>(
        "SELECT tmdb_id, is_movie, title, year, overview, poster_path,
                backdrop_path, release_date, vote_average
         FROM discover_feed WHERE kind = ? ORDER BY position",
    )
    .bind(kind.as_str())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(tmdb_id, is_movie, title, year, overview, poster, backdrop, release, vote)| {
            DiscoverItem {
                tmdb_id,
                is_movie: is_movie != 0,
                title,
                year,
                overview,
                poster_path: poster,
                backdrop_path: backdrop,
                release_date: release,
                vote_average: vote,
            }
        })
        .collect())
}

/// Returns the unix timestamp of the freshest row for `kind`, or `None` when
/// the cache is empty. The refresh scheduler uses this to decide whether to
/// hit TMDB again or serve from cache.
pub async fn last_fetched(pool: &SqlitePool, kind: DiscoverKind) -> Result<Option<i64>> {
    let v: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(fetched_at) FROM discover_feed WHERE kind = ?",
    )
    .bind(kind.as_str())
    .fetch_one(pool)
    .await?;
    Ok(v)
}

/// Pull a fresh page from `provider` and mirror it into the cache.
pub async fn refresh<P: DiscoverProvider + ?Sized>(
    pool: &SqlitePool,
    provider: &P,
    kind: DiscoverKind,
) -> Result<Vec<DiscoverItem>> {
    let items = provider.feed(kind, 1).await?;
    cache_feed(pool, kind, &items).await?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use std::sync::Mutex;

    struct FakeProvider {
        calls: Mutex<Vec<(DiscoverKind, u32)>>,
        rows: Vec<DiscoverItem>,
    }

    #[async_trait]
    impl DiscoverProvider for FakeProvider {
        async fn feed(&self, kind: DiscoverKind, page: u32) -> Result<Vec<DiscoverItem>> {
            self.calls.lock().unwrap().push((kind, page));
            Ok(self.rows.clone())
        }
    }

    fn sample_movie(id: i64, title: &str, year: i64) -> DiscoverItem {
        DiscoverItem {
            tmdb_id: id,
            is_movie: true,
            title: title.into(),
            year: Some(year),
            overview: Some("plot".into()),
            poster_path: Some(format!("/p{id}.jpg")),
            backdrop_path: None,
            release_date: Some(format!("{year}-06-14")),
            vote_average: Some(7.5),
        }
    }

    #[test]
    fn parse_trending_movie_payload() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[
                {"id":550,"title":"Fight Club","release_date":"1999-10-15",
                 "overview":"...","poster_path":"/p.jpg","backdrop_path":"/b.jpg","vote_average":8.4},
                {"id":11,"title":"Star Wars","release_date":"1977-05-25"}
            ]}"#,
        )
        .unwrap();
        let parsed = parse_results(&v, true);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].tmdb_id, 550);
        assert_eq!(parsed[0].year, Some(1999));
        assert_eq!(parsed[0].vote_average, Some(8.4));
        assert!(parsed[0].is_movie);
        assert_eq!(parsed[1].title, "Star Wars");
        assert_eq!(parsed[1].year, Some(1977));
    }

    #[test]
    fn parse_trending_show_payload_uses_name_and_first_air_date() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[
                {"id":1396,"name":"Breaking Bad","first_air_date":"2008-01-20","overview":"meth"}
            ]}"#,
        )
        .unwrap();
        let parsed = parse_results(&v, false);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Breaking Bad");
        assert_eq!(parsed[0].year, Some(2008));
        assert!(!parsed[0].is_movie);
    }

    #[test]
    fn parse_skips_rows_missing_id() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"results":[{"title":"orphan"},{"id":42,"title":"ok"}]}"#).unwrap();
        let parsed = parse_results(&v, true);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].tmdb_id, 42);
    }

    #[tokio::test]
    async fn cache_round_trip_preserves_order() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let items = vec![
            sample_movie(1, "A", 2020),
            sample_movie(2, "B", 2021),
            sample_movie(3, "C", 2022),
        ];
        cache_feed(&pool, DiscoverKind::TrendingMoviesDay, &items)
            .await
            .unwrap();
        let loaded = load_feed(&pool, DiscoverKind::TrendingMoviesDay).await.unwrap();
        assert_eq!(loaded, items);
    }

    #[tokio::test]
    async fn cache_replaces_previous_kind_rows() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_feed(
            &pool,
            DiscoverKind::TrendingMoviesDay,
            &[sample_movie(1, "A", 2020), sample_movie(2, "B", 2021)],
        )
        .await
        .unwrap();
        cache_feed(
            &pool,
            DiscoverKind::TrendingMoviesDay,
            &[sample_movie(9, "Z", 2025)],
        )
        .await
        .unwrap();
        let loaded = load_feed(&pool, DiscoverKind::TrendingMoviesDay).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].tmdb_id, 9);
    }

    #[tokio::test]
    async fn cache_isolates_kinds() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_feed(
            &pool,
            DiscoverKind::TrendingMoviesDay,
            &[sample_movie(1, "Day", 2020)],
        )
        .await
        .unwrap();
        cache_feed(
            &pool,
            DiscoverKind::UpcomingMovies,
            &[sample_movie(2, "Upcoming", 2026)],
        )
        .await
        .unwrap();
        let day = load_feed(&pool, DiscoverKind::TrendingMoviesDay).await.unwrap();
        let up = load_feed(&pool, DiscoverKind::UpcomingMovies).await.unwrap();
        assert_eq!(day.len(), 1);
        assert_eq!(up.len(), 1);
        assert_eq!(day[0].title, "Day");
        assert_eq!(up[0].title, "Upcoming");
    }

    #[tokio::test]
    async fn refresh_pulls_provider_then_persists() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let provider = FakeProvider {
            calls: Mutex::new(Vec::new()),
            rows: vec![sample_movie(7, "Live", 2026)],
        };
        let got = refresh(&pool, &provider, DiscoverKind::UpcomingMovies)
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(provider.calls.lock().unwrap().as_slice(), &[(DiscoverKind::UpcomingMovies, 1)]);
        let stamp = last_fetched(&pool, DiscoverKind::UpcomingMovies).await.unwrap();
        assert!(stamp.is_some());
        let cached = load_feed(&pool, DiscoverKind::UpcomingMovies).await.unwrap();
        assert_eq!(cached, got);
    }

    #[tokio::test]
    async fn last_fetched_none_when_empty() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        assert_eq!(
            last_fetched(&pool, DiscoverKind::TrendingShowsWeek).await.unwrap(),
            None
        );
    }
}
