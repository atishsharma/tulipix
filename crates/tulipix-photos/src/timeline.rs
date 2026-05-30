//! Virtualised timeline data source.
//!
//! Drives the Slint `ListView` that paints the month/year-grouped grid. UI
//! lives in `ui/main.slint`; this module just produces the rows the model
//! reads in. Pagination is by month bucket so even libraries with hundreds of
//! thousands of items can scroll without preloading everything.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupHeader {
    pub year: i32,
    pub month: u32,           // 1..12; 0 = undated bucket
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineRow {
    pub item_id: i64,
    pub abs_path: String,
    pub taken_at: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub starred: bool,
}

/// List all (year, month) buckets present in the library, newest first.
/// `month = 0` is the "no EXIF date" bucket — items fall into it whenever
/// `taken_at` is NULL.
pub async fn months(pool: &SqlitePool) -> Result<Vec<GroupHeader>> {
    let rows: Vec<(Option<i64>, i64)> = sqlx::query_as(
        "SELECT photo_meta.taken_at, COUNT(*) AS c
         FROM items
         JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.missing_since IS NULL
           AND photo_meta.deleted_at IS NULL
           AND photo_meta.archived = 0
         GROUP BY (taken_at IS NULL),
                  strftime('%Y-%m', datetime(taken_at, 'unixepoch'))
         ORDER BY taken_at DESC NULLS LAST",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (taken, count) in rows {
        match taken {
            None => out.push(GroupHeader { year: 0, month: 0, count }),
            Some(t) => {
                let (y, m) = year_month(t);
                out.push(GroupHeader { year: y, month: m, count });
            }
        }
    }
    Ok(out)
}

/// Page of rows inside one month bucket. `month == 0` returns the undated
/// bucket. Newest first inside the month.
pub async fn page(
    pool: &SqlitePool,
    year: i32,
    month: u32,
    offset: i64,
    limit: i64,
) -> Result<Vec<TimelineRow>> {
    let rows: Vec<(i64, String, Option<i64>, Option<i64>, Option<i64>, i64)> =
        if month == 0 {
            sqlx::query_as(
                "SELECT items.id, items.abs_path, photo_meta.taken_at,
                        photo_meta.width, photo_meta.height, photo_meta.starred
                 FROM items
                 JOIN photo_meta ON photo_meta.item_id = items.id
                 WHERE items.missing_since IS NULL
                   AND photo_meta.taken_at IS NULL
                   AND photo_meta.deleted_at IS NULL
                   AND photo_meta.archived = 0
                 ORDER BY items.added DESC
                 LIMIT ? OFFSET ?",
            )
            .bind(limit).bind(offset)
            .fetch_all(pool).await?
        } else {
            let (start, end) = month_bounds(year, month);
            sqlx::query_as(
                "SELECT items.id, items.abs_path, photo_meta.taken_at,
                        photo_meta.width, photo_meta.height, photo_meta.starred
                 FROM items
                 JOIN photo_meta ON photo_meta.item_id = items.id
                 WHERE items.missing_since IS NULL
                   AND photo_meta.deleted_at IS NULL
                   AND photo_meta.archived = 0
                   AND photo_meta.taken_at >= ? AND photo_meta.taken_at < ?
                 ORDER BY photo_meta.taken_at DESC
                 LIMIT ? OFFSET ?",
            )
            .bind(start).bind(end).bind(limit).bind(offset)
            .fetch_all(pool).await?
        };
    Ok(rows
        .into_iter()
        .map(|(id, path, taken, w, h, starred)| TimelineRow {
            item_id: id,
            abs_path: path,
            taken_at: taken,
            width: w,
            height: h,
            starred: starred != 0,
        })
        .collect())
}

pub(crate) fn year_month(unix: i64) -> (i32, u32) {
    let days = unix.div_euclid(86_400);
    // Inverse of days_from_civil — Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let _ = d;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32)
}

fn month_bounds(year: i32, month: u32) -> (i64, i64) {
    let start = unix_at(year, month, 1);
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let end = unix_at(ny, nm, 1);
    (start, end)
}

fn unix_at(y: i32, m: u32, d: u32) -> i64 {
    let yi = y as i64;
    let mi = m as i64;
    let di = d as i64;
    let y = if mi <= 2 { yi - 1 } else { yi };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (if mi > 2 { mi - 3 } else { mi + 9 }) + 2) / 5 + di - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn empty_db_has_no_months() {
        let (_t, pool) = open_pool().await;
        let m = months(&pool).await.unwrap();
        assert!(m.is_empty());
    }

    #[tokio::test]
    async fn buckets_split_by_month_descending() {
        let (_t, pool) = open_pool().await;
        // 2024-06-15 and 2023-12-01
        let t1 = unix_at(2024, 6, 15);
        let t2 = unix_at(2023, 12, 1);
        for (i, taken) in [t1, t2, t1].iter().enumerate() {
            let path = format!("/p/{i}.jpg");
            sqlx::query(
                "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)",
            ).bind(&path).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
                .bind(&path).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO photo_meta (item_id, taken_at) VALUES (?, ?)")
                .bind(id).bind(taken).execute(&pool).await.unwrap();
        }
        let m = months(&pool).await.unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].year, 2024);
        assert_eq!(m[0].month, 6);
        assert_eq!(m[0].count, 2);
        assert_eq!(m[1].year, 2023);
        assert_eq!(m[1].month, 12);

        let p = page(&pool, 2024, 6, 0, 10).await.unwrap();
        assert_eq!(p.len(), 2);
    }
}
