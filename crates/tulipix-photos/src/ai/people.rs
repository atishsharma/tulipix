//! People naming + search-by-face.
//!
//! `set_name` renames a cluster (`'Person 4' → 'Mom'`); `find_by_face` returns
//! every photo containing the given person; `top_unnamed` powers the People
//! tab's "needs a name" carousel. `list` returns everyone with face counts.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub face_count: i64,
    pub cover_face_id: Option<i64>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Person>> {
    let rows: Vec<(i64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT id, name, cover_face FROM people ORDER BY (name IS NULL), name, id"
    ).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, name, cover_face) in rows {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM faces WHERE person_id = ?")
            .bind(id).fetch_one(pool).await?;
        out.push(Person { id, name, face_count: count, cover_face_id: cover_face });
    }
    Ok(out)
}

pub async fn set_name(pool: &SqlitePool, person_id: i64, name: &str) -> Result<()> {
    let n = if name.trim().is_empty() { None } else { Some(name.trim().to_string()) };
    sqlx::query("UPDATE people SET name = ?, updated = ? WHERE id = ?")
        .bind(n).bind(now()).bind(person_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_cover_face(pool: &SqlitePool, person_id: i64, face_id: i64) -> Result<()> {
    sqlx::query("UPDATE people SET cover_face = ?, updated = ? WHERE id = ?")
        .bind(face_id).bind(now()).bind(person_id)
        .execute(pool).await?;
    Ok(())
}

/// Photos that contain `person_id`, newest first.
pub async fn photos_of(pool: &SqlitePool, person_id: i64, limit: i64) -> Result<Vec<(i64, String)>> {
    sqlx::query_as::<_, (i64, String)>(
        "SELECT DISTINCT items.id, items.abs_path
         FROM items
         JOIN faces ON faces.item_id = items.id
         JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE faces.person_id = ?
           AND items.missing_since IS NULL
           AND photo_meta.deleted_at IS NULL
         ORDER BY items.added DESC LIMIT ?"
    )
    .bind(person_id).bind(limit).fetch_all(pool).await
    .map_err(Into::into)
}

/// People whose name is still NULL — ranked by face count so the user names
/// the most-photographed strangers first.
pub async fn top_unnamed(pool: &SqlitePool, limit: i64) -> Result<Vec<Person>> {
    let rows: Vec<(i64, Option<i64>, i64)> = sqlx::query_as(
        "SELECT p.id, p.cover_face, COUNT(f.id) AS c
         FROM people p
         JOIN faces f ON f.person_id = p.id
         WHERE p.name IS NULL
         GROUP BY p.id ORDER BY c DESC LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, cover, c)| Person { id, name: None, face_count: c, cover_face_id: cover }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn set_name_clears_on_empty() {
        let (_t, pool) = open_pool().await;
        let id: i64 = sqlx::query_scalar("INSERT INTO people (created, updated) VALUES (0, 0) RETURNING id").fetch_one(&pool).await.unwrap();
        set_name(&pool, id, "Mom").await.unwrap();
        let n: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id=?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(n.as_deref(), Some("Mom"));
        set_name(&pool, id, "   ").await.unwrap();
        let n: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id=?").bind(id).fetch_one(&pool).await.unwrap();
        assert!(n.is_none());
    }
    #[tokio::test]
    async fn top_unnamed_orders_by_count() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let iid: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        let a: i64 = sqlx::query_scalar("INSERT INTO people (created, updated) VALUES (0, 0) RETURNING id").fetch_one(&pool).await.unwrap();
        let b: i64 = sqlx::query_scalar("INSERT INTO people (created, updated) VALUES (0, 0) RETURNING id").fetch_one(&pool).await.unwrap();
        for _ in 0..3 { sqlx::query("INSERT INTO faces (item_id, person_id, bbox_x, bbox_y, bbox_w, bbox_h, created) VALUES (?, ?, 0, 0, 1, 1, 0)").bind(iid).bind(a).execute(&pool).await.unwrap(); }
        sqlx::query("INSERT INTO faces (item_id, person_id, bbox_x, bbox_y, bbox_w, bbox_h, created) VALUES (?, ?, 0, 0, 1, 1, 0)").bind(iid).bind(b).execute(&pool).await.unwrap();
        let ranked = top_unnamed(&pool, 10).await.unwrap();
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].id, a);
    }
}
