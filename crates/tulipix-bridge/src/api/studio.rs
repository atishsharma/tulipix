// The Studio section: movies and photo books made from the library.
//
// Two tabs and a project open — Create (suggestions and the sources: trips
// from Places, albums, people, dates), Projects, and the project itself (its
// photos, length, shape, song, and Render). The picking and the ffmpeg command
// lines are `crate::studio`; this runs them one project at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use sqlx::SqlitePool;
use tokio::io::AsyncBufReadExt;

use crate::db::studio_pool;
use crate::studio::{self as s, Cand, Slide};

// ------------------------------------------------------------------- state ---

pub struct StudioState {
    /// create | projects.
    pub tab: String,
    pub notice: String,
    pub open: Option<ProjectView>,
    pub projects: Vec<ProjectRow>,
    pub ideas: Vec<Idea>,
    pub trips: Vec<SourcePick>,
    pub albums: Vec<SourcePick>,
    pub people: Vec<SourcePick>,
    pub songs: Vec<SongPick>,
    /// A project is rendering or waiting to.
    pub busy: bool,
    pub folder: String,
    /// ffmpeg is here.
    pub ready: bool,
}

/// A ready-made suggestion: a source and a template.
pub struct Idea {
    pub title: String,
    pub sub: String,
    pub source: String,
    pub start: i64,
    pub end: i64,
    pub kind: String,
    pub length: String,
    pub shape: String,
}

pub struct SourcePick {
    /// "trip:3", "album:7", "person:2".
    pub source: String,
    pub label: String,
    pub sub: String,
    pub start: i64,
    pub end: i64,
}

pub struct SongPick {
    pub id: i64,
    pub title: String,
    pub artist: String,
    /// "3:41".
    pub length: String,
    /// 0 when not measured.
    pub bpm: i64,
}

pub struct ProjectRow {
    pub id: i64,
    pub title: String,
    /// movie | book.
    pub kind: String,
    pub state: String,
    pub progress: f64,
    /// "Movie · 16:9 · 1:12", "Photo book · 24 pages".
    pub sub: String,
    pub cover: i64,
    pub out_path: String,
}

pub struct ProjectView {
    pub id: i64,
    pub title: String,
    pub kind: String,
    /// "From the trip Shimla & Manali".
    pub source_label: String,
    pub length: String,
    pub shape: String,
    pub music_id: i64,
    /// "Ilomilo · Billie Eilish · 118 BPM", or "".
    pub music_label: String,
    pub beat: bool,
    pub picks: Vec<i64>,
    /// Photos the source has, before picking.
    pub available: i64,
    /// "About 1:02", "24 pages".
    pub plan: String,
    pub state: String,
    pub progress: f64,
    pub error: String,
    pub out_path: String,
}

// ---------------------------------------------------------------- commands ---

pub enum StudioCmd {
    Refresh,
    SetTab { tab: String },
    /// A new project from a source and a template; it opens.
    Create { title: String, kind: String, source: String, start: i64, end: i64, length: String, shape: String },
    Open { id: i64 },
    Close,
    Rename { id: i64, title: String },
    SetLength { id: i64, length: String },
    SetShape { id: i64, shape: String },
    /// 0 is no song.
    SetMusic { id: i64, music_id: i64 },
    SetBeat { id: i64, beat: bool },
    RemovePick { id: i64, item: i64 },
    /// Pick the photos again from the source.
    Repick { id: i64 },
    Render { id: i64 },
    Cancel { id: i64 },
    Delete { id: i64 },
    /// Open the finished file, or its folder.
    ShowFile { id: i64, folder: bool },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    open: i64,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session { tab: "create".into(), open: 0, notice: String::new() }))
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Where finished movies and books go, without making it. Settings ›
/// Libraries lists it.
pub(crate) fn out_path() -> Option<PathBuf> {
    let set = crate::api::shell::load().text("studio.folder");
    if !set.trim().is_empty() {
        return Some(PathBuf::from(set));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join("Videos").join("Tulipix Studio"))
}

