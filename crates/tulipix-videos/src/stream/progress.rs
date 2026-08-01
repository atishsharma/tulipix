//! Watch progress for remote Stream titles.
//!
//! The local library tracks progress in `watch_progress`, keyed by an `items`
//! row — which a remote title does not have. This is the parallel table for
//! the catalogue, keyed by what does identify a remote episode:
//! `(subject_id, season, episode)`. A movie is season 0, episode 0.
//!
//! Title and cover are snapshotted onto the row so the Continue Watching strip
//! and the history page render without a detail fetch per entry.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS stream_progress (
    subject_id  TEXT    NOT NULL,
    season      INTEGER NOT NULL,
    episode     INTEGER NOT NULL,
    title       TEXT    NOT NULL DEFAULT '',
    cover_url   TEXT    NOT NULL DEFAULT '',
    is_series   INTEGER NOT NULL DEFAULT 0,
    position_s  REAL    NOT NULL DEFAULT 0,
    duration_s  REAL    NOT NULL DEFAULT 0,
    finished    INTEGER NOT NULL DEFAULT 0,
    updated     INTEGER NOT NULL,
    source      TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (subject_id, season, episode)
);
CREATE INDEX IF NOT EXISTS stream_progress_updated_idx ON stream_progress(updated DESC);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    // `CREATE TABLE IF NOT EXISTS` leaves an existing table alone, so the column
    // has to be added separately for anyone who already has a progress table.
    // Failure is the "column already exists" case and nothing else worth acting
    // on — rows written before this land with '', which reads as "unknown" and
    // falls back to whichever source is selected.
    let _ = sqlx::query("ALTER TABLE stream_progress ADD COLUMN source TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    Ok(())
}

/// Watched this far through and the episode counts as done: it stops offering a
/// resume and becomes the trigger for playing the next one.
pub const FINISHED_AT: f64 = 0.9;

/// Below this many seconds there is nothing worth resuming — the user pressed
/// play, changed their mind, and does not want to be asked about it later.
pub const RESUME_FLOOR_S: f64 = 60.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub subject_id: String,
    pub season: i64,
    pub episode: i64,
    pub title: String,
    pub cover_url: String,
    pub is_series: bool,
    pub position_s: f64,
    pub duration_s: f64,
    pub finished: bool,
    pub updated: i64,
    /// Which catalogue this was played from (`Source::key`). Empty on rows
    /// written before the column existed, and on those the caller falls back to
    /// whichever source is selected rather than guessing.
    pub source: String,
}

impl Entry {
    /// How far through, `0.0..=1.0`. Zero when the length is unknown — a
    /// progress bar that guesses is worse than one that stays empty.
    pub fn fraction(&self) -> f64 {
        if self.duration_s <= 0.0 {
            return 0.0;
        }
        (self.position_s / self.duration_s).clamp(0.0, 1.0)
    }

    /// Seconds remaining, or `None` when the length is unknown.
    pub fn remaining_s(&self) -> Option<f64> {
        (self.duration_s > 0.0).then(|| (self.duration_s - self.position_s).max(0.0))
    }

    /// Where playback should pick up, or `None` to start from the beginning.
    pub fn resume_at(&self) -> Option<f64> {
        (!self.finished && self.position_s > RESUME_FLOOR_S).then_some(self.position_s)
    }
}

