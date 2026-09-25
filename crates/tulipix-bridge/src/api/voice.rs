// The Voice section: voice notes transcribed on this computer, searchable,
// with the tasks said in them.
//
// Four tabs over one snapshot — Notes, Ask, Tasks and Setup. The store and the
// parsers are `crate::voice`; the recorder chain and whisper-cli are the ones
// Journal's voice notes use. A note is saved first and transcribed after, one
// at a time in the background, so recording never waits on whisper.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Local, NaiveDate, TimeZone};
use sqlx::SqlitePool;

use crate::api::journal::{self as journal, Rec};
use crate::db::voice_pool;
use crate::voice::{self as v, Seg};

// ------------------------------------------------------------------- state ---

pub struct VoiceState {
    /// notes | ask | tasks | setup.
    pub tab: String,
    pub notice: String,
    /// all | starred | tasks.
    pub filter: String,
    pub notes: Vec<VoiceNoteRow>,
    pub open: Option<VoiceNote>,
    pub recording: bool,
    /// Seconds into the recording, when the snapshot was taken.
    pub rec_secs: f64,
    pub rec_marks: i64,
    /// Notes waiting for whisper or being read by it.
    pub working: i64,
    pub query: String,
    pub hits: Vec<VoiceHit>,
    pub tasks: Vec<VoiceTask>,
    /// Not done yet, for the tab's count.
    pub tasks_open: i64,
    pub setup: VoiceSetup,
}

pub struct VoiceNoteRow {
    pub id: i64,
    pub title: String,
    /// "Today", "Yesterday", "Tue 22 Sep": the list's group.
    pub day: String,
    /// "14:02".
    pub time: String,
    /// "6:48".
    pub length: String,
    /// The first words, or why there are none yet.
    pub snippet: String,
    /// new | working | done | failed.
    pub state: String,
    pub starred: bool,
    pub tasks: i64,
    /// mic | file.
    pub source: String,
}

pub struct VoiceNote {
    pub id: i64,
    pub title: String,
    /// The WAV to play; empty until an imported file is converted.
    pub path: String,
    /// "Friday 25 September, 14:02".
    pub when: String,
    pub duration_s: f64,
    /// 0–1, for the waveform.
    pub peaks: Vec<f64>,
    pub state: String,
    pub error: String,
    pub segments: Vec<VoiceSeg>,
    /// Bookmarks, in milliseconds.
    pub marks: Vec<i64>,
    pub summary: Vec<String>,
    /// The model's name, or "" when the summary was picked from the words.
    pub summary_by: String,
    pub tasks: Vec<VoiceTask>,
    pub starred: bool,
    /// "25 Sep" once it is in Journal, else "".
    pub in_journal: String,
    /// The imported file's name, for a note that came from one.
    pub origin: String,
}

pub struct VoiceSeg {
    pub start_ms: i64,
    pub end_ms: i64,
    /// "1:48".
    pub at: String,
    pub text: String,
}

pub struct VoiceHit {
    pub note_id: i64,
    pub title: String,
    pub day: String,
    pub start_ms: i64,
    pub at: String,
    /// The words around the match, the match itself between « and ».
    pub text: String,
}

pub struct VoiceTask {
    pub id: i64,
    pub note_id: i64,
    pub note_title: String,
    pub text: String,
    pub at_ms: i64,
    pub at: String,
    /// ISO, or "".
    pub due: String,
    /// "Today", "Tomorrow", "Tue 29 Sep", or "".
    pub due_label: String,
    pub overdue: bool,
    pub done: bool,
}

pub struct VoiceSetup {
    /// whisper-cli and a model are both here.
    pub ready: bool,
    /// Why not, when not.
    pub why: String,
    /// The model file's name.
    pub model: String,
    /// "" follows Settings › AI Features; "auto" detects; else a code.
    pub lang: String,
    /// The summary server set up in Feeds, or "".
    pub summarizer: String,
    pub summarizer_model: String,
    pub notes: i64,
    pub bytes: i64,
    pub folder: String,
}

// ---------------------------------------------------------------- commands ---

