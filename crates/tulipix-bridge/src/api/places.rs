// The Places section: trips found in the photos, a page per trip, a map of
// everywhere, the towns and countries you have been to.
//
// Four tabs over one snapshot — Trips (with a trip open), Map, Places and Been.
// Everything is worked out from Photos' locations and times; a trip page's days
// are Journal's `gather`, the same rows its Today shows. Names come from the
// GeoNames table Journal downloads. The only thing Places stores is which
// trips were kept or dismissed, their titles and notes, and the wishlist.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use sqlx::SqlitePool;

use crate::db::places_pool;
use crate::journal::{self as j, Town, km};
use crate::places::{self as p, Found, Pt, Rules};

// ------------------------------------------------------------------- state ---

pub struct PlacesState {
    /// trips | map | places | been.
    pub tab: String,
    pub notice: String,
    /// Photos with a location, and all photos.
    pub located: i64,
    pub photos: i64,
    /// The town table is here, with countries: names, Places and Been work.
    pub named: bool,
    /// "Delhi", or coordinates when there are no names.
    pub home: String,
    /// Home was worked out, not set.
    pub home_auto: bool,
    pub trips: Vec<TripCard>,
    /// Found in the photos and not yet kept or dismissed.
    pub suggested: Vec<TripCard>,
    pub open: Option<TripView>,
    pub map: PlacesMap,
    pub towns: Vec<TownRow>,
    pub wishes: Vec<Wish>,
    pub been: Been,
    pub min_km: i64,
    pub gap_days: i64,
    pub min_photos: i64,
}

pub struct TripCard {
    /// 0 for a suggestion.
    pub id: i64,
    pub title: String,
    pub start: i64,
    pub end: i64,
    /// "10–15 May 2025".
    pub range: String,
    pub year: i64,
    pub days: i64,
    pub photos: i64,
    /// Up to three photo ids: the first, the middle and the last.
    pub cover: Vec<i64>,
    /// The countries' flags.
    pub flags: String,
    pub km: i64,
}

pub struct TripView {
    pub id: i64,
    pub title: String,
    pub range: String,
    pub days_n: i64,
    pub photos: i64,
    pub km: i64,
    /// The towns in order, each once.
    pub stops: Vec<String>,
    pub route: Vec<GeoPoint>,
    pub days: Vec<TripDay>,
    pub note: String,
    /// The highest town's height is not known; the furthest from home is.
    pub furthest: String,
}

pub struct TripDay {
    /// ISO.
    pub date: String,
    /// "Day 3".
    pub label: String,
    /// "Mon 12 May".
    pub dow: String,
    /// "Kufri → Manali", or "".
    pub towns: String,
    pub photos: Vec<i64>,
    pub photo_n: i64,
    /// What the other sections have for the day, photos aside.
    pub lines: Vec<DayLine>,
    /// The day's Journal entry, its first lines.
    pub quote: String,
}

pub struct DayLine {
    /// music | finances | videos | books | kitchen | feeds | papers.
    pub source: String,
    pub text: String,
}

pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    pub n: i64,
}

pub struct PlacesMap {
    /// Photos in cells of a tenth of a degree, the busiest first.
    pub points: Vec<GeoPoint>,
    /// Kept trips' routes.
    pub routes: Vec<MapRoute>,
    pub home: Option<GeoPoint>,
    /// "2016".."2025": the years photos with a location span.
    pub first_year: i64,
    pub last_year: i64,
}

pub struct MapRoute {
    pub trip_id: i64,
    pub title: String,
    pub points: Vec<GeoPoint>,
}

pub struct TownRow {
    pub name: String,
    pub region: String,
    pub country: String,
    pub flag: String,
    pub photos: i64,
    /// Days with a photo there.
    pub days: i64,
    /// "May 2019".
    pub first: String,
    pub last: String,
    pub home: bool,
    pub lat: f64,
    pub lon: f64,
}

