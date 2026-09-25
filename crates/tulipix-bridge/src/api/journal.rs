// The Journal section: a private daily log that half-writes itself.
//
// Four tabs over one snapshot — Today (the day's entries, its voice notes and
// what the other sections saw of it), Calendar, On this day and Insights. The
// store and the gathering are `crate::journal`; this file keeps the session
// (which tab, which day, which month) and maps rows into what Dart draws.
//
// Voice notes are recorded by whatever recorder the desktop has (the same
// list the Slint voice search tries) and transcribed by the bundled
// whisper-cli, here, never over the network.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate, TimeZone};
use sqlx::SqlitePool;

use crate::db::journal_pool;
use crate::journal::{self as j, PROMPTS};

// ------------------------------------------------------------------- state ---

pub struct JournalState {
    /// today | calendar | otd | insights.
    pub tab: String,
    /// The day Today shows, ISO.
    pub day: String,
    /// "Saturday, 19 September".
    pub day_title: String,
    /// "Bristol · 2 entries".
    pub day_sub: String,
    /// "18° and clear" for the day shown; "" when off, unknown or not yet in.
    pub weather: String,
    /// Settings: ask Open-Meteo for each day's weather.
    pub weather_on: bool,
    pub is_today: bool,
    pub entries: Vec<EntryView>,
    /// Said once: the entry just made, for the page to put the cursor in.
    pub focus: i64,
    pub gathered: Vec<GatherRow>,
    /// Every photo taken that day, for the Photo picker.
    pub day_photos: Vec<i64>,
    pub places: PlacesView,
    /// The viewed day in other years, newest first.
    pub otd: Vec<OtdCard>,
    pub prompt: String,
    pub streak: i64,
    pub longest: i64,
    /// "in March", "in March 2025"; empty with no run yet.
    pub longest_when: String,
    pub month: MonthView,
    pub insights: Insights,
    pub query: String,
    pub results: Vec<JournalHit>,
    /// Places named before, most used first, for the Place dialog.
    pub known_places: Vec<String>,
    pub known_tags: Vec<String>,
    /// The entry being recorded into; 0 when nothing is.
    pub recording: i64,
    pub notice: String,
}

pub struct EntryView {
    pub id: i64,
    pub created: i64,
    pub body: String,
    pub words: i64,
    /// 0 unset, 1 rough … 5 great.
    pub mood: i64,
    pub place: String,
    pub tags: Vec<String>,
    pub photos: Vec<i64>,
    pub voice: Vec<VoiceView>,
}

pub struct VoiceView {
    pub id: i64,
    pub path: String,
    pub duration_s: f64,
    /// 0..1, left to right.
    pub peaks: Vec<f64>,
    pub transcript: String,
    /// new (not transcribed yet) | done | failed.
    pub state: String,
}

pub struct GatherRow {
    /// photos | music | finances | videos | books | kitchen | feeds | papers.
    pub source: String,
    /// "08:12"; empty when the section keeps no time.
    pub clock: String,
    pub title: String,
    /// Photo ids to show as thumbnails.
    pub ids: Vec<i64>,
    pub shown: bool,
}

#[derive(Default)]
pub struct PlacesView {
    /// Stops fitted into a unit square, in the order visited.
    pub points: Vec<MapPoint>,
    pub stops: i64,
    pub km: f64,
    /// The places the day's entries name — or, when they name none, the
    /// towns the stops were in.
    pub names: Vec<String>,
    /// A town for each stop, "" where none is near or the table is not here.
    pub labels: Vec<String>,
    /// There are stops to name and the place table has not been fetched.
    pub can_name: bool,
}

pub struct MapPoint {
    pub x: f64,
    pub y: f64,
}

pub struct OtdCard {
    pub day: String,
    pub years_ago: i64,
    pub year: i64,
    /// Something was written that day; otherwise [text] is what the other
    /// sections say about it.
    pub written: bool,
    pub text: String,
    pub mood: i64,
    /// "Clevedon", "Bonobo", "12 photos", "£212 spent".
    pub meta: Vec<String>,
    pub photos: Vec<i64>,
}

#[derive(Default)]
pub struct MonthView {
    /// The first of the month, ISO.
    pub first: String,
    /// "September 2026".
    pub title: String,
    /// photos | mood | words.
    pub mode: String,
    /// Monday 0 … Sunday 6.
    pub lead: i64,
    pub cells: Vec<DayCell>,
    /// "17 of 19 days written · 4,812 words".
    pub summary: String,
}

pub struct DayCell {
    pub day: String,
    pub n: i64,
    pub words: i64,
    pub mood: i64,
    /// First photo of the day; 0 for none.
    pub photo: i64,
    pub written: bool,
    pub future: bool,
    pub today: bool,
}

#[derive(Default)]
pub struct Insights {
    pub words_month: i64,
    /// Against the same days of last month, in percent.
    pub words_change: i64,
    pub has_change: bool,
    pub days_written: i64,
    pub days_so_far: i64,
    pub streak: i64,
    pub new_places: i64,
    pub top_place: String,
    pub photos_kept: i64,
    pub photos_taken: i64,
    /// The last 30 days, oldest first, 0 for no mood.
    pub moods: Vec<i64>,
    pub mood_line: String,
    pub cards: Vec<InsightCard>,
    pub tags: Vec<TagCount>,
}

pub struct InsightCard {
    /// music | photos — which icon.
    pub kind: String,
    pub title: String,
    pub body: String,
}

pub struct TagCount {
    pub tag: String,
    pub n: i64,
}

pub struct JournalHit {
    pub entry_id: i64,
    pub day: String,
    /// "Sat 19 Sep 2026".
    pub label: String,
    pub excerpt: String,
    pub mood: i64,
}

// ---------------------------------------------------------------- commands ---