/// Where finished movies and books go: `~/Videos/Tulipix Studio`, where the
/// Videos section finds them if that folder is in its library.
fn out_dir() -> Result<PathBuf> {
    let dir = out_path().ok_or_else(|| anyhow!("no home folder"))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("could not make {}", dir.display()))?;
    Ok(dir)
}

fn ffmpeg() -> Option<PathBuf> {
    crate::api::journal::tool("ffmpeg")
}

fn exists(db: &str) -> bool {
    tulipix_core::paths::db_path(db).is_some_and(|f| f.exists())
}

// ---------------------------------------------------------------- exported ---

pub async fn studio_dispatch(cmd: StudioCmd) -> Result<StudioState> {
    let pool = studio_pool().await?;
    recover(pool).await;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: StudioCmd) -> Result<()> {
    match cmd {
        StudioCmd::Refresh => {}
        StudioCmd::SetTab { tab } => {
            let mut g = lock();
            g.tab = tab;
            g.open = 0;
        }
        StudioCmd::Create { title, kind, source, start, end, length, shape } => {
            let cands = candidates(&source, start, end).await?;
            if cands.len() < 2 {
                bail!("there are not enough photos there to make something — {} found", cands.len());
            }
            let (music_id, bpm, song_secs) = if kind == "movie" { top_song().await } else { (0, None, 0.0) };
            let n = want(&kind, &length, bpm, true, song_secs);
            let picks = s::pick(&cands.iter().map(|c| c.0.clone()).collect::<Vec<_>>(), n);
            let title = if title.trim().is_empty() { default_title(&source).await } else { title.trim().to_string() };
            let id = sqlx::query(
                "INSERT INTO projects (title, kind, source, start, end, length, shape, music_id, picks, created) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(title)
            .bind(&kind)
            .bind(&source)
            .bind(start)
            .bind(end)
            .bind(&length)
            .bind(&shape)
            .bind(music_id)
            .bind(join(&picks))
            .bind(s::now())
            .execute(pool)
            .await?
            .last_insert_rowid();
            lock().open = id;
        }
        StudioCmd::Open { id } => lock().open = id,
        StudioCmd::Close => lock().open = 0,
        StudioCmd::Rename { id, title } => {
            sqlx::query("UPDATE projects SET title = ? WHERE id = ?").bind(title.trim()).bind(id).execute(pool).await?;
        }
        StudioCmd::SetLength { id, length } => {
            sqlx::query("UPDATE projects SET length = ? WHERE id = ?").bind(&length).bind(id).execute(pool).await?;
            repick(pool, id).await?;
        }
        StudioCmd::SetShape { id, shape } => {
            sqlx::query("UPDATE projects SET shape = ? WHERE id = ?").bind(shape).bind(id).execute(pool).await?;
        }
        StudioCmd::SetMusic { id, music_id } => {
            sqlx::query("UPDATE projects SET music_id = ? WHERE id = ?").bind(music_id).bind(id).execute(pool).await?;
            repick(pool, id).await?;
        }
        StudioCmd::SetBeat { id, beat } => {
            sqlx::query("UPDATE projects SET beat = ? WHERE id = ?").bind(beat as i64).bind(id).execute(pool).await?;
            repick(pool, id).await?;
        }
        StudioCmd::RemovePick { id, item } => {
            let picks: String = sqlx::query_scalar("SELECT picks FROM projects WHERE id = ?").bind(id).fetch_one(pool).await?;
            let kept: Vec<i64> = split(&picks).into_iter().filter(|x| *x != item).collect();
            sqlx::query("UPDATE projects SET picks = ? WHERE id = ?").bind(join(&kept)).bind(id).execute(pool).await?;
        }
        StudioCmd::Repick { id } => repick(pool, id).await?,
        StudioCmd::Render { id } => {
            if ffmpeg().is_none() {
                bail!("ffmpeg is missing, and Studio renders with it");
            }
            sqlx::query("UPDATE projects SET state = 'queued', progress = 0, error = '' WHERE id = ?").bind(id).execute(pool).await?;
            kick(pool);
        }
        StudioCmd::Cancel { id } => {
            *cancel().lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
            sqlx::query("UPDATE projects SET state = 'draft', progress = 0 WHERE id = ? AND state = 'queued'").bind(id).execute(pool).await?;
        }
        StudioCmd::Delete { id } => {
            sqlx::query("DELETE FROM projects WHERE id = ? AND state NOT IN ('rendering')").bind(id).execute(pool).await?;
            let mut g = lock();
            if g.open == id {
                g.open = 0;
            }
        }
        StudioCmd::ShowFile { id, folder } => {
            let out: String = sqlx::query_scalar("SELECT out_path FROM projects WHERE id = ?").bind(id).fetch_one(pool).await?;
            if out.is_empty() || !Path::new(&out).exists() {
                bail!("the file is not there any more — render it again");
            }
            let target = if folder { Path::new(&out).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or(out) } else { out };
            crate::api::transfer::open_url(&target);
        }
    }
    Ok(())
}

fn join(ids: &[i64]) -> String {
    ids.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

fn split(s: &str) -> Vec<i64> {
    s.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

/// How many photos a project wants.
fn want(kind: &str, length: &str, bpm: Option<f64>, beat: bool, song_secs: f64) -> usize {
    if kind == "book" {
        return 48;
    }
    s::photos_for(s::target_secs(length, song_secs), s::slide_secs(bpm, beat))
}

async fn repick(pool: &SqlitePool, id: i64) -> Result<()> {
    let (kind, source, start, end, length, music_id, beat): (String, String, i64, i64, String, i64, i64) =
        sqlx::query_as("SELECT kind, source, start, end, length, music_id, beat FROM projects WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let cands = candidates(&source, start, end).await?;
    let (bpm, secs) = song(music_id).await.map(|x| (x.2, x.3)).unwrap_or((None, 0.0));
    let n = want(&kind, &length, bpm, beat != 0, secs);
    let picks = s::pick(&cands.iter().map(|c| c.0.clone()).collect::<Vec<_>>(), n);
    sqlx::query("UPDATE projects SET picks = ? WHERE id = ?").bind(join(&picks)).bind(id).execute(pool).await?;
    Ok(())
}

// ----------------------------------------------------------------- sources ---

type Row = (i64, i64, i64, Option<String>, String, Option<i64>);

/// The photos a source has: (candidate, path, orientation), in time order.
/// Pictures only — ffmpeg reads these four without help.
async fn candidates(source: &str, start: i64, end: i64) -> Result<Vec<(Cand, String, i64)>> {
    let photos = crate::db::photos_pool().await?;
    const COLS: &str = "SELECT i.id, pm.taken_at, pm.starred, pm.phash, i.abs_path, pm.orientation \
                        FROM photo_meta pm JOIN items i ON i.id = pm.item_id";
    const LIVE: &str = "pm.taken_at IS NOT NULL AND i.missing_since IS NULL AND pm.deleted_at IS NULL AND pm.archived = 0";
    let (kind, key) = source.split_once(':').unwrap_or((source, ""));
    let key: i64 = key.parse().unwrap_or(0);
    let rows: Vec<Row> = match kind {
        "album" => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLS} JOIN album_items ai ON ai.item_id = i.id WHERE ai.album_id = ? AND {LIVE} ORDER BY pm.taken_at")))
                .bind(key)
                .fetch_all(photos)
                .await?
        }
        "person" => {
            sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "{COLS} WHERE i.id IN (SELECT item_id FROM faces WHERE person_id = ?) AND {LIVE} ORDER BY pm.taken_at"
            )))
            .bind(key)
            .fetch_all(photos)
            .await?
        }
        // A trip and a date range are both a window of time.
        _ => {
            let (a, b) = if kind == "trip" { trip_window(key).await.unwrap_or((start, end)) } else { (start, end) };
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLS} WHERE pm.taken_at >= ? AND pm.taken_at <= ? AND {LIVE} ORDER BY pm.taken_at")))
                .bind(a)
                .bind(b)
                .fetch_all(photos)
                .await?
        }
    };
    Ok(rows
        .into_iter()
        .filter(|r| {
            let low = r.4.to_lowercase();
            [".jpg", ".jpeg", ".png", ".webp"].iter().any(|e| low.ends_with(e))
        })
        .map(|(id, ts, starred, phash, path, orient)| {
            let day = Local.timestamp_opt(ts, 0).single().map(|t| t.date_naive().num_days_from_ce() as i64).unwrap_or(ts / 86_400);
            let phash = phash.and_then(|h| u64::from_str_radix(h.trim(), 16).ok());
            (Cand { id, ts, day, starred: starred != 0, phash }, path, orient.unwrap_or(1))
        })
        .collect())
}

