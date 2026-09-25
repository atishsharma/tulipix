//! The Places section's store, and finding trips in the photos.
//!
//! A trip is photos taken far from home, a few days apart at most, enough of
//! them to be more than a day out. Home is where most photos were taken,
//! unless somebody says otherwise. Names — towns, states, countries — come
//! from the GeoNames table Journal downloads (`journal::download_place_names`),
//! which also keeps each town's country and state for Places.

use std::collections::HashMap;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::journal::{Town, km};

const SCHEMA: &str = r#"
-- Kept trips (state 1) and dismissed suggestions (state 0), by their times.
CREATE TABLE IF NOT EXISTS trips (
    id      INTEGER PRIMARY KEY,
    title   TEXT    NOT NULL DEFAULT '',
    start   INTEGER NOT NULL,
    end     INTEGER NOT NULL,
    state   INTEGER NOT NULL DEFAULT 1,
    note    TEXT    NOT NULL DEFAULT '',
    created INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS wishes (
    id      INTEGER PRIMARY KEY,
    name    TEXT    NOT NULL,
    note    TEXT    NOT NULL DEFAULT '',
    created INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// One photo with a location.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub id: i64,
    pub ts: i64,
    pub lat: f64,
    pub lon: f64,
}

impl Pt {
    pub fn at(&self) -> (f64, f64) {
        (self.lat, self.lon)
    }
}

/// A 0.25° cell, about 25 km: close enough to call one place.
fn cell(p: (f64, f64), size: f64) -> (i64, i64) {
    ((p.0 / size).floor() as i64, (p.1 / size).floor() as i64)
}

/// Where most photos were taken — the middle of the busiest cell.
pub fn home_of(pts: &[Pt]) -> Option<(f64, f64)> {
    let mut by: HashMap<(i64, i64), (usize, f64, f64)> = HashMap::new();
    for p in pts {
        let e = by.entry(cell(p.at(), 0.25)).or_default();
        e.0 += 1;
        e.1 += p.lat;
        e.2 += p.lon;
    }
    by.into_values().max_by_key(|e| e.0).map(|(n, la, lo)| (la / n as f64, lo / n as f64))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    /// Further than this from home is away.
    pub min_km: f64,
    /// A longer gap between photos away ends the trip.
    pub gap_days: i64,
    /// Fewer photos than this is not a trip.
    pub min_photos: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { min_km: 80.0, gap_days: 2, min_photos: 15 }
    }
}

/// A run of photos away from home: its first and last photo's times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    pub start: i64,
    pub end: i64,
    pub photos: usize,
}

/// Trips in photos sorted by time. A photo at home ends the trip it follows;
/// so does a gap longer than the rules allow.
pub fn find_trips(pts: &[Pt], home: (f64, f64), rules: Rules) -> Vec<Found> {
    let mut out = Vec::new();
    let mut cur: Option<Found> = None;
    let gap = rules.gap_days * 86_400;
    let close = |cur: &mut Option<Found>, out: &mut Vec<Found>| {
        if let Some(f) = cur.take()
            && f.photos >= rules.min_photos
        {
            out.push(f);
        }
    };
    for p in pts {
        if km(home, p.at()) <= rules.min_km {
            close(&mut cur, &mut out);
            continue;
        }
        match cur.as_mut() {
            Some(f) if p.ts - f.end <= gap => {
                f.end = p.ts;
                f.photos += 1;
            }
            _ => {
                close(&mut cur, &mut out);
                cur = Some(Found { start: p.ts, end: p.ts, photos: 1 });
            }
        }
    }
    close(&mut cur, &mut out);
    out
}

/// The nearest town within 25 km, as an index into `towns`.
pub fn town_index(towns: &[Town], p: (f64, f64)) -> Option<usize> {
    towns
        .iter()
        .enumerate()
        .filter(|(_, t)| (t.lat - p.0).abs() < 0.5)
        .map(|(i, t)| (km((t.lat, t.lon), p), i))
        .filter(|(d, _)| *d <= 25.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i)| i)
}

/// The nearest town within 25 km.
pub fn town_at(towns: &[Town], p: (f64, f64)) -> Option<&Town> {
    town_index(towns, p).map(|i| &towns[i])
}

/// Where the camera stopped, in order: a new stop each time it moved more than
/// 15 km from the last one. The trip's route, at the scale a map shows it.
pub fn route(pts: &[Pt]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for p in pts {
        if out.last().is_none_or(|&l| km(l, p.at()) > 15.0) {
            out.push(p.at());
        }
    }
    out
}

pub fn route_km(stops: &[(f64, f64)]) -> f64 {
    stops.windows(2).map(|w| km(w[0], w[1])).sum()
}

