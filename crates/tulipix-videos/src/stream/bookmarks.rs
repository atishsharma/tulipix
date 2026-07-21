//! Saved shows/movies for the Stream tab.
//!
//! A tiny table in the videos DB: enough to render the Saved page and reopen a
//! title. Cover is kept as its remote URL — the poster cache turns it into a
//! local image on display, same as search results.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS stream_bookmarks (
    subject_id  TEXT    PRIMARY KEY,
    title       TEXT    NOT NULL,
    year        TEXT    NOT NULL DEFAULT '',
    cover_url   TEXT    NOT NULL DEFAULT '',
    is_series   INTEGER NOT NULL DEFAULT 0,
    meta        TEXT    NOT NULL DEFAULT '',
    overview    TEXT    NOT NULL DEFAULT '',
    added_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS stream_bookmarks_added_idx ON stream_bookmarks(added_at);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    // Idempotent migrations for DBs created before the info fields existed —
    // SQLite errors (harmlessly) on a duplicate column.
    let _ = sqlx::query("ALTER TABLE stream_bookmarks ADD COLUMN meta TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE stream_bookmarks ADD COLUMN overview TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bookmark {
    pub subject_id: String,
    pub title: String,
    pub year: String,
    pub cover_url: String,
    pub is_series: bool,
    /// Pre-rendered meta line ("2024 · Drama · ★ 8.7") shown on the Saved card.
    pub meta: String,
    /// Synopsis, so the Saved card looks as filled as the hover preview.
    pub overview: String,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn is_saved(pool: &SqlitePool, subject_id: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT 1 FROM stream_bookmarks WHERE subject_id = ?")
        .bind(subject_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Add if absent, remove if present. Returns the new saved state (`true` = now
/// saved) so the caller can flip the button without a second query.
pub async fn toggle(pool: &SqlitePool, b: &Bookmark) -> Result<bool> {
    if b.subject_id.is_empty() {
        return Ok(false);
    }
    if is_saved(pool, &b.subject_id).await {
        sqlx::query("DELETE FROM stream_bookmarks WHERE subject_id = ?")
            .bind(&b.subject_id)
            .execute(pool)
            .await?;
        Ok(false)
    } else {
        sqlx::query(
            "INSERT OR REPLACE INTO stream_bookmarks
             (subject_id, title, year, cover_url, is_series, meta, overview, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&b.subject_id)
        .bind(&b.title)
        .bind(&b.year)
        .bind(&b.cover_url)
        .bind(b.is_series as i64)
        .bind(&b.meta)
        .bind(&b.overview)
        .bind(now_secs())
        .execute(pool)
        .await?;
        Ok(true)
    }
}

pub async fn remove(pool: &SqlitePool, subject_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM stream_bookmarks WHERE subject_id = ?")
        .bind(subject_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Newest first.
pub async fn list(pool: &SqlitePool) -> Vec<Bookmark> {
    let rows: Vec<(String, String, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT subject_id, title, year, cover_url, is_series, meta, overview
         FROM stream_bookmarks ORDER BY added_at DESC",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(subject_id, title, year, cover_url, is_series, meta, overview)| Bookmark {
            subject_id,
            title,
            year,
            cover_url,
            is_series: is_series != 0,
            meta,
            overview,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn mk(id: &str, title: &str) -> Bookmark {
        Bookmark {
            subject_id: id.into(),
            title: title.into(),
            year: "2024".into(),
            cover_url: "https://c/x.jpg".into(),
            is_series: true,
            meta: "2024 · Drama".into(),
            overview: "A show.".into(),
        }
    }

    #[tokio::test]
    async fn toggle_adds_then_removes() {
        let (_t, pool) = open_pool().await;
        assert!(!is_saved(&pool, "a").await);
        assert!(toggle(&pool, &mk("a", "Show")).await.unwrap()); // now saved
        assert!(is_saved(&pool, "a").await);
        assert!(!toggle(&pool, &mk("a", "Show")).await.unwrap()); // toggled off
        assert!(!is_saved(&pool, "a").await);
    }

    #[tokio::test]
    async fn list_is_newest_first_and_survives_removal() {
        let (_t, pool) = open_pool().await;
        toggle(&pool, &mk("a", "First")).await.unwrap();
        toggle(&pool, &mk("b", "Second")).await.unwrap();
        let l = list(&pool).await;
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].subject_id, "b"); // most recent first
        remove(&pool, "b").await.unwrap();
        let l = list(&pool).await;
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].subject_id, "a");
    }

    #[tokio::test]
    async fn empty_id_is_ignored() {
        let (_t, pool) = open_pool().await;
        assert!(!toggle(&pool, &mk("", "x")).await.unwrap());
        assert!(list(&pool).await.is_empty());
    }
}