pub enum VoiceCmd {
    Refresh,
    SetTab { tab: String },
    SetFilter { filter: String },
    Open { id: i64 },
    /// Start recording a new note.
    Record,
    /// Stop, save, and queue it for whisper.
    Stop,
    /// A bookmark at this moment of the recording.
    Mark,
    /// Audio or video files, copied in as notes.
    AddFiles { paths: Vec<String> },
    Rename { id: i64, title: String },
    Star { id: i64 },
    Delete { id: i64 },
    /// Read it again: a new model, or a language that was wrong.
    Retranscribe { id: i64 },
    /// Ask the summary server again.
    Summarise { id: i64 },
    Search { text: String },
    ToggleTask { id: i64 },
    DeleteTask { id: i64 },
    /// Dated tasks not yet done, as a calendar file.
    TasksToCalendar,
    /// The note as a Journal entry on the day it was made.
    ToJournal { id: i64 },
    SetLang { lang: String },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    filter: String,
    open: i64,
    query: String,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "notes".into(),
            filter: "all".into(),
            open: 0,
            query: String::new(),
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

/// The recording under way: the recorder, when it started, the bookmarks.
struct Live {
    rec: Rec,
    started: Instant,
    marks: Vec<i64>,
}

fn live() -> &'static Mutex<Option<Live>> {
    static L: OnceLock<Mutex<Option<Live>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

fn live_lock() -> MutexGuard<'static, Option<Live>> {
    match live().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn voice_dir() -> Result<PathBuf> {
    let dir = tulipix_core::paths::data_dir().context("no data folder")?.join("voice");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ---------------------------------------------------------------- exported ---

pub async fn voice_dispatch(cmd: VoiceCmd) -> Result<VoiceState> {
    let pool = voice_pool().await?;
    recover(pool).await;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// One note written to `path` as txt, srt or md.
pub async fn voice_export(id: i64, format: String, path: String) -> Result<()> {
    let pool = voice_pool().await?;
    let (title, created, summary): (String, i64, String) =
        sqlx::query_as("SELECT title, created, summary FROM notes WHERE id = ?").bind(id).fetch_one(pool).await?;
    let segs = segments(pool, id).await?;
    let body = match format.as_str() {
        "srt" => v::srt(&segs),
        "md" => {
            let tasks: Vec<(String, bool)> =
                sqlx::query_as::<_, (String, i64)>("SELECT text, done FROM tasks WHERE note_id = ? ORDER BY at_ms")
                    .bind(id)
                    .fetch_all(pool)
                    .await?
                    .into_iter()
                    .map(|(t, d)| (t, d != 0))
                    .collect();
            let lines: Vec<String> = summary.lines().map(str::to_string).collect();
            v::markdown(&title, &when(created), &lines, &tasks, &segs)
        }
        _ => v::plain(&segs),
    };
    tokio::fs::write(&path, body).await.with_context(|| format!("could not write {path}"))?;
    Ok(())
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: VoiceCmd) -> Result<()> {
    match cmd {
        VoiceCmd::Refresh => {}
        VoiceCmd::SetTab { tab } => lock().tab = tab,
        VoiceCmd::SetFilter { filter } => lock().filter = filter,
        VoiceCmd::Open { id } => {
            let mut s = lock();
            s.open = id;
            s.tab = "notes".into();
        }
        VoiceCmd::Record => {
            if live_lock().is_some() {
                bail!("already recording");
            }
            let path = voice_dir()?.join(format!("{}.wav", v::now()));
            let rec = journal::start_recorder(&path).await?;
            *live_lock() = Some(Live { rec, started: Instant::now(), marks: Vec::new() });
        }
        VoiceCmd::Mark => {
            if let Some(l) = live_lock().as_mut() {
                l.marks.push(l.started.elapsed().as_millis() as i64);
            }
        }
        VoiceCmd::Stop => {
            let Some(l) = live_lock().take() else { return Ok(()) };
            let path = l.rec.path.clone();
            journal::finish(l.rec).await;
            let bytes = tokio::fs::read(&path).await.unwrap_or_default();
            let (secs, peaks) = crate::journal::wave(&bytes, 120);
            if secs < 0.5 {
                std::fs::remove_file(&path).ok();
                bail!("nothing was recorded — is a microphone connected and allowed?");
            }
            let id = sqlx::query(
                "INSERT INTO notes (path, source, created, duration_s, peaks) VALUES (?, 'mic', ?, ?, ?)",
            )
            .bind(path.to_string_lossy().to_string())
            .bind(v::now())
            .bind(secs)
            .bind(join_peaks(&peaks))
            .execute(pool)
            .await?
            .last_insert_rowid();
            for at in l.marks {
                sqlx::query("INSERT INTO marks (note_id, at_ms) VALUES (?, ?)").bind(id).bind(at).execute(pool).await?;
            }
            {
                let mut s = lock();
                s.open = id;
                s.tab = "notes".into();
            }
            kick(pool);
        }
        VoiceCmd::AddFiles { paths } => {
            let mut last = 0;
            for p in &paths {
                let name = Path::new(p).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                last = sqlx::query("INSERT INTO notes (title, origin, source, created) VALUES (?, ?, 'file', ?)")
                    .bind(name)
                    .bind(p)
                    .bind(v::now())
                    .execute(pool)
                    .await?
                    .last_insert_rowid();
            }
            if last > 0 {
                lock().open = last;
                say(match paths.len() {
                    1 => "Added — transcribing it now".to_string(),
                    n => format!("Added {n} files — transcribing them one at a time"),
                });
                kick(pool);
            }
        }
        VoiceCmd::Rename { id, title } => {
            sqlx::query("UPDATE notes SET title = ? WHERE id = ?").bind(title.trim()).bind(id).execute(pool).await?;
        }
        VoiceCmd::Star { id } => {
            sqlx::query("UPDATE notes SET starred = 1 - starred WHERE id = ?").bind(id).execute(pool).await?;
        }
        VoiceCmd::Delete { id } => {
            let path: Option<String> = sqlx::query_scalar("SELECT path FROM notes WHERE id = ?").bind(id).fetch_optional(pool).await?;
            // Only our own copy: an imported file stays where the user keeps it.
            if let Some(p) = path.filter(|p| !p.is_empty()) {
                if voice_dir().is_ok_and(|d| Path::new(&p).starts_with(d)) {
                    std::fs::remove_file(&p).ok();
                }
            }
            forget_words(pool, id).await?;
            for q in ["DELETE FROM marks WHERE note_id = ?", "DELETE FROM tasks WHERE note_id = ?", "DELETE FROM notes WHERE id = ?"] {
                sqlx::query(q).bind(id).execute(pool).await?;
            }
            let mut s = lock();
            if s.open == id {
                s.open = 0;
            }
        }
        VoiceCmd::Retranscribe { id } => {
            sqlx::query("UPDATE notes SET state = 'new', error = '' WHERE id = ?").bind(id).execute(pool).await?;
            kick(pool);
        }
        VoiceCmd::Summarise { id } => {
            let (url, model) = summarizer();
            if url.is_empty() {
                bail!("no summary server — set one up in Feeds, and Voice uses it too");
            }
            model_summary(pool, id, &url, &model).await?;
        }
        VoiceCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            s.tab = "ask".into();
        }
        VoiceCmd::ToggleTask { id } => {
            sqlx::query("UPDATE tasks SET done = 1 - done WHERE id = ?").bind(id).execute(pool).await?;
        }
        VoiceCmd::DeleteTask { id } => {
            sqlx::query("DELETE FROM tasks WHERE id = ?").bind(id).execute(pool).await?;
        }
        VoiceCmd::TasksToCalendar => {
            let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
                "SELECT t.id, t.text, t.due, n.title FROM tasks t JOIN notes n ON n.id = t.note_id \
                 WHERE t.done = 0 AND t.due != '' ORDER BY t.due",
            )
            .fetch_all(pool)
            .await?;
            let events: Vec<crate::ics::Event> = rows
                .into_iter()
                .filter_map(|(id, text, due, title)| {
                    Some(crate::ics::Event {
                        uid: format!("voice-task-{id}"),
                        day: NaiveDate::parse_from_str(&due, "%Y-%m-%d").ok()?,
                        title: text,
                        note: format!("From the voice note “{title}”"),
                    })
                })
                .collect();
            if events.is_empty() {
                bail!("no tasks with a day on them");
            }
            crate::ics::open("voice-tasks", &events)?;
            say(format!("{} opened in your calendar", plural(events.len() as i64, "task")));
        }
        VoiceCmd::ToJournal { id } => {
            let (title, created, transcript, secs): (String, i64, String, f64) =
                sqlx::query_as("SELECT title, created, transcript, duration_s FROM notes WHERE id = ?")
                    .bind(id)
                    .fetch_one(pool)
                    .await?;
            if transcript.trim().is_empty() {
                bail!("there are no words to put in Journal yet");
            }
            let day = local(created).date_naive();
            let body = format!("**{}** · voice note, {}\n\n{transcript}", if title.is_empty() { "Voice note" } else { title.as_str() }, v::clock((secs * 1000.0) as i64));
            let jp = crate::db::journal_pool().await?;
            let entry = journal::new_entry(jp, day, &body).await?;
            sqlx::query("UPDATE notes SET journal_id = ? WHERE id = ?").bind(entry).bind(id).execute(pool).await?;
            say(format!("Added to Journal on {}", day.format("%-d %B")));
        }
        VoiceCmd::SetLang { lang } => crate::api::shell::put("voice.lang", lang.trim()),
    }
    Ok(())
}

fn join_peaks(p: &[f64]) -> String {
    p.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(",")
}

fn plural(n: i64, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

/// (address, model) of the summary server Feeds set up, both "" when none.
fn summarizer() -> (String, String) {
    let s = crate::api::shell::load();
    let url = s.text("feeds.summary-url");
    if url.trim().is_empty() { (String::new(), String::new()) } else { (url, s.text("feeds.summary-model")) }
}

async fn forget_words(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM segments WHERE note_id = ?").bind(id).execute(pool).await?;
    sqlx::query("DELETE FROM seg_fts WHERE note_id = ?").bind(id).execute(pool).await?;
    Ok(())
}

async fn segments(pool: &SqlitePool, id: i64) -> Result<Vec<Seg>> {
    let rows: Vec<(i64, i64, String)> =
        sqlx::query_as("SELECT start_ms, end_ms, text FROM segments WHERE note_id = ? ORDER BY idx")
            .bind(id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(start_ms, end_ms, text)| Seg { start_ms, end_ms, text }).collect())
}

// ------------------------------------------------------------------ worker ---

static WORKING: AtomicBool = AtomicBool::new(false);

/// A note left mid-transcription by a quit goes back in the queue, once per
/// run, and the queue starts.
async fn recover(pool: &'static SqlitePool) {
    static ONCE: OnceLock<()> = OnceLock::new();
    if ONCE.set(()).is_ok() {
        sqlx::query("UPDATE notes SET state = 'new' WHERE state = 'working'").execute(pool).await.ok();
        kick(pool);
    }
}

/// Start the transcriber unless it is already running. It takes waiting notes
/// one at a time, oldest first, until there are none.
fn kick(pool: &'static SqlitePool) {
    if WORKING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let next: Option<i64> =
                sqlx::query_scalar("SELECT id FROM notes WHERE state = 'new' ORDER BY id LIMIT 1").fetch_optional(pool).await.ok().flatten();
            let Some(id) = next else { break };
            sqlx::query("UPDATE notes SET state = 'working' WHERE id = ?").bind(id).execute(pool).await.ok();
            if let Err(e) = work(pool, id).await {
                sqlx::query("UPDATE notes SET state = 'failed', error = ? WHERE id = ?")
                    .bind(e.to_string())
                    .bind(id)
                    .execute(pool)
                    .await
                    .ok();
            }
        }
        WORKING.store(false, Ordering::SeqCst);
        // A note added between the last look and letting go.
        let waiting: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes WHERE state = 'new'").fetch_one(pool).await.unwrap_or(0);
        if waiting > 0 {
            kick(pool);
        }
    });
}

