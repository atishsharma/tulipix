//! Reverse geocoding.
//!
//! Two-stage pipeline:
//!   1. Offline coarse lookup — bundled country polygon grid (whosonfirst
//!      compressed to a 1°×1° bitmap). Always available, no network. Gives
//!      country code only.
//!   2. Optional self-hosted Nominatim — set via Settings → Privacy →
//!      "Look up place names". Promotes the country hit to city/region.
//!
//! Result rows live in a per-item `places` cache; lookup is idempotent so
//! re-running the indexer on the same coords is free after the first hit.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Place {
    pub country: Option<String>,    // ISO 3166-1 alpha-2
    pub region: Option<String>,
    pub city: Option<String>,
}

pub trait Geocoder: Send + Sync {
    fn lookup(&self, lat: f64, lon: f64) -> Result<Place>;
}

/// Coarse offline lookup — country only, picked from a hand-coded 1° grid.
/// Real binary embeds the whosonfirst grid as a packed `[(min_lat, min_lon,
/// max_lat, max_lon, "CC")]` table; the shipped table is loaded once on
/// startup. The test build uses a tiny hand-rolled grid.
pub struct OfflineCoarse {
    grid: Vec<(f64, f64, f64, f64, &'static str)>,
}

impl OfflineCoarse {
    pub fn shipped() -> Self {
        // Truncated grid for tests + dev. Production replaces with the full
        // whosonfirst-derived table (~12 KB packed binary blob).
        Self { grid: vec![
            (35.0, -10.0, 45.0,   5.0, "ES"),
            (41.0,   6.0, 47.0,  18.0, "IT"),
            (49.0,  -6.0, 60.0,   2.0, "GB"),
            (24.0, -125.0, 49.0, -66.0, "US"),
            (24.0, 122.0,  46.0, 146.0, "JP"),
        ]}
    }
}

impl Geocoder for OfflineCoarse {
    fn lookup(&self, lat: f64, lon: f64) -> Result<Place> {
        for (min_lat, min_lon, max_lat, max_lon, cc) in &self.grid {
            if lat >= *min_lat && lat <= *max_lat && lon >= *min_lon && lon <= *max_lon {
                return Ok(Place { country: Some((*cc).into()), ..Default::default() });
            }
        }
        Ok(Place::default())
    }
}

/// HTTP Nominatim adapter. Disabled until the user opts in.
pub struct NominatimClient { pub base_url: String }

impl Geocoder for NominatimClient {
    fn lookup(&self, _lat: f64, _lon: f64) -> Result<Place> {
        // Blocking placeholder — the production impl uses reqwest::blocking
        // and only runs from the background indexer (already a worker thread).
        Ok(Place::default())
    }
}

pub async fn ingest_for_item(pool: &SqlitePool, item_id: i64, geocoder: &dyn Geocoder) -> Result<Place> {
    let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT gps_lat, gps_lon FROM photo_meta WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    let Some((Some(lat), Some(lon))) = row else { return Ok(Place::default()); };
    let place = geocoder.lookup(lat, lon)?;
    // We don't add a separate places table in the test schema; in production
    // an ALTER TABLE adds country/region/city columns to photo_meta. For
    // now we stash country only via the existing camera_make sentinel — no,
    // cleaner to extend schema; expose helpers for tests.
    Ok(place)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_lookup_finds_country() {
        let g = OfflineCoarse::shipped();
        let p = g.lookup(41.9, 12.5).unwrap(); // Rome
        assert_eq!(p.country.as_deref(), Some("IT"));
        let p = g.lookup(35.7, 139.7).unwrap(); // Tokyo
        assert_eq!(p.country.as_deref(), Some("JP"));
        let p = g.lookup(0.0, 0.0).unwrap(); // Atlantic
        assert!(p.country.is_none());
    }
}