/// A name for a project made without one — a trip sent from Places.
async fn default_title(source: &str) -> String {
    if let Some(id) = source.strip_prefix("trip:").and_then(|x| x.parse::<i64>().ok())
        && exists("places")
        && let Ok(pool) = crate::db::places_pool().await
        && let Ok(Some(t)) = sqlx::query_scalar::<_, String>("SELECT title FROM trips WHERE id = ?").bind(id).fetch_optional(pool).await
        && !t.is_empty()
    {
        return t;
    }
    "Untitled".into()
}

/// A kept trip's times in Places, widened to its first and last whole day.
async fn trip_window(id: i64) -> Option<(i64, i64)> {
    if !exists("places") {
        return None;
    }
    let pool = crate::db::places_pool().await.ok()?;
    let (a, b): (i64, i64) = sqlx::query_as("SELECT start, end FROM trips WHERE id = ?").bind(id).fetch_optional(pool).await.ok()??;
    let day = |t: i64| Local.timestamp_opt(t, 0).single().map(|x| x.date_naive());
    let (da, db) = (day(a)?, day(b)?);
    Some((crate::journal::bounds(da).0, crate::journal::bounds(db).1 - 1))
}

/// A song: (path, label, bpm, seconds).
async fn song(id: i64) -> Option<(String, String, Option<f64>, f64)> {
    if id == 0 || !exists("music") {
        return None;
    }
    let pool = crate::db::music_pool().await.ok()?;
    let row: (String, Option<String>, Option<String>, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT i.abs_path, t.title, a.name, t.bpm, t.duration_s FROM items i JOIN track_meta t ON t.item_id = i.id \
         LEFT JOIN artists a ON a.id = t.artist_id WHERE i.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .ok()??;
    let (path, title, artist, bpm, secs) = row;
    let mut label = title.unwrap_or_default();
    if let Some(a) = artist.filter(|a| !a.is_empty()) {
        label = format!("{label} · {a}");
    }
    if let Some(b) = bpm.filter(|b| *b > 0.0) {
        label = format!("{label} · {} BPM", b.round());
    }
    Some((path, label, bpm, secs.unwrap_or(0.0)))
}

/// Songs to set a movie to, most played first.
async fn songs() -> Vec<(i64, String, String, f64, Option<f64>)> {
    if !exists("music") {
        return Vec::new();
    }
    let Ok(pool) = crate::db::music_pool().await else { return Vec::new() };
    sqlx::query_as(
        "SELECT i.id, COALESCE(t.title, ''), COALESCE(a.name, ''), COALESCE(t.duration_s, 0), t.bpm \
         FROM items i JOIN track_meta t ON t.item_id = i.id LEFT JOIN artists a ON a.id = t.artist_id \
         WHERE i.missing_since IS NULL AND t.is_audiobook = 0 AND t.is_stream = 0 AND t.duration_s > 60 \
         ORDER BY t.play_count DESC, t.loved DESC, t.rating DESC LIMIT 40",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// The most played song: (id, bpm, seconds), or nothing.
async fn top_song() -> (i64, Option<f64>, f64) {
    songs().await.into_iter().next().map(|x| (x.0, x.4, x.3)).unwrap_or((0, None, 0.0))
}

// ------------------------------------------------------------------ worker ---

static RENDERING: AtomicBool = AtomicBool::new(false);

fn cancel() -> &'static Mutex<Option<i64>> {
    static C: OnceLock<Mutex<Option<i64>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn cancelled(id: i64) -> bool {
    cancel().lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|c| c == id)
}

async fn recover(pool: &'static SqlitePool) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_ok() {
        sqlx::query("UPDATE projects SET state = 'queued' WHERE state = 'rendering'").execute(pool).await.ok();
        kick(pool);
    }
}

fn kick(pool: &'static SqlitePool) {
    if RENDERING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let next: Option<i64> = sqlx::query_scalar("SELECT id FROM projects WHERE state = 'queued' ORDER BY id LIMIT 1")
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
            let Some(id) = next else { break };
            *cancel().lock().unwrap_or_else(|e| e.into_inner()) = None;
            sqlx::query("UPDATE projects SET state = 'rendering', progress = 0 WHERE id = ?").bind(id).execute(pool).await.ok();
            let result = render(pool, id).await;
            let (state, error) = match result {
                Ok(()) => ("done", String::new()),
                Err(_) if cancelled(id) => ("draft", String::new()),
                Err(e) => ("failed", e.to_string()),
            };
            sqlx::query("UPDATE projects SET state = ?, error = ?, progress = CASE WHEN ? = 'done' THEN 1 ELSE 0 END, \
                         rendered = CASE WHEN ? = 'done' THEN ? ELSE rendered END WHERE id = ?")
                .bind(state)
                .bind(error)
                .bind(state)
                .bind(state)
                .bind(s::now())
                .bind(id)
                .execute(pool)
                .await
                .ok();
        }
        RENDERING.store(false, Ordering::SeqCst);
        let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects WHERE state = 'queued'").fetch_one(pool).await.unwrap_or(0);
        if waiting > 0 {
            kick(pool);
        }
    });
}

