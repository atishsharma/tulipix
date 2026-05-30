//! Movie collections — TMDB `belongs_to_collection` grouping (MCU, LOTR,
//! Mission Impossible …). The Videos landing page renders the collection as
//! a single card; expanding it shows the members in release-date order.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CollectionMeta {
    pub tmdb_id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionMember {
    pub tmdb_movie_id: i64,
    pub title: String,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
}

#[async_trait]
pub trait CollectionProvider: Send + Sync {
    /// Returns the metadata + ordered member list for a TMDB collection.
    async fn collection(&self, collection_id: i64) -> Result<Option<(CollectionMeta, Vec<CollectionMember>)>>;
}

pub struct TmdbCollections {
    pub api_key: String,
    pub base: String,
    http: reqwest::Client,
}

impl TmdbCollections {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base: "https://api.themoviedb.org/3".into(),
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl CollectionProvider for TmdbCollections {
    async fn collection(&self, collection_id: i64) -> Result<Option<(CollectionMeta, Vec<CollectionMember>)>> {
        let url = format!("{}/collection/{collection_id}", self.base);
        let resp = self
            .http
            .get(url)
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(Some(parse(&v)))
    }
}

fn parse(v: &serde_json::Value) -> (CollectionMeta, Vec<CollectionMember>) {
    let meta = CollectionMeta {
        tmdb_id: v.get("id").and_then(|x| x.as_i64()).unwrap_or_default(),
        name: v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        overview: v.get("overview").and_then(|x| x.as_str()).map(str::to_string),
        poster_path: v.get("poster_path").and_then(|x| x.as_str()).map(str::to_string),
        backdrop_path: v.get("backdrop_path").and_then(|x| x.as_str()).map(str::to_string),
    };
    let mut members: Vec<CollectionMember> = v
        .get("parts")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|row| {
                    Some(CollectionMember {
                        tmdb_movie_id: row.get("id").and_then(|x| x.as_i64())?,
                        title: row.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        release_date: row.get("release_date").and_then(|x| x.as_str())
                            .filter(|s| !s.is_empty())
                            .map(str::to_string),
                        poster_path: row.get("poster_path").and_then(|x| x.as_str()).map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    members.sort_by(|a, b| {
        let ad = a.release_date.as_deref().unwrap_or("9999-99-99");
        let bd = b.release_date.as_deref().unwrap_or("9999-99-99");
        ad.cmp(bd)
    });
    (meta, members)
}

pub const COLLECTIONS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS collections (
    tmdb_id      INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL,
    overview     TEXT,
    poster_path  TEXT,
    backdrop_path TEXT,
    updated      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS collection_members (
    collection_id  INTEGER NOT NULL REFERENCES collections(tmdb_id) ON DELETE CASCADE,
    tmdb_movie_id  INTEGER NOT NULL,
    title          TEXT    NOT NULL,
    release_date   TEXT,
    poster_path    TEXT,
    PRIMARY KEY (collection_id, tmdb_movie_id)
);
CREATE INDEX IF NOT EXISTS collection_members_release_idx
    ON collection_members(collection_id, release_date);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(COLLECTIONS_SCHEMA).execute(pool).await?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn upsert(pool: &SqlitePool, meta: &CollectionMeta, members: &[CollectionMember]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO collections (tmdb_id, name, overview, poster_path, backdrop_path, updated)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(tmdb_id) DO UPDATE SET
            name = excluded.name, overview = excluded.overview,
            poster_path = excluded.poster_path, backdrop_path = excluded.backdrop_path,
            updated = excluded.updated",
    )
    .bind(meta.tmdb_id)
    .bind(&meta.name)
    .bind(&meta.overview)
    .bind(&meta.poster_path)
    .bind(&meta.backdrop_path)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM collection_members WHERE collection_id = ?")
        .bind(meta.tmdb_id)
        .execute(&mut *tx)
        .await?;
    for m in members {
        sqlx::query(
            "INSERT INTO collection_members (collection_id, tmdb_movie_id, title, release_date, poster_path)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(meta.tmdb_id)
        .bind(m.tmdb_movie_id)
        .bind(&m.title)
        .bind(&m.release_date)
        .bind(&m.poster_path)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load(pool: &SqlitePool, collection_id: i64) -> Result<Option<(CollectionMeta, Vec<CollectionMember>)>> {
    let meta: Option<(i64, String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT tmdb_id, name, overview, poster_path, backdrop_path FROM collections WHERE tmdb_id = ?",
    )
    .bind(collection_id)
    .fetch_optional(pool)
    .await?;
    let Some((tmdb_id, name, overview, poster_path, backdrop_path)) = meta else {
        return Ok(None);
    };
    let rows = sqlx::query_as::<_, (i64, String, Option<String>, Option<String>)>(
        "SELECT tmdb_movie_id, title, release_date, poster_path
         FROM collection_members WHERE collection_id = ?
         ORDER BY COALESCE(release_date, '9999-99-99'), tmdb_movie_id",
    )
    .bind(collection_id)
    .fetch_all(pool)
    .await?;
    Ok(Some((
        CollectionMeta {
            tmdb_id,
            name,
            overview,
            poster_path,
            backdrop_path,
        },
        rows.into_iter()
            .map(|(tmdb_movie_id, title, release_date, poster_path)| CollectionMember {
                tmdb_movie_id,
                title,
                release_date,
                poster_path,
            })
            .collect(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn parse_orders_members_by_release_date() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id":1,"name":"LOTR","parts":[
                {"id":121,"title":"The Two Towers","release_date":"2002-12-18"},
                {"id":120,"title":"Fellowship","release_date":"2001-12-19"},
                {"id":122,"title":"Return of the King","release_date":"2003-12-17"},
                {"id":999,"title":"Mystery Cut"}
            ]}"#,
        ).unwrap();
        let (meta, members) = parse(&v);
        assert_eq!(meta.name, "LOTR");
        assert_eq!(members.len(), 4);
        assert_eq!(members[0].tmdb_movie_id, 120);
        assert_eq!(members[1].tmdb_movie_id, 121);
        assert_eq!(members[2].tmdb_movie_id, 122);
        assert_eq!(members[3].tmdb_movie_id, 999, "unreleased ranks last");
    }

    #[test]
    fn parse_skips_member_without_id() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id":1,"name":"X","parts":[{"title":"orphan"}]}"#,
        ).unwrap();
        let (_, members) = parse(&v);
        assert!(members.is_empty());
    }

    #[tokio::test]
    async fn db_round_trip() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let meta = CollectionMeta {
            tmdb_id: 10,
            name: "MCU".into(),
            ..CollectionMeta::default()
        };
        let members = vec![
            CollectionMember {
                tmdb_movie_id: 1,
                title: "A".into(),
                release_date: Some("2020".into()),
                poster_path: None,
            },
            CollectionMember {
                tmdb_movie_id: 2,
                title: "B".into(),
                release_date: Some("2018".into()),
                poster_path: None,
            },
        ];
        upsert(&pool, &meta, &members).await.unwrap();
        let (_, loaded) = load(&pool, 10).await.unwrap().unwrap();
        assert_eq!(loaded[0].title, "B");
        assert_eq!(loaded[1].title, "A");
    }

    #[tokio::test]
    async fn upsert_replaces_members() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let meta = CollectionMeta {
            tmdb_id: 7,
            name: "X".into(),
            ..CollectionMeta::default()
        };
        upsert(
            &pool,
            &meta,
            &[CollectionMember {
                tmdb_movie_id: 1,
                title: "Old".into(),
                release_date: None,
                poster_path: None,
            }],
        )
        .await
        .unwrap();
        upsert(
            &pool,
            &meta,
            &[CollectionMember {
                tmdb_movie_id: 2,
                title: "New".into(),
                release_date: None,
                poster_path: None,
            }],
        )
        .await
        .unwrap();
        let (_, members) = load(&pool, 7).await.unwrap().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].tmdb_movie_id, 2);
    }
}
