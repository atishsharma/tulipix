//! Batch date correction for photos.
//!
//! Three input modes:
//!  * `SetAbsolute(unix)` — overwrite `DateTimeOriginal` on every item with the
//!    same Unix timestamp.
//!  * `Shift(seconds)`     — add a signed offset to each item's current taken_at.
//!  * `MatchNeighbour`     — borrow `taken_at` from the nearest sibling that
//!    has a date set, picking the previous one when both sides exist.
//!
//! `plan()` walks the selection and produces a preview list of (item, before,
//! after) without touching anything. `apply()` writes EXIF in-place via the
//! bundled exiftool and reconciles `photo_meta.taken_at`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

use crate::exif_write::{apply as exif_apply, ExifPatch};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DateFixOp {
    SetAbsolute { unix: i64 },
    Shift { seconds: i64 },
    MatchNeighbour,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateChange {
    pub item_id: i64,
    pub abs_path: String,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

impl DateChange {
    pub fn changed(&self) -> bool { self.before != self.after && self.after.is_some() }
}

/// Build the preview list without writing anything.
pub async fn plan(pool: &SqlitePool, ids: &[i64], op: DateFixOp) -> Result<Vec<DateChange>> {
    let rows = fetch_rows(pool, ids).await?;
    let mut out: Vec<DateChange> = Vec::with_capacity(rows.len());

    match op {
        DateFixOp::SetAbsolute { unix } => {
            for (id, path, before) in rows {
                out.push(DateChange { item_id: id, abs_path: path, before, after: Some(unix) });
            }
        }
        DateFixOp::Shift { seconds } => {
            for (id, path, before) in rows {
                let after = before.map(|b| b + seconds);
                out.push(DateChange { item_id: id, abs_path: path, before, after });
            }
        }
        DateFixOp::MatchNeighbour => {
            // Pull every dated photo once; then for each undated id, pick the
            // sibling with the closest taken_at by id ordering (proxy for
            // capture order when timestamps are missing).
            let all: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT photo_meta.item_id, photo_meta.taken_at
                 FROM photo_meta WHERE photo_meta.taken_at IS NOT NULL
                 ORDER BY photo_meta.item_id",
            ).fetch_all(pool).await?;
            for (id, path, before) in rows {
                let after = nearest_neighbour(&all, id);
                out.push(DateChange { item_id: id, abs_path: path, before, after });
            }
        }
    }
    Ok(out)
}

fn nearest_neighbour(dated: &[(i64, i64)], id: i64) -> Option<i64> {
    let pos = dated.partition_point(|(i, _)| *i < id);
    let prev = if pos == 0 { None } else { dated.get(pos - 1) };
    let next = dated.get(pos);
    match (prev, next) {
        (Some((_, t)), _) => Some(*t),
        (None, Some((_, t))) => Some(*t),
        _ => None,
    }
}

async fn fetch_rows(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<(i64, String, Option<i64>)>> {
    if ids.is_empty() { return Ok(Vec::new()); }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT items.id, items.abs_path, photo_meta.taken_at
         FROM items LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.id IN ({placeholders}) AND items.section = 'photos'",
    );
    let mut q = sqlx::query_as::<_, (i64, String, Option<i64>)>(sqlx::AssertSqlSafe(&*sql));
    for id in ids { q = q.bind(*id); }
    Ok(q.fetch_all(pool).await?)
}

/// Apply the changes returned by `plan()`. Each row writes
/// `DateTimeOriginal`, `CreateDate` and `ModifyDate` via exiftool, then
/// updates `photo_meta.taken_at`. Returns the number of files updated.
pub async fn apply(pool: &SqlitePool, changes: &[DateChange]) -> Result<u64> {
    let mut wrote = 0u64;
    for ch in changes {
        if !ch.changed() { continue; }
        let Some(after) = ch.after else { continue; };
        let formatted = format_exif_datetime(after);
        let patch = ExifPatch::new()
            .set("DateTimeOriginal", &formatted)
            .set("CreateDate", &formatted)
            .set("ModifyDate", &formatted);
        if let Err(e) = exif_apply(Path::new(&ch.abs_path), &patch) {
            tracing::warn!(item = ch.item_id, "exiftool failed: {e}");
            continue;
        }
        sqlx::query("UPDATE photo_meta SET taken_at = ? WHERE item_id = ?")
            .bind(after).bind(ch.item_id)
            .execute(pool).await?;
        wrote += 1;
    }
    Ok(wrote)
}

/// `unix` → `YYYY:MM:DD HH:MM:SS` (the format EXIF actually wants).
pub fn format_exif_datetime(unix: i64) -> String {
    // Reuse the inverse of the tiny calendar in exif.rs.
    let days = unix.div_euclid(86_400);
    let rem  = unix.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hour = rem / 3600;
    let min  = (rem % 3600) / 60;
    let sec  = rem % 60;
    format!("{:04}:{:02}:{:02} {:02}:{:02}:{:02}", y, m, d, hour, min, sec)
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z / 146_097 } else { (z - 146_096) / 146_097 };
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let m   = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn seed(pool: &SqlitePool, path: &str, taken: Option<i64>) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        if let Some(t) = taken {
            sqlx::query("INSERT INTO photo_meta (item_id, taken_at) VALUES (?, ?)")
                .bind(id).bind(t).execute(pool).await.unwrap();
        } else {
            sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)")
                .bind(id).execute(pool).await.unwrap();
        }
        id
    }

    #[test]
    fn format_round_trips_known_dates() {
        // 2024-02-29 12:34:56 UTC
        let unix = 1_709_210_096;
        assert_eq!(format_exif_datetime(unix), "2024:02:29 12:34:56");
        // epoch
        assert_eq!(format_exif_datetime(0), "1970:01:01 00:00:00");
    }

    #[tokio::test]
    async fn plan_set_absolute_overwrites_all() {
        let (_t, pool) = open_pool().await;
        let a = seed(&pool, "/a.jpg", Some(1000)).await;
        let b = seed(&pool, "/b.jpg", None).await;
        let p = plan(&pool, &[a, b], DateFixOp::SetAbsolute { unix: 5000 }).await.unwrap();
        assert_eq!(p.len(), 2);
        for ch in &p { assert_eq!(ch.after, Some(5000)); }
        assert!(p[0].changed());
        assert!(p[1].changed());
    }

    #[tokio::test]
    async fn plan_shift_skips_undated() {
        let (_t, pool) = open_pool().await;
        let a = seed(&pool, "/a.jpg", Some(1000)).await;
        let b = seed(&pool, "/b.jpg", None).await;
        let p = plan(&pool, &[a, b], DateFixOp::Shift { seconds: 60 }).await.unwrap();
        let map: std::collections::HashMap<i64, Option<i64>> =
            p.into_iter().map(|c| (c.item_id, c.after)).collect();
        assert_eq!(map[&a], Some(1060));
        assert_eq!(map[&b], None);
    }

    #[tokio::test]
    async fn plan_match_neighbour_borrows_from_dated_sibling() {
        let (_t, pool) = open_pool().await;
        let a = seed(&pool, "/a.jpg", Some(1000)).await;
        let b = seed(&pool, "/b.jpg", None).await; // undated
        let _c = seed(&pool, "/c.jpg", Some(3000)).await;
        let p = plan(&pool, &[b], DateFixOp::MatchNeighbour).await.unwrap();
        assert_eq!(p[0].after, Some(1000)); // picks the previous-id neighbour `a`
        // and the borrowed value differs from before (None)
        assert!(p[0].changed());
        // `a` is the closest by id; sanity-check via the helper
        let dated = vec![(a, 1000), (_c, 3000)];
        assert_eq!(nearest_neighbour(&dated, b), Some(1000));
    }
}