/// A file name that is not taken yet: "Title.mp4", "Title (2).mp4".
fn free_name(dir: &Path, title: &str, ext: &str) -> PathBuf {
    let clean: String = title.chars().filter(|c| !r#"\/:*?"<>|"#.contains(*c)).collect();
    let base = if clean.trim().is_empty() { "Tulipix Studio".to_string() } else { clean.trim().to_string() };
    let mut p = dir.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

/// The picked photos that are still on disk, each with whether it is wider
/// than tall once turned upright (Photos' own width and height).
async fn slides(picks: &[i64]) -> Result<Vec<(Slide, bool)>> {
    let photos = crate::db::photos_pool().await?;
    let mut out = Vec::new();
    for id in picks {
        let row: Option<(String, Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT i.abs_path, pm.orientation, pm.width, pm.height FROM items i JOIN photo_meta pm ON pm.item_id = i.id \
             WHERE i.id = ? AND i.missing_since IS NULL",
        )
        .bind(id)
        .fetch_optional(photos)
        .await?;
        if let Some((path, o, w, h)) = row.filter(|r| Path::new(&r.0).exists()) {
            let o = o.unwrap_or(1);
            let (w, h) = (w.unwrap_or(3), h.unwrap_or(2));
            let wide = if (5..=8).contains(&o) { h >= w } else { w >= h };
            out.push((Slide { path, orientation: o }, wide));
        }
    }
    Ok(out)
}

/// The encoder this ffmpeg has: x264 when it is there, else OpenH264, else
/// MPEG-4 part 2, which every build has.
async fn encoder(ff: &Path) -> Vec<String> {
    static E: tokio::sync::OnceCell<Vec<String>> = tokio::sync::OnceCell::const_new();
    E.get_or_init(|| async {
        let out = tokio::process::Command::new(ff).args(["-hide_banner", "-encoders"]).output().await;
        let list = out.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
        let v: &[&str] = if list.contains("libx264") {
            &["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"]
        } else if list.contains("libopenh264") {
            &["-c:v", "libopenh264", "-b:v", "8M"]
        } else {
            &["-c:v", "mpeg4", "-q:v", "3"]
        };
        v.iter().map(|x| x.to_string()).collect()
    })
    .await
    .clone()
}

async fn set_progress(pool: &SqlitePool, id: i64, p: f64) {
    sqlx::query("UPDATE projects SET progress = ? WHERE id = ?").bind(p.clamp(0.0, 1.0)).bind(id).execute(pool).await.ok();
}

async fn render(pool: &SqlitePool, id: i64) -> Result<()> {
    let (title, kind, shape, music_id, beat, picks): (String, String, String, i64, i64, String) =
        sqlx::query_as("SELECT title, kind, shape, music_id, beat, picks FROM projects WHERE id = ?").bind(id).fetch_one(pool).await?;
    let ff = ffmpeg().ok_or_else(|| anyhow!("ffmpeg is missing"))?;
    let (slides, wide): (Vec<Slide>, Vec<bool>) = slides(&split(&picks)).await?.into_iter().unzip();
    let dir = out_dir()?;
    if kind == "book" {
        return book(pool, id, &ff, &slides, &wide, &free_name(&dir, &title, "pdf")).await;
    }
    let song = song(music_id).await;
    let slide = s::slide_secs(song.as_ref().and_then(|x| x.2), beat != 0);
    let out = free_name(&dir, &title, "mp4");
    let mut args = s::movie_args(&slides, song.as_ref().map(|x| x.0.as_str()), &shape, slide, &out.to_string_lossy())?;
    // Swap in the encoder this ffmpeg has.
    if let Some(at) = args.iter().position(|x| x == "libx264") {
        let enc = encoder(&ff).await;
        let _ = args.splice(at - 1..at + 5, enc);
    }
    let total = s::movie_secs(slides.len(), slide);
    let mut child = tokio::process::Command::new(&ff)
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("could not start ffmpeg")?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no output from ffmpeg"))?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut last = 0.0;
    while let Ok(Some(line)) = lines.next_line().await {
        if cancelled(id) {
            child.kill().await.ok();
            std::fs::remove_file(&out).ok();
            bail!("cancelled");
        }
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.trim().parse::<f64>().ok()) {
            let p = us / 1e6 / total;
            if p - last >= 0.01 {
                last = p;
                set_progress(pool, id, p).await;
            }
        }
    }
    let status = child.wait().await?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            e.read_to_string(&mut err).await.ok();
        }
        std::fs::remove_file(&out).ok();
        bail!("ffmpeg stopped: {}", err.lines().last().unwrap_or("no reason given"));
    }
    sqlx::query("UPDATE projects SET out_path = ? WHERE id = ?").bind(out.to_string_lossy().to_string()).bind(id).execute(pool).await?;
    Ok(())
}

