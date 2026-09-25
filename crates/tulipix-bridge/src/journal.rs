// The Journal section's store, and the day it gathers.
//
// `journal.db` holds only what you wrote: entries, their moods and tags, the
// photos you kept in them, voice notes, and which gathered rows you switched
// off for a day. Everything else a day shows is read from the other sections'
// databases when it is drawn. Nothing is copied in, so a photo deleted in
// Photos is gone from the journal too, and switching a row off hides it
// without touching the section it came from.

use std::collections::{BTreeSet, HashMap};

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone};
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS entries (
    id      INTEGER PRIMARY KEY,
    day     TEXT    NOT NULL,             -- the local ISO date it belongs to
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    body    TEXT    NOT NULL DEFAULT '',
    words   INTEGER NOT NULL DEFAULT 0,
    mood    INTEGER NOT NULL DEFAULT 0,   -- 0 unset, 1 rough … 5 great
    place   TEXT    NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS entries_day_idx ON entries(day);

CREATE TABLE IF NOT EXISTS entry_tags (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag      TEXT    NOT NULL,
    PRIMARY KEY (entry_id, tag)
);

-- Photos kept in an entry, by their item id in photos.db.
CREATE TABLE IF NOT EXISTS entry_photos (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    photo_id INTEGER NOT NULL,
    PRIMARY KEY (entry_id, photo_id)
);

CREATE TABLE IF NOT EXISTS voice_notes (
    id         INTEGER PRIMARY KEY,
    entry_id   INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    path       TEXT    NOT NULL,
    duration_s REAL    NOT NULL DEFAULT 0,
    peaks      TEXT    NOT NULL DEFAULT '',     -- comma-separated, 0..1
    transcript TEXT    NOT NULL DEFAULT '',
    state      TEXT    NOT NULL DEFAULT 'new',  -- new | done | failed
    created    INTEGER NOT NULL
);

-- The day's weather, once fetched: "18° and clear".
CREATE TABLE IF NOT EXISTS weather (
    day  TEXT PRIMARY KEY,
    text TEXT NOT NULL
);

-- A gathered row switched off for one day.
CREATE TABLE IF NOT EXISTS hidden (
    day    TEXT NOT NULL,
    source TEXT NOT NULL,                       -- photos | music | …
    PRIMARY KEY (day, source)
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

// ── days ────────────────────────────────────────────────────────────────────

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn parse_day(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

pub fn shift(d: NaiveDate, days: i64) -> NaiveDate {
    d.checked_add_signed(chrono::TimeDelta::days(days)).unwrap_or(d)
}

/// Local midnight to the next local midnight, in Unix seconds. `earliest`,
/// because a clock change at midnight makes midnight ambiguous or missing.
pub fn bounds(d: NaiveDate) -> (i64, i64) {
    let at = |d: NaiveDate| {
        Local
            .from_local_datetime(&d.and_time(NaiveTime::MIN))
            .earliest()
            .map(|t| t.timestamp())
            .unwrap_or(0)
    };
    (at(d), at(shift(d, 1)))
}

/// The same date `years` back; None for 29 February in a year without one.
pub fn years_back(d: NaiveDate, years: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(d.year() - years, d.month(), d.day())
}

pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

pub fn days_in_month(d: NaiveDate) -> u32 {
    let first = month_start(d);
    let next = first.checked_add_months(chrono::Months::new(1)).unwrap_or(first);
    (next - first).num_days() as u32
}

/// "08:12" in local time.
pub fn clock(unix: i64) -> String {
    Local
        .timestamp_opt(unix, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

pub fn words(text: &str) -> i64 {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count() as i64
}

/// The run that ends today, or yesterday — a day not yet written is not a day
/// missed — then the longest run ever and the day it ended.
pub fn streaks(days: &BTreeSet<NaiveDate>, today: NaiveDate) -> (i64, i64, Option<NaiveDate>) {
    let (mut longest, mut end, mut run) = (0, None, 0);
    let mut prev: Option<NaiveDate> = None;
    for &d in days {
        run = match prev {
            Some(p) if p.succ_opt() == Some(d) => run + 1,
            _ => 1,
        };
        if run > longest {
            longest = run;
            end = Some(d);
        }
        prev = Some(d);
    }
    let mut d = if days.contains(&today) { today } else { shift(today, -1) };
    let mut current = 0;
    while days.contains(&d) {
        current += 1;
        d = shift(d, -1);
    }
    (current, longest, end)
}

// ── places ──────────────────────────────────────────────────────────────────

pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

/// Where the day's photos were taken, in the order they were: a new stop each
/// time the camera moved more than 300 m from the last one.
///
/// Names come from `name_for`, once the GeoNames table is downloaded.
pub fn stops(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for &p in points {
        if out.last().is_none_or(|&l| km(l, p) > 0.3) {
            out.push(p);
        }
    }
    out
}

// ── place names ─────────────────────────────────────────────────────────────
//
// GeoNames' towns and cities over 15,000 people, downloaded once (a 3 MB zip)
// and kept as name / latitude / longitude. The nearest one within 25 km names
// a stop. Offline after that one download, and a town rather than a street —
// "Bristol", not "Harbourside"; the entry's own Place is still the way to say
// which part.

const GEONAMES: &str = "https://download.geonames.org/export/dump/cities15000.zip";

/// A stop further than this from every town has no name.
const NAME_KM: f64 = 25.0;

pub fn places_file() -> Option<std::path::PathBuf> {
    tulipix_core::paths::data_dir().map(|d| d.join("journal").join("places.tsv"))
}

pub fn has_place_names() -> bool {
    places_file().is_some_and(|f| f.exists())
}

/// A town from GeoNames, with the country and state it is in — Places counts
/// those; Journal only needs the name.
pub struct Town {
    pub lat: f64,
    pub lon: f64,
    pub name: String,
    /// ISO 3166 alpha-2.
    pub cc: String,
    /// GeoNames' first-level code, "11"; `regions.tsv` names "IN.11".
    pub admin1: String,
}

/// GeoNames' tab-separated table as towns.
pub fn parse_towns(tsv: &str) -> Vec<Town> {
    tsv.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let (name, lat, lon) = (f.get(1)?, f.get(4)?.parse::<f64>().ok()?, f.get(5)?.parse::<f64>().ok()?);
            (!name.is_empty()).then(|| Town {
                lat,
                lon,
                name: name.to_string(),
                cc: f.get(8).unwrap_or(&"").to_string(),
                admin1: f.get(10).unwrap_or(&"").to_string(),
            })
        })
        .collect()
}

/// GeoNames' tab-separated table as (latitude, longitude, name). Only the
/// tests still want it; the table itself is read as `Town`s.
#[cfg(test)]
fn parse_geonames(tsv: &str) -> Vec<(f64, f64, String)> {
    parse_towns(tsv).into_iter().map(|t| (t.lat, t.lon, t.name)).collect()
}

/// The nearest name within `NAME_KM`.
pub fn nearest(table: &[(f64, f64, String)], p: (f64, f64)) -> Option<&str> {
    table
        .iter()
        // A degree of latitude is 111 km: anything further off is not a
        // candidate, and the trig is skipped for it.
        .filter(|(la, _, _)| (la - p.0).abs() < 0.5)
        .map(|(la, lo, n)| (km((*la, *lo), p), n))
        .filter(|(d, _)| *d <= NAME_KM)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, n)| n.as_str())
}

fn table() -> &'static std::sync::Mutex<Option<std::sync::Arc<Vec<(f64, f64, String)>>>> {
    static T: std::sync::Mutex<Option<std::sync::Arc<Vec<(f64, f64, String)>>>> = std::sync::Mutex::new(None);
    &T
}