pub struct Wish {
    pub id: i64,
    pub name: String,
    pub note: String,
}

#[derive(Default)]
pub struct Been {
    pub countries: Vec<CountryRow>,
    pub regions: Vec<String>,
    pub towns: i64,
    pub km: i64,
    pub days_away: i64,
    pub years: Vec<YearBar>,
    pub firsts: Vec<FirstRow>,
}

pub struct CountryRow {
    pub cc: String,
    pub name: String,
    pub flag: String,
    pub photos: i64,
    pub regions: i64,
    pub first_year: i64,
}

pub struct YearBar {
    pub year: i64,
    pub days: i64,
}

pub struct FirstRow {
    pub label: String,
    pub value: String,
}

// ---------------------------------------------------------------- commands ---

pub enum PlacesCmd {
    Refresh,
    SetTab { tab: String },
    OpenTrip { id: i64 },
    CloseTrip,
    /// Keep a suggestion, by its times.
    KeepTrip { start: i64, end: i64 },
    KeepAll,
    /// Not a trip; it is not suggested again.
    Dismiss { start: i64, end: i64 },
    RenameTrip { id: i64, title: String },
    SetNote { id: i64, note: String },
    /// Forget a kept trip; its photos are not suggested again.
    RemoveTrip { id: i64 },
    AddWish { name: String, note: String },
    RemoveWish { id: i64 },
    /// 0, 0 goes back to working home out.
    SetHome { lat: f64, lon: f64 },
    SetRules { min_km: i64, gap_days: i64, min_photos: i64 },
    /// Download the town table (Journal's; Places uses it too).
    GetPlaceNames,
    /// The trip as an entry on its first day.
    ToJournal { id: i64 },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    open: i64,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session { tab: "trips".into(), open: 0, notice: String::new() }))
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn say(notice: impl Into<String>) {
    lock().notice = notice.into();
}

// ---------------------------------------------------------------- exported ---