pub enum JournalCmd {
    /// The snapshot, from the databases only.
    Refresh,
    SetTab { tab: String },
    /// Show a day on Today.
    Go { day: String },
    /// A day back or forward from the one shown; not past today.
    Step { delta: i64 },
    /// A new entry on the day shown, unless an empty one is already there.
    NewEntry,
    /// A new entry on another day, from On this day's "Write it now".
    WriteOn { day: String },
    SaveBody { id: i64, body: String },
    SetMood { id: i64, mood: i64 },
    SetPlace { id: i64, place: String },
    AddTag { id: i64, tag: String },
    RemoveTag { id: i64, tag: String },
    KeepPhoto { id: i64, photo: i64, keep: bool },
    DeleteEntry { id: i64 },
    /// Switch a gathered row on or off for the day shown.
    SetShown { source: String, shown: bool },
    SetMonth { delta: i64 },
    SetCalMode { mode: String },
    Search { text: String },
    NextPrompt,
    /// Today's question as the first line of a new entry.
    Answer,
    RecordStart { id: i64 },
    /// Stop and keep the note. Transcription is `journal_transcribe`, which
    /// takes long enough to want its own call.
    RecordStop,
    RecordCancel,
    DeleteVoice { id: i64 },
    /// Weather on each day, from Open-Meteo, or not.
    SetWeather { on: bool },
    /// Fetch the town table once, so stops have names.
    GetPlaceNames,
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    /// None follows the calendar, so a page left open overnight moves on.
    day: Option<NaiveDate>,
    month: Option<NaiveDate>,
    cal_mode: String,
    query: String,
    prompt: usize,
    focus: i64,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "today".into(),
            day: None,
            month: None,
            cal_mode: "photos".into(),
            query: String::new(),
            // A different question each day, not the same one every launch.
            prompt: j::today().num_days_from_ce() as usize % PROMPTS.len(),
            focus: 0,
            notice: String::new(),
        })
    })
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

fn viewed() -> NaiveDate {
    lock().day.unwrap_or_else(j::today)
}

// ---------------------------------------------------------------- exported ---

pub async fn journal_dispatch(cmd: JournalCmd) -> Result<JournalState> {
    let pool = journal_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// Every entry as Markdown under `folder`, one file a day at
/// `YYYY/MM-DD.md`: each entry's time, mood, place and tags, its words, and
/// its voice notes' transcripts. With `photos`, the kept photos are copied to
/// `YYYY/MM-DD/` and shown in the file. Plain files any editor opens — the
/// way out, and a backup that needs no Tulipix. Returns the days written.
pub async fn journal_export(folder: String, photos: bool) -> Result<i64> {
    const MOODS: [&str; 6] = ["", "rough", "low", "okay", "good", "great"];
    let pool = journal_pool().await?;
    let root = PathBuf::from(folder.trim());
    if folder.trim().is_empty() {
        bail!("choose a folder to export to");
    }
    let entries: Vec<(i64, String, i64, String, i64, String)> =
        sqlx::query_as("SELECT id, day, created, body, mood, place FROM entries ORDER BY day, created").fetch_all(pool).await?;
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for (id, tag) in sqlx::query_as::<_, (i64, String)>("SELECT entry_id, tag FROM entry_tags ORDER BY tag").fetch_all(pool).await? {
        tags.entry(id).or_default().push(tag);
    }
    let mut voice: HashMap<i64, Vec<String>> = HashMap::new();
    for (id, text) in sqlx::query_as::<_, (i64, String)>(
        "SELECT entry_id, transcript FROM voice_notes WHERE transcript != '' ORDER BY created",
    )
    .fetch_all(pool)
    .await?
    {
        voice.entry(id).or_default().push(text);
    }
    let mut kept: HashMap<i64, Vec<String>> = HashMap::new();
    if photos && let Ok(pp) = crate::db::photos_pool().await {
        for (id, photo) in sqlx::query_as::<_, (i64, i64)>("SELECT entry_id, photo_id FROM entry_photos").fetch_all(pool).await? {
            let path: Option<String> =
                sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?").bind(photo).fetch_optional(pp).await.ok().flatten();
            if let Some(path) = path {
                kept.entry(id).or_default().push(path);
            }
        }
    }

    let mut days: BTreeMap<String, Vec<&(i64, String, i64, String, i64, String)>> = BTreeMap::new();
    for e in entries.iter().filter(|e| !e.3.trim().is_empty() || voice.contains_key(&e.0) || kept.contains_key(&e.0)) {
        days.entry(e.1.clone()).or_default().push(e);
    }
    for (day, list) in &days {
        let Ok(d) = day.parse::<NaiveDate>() else { continue };
        let dir = root.join(d.format("%Y").to_string());
        tokio::fs::create_dir_all(&dir).await?;
        let stem = d.format("%m-%d").to_string();
        let mut md = format!("# {}\n", d.format("%A, %-d %B %Y"));
        for (id, _, created, body, mood, place) in list.iter().map(|e| (&e.0, &e.1, &e.2, &e.3, &e.4, &e.5)) {
            let clock = chrono::Local.timestamp_opt(*created, 0).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default();
            let mut meta = vec![clock];
            if let Some(m) = MOODS.get(*mood as usize).filter(|m| !m.is_empty()) {
                meta.push(format!("feeling {m}"));
            }
            if !place.is_empty() {
                meta.push(place.clone());
            }
            if let Some(t) = tags.get(id) {
                meta.push(t.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" "));
            }
            md.push_str(&format!("\n## {}\n\n{}\n", meta.join(" · "), body.trim()));
            for t in voice.get(id).into_iter().flatten() {
                md.push_str(&format!("\n> 🎙 {}\n", t.trim().replace('\n', "\n> ")));
            }
            for src in kept.get(id).into_iter().flatten() {
                let src = std::path::Path::new(src);
                let Some(name) = src.file_name() else { continue };
                let pics = dir.join(&stem);
                tokio::fs::create_dir_all(&pics).await?;
                if tokio::fs::copy(src, pics.join(name)).await.is_ok() {
                    md.push_str(&format!("\n![]({stem}/{})\n", name.to_string_lossy().replace(' ', "%20")));
                }
            }
        }
        tokio::fs::write(dir.join(format!("{stem}.md")), md).await.with_context(|| format!("could not write {day}"))?;
    }
    Ok(days.len() as i64)
}

