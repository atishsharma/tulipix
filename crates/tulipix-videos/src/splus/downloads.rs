//! The Stream Plus download queue.
//!
//! A progressive file is streamed to disk here; an HLS playlist is handed to
//! ffmpeg, which is already bundled and already how the rest of the app remuxes.
//! The queue lives in a table so a restart resumes it rather than forgetting it,
//! and one worker per slot keeps a rotating host from being hammered.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};
use tulipix_core::util::unix_secs_i64 as now;

use super::source::{Playable, PlayableKind};
use super::{EpisodeRef, prefs};

/// One row of the Downloads page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DownloadRow {
    pub id: i64,
    pub title: String,
    pub label: String,
    pub quality: String,
    pub source: String,
    pub dest: String,
    /// waiting | running | done | failed | missing
    pub state: String,
    pub detail: String,
    pub done_bytes: i64,
    pub total_bytes: i64,
    pub progress: f32,
    pub batch: String,
}

impl DownloadRow {
    pub fn active(&self) -> bool {
        self.state == "waiting" || self.state == "running"
    }
    pub fn done(&self) -> bool {
        self.state == "done"
    }
    pub fn failed(&self) -> bool {
        self.state == "failed" || self.state == "missing"
    }
}

/// Add one episode to the queue. `batch` groups a season download so it can be
/// cancelled and reported as one thing.
pub async fn enqueue(
    pool: &SqlitePool,
    ep: &EpisodeRef,
    file: &Playable,
    sub_url: &str,
    batch: &str,
) -> Result<i64> {
    let dest = destination(ep, file);
    let res = sqlx::query(
        "INSERT INTO splus_downloads
         (key, title, label, quality, source, url, kind, dest, sub_url, state, detail, batch, added_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'waiting','',?10,?11)",
    )
    .bind(ep.key())
    .bind(ep.title.display_title())
    .bind(ep.label())
    .bind(&file.label)
    .bind(file.source)
    .bind(&file.url)
    .bind(file.kind.as_str())
    .bind(dest.to_string_lossy().to_string())
    .bind(sub_url)
    .bind(batch)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

/// Where a download lands. Kept flat and predictable so the file is findable
/// without the app.
fn destination(ep: &EpisodeRef, file: &Playable) -> PathBuf {
    let dir = prefs::download_dir();
    let title = sanitise(ep.title.display_title());
    let stem = if ep.title.is_series() {
        format!("{title} - {}", ep.label())
    } else {
        title
    };
    let quality = if file.label.is_empty() { String::new() } else { format!(" [{}]", file.label) };
    dir.join(format!("{stem}{quality}.mp4"))
}

/// Strip what no filesystem in the fleet accepts, and collapse the run of
/// spaces that removing them leaves behind.
fn sanitise(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if r#"/\:*?"<>|"#.contains(c) { ' ' } else { c })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub async fn list(pool: &SqlitePool) -> Vec<DownloadRow> {
    sqlx::query(
        "SELECT * FROM splus_downloads
         ORDER BY CASE state WHEN 'running' THEN 0 WHEN 'waiting' THEN 1
                             WHEN 'failed' THEN 2 WHEN 'missing' THEN 3 ELSE 4 END,
                  added_at DESC",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .iter()
    .map(to_row)
    .collect()
}

fn to_row(r: &sqlx::sqlite::SqliteRow) -> DownloadRow {
    let done: i64 = r.try_get("done_bytes").unwrap_or(0);
    let total: i64 = r.try_get("total_bytes").unwrap_or(0);
    DownloadRow {
        id: r.try_get("id").unwrap_or(0),
        title: r.try_get("title").unwrap_or_default(),
        label: r.try_get("label").unwrap_or_default(),
        quality: r.try_get("quality").unwrap_or_default(),
        source: r.try_get("source").unwrap_or_default(),
        dest: r.try_get("dest").unwrap_or_default(),
        state: r.try_get("state").unwrap_or_else(|_| "waiting".into()),
        detail: r.try_get("detail").unwrap_or_default(),
        done_bytes: done,
        total_bytes: total,
        progress: if total > 0 { (done as f64 / total as f64) as f32 } else { 0.0 },
        batch: r.try_get("batch").unwrap_or_default(),
    }
}

async fn set_state(pool: &SqlitePool, id: i64, state: &str, detail: &str) {
    let _ = sqlx::query("UPDATE splus_downloads SET state = ?2, detail = ?3 WHERE id = ?1")
        .bind(id)
        .bind(state)
        .bind(detail)
        .execute(pool)
        .await;
}

async fn set_bytes(pool: &SqlitePool, id: i64, done: i64, total: i64) {
    let _ = sqlx::query("UPDATE splus_downloads SET done_bytes = ?2, total_bytes = ?3 WHERE id = ?1")
        .bind(id)
        .bind(done)
        .bind(total)
        .execute(pool)
        .await;
}

/// A claimed row: id, url, kind, dest, sub_url.
type Job = (i64, String, String, String, String);

/// Claim the next waiting row, if a slot is free.
async fn claim(pool: &SqlitePool) -> Option<Job> {
    let running: i64 = sqlx::query("SELECT COUNT(*) AS n FROM splus_downloads WHERE state = 'running'")
        .fetch_one(pool)
        .await
        .ok()
        .and_then(|r| r.try_get("n").ok())
        .unwrap_or(0);
    if running as usize >= prefs::download_slots() {
        return None;
    }
    let row = sqlx::query(
        "SELECT id, url, kind, dest, sub_url FROM splus_downloads
         WHERE state = 'waiting' ORDER BY added_at LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    let id: i64 = row.try_get("id").ok()?;
    set_state(pool, id, "running", "starting…").await;
    Some((
        id,
        row.try_get("url").unwrap_or_default(),
        row.try_get("kind").unwrap_or_else(|_| "mp4".into()),
        row.try_get("dest").unwrap_or_default(),
        row.try_get("sub_url").unwrap_or_default(),
    ))
}

/// Run whatever is queued until nothing is left or a slot cap is hit.
///
/// Called after every enqueue and once at startup; it is a no-op when the queue
/// is empty, so calling it too often costs one count query.
///
/// Each spawned task drains the queue itself rather than the finished task
/// calling `pump` again. That recursion described the future's type in terms of
/// itself, so rustc could not settle whether it was `Send` and `tokio::spawn`
/// rejected it. A worker that loops is also the plainer read: one task owns one
/// slot for as long as there is work for it.
pub async fn pump(pool: &SqlitePool) {
    while let Some(job) = claim(pool).await {
        let pool2 = pool.clone();
        tokio::spawn(async move {
            let mut job = job;
            loop {
                run_one(&pool2, job).await;
                // A finished slot is a free slot.
                match claim(&pool2).await {
                    Some(next) => job = next,
                    None => break,
                }
            }
        });
    }
}

/// One queued file, start to finish. A failure lands in the row rather than
/// propagating — the queue outlives any single download.
async fn run_one(pool: &SqlitePool, (id, url, kind, dest, sub_url): Job) {
    let dest = PathBuf::from(&dest);
    if let Some(parent) = dest.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let outcome = match PlayableKind::parse(&kind) {
        PlayableKind::Mp4 => fetch_file(pool, id, &url, &dest).await,
        PlayableKind::M3u8 => remux(&url, &dest).await,
    };
    match outcome {
        Ok(()) => {
            if prefs::subs_with_file() && !sub_url.is_empty() {
                if let Err(e) = fetch_subtitle(&sub_url, &dest).await {
                    tracing::warn!(error = %e, "splus: subtitle not saved beside the file");
                }
            }
            let size = tokio::fs::metadata(&dest).await.map(|m| m.len() as i64).unwrap_or(0);
            set_bytes(pool, id, size, size).await;
            set_state(pool, id, "done", &dest.to_string_lossy()).await;
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            set_state(pool, id, "failed", &friendly(&e)).await;
        }
    }
}

/// Turn a chain of context into the one line a row can show.
fn friendly(e: &anyhow::Error) -> String {
    let s = e.to_string();
    match s.char_indices().nth(90) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

async fn fetch_file(pool: &SqlitePool, id: i64, url: &str, dest: &Path) -> Result<()> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let resp = tulipix_core::net::http()
        .get(url)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await
        .context("the host did not answer")?;
    if !resp.status().is_success() {
        anyhow::bail!("the host answered {}", resp.status().as_u16());
    }
    let total = resp.content_length().unwrap_or(0) as i64;
    set_bytes(pool, id, 0, total).await;

    let mut file = tokio::fs::File::create(dest).await.context("could not create the file")?;
    let mut stream = resp.bytes_stream();
    let mut done: i64 = 0;
    let mut last_report = 0i64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("the transfer was cut short")?;
        file.write_all(&chunk).await.context("could not write to disk")?;
        done += chunk.len() as i64;
        // One row update per megabyte, not per chunk — the UI cannot see faster
        // and the DB should not be asked to.
        if done - last_report > 1_000_000 {
            last_report = done;
            set_bytes(pool, id, done, total).await;
        }
    }
    file.flush().await.ok();
    set_bytes(pool, id, done, total.max(done)).await;
    Ok(())
}

/// HLS → mp4 with the bundled ffmpeg. Stream copy: no re-encode, so this is
/// disk-bound rather than CPU-bound.
async fn remux(url: &str, dest: &Path) -> Result<()> {
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let out = tokio::process::Command::new(ff)
        .arg("-y")
        .arg("-loglevel").arg("error")
        .arg("-user_agent").arg(tulipix_core::net::BROWSER_UA)
        .arg("-i").arg(url)
        .arg("-c").arg("copy")
        .arg("-bsf:a").arg("aac_adtstoasc")
        .arg(dest)
        .output()
        .await
        .context("ffmpeg could not be started")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err.lines().last().unwrap_or("ffmpeg failed").trim();
        anyhow::bail!("{line}");
    }
    Ok(())
}

async fn fetch_subtitle(url: &str, video: &Path) -> Result<()> {
    let body = tulipix_core::net::http()
        .get(url)
        .send()
        .await
        .context("subtitle download failed")?
        .bytes()
        .await?;
    let stem = video.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dest = video.with_file_name(format!("{stem}.srt"));
    tokio::fs::write(dest, &body).await.context("could not write the subtitle")?;
    Ok(())
}

// ── row actions ───────────────────────────────────────────────────────────

pub async fn cancel(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM splus_downloads WHERE id = ?1").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn retry(pool: &SqlitePool, id: i64) -> Result<()> {
    set_state(pool, id, "waiting", "").await;
    set_bytes(pool, id, 0, 0).await;
    pump(pool).await;
    Ok(())
}

pub async fn cancel_batch(pool: &SqlitePool, batch: &str) -> Result<()> {
    sqlx::query("DELETE FROM splus_downloads WHERE batch = ?1 AND state IN ('waiting','running')")
        .bind(batch)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn cancel_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM splus_downloads WHERE state IN ('waiting','running')")
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_finished(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM splus_downloads WHERE state IN ('done','failed','missing')")
        .execute(pool)
        .await?;
    Ok(())
}

/// Forget a row and delete what it produced.
pub async fn delete_file(pool: &SqlitePool, id: i64) -> Result<()> {
    if let Ok(Some(r)) = sqlx::query("SELECT dest FROM splus_downloads WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await
    {
        if let Ok(dest) = r.try_get::<String, _>("dest") {
            let _ = tokio::fs::remove_file(&dest).await;
        }
    }
    cancel(pool, id).await
}

/// Mark finished rows whose file has since been moved or deleted.
///
/// Run on entering the tab: a row that says "Saved" and points at nothing is the
/// single most confusing state a download list can be in.
pub async fn reconcile(pool: &SqlitePool) {
    let rows = sqlx::query("SELECT id, dest FROM splus_downloads WHERE state = 'done'")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for r in rows {
        let (Ok(id), Ok(dest)) = (r.try_get::<i64, _>("id"), r.try_get::<String, _>("dest")) else {
            continue;
        };
        if tokio::fs::metadata(&dest).await.is_err() {
            set_state(pool, id, "missing", "the file is no longer where it was saved").await;
        }
    }
    // Anything left 'running' from a previous session is not running now.
    let _ = sqlx::query("UPDATE splus_downloads SET state = 'waiting' WHERE state = 'running'")
        .execute(pool)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::splus::Title;

    fn ep(series: bool) -> EpisodeRef {
        EpisodeRef {
            title: Title {
                anilist_id: Some(1),
                title: "Sousou no Frieren".into(),
                english: Some("Frieren: Beyond Journey's End".into()),
                format: if series { "TV".into() } else { "MOVIE".into() },
                ..Title::default()
            },
            season: 1,
            episode: 13,
            audio: "sub".into(),
        }
    }

    #[test]
    fn a_series_file_carries_its_episode_a_film_does_not() {
        let mut f = Playable::new("u", PlayableKind::Mp4, "allmanga");
        f.label = "1080p".into();
        let s = destination(&ep(true), &f);
        assert!(s.to_string_lossy().ends_with("Frieren: Beyond Journey's End - S01E13 [1080p].mp4"));
        let m = destination(&ep(false), &f);
        assert!(m.to_string_lossy().ends_with("Frieren: Beyond Journey's End [1080p].mp4"));
    }

    #[test]
    fn path_separators_never_survive_into_a_filename() {
        assert_eq!(sanitise("Re:Zero / Season 2"), "Re Zero Season 2");
        assert_eq!(sanitise(r#"a"b<c>d|e*f?g"#), "a b c d e f g");
    }

    #[test]
    fn row_state_predicates_agree_with_the_ui_filters() {
        let mk = |s: &str| DownloadRow { state: s.into(), ..DownloadRow::default() };
        assert!(mk("waiting").active() && mk("running").active());
        assert!(mk("done").done());
        assert!(mk("failed").failed() && mk("missing").failed());
        assert!(!mk("done").active() && !mk("done").failed());
    }
}