pub async fn places_dispatch(cmd: PlacesCmd) -> Result<PlacesState> {
    let pool = places_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// A kept trip's route as a GPX file at `path`.
pub async fn places_export_gpx(id: i64, path: String) -> Result<()> {
    let pool = places_pool().await?;
    let (title, start, end): (String, i64, i64) =
        sqlx::query_as("SELECT title, start, end FROM trips WHERE id = ?").bind(id).fetch_one(pool).await?;
    let a = analysis(pool).await?;
    let pts: Vec<Pt> = a.pts.iter().filter(|x| x.ts >= start && x.ts <= end).copied().collect();
    tokio::fs::write(&path, p::gpx(&title, &pts)).await.with_context(|| format!("could not write {path}"))?;
    Ok(())
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: PlacesCmd) -> Result<()> {
    match cmd {
        PlacesCmd::Refresh => {}
        PlacesCmd::SetTab { tab } => lock().tab = tab,
        PlacesCmd::OpenTrip { id } => {
            let mut s = lock();
            s.open = id;
            s.tab = "trips".into();
        }
        PlacesCmd::CloseTrip => lock().open = 0,
        PlacesCmd::KeepTrip { start, end } => {
            let a = analysis(pool).await?;
            keep(pool, &a, start, end).await?;
        }
        PlacesCmd::KeepAll => {
            let a = analysis(pool).await?;
            let taken = stored(pool).await?;
            let fresh: Vec<Found> = a.found.iter().filter(|f| !overlaps(&taken, f.start, f.end)).copied().collect();
            for f in &fresh {
                keep(pool, &a, f.start, f.end).await?;
            }
            say(format!("Kept {}", plural(fresh.len() as i64, "trip")));
        }
        PlacesCmd::Dismiss { start, end } => {
            sqlx::query("INSERT INTO trips (start, end, state, created) VALUES (?, ?, 0, ?)")
                .bind(start)
                .bind(end)
                .bind(p::now())
                .execute(pool)
                .await?;
        }
        PlacesCmd::RenameTrip { id, title } => {
            sqlx::query("UPDATE trips SET title = ? WHERE id = ?").bind(title.trim()).bind(id).execute(pool).await?;
        }
        PlacesCmd::SetNote { id, note } => {
            sqlx::query("UPDATE trips SET note = ? WHERE id = ?").bind(note.trim()).bind(id).execute(pool).await?;
        }
        PlacesCmd::RemoveTrip { id } => {
            sqlx::query("UPDATE trips SET state = 0 WHERE id = ?").bind(id).execute(pool).await?;
            let mut s = lock();
            if s.open == id {
                s.open = 0;
            }
        }
        PlacesCmd::AddWish { name, note } => {
            if name.trim().is_empty() {
                bail!("a place needs a name");
            }
            sqlx::query("INSERT INTO wishes (name, note, created) VALUES (?, ?, ?)")
                .bind(name.trim())
                .bind(note.trim())
                .bind(p::now())
                .execute(pool)
                .await?;
        }
        PlacesCmd::RemoveWish { id } => {
            sqlx::query("DELETE FROM wishes WHERE id = ?").bind(id).execute(pool).await?;
        }
        PlacesCmd::SetHome { lat, lon } => {
            let v = if lat == 0.0 && lon == 0.0 { String::new() } else { format!("{lat:.5},{lon:.5}") };
            crate::api::shell::put("places.home", &v);
        }
        PlacesCmd::SetRules { min_km, gap_days, min_photos } => {
            let v = format!("{},{},{}", min_km.clamp(10, 2000), gap_days.clamp(0, 30), min_photos.clamp(2, 1000));
            crate::api::shell::put("places.rules", &v);
        }
        PlacesCmd::GetPlaceNames => {
            let n = j::download_place_names(&crate::feeds::client()).await?;
            *towns_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
            say(format!("{n} towns and cities known — trips and places have names now"));
        }
        PlacesCmd::ToJournal { id } => {
            let v = trip_view(pool, id).await?.ok_or_else(|| anyhow!("that trip is gone"))?;
            let day = v.days.first().and_then(|d| NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()).ok_or_else(|| anyhow!("the trip has no days"))?;
            let mut body = format!("**{}** · {} · {} days", v.title, v.range, v.days_n);
            if !v.stops.is_empty() {
                body.push_str(&format!("\n\n{}", v.stops.join(" → ")));
            }
            if !v.note.is_empty() {
                body.push_str(&format!("\n\n{}", v.note));
            }
            let jp = crate::db::journal_pool().await?;
            crate::api::journal::new_entry(jp, day, &body).await?;
            say(format!("Written up in Journal on {}", day.format("%-d %B %Y")));
        }
    }
    Ok(())
}

async fn keep(pool: &SqlitePool, a: &Analysis, start: i64, end: i64) -> Result<i64> {
    let title = title_of(a, start, end);
    let id = sqlx::query("INSERT INTO trips (title, start, end, state, created) VALUES (?, ?, ?, 1, ?)")
        .bind(title)
        .bind(start)
        .bind(end)
        .bind(p::now())
        .execute(pool)
        .await?
        .last_insert_rowid();
    Ok(id)
}

fn plural(n: i64, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// Every trip row, kept and dismissed: (id, title, start, end, state, note).
async fn stored(pool: &SqlitePool) -> Result<Vec<(i64, String, i64, i64, i64, String)>> {
    Ok(sqlx::query_as("SELECT id, title, start, end, state, note FROM trips ORDER BY start DESC").fetch_all(pool).await?)
}

fn overlaps(rows: &[(i64, String, i64, i64, i64, String)], start: i64, end: i64) -> bool {
    rows.iter().any(|r| start <= r.3 && end >= r.2)
}

// ---------------------------------------------------------------- analysis ---

/// Everything worked out from the photos, kept until they change.
struct Analysis {
    key: String,
    pts: Vec<Pt>,
    photos: i64,
    home: Option<(f64, f64)>,
    home_auto: bool,
    rules: Rules,
    found: Vec<Found>,
    towns: Arc<Towns>,
    /// Each photo's town, by index into `towns.list`, parallel to `pts`.
    town_of: Vec<Option<usize>>,
}

/// `frb(ignore)`: private state, not part of the contract.
#[flutter_rust_bridge::frb(ignore)]
#[derive(Default)]
struct Towns {
    list: Vec<Town>,
    countries: HashMap<String, String>,
    regions: HashMap<String, String>,
}

fn towns_cache() -> &'static Mutex<Option<Arc<Towns>>> {
    static T: OnceLock<Mutex<Option<Arc<Towns>>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}

/// The town table with countries and states, read once. Empty until it has
/// been downloaded — or when an older download kept names only.
fn towns() -> Arc<Towns> {
    let mut g = towns_cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = g.as_ref() {
        return t.clone();
    }
    let read = |f: Option<PathBuf>| f.and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
    let file = j::places_file();
    let dir = file.as_ref().and_then(|f| f.parent().map(|d| d.to_path_buf()));
    let list: Vec<Town> = read(file)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some(Town {
                name: f.first()?.to_string(),
                lat: f.get(1)?.parse().ok()?,
                lon: f.get(2)?.parse().ok()?,
                cc: f.get(3)?.to_string(),
                admin1: f.get(4).unwrap_or(&"").to_string(),
            })
        })
        .collect();
    let pairs = |name: &str| -> HashMap<String, String> {
        read(dir.as_ref().map(|d| d.join(name)))
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.to_string(), b.to_string())))
            .collect()
    };
    let t = Arc::new(Towns { list, countries: pairs("countries.tsv"), regions: pairs("regions.tsv") });
    *g = Some(t.clone());
    t
}