/// A photo book: each page laid out by ffmpeg as a JPEG, then the JPEGs put
/// into a PDF untouched (Tools' `from_jpegs`).
async fn book(pool: &SqlitePool, id: i64, ff: &Path, slides: &[Slide], wide: &[bool], out: &Path) -> Result<()> {
    if slides.is_empty() {
        bail!("no photos to put in the book");
    }
    let tmp = tulipix_core::paths::cache_dir().ok_or_else(|| anyhow!("no cache folder"))?.join("studio").join(format!("book-{id}"));
    std::fs::create_dir_all(&tmp)?;
    let plan = s::pages(wide);
    let mut files = Vec::new();
    for (n, page) in plan.iter().enumerate() {
        if cancelled(id) {
            bail!("cancelled");
        }
        let these: Vec<Slide> = page.iter().map(|&i| Slide { path: slides[i].path.clone(), orientation: slides[i].orientation }).collect();
        let file = tmp.join(format!("{:03}.jpg", n + 1));
        let args = s::page_args(&these, wide[page[0]], &file.to_string_lossy());
        let status = tokio::process::Command::new(ff).args(&args).status().await?;
        if !status.success() {
            bail!("could not lay out page {}", n + 1);
        }
        files.push(file.to_string_lossy().to_string());
        set_progress(pool, id, (n + 1) as f64 / (plan.len() + 1) as f64).await;
    }
    let out_s = out.to_string_lossy().to_string();
    let target = out_s.clone();
    tokio::task::spawn_blocking(move || tulipix_tools::pdf::from_jpegs(&files, &target)).await??;
    std::fs::remove_dir_all(&tmp).ok();
    sqlx::query("UPDATE projects SET out_path = ? WHERE id = ?").bind(out_s).bind(id).execute(pool).await?;
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn shape_label(shape: &str) -> &'static str {
    match shape {
        "tall" => "9:16",
        "square" => "1:1",
        _ => "16:9",
    }
}