/// Transcribe a voice note with whisper-cli, on this computer.
pub async fn journal_transcribe(id: i64) -> Result<JournalState> {
    let pool = journal_pool().await?;
    let path: String = sqlx::query_scalar("SELECT path FROM voice_notes WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    match transcribe(&PathBuf::from(&path)).await {
        Ok(text) => {
            sqlx::query("UPDATE voice_notes SET transcript = ?, state = 'done' WHERE id = ?")
                .bind(text)
                .bind(id)
                .execute(pool)
                .await?;
        }
        Err(e) => {
            sqlx::query("UPDATE voice_notes SET state = 'failed' WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            say(format!("Kept the recording; it could not be transcribed — {e}"));
        }
    }
    snapshot(pool).await
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: JournalCmd) -> Result<()> {
    match cmd {
        JournalCmd::Refresh => {}
        JournalCmd::SetTab { tab } => {
            let mut s = lock();
            s.tab = tab;
            s.query.clear();
        }
        JournalCmd::Go { day } => {
            let d = j::parse_day(&day).ok_or_else(|| anyhow!("not a date: {day}"))?;
            let mut s = lock();
            s.day = if d >= j::today() { None } else { Some(d) };
            s.tab = "today".into();
            s.query.clear();
        }
        JournalCmd::Step { delta } => {
            let d = j::shift(viewed(), delta);
            lock().day = if d >= j::today() { None } else { Some(d) };
        }
        JournalCmd::NewEntry => {
            let id = new_entry(pool, viewed(), "").await?;
            lock().focus = id;
        }
        JournalCmd::WriteOn { day } => {
            let d = j::parse_day(&day).ok_or_else(|| anyhow!("not a date: {day}"))?;
            let id = new_entry(pool, d, "").await?;
            let mut s = lock();
            s.day = if d >= j::today() { None } else { Some(d) };
            s.tab = "today".into();
            s.focus = id;
        }
        JournalCmd::SaveBody { id, body } => {
            sqlx::query("UPDATE entries SET body = ?, words = ?, updated = ? WHERE id = ?")
                .bind(&body)
                .bind(j::words(&body))
                .bind(j::now())
                .bind(id)
                .execute(pool)
                .await?;
        }
        JournalCmd::SetMood { id, mood } => {
            sqlx::query("UPDATE entries SET mood = ?, updated = ? WHERE id = ?")
                .bind(mood.clamp(0, 5))
                .bind(j::now())
                .bind(id)
                .execute(pool)
                .await?;
        }
        JournalCmd::SetPlace { id, place } => {
            sqlx::query("UPDATE entries SET place = ?, updated = ? WHERE id = ?")
                .bind(place.trim())
                .bind(j::now())
                .bind(id)
                .execute(pool)
                .await?;
        }
        JournalCmd::AddTag { id, tag } => {
            let tag = clean_tag(&tag);
            if tag.is_empty() {
                bail!("a tag needs a letter or a number");
            }
            sqlx::query("INSERT OR IGNORE INTO entry_tags (entry_id, tag) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(pool)
                .await?;
        }
        JournalCmd::RemoveTag { id, tag } => {
            sqlx::query("DELETE FROM entry_tags WHERE entry_id = ? AND tag = ?")
                .bind(id)
                .bind(tag)
                .execute(pool)
                .await?;
        }
        JournalCmd::KeepPhoto { id, photo, keep } => {
            let sql = if keep {
                "INSERT OR IGNORE INTO entry_photos (entry_id, photo_id) VALUES (?, ?)"
            } else {
                "DELETE FROM entry_photos WHERE entry_id = ? AND photo_id = ?"
            };
            sqlx::query(sql).bind(id).bind(photo).execute(pool).await?;
        }
        JournalCmd::DeleteEntry { id } => {
            let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM voice_notes WHERE entry_id = ?")
                .bind(id)
                .fetch_all(pool)
                .await?;
            sqlx::query("DELETE FROM entries WHERE id = ?").bind(id).execute(pool).await?;
            for p in paths {
                std::fs::remove_file(p).ok();
            }
        }
        JournalCmd::SetShown { source, shown } => {
            let day = j::iso(viewed());
            let sql = if shown {
                "DELETE FROM hidden WHERE day = ? AND source = ?"
            } else {
                "INSERT OR IGNORE INTO hidden (day, source) VALUES (?, ?)"
            };
            sqlx::query(sql).bind(day).bind(source).execute(pool).await?;
        }
        JournalCmd::SetMonth { delta } => {
            let mut s = lock();
            let m = s.month.unwrap_or_else(|| j::month_start(j::today()));
            let m = if delta >= 0 {
                m.checked_add_months(chrono::Months::new(delta as u32))
            } else {
                m.checked_sub_months(chrono::Months::new(delta.unsigned_abs() as u32))
            }
            .unwrap_or(m);
            s.month = Some(m);
        }
        JournalCmd::SetCalMode { mode } => lock().cal_mode = mode,
        JournalCmd::Search { text } => lock().query = text.trim().to_string(),
        JournalCmd::NextPrompt => {
            let mut s = lock();
            s.prompt = (s.prompt + 1) % PROMPTS.len();
        }
        JournalCmd::Answer => {
            let q = PROMPTS[lock().prompt % PROMPTS.len()];
            let id = new_entry(pool, j::today(), &format!("{q}\n\n")).await?;
            let mut s = lock();
            s.day = None;
            s.tab = "today".into();
            s.focus = id;
        }
        JournalCmd::RecordStart { id } => record_start(id).await?,
        JournalCmd::RecordStop => record_stop(pool).await?,
        JournalCmd::RecordCancel => {
            if let Some(r) = take_rec() {
                let path = r.path.clone();
                finish(r).await;
                std::fs::remove_file(path).ok();
            }
        }
        JournalCmd::DeleteVoice { id } => {
            let path: Option<String> = sqlx::query_scalar("SELECT path FROM voice_notes WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
            sqlx::query("DELETE FROM voice_notes WHERE id = ?").bind(id).execute(pool).await?;
            if let Some(p) = path {
                std::fs::remove_file(p).ok();
            }
        }
        JournalCmd::SetWeather { on } => {
            crate::api::shell::put("journal.weather", if on { "true" } else { "false" });
            say(if on { "Each day shows its weather" } else { "No more weather" });
        }
        JournalCmd::GetPlaceNames => {
            let n = j::download_place_names(&crate::feeds::client()).await?;
            say(format!("{n} towns and cities to name the places you go"));
        }
    }
    Ok(())
}

/// The day's weather: from journal.db, or asked once and kept. A day not yet
/// over is kept in memory only (its high is not in yet), and so is a failure,
/// so an offline machine asks once a session rather than every refresh.
async fn day_weather(pool: &SqlitePool, day: NaiveDate) -> String {
    static SEEN: Mutex<Option<HashMap<NaiveDate, String>>> = Mutex::new(None);
    if day > j::today() {
        return String::new();
    }
    let kept: Option<String> =
        sqlx::query_scalar("SELECT text FROM weather WHERE day = ?").bind(j::iso(day)).fetch_optional(pool).await.ok().flatten();
    if let Some(w) = kept {
        return w;
    }
    if let Some(w) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&day) {
        return w.clone();
    }
    let got = match j::where_was(day).await {
        Some(at) => j::weather_on(&crate::feeds::client(), day, at).await.unwrap_or_else(|e| {
            tracing::info!(error = %e, "journal: no weather for the day");
            String::new()
        }),
        None => String::new(),
    };
    if !got.is_empty() && day < j::today() {
        sqlx::query("INSERT OR REPLACE INTO weather (day, text) VALUES (?, ?)").bind(j::iso(day)).bind(&got).execute(pool).await.ok();
    }
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(day, got.clone());
    got
}

/// A new entry, or the day's last one when it is still blank — pressing New
/// entry twice should not leave an empty page behind.
async fn new_entry(pool: &SqlitePool, day: NaiveDate, body: &str) -> Result<i64> {
    let blank: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM entries e WHERE day = ? AND words = 0 AND mood = 0 AND body = '' \
           AND NOT EXISTS (SELECT 1 FROM voice_notes v WHERE v.entry_id = e.id) \
         ORDER BY created DESC LIMIT 1",
    )
    .bind(j::iso(day))
    .fetch_optional(pool)
    .await?;
    if let Some(id) = blank {
        if !body.is_empty() {
            sqlx::query("UPDATE entries SET body = ?, words = ?, updated = ? WHERE id = ?")
                .bind(body)
                .bind(j::words(body))
                .bind(j::now())
                .bind(id)
                .execute(pool)
                .await?;
        }
        return Ok(id);
    }
    let now = j::now();
    let id = sqlx::query(
        "INSERT INTO entries (day, created, updated, body, words) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(j::iso(day))
    .bind(now)
    .bind(now)
    .bind(body)
    .bind(j::words(body))
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// "#Lisbon trip" → "lisbon-trip".
fn clean_tag(tag: &str) -> String {
    tag.trim()
        .trim_start_matches('#')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

// --------------------------------------------------------------- recording ---

struct Rec {
    child: tokio::process::Child,
    path: PathBuf,
    entry_id: i64,
    /// ffmpeg stops on `q`; the desktop recorders on SIGINT.
    quits_on_q: bool,
}

fn rec() -> &'static Mutex<Option<Rec>> {
    static R: OnceLock<Mutex<Option<Rec>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

fn take_rec() -> Option<Rec> {
    match rec().lock() {
        Ok(mut g) => g.take(),
        Err(p) => p.into_inner().take(),
    }
}

fn recording_into() -> i64 {
    rec().lock().ok().and_then(|g| g.as_ref().map(|r| r.entry_id)).unwrap_or(0)
}

fn voice_dir() -> Result<PathBuf> {
    let dir = tulipix_core::paths::data_dir().context("no data folder")?.join("journal").join("voice");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A tool from the bundle, then from PATH.
fn tool(name: &str) -> Option<PathBuf> {
    let p = tulipix_core::thumbs::tool_bin(name);
    if p.is_absolute() && p.exists() {
        return Some(p);
    }
    tulipix_common::on_path(name).then(|| PathBuf::from(name))
}

/// The recorders to try, in order, as (program, arguments, stops on `q`).
///
/// Linux: the bundled ffmpeg is static and has no pulse or alsa input, so the
/// desktop's own recorders come first and a system ffmpeg last — the list the
/// Slint voice search tries. macOS and Windows: the bundled ffmpeg has
/// avfoundation and dshow.
async fn recorders(out: &str) -> Vec<(PathBuf, Vec<String>, bool)> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let mut out_list = Vec::new();
    #[cfg(target_os = "linux")]
    {
        for (name, args, q) in [
            ("pw-record", v(&["--rate", "16000", "--channels", "1", "--format", "s16", out]), false),
            (
                "parecord",
                v(&["--rate=16000", "--channels=1", "--format=s16le", "--file-format=wav", out]),
                false,
            ),
            ("arecord", v(&["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", out]), false),
            (
                "ffmpeg",
                v(&["-hide_banner", "-loglevel", "error", "-f", "pulse", "-i", "default", "-ac", "1", "-ar", "16000", "-y", out]),
                true,
            ),
        ] {
            if tulipix_common::on_path(name) {
                out_list.push((PathBuf::from(name), args, q));
            }
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(ff) = tool("ffmpeg") {
        out_list.push((
            ff,
            v(&["-hide_banner", "-loglevel", "error", "-f", "avfoundation", "-i", ":0", "-ac", "1", "-ar", "16000", "-y", out]),
            true,
        ));
    }
    #[cfg(target_os = "windows")]
    if let Some(ff) = tool("ffmpeg") {
        if let Some(dev) = windows_mic(&ff).await {
            out_list.push((
                ff,
                v(&["-hide_banner", "-loglevel", "error", "-f", "dshow", "-i", &dev, "-ac", "1", "-ar", "16000", "-y", out]),
                true,
            ));
        }
    }
    out_list
}

/// The first dshow audio device ffmpeg lists.
#[cfg(target_os = "windows")]
async fn windows_mic(ffmpeg: &std::path::Path) -> Option<String> {
    let out = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .output()
        .await
        .ok()?;
    let s = String::from_utf8_lossy(&out.stderr);
    let line = s.lines().find(|l| l.contains("(audio)"))?;
    let a = line.find('"')?;
    let rest = &line[a + 1..];
    let b = rest.find('"')?;
    Some(format!("audio={}", &rest[..b]))
}

async fn record_start(entry_id: i64) -> Result<()> {
    if recording_into() != 0 {
        bail!("already recording");
    }
    let path = voice_dir()?.join(format!("{}-{entry_id}.wav", j::now()));
    let out = path.to_string_lossy().to_string();
    for (prog, args, q) in recorders(&out).await {
        let spawned = tokio::process::Command::new(&prog)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else { continue };
        // A recorder with no device to open gives up at once; one that is
        // still running after a moment is listening.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        if matches!(child.try_wait(), Ok(Some(_))) {
            std::fs::remove_file(&path).ok();
            continue;
        }
        if let Ok(mut g) = rec().lock() {
            *g = Some(Rec { child, path, entry_id, quits_on_q: q });
        }
        return Ok(());
    }
    bail!("no microphone could be opened — tried the recorders this computer has")
}

/// A few seconds from the microphone into `path`: Kitchen's listening for
/// "next". The recorders a voice note tries, started and stopped per clip.
pub(crate) async fn record_clip(path: &std::path::Path, secs: f64) -> Result<()> {
    let out = path.to_string_lossy().to_string();
    for (prog, args, q) in recorders(&out).await {
        let spawned = tokio::process::Command::new(&prog)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(child) = spawned else { continue };
        tokio::time::sleep(std::time::Duration::from_secs_f64(secs)).await;
        let mut r = Rec { child, path: path.to_path_buf(), entry_id: 0, quits_on_q: q };
        if matches!(r.child.try_wait(), Ok(Some(_))) {
            continue;
        }
        finish(r).await;
        return Ok(());
    }
    bail!("no microphone could be opened — tried the recorders this computer has")
}

/// Ask the recorder to finish its file, and give it a moment to.
async fn finish(mut r: Rec) {
    if r.quits_on_q {
        if let Some(mut stdin) = r.child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(b"q\n").await.ok();
        }
    } else if let Some(pid) = r.child.id() {
        // SIGINT, not a kill: the recorders write the WAV header on the way out.
        tokio::process::Command::new("kill").args(["-INT", &pid.to_string()]).status().await.ok();
    }
    if tokio::time::timeout(std::time::Duration::from_secs(5), r.child.wait()).await.is_err() {
        r.child.kill().await.ok();
    }
}

async fn record_stop(pool: &SqlitePool) -> Result<()> {
    let Some(r) = take_rec() else { return Ok(()) };
    let (path, entry_id) = (r.path.clone(), r.entry_id);
    finish(r).await;
    let bytes = tokio::fs::read(&path).await.unwrap_or_default();
    let (secs, peaks) = j::wave(&bytes, 48);
    if secs < 0.5 {
        std::fs::remove_file(&path).ok();
        bail!("nothing was recorded — is a microphone connected and allowed?");
    }
    let peaks: Vec<String> = peaks.iter().map(|p| p.to_string()).collect();
    sqlx::query(
        "INSERT INTO voice_notes (entry_id, path, duration_s, peaks, created) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(entry_id)
    .bind(path.to_string_lossy().to_string())
    .bind(secs)
    .bind(peaks.join(","))
    .bind(j::now())
    .execute(pool)
    .await?;
    sqlx::query("UPDATE entries SET updated = ? WHERE id = ?").bind(j::now()).bind(entry_id).execute(pool).await?;
    Ok(())
}

/// whisper-cli over a 16 kHz WAV, with the model and language the voice
/// search uses (Settings › AI Features), or the model beside whisper-cli.
/// whisper-cli and a model are both here, or the reason they are not.
pub(crate) fn transcribe_ready() -> Result<()> {
    tulipix_core::ai_models::whisper_model_for("voice")
        .or_else(crate::api::tools::whisper_model)
        .ok_or_else(|| anyhow!("no speech model — Settings › AI Features"))?;
    tool("whisper-cli").ok_or_else(|| anyhow!("whisper-cli is missing"))?;
    Ok(())
}

pub(crate) async fn transcribe(wav: &std::path::Path) -> Result<String> {
    let model = tulipix_core::ai_models::whisper_model_for("voice")
        .or_else(crate::api::tools::whisper_model)
        .ok_or_else(|| anyhow!("no speech model — Settings › AI Features"))?;
    let whisper = tool("whisper-cli").ok_or_else(|| anyhow!("whisper-cli is missing"))?;
    let lang = tulipix_core::settings::Settings::load()
        .ok()
        .map(|s| s.text("ai.voice-lang"))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".into());
    let out = tokio::process::Command::new(&whisper)
        .arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(wav)
        .args(["-nt", "-l", &lang])
        .output()
        .await?;
    if !out.status.success() {
        bail!("whisper-cli stopped ({})", out.status);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

// ---------------------------------------------------------------- snapshot ---

/// Words and mood for every day anything was written, keyed by date. The
/// journal is one person's writing, so all of it fits in memory.
struct DayInfo {
    words: i64,
    mood: i64,
}

async fn days(pool: &SqlitePool) -> Result<BTreeMap<NaiveDate, DayInfo>> {
    // The day's mood is its latest entry's that has one.
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT e.day, SUM(e.words), \
                COALESCE((SELECT m.mood FROM entries m WHERE m.day = e.day AND m.mood > 0 \
                          ORDER BY m.updated DESC LIMIT 1), 0) \
         FROM entries e GROUP BY e.day",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| r.1 > 0 || r.2 > 0)
        .filter_map(|(d, words, mood)| Some((j::parse_day(&d)?, DayInfo { words, mood })))
        .collect())
}

async fn snapshot(pool: &SqlitePool) -> Result<JournalState> {
    let (tab, day, month, cal_mode, query, prompt, focus, notice) = {
        let mut s = lock();
        (
            s.tab.clone(),
            s.day.unwrap_or_else(j::today),
            s.month.unwrap_or_else(|| j::month_start(j::today())),
            s.cal_mode.clone(),
            s.query.clone(),
            PROMPTS[s.prompt % PROMPTS.len()].to_string(),
            std::mem::take(&mut s.focus),
            std::mem::take(&mut s.notice),
        )
    };
    let today = j::today();
    let all = days(pool).await?;
    let written: BTreeSet<NaiveDate> = all.keys().copied().collect();
    let (streak, longest, longest_end) = j::streaks(&written, today);

    let entries = entries(pool, day).await?;
    let hidden: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT source FROM hidden WHERE day = ?")
        .bind(j::iso(day))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    let (rows, points) = j::gather(day).await;
    let day_photos: Vec<i64> = j::day_photos(day).await.into_iter().take(60).map(|p| p.0).collect();
    let stops = j::stops(&points);
    let labels = j::names_for(&stops);
    let names: Vec<String> = {
        let mut seen = HashSet::new();
        let named: Vec<String> =
            entries.iter().map(|e| e.place.clone()).filter(|p| !p.is_empty() && seen.insert(p.clone())).collect();
        if named.is_empty() { labels.iter().filter(|l| !l.is_empty() && seen.insert((*l).clone())).cloned().collect() } else { named }
    };
    let weather_on = crate::api::shell::load().flag("journal.weather", false);
    let weather = if weather_on && tab == "today" { day_weather(pool, day).await } else { String::new() };

    let otd = if matches!(tab.as_str(), "today" | "otd") { otd(pool, day).await? } else { Vec::new() };

    let n = entries.iter().filter(|e| e.words > 0 || e.mood > 0).count() as i64;
    let mut sub: Vec<String> = names.iter().take(2).cloned().collect();
    sub.push(match n {
        0 => "Nothing written yet".into(),
        1 => "1 entry".into(),
        n => format!("{n} entries"),
    });

    Ok(JournalState {
        is_today: day == today,
        day_title: if day.year() == today.year() {
            day.format("%A, %-d %B").to_string()
        } else {
            day.format("%A, %-d %B %Y").to_string()
        },
        day_sub: sub.join(" · "),
        day: j::iso(day),
        focus,
        gathered: rows
            .into_iter()
            .map(|r| GatherRow {
                shown: !hidden.contains(r.source),
                source: r.source.to_string(),
                clock: if r.timed { j::clock(r.at) } else { String::new() },
                title: r.title,
                ids: r.ids,
            })
            .collect(),
        day_photos,
        places: PlacesView {
            points: j::fit(&stops).into_iter().map(|(x, y)| MapPoint { x, y }).collect(),
            stops: stops.len() as i64,
            km: (j::route_km(&stops) * 10.0).round() / 10.0,
            can_name: !stops.is_empty() && !j::has_place_names(),
            labels,
            names,
        },
        weather,
        weather_on,
        entries,
        otd,
        prompt,
        streak,
        longest,
        longest_when: longest_end
            .map(|d| if d.year() == today.year() { d.format("in %B").to_string() } else { d.format("in %B %Y").to_string() })
            .unwrap_or_default(),
        month: if tab == "calendar" { month_view(month, &cal_mode, &all, today).await } else { MonthView::default() },
        insights: if tab == "insights" { insights(pool, &all, today, streak).await? } else { Insights::default() },
        results: if query.is_empty() { Vec::new() } else { search(pool, &query).await? },
        query,
        known_places: sqlx::query_scalar(
            "SELECT place FROM entries WHERE place != '' GROUP BY place ORDER BY COUNT(*) DESC LIMIT 12",
        )
        .fetch_all(pool)
        .await?,
        known_tags: sqlx::query_scalar("SELECT tag FROM entry_tags GROUP BY tag ORDER BY COUNT(*) DESC LIMIT 12")
            .fetch_all(pool)
            .await?,
        recording: recording_into(),
        notice,
        tab,
    })
}

async fn entries(pool: &SqlitePool, day: NaiveDate) -> Result<Vec<EntryView>> {
    let rows: Vec<(i64, i64, String, i64, i64, String)> = sqlx::query_as(
        "SELECT id, created, body, words, mood, place FROM entries WHERE day = ? ORDER BY created, id",
    )
    .bind(j::iso(day))
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, created, body, words, mood, place) in rows {
        let tags: Vec<String> = sqlx::query_scalar("SELECT tag FROM entry_tags WHERE entry_id = ? ORDER BY tag")
            .bind(id)
            .fetch_all(pool)
            .await?;
        let photos: Vec<i64> =
            sqlx::query_scalar("SELECT photo_id FROM entry_photos WHERE entry_id = ? ORDER BY photo_id")
                .bind(id)
                .fetch_all(pool)
                .await?;
        let voice = sqlx::query_as::<_, (i64, String, f64, String, String, String)>(
            "SELECT id, path, duration_s, peaks, transcript, state FROM voice_notes WHERE entry_id = ? ORDER BY created",
        )
        .bind(id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, path, duration_s, peaks, transcript, state)| VoiceView {
            id,
            path,
            duration_s,
            peaks: peaks.split(',').filter_map(|p| p.parse().ok()).collect(),
            transcript,
            state,
        })
        .collect();
        out.push(EntryView { id, created, body, words, mood, place, tags, photos, voice });
    }
    Ok(out)
}

/// The viewed day in each of the last ten years that has anything to show.
async fn otd(pool: &SqlitePool, day: NaiveDate) -> Result<Vec<OtdCard>> {
    let mut out = Vec::new();
    for k in 1..=10 {
        let Some(d) = j::years_back(day, k) else { continue };
        let first: Option<(String, i64, String)> = sqlx::query_as(
            "SELECT body, mood, place FROM entries WHERE day = ? AND (words > 0 OR mood > 0) \
             ORDER BY created LIMIT 1",
        )
        .bind(j::iso(d))
        .fetch_optional(pool)
        .await?;
        let e = j::echo(d).await;
        let heard = !e.artist.is_empty();
        if first.is_none() && e.photo_count == 0 && !heard {
            continue;
        }
        let photos = plural(e.photo_count, "photo", "photos");
        let (written, text, mood, mut meta) = match first {
            Some((body, mood, place)) => {
                let mut meta = Vec::new();
                if !place.is_empty() {
                    meta.push(place);
                }
                (true, clip(&body, 180), mood, meta)
            }
            None => {
                let mut said = Vec::new();
                if e.photo_count > 0 {
                    said.push(format!("you took {photos}"));
                }
                if heard {
                    said.push(format!("listened to {} {}", e.artist, times(e.artist_plays)));
                }
                (false, format!("No entry that day, but {}.", said.join(" and ")), 0, Vec::new())
            }
        };
        if written {
            if heard {
                meta.push(e.artist.clone());
            }
            if e.photo_count > 0 {
                meta.push(photos);
            }
            if e.spent > 0 {
                meta.push(format!("{} spent", j::spent_text(e.spent)));
            }
        }
        out.push(OtdCard {
            day: j::iso(d),
            years_ago: k as i64,
            year: d.year() as i64,
            written,
            text,
            mood,
            meta,
            photos: e.photos,
        });
    }
    Ok(out)
}

async fn month_view(first: NaiveDate, mode: &str, all: &BTreeMap<NaiveDate, DayInfo>, today: NaiveDate) -> MonthView {
    let photos = j::month_photos(first).await;
    let dim = j::days_in_month(first);
    let cells: Vec<DayCell> = (0..dim as i64)
        .map(|i| {
            let d = j::shift(first, i);
            let info = all.get(&d);
            DayCell {
                day: j::iso(d),
                n: i + 1,
                words: info.map(|x| x.words).unwrap_or(0),
                mood: info.map(|x| x.mood).unwrap_or(0),
                photo: photos.get(&j::iso(d)).copied().unwrap_or(0),
                written: info.is_some(),
                future: d > today,
                today: d == today,
            }
        })
        .collect();
    let past: Vec<&DayCell> = cells.iter().filter(|c| !c.future).collect();
    let summary = if past.is_empty() {
        String::new()
    } else {
        let w = past.iter().filter(|c| c.written).count();
        let words: i64 = past.iter().map(|c| c.words).sum();
        format!("{w} of {} days written · {} words", past.len(), thousands(words))
    };
    MonthView {
        first: j::iso(first),
        title: first.format("%B %Y").to_string(),
        mode: mode.to_string(),
        lead: first.weekday().num_days_from_monday() as i64,
        cells,
        summary,
    }
}

async fn insights(
    pool: &SqlitePool,
    all: &BTreeMap<NaiveDate, DayInfo>,
    today: NaiveDate,
    streak: i64,
) -> Result<Insights> {
    let m0 = j::month_start(today);
    let in_span = |a: NaiveDate, b: NaiveDate| all.range(a..=b);
    let words_month: i64 = in_span(m0, today).map(|(_, x)| x.words).sum();
    // The same stretch of last month: the 1st to today's date, or its end.
    let p0 = j::month_start(j::shift(m0, -1));
    let p1 = j::shift(p0, (today.day() as i64 - 1).min(j::days_in_month(p0) as i64 - 1));
    let words_prev: i64 = in_span(p0, p1).map(|(_, x)| x.words).sum();

    let places: Vec<(String, i64)> = sqlx::query_as(
        "SELECT place, COUNT(*) FROM entries WHERE day >= ? AND place != '' GROUP BY place ORDER BY 2 DESC, 1",
    )
    .bind(j::iso(m0))
    .fetch_all(pool)
    .await?;
    let before: HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT DISTINCT place FROM entries WHERE day < ? AND place != ''")
            .bind(j::iso(m0))
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let photos_kept: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT ep.photo_id) FROM entry_photos ep JOIN entries e ON e.id = ep.entry_id WHERE e.day >= ?",
    )
    .bind(j::iso(m0))
    .fetch_one(pool)
    .await?;

    let moods: Vec<i64> =
        (0..30).rev().map(|i| all.get(&j::shift(today, -i)).map(|x| x.mood).unwrap_or(0)).collect();

    let (a90, b90) = (j::bounds(j::shift(today, -89)).0, j::bounds(today).1);
    let mut cards = Vec::new();
    if let Some(c) = music_card(all, today, &j::morning_artists(a90, b90).await) {
        cards.push(c);
    }
    if let Some(c) = photos_card(all, today, &j::photo_days(a90, b90).await) {
        cards.push(c);
    }

    Ok(Insights {
        words_month,
        has_change: words_prev > 0,
        words_change: if words_prev > 0 { (words_month - words_prev) * 100 / words_prev } else { 0 },
        days_written: in_span(m0, today).count() as i64,
        days_so_far: today.day() as i64,
        streak,
        new_places: places.iter().filter(|p| !before.contains(&p.0)).count() as i64,
        top_place: places.first().map(|p| p.0.clone()).unwrap_or_default(),
        photos_kept,
        photos_taken: j::photos_between(j::bounds(m0).0, j::bounds(today).1).await,
        mood_line: mood_line(&moods),
        moods,
        cards,
        tags: sqlx::query_as::<_, (String, i64)>(
            "SELECT tag, COUNT(*) FROM entry_tags GROUP BY tag ORDER BY 2 DESC, 1 LIMIT 3",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(tag, n)| TagCount { tag, n })
        .collect(),
    })
}

fn mood_line(moods: &[i64]) -> String {
    let good = moods.iter().filter(|m| **m >= 4).count();
    let low = moods.iter().filter(|m| (1..=2).contains(*m)).count();
    match (good, low) {
        (g, l) if g + l < 4 => String::new(),
        (_, 0) => "No low days in the last month".into(),
        (g, l) if g >= 2 * l => format!("Good days outnumber low ones {} to 1", (g as f64 / l as f64).round() as i64),
        (g, l) if g > l => "A few more good days than low ones".into(),
        (g, l) if g == l => "As many good days as low ones".into(),
        _ => "More low days than good ones lately".into(),
    }
}

/// "Your best days had music in the morning": when most great days of the
/// last ninety had something playing before noon, and who it was.
fn music_card(
    all: &BTreeMap<NaiveDate, DayInfo>,
    today: NaiveDate,
    mornings: &HashMap<String, Vec<String>>,
) -> Option<InsightCard> {
    let great: Vec<String> =
        all.range(j::shift(today, -89)..=today).filter(|(_, x)| x.mood == 5).map(|(d, _)| j::iso(*d)).collect();
    let with: Vec<&Vec<String>> = great.iter().filter_map(|d| mornings.get(d)).collect();
    if great.len() < 3 || with.len() * 2 < great.len() {
        return None;
    }
    let mut days_per: HashMap<&str, usize> = HashMap::new();
    for artists in &with {
        let unique: HashSet<&str> = artists.iter().map(String::as_str).collect();
        for a in unique {
            *days_per.entry(a).or_default() += 1;
        }
    }
    let mut top: Vec<(&str, usize)> = days_per.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let names: Vec<&str> = top.iter().take(2).map(|t| t.0).collect();
    Some(InsightCard {
        kind: "music".into(),
        title: "Your best days had music in the morning".into(),
        body: format!("{} on {} of your {} great days.", names.join(" and "), with.len(), great.len()),
    })
}

/// Whether you write more on days you took photos, over the last ninety.
fn photos_card(all: &BTreeMap<NaiveDate, DayInfo>, today: NaiveDate, photo_days: &BTreeSet<String>) -> Option<InsightCard> {
    let (mut on, mut off) = (Vec::new(), Vec::new());
    for (d, x) in all.range(j::shift(today, -89)..=today).filter(|(_, x)| x.words > 0) {
        if photo_days.contains(&j::iso(*d)) { on.push(x.words) } else { off.push(x.words) }
    }
    if on.len() < 3 || off.len() < 3 {
        return None;
    }
    let avg = |v: &[i64]| v.iter().sum::<i64>() / v.len() as i64;
    let (a, b) = (avg(&on[..]), avg(&off[..]));
    let (title, body) = if a * 10 >= b * 13 {
        ("You write more on days you take photos", format!("{a} words on photo days, {b} on the rest."))
    } else if b * 10 >= a * 13 {
        ("You write more on quieter days", format!("{b} words on days without photos, {a} on days with them."))
    } else {
        return None;
    };
    Some(InsightCard { kind: "photos".into(), title: title.into(), body })
}

async fn search(pool: &SqlitePool, query: &str) -> Result<Vec<JournalHit>> {
    let q = query.to_lowercase();
    let rows: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT e.id, e.day, e.body, e.mood FROM entries e \
         WHERE instr(lower(e.body), ?1) > 0 OR instr(lower(e.place), ?1) > 0 \
            OR EXISTS (SELECT 1 FROM entry_tags t WHERE t.entry_id = e.id AND instr(t.tag, ?1) > 0) \
         ORDER BY e.day DESC, e.created DESC LIMIT 100",
    )
    .bind(&q)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(entry_id, day, body, mood)| JournalHit {
            label: j::parse_day(&day).map(|d| d.format("%a %-d %b %Y").to_string()).unwrap_or_default(),
            excerpt: around(&body, &q, 90),
            entry_id,
            day,
            mood,
        })
        .collect())
}