/// The loaded table, read from disk the first time it is asked for.
fn loaded() -> Option<std::sync::Arc<Vec<(f64, f64, String)>>> {
    let mut g = table().lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        let text = std::fs::read_to_string(places_file()?).ok()?;
        // The file is ours: name, latitude, longitude.
        let rows = text
            .lines()
            .filter_map(|l| {
                let mut f = l.split('\t');
                let name = f.next()?.to_string();
                Some((f.next()?.parse::<f64>().ok()?, f.next()?.parse::<f64>().ok()?, name))
            })
            .collect();
        *g = Some(std::sync::Arc::new(rows));
    }
    g.clone()
}

/// A name for each stop, "" where there is none (or no table yet).
pub fn names_for(stops: &[(f64, f64)]) -> Vec<String> {
    let Some(t) = loaded() else { return vec![String::new(); stops.len()] };
    stops.iter().map(|p| nearest(&t, *p).unwrap_or_default().to_string()).collect()
}

/// Fetch GeoNames' table once and keep what naming needs. Returns the places.
///
/// The file is name, latitude, longitude, country, state: Journal reads the
/// first three, Places all five. Country and state names come beside it, in
/// `countries.tsv` and `regions.tsv`; those two are small and optional.
pub async fn download_place_names(client: &reqwest::Client) -> Result<usize> {
    let bytes = client.get(GEONAMES).send().await?.error_for_status()?.bytes().await?;
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<Town>> {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        let mut text = String::new();
        z.by_name("cities15000.txt")?.read_to_string(&mut text)?;
        Ok(parse_towns(&text))
    })
    .await??;
    if rows.len() < 1000 {
        anyhow::bail!("the place table came back short ({} places)", rows.len());
    }
    let file = places_file().ok_or_else(|| anyhow::anyhow!("no data folder"))?;
    let dir = file.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    tokio::fs::create_dir_all(&dir).await?;
    let out: String = rows.iter().map(|t| format!("{}\t{}\t{}\t{}\t{}\n", t.name, t.lat, t.lon, t.cc, t.admin1)).collect();
    tokio::fs::write(&file, out).await?;
    for (url, name, key, value) in [
        ("https://download.geonames.org/export/dump/countryInfo.txt", "countries.tsv", 0, 4),
        ("https://download.geonames.org/export/dump/admin1CodesASCII.txt", "regions.tsv", 0, 1),
    ] {
        let Ok(text) = async { client.get(url).send().await?.error_for_status()?.text().await }.await else { continue };
        let kept: String = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                Some(format!("{}\t{}\n", f.get(key)?, f.get(value)?))
            })
            .collect();
        tokio::fs::write(dir.join(name), kept).await.ok();
    }
    *table().lock().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(rows.len())
}

