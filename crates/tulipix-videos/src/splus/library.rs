//! Saved titles, watch progress, history and recent search terms.
//!
//! Stream Plus keeps its own tables rather than adding a column to the Stream
//! tab's: that tab is not modified by this feature, and a shared table is the
//! one thing that would make a later change to either tab a change to both.

use anyhow::Result;
use sqlx::{Row, SqlitePool};
use tulipix_core::util::unix_secs_i64 as now;

use super::{EpisodeRef, Title, prefs};

/// A saved title, as the Library grid draws it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    pub key: String,
    pub anilist_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    pub title: String,
    pub english: String,
    pub year: Option<i64>,
    pub cover_url: String,
    pub overview: String,
    pub format: String,
    pub episodes: i64,
    pub certification: String,
    /// Furthest episode with progress, and how far into it.
    pub last_episode: i64,
    pub last_season: i64,
    pub progress: f32,
    pub added_at: i64,
}

/// One row of the History list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistoryRow {
    pub key: String,
    pub title_key: String,
    pub title: String,
    /// "Series · S02E07" / "Movie"
    pub kind: String,
    /// "62% · 18 min left" / "Watched"
    pub state: String,
    /// "today 21:14" / "2 days ago"
    pub when: String,
    pub source: String,
    pub audio: String,
    pub quality: String,
    pub cover_url: String,
    pub season: i64,
    pub episode: i64,
    pub progress: f32,
    pub finished: bool,
}

fn title_key(t: &Title) -> String {
    match (t.anilist_id, t.tmdb_id) {
        (Some(a), _) => format!("al:{a}"),
        (None, Some(x)) => format!("tmdb:{x}"),
        _ => format!("q:{}", t.title),
    }
}

// ── bookmarks ─────────────────────────────────────────────────────────────