/// One note: an imported file converted, whisper over it, then the words
/// stored, a title, the tasks and a summary.
async fn work(pool: &'static SqlitePool, id: i64) -> Result<()> {
    let (mut path, origin, title, created): (String, String, String, i64) =
        sqlx::query_as("SELECT path, origin, title, created FROM notes WHERE id = ?").bind(id).fetch_one(pool).await?;
    if path.is_empty() {
        path = convert(&origin, id).await?;
        let bytes = tokio::fs::read(&path).await.unwrap_or_default();
        let (secs, peaks) = crate::journal::wave(&bytes, 120);
        sqlx::query("UPDATE notes SET path = ?, duration_s = ?, peaks = ? WHERE id = ?")
            .bind(&path)
            .bind(secs)
            .bind(join_peaks(&peaks))
            .bind(id)
            .execute(pool)
            .await?;
    }
    let segs = whisper(Path::new(&path)).await?;
    let text = v::plain(&segs);

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM segments WHERE note_id = ?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM seg_fts WHERE note_id = ?").bind(id).execute(&mut *tx).await?;
    for (i, s) in segs.iter().enumerate() {
        sqlx::query("INSERT INTO segments (note_id, idx, start_ms, end_ms, text) VALUES (?, ?, ?, ?, ?)")
            .bind(id)
            .bind(i as i64)
            .bind(s.start_ms)
            .bind(s.end_ms)
            .bind(&s.text)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO seg_fts (text, note_id, start_ms) VALUES (?, ?, ?)")
            .bind(&s.text)
            .bind(id)
            .bind(s.start_ms)
            .execute(&mut *tx)
            .await?;
    }
    // Tasks found before, and not ticked or kept by hand, give way to the new
    // reading's.
    sqlx::query("DELETE FROM tasks WHERE note_id = ? AND done = 0").bind(id).execute(&mut *tx).await?;
    let made = local(created).date_naive();
    for f in v::find_tasks(&segs, made) {
        sqlx::query("INSERT INTO tasks (note_id, text, at_ms, due, created) VALUES (?, ?, ?, ?, ?)")
            .bind(id)
            .bind(&f.text)
            .bind(f.at_ms)
            .bind(f.due.map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default())
            .bind(v::now())
            .execute(&mut *tx)
            .await?;
    }
    let summary = crate::feeds::summarise(&text).join("\n");
    let new_title = if title.trim().is_empty() || title.starts_with("Voice note") { v::auto_title(&text) } else { title };
    sqlx::query("UPDATE notes SET transcript = ?, title = ?, summary = ?, summary_by = '', state = 'done', error = '' WHERE id = ?")
        .bind(&text)
        .bind(new_title)
        .bind(summary)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // The model's summary is better when there is one, but a note is done
    // without it: a server that is off must not fail the transcript.
    let (url, model) = summarizer();
    if !url.is_empty() && text.split_whitespace().count() >= 40 {
        model_summary(pool, id, &url, &model).await.ok();
    }
    Ok(())
}