fn analysis_cache() -> &'static Mutex<Option<Arc<Analysis>>> {
    static A: OnceLock<Mutex<Option<Arc<Analysis>>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(None))
}

fn rules() -> Rules {
    let s = crate::api::shell::load().text("places.rules");
    let n: Vec<i64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    match n.as_slice() {
        [a, b, c] => Rules { min_km: *a as f64, gap_days: *b, min_photos: *c as usize },
        _ => Rules::default(),
    }
}

fn home_setting() -> Option<(f64, f64)> {
    let s = crate::api::shell::load().text("places.home");
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The photos' locations and what follows from them, recomputed only when the
/// photos, home, the rules or the town table change.
async fn analysis(_pool: &SqlitePool) -> Result<Arc<Analysis>> {
    let photos = crate::db::photos_pool().await?;
    let (count, max_id, located): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(MAX(i.id), 0), \
                COALESCE(SUM(pm.gps_lat IS NOT NULL AND pm.gps_lon IS NOT NULL AND pm.taken_at IS NOT NULL), 0) \
         FROM photo_meta pm JOIN items i ON i.id = pm.item_id \
         WHERE i.missing_since IS NULL AND pm.deleted_at IS NULL",
    )
    .fetch_one(photos)
    .await?;
    let set_home = home_setting();
    let rules = rules();
    let towns = towns();
    let key = format!("{count}/{max_id}/{located}/{set_home:?}/{rules:?}/{}", towns.list.len());
    if let Some(a) = analysis_cache().lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && a.key == key
    {
        return Ok(a.clone());
    }
    let rows: Vec<(i64, i64, f64, f64)> = sqlx::query_as(
        "SELECT i.id, pm.taken_at, pm.gps_lat, pm.gps_lon FROM photo_meta pm JOIN items i ON i.id = pm.item_id \
         WHERE pm.taken_at IS NOT NULL AND pm.gps_lat IS NOT NULL AND pm.gps_lon IS NOT NULL \
           AND i.missing_since IS NULL AND pm.deleted_at IS NULL \
           AND NOT (pm.gps_lat = 0 AND pm.gps_lon = 0) \
         ORDER BY pm.taken_at",
    )
    .fetch_all(photos)
    .await?;
    let a = tokio::task::spawn_blocking(move || {
        let pts: Vec<Pt> = rows.into_iter().map(|(id, ts, lat, lon)| Pt { id, ts, lat, lon }).collect();
        let home = set_home.or_else(|| p::home_of(&pts));
        let found = home.map(|h| p::find_trips(&pts, h, rules)).unwrap_or_default();
        // Photos a street apart share a town: look each cell up once.
        let mut by_cell: HashMap<(i64, i64), Option<usize>> = HashMap::new();
        let town_of: Vec<Option<usize>> = if towns.list.is_empty() {
            vec![None; pts.len()]
        } else {
            pts.iter()
                .map(|x| {
                    let c = ((x.lat * 20.0).floor() as i64, (x.lon * 20.0).floor() as i64);
                    *by_cell.entry(c).or_insert_with(|| p::town_index(&towns.list, x.at()))
                })
                .collect()
        };
        Analysis { key, pts, photos: count, home, home_auto: set_home.is_none(), rules, found, towns, town_of }
    })
    .await?;
    let a = Arc::new(a);
    *analysis_cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(a.clone());
    Ok(a)
}

