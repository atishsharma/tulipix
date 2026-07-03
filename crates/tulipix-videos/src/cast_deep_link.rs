//! Cast & Crew deep links — click an actor → unified view across local
//! movies/shows AND photos that contain the same face cluster.
//!
//! Movies/shows come from TMDB credits stored in `cast` (per-item) and `crew`
//! (per-item) tables. Photo bridge lives via `face_clusters.tmdb_person_id`
//! once a user manually maps a cluster to a TMDB person, or via AI face-match
//! (np.p3.faces in photos). Lookup here is pure-SQL — no scraping — so the UI
//! stays responsive on cold starts.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tulipix_core::util::unix_secs_i64 as now;

pub const CAST_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS people (
    tmdb_id      INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL,
    profile_path TEXT,
    updated      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cast_credits (
    item_id      INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tmdb_person  INTEGER NOT NULL REFERENCES people(tmdb_id) ON DELETE CASCADE,
    character    TEXT,
    ord          INTEGER NOT NULL,
    PRIMARY KEY (item_id, tmdb_person)
);
CREATE INDEX IF NOT EXISTS cast_credits_person_idx ON cast_credits(tmdb_person, ord);

CREATE TABLE IF NOT EXISTS crew_credits (
    item_id      INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tmdb_person  INTEGER NOT NULL REFERENCES people(tmdb_id) ON DELETE CASCADE,
    job          TEXT    NOT NULL,
    department   TEXT,
    PRIMARY KEY (item_id, tmdb_person, job)
);
CREATE INDEX IF NOT EXISTS crew_credits_person_idx ON crew_credits(tmdb_person);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(CAST_SCHEMA).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonRef {
    pub tmdb_id: i64,
    pub name: String,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastEntry {
    pub item_id: i64,
    pub character: Option<String>,
    pub ord: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewEntry {
    pub item_id: i64,
    pub job: String,
    pub department: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonDeepLink {
    pub person: PersonRef,
    pub acted_in: Vec<CastEntry>,
    pub crewed_on: Vec<CrewEntry>,
    /// Photos-section bridge — face-cluster ids that the user (or AI) mapped
    /// to this TMDB person. Empty when no mapping exists yet.
    pub photo_clusters: Vec<i64>,
}

pub async fn upsert_person(pool: &SqlitePool, p: &PersonRef) -> Result<()> {
    sqlx::query(
        "INSERT INTO people (tmdb_id, name, profile_path, updated)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(tmdb_id) DO UPDATE SET
            name = excluded.name,
            profile_path = excluded.profile_path,
            updated = excluded.updated",
    )
    .bind(p.tmdb_id)
    .bind(&p.name)
    .bind(&p.profile_path)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_cast(
    pool: &SqlitePool,
    item_id: i64,
    tmdb_person: i64,
    character: Option<&str>,
    ord: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cast_credits (item_id, tmdb_person, character, ord)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(item_id, tmdb_person) DO UPDATE SET
            character = excluded.character, ord = excluded.ord",
    )
    .bind(item_id)
    .bind(tmdb_person)
    .bind(character)
    .bind(ord)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_crew(
    pool: &SqlitePool,
    item_id: i64,
    tmdb_person: i64,
    job: &str,
    department: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO crew_credits (item_id, tmdb_person, job, department)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(item_id, tmdb_person, job) DO UPDATE SET
            department = excluded.department",
    )
    .bind(item_id)
    .bind(tmdb_person)
    .bind(job)
    .bind(department)
    .execute(pool)
    .await?;
    Ok(())
}

/// Pulls everything we know about a TMDB person across the local library —
/// powers the actor profile page. Caller bolts on photo clusters via
/// `tulipix-photos::faces::clusters_for_person` to avoid an unconditional
/// cross-section join (Photos may live in a separate DB).
pub async fn deep_link(
    pool: &SqlitePool,
    tmdb_person: i64,
    photo_clusters: Vec<i64>,
) -> Result<Option<PersonDeepLink>> {
    let person: Option<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT tmdb_id, name, profile_path FROM people WHERE tmdb_id = ?",
    )
    .bind(tmdb_person)
    .fetch_optional(pool)
    .await?;
    let Some((tmdb_id, name, profile_path)) = person else {
        return Ok(None);
    };

    let cast_rows: Vec<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT item_id, character, ord FROM cast_credits
         WHERE tmdb_person = ? ORDER BY ord",
    )
    .bind(tmdb_person)
    .fetch_all(pool)
    .await?;
    let crew_rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT item_id, job, department FROM crew_credits
         WHERE tmdb_person = ? ORDER BY job",
    )
    .bind(tmdb_person)
    .fetch_all(pool)
    .await?;

    Ok(Some(PersonDeepLink {
        person: PersonRef {
            tmdb_id,
            name,
            profile_path,
        },
        acted_in: cast_rows
            .into_iter()
            .map(|(item_id, character, ord)| CastEntry {
                item_id,
                character,
                ord,
            })
            .collect(),
        crewed_on: crew_rows
            .into_iter()
            .map(|(item_id, job, department)| CrewEntry {
                item_id,
                job,
                department,
            })
            .collect(),
        photo_clusters,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn insert_item(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query(
            "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated)
             VALUES (?, 0, 1, 0, 'videos', 0, 0)",
        )
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(path)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn deep_link_assembles_cast_and_crew() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let person = PersonRef {
            tmdb_id: 287,
            name: "Brad Pitt".into(),
            profile_path: Some("/bp.jpg".into()),
        };
        upsert_person(&pool, &person).await.unwrap();
        let fight_club = insert_item(&pool, "/fc.mkv").await;
        let troy = insert_item(&pool, "/troy.mkv").await;
        record_cast(&pool, fight_club, 287, Some("Tyler Durden"), 0).await.unwrap();
        record_cast(&pool, troy, 287, Some("Achilles"), 0).await.unwrap();
        record_crew(&pool, fight_club, 287, "Producer", Some("Production")).await.unwrap();

        let link = deep_link(&pool, 287, vec![42, 99]).await.unwrap().unwrap();
        assert_eq!(link.person.name, "Brad Pitt");
        assert_eq!(link.acted_in.len(), 2);
        assert_eq!(link.crewed_on.len(), 1);
        assert_eq!(link.photo_clusters, vec![42, 99]);
    }

    #[tokio::test]
    async fn deep_link_missing_returns_none() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let got = deep_link(&pool, 1, vec![]).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn cast_upsert_idempotent() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let person = PersonRef {
            tmdb_id: 1,
            name: "X".into(),
            profile_path: None,
        };
        upsert_person(&pool, &person).await.unwrap();
        let item = insert_item(&pool, "/x.mkv").await;
        record_cast(&pool, item, 1, Some("Role A"), 0).await.unwrap();
        record_cast(&pool, item, 1, Some("Role A revised"), 5).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cast_credits")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