// ── weather ─────────────────────────────────────────────────────────────────
//
// Open-Meteo: no key, and it keeps past days. Asked once per day and kept in
// journal.db; only the date and a location rounded to about 10 km are sent.

/// Where the day happened: its first photo's location, else the last photo
/// with one in the month before. None when nothing says.
pub async fn where_was(day: NaiveDate) -> Option<(f64, f64)> {
    if let Some(p) = day_photos(day).await.into_iter().find_map(|p| p.2) {
        return Some(p);
    }
    let pool = crate::db::photos_pool().await.ok()?;
    let (_, end) = bounds(day);
    let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT pm.gps_lat, pm.gps_lon FROM photo_meta pm JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at < ? AND pm.taken_at >= ? AND pm.gps_lat IS NOT NULL AND pm.gps_lon IS NOT NULL \
           AND i.missing_since IS NULL AND pm.deleted_at IS NULL \
         ORDER BY pm.taken_at DESC LIMIT 1",
    )
    .bind(end)
    .bind(end - 31 * 86_400)
    .fetch_optional(pool)
    .await
    .ok()?;
    row.and_then(|(a, b)| a.zip(b))
}

/// WMO weather codes, as a few words.
pub fn wmo(code: i64) -> &'static str {
    match code {
        0 => "clear",
        1 => "mostly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51..=57 => "drizzle",
        61..=67 => "rain",
        71..=77 => "snow",
        80..=82 => "showers",
        85 | 86 => "snow showers",
        95..=99 => "thunderstorms",
        _ => "",
    }
}