/// An imported file as a 16 kHz mono WAV in the voice folder, by ffmpeg.
async fn convert(origin: &str, id: i64) -> Result<String> {
    if origin.is_empty() {
        bail!("the recording is missing");
    }
    let ff = journal::tool("ffmpeg").ok_or_else(|| anyhow!("ffmpeg is missing"))?;
    let out = voice_dir()?.join(format!("{}-{id}.wav", v::now()));
    let status = tokio::process::Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(origin)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-y"])
        .arg(&out)
        .status()
        .await?;
    if !status.success() {
        std::fs::remove_file(&out).ok();
        bail!("could not read {} as audio", Path::new(origin).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    }
    Ok(out.to_string_lossy().to_string())
}

/// The speech model and the language to read with. Voice's own language wins,
/// then the one Settings › AI Features gives voice search.
fn language() -> String {
    let s = crate::api::shell::load();
    let own = s.text("voice.lang");
    if !own.trim().is_empty() {
        return own;
    }
    let shared = s.text("ai.voice-lang");
    if shared.trim().is_empty() { "en".into() } else { shared }
}

fn model() -> Option<PathBuf> {
    tulipix_core::ai_models::whisper_model_for("voice").or_else(crate::api::tools::whisper_model)
}

/// whisper-cli over a WAV, with times on every line.
async fn whisper(wav: &Path) -> Result<Vec<Seg>> {
    journal::transcribe_ready()?;
    let model = model().ok_or_else(|| anyhow!("no speech model — Settings › AI Features"))?;
    let bin = journal::tool("whisper-cli").ok_or_else(|| anyhow!("whisper-cli is missing"))?;
    let out = tokio::process::Command::new(&bin)
        .arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(wav)
        .args(["-l", &language()])
        .output()
        .await?;
    if !out.status.success() {
        bail!("whisper-cli stopped ({})", out.status);
    }
    let segs = v::parse_whisper(&String::from_utf8_lossy(&out.stdout));
    if segs.is_empty() {
        bail!("whisper heard no words in it");
    }
    Ok(segs)
}

async fn model_summary(pool: &SqlitePool, id: i64, url: &str, model: &str) -> Result<()> {
    let (title, text): (String, String) =
        sqlx::query_as("SELECT title, transcript FROM notes WHERE id = ?").bind(id).fetch_one(pool).await?;
    let clipped: String = text.chars().take(12000).collect();
    let answer = crate::feeds::chat(
        &crate::feeds::client(),
        url,
        model,
        "You summarise a voice note somebody recorded for themselves. Write at most three short \
         sentences in the second person: what it was about and what was decided. Plain sentences, \
         one per line, no bullets, no preamble.",
        &format!("{title}\n\n{clipped}"),
    )
    .await?;
    let lines = crate::feeds::summary_lines(&answer);
    if lines.is_empty() {
        bail!("the summary server sent an empty answer");
    }
    sqlx::query("UPDATE notes SET summary = ?, summary_by = ? WHERE id = ?")
        .bind(lines.join("\n"))
        .bind(if model.is_empty() { "local model" } else { model })
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

fn day_label(d: NaiveDate) -> String {
    let today = Local::now().date_naive();
    match (today - d).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        n if (2..7).contains(&n) => d.format("%A").to_string(),
        _ => d.format("%a %-d %b %Y").to_string(),
    }
}

fn when(ts: i64) -> String {
    local(ts).format("%A %-d %B %Y, %H:%M").to_string()
}

fn due_label(due: &str) -> (String, bool) {
    let Ok(d) = NaiveDate::parse_from_str(due, "%Y-%m-%d") else { return (String::new(), false) };
    let today = Local::now().date_naive();
    let label = match (d - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        _ => d.format("%a %-d %b").to_string(),
    };
    (label, d < today)
}

async fn task_rows(pool: &SqlitePool, note: Option<i64>) -> Result<Vec<VoiceTask>> {
    let rows: Vec<(i64, i64, String, String, i64, String, i64)> = sqlx::query_as(
        "SELECT t.id, t.note_id, n.title, t.text, t.at_ms, t.due, t.done FROM tasks t JOIN notes n ON n.id = t.note_id \
         WHERE (?1 = 0 OR t.note_id = ?1) \
         ORDER BY t.done, CASE WHEN t.due = '' THEN 1 ELSE 0 END, t.due, t.created DESC",
    )
    .bind(note.unwrap_or(0))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, note_id, note_title, text, at_ms, due, done)| {
            let (due_label, late) = due_label(&due);
            VoiceTask {
                id,
                note_id,
                note_title,
                text,
                at_ms,
                at: v::clock(at_ms),
                due,
                due_label,
                overdue: late && done == 0,
                done: done != 0,
            }
        })
        .collect())
}