/// Did this playback reach the end? Unknown length means no — better to offer a
/// resume that was not needed than to bury an episode the user never finished.
pub fn is_finished(position_s: f64, duration_s: f64) -> bool {
    duration_s > 0.0 && position_s >= duration_s * FINISHED_AT
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type Row = (String, i64, i64, String, String, i64, f64, f64, i64, i64, String);

const COLUMNS: &str = "subject_id, season, episode, title, cover_url, is_series, \
                       position_s, duration_s, finished, updated, source";

fn entry_of(r: Row) -> Entry {
    Entry {
        subject_id: r.0,
        season: r.1,
        episode: r.2,
        title: r.3,
        cover_url: r.4,
        is_series: r.5 != 0,
        position_s: r.6,
        duration_s: r.7,
        finished: r.8 != 0,
        updated: r.9,
        source: r.10,
    }
}

/// Write where playback got to. `finished` is derived, not supplied, so every
/// caller agrees on what "watched" means.
///
/// A blank subject id is ignored: it would collide with every other unidentified
/// title on the primary key and show up as one nonsense history row.
///
/// `updated` is normally left at 0 and filled in from the clock; a caller that
/// sets it (the tests, replaying a sequence) keeps its own value.
pub async fn record(pool: &SqlitePool, e: &Entry) -> Result<()> {
    if e.subject_id.is_empty() {
        return Ok(());
    }
    let finished = is_finished(e.position_s, e.duration_s);
    let updated = if e.updated > 0 { e.updated } else { now_secs() };
    sqlx::query(&format!(
        "INSERT INTO stream_progress ({COLUMNS})
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(subject_id, season, episode) DO UPDATE SET
             title      = excluded.title,
             cover_url  = excluded.cover_url,
             is_series  = excluded.is_series,
             position_s = excluded.position_s,
             duration_s = excluded.duration_s,
             finished   = excluded.finished,
             updated    = excluded.updated,
             -- A blank means the caller did not know; keep what is already
             -- there rather than erasing a source we can still resume with.
             source     = CASE WHEN excluded.source = '' THEN stream_progress.source
                               ELSE excluded.source END"
    ))
    .bind(&e.subject_id)
    .bind(e.season)
    .bind(e.episode)
    .bind(&e.title)
    .bind(&e.cover_url)
    .bind(e.is_series as i64)
    .bind(e.position_s.max(0.0))
    .bind(e.duration_s.max(0.0))
    .bind(finished as i64)
    .bind(updated)
    .bind(&e.source)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, subject_id: &str, season: i64, episode: i64) -> Option<Entry> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_progress
         WHERE subject_id = ? AND season = ? AND episode = ?"
    ))
    .bind(subject_id)
    .bind(season)
    .bind(episode)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(entry_of)
}

/// Every episode of one title that has been played, for the episode strip's
/// progress bars.
pub async fn for_subject(pool: &SqlitePool, subject_id: &str) -> Vec<Entry> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_progress WHERE subject_id = ? ORDER BY season, episode"
    ))
    .bind(subject_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(entry_of).collect()
}

/// Newest first, everything that has been played.
pub async fn history(pool: &SqlitePool, limit: i64) -> Vec<Entry> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_progress ORDER BY updated DESC LIMIT ?"
    ))
    .bind(limit.max(0))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(entry_of).collect()
}

/// The Continue Watching row: the most recent unfinished title, newest first.
/// One card per show, never one per episode.
///
/// Films count as well as series. They were excluded on the reasoning that a
/// film has no next episode to carry on to — but resuming is about picking a
/// part-watched thing back up, which is exactly what a film left half-finished
/// is. Both catalogues feed the same table, so the row already mixes sources;
/// `source` on the entry is what lets the card open against the right one.
pub async fn continue_watching(pool: &SqlitePool, limit: usize) -> Vec<Entry> {
    if limit == 0 {
        return Vec::new();
    }
    // Deduping in SQL means either a correlated subquery or SQLite's bare-column
    // trick; the candidate set is small, so the loop below is the honest version.
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_progress
         WHERE finished = 0 AND position_s > ?
         ORDER BY updated DESC LIMIT 200"
    ))
    .bind(RESUME_FLOOR_S)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in rows.into_iter().map(entry_of) {
        if seen.insert(show_key(&e)) {
            out.push(e);
            if out.len() == limit {
                break;
            }
        }
    }
    out
}

/// What counts as "the same show" in Continue Watching.
///
/// The subject id alone is not enough: the catalogue splits a long-running
/// series into one subject per season, so resuming season 2 after season 1
/// leaves two ids for one show and the row shows it twice. The season-stripped
/// title is what a viewer means by the show, so that is the key — falling back
/// to the id when a row was written without a title.
/// Now that films are on the row too, the kind is part of the key: a film and a
/// series sharing a name are two different things to carry on with, and folding
/// them together would hide whichever was watched less recently.
fn show_key(e: &Entry) -> String {
    let base = super::split_season_suffix(e.title.trim()).0;
    let name = if base.is_empty() { e.subject_id.clone() } else { base.to_lowercase() };
    format!("{}:{name}", e.is_series as u8)
}