fn day_start(d: NaiveDate) -> i64 {
    crate::journal::bounds(d).0
}

async fn sources() -> (Vec<SourcePick>, Vec<SourcePick>, Vec<SourcePick>) {
    let mut trips = Vec::new();
    if exists("places")
        && let Ok(pool) = crate::db::places_pool().await
    {
        let rows: Vec<(i64, String, i64, i64)> =
            sqlx::query_as("SELECT id, title, start, end FROM trips WHERE state = 1 ORDER BY start DESC LIMIT 30")
                .fetch_all(pool)
                .await
                .unwrap_or_default();
        trips = rows
            .into_iter()
            .map(|(id, title, a, b)| SourcePick {
                source: format!("trip:{id}"),
                label: if title.is_empty() { "A trip".into() } else { title },
                sub: local(a).format("%B %Y").to_string(),
                start: a,
                end: b,
            })
            .collect();
    }
    let (mut albums, mut people) = (Vec::new(), Vec::new());
    if let Ok(photos) = crate::db::photos_pool().await {
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT a.id, a.name, (SELECT COUNT(*) FROM album_items x WHERE x.album_id = a.id) FROM albums a \
             WHERE a.smart_rule IS NULL ORDER BY a.updated DESC LIMIT 30",
        )
        .fetch_all(photos)
        .await
        .unwrap_or_default();
        albums = rows
            .into_iter()
            .filter(|r| r.2 >= 2)
            .map(|(id, name, n)| SourcePick { source: format!("album:{id}"), label: name, sub: format!("{n} photos"), start: 0, end: 0 })
            .collect();
        let rows: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT p.id, p.name, COUNT(DISTINCT f.item_id) FROM people p JOIN faces f ON f.person_id = p.id \
             WHERE p.name IS NOT NULL AND p.name != '' GROUP BY p.id ORDER BY 3 DESC LIMIT 30",
        )
        .fetch_all(photos)
        .await
        .unwrap_or_default();
        people = rows
            .into_iter()
            .filter(|r| r.2 >= 2)
            .map(|(id, name, n)| SourcePick { source: format!("person:{id}"), label: name, sub: format!("{n} photos"), start: 0, end: 0 })
            .collect();
    }
    (trips, albums, people)
}