async fn search(pool: &SqlitePool, q: &str) -> Result<Vec<VoiceHit>> {
    let fq = crate::api::papers::fts_query(q);
    if fq.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(i64, i64, String, String, i64)> = sqlx::query_as(
        "SELECT seg_fts.note_id, seg_fts.start_ms, snippet(seg_fts, 0, '«', '»', '…', 14), n.title, n.created \
         FROM seg_fts JOIN notes n ON n.id = seg_fts.note_id \
         WHERE seg_fts MATCH ? ORDER BY seg_fts.rank LIMIT 80",
    )
    .bind(&fq)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(note_id, start_ms, text, title, created)| VoiceHit {
            note_id,
            title,
            day: day_label(local(created).date_naive()),
            start_ms,
            at: v::clock(start_ms),
            text,
        })
        .collect())
}

fn folder_bytes() -> i64 {
    let Ok(dir) = voice_dir() else { return 0 };
    std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len() as i64).sum())
        .unwrap_or(0)
}

async fn snapshot(pool: &SqlitePool) -> Result<VoiceState> {
    let (tab, filter, open, query, notice) = {
        let mut s = lock();
        (s.tab.clone(), s.filter.clone(), s.open, s.query.clone(), std::mem::take(&mut s.notice))
    };
    let (recording, rec_secs, rec_marks) = match live_lock().as_ref() {
        Some(l) => (true, l.started.elapsed().as_secs_f64(), l.marks.len() as i64),
        None => (false, 0.0, 0),
    };

    let rows: Vec<(i64, String, i64, f64, String, String, i64, String, String, i64)> = sqlx::query_as(
        "SELECT n.id, n.title, n.created, n.duration_s, n.transcript, n.state, n.starred, n.source, n.error, \
                (SELECT COUNT(*) FROM tasks t WHERE t.note_id = n.id AND t.done = 0) \
         FROM notes n ORDER BY n.created DESC, n.id DESC",
    )
    .fetch_all(pool)
    .await?;
    let working = rows.iter().filter(|r| r.5 == "new" || r.5 == "working").count() as i64;
    let notes: Vec<VoiceNoteRow> = rows
        .into_iter()
        .filter(|r| match filter.as_str() {
            "starred" => r.6 != 0,
            "tasks" => r.9 > 0,
            _ => true,
        })
        .map(|(id, title, created, secs, transcript, state, starred, source, error, tasks)| {
            let t = local(created);
            let snippet = match state.as_str() {
                "new" => "Waiting to be transcribed".to_string(),
                "working" => "Transcribing…".to_string(),
                "failed" => format!("Not transcribed — {error}"),
                _ => crate::feeds::clip(&transcript, 140),
            };
            VoiceNoteRow {
                id,
                title: if title.is_empty() { format!("Voice note, {}", t.format("%H:%M")) } else { title },
                day: day_label(t.date_naive()),
                time: t.format("%H:%M").to_string(),
                length: v::clock((secs * 1000.0) as i64),
                snippet,
                state,
                starred: starred != 0,
                tasks,
                source,
            }
        })
        .collect();

    let open = if open > 0 { note_view(pool, open).await? } else { None };
    let hits = if query.is_empty() { Vec::new() } else { search(pool, &query).await? };
    let tasks = task_rows(pool, None).await?;
    let tasks_open = tasks.iter().filter(|t| !t.done).count() as i64;

    let (ready, why) = match journal::transcribe_ready() {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e.to_string()),
    };
    let (summarizer, summarizer_model) = summarizer();
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes").fetch_one(pool).await?;
    let setup = VoiceSetup {
        ready,
        why,
        model: model().and_then(|m| m.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default(),
        lang: crate::api::shell::load().text("voice.lang"),
        summarizer,
        summarizer_model,
        notes: total,
        bytes: folder_bytes(),
        folder: voice_dir().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
    };

    Ok(VoiceState {
        tab,
        notice,
        filter,
        notes,
        open,
        recording,
        rec_secs,
        rec_marks,
        working,
        query,
        hits,
        tasks,
        tasks_open,
        setup,
    })
}