pub async fn weather_on(client: &reqwest::Client, day: NaiveDate, at: (f64, f64)) -> Result<String> {
    // The forecast service holds the last three months, the archive the rest
    // (a few days behind).
    let recent = (today() - day).num_days() < 80;
    let base = if recent { "https://api.open-meteo.com/v1/forecast" } else { "https://archive-api.open-meteo.com/v1/archive" };
    let round = |x: f64| format!("{:.1}", x);
    let v: serde_json::Value = client
        .get(base)
        .query(&[
            ("latitude", round(at.0)),
            ("longitude", round(at.1)),
            ("daily", "weather_code,temperature_2m_max".into()),
            ("timezone", "auto".into()),
            ("start_date", day.to_string()),
            ("end_date", day.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let code = v["daily"]["weather_code"][0].as_f64().map(|c| c as i64);
    let temp = v["daily"]["temperature_2m_max"][0].as_f64();
    Ok(weather_line(temp, code))
}

/// "18° and clear"; either half alone when the other is missing.
pub fn weather_line(temp: Option<f64>, code: Option<i64>) -> String {
    let sky = code.map(wmo).unwrap_or_default();
    match (temp, sky) {
        (Some(t), "") => format!("{}°", t.round() as i64),
        (Some(t), s) => format!("{}° and {s}", t.round() as i64),
        (None, s) => s.to_string(),
    }
}

pub fn route_km(stops: &[(f64, f64)]) -> f64 {
    stops.windows(2).map(|w| km(w[0], w[1])).sum()
}

/// Stops fitted into a unit square, north up, keeping the aspect so a walk
/// along a line of latitude does not become a diagonal. One stop sits in the
/// middle.
pub fn fit(stops: &[(f64, f64)]) -> Vec<(f64, f64)> {
    if stops.is_empty() {
        return Vec::new();
    }
    let k = stops[0].0.to_radians().cos().max(0.01);
    let xs: Vec<f64> = stops.iter().map(|p| p.1 * k).collect();
    let ys: Vec<f64> = stops.iter().map(|p| p.0).collect();
    let (x0, x1) = (xs.iter().cloned().fold(f64::MAX, f64::min), xs.iter().cloned().fold(f64::MIN, f64::max));
    let (y0, y1) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
    let span = (x1 - x0).max(y1 - y0);
    if span <= 0.0 {
        return vec![(0.5, 0.5); stops.len()];
    }
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    xs.iter()
        .zip(&ys)
        .map(|(x, y)| (0.5 + (x - cx) / span * 0.8, 0.5 - (y - cy) / span * 0.8))
        .collect()
}

// ── what the other sections know about a day ────────────────────────────────

/// One section's part of a day, before it is formatted.
pub struct Gathered {
    pub source: &'static str,
    /// When it started, for the order and the clock. Money has a date and no
    /// time, so it sorts at noon and prints no clock.
    pub at: i64,
    pub timed: bool,
    pub title: String,
    pub ids: Vec<i64>,
}

/// Morning, afternoon, evening or night, from a local clock.
fn part_of_day(unix: i64) -> &'static str {
    let h = Local.timestamp_opt(unix, 0).single().map(|t| t.format("%H").to_string());
    match h.and_then(|h| h.parse::<u32>().ok()).unwrap_or(12) {
        5..=11 => "in the morning",
        12..=16 => "in the afternoon",
        17..=21 => "in the evening",
        _ => "at night",
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// The day's photos: every id, in order, and where each was taken.
pub async fn day_photos(day: NaiveDate) -> Vec<(i64, i64, Option<(f64, f64)>)> {
    let Ok(p) = crate::db::photos_pool().await else { return Vec::new() };
    let (a, b) = bounds(day);
    sqlx::query_as::<_, (i64, i64, Option<f64>, Option<f64>)>(
        "SELECT i.id, pm.taken_at, pm.gps_lat, pm.gps_lon FROM photo_meta pm \
         JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at >= ? AND pm.taken_at < ? \
           AND i.missing_since IS NULL AND pm.deleted_at IS NULL \
         ORDER BY pm.taken_at",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, at, lat, lon)| (id, at, lat.zip(lon)))
    .collect()
}

fn photos_row(photos: &[(i64, i64, Option<(f64, f64)>)]) -> Option<Gathered> {
    let first = photos.first()?;
    let last = photos.last()?;
    let n = photos.len() as i64;
    let when = if last.1 - first.1 > 6 * 3600 { "across the day" } else { part_of_day(first.1) };
    Some(Gathered {
        source: "photos",
        at: first.1,
        timed: true,
        title: format!("{} {when}", plural(n, "photo", "photos")),
        ids: photos.iter().take(6).map(|p| p.0).collect(),
    })
}

/// Plays on a day: (when, seconds heard, artist).
async fn day_plays(day: NaiveDate) -> Vec<(i64, f64, String)> {
    let Ok(p) = crate::db::music_pool().await else { return Vec::new() };
    let (a, b) = bounds(day);
    sqlx::query_as::<_, (i64, i64, String, f64)>(
        "SELECT h.played_at, h.ms_played, COALESCE(ar.name, ''), COALESCE(tm.duration_s, 0) \
         FROM play_history h \
         LEFT JOIN track_meta tm ON tm.item_id = h.item_id \
         LEFT JOIN artists ar ON ar.id = tm.artist_id \
         WHERE h.played_at >= ? AND h.played_at < ? ORDER BY h.played_at",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    // A play the player timed is what was heard; an untimed one counts whole.
    .map(|(at, ms, artist, dur)| (at, if ms > 0 { ms as f64 / 1000.0 } else { dur }, artist))
    .collect()
}

/// Artists by plays, most first.
fn top_artists(plays: &[(i64, f64, String)]) -> Vec<(String, i64)> {
    let mut n: HashMap<&str, i64> = HashMap::new();
    for (_, _, a) in plays {
        if !a.is_empty() {
            *n.entry(a.as_str()).or_default() += 1;
        }
    }
    let mut v: Vec<(String, i64)> = n.into_iter().map(|(a, c)| (a.to_string(), c)).collect();
    v.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    v
}

fn music_row(plays: &[(i64, f64, String)]) -> Option<Gathered> {
    let first = plays.first()?;
    let minutes = (plays.iter().map(|p| p.1).sum::<f64>() / 60.0).round() as i64;
    let names: Vec<String> = top_artists(plays).into_iter().take(2).map(|a| a.0).collect();
    let who = if names.is_empty() {
        plural(plays.len() as i64, "track", "tracks")
    } else {
        names.join(", ")
    };
    Some(Gathered {
        source: "music",
        at: first.0,
        timed: true,
        title: format!("{who} · {}", plural(minutes.max(1), "minute", "minutes")),
        ids: Vec::new(),
    })
}

/// Spending on a day, in the base currency: the first thing bought and the
/// total. Income and transfers are not what a diary means by "spent".
async fn day_spend(day: NaiveDate) -> Option<(i64, String, i64, i64)> {
    let p = crate::db::finances_pool().await.ok()?;
    let rows = sqlx::query_as::<_, (i64, String, i64)>(
        "SELECT base_minor, description, created_at FROM transactions \
         WHERE occurred_on = ? AND kind = 'expense' ORDER BY created_at, id",
    )
    .bind(iso(day))
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (first, desc, created) = rows.first()?.clone();
    let total: i64 = rows.iter().map(|r| r.0.abs()).sum();
    Some((first.abs(), desc, total, created))
}

fn money(minor: i64) -> String {
    tulipix_finances::money::format_minor(minor, &tulipix_finances::fx::base_currency())
}

fn finances_row(day: NaiveDate, spend: Option<(i64, String, i64, i64)>) -> Option<Gathered> {
    let (first, desc, total, created) = spend?;
    let (a, b) = bounds(day);
    let timed = created >= a && created < b;
    let head = if desc.trim().is_empty() { money(first) } else { format!("{} at {}", money(first), desc.trim()) };
    Some(Gathered {
        source: "finances",
        at: if timed { created } else { a + 12 * 3600 },
        timed,
        title: if total == first { head } else { format!("{head} · {} spent", money(total)) },
        ids: Vec::new(),
    })
}

/// What was watched: films by title, episodes as "Show S2 E3", anything else
/// by its file name. Only the last position of each is kept by Videos, so a
/// film started one day and finished the next shows on the second.
async fn videos_row(day: NaiveDate) -> Option<Gathered> {
    let p = crate::db::videos_pool().await.ok()?;
    let (a, b) = bounds(day);
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT wp.updated, COALESCE(m.title, s.title || ' S' || e.season || ' E' || e.episode, i.abs_path, '') \
         FROM watch_progress wp JOIN items i ON i.id = wp.item_id \
         LEFT JOIN movies m ON m.item_id = wp.item_id \
         LEFT JOIN episodes e ON e.item_id = wp.item_id \
         LEFT JOIN shows s ON s.id = e.show_id \
         WHERE wp.updated >= ? AND wp.updated < ? ORDER BY wp.updated",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (at, first) = rows.first()?.clone();
    let name = |s: &str| std::path::Path::new(s).file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let first = if first.contains('/') || first.contains('\\') { name(&first) } else { first };
    let more = rows.len() - 1;
    Some(Gathered {
        source: "videos",
        at,
        timed: true,
        title: if more == 0 { format!("Watched {first}") } else { format!("Watched {first} and {more} more") },
        ids: Vec::new(),
    })
}

async fn books_row(day: NaiveDate) -> Option<Gathered> {
    let p = crate::db::books_pool().await.ok()?;
    let (a, b) = bounds(day);
    let rows = sqlx::query_as::<_, (i64, String, f64)>(
        "SELECT p.updated_at, COALESCE(b.title, ''), p.percent FROM progress p \
         JOIN books b ON b.id = p.book_id \
         WHERE p.updated_at >= ? AND p.updated_at < ? ORDER BY p.updated_at",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (at, title, pct) = rows.first()?.clone();
    let more = rows.len() - 1;
    let pct = pct.clamp(0.0, 100.0).round() as i64;
    Some(Gathered {
        source: "books",
        at,
        timed: true,
        title: if more == 0 {
            format!("Read {title} · {pct}% through")
        } else {
            format!("Read {title} and {}", plural(more as i64, "other book", "other books"))
        },
        ids: Vec::new(),
    })
}

/// The sections that shipped with Journal open their databases on first use;
/// asking a day of one never opened would create it empty.
fn made(section: &str) -> bool {
    tulipix_core::paths::db_path(section).is_some_and(|f| f.exists())
}

/// What was cooked to the end in cook mode. Kitchen keeps only the last time
/// each recipe was cooked, so a dish made twice shows on the later day.
async fn kitchen_row(day: NaiveDate) -> Option<Gathered> {
    if !made("kitchen") {
        return None;
    }
    let p = crate::db::kitchen_pool().await.ok()?;
    let (a, b) = bounds(day);
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT last_cooked, title FROM recipes WHERE last_cooked >= ? AND last_cooked < ? ORDER BY last_cooked",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (at, first) = rows.first()?.clone();
    let more = rows.len() - 1;
    Some(Gathered {
        source: "kitchen",
        at,
        timed: true,
        title: if more == 0 { format!("Cooked {first}") } else { format!("Cooked {first} and {more} more") },
        ids: Vec::new(),
    })
}

/// Passages kept from articles that day — the reading worth remembering.
async fn feeds_row(day: NaiveDate) -> Option<Gathered> {
    if !made("feeds") {
        return None;
    }
    let p = crate::db::feeds_pool().await.ok()?;
    let (a, b) = bounds(day);
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT h.created, a.title FROM highlights h JOIN articles a ON a.id = h.article_id \
         WHERE h.created >= ? AND h.created < ? ORDER BY h.created",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (at, first) = rows.first()?.clone();
    let articles: BTreeSet<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    let n = rows.len() as i64;
    Some(Gathered {
        source: "feeds",
        at,
        timed: true,
        title: if articles.len() == 1 {
            format!("Highlighted “{first}” · {}", plural(n, "passage", "passages"))
        } else {
            format!("{} from {} articles", plural(n, "highlight", "highlights"), articles.len())
        },
        ids: Vec::new(),
    })
}

/// Papers that came in that day. Vault papers stay out: their names are not
/// for a page that shows without the PIN.
async fn papers_row(day: NaiveDate) -> Option<Gathered> {
    if !made("papers") {
        return None;
    }
    let p = crate::db::papers_pool().await.ok()?;
    let (a, b) = bounds(day);
    let rows = sqlx::query_as::<_, (i64, String)>(
        "SELECT created, title FROM papers WHERE vault = 0 AND status = 'read' AND created >= ? AND created < ? ORDER BY created",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let (at, first) = rows.first()?.clone();
    let more = rows.len() - 1;
    Some(Gathered {
        source: "papers",
        at,
        timed: true,
        title: if more == 0 { format!("Added {first} to Papers") } else { format!("Added {first} and {more} more to Papers") },
        ids: Vec::new(),
    })
}

/// Everything the other sections have for a day, in the order it happened,
/// plus the photos' coordinates for the map.
pub async fn gather(day: NaiveDate) -> (Vec<Gathered>, Vec<(f64, f64)>) {
    let (photos, plays, spend, videos, books) =
        tokio::join!(day_photos(day), day_plays(day), day_spend(day), videos_row(day), books_row(day));
    let (kitchen, feeds, papers) = tokio::join!(kitchen_row(day), feeds_row(day), papers_row(day));
    let points: Vec<(f64, f64)> = photos.iter().filter_map(|p| p.2).collect();
    let mut rows: Vec<Gathered> =
        [photos_row(&photos), music_row(&plays), finances_row(day, spend), videos, books, kitchen, feeds, papers]
            .into_iter()
            .flatten()
            .collect();
    rows.sort_by_key(|r| r.at);
    (rows, points)
}

/// A year-ago day in one line, for "On this day" when nothing was written:
/// what the photos and the music say about it.
pub struct Echo {
    pub photos: Vec<i64>,
    pub photo_count: i64,
    pub artist: String,
    pub artist_plays: i64,
    pub spent: i64,
}

pub async fn echo(day: NaiveDate) -> Echo {
    let (photos, plays, spend) = tokio::join!(day_photos(day), day_plays(day), day_spend(day));
    let top = top_artists(&plays).into_iter().next().unwrap_or_default();
    Echo {
        photo_count: photos.len() as i64,
        photos: photos.iter().take(3).map(|p| p.0).collect(),
        artist: top.0,
        artist_plays: top.1,
        spent: spend.map(|s| s.2).unwrap_or(0),
    }
}

pub fn spent_text(minor: i64) -> String {
    money(minor)
}

/// First photo of every day in a month, for the calendar's Photos view.
pub async fn month_photos(first: NaiveDate) -> HashMap<String, i64> {
    let Ok(p) = crate::db::photos_pool().await else { return HashMap::new() };
    let (a, _) = bounds(first);
    let (b, _) = bounds(shift(first, days_in_month(first) as i64));
    // SQLite returns the bare column from the row MIN picked.
    sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT date(pm.taken_at, 'unixepoch', 'localtime') AS d, i.id, MIN(pm.taken_at) \
         FROM photo_meta pm JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at >= ? AND pm.taken_at < ? \
           AND i.missing_since IS NULL AND pm.deleted_at IS NULL \
         GROUP BY d",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(d, id, _)| (d, id))
    .collect()
}

/// Photos taken in a span, for "96 of 412 taken".
pub async fn photos_between(a: i64, b: i64) -> i64 {
    let Ok(p) = crate::db::photos_pool().await else { return 0 };
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM photo_meta pm JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at >= ? AND pm.taken_at < ? \
           AND i.missing_since IS NULL AND pm.deleted_at IS NULL",
    )
    .bind(a)
    .bind(b)
    .fetch_one(p)
    .await
    .unwrap_or(0)
}

/// Local days with a photo in them, over a span.
pub async fn photo_days(a: i64, b: i64) -> BTreeSet<String> {
    let Ok(p) = crate::db::photos_pool().await else { return BTreeSet::new() };
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT date(pm.taken_at, 'unixepoch', 'localtime') FROM photo_meta pm \
         JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at >= ? AND pm.taken_at < ? AND i.missing_since IS NULL",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .collect()
}

/// Artists heard before noon, per local day, over a span.
pub async fn morning_artists(a: i64, b: i64) -> HashMap<String, Vec<String>> {
    let Ok(p) = crate::db::music_pool().await else { return HashMap::new() };
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT date(h.played_at, 'unixepoch', 'localtime'), ar.name FROM play_history h \
         JOIN track_meta tm ON tm.item_id = h.item_id JOIN artists ar ON ar.id = tm.artist_id \
         WHERE h.played_at >= ? AND h.played_at < ? \
           AND CAST(strftime('%H', h.played_at, 'unixepoch', 'localtime') AS INTEGER) < 12",
    )
    .bind(a)
    .bind(b)
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (d, artist) in rows {
        out.entry(d).or_default().push(artist);
    }
    out
}

// ── voice ───────────────────────────────────────────────────────────────────

/// A WAV's length and a coarse loudness outline: `n` buckets, each its
/// loudest sample, scaled so the loudest bucket is 1. 16-bit PCM only, which
/// is what every recorder here is asked for.
pub fn wave(bytes: &[u8], n: usize) -> (f64, Vec<f64>) {
    let (mut rate, mut channels, mut data) = (16000u32, 1u16, &bytes[0..0]);
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        let body = i + 8;
        if id == b"fmt " && body + 4 <= bytes.len() {
            channels = u16::from_le_bytes([bytes[body + 2], bytes[body + 3]]).max(1);
            if body + 8 <= bytes.len() {
                rate = u32::from_le_bytes([bytes[body + 4], bytes[body + 5], bytes[body + 6], bytes[body + 7]]).max(1);
            }
        }
        if id == b"data" {
            // A recorder stopped by a signal can leave the size at 0 or at
            // u32::MAX: the rest of the file is the data either way.
            let end = if len == 0 || body + len > bytes.len() { bytes.len() } else { body + len };
            data = &bytes[body..end];
            break;
        }
        i = body + len + (len & 1);
    }
    let samples: Vec<i16> = data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let secs = samples.len() as f64 / channels as f64 / rate as f64;
    if samples.is_empty() || n == 0 {
        return (secs, Vec::new());
    }
    let per = samples.len().div_ceil(n);
    let peaks: Vec<f64> = samples
        .chunks(per)
        .map(|c| c.iter().map(|s| (*s as f64).abs()).fold(0.0, f64::max))
        .collect();
    let top = peaks.iter().cloned().fold(1.0, f64::max);
    (secs, peaks.into_iter().map(|p| (p / top * 100.0).round() / 100.0).collect())
}

// ── questions ───────────────────────────────────────────────────────────────

pub const PROMPTS: [&str; 16] = [
    "What surprised you today?",
    "What would you like to remember about today in a year?",
    "Who did you talk to, and what stayed with you?",
    "What took longer than it should have?",
    "What was the best thing you ate?",
    "What are you looking forward to this week?",
    "What did you notice on the way somewhere?",
    "What made you laugh?",
    "What would you do differently tomorrow?",
    "What are you still thinking about?",
    "What did you finish, and what did you start?",
    "Where did you feel most like yourself today?",
    "What did you learn that you did not know this morning?",
    "What is one small thing that went right?",
    "What are you putting off, and why?",
    "What would you tell yourself from a year ago?",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_take_the_nearest_town_within_reach() {
        let tsv = "2654675\tBristol\tBristol\t\t51.45523\t-2.59665\tP\n\
                   2655095\tBath\tBath\t\t51.3751\t-2.36172\tP\n\
                   bad line\n";
        let t = parse_geonames(tsv);
        assert_eq!(t.len(), 2);
        assert_eq!(nearest(&t, (51.449, -2.60)), Some("Bristol"));
        assert_eq!(nearest(&t, (51.38, -2.35)), Some("Bath"));
        assert_eq!(nearest(&t, (48.85, 2.35)), None);
    }

    #[test]
    fn weather_reads_like_a_diary() {
        assert_eq!(weather_line(Some(18.4), Some(0)), "18° and clear");
        assert_eq!(weather_line(Some(-0.6), Some(71)), "-1° and snow");
        assert_eq!(weather_line(Some(12.0), Some(999)), "12°");
        assert_eq!(weather_line(None, Some(61)), "rain");
    }

    fn d(s: &str) -> NaiveDate {
        parse_day(s).unwrap()
    }

    #[test]
    fn words_count_what_reads_as_words() {
        assert_eq!(words("Took the long way home — six photos."), 7);
        assert_eq!(words("  \n "), 0);
    }

    #[test]
    fn a_streak_survives_until_the_day_after_is_over() {
        let days: BTreeSet<NaiveDate> = ["2026-09-16", "2026-09-17", "2026-09-18", "2026-09-10", "2026-09-11"]
            .iter()
            .map(|s| d(s))
            .collect();
        // Today unwritten: yesterday's run still counts.
        assert_eq!(streaks(&days, d("2026-09-19")), (3, 3, Some(d("2026-09-18"))));
        // A day missed: the run is over.
        assert_eq!(streaks(&days, d("2026-09-20")).0, 0);
    }

    #[test]
    fn stops_ignore_jitter_and_the_route_adds_up() {
        let pts = [(51.4500, -2.6000), (51.4501, -2.6001), (51.4500, -2.5800), (51.4400, -2.5800)];
        let s = stops(&pts);
        assert_eq!(s.len(), 3);
        let r = route_km(&s);
        assert!(r > 2.4 && r < 2.6, "{r}");
        let f = fit(&s);
        assert!(f.iter().all(|p| (0.0..=1.0).contains(&p.0) && (0.0..=1.0).contains(&p.1)));
        assert_eq!(fit(&s[..1]), vec![(0.5, 0.5)]);
    }

    #[test]
    fn a_wav_gives_its_length_and_outline() {
        let samples: Vec<i16> = (0..16000).map(|i| if i < 8000 { 1000 } else { 4000 }).collect();
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes()); // PCM
        wav.extend(1u16.to_le_bytes()); // mono
        wav.extend(16000u32.to_le_bytes());
        wav.extend(32000u32.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(0u32.to_le_bytes()); // left unfinished by a signal
        wav.extend(&data);
        let (secs, peaks) = wave(&wav, 4);
        assert!((secs - 1.0).abs() < 1e-9);
        assert_eq!(peaks, vec![0.25, 0.25, 1.0, 1.0]);
    }

    #[test]
    fn leap_days_have_no_echo_in_ordinary_years() {
        assert_eq!(years_back(d("2028-02-29"), 1), None);
        assert_eq!(years_back(d("2026-09-19"), 3), Some(d("2023-09-19")));
        assert_eq!(days_in_month(d("2026-02-10")), 28);
    }
}