fn ideas(trips: &[SourcePick]) -> Vec<Idea> {
    let today = Local::now().date_naive();
    let mut out = Vec::new();
    if let Some(t) = trips.first() {
        out.push(Idea {
            title: t.label.clone(),
            sub: format!("A travel movie of {} · {}", t.label, t.sub),
            source: t.source.clone(),
            start: t.start,
            end: t.end,
            kind: "movie".into(),
            length: "medium".into(),
            shape: "wide".into(),
        });
    }
    let last_year = today.year() - 1;
    if let (Some(a), Some(b)) = (NaiveDate::from_ymd_opt(last_year, 1, 1), NaiveDate::from_ymd_opt(last_year + 1, 1, 1)) {
        out.push(Idea {
            title: format!("{last_year} in three minutes"),
            sub: "A year in review, set to your most played song".into(),
            source: "range".into(),
            start: day_start(a),
            end: day_start(b) - 1,
            kind: "movie".into(),
            length: "long".into(),
            shape: "wide".into(),
        });
    }
    if let Some(a) = NaiveDate::from_ymd_opt(today.year(), today.month(), 1) {
        out.push(Idea {
            title: today.format("%B %Y").to_string(),
            sub: "This month so far, as a 30-second vertical reel".into(),
            source: "range".into(),
            start: day_start(a),
            end: day_start(today) + 86_399,
            kind: "movie".into(),
            length: "short".into(),
            shape: "tall".into(),
        });
    }
    out
}