impl Analysis {
    fn town(&self, i: usize) -> Option<&Town> {
        self.town_of.get(i).copied().flatten().and_then(|t| self.towns.list.get(t))
    }

    /// Indices into `pts` between two times.
    fn within(&self, start: i64, end: i64) -> std::ops::Range<usize> {
        let a = self.pts.partition_point(|x| x.ts < start);
        let b = self.pts.partition_point(|x| x.ts <= end);
        a..b
    }

    fn home_cc(&self) -> String {
        self.home.and_then(|h| p::town_at(&self.towns.list, h)).map(|t| t.cc.clone()).unwrap_or_default()
    }
}

fn title_of(a: &Analysis, start: i64, end: i64) -> String {
    let mut n: HashMap<String, usize> = HashMap::new();
    for i in a.within(start, end) {
        if let Some(t) = a.town(i) {
            *n.entry(t.name.clone()).or_default() += 1;
        }
    }
    let t = p::title_from(&n.into_iter().collect::<Vec<_>>());
    if t.is_empty() { format!("Trip, {}", local(start).format("%B %Y")) } else { t }
}

// ---------------------------------------------------------------- snapshot ---

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

fn range(start: i64, end: i64) -> String {
    let (a, b) = (local(start).date_naive(), local(end).date_naive());
    if a == b {
        a.format("%-d %B %Y").to_string()
    } else if a.year() == b.year() && a.month() == b.month() {
        format!("{}–{}", a.format("%-d"), b.format("%-d %B %Y"))
    } else if a.year() == b.year() {
        format!("{} – {}", a.format("%-d %b"), b.format("%-d %b %Y"))
    } else {
        format!("{} – {}", a.format("%-d %b %Y"), b.format("%-d %b %Y"))
    }
}

fn days_of(start: i64, end: i64) -> i64 {
    (local(end).date_naive() - local(start).date_naive()).num_days() + 1
}

fn card(a: &Analysis, id: i64, title: String, start: i64, end: i64) -> TripCard {
    let r = a.within(start, end);
    let idx: Vec<usize> = r.clone().collect();
    let cover: Vec<i64> = match idx.len() {
        0 => Vec::new(),
        1 | 2 => idx.iter().map(|&i| a.pts[i].id).collect(),
        n => vec![a.pts[idx[0]].id, a.pts[idx[n / 2]].id, a.pts[idx[n - 1]].id],
    };
    let ccs: BTreeSet<&str> = idx.iter().filter_map(|&i| a.town(i)).map(|t| t.cc.as_str()).collect();
    let stops = p::route(&a.pts[r]);
    TripCard {
        id,
        title,
        start,
        end,
        range: range(start, end),
        year: local(start).year() as i64,
        days: days_of(start, end),
        photos: idx.len() as i64,
        cover,
        flags: ccs.into_iter().map(p::flag).collect(),
        km: p::route_km(&stops).round() as i64,
    }
}

