//! Native map widget data layer — MBTiles reader + bounds queries.
//!
//! MBTiles is a single-file SQLite container of XYZ tiles (TMS scheme: y is
//! flipped). This module:
//!   * `open_mbtiles` — opens an MBTiles file as a sqlx pool.
//!   * `tile`        — fetches one PNG/JPEG tile blob.
//!   * `bbox_for_photos` — returns the lat/lon bbox enclosing every GPS
//!     point in the photos DB (the initial viewport for the map view).
//!   * `cluster_pins` — buckets photos into screen-space clusters for the
//!     "1042 photos here" pin pattern.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BBox {
    pub min_lat: f64, pub min_lon: f64,
    pub max_lat: f64, pub max_lon: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinCluster {
    pub lat: f64,
    pub lon: f64,
    pub count: u32,
    pub cover_item_id: i64,
}

pub async fn open_mbtiles(path: &Path) -> Result<SqlitePool> {
    let url = format!("sqlite://{}?mode=ro", path.display());
    let opts = SqliteConnectOptions::from_str(&url)?.read_only(true)
        // Map tiles are read-heavy blobs — memory-map + a larger page cache cut
        // the per-tile syscall/copy overhead while panning/zooming.
        .pragma("mmap_size", "268435456")
        .pragma("cache_size", "-16000");
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts).await
        .with_context(|| format!("open mbtiles {}", path.display()))?;
    Ok(pool)
}

/// Returns the tile blob for (z, x, y) in XYZ scheme. MBTiles stores y in
/// TMS (flipped) so we flip on read.
pub async fn tile(pool: &SqlitePool, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
    let tms_y = (1u32 << z).saturating_sub(1).saturating_sub(y);
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT tile_data FROM tiles WHERE zoom_level = ? AND tile_column = ? AND tile_row = ?",
    )
    .bind(z as i64).bind(x as i64).bind(tms_y as i64)
    .fetch_optional(pool).await?;
    Ok(row.map(|(b,)| b))
}

pub async fn bbox_for_photos(photos_pool: &SqlitePool) -> Result<Option<BBox>> {
    let row: Option<(Option<f64>, Option<f64>, Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT MIN(photo_meta.gps_lat), MIN(photo_meta.gps_lon),
                MAX(photo_meta.gps_lat), MAX(photo_meta.gps_lon)
         FROM photo_meta JOIN items ON items.id = photo_meta.item_id
         WHERE photo_meta.gps_lat IS NOT NULL
           AND photo_meta.gps_lon IS NOT NULL
           AND items.missing_since IS NULL",
    ).fetch_optional(photos_pool).await?;
    match row {
        Some((Some(min_lat), Some(min_lon), Some(max_lat), Some(max_lon))) =>
            Ok(Some(BBox { min_lat, min_lon, max_lat, max_lon })),
        _ => Ok(None),
    }
}

/// Grid cluster — `precision` is the rounding factor in degrees (0.1 ≈
/// 11 km bins at the equator). Lower precision = bigger clusters.
pub async fn cluster_pins(photos_pool: &SqlitePool, precision: f64) -> Result<Vec<PinCluster>> {
    let p = precision.max(0.001);
    let rows: Vec<(i64, f64, f64)> = sqlx::query_as(
        "SELECT items.id, photo_meta.gps_lat, photo_meta.gps_lon
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.gps_lat IS NOT NULL
           AND photo_meta.gps_lon IS NOT NULL
           AND items.missing_since IS NULL",
    ).fetch_all(photos_pool).await?;
    let mut buckets: std::collections::BTreeMap<(i64, i64), PinCluster> = std::collections::BTreeMap::new();
    for (id, lat, lon) in rows {
        let k = ((lat / p).round() as i64, (lon / p).round() as i64);
        let entry = buckets.entry(k).or_insert(PinCluster {
            lat: 0.0, lon: 0.0, count: 0, cover_item_id: id,
        });
        // running average for centroid
        let n = entry.count as f64;
        entry.lat = (entry.lat * n + lat) / (n + 1.0);
        entry.lon = (entry.lon * n + lon) / (n + 1.0);
        entry.count += 1;
    }
    Ok(buckets.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn bbox_returns_none_when_empty() {
        let (_t, pool) = open_pool().await;
        let b = bbox_for_photos(&pool).await.unwrap();
        assert!(b.is_none());
    }

    #[tokio::test]
    async fn cluster_groups_by_grid() {
        let (_t, pool) = open_pool().await;
        for (p, lat, lon) in [("/a.jpg", 41.9001, 12.5001), ("/b.jpg", 41.9002, 12.5002), ("/c.jpg", 35.7, 139.7)] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(p).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO photo_meta (item_id, gps_lat, gps_lon) VALUES (?, ?, ?)")
                .bind(id).bind(lat).bind(lon).execute(&pool).await.unwrap();
        }
        let pins = cluster_pins(&pool, 0.5).await.unwrap();
        assert_eq!(pins.len(), 2);
        let total: u32 = pins.iter().map(|p| p.count).sum();
        assert_eq!(total, 3);
    }
}
