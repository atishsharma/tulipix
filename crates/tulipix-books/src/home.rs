//! Book Home page data — hero "Continue Reading" card, Recently Added shelf,
//! In Progress shelf, and the stats strip (books / finished / hours read).

use crate::library::BookRow;
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default)]
pub struct HomeData {
    /// Most recently read, unfinished book (hero card). None = empty-state hero.
    pub continue_reading: Option<(BookRow, i64, i64)>, // (book, page, total_pages)
    /// Up to 5 most-recently-read in-progress books, for the hero slider.
    pub slider: Vec<(BookRow, i64, i64)>, // (book, page, total_pages)
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
    /// Books added since the start of the current month.
    pub added_month: i64,
    /// Authors whose first book landed this month.
    pub authors_month: i64,
}

/// Unix epoch (secs) of the first day of the current month (UTC) — civil date
/// from days, no chrono dependency.
fn month_start_epoch() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = now / 86400 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day0 = doy - (153 * mp + 2) / 5; // day-of-month minus one
    (now / 86400 - day0) * 86400
}

const SHELF: usize = 12;

const ROW_SQL: &str =
    "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
            b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
            b.rating, b.summary, b.summary_fetched_at,
            COALESCE(p.percent, 0.0) AS percent,
            COALESCE(p.time_read_secs, 0) AS time_read,
            COALESCE(p.updated_at, 0) AS last_read
     FROM books b LEFT JOIN progress p ON p.book_id = b.id
     WHERE b.missing = 0 AND b.trashed = 0";

pub async fn load(pool: &SqlitePool) -> Result<HomeData> {
    let recently_added: Vec<BookRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ROW_SQL} ORDER BY b.added_at DESC LIMIT {SHELF}"
    )))
    .fetch_all(pool)
    .await?;

    let in_progress: Vec<BookRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ROW_SQL} AND b.finished = 0 AND COALESCE(p.percent, 0) > 0
         ORDER BY last_read DESC LIMIT {SHELF}"
    )))
    .fetch_all(pool)
    .await?;

    // Slider: page/total for the up-to-6 most recent in-progress books, in one
    // query instead of a `progress::get` per book.
    let ids: Vec<i64> = in_progress.iter().take(6).map(|b| b.id).collect();
    let mut pos_by_id: std::collections::HashMap<i64, (i64, i64)> =
        std::collections::HashMap::new();
    if !ids.is_empty() {
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT book_id, page, total_pages FROM progress WHERE book_id IN ({list})"
        )))
        .fetch_all(pool)
        .await?;
        pos_by_id = rows.into_iter().map(|(id, p, t)| (id, (p, t))).collect();
    }
    let slider: Vec<(BookRow, i64, i64)> = in_progress
        .iter()
        .take(6)
        .map(|b| {
            let (page, total) = pos_by_id.get(&b.id).copied().unwrap_or((0, 0));
            (b.clone(), page, total)
        })
        .collect();
    // The hero card is the most-recently-read in-progress book — which is
    // exactly the first slider slide, so it needs no query of its own.
    let continue_reading = slider.first().cloned();

    let month_start = month_start_epoch();
    let (total, finished, authors, added_month): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*),
                COALESCE(SUM(finished), 0),
                COUNT(DISTINCT CASE WHEN author != '' THEN author END),
                COALESCE(SUM(added_at >= ?), 0)
         FROM books WHERE missing = 0 AND trashed = 0",
    )
    .bind(month_start)
    .fetch_one(pool)
    .await?;
    let secs: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(time_read_secs), 0) FROM progress")
            .fetch_one(pool)
            .await?;
    let authors_month: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT author FROM books
         WHERE missing = 0 AND trashed = 0 AND author != ''
         GROUP BY author HAVING MIN(added_at) >= ?)",
    )
    .bind(month_start)
    .fetch_one(pool)
    .await?;

    Ok(HomeData {
        continue_reading,
        slider,
        recently_added,
        in_progress: in_progress.clone(),
        stats: Stats {
            total,
            authors,
            finished,
            in_progress: in_progress.len() as i64,
            hours_read: secs as f64 / 3600.0,
            added_month,
            authors_month,
        },
    })
}