/// Photos with a location, bucketed into cells of `size` degrees:
/// (latitude, longitude, photos) at each cell's mean.
pub fn cells(pts: &[Pt], size: f64) -> Vec<(f64, f64, i64)> {
    let mut by: HashMap<(i64, i64), (i64, f64, f64)> = HashMap::new();
    for p in pts {
        let e = by.entry(cell(p.at(), size)).or_default();
        e.0 += 1;
        e.1 += p.lat;
        e.2 += p.lon;
    }
    let mut out: Vec<(f64, f64, i64)> = by.into_values().map(|(n, la, lo)| (la / n as f64, lo / n as f64, n)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

/// A flag from a country code: two regional-indicator letters.
pub fn flag(cc: &str) -> String {
    if cc.len() != 2 || !cc.chars().all(|c| c.is_ascii_alphabetic()) {
        return String::new();
    }
    cc.to_ascii_uppercase().chars().filter_map(|c| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32))).collect()
}

/// A title from the towns a trip spent most photos in: "Shimla & Manali",
/// "Goa", or three and more as "Delhi, Shimla & Manali".
pub fn title_from(towns: &[(String, usize)]) -> String {
    let mut t: Vec<&(String, usize)> = towns.iter().filter(|(n, _)| !n.is_empty()).collect();
    t.sort_by(|a, b| b.1.cmp(&a.1));
    let names: Vec<&str> = t.iter().take(3).map(|(n, _)| n.as_str()).collect();
    match names.as_slice() {
        [] => String::new(),
        [a] => a.to_string(),
        [a, b] => format!("{a} & {b}"),
        [a, b, c] => format!("{a}, {b} & {c}"),
        _ => unreachable!(),
    }
}

/// A trip as GPX: the route's stops as a track.
pub fn gpx(name: &str, pts: &[Pt]) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"Tulipix\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n<trk><name>{}</name><trkseg>\n",
        esc(name)
    );
    for p in pts {
        let t = chrono::DateTime::from_timestamp(p.ts, 0).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()).unwrap_or_default();
        out.push_str(&format!("<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><time>{t}</time></trkpt>\n", p.lat, p.lon));
    }
    out.push_str("</trkseg></trk>\n</gpx>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    const DELHI: (f64, f64) = (28.61, 77.21);
    const SHIMLA: (f64, f64) = (31.10, 77.17);
    const MANALI: (f64, f64) = (32.24, 77.19);

    fn at(id: i64, ts: i64, p: (f64, f64)) -> Pt {
        Pt { id, ts, lat: p.0, lon: p.1 }
    }

    #[test]
    fn home_is_where_most_photos_are() {
        let mut pts: Vec<Pt> = (0..30).map(|i| at(i, i, DELHI)).collect();
        pts.extend((0..5).map(|i| at(100 + i, i, SHIMLA)));
        let h = home_of(&pts).unwrap();
        assert!(km(h, DELHI) < 1.0);
    }

    #[test]
    fn a_trip_is_a_run_away_from_home() {
        let rules = Rules { min_km: 80.0, gap_days: 2, min_photos: 3 };
        let mut pts = vec![at(1, 0, DELHI)];
        // Four photos over two days in Shimla and Manali: one trip.
        pts.push(at(2, DAY, SHIMLA));
        pts.push(at(3, DAY + 3600, SHIMLA));
        pts.push(at(4, 2 * DAY, MANALI));
        pts.push(at(5, 2 * DAY + 60, MANALI));
        // Home again, then two lone photos away: too few.
        pts.push(at(6, 3 * DAY, DELHI));
        pts.push(at(7, 10 * DAY, SHIMLA));
        pts.push(at(8, 10 * DAY + 60, SHIMLA));
        let t = find_trips(&pts, DELHI, rules);
        assert_eq!(t, vec![Found { start: DAY, end: 2 * DAY + 60, photos: 4 }]);
        // With no gap allowed, every photo away is its own run.
        let t = find_trips(&pts[1..5], DELHI, Rules { gap_days: 0, min_photos: 1, ..rules });
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn routes_titles_and_flags() {
        let pts = vec![at(1, 0, SHIMLA), at(2, 1, (31.101, 77.171)), at(3, 2, MANALI)];
        let r = route(&pts);
        assert_eq!(r.len(), 2);
        assert!((route_km(&r) - 126.0).abs() < 5.0);
        assert_eq!(title_from(&[("Manali".into(), 3), ("Shimla".into(), 9)]), "Shimla & Manali");
        assert_eq!(title_from(&[("Goa".into(), 3), (String::new(), 9)]), "Goa");
        assert_eq!(flag("in"), "🇮🇳");
        assert_eq!(flag(""), "");
        assert!(gpx("A & B", &pts).contains("<name>A &amp; B</name>"));
    }
}