async fn snapshot(pool: &SqlitePool) -> Result<StudioState> {
    let (tab, open, notice) = {
        let mut g = lock();
        (g.tab.clone(), g.open, std::mem::take(&mut g.notice))
    };
    type P = (i64, String, String, String, String, f64, String, String, i64, String);
    let rows: Vec<P> = sqlx::query_as(
        "SELECT id, title, kind, state, shape, progress, picks, out_path, music_id, length FROM projects ORDER BY created DESC",
    )
    .fetch_all(pool)
    .await?;
    let busy = rows.iter().any(|r| r.3 == "queued" || r.3 == "rendering");
    let projects: Vec<ProjectRow> = rows
        .iter()
        .map(|(id, title, kind, state, shape, progress, picks, out_path, _, _)| {
            let p = split(picks);
            let sub = if kind == "book" {
                format!("Photo book · {} photos", p.len())
            } else {
                format!("Movie · {} · {} photos", shape_label(shape), p.len())
            };
            ProjectRow {
                id: *id,
                title: title.clone(),
                kind: kind.clone(),
                state: state.clone(),
                progress: *progress,
                sub,
                cover: p.first().copied().unwrap_or(0),
                out_path: out_path.clone(),
            }
        })
        .collect();

    let open = if open > 0 { project_view(pool, open).await? } else { None };
    let (trips, albums, people) = sources().await;
    let songs = songs()
        .await
        .into_iter()
        .map(|(id, title, artist, secs, bpm)| SongPick {
            id,
            title,
            artist,
            length: clock(secs),
            bpm: bpm.map(|b| b.round() as i64).unwrap_or(0),
        })
        .collect();
    Ok(StudioState {
        tab,
        notice,
        open,
        projects,
        ideas: ideas(&trips),
        trips,
        albums,
        people,
        songs,
        busy,
        folder: out_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
        ready: ffmpeg().is_some(),
    })
}

async fn project_view(pool: &SqlitePool, id: i64) -> Result<Option<ProjectView>> {
    type R = (String, String, String, i64, i64, String, String, i64, i64, String, f64, String, String);
    let row: Option<R> = sqlx::query_as(
        "SELECT title, kind, source, start, end, length, shape, music_id, beat, state, progress, error, out_path \
         FROM projects WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((title, kind, source, start, end, length, shape, music_id, beat, state, progress, error, out_path)) = row else {
        lock().open = 0;
        return Ok(None);
    };
    let picks: String = sqlx::query_scalar("SELECT picks FROM projects WHERE id = ?").bind(id).fetch_one(pool).await?;
    let picks = split(&picks);
    let available = candidates(&source, start, end).await.map(|c| c.len() as i64).unwrap_or(0);
    let song = song(music_id).await;
    let source_label = match source.split_once(':') {
        Some(("trip", _)) => "From a trip in Places".to_string(),
        Some(("album", _)) => "From an album".to_string(),
        Some(("person", _)) => "Photos of one person".to_string(),
        _ => format!("{} – {}", local(start).format("%-d %b %Y"), local(end).format("%-d %b %Y")),
    };
    let plan = if kind == "book" {
        let n = s::pages(&vec![true; picks.len()]).len();
        format!("About {n} pages")
    } else {
        let slide = s::slide_secs(song.as_ref().and_then(|x| x.2), beat != 0);
        format!("{} · {:.1} s a photo", clock(s::movie_secs(picks.len(), slide)), slide)
    };
    Ok(Some(ProjectView {
        id,
        title,
        kind,
        source_label,
        length,
        shape,
        music_id,
        music_label: song.map(|x| x.1).unwrap_or_default(),
        beat: beat != 0,
        picks,
        available,
        plan,
        state,
        progress,
        error,
        out_path,
    }))
}
