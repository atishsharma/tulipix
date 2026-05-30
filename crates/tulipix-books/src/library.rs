//! `np.p4.books.library` — library views.
//!
//! Series grouping, custom collections, author pages, and read/unread filters.
//! Each is a small query over `book_meta` + `series` + `collections` +
//! `reading_progress`, scoped to present files.

use anyhow::Result;
use sqlx::SqlitePool;

/// Get-or-create a series row by name; assign a book to it at `index`.
pub async fn assign_series(pool: &SqlitePool, item_id: i64, series_name: &str, index: f64) -> Result<i64> {
    let sid: i64 = match sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?").bind(series_name).fetch_optional(pool).await? {
        Some(id) => id,
        None => sqlx::query_scalar("INSERT INTO series (name) VALUES (?) RETURNING id").bind(series_name).fetch_one(pool).await?,
    };
    sqlx::query("UPDATE book_meta SET series_id = ?, series_index = ? WHERE item_id = ?")
        .bind(sid).bind(index).bind(item_id).execute(pool).await?;
    Ok(sid)
}

/// Books in a series, in reading order.
pub async fn series_books(pool: &SqlitePool, series_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT book_meta.item_id FROM book_meta JOIN items ON items.id = book_meta.item_id
         WHERE series_id = ? AND items.missing_since IS NULL
         ORDER BY series_index, book_meta.title",
    ).bind(series_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Distinct authors with book counts.
pub async fn authors(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT author, COUNT(*) FROM book_meta JOIN items ON items.id = book_meta.item_id
         WHERE items.missing_since IS NULL AND author IS NOT NULL AND author != ''
         GROUP BY author ORDER BY author COLLATE NOCASE",
    ).fetch_all(pool).await?)
}

pub async fn add_to_collection(pool: &SqlitePool, name: &str, item_id: i64) -> Result<i64> {
    let cid: i64 = match sqlx::query_scalar::<_, i64>("SELECT id FROM collections WHERE name = ?").bind(name).fetch_optional(pool).await? {
        Some(id) => id,
        None => sqlx::query_scalar("INSERT INTO collections (name) VALUES (?) RETURNING id").bind(name).fetch_one(pool).await?,
    };
    sqlx::query("INSERT OR IGNORE INTO collection_items (collection_id, item_id) VALUES (?, ?)")
        .bind(cid).bind(item_id).execute(pool).await?;
    Ok(cid)
}

/// Unread books (no progress row, or progress not finished).
pub async fn unread(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT book_meta.item_id FROM book_meta
         JOIN items ON items.id = book_meta.item_id
         LEFT JOIN reading_progress rp ON rp.item_id = book_meta.item_id
         WHERE items.missing_since IS NULL AND COALESCE(rp.finished, 0) = 0
         ORDER BY book_meta.title COLLATE NOCASE",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_book};

    #[tokio::test]
    async fn series_groups_in_order() {
        let (_t, pool) = open_pool().await;
        let a = add_book(&pool, "/b/2.cbz", "cbz", true).await;
        let b = add_book(&pool, "/b/1.cbz", "cbz", true).await;
        let sid = assign_series(&pool, a, "Saga", 2.0).await.unwrap();
        assign_series(&pool, b, "Saga", 1.0).await.unwrap();
        assert_eq!(series_books(&pool, sid).await.unwrap(), vec![b, a]);
    }

    #[tokio::test]
    async fn unread_excludes_finished() {
        let (_t, pool) = open_pool().await;
        let a = add_book(&pool, "/b/a.epub", "epub", false).await;
        let b = add_book(&pool, "/b/b.epub", "epub", false).await;
        sqlx::query("INSERT INTO reading_progress (item_id, page, finished, updated) VALUES (?, 10, 1, 0)").bind(a).execute(&pool).await.unwrap();
        let u = unread(&pool).await.unwrap();
        assert_eq!(u, vec![b]);
    }

    #[tokio::test]
    async fn collections_dedup() {
        let (_t, pool) = open_pool().await;
        let a = add_book(&pool, "/b/a.epub", "epub", false).await;
        add_to_collection(&pool, "Fav", a).await.unwrap();
        add_to_collection(&pool, "Fav", a).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM collection_items").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(authors(&pool).await.unwrap().len(), 0);
    }
}