// ------------------------------------------------------------------ format ---

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn times(n: i64) -> &'static str {
    match n {
        1 => "once",
        2 => "twice",
        _ => "again and again",
    }
}

fn thousands(n: i64) -> String {
    let d = n.to_string();
    let b = d.as_bytes();
    let mut out = String::new();
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// The start of a text, cut at a word near `max` characters.
fn clip(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|c| c.0).unwrap_or(&cut);
    format!("{cut}…")
}

/// About `max` characters of `text` around the first place `q` (lowercase)
/// appears. Compared a character at a time, so a letter whose lowercase is
/// longer than itself cannot shift the window off its match.
fn around(text: &str, q: &str, max: usize) -> String {
    let chars: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let needle: Vec<char> = q.chars().collect();
    let at = if needle.is_empty() {
        0
    } else {
        lower.windows(needle.len()).position(|w| w == needle.as_slice()).unwrap_or(0)
    };
    let start = at.saturating_sub(max / 3);
    let end = (start + max).min(chars.len());
    let mut s: String = chars[start..end].iter().collect();
    if start > 0 {
        s = format!("…{s}");
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_come_out_plain() {
        assert_eq!(clean_tag("#Lisbon trip"), "lisbon-trip");
        assert_eq!(clean_tag("  #walks "), "walks");
        assert_eq!(clean_tag("#!!"), "");
    }

    #[test]
    fn the_mood_line_needs_enough_days_to_say_anything() {
        assert_eq!(mood_line(&[5, 4, 0, 0]), "");
        assert_eq!(mood_line(&[5, 4, 4, 5, 4, 5, 1, 2]), "Good days outnumber low ones 3 to 1");
        assert_eq!(mood_line(&[5, 4, 4, 5]), "No low days in the last month");
        assert_eq!(mood_line(&[1, 2, 1, 4]), "More low days than good ones lately");
    }

    #[test]
    fn a_search_hit_shows_the_words_around_it() {
        let body = "The light on the water at five was the kind you can't photograph, so I didn't try.";
        let s = around(body, "photograph", 30);
        assert!(s.contains("photograph"), "{s}");
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert_eq!(clip("one two three", 7), "one…");
    }
}
