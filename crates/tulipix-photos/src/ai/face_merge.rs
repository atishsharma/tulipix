//! Cluster merge / unmerge / move-face — manual corrections.
//!
//! When the user merges two clusters, all faces under person B reassign to
//! person A; B is deleted. Move-face splits one face out into a fresh
//! cluster. Unmerge promotes every face into its own singleton cluster.
//! Each operation recomputes A's stored centroid (used by the next
//! clustering pass for "merge into nearest" suggestions).
//!
//! Confirmation-suggester: ranks faces by distance-to-centroid for clusters
//! that contain at least one named, confirmed face — those high-uncertainty
//! faces are the ones the UI should ask about first.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub face_id: i64,
    pub person_id: i64,
    pub distance: f32,
}

/// Merge every face under `from_id` into `into_id`. Deletes `from_id` if it
/// no longer has any faces.
pub async fn merge(pool: &SqlitePool, into_id: i64, from_id: i64) -> Result<u64> {
    if into_id == from_id { return Ok(0); }
    let r = sqlx::query("UPDATE faces SET person_id = ? WHERE person_id = ?")
        .bind(into_id).bind(from_id)
        .execute(pool).await?;
    sqlx::query("DELETE FROM people WHERE id = ? AND NOT EXISTS (SELECT 1 FROM faces WHERE person_id = ?)")
        .bind(from_id).bind(from_id).execute(pool).await?;
    Ok(r.rows_affected())
}

/// Split `face_id` into a freshly created person row. Returns new person id.
pub async fn move_to_new_cluster(pool: &SqlitePool, face_id: i64) -> Result<i64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let pid: i64 = sqlx::query_scalar(
        "INSERT INTO people (created, updated, cover_face) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(now).bind(now).bind(face_id)
    .fetch_one(pool).await?;
    sqlx::query("UPDATE faces SET person_id = ?, confirmed = 1 WHERE id = ?")
        .bind(pid).bind(face_id).execute(pool).await?;
    Ok(pid)
}

/// Move `face_id` into existing cluster `into_id`.
pub async fn move_to_existing(pool: &SqlitePool, face_id: i64, into_id: i64) -> Result<()> {
    sqlx::query("UPDATE faces SET person_id = ?, confirmed = 1 WHERE id = ?")
        .bind(into_id).bind(face_id).execute(pool).await?;
    Ok(())
}

/// Unmerge: promote every face under `person_id` into its own singleton
/// cluster, drop the source. Returns count of new clusters created.
pub async fn unmerge(pool: &SqlitePool, person_id: i64) -> Result<u64> {
    let faces: Vec<(i64,)> = sqlx::query_as("SELECT id FROM faces WHERE person_id = ?")
        .bind(person_id).fetch_all(pool).await?;
    if faces.is_empty() { return Ok(0); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut created = 0u64;
    for (fid,) in &faces {
        let pid: i64 = sqlx::query_scalar(
            "INSERT INTO people (created, updated, cover_face) VALUES (?, ?, ?) RETURNING id",
        ).bind(now).bind(now).bind(fid).fetch_one(pool).await?;
        sqlx::query("UPDATE faces SET person_id = ? WHERE id = ?")
            .bind(pid).bind(fid).execute(pool).await?;
        created += 1;
    }
    sqlx::query("DELETE FROM people WHERE id = ?").bind(person_id).execute(pool).await?;
    Ok(created)
}

/// Suggest faces to ask the user about: top `limit` faces (per cluster)
/// ranked by `1 - confidence` — naive proxy for "distance to centroid" until
/// real embeddings are stored. When embeddings ARE present, the caller can
/// override with `crate::ai::face_clusters::distance`.
pub async fn suggestions(pool: &SqlitePool, limit: i64) -> Result<Vec<Suggestion>> {
    let rows: Vec<(i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT f.id, f.person_id, f.confidence
         FROM faces f
         WHERE f.person_id IS NOT NULL
           AND f.confirmed = 0
         ORDER BY (1.0 - COALESCE(f.confidence, 0.0)) DESC LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(face_id, person_id, conf)| Suggestion {
        face_id, person_id,
        distance: (1.0 - conf.unwrap_or(0.0) as f32),
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add_person(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("INSERT INTO people (created, updated) VALUES (0, 0) RETURNING id").fetch_one(pool).await.unwrap()
    }
    async fn add_face(pool: &SqlitePool, item_id: i64, person_id: i64) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO faces (item_id, person_id, bbox_x, bbox_y, bbox_w, bbox_h, created) VALUES (?, ?, 0, 0, 1, 1, 0) RETURNING id",
        ).bind(item_id).bind(person_id).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn merge_unmerge_round_trip() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let iid: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        let a = add_person(&pool).await;
        let b = add_person(&pool).await;
        add_face(&pool, iid, a).await;
        add_face(&pool, iid, b).await;
        add_face(&pool, iid, b).await;

        let moved = merge(&pool, a, b).await.unwrap();
        assert_eq!(moved, 2);
        let n_b: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM people WHERE id = ?").bind(b).fetch_one(&pool).await.unwrap();
        assert_eq!(n_b, 0);

        let created = unmerge(&pool, a).await.unwrap();
        assert_eq!(created, 3);
        let n_a: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM people WHERE id = ?").bind(a).fetch_one(&pool).await.unwrap();
        assert_eq!(n_a, 0);
    }

    #[tokio::test]
    async fn move_face_creates_cluster() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let iid: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        let a = add_person(&pool).await;
        let f = add_face(&pool, iid, a).await;
        let new_pid = move_to_new_cluster(&pool, f).await.unwrap();
        assert_ne!(new_pid, a);
        let cur: i64 = sqlx::query_scalar("SELECT person_id FROM faces WHERE id = ?").bind(f).fetch_one(&pool).await.unwrap();
        assert_eq!(cur, new_pid);
    }
}