async fn note_view(pool: &SqlitePool, id: i64) -> Result<Option<VoiceNote>> {
    type Row = (String, String, String, i64, f64, String, String, String, String, String, i64, i64);
    let row: Option<Row> = sqlx::query_as(
        "SELECT title, path, origin, created, duration_s, peaks, state, error, summary, summary_by, starred, journal_id \
         FROM notes WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((title, path, origin, created, duration_s, peaks, state, error, summary, summary_by, starred, journal_id)) = row else {
        lock().open = 0;
        return Ok(None);
    };
    let segments = segments(pool, id)
        .await?
        .into_iter()
        .map(|s| VoiceSeg { at: v::clock(s.start_ms), start_ms: s.start_ms, end_ms: s.end_ms, text: s.text })
        .collect();
    let marks: Vec<i64> =
        sqlx::query_scalar("SELECT at_ms FROM marks WHERE note_id = ? ORDER BY at_ms").bind(id).fetch_all(pool).await?;
    let in_journal = if journal_id > 0 { local(created).format("%-d %b").to_string() } else { String::new() };
    Ok(Some(VoiceNote {
        id,
        title: if title.is_empty() { format!("Voice note, {}", local(created).format("%H:%M")) } else { title },
        path,
        when: when(created),
        duration_s,
        peaks: peaks.split(',').filter_map(|p| p.parse().ok()).collect(),
        state,
        error,
        segments,
        marks,
        summary: summary.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect(),
        summary_by,
        tasks: task_rows(pool, Some(id)).await?,
        starred: starred != 0,
        in_journal,
        origin: Path::new(&origin).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
    }))
}