/// Which catalogue a title was last played from, or `None` when nothing was
/// recorded — an unplayed title, or a row written before the column existed.
///
/// Newest row wins: the two catalogues carry unrelated id namespaces, so in
/// practice a subject only ever has one, but a title re-watched from the other
/// source should open against wherever it was actually watched.
pub async fn source_of(pool: &SqlitePool, subject_id: &str) -> Option<String> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT source FROM stream_progress
         WHERE subject_id = ? AND source <> '' ORDER BY updated DESC LIMIT 1",
    )
    .bind(subject_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(|r| r.0)
}

/// Forget one title — every episode of it.
pub async fn remove(pool: &SqlitePool, subject_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM stream_progress WHERE subject_id = ?")
        .bind(subject_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Forget everything (the history page's Clear).
pub async fn clear(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM stream_progress").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn mk(id: &str, season: i64, episode: i64, pos: f64, dur: f64) -> Entry {
        Entry {
            subject_id: id.into(),
            season,
            episode,
            // Distinct per subject: Continue Watching folds on the title, so a
            // shared one would make every fixture the same show. The id goes in
            // front — trailing "s1" would be read as a season marker and folded
            // away again.
            title: format!("{id} Severance"),
            cover_url: "https://c/x.jpg".into(),
            is_series: true,
            position_s: pos,
            duration_s: dur,
            finished: false,
            updated: 0,
            source: "moviebox".into(),
        }
    }

    /// Same, at an explicit time — several `record` calls inside one second
    /// would otherwise tie on `updated` and leave the ordering undefined.
    fn at(t: i64, e: Entry) -> Entry {
        Entry { updated: t, ..e }
    }

    #[test]
    fn finished_needs_a_known_length_and_ninety_percent() {
        assert!(is_finished(900.0, 1000.0));
        assert!(is_finished(1000.0, 1000.0));
        assert!(!is_finished(899.0, 1000.0));
        // An unknown length must never mark something watched.
        assert!(!is_finished(5000.0, 0.0));
        assert!(!is_finished(0.0, 0.0));
    }

    #[test]
    fn resume_skips_the_first_minute_and_finished_episodes() {
        assert_eq!(mk("a", 1, 1, 300.0, 1000.0).resume_at(), Some(300.0));
        // A false start is not worth resuming.
        assert_eq!(mk("a", 1, 1, 30.0, 1000.0).resume_at(), None);
        let done = Entry { finished: true, ..mk("a", 1, 1, 950.0, 1000.0) };
        assert_eq!(done.resume_at(), None);
    }

    #[test]
    fn fraction_and_remaining_stay_sane_without_a_duration() {
        let e = mk("a", 1, 1, 250.0, 1000.0);
        assert_eq!(e.fraction(), 0.25);
        assert_eq!(e.remaining_s(), Some(750.0));
        let unknown = mk("a", 1, 1, 250.0, 0.0);
        assert_eq!(unknown.fraction(), 0.0);
        assert_eq!(unknown.remaining_s(), None);
        // Overshoot (mpv reports past the end) must not exceed a full bar.
        assert_eq!(mk("a", 1, 1, 1200.0, 1000.0).fraction(), 1.0);
    }

    #[tokio::test]
    async fn record_upserts_and_derives_finished() {
        let (_t, pool) = open_pool().await;
        record(&pool, &mk("s1", 1, 3, 120.0, 1000.0)).await.unwrap();
        let got = get(&pool, "s1", 1, 3).await.unwrap();
        assert_eq!(got.position_s, 120.0);
        assert!(!got.finished);
        assert_eq!(got.title, "s1 Severance");

        // Same episode again, now watched to the end.
        record(&pool, &mk("s1", 1, 3, 990.0, 1000.0)).await.unwrap();
        let got = get(&pool, "s1", 1, 3).await.unwrap();
        assert_eq!(got.position_s, 990.0);
        assert!(got.finished, "past 90% must be marked watched");
        assert_eq!(for_subject(&pool, "s1").await.len(), 1, "upsert, not a second row");
    }

    #[tokio::test]
    async fn blank_subject_is_not_recorded() {
        let (_t, pool) = open_pool().await;
        record(&pool, &mk("", 0, 0, 300.0, 1000.0)).await.unwrap();
        assert!(history(&pool, 10).await.is_empty());
    }

    #[tokio::test]
    async fn continue_watching_is_one_card_per_title_and_hides_finished() {
        let (_t, pool) = open_pool().await;
        record(&pool, &at(100, mk("s1", 1, 1, 950.0, 1000.0))).await.unwrap(); // finished
        record(&pool, &at(200, mk("s1", 1, 2, 300.0, 1000.0))).await.unwrap(); // in progress
        record(&pool, &at(300, mk("s2", 0, 0, 400.0, 2000.0))).await.unwrap();
        record(&pool, &at(400, mk("s3", 1, 1, 10.0, 1000.0))).await.unwrap(); // false start
        // A part-watched film belongs here too — it is a thing left unfinished,
        // which is all this row is about.
        let film = Entry { is_series: false, ..mk("m1", 0, 0, 600.0, 3000.0) };
        record(&pool, &at(500, film)).await.unwrap();

        let cw = continue_watching(&pool, 10).await;
        assert_eq!(cw.len(), 3, "{cw:#?}");
        assert_eq!(cw[0].subject_id, "m1", "newest first");
        let s1 = cw.iter().find(|e| e.subject_id == "s1").unwrap();
        assert_eq!(s1.episode, 2, "the unfinished episode, not the watched one");
        assert!(!cw.iter().any(|e| e.subject_id == "s3"), "a false start is not resumable");

        assert_eq!(continue_watching(&pool, 1).await.len(), 1, "limit honoured");
    }

    /// A film and a series can share a name; folding them onto one card would
    /// hide whichever was watched less recently.
    #[tokio::test]
    async fn a_film_and_a_series_of_the_same_name_are_two_cards() {
        let (_t, pool) = open_pool().await;
        let show = Entry { title: "Dune".into(), ..mk("d-show", 1, 2, 300.0, 1000.0) };
        let film = Entry { title: "Dune".into(), is_series: false, ..mk("d-film", 0, 0, 600.0, 3000.0) };
        record(&pool, &at(100, show)).await.unwrap();
        record(&pool, &at(200, film)).await.unwrap();

        let cw = continue_watching(&pool, 10).await;
        assert_eq!(cw.len(), 2, "{cw:#?}");
    }

    /// The card has to reopen against the catalogue it was played from, so the
    /// source rides on the row — and a later write that does not know it must
    /// not wipe what is already there.
    #[tokio::test]
    async fn the_source_is_remembered_and_never_blanked() {
        let (_t, pool) = open_pool().await;
        let watched = Entry { source: "fourk".into(), ..mk("f1", 1, 1, 300.0, 1000.0) };
        record(&pool, &at(100, watched)).await.unwrap();
        assert_eq!(source_of(&pool, "f1").await.as_deref(), Some("fourk"));
        assert_eq!(continue_watching(&pool, 10).await[0].source, "fourk");

        // Same episode, further along, written by a caller with no source.
        let later = Entry { source: String::new(), ..mk("f1", 1, 1, 500.0, 1000.0) };
        record(&pool, &at(200, later)).await.unwrap();
        assert_eq!(source_of(&pool, "f1").await.as_deref(), Some("fourk"), "kept");

        // Nothing recorded at all: the caller falls back to the selection.
        assert_eq!(source_of(&pool, "nobody").await, None);
    }

    #[tokio::test]
    async fn a_split_series_is_one_continue_watching_card() {
        let (_t, pool) = open_pool().await;
        // The catalogue hands out a subject per season; both are the same show.
        let s1 = Entry { title: "Person of Interest".into(), ..mk("p-s1", 1, 4, 300.0, 1000.0) };
        let s2 = Entry { title: "Person of Interest Season 2".into(), ..mk("p-s2", 2, 1, 300.0, 1000.0) };
        record(&pool, &at(100, s1)).await.unwrap();
        record(&pool, &at(200, s2)).await.unwrap();

        let cw = continue_watching(&pool, 10).await;
        assert_eq!(cw.len(), 1, "one card per show, not per season subject: {cw:#?}");
        assert_eq!(cw[0].subject_id, "p-s2", "the season most recently watched");
    }

    #[tokio::test]
    async fn history_is_newest_first_and_clearable() {
        let (_t, pool) = open_pool().await;
        record(&pool, &at(100, mk("s1", 1, 1, 300.0, 1000.0))).await.unwrap();
        record(&pool, &at(200, mk("s2", 1, 1, 300.0, 1000.0))).await.unwrap();
        let h = history(&pool, 10).await;
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].subject_id, "s2", "newest first");

        remove(&pool, "s1").await.unwrap();
        assert_eq!(history(&pool, 10).await.len(), 1);
        clear(&pool).await.unwrap();
        assert!(history(&pool, 10).await.is_empty());
    }
}