async fn trip_view(pool: &SqlitePool, id: i64) -> Result<Option<TripView>> {
    let row: Option<(String, i64, i64, String)> =
        sqlx::query_as("SELECT title, start, end, note FROM trips WHERE id = ? AND state = 1").bind(id).fetch_optional(pool).await?;
    let Some((title, start, end, note)) = row else { return Ok(None) };
    let a = analysis(pool).await?;
    let r = a.within(start, end);
    let pts = &a.pts[r.clone()];
    let stops_at = p::route(pts);
    // Towns in the order they came, each once.
    let mut stops: Vec<String> = Vec::new();
    for i in r.clone() {
        if let Some(t) = a.town(i)
            && !stops.contains(&t.name)
        {
            stops.push(t.name.clone());
        }
    }
    let furthest = a
        .home
        .and_then(|h| r.clone().map(|i| (km(h, a.pts[i].at()), i)).max_by(|x, y| x.0.total_cmp(&y.0)))
        .map(|(d, i)| format!("{}{} km from home", a.town(i).map(|t| format!("{}, ", t.name)).unwrap_or_default(), d.round()))
        .unwrap_or_default();

    let first = local(start).date_naive();
    let last = local(end).date_naive();
    let journal_on = tulipix_core::paths::db_path("journal").is_some_and(|f| f.exists());
    let mut days = Vec::new();
    let mut d = first;
    let mut n = 1;
    while d <= last && n <= 60 {
        let (rows, _) = j::gather(d).await;
        let photos = j::day_photos(d).await;
        let (a0, b0) = j::bounds(d);
        let mut names: Vec<String> = Vec::new();
        for i in a.within(a0, b0 - 1) {
            if let Some(t) = a.town(i)
                && names.last() != Some(&t.name)
            {
                names.push(t.name.clone());
            }
        }
        let quote = if journal_on { quote_for(d).await } else { String::new() };
        days.push(TripDay {
            date: d.format("%Y-%m-%d").to_string(),
            label: format!("Day {n}"),
            dow: d.format("%a %-d %b").to_string(),
            towns: names.join(" → "),
            photo_n: photos.len() as i64,
            photos: photos.iter().take(12).map(|x| x.0).collect(),
            lines: rows
                .into_iter()
                .filter(|g| g.source != "photos")
                .map(|g| DayLine { source: g.source.to_string(), text: g.title })
                .collect(),
            quote,
        });
        d = d.succ_opt().unwrap_or(d);
        n += 1;
    }
    Ok(Some(TripView {
        id,
        title,
        range: range(start, end),
        days_n: days_of(start, end),
        photos: days.iter().map(|x| x.photo_n).sum(),
        km: p::route_km(&stops_at).round() as i64,
        stops,
        route: stops_at.iter().map(|&(lat, lon)| GeoPoint { lat, lon, n: 1 }).collect(),
        days,
        note,
        furthest,
    }))
}

/// The day's first Journal words, up to a couple of sentences.
async fn quote_for(day: NaiveDate) -> String {
    let Ok(jp) = crate::db::journal_pool().await else { return String::new() };
    let body: Option<String> = sqlx::query_scalar("SELECT body FROM entries WHERE day = ? AND body != '' ORDER BY created LIMIT 1")
        .bind(day.format("%Y-%m-%d").to_string())
        .fetch_optional(jp)
        .await
        .ok()
        .flatten();
    body.map(|b| crate::feeds::clip(b.replace(['*', '#', '_'], "").trim(), 220)).unwrap_or_default()
}

