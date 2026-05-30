//! FTS5 search across filename, camera, tags, people, and notes, plus a
//! date-range filter.
//!
//! The FTS table is built in the schema (`photo_fts`); this module owns the
//! `index_item` writer and the `query` reader. The query language is a small
//! subset:
//!
//!   "kw1 kw2"           → both keywords (AND, default FTS5)
//!   tag:beach           → matches a tag column term
//!   person:"Mom"        → matches the people column
//!   "phrase like this"  → phrase match
//!
//! Date filtering is supplied separately (start/end Unix seconds).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub item_id: i64,
    pub rank: f64,
    pub abs_path: String,
    pub taken_at: Option<i64>,
}

/// Build the FTS row for one item from the current join over items + tags +
/// faces. Idempotent — deletes the prior row first.
pub async fn index_item(pool: &SqlitePool, item_id: i64) -> Result<()> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT items.abs_path, photo_meta.camera_make || ' ' || photo_meta.camera_model, NULL
         FROM items LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.id = ?",
    )
    .bind(item_id).fetch_optional(pool).await?;
    let Some((abs, camera, _)) = row else { return Ok(()); };
    let filename = Path::new(&abs).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();

    let tags: Vec<(String,)> = sqlx::query_as(
        "SELECT t.name FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE it.item_id = ?",
    ).bind(item_id).fetch_all(pool).await?;
    let people: Vec<(String,)> = sqlx::query_as(
        "SELECT pe.name FROM faces f JOIN people pe ON pe.id = f.person_id WHERE f.item_id = ? AND pe.name IS NOT NULL",
    ).bind(item_id).fetch_all(pool).await?;

    let tags_str = tags.into_iter().map(|(s,)| s).collect::<Vec<_>>().join(" ");
    let people_str = people.into_iter().map(|(s,)| s).collect::<Vec<_>>().join(" ");

    sqlx::query("DELETE FROM photo_fts WHERE item_id = ?").bind(item_id).execute(pool).await?;
    sqlx::query(
        "INSERT INTO photo_fts (item_id, filename, camera, tags, people, notes) VALUES (?, ?, ?, ?, ?, '')",
    )
    .bind(item_id).bind(filename).bind(camera.unwrap_or_default()).bind(tags_str).bind(people_str)
    .execute(pool).await?;
    Ok(())
}

/// Translate the user-friendly mini-language into FTS5 MATCH syntax.
pub fn translate_query(q: &str) -> String {
    // Strip dangerous characters; let FTS5 see only safe tokens + colons.
    let mut out = String::with_capacity(q.len() + 8);
    let mut in_quote = false;
    for ch in q.chars() {
        match ch {
            '"' => { in_quote = !in_quote; out.push('"'); }
            c if c.is_alphanumeric() || c == '_' || c == '-' || c == ':' || c == ' ' || c == '*' => out.push(c),
            _ if in_quote => out.push(ch),
            _ => out.push(' '),
        }
    }
    out.trim().to_string()
}

pub async fn query(
    pool: &SqlitePool,
    text: &str,
    date_from: Option<i64>,
    date_to: Option<i64>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let needle = translate_query(text);
    let mut sql = String::from(
        "SELECT items.id, bm25(photo_fts) AS rk, items.abs_path, photo_meta.taken_at
         FROM photo_fts
         JOIN items ON items.id = photo_fts.item_id
         LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_fts MATCH ?
           AND items.missing_since IS NULL",
    );
    if date_from.is_some() { sql.push_str(" AND photo_meta.taken_at >= ?"); }
    if date_to.is_some()   { sql.push_str(" AND photo_meta.taken_at <  ?"); }
    sql.push_str(" ORDER BY rk LIMIT ?");

    let mut q = sqlx::query_as::<sqlx::Sqlite, (i64, f64, String, Option<i64>)>(&sql).bind(needle);
    if let Some(s) = date_from { q = q.bind(s); }
    if let Some(e) = date_to   { q = q.bind(e); }
    q = q.bind(limit);
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, rk, p, t)| SearchHit { item_id: id, rank: rk, abs_path: p, taken_at: t })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn index_and_query_finds_filename_and_tag() {
        let (_t, pool) = open_pool().await;
        // Seed an item, a tag, an item_tags link.
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p/beach-1.jpg', 0, 1, 0, 'photos', 0, 0)")
            .execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/p/beach-1.jpg'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, taken_at) VALUES (?, 1700000000)").bind(id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tags (name) VALUES ('beach')").execute(&pool).await.unwrap();
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'beach'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO item_tags (item_id, tag_id) VALUES (?, ?)").bind(id).bind(tag_id).execute(&pool).await.unwrap();

        index_item(&pool, id).await.unwrap();

        let hits = query(&pool, "beach", None, None, 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        // Date filter narrows out
        let none = query(&pool, "beach", Some(1900000000), None, 10).await.unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn translate_strips_unsafe_chars() {
        assert_eq!(translate_query("beach AND cat"), "beach AND cat");
        let cleaned = translate_query("hi;DROP TABLE x");
        assert!(!cleaned.contains(';'));
    }
}
