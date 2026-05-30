//! Memories — On This Day rail + N years ago + trip clusters.
//!
//! "On This Day": every photo whose taken_at month/day matches today, across
//! all years. "X years ago" filters that further to a specific year delta.
//! "Trip clusters": agglomerate photos by spatial+temporal proximity —
//! photos within `TRIP_DAYS` of each other and within `TRIP_KM` of the
//! cluster centroid form a trip.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const TRIP_DAYS: i64 = 5;
pub const TRIP_KM: f64 = 50.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnThisDayHit {
    pub item_id: i64,
    pub abs_path: String,
    pub years_ago: i32,
    pub taken_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trip {
    pub start_unix: i64,
    pub end_unix: i64,
    pub lat: f64,
    pub lon: f64,
    pub items: Vec<i64>,
}

fn today_yyyy_mmdd(now: i64) -> (i32, u32, u32) {
    let (y, m) = crate::timeline::year_month(now);
    let days = now.div_euclid(86_400);
    let d = days_to_day_of_month(days, y, m);
    (y, m, d)
}

fn days_to_day_of_month(days: i64, year: i32, month: u32) -> u32 {
    // Reverse of timeline::unix_at — derive day-of-month from the first-of-month base.
    let base_unix = first_of_month_unix(year, month);
    let base_days = base_unix / 86_400;
    ((days - base_days) + 1).max(1) as u32
}

fn first_of_month_unix(year: i32, month: u32) -> i64 {
    let y = if month <= 2 { (year - 1) as i64 } else { year as i64 };
    let m = month as i64;
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400
}

pub async fn on_this_day(pool: &SqlitePool, now_unix: i64) -> Result<Vec<OnThisDayHit>> {
    let (today_y, today_m, today_d) = today_yyyy_mmdd(now_unix);
    let pattern = format!("%-{:02}-{:02} %", today_m, today_d);
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.taken_at
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.missing_since IS NULL
           AND photo_meta.deleted_at IS NULL
           AND photo_meta.archived = 0
           AND photo_meta.taken_at IS NOT NULL
           AND strftime('%m-%d', datetime(photo_meta.taken_at, 'unixepoch')) = ?
         ORDER BY photo_meta.taken_at DESC",
    )
    .bind(format!("{:02}-{:02}", today_m, today_d))
    .fetch_all(pool).await?;
    let _ = pattern;
    Ok(rows.into_iter().map(|(item_id, abs_path, taken_at)| {
        let (y, _) = crate::timeline::year_month(taken_at);
        OnThisDayHit { item_id, abs_path, taken_at, years_ago: today_y - y }
    }).collect())
}

pub async fn n_years_ago(pool: &SqlitePool, now_unix: i64, n: i32) -> Result<Vec<OnThisDayHit>> {
    let hits = on_this_day(pool, now_unix).await?;
    Ok(hits.into_iter().filter(|h| h.years_ago == n).collect())
}

/// Great-circle distance, km, between two (lat, lon) pairs.
pub fn haversine_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6371.0;
    let (lat1, lat2) = (a.0.to_radians(), b.0.to_radians());
    let dlat = (b.0 - a.0).to_radians();
    let dlon = (b.1 - a.1).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

pub async fn trips(pool: &SqlitePool) -> Result<Vec<Trip>> {
    let rows: Vec<(i64, i64, f64, f64)> = sqlx::query_as(
        "SELECT items.id, photo_meta.taken_at, photo_meta.gps_lat, photo_meta.gps_lon
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.taken_at IS NOT NULL
           AND photo_meta.gps_lat IS NOT NULL
           AND photo_meta.gps_lon IS NOT NULL
           AND items.missing_since IS NULL
         ORDER BY photo_meta.taken_at"
    ).fetch_all(pool).await?;

    let mut clusters: Vec<Trip> = Vec::new();
    for (id, taken, lat, lon) in rows {
        let matched = clusters.iter_mut().find(|t| {
            let time_gap = (taken - t.end_unix).abs();
            let in_time = time_gap <= TRIP_DAYS * 86_400;
            let in_space = haversine_km((t.lat, t.lon), (lat, lon)) <= TRIP_KM;
            in_time && in_space
        });
        match matched {
            Some(t) => {
                let n = t.items.len() as f64;
                t.lat = (t.lat * n + lat) / (n + 1.0);
                t.lon = (t.lon * n + lon) / (n + 1.0);
                t.end_unix = taken.max(t.end_unix);
                t.start_unix = taken.min(t.start_unix);
                t.items.push(id);
            }
            None => {
                clusters.push(Trip { start_unix: taken, end_unix: taken, lat, lon, items: vec![id] });
            }
        }
    }
    // Trips need at least 3 photos to count — drops random one-off geo-tagged shots.
    clusters.retain(|t| t.items.len() >= 3);
    Ok(clusters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add(pool: &SqlitePool, path: &str, taken: i64, lat: Option<f64>, lon: Option<f64>) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, taken_at, gps_lat, gps_lon) VALUES (?, ?, ?, ?)")
            .bind(id).bind(taken).bind(lat).bind(lon).execute(pool).await.unwrap();
        id
    }

    #[test]
    fn haversine_known_distance() {
        // London ↔ Paris ≈ 343 km
        let d = haversine_km((51.5074, -0.1278), (48.8566, 2.3522));
        assert!((d - 343.0).abs() < 5.0);
    }

    #[tokio::test]
    async fn trips_group_nearby_dates_and_drop_singletons() {
        let (_t, pool) = open_pool().await;
        // Three photos in Rome over 3 days
        let base = 1_700_000_000;
        for i in 0..3 { add(&pool, &format!("/rome/{i}.jpg"), base + i * 86400, Some(41.9), Some(12.5)).await; }
        // One photo in Tokyo
        add(&pool, "/tokyo.jpg", base + 200 * 86400, Some(35.6), Some(139.7)).await;
        let ts = trips(&pool).await.unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].items.len(), 3);
    }
}