pub async fn is_saved(pool: &SqlitePool, t: &Title) -> bool {
    sqlx::query("SELECT 1 FROM splus_bookmarks WHERE key = ?1")
        .bind(title_key(t))
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Save or unsave. Returns the state it ended in, so the caller does not have to
/// ask again to paint the button.
pub async fn toggle_saved(pool: &SqlitePool, t: &Title) -> Result<bool> {
    let key = title_key(t);
    if is_saved(pool, t).await {
        sqlx::query("DELETE FROM splus_bookmarks WHERE key = ?1")
            .bind(&key)
            .execute(pool)
            .await?;
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR REPLACE INTO splus_bookmarks
         (key, anilist_id, tmdb_id, title, english, year, cover_url, overview,
          format, episodes, certification, seen_ep, latest_ep, checked_at, added_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12,0,?13)",
    )
    .bind(&key)
    .bind(t.anilist_id)
    .bind(t.tmdb_id)
    .bind(&t.title)
    .bind(t.english.clone().unwrap_or_default())
    .bind(t.year)
    .bind(&t.cover_url)
    .bind(&t.overview)
    .bind(&t.format)
    .bind(t.episodes.unwrap_or(0))
    .bind(&t.certification)
    .bind(t.next_episode.map(|e| e - 1).unwrap_or(t.episodes.unwrap_or(0)))
    .bind(now())
    .execute(pool)
    .await?;
    Ok(true)
}

pub async fn remove_saved(pool: &SqlitePool, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM splus_bookmarks WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// How the Library grid is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Added,
    Watched,
    TitleAz,
    Year,
    Progress,
}

impl Sort {
    pub fn parse(s: &str) -> Self {
        match s {
            "watched" => Self::Watched,
            "title" => Self::TitleAz,
            "year" => Self::Year,
            "progress" => Self::Progress,
            _ => Self::Added,
        }
    }
}

pub async fn saved(pool: &SqlitePool, sort: Sort) -> Vec<Saved> {
    let rows = sqlx::query(
        "SELECT b.key, b.anilist_id, b.tmdb_id, b.title, b.english, b.year, b.cover_url,
                b.overview, b.format, b.episodes, b.certification, b.added_at,
                COALESCE(p.episode, 0) AS last_ep,
                COALESCE(p.season, 1)  AS last_season,
                COALESCE(CASE WHEN p.duration_s > 0 THEN p.position_s / p.duration_s ELSE 0 END, 0) AS frac,
                COALESCE(p.played_at, 0) AS played_at
         FROM splus_bookmarks b
         LEFT JOIN (
            SELECT title_key, season, episode, position_s, duration_s, played_at,
                   ROW_NUMBER() OVER (PARTITION BY title_key ORDER BY played_at DESC) AS rn
            FROM splus_progress
         ) p ON p.title_key = b.key AND p.rn = 1",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut out: Vec<Saved> = rows
        .iter()
        .map(|r| Saved {
            key: r.try_get("key").unwrap_or_default(),
            anilist_id: r.try_get("anilist_id").ok(),
            tmdb_id: r.try_get("tmdb_id").ok(),
            title: r.try_get("title").unwrap_or_default(),
            english: r.try_get("english").unwrap_or_default(),
            year: r.try_get("year").ok(),
            cover_url: r.try_get("cover_url").unwrap_or_default(),
            overview: r.try_get("overview").unwrap_or_default(),
            format: r.try_get("format").unwrap_or_else(|_| "TV".into()),
            episodes: r.try_get("episodes").unwrap_or(0),
            certification: r.try_get("certification").unwrap_or_default(),
            last_episode: r.try_get("last_ep").unwrap_or(0),
            last_season: r.try_get("last_season").unwrap_or(1),
            progress: r.try_get::<f64, _>("frac").unwrap_or(0.0) as f32,
            added_at: r.try_get("added_at").unwrap_or(0),
        })
        .collect();

    let played: Vec<i64> = rows.iter().map(|r| r.try_get("played_at").unwrap_or(0)).collect();
    match sort {
        Sort::Added => out.sort_by(|a, b| b.added_at.cmp(&a.added_at)),
        Sort::TitleAz => out.sort_by(|a, b| {
            a.display().to_lowercase().cmp(&b.display().to_lowercase())
        }),
        Sort::Year => out.sort_by(|a, b| b.year.cmp(&a.year)),
        Sort::Progress => out.sort_by(|a, b| b.progress.total_cmp(&a.progress)),
        Sort::Watched => {
            let mut zipped: Vec<_> = out.into_iter().zip(played).collect();
            zipped.sort_by(|a, b| b.1.cmp(&a.1));
            out = zipped.into_iter().map(|(s, _)| s).collect();
        }
    }
    out
}

impl Saved {
    pub fn display(&self) -> &str {
        if self.english.is_empty() { &self.title } else { &self.english }
    }
}

// ── progress and history ──────────────────────────────────────────────────

/// Record where playback got to. Called once, when mpv exits.
pub async fn note_progress(
    pool: &SqlitePool,
    ep: &EpisodeRef,
    source: &str,
    quality: &str,
    position_s: f64,
    duration_s: f64,
) -> Result<()> {
    // A session under a minute is a misclick or a failed resolve, not a watch.
    if position_s < 60.0 && duration_s > 0.0 {
        return Ok(());
    }
    let finished = duration_s > 0.0 && (position_s / duration_s) >= f64::from(prefs::watched_threshold());
    sqlx::query(
        "INSERT OR REPLACE INTO splus_progress
         (key, title_key, title, label, season, episode, audio, source, quality,
          cover_url, position_s, duration_s, finished, played_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
    )
    .bind(ep.key())
    .bind(title_key(&ep.title))
    .bind(ep.title.display_title())
    .bind(ep.label())
    .bind(ep.season)
    .bind(ep.episode)
    .bind(&ep.audio)
    .bind(source)
    .bind(quality)
    .bind(&ep.title.cover_url)
    .bind(position_s)
    .bind(duration_s)
    .bind(i64::from(finished))
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Where to resume this episode, or `None` if it was finished or never started.
pub async fn resume_at(pool: &SqlitePool, ep: &EpisodeRef) -> Option<f64> {
    let row = sqlx::query(
        "SELECT position_s, duration_s, finished FROM splus_progress WHERE key = ?1",
    )
    .bind(ep.key())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    if row.try_get::<i64, _>("finished").unwrap_or(0) == 1 {
        return None;
    }
    let pos: f64 = row.try_get("position_s").unwrap_or(0.0);
    let dur: f64 = row.try_get("duration_s").unwrap_or(0.0);
    // Right at the end is "watched", right at the start is "not started".
    (pos > 30.0 && (dur <= 0.0 || pos < dur - 30.0)).then_some(pos)
}

/// Per-episode progress for a title, for the episode strip.
pub async fn progress_map(pool: &SqlitePool, t: &Title) -> std::collections::HashMap<i64, f32> {
    let rows = sqlx::query(
        "SELECT episode, position_s, duration_s, finished FROM splus_progress WHERE title_key = ?1",
    )
    .bind(title_key(t))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| {
            let ep: i64 = r.try_get("episode").unwrap_or(0);
            let done = r.try_get::<i64, _>("finished").unwrap_or(0) == 1;
            let pos: f64 = r.try_get("position_s").unwrap_or(0.0);
            let dur: f64 = r.try_get("duration_s").unwrap_or(0.0);
            let frac = if done {
                1.0
            } else if dur > 0.0 {
                (pos / dur) as f32
            } else {
                0.0
            };
            (ep, frac.clamp(0.0, 1.0))
        })
        .collect()
}

pub async fn history(pool: &SqlitePool, limit: i64) -> Vec<HistoryRow> {
    let rows = sqlx::query(
        "SELECT * FROM splus_progress ORDER BY played_at DESC LIMIT ?1",
    )
    .bind(limit.max(1))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| {
            let pos: f64 = r.try_get("position_s").unwrap_or(0.0);
            let dur: f64 = r.try_get("duration_s").unwrap_or(0.0);
            let finished = r.try_get::<i64, _>("finished").unwrap_or(0) == 1;
            let frac = if finished {
                1.0
            } else if dur > 0.0 {
                (pos / dur) as f32
            } else {
                0.0
            };
            let label: String = r.try_get("label").unwrap_or_default();
            HistoryRow {
                key: r.try_get("key").unwrap_or_default(),
                title_key: r.try_get("title_key").unwrap_or_default(),
                title: r.try_get("title").unwrap_or_default(),
                kind: if label.is_empty() { "Movie".into() } else { format!("Series · {label}") },
                state: state_line(frac, pos, dur, finished),
                when: relative(r.try_get("played_at").unwrap_or(0)),
                source: r.try_get("source").unwrap_or_default(),
                audio: r.try_get("audio").unwrap_or_default(),
                quality: r.try_get("quality").unwrap_or_default(),
                cover_url: r.try_get("cover_url").unwrap_or_default(),
                season: r.try_get("season").unwrap_or(1),
                episode: r.try_get("episode").unwrap_or(0),
                progress: frac.clamp(0.0, 1.0),
                finished,
            }
        })
        .collect()
}

/// Titles with something part-watched, newest first — the Continue row.
pub async fn continue_watching(pool: &SqlitePool, limit: i64) -> Vec<HistoryRow> {
    history(pool, limit * 4)
        .await
        .into_iter()
        .filter(|h| !h.finished && h.progress > 0.02)
        .take(limit.max(1) as usize)
        .collect()
}

pub async fn remove_history(pool: &SqlitePool, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM splus_progress WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_history(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM splus_progress").execute(pool).await?;
    Ok(())
}

fn state_line(frac: f32, pos: f64, dur: f64, finished: bool) -> String {
    if finished {
        return "Watched".into();
    }
    if dur <= 0.0 {
        return format!("{}% ", (frac * 100.0).round() as i64);
    }
    let left = ((dur - pos) / 60.0).ceil().max(1.0) as i64;
    format!("{}% · {left} min left", (frac * 100.0).round() as i64)
}

fn relative(ts: i64) -> String {
    let d = now() - ts;
    match d {
        _ if d < 60 => "just now".into(),
        _ if d < 3600 => format!("{} min ago", d / 60),
        _ if d < 86_400 => format!("{} h ago", d / 3600),
        _ if d < 172_800 => "yesterday".into(),
        _ if d < 2_592_000 => format!("{} days ago", d / 86_400),
        _ => format!("{} months ago", (d / 2_592_000).max(1)),
    }
}

// ── recent search terms ───────────────────────────────────────────────────

pub async fn note_term(pool: &SqlitePool, term: &str) {
    let t = term.trim();
    if t.len() < 2 {
        return;
    }
    let _ = sqlx::query(
        "INSERT INTO splus_recent (term, used_at) VALUES (?1, ?2)
         ON CONFLICT(term) DO UPDATE SET used_at = ?2",
    )
    .bind(t)
    .bind(now())
    .execute(pool)
    .await;
}

pub async fn recent(pool: &SqlitePool, limit: i64) -> Vec<String> {
    sqlx::query("SELECT term FROM splus_recent ORDER BY used_at DESC LIMIT ?1")
        .bind(limit.max(1))
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|r| r.try_get::<String, _>("term").ok())
        .collect()
}

pub async fn clear_recent(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM splus_recent").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_line_reads_as_time_left_not_a_bare_percent() {
        assert_eq!(state_line(0.0, 0.0, 0.0, true), "Watched");
        assert_eq!(state_line(0.62, 1200.0, 1500.0, false), "62% · 5 min left");
    }

    #[test]
    fn title_key_prefixes_keep_the_two_lanes_apart() {
        let anime = Title { anilist_id: Some(7), ..Title::default() };
        let film = Title { tmdb_id: Some(7), ..Title::default() };
        assert_ne!(title_key(&anime), title_key(&film));
    }

    #[test]
    fn episode_keys_are_unique_per_episode_but_not_for_a_film() {
        let t = Title { anilist_id: Some(1), format: "TV".into(), ..Title::default() };
        let a = EpisodeRef { title: t.clone(), season: 1, episode: 1, audio: "sub".into() };
        let b = EpisodeRef { episode: 2, ..a.clone() };
        assert_ne!(a.key(), b.key());

        let film = Title { anilist_id: Some(1), format: "MOVIE".into(), ..Title::default() };
        let f1 = EpisodeRef { title: film, season: 1, episode: 1, audio: "sub".into() };
        assert_eq!(f1.key(), "al:1");
    }
}
