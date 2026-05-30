//! Extras rail — trailers / featurettes / behind-the-scenes via TMDB
//! `/movie/{id}/videos` + `/tv/{id}/videos`. Cached to a tiny table so the
//! detail page renders instantly even when offline.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtraKind {
    Trailer,
    Teaser,
    Featurette,
    BehindTheScenes,
    Clip,
    Bloopers,
    Other,
}

impl ExtraKind {
    pub fn parse(tmdb_type: &str) -> Self {
        match tmdb_type {
            "Trailer" => ExtraKind::Trailer,
            "Teaser" => ExtraKind::Teaser,
            "Featurette" => ExtraKind::Featurette,
            "Behind the Scenes" => ExtraKind::BehindTheScenes,
            "Clip" => ExtraKind::Clip,
            "Bloopers" => ExtraKind::Bloopers,
            _ => ExtraKind::Other,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ExtraKind::Trailer => "trailer",
            ExtraKind::Teaser => "teaser",
            ExtraKind::Featurette => "featurette",
            ExtraKind::BehindTheScenes => "behind-the-scenes",
            ExtraKind::Clip => "clip",
            ExtraKind::Bloopers => "bloopers",
            ExtraKind::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraVideo {
    pub tmdb_id: i64,
    pub key: String,        // YouTube key — playable via yt-dlp
    pub site: String,       // YouTube / Vimeo
    pub kind: ExtraKind,
    pub name: String,
    pub official: bool,
    pub published_at: Option<String>,
}

#[async_trait]
pub trait ExtrasProvider: Send + Sync {
    async fn movie_videos(&self, tmdb_id: i64) -> Result<Vec<ExtraVideo>>;
    async fn show_videos(&self, tmdb_id: i64) -> Result<Vec<ExtraVideo>>;
}

pub struct TmdbExtras {
    pub api_key: String,
    pub base: String,
    http: reqwest::Client,
}

impl TmdbExtras {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: "https://api.themoviedb.org/3".into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ExtrasProvider for TmdbExtras {
    async fn movie_videos(&self, tmdb_id: i64) -> Result<Vec<ExtraVideo>> {
        let url = format!("{}/movie/{tmdb_id}/videos", self.base);
        let v: serde_json::Value = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_videos(&v, tmdb_id))
    }
    async fn show_videos(&self, tmdb_id: i64) -> Result<Vec<ExtraVideo>> {
        let url = format!("{}/tv/{tmdb_id}/videos", self.base);
        let v: serde_json::Value = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(parse_videos(&v, tmdb_id))
    }
}

fn parse_videos(v: &serde_json::Value, tmdb_id: i64) -> Vec<ExtraVideo> {
    let Some(arr) = v.get("results").and_then(|x| x.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|row| {
            Some(ExtraVideo {
                tmdb_id,
                key: row.get("key").and_then(|x| x.as_str())?.to_string(),
                site: row.get("site").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                kind: ExtraKind::parse(row.get("type").and_then(|x| x.as_str()).unwrap_or("")),
                name: row.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                official: row.get("official").and_then(|x| x.as_bool()).unwrap_or(false),
                published_at: row.get("published_at").and_then(|x| x.as_str()).map(str::to_string),
            })
        })
        .collect()
}

pub const EXTRAS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS extras (
    tmdb_id      INTEGER NOT NULL,
    media        TEXT    NOT NULL,  -- 'movie' | 'show'
    key          TEXT    NOT NULL,
    site         TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    official     INTEGER NOT NULL,
    published_at TEXT,
    fetched_at   INTEGER NOT NULL,
    PRIMARY KEY (tmdb_id, media, key)
);
CREATE INDEX IF NOT EXISTS extras_lookup_idx ON extras(tmdb_id, media, kind);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(EXTRAS_SCHEMA).execute(pool).await?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn cache(pool: &SqlitePool, media: &str, videos: &[ExtraVideo]) -> Result<()> {
    let mut tx = pool.begin().await?;
    let ts = now();
    for v in videos {
        sqlx::query(
            "INSERT INTO extras (tmdb_id, media, key, site, kind, name, official, published_at, fetched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(tmdb_id, media, key) DO UPDATE SET
                site = excluded.site, kind = excluded.kind, name = excluded.name,
                official = excluded.official, published_at = excluded.published_at,
                fetched_at = excluded.fetched_at",
        )
        .bind(v.tmdb_id)
        .bind(media)
        .bind(&v.key)
        .bind(&v.site)
        .bind(v.kind.as_str())
        .bind(&v.name)
        .bind(i64::from(v.official))
        .bind(&v.published_at)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn list_for(pool: &SqlitePool, tmdb_id: i64, media: &str) -> Result<Vec<ExtraVideo>> {
    let rows = sqlx::query_as::<_, (String, String, String, String, i64, Option<String>)>(
        "SELECT key, site, kind, name, official, published_at
         FROM extras WHERE tmdb_id = ? AND media = ?
         ORDER BY (kind = 'trailer') DESC, official DESC, published_at DESC",
    )
    .bind(tmdb_id)
    .bind(media)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(key, site, kind, name, official, published_at)| ExtraVideo {
            tmdb_id,
            key,
            site,
            kind: parse_kind_str(&kind),
            name,
            official: official != 0,
            published_at,
        })
        .collect())
}

fn parse_kind_str(s: &str) -> ExtraKind {
    match s {
        "trailer" => ExtraKind::Trailer,
        "teaser" => ExtraKind::Teaser,
        "featurette" => ExtraKind::Featurette,
        "behind-the-scenes" => ExtraKind::BehindTheScenes,
        "clip" => ExtraKind::Clip,
        "bloopers" => ExtraKind::Bloopers,
        _ => ExtraKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn parse_extras_payload() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"results":[
                {"key":"abc","site":"YouTube","type":"Trailer","name":"Trailer #1","official":true,"published_at":"2024-01-01"},
                {"key":"def","site":"YouTube","type":"Featurette","name":"FX breakdown","official":false}
            ]}"#,
        )
        .unwrap();
        let parsed = parse_videos(&v, 1);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].kind, ExtraKind::Trailer);
        assert_eq!(parsed[1].kind, ExtraKind::Featurette);
        assert!(parsed[0].official);
        assert!(!parsed[1].official);
    }

    #[test]
    fn parse_skips_rows_without_key() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"results":[{"site":"YouTube","type":"Trailer"}]}"#).unwrap();
        assert!(parse_videos(&v, 1).is_empty());
    }

    #[tokio::test]
    async fn cache_round_trip_trailers_first() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let vids = vec![
            ExtraVideo {
                tmdb_id: 1,
                key: "feat".into(),
                site: "YouTube".into(),
                kind: ExtraKind::Featurette,
                name: "FX".into(),
                official: false,
                published_at: Some("2024-01-01".into()),
            },
            ExtraVideo {
                tmdb_id: 1,
                key: "tr".into(),
                site: "YouTube".into(),
                kind: ExtraKind::Trailer,
                name: "Trailer".into(),
                official: true,
                published_at: Some("2023-12-01".into()),
            },
        ];
        cache(&pool, "movie", &vids).await.unwrap();
        let got = list_for(&pool, 1, "movie").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, ExtraKind::Trailer, "trailer ranks first");
    }

    #[test]
    fn extra_kind_round_trip() {
        for k in [
            ExtraKind::Trailer,
            ExtraKind::Teaser,
            ExtraKind::Featurette,
            ExtraKind::BehindTheScenes,
            ExtraKind::Clip,
            ExtraKind::Bloopers,
            ExtraKind::Other,
        ] {
            assert_eq!(parse_kind_str(k.as_str()), k);
        }
    }
}
