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
    seen_max_ep    INTEGER NOT NULL DEFAULT 0,
    latest_max_ep  INTEGER NOT NULL DEFAULT 0,
    checked_at     INTEGER NOT NULL DEFAULT 0,
    added_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS stream_bookmarks_added_idx ON stream_bookmarks(added_at);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    // Idempotent migrations for DBs created before these fields existed —
    // SQLite errors (harmlessly) on a duplicate column.
    for ddl in [
        "ALTER TABLE stream_bookmarks ADD COLUMN meta TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE stream_bookmarks ADD COLUMN overview TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE stream_bookmarks ADD COLUMN seen_max_ep INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE stream_bookmarks ADD COLUMN latest_max_ep INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE stream_bookmarks ADD COLUMN checked_at INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = sqlx::query(ddl).execute(pool).await;
    }
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
    /// Episode count the user has already seen listed for this show.
    pub seen_max_ep: i64,
    /// Episode count the last refresh found. Ahead of `seen_max_ep` means the
    /// show has put out something new since the card was last opened.
    pub latest_max_ep: i64,
}

impl Bookmark {
    /// Should the Saved card wear a "new episodes" badge?
    pub fn has_new(&self) -> bool {
        self.is_series && self.latest_max_ep > self.seen_max_ep
    }

    /// How many new episodes, for the badge text.
    pub fn new_count(&self) -> i64 {
        (self.latest_max_ep - self.seen_max_ep).max(0)
    }
}

use tulipix_core::util::unix_secs_i64 as now_secs;

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
             (subject_id, title, year, cover_url, is_series, meta, overview,
              seen_max_ep, latest_max_ep, checked_at, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&b.subject_id)
        .bind(&b.title)
        .bind(&b.year)
        .bind(&b.cover_url)
        .bind(b.is_series as i64)
        .bind(&b.meta)
        .bind(&b.overview)
        // Saving a show now means everything it lists today is "already seen";
        // the badge is for what turns up afterwards.
        .bind(b.seen_max_ep)
        .bind(b.seen_max_ep)
        .bind(now_secs())
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

type Row = (String, String, String, String, i64, String, String, i64, i64);

const COLUMNS: &str = "subject_id, title, year, cover_url, is_series, meta, overview, \
                       seen_max_ep, latest_max_ep";

fn bookmark_of(r: Row) -> Bookmark {
    Bookmark {
        subject_id: r.0,
        title: r.1,
        year: r.2,
        cover_url: r.3,
        is_series: r.4 != 0,
        meta: r.5,
        overview: r.6,
        seen_max_ep: r.7,
        latest_max_ep: r.8,
    }
}

/// Newest first.
pub async fn list(pool: &SqlitePool) -> Vec<Bookmark> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_bookmarks ORDER BY added_at DESC"
    ))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(bookmark_of).collect()
}

/// Saved series that have not been checked for new episodes recently.
pub async fn due_for_check(pool: &SqlitePool, older_than_secs: i64) -> Vec<Bookmark> {
    // A negative age means "everything is due" — the refresh-now path, and what
    // the tests use to avoid waiting out a real interval.
    let cutoff = now_secs().saturating_sub(older_than_secs);
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_bookmarks
         WHERE is_series = 1 AND checked_at < ? ORDER BY added_at DESC LIMIT 40"
    ))
    .bind(cutoff)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(bookmark_of).collect()
}

/// Record what a refresh found. Returns true when this is news.
pub async fn note_latest(pool: &SqlitePool, subject_id: &str, latest_max_ep: i64) -> Result<bool> {
    sqlx::query(
        "UPDATE stream_bookmarks SET latest_max_ep = ?, checked_at = ? WHERE subject_id = ?",
    )
    .bind(latest_max_ep.max(0))
    .bind(now_secs())
    .bind(subject_id)
    .execute(pool)
    .await?;
    let seen: Option<i64> =
        sqlx::query_scalar("SELECT seen_max_ep FROM stream_bookmarks WHERE subject_id = ?")
            .bind(subject_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    Ok(seen.is_some_and(|s| latest_max_ep > s))
}

/// The user has looked at the show — clear its badge.
pub async fn mark_seen(pool: &SqlitePool, subject_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE stream_bookmarks SET seen_max_ep = MAX(seen_max_ep, latest_max_ep)
         WHERE subject_id = ?",
    )
    .bind(subject_id)
    .execute(pool)
    .await?;
    Ok(())
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
            seen_max_ep: 8,
            latest_max_ep: 8,
        }
    }

    #[tokio::test]
    async fn a_new_episode_raises_a_badge_until_the_show_is_opened() {
        let (_t, pool) = open_pool().await;
        toggle(&pool, &mk("a", "Severance")).await.unwrap();
        assert!(!list(&pool).await[0].has_new(), "saving is not news");

        // Same count on a later check is still not news.
        assert!(!note_latest(&pool, "a", 8).await.unwrap());
        assert!(!list(&pool).await[0].has_new());

        assert!(note_latest(&pool, "a", 10).await.unwrap(), "two more episodes");
        let saved = &list(&pool).await[0];
        assert!(saved.has_new());
        assert_eq!(saved.new_count(), 2);

        mark_seen(&pool, "a").await.unwrap();
        assert!(!list(&pool).await[0].has_new(), "opening the show clears it");

        // An unknown id must not error or invent a row.
        assert!(!note_latest(&pool, "nope", 99).await.unwrap());
    }

    #[tokio::test]
    async fn due_for_check_only_returns_stale_series() {
        let (_t, pool) = open_pool().await;
        toggle(&pool, &mk("a", "Show")).await.unwrap();
        let movie = Bookmark { is_series: false, ..mk("b", "Film") };
        toggle(&pool, &movie).await.unwrap();

        // Just checked, so nothing is due yet.
        assert!(due_for_check(&pool, 3600).await.is_empty());
        // Everything checked before "now" is due.
        let due = due_for_check(&pool, -1).await;
        assert_eq!(due.len(), 1, "movies never get episode checks");
        assert_eq!(due[0].subject_id, "a");
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