async fn snapshot(pool: &SqlitePool) -> Result<PlacesState> {
    let (tab, open, notice) = {
        let mut s = lock();
        (s.tab.clone(), s.open, std::mem::take(&mut s.notice))
    };
    let a = analysis(pool).await?;
    let rows = stored(pool).await?;

    let trips: Vec<TripCard> = rows
        .iter()
        .filter(|r| r.4 == 1)
        .map(|r| card(&a, r.0, if r.1.is_empty() { title_of(&a, r.2, r.3) } else { r.1.clone() }, r.2, r.3))
        .collect();
    let mut suggested: Vec<TripCard> = a
        .found
        .iter()
        .filter(|f| !overlaps(&rows, f.start, f.end))
        .map(|f| card(&a, 0, title_of(&a, f.start, f.end), f.start, f.end))
        .collect();
    suggested.sort_by(|x, y| y.start.cmp(&x.start));

    let open = if open > 0 { trip_view(pool, open).await? } else { None };
    if open.is_none() && lock().open != 0 {
        lock().open = 0;
    }

    // The map.
    let points: Vec<GeoPoint> =
        p::cells(&a.pts, 0.1).into_iter().take(3000).map(|(lat, lon, n)| GeoPoint { lat, lon, n }).collect();
    let routes = rows
        .iter()
        .filter(|r| r.4 == 1)
        .map(|r| MapRoute {
            trip_id: r.0,
            title: r.1.clone(),
            points: p::route(&a.pts[a.within(r.2, r.3)]).into_iter().map(|(lat, lon)| GeoPoint { lat, lon, n: 1 }).collect(),
        })
        .collect();
    let map = PlacesMap {
        points,
        routes,
        home: a.home.map(|(lat, lon)| GeoPoint { lat, lon, n: 0 }),
        first_year: a.pts.first().map(|x| local(x.ts).year() as i64).unwrap_or(0),
        last_year: a.pts.last().map(|x| local(x.ts).year() as i64).unwrap_or(0),
    };

    // Towns.
    let home_town = a.home.and_then(|h| p::town_at(&a.towns.list, h)).map(|t| t.name.clone()).unwrap_or_default();
    struct Agg {
        photos: i64,
        days: BTreeSet<NaiveDate>,
        first: i64,
        last: i64,
    }
    let mut by: BTreeMap<usize, Agg> = BTreeMap::new();
    for (i, x) in a.pts.iter().enumerate() {
        let Some(t) = a.town_of[i] else { continue };
        let e = by.entry(t).or_insert(Agg { photos: 0, days: BTreeSet::new(), first: x.ts, last: x.ts });
        e.photos += 1;
        e.days.insert(local(x.ts).date_naive());
        e.last = x.ts;
    }
    let region_name = |t: &Town| a.towns.regions.get(&format!("{}.{}", t.cc, t.admin1)).cloned().unwrap_or_default();
    let country_name = |cc: &str| a.towns.countries.get(cc).cloned().unwrap_or_else(|| cc.to_string());
    let mut towns: Vec<TownRow> = by
        .iter()
        .map(|(&ti, g)| {
            let t = &a.towns.list[ti];
            TownRow {
                name: t.name.clone(),
                region: region_name(t),
                country: country_name(&t.cc),
                flag: p::flag(&t.cc),
                photos: g.photos,
                days: g.days.len() as i64,
                first: local(g.first).format("%b %Y").to_string(),
                last: local(g.last).format("%b %Y").to_string(),
                home: t.name == home_town,
                lat: t.lat,
                lon: t.lon,
            }
        })
        .collect();
    towns.sort_by(|x, y| y.days.cmp(&x.days).then(y.photos.cmp(&x.photos)));

    // Been.
    let mut countries: BTreeMap<String, (i64, BTreeSet<String>, i64)> = BTreeMap::new();
    let mut regions: BTreeSet<String> = BTreeSet::new();
    for (&ti, g) in &by {
        let t = &a.towns.list[ti];
        if t.cc.is_empty() {
            continue;
        }
        let e = countries.entry(t.cc.clone()).or_insert((0, BTreeSet::new(), i64::MAX));
        e.0 += g.photos;
        e.1.insert(t.admin1.clone());
        e.2 = e.2.min(local(g.first).year() as i64);
        let rn = region_name(t);
        if !rn.is_empty() {
            regions.insert(format!("{rn}, {}", country_name(&t.cc)));
        }
    }
    let mut countries: Vec<CountryRow> = countries
        .into_iter()
        .map(|(cc, (photos, regs, first_year))| CountryRow {
            name: country_name(&cc),
            flag: p::flag(&cc),
            photos,
            regions: regs.len() as i64,
            first_year,
            cc,
        })
        .collect();
    countries.sort_by(|x, y| y.photos.cmp(&x.photos));
    let kept: Vec<&TripCard> = trips.iter().collect();
    let mut years: BTreeMap<i64, i64> = BTreeMap::new();
    for t in &kept {
        *years.entry(t.year).or_default() += t.days;
    }
    let mut firsts = Vec::new();
    let home_cc = a.home_cc();
    if let Some(t) = kept.iter().rev().find(|t| {
        a.within(t.start, t.end).any(|i| a.town(i).is_some_and(|x| !x.cc.is_empty() && x.cc != home_cc))
    }) {
        firsts.push(FirstRow { label: format!("First trip abroad {}", t.flags), value: format!("{} · {}", t.title, local(t.start).format("%b %Y")) });
    }
    if let Some(h) = a.home
        && let Some((d, i)) = a.pts.iter().enumerate().map(|(i, x)| (km(h, x.at()), i)).max_by(|x, y| x.0.total_cmp(&y.0))
    {
        let place = a.town(i).map(|t| format!("{} {}", t.name, p::flag(&t.cc))).unwrap_or_else(|| "Somewhere unnamed".into());
        firsts.push(FirstRow { label: "Furthest from home".into(), value: format!("{place} · {} km", d.round()) });
    }
    if let Some(t) = kept.iter().max_by_key(|t| t.days) {
        firsts.push(FirstRow { label: "Longest trip".into(), value: format!("{} · {} days", t.title, t.days) });
    }
    if let Some(t) = towns.iter().find(|t| !t.home) {
        firsts.push(FirstRow { label: "Most visited".into(), value: format!("{} {} · {} days", t.name, t.flag, t.days) });
    }
    let been = Been {
        countries,
        regions: regions.into_iter().collect(),
        towns: towns.len() as i64,
        km: kept.iter().map(|t| t.km).sum(),
        days_away: kept.iter().map(|t| t.days).sum(),
        years: years.into_iter().map(|(year, days)| YearBar { year, days }).collect(),
        firsts,
    };

    let wishes: Vec<Wish> = sqlx::query_as::<_, (i64, String, String)>("SELECT id, name, note FROM wishes ORDER BY created DESC")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, name, note)| Wish { id, name, note })
        .collect();

    Ok(PlacesState {
        tab,
        notice,
        located: a.pts.len() as i64,
        photos: a.photos,
        named: a.towns.list.iter().any(|t| !t.cc.is_empty()),
        home: match a.home {
            None => String::new(),
            Some(h) => p::town_at(&a.towns.list, h).map(|t| t.name.clone()).unwrap_or_else(|| format!("{:.2}, {:.2}", h.0, h.1)),
        },
        home_auto: a.home_auto,
        trips,
        suggested,
        open,
        map,
        towns,
        wishes,
        been,
        min_km: a.rules.min_km as i64,
        gap_days: a.rules.gap_days,
        min_photos: a.rules.min_photos as i64,
    })
}
