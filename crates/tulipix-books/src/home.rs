//! Book Home page data — hero "Continue Reading" card, Recently Added shelf,
//! In Progress shelf, and the stats strip (books / finished / hours read).

use crate::library::BookRow;
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default)]
pub struct HomeData {
    /// Most recently read, unfinished book (hero card). None = empty-state hero.
    pub continue_reading: Option<(BookRow, i64, i64)>, // (book, page, total_pages)
    pub recently_added: Vec<BookRow>,
    pub in_progress: Vec<BookRow>,
    pub stats: Stats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub total: i64,
    pub authors: i64,
    pub finished: i64,
    pub in_progress: i64,
    pub hours_read: f64,
}

const SHELF: usize = 12;

const ROW_SQL: &str =
    "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
            b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
            b.rating, b.summary, b.summary_fetched_at,
            COALESCE(p.percent, 0.0) AS percent,
            COALESCE(p.updated_at, 0) AS last_read
     FROM books b LEFT JOIN progress p ON p.book_id = b.id
     WHERE b.missing = 0";

pub async fn load(pool: &SqlitePool) -> Result<HomeData> {
    let hero: Option<BookRow> = sqlx::query_as(&format!(
        "{ROW_SQL} AND b.finished = 0 AND COALESCE(p.percent, 0) > 0
         ORDER BY last_read DESC LIMIT 1"
    ))
    .fetch_optional(pool)
    .await?;
    let continue_reading = match hero {
        Some(b) => {
            let pos = crate::progress::get(pool, b.id).await?.unwrap_or_default();
            Some((b, pos.page, pos.total_pages))
        }
        None => None,
    };

    let recently_added: Vec<BookRow> = sqlx::query_as(&format!(
        "{ROW_SQL} ORDER BY b.added_at DESC LIMIT {SHELF}"
    ))
    .fetch_all(pool)
    .await?;

    let in_progress: Vec<BookRow> = sqlx::query_as(&format!(
        "{ROW_SQL} AND b.finished = 0 AND COALESCE(p.percent, 0) > 0
         ORDER BY last_read DESC LIMIT {SHELF}"
    ))
    .fetch_all(pool)
    .await?;

    let (total, finished): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(finished), 0) FROM books WHERE missing = 0",
    )
    .fetch_one(pool)
    .await?;
    let authors: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT author) FROM books WHERE missing = 0 AND author != ''",
    )
    .fetch_one(pool)
    .await?;
    let secs: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(time_read_secs), 0) FROM progress")
            .fetch_one(pool)
            .await?;

    Ok(HomeData {
        continue_reading,
        recently_added,
        in_progress: in_progress.clone(),
        stats: Stats {
            total,
            authors,
            finished,
            in_progress: in_progress.len() as i64,
            hours_read: secs as f64 / 3600.0,
        },
    })
}
