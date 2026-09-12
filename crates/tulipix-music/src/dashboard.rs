//! `np.p4.music.dashboard` — music landing dashboard rails.
//!
//! Recently Played / Most Played / Loved / Resume / New This Week — each a
//! small query over `track_meta` + `audiobook_progress`. The hero card's
//! "For You" seed is just the most-played track id.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

async fn ids(pool: &SqlitePool, sql: &'static str, limit: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(sql).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn recently_played(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE last_played IS NOT NULL AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY last_played DESC LIMIT ?", limit).await
}

pub async fn most_played(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE play_count > 0 AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY play_count DESC LIMIT ?", limit).await
}

/// New This Week — items added in the last 7 days.
pub async fn new_this_week(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    let cutoff = now() - 7 * 86_400;
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE items.added >= ? AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY items.added DESC LIMIT ?",
    ).bind(cutoff).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Resume — audiobooks/podcasts with a saved, unfinished position.
pub async fn resume(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT ap.item_id FROM audiobook_progress ap JOIN items ON items.id = ap.item_id
         WHERE ap.finished = 0 AND ap.position_s > 0 AND items.missing_since IS NULL
         ORDER BY ap.updated DESC LIMIT ?", limit).await
}

/// Resume, with how far in each one is (0..100). Anything at 0% has not really
/// been started and anything marked finished is done, so neither belongs on a
/// "continue" rail — the SQL filters both. Tracks with no known duration are
/// reported at 0% rather than dropped: the row is still worth resuming, we
/// just cannot draw a bar for it.
///
/// MUSIC ONLY. The table it reads is `audiobook_progress` — every long-form
/// player writes its position there, books included — so without the
/// `is_audiobook = 0` filter this returned book chapters, and My Music's
/// Continue listening rail filled up with the audiobook the user was halfway
/// through. Books have their own shelf and their own resume; the two libraries
/// do not mix. The join has to be inner for the same reason: a row with no
/// `track_meta` at all cannot be shown to be music.
pub async fn resume_pct(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, i32)>> {
    let rows: Vec<(i64, f64, Option<f64>)> = sqlx::query_as(
        "SELECT ap.item_id, ap.position_s, tm.duration_s
         FROM audiobook_progress ap
         JOIN items ON items.id = ap.item_id
         JOIN track_meta tm ON tm.item_id = ap.item_id
         WHERE ap.finished = 0 AND ap.position_s > 0 AND items.missing_since IS NULL
           AND tm.is_audiobook = 0
         ORDER BY ap.updated DESC LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, pos, dur)| {
        let pct = match dur {
            Some(d) if d > 0.0 => ((pos / d) * 100.0).clamp(0.0, 100.0) as i32,
            _ => 0,
        };
        (id, pct)
    }).collect())
}

/// What the Home listening strip shows. Milliseconds, not minutes, because
/// rounding belongs at the formatting step where the unit is chosen.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub week_ms: i64,
    pub total_ms: i64,
    pub top_genre: Option<String>,
    /// Consecutive days ending today (or yesterday) that had at least one play.
    pub streak_days: i64,
}

/// One pass over `play_history` for the whole strip.
///
/// The streak counts back from today OR yesterday: at 00:30 you have usually
/// not played anything yet, and zeroing a real streak because the clock rolled
/// over would be wrong. It breaks at the first day with no play.
/// Every listening figure here is MUSIC listening: these sit on My Music, and
/// books keep their own reading stats. `play_history` is shared by every
/// long-form player, so each query joins `track_meta` to filter chapters out —
/// and the join being inner also drops rows with no music metadata at all.
const MUSIC: &str = "JOIN track_meta tm ON tm.item_id = ph.item_id \
     WHERE COALESCE(tm.is_audiobook, 0) = 0";

/// The same filter in two halves, for queries that need another JOIN of their
/// own: SQL wants every join before the WHERE, so those cannot append to a
/// clause that already has one.
const MUSIC_JOIN: &str = "JOIN track_meta tm ON tm.item_id = ph.item_id";
const MUSIC_WHERE: &str = "COALESCE(tm.is_audiobook, 0) = 0";

pub async fn stats(pool: &SqlitePool) -> Result<Stats> {
    let n = now();
    let (total_ms,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(SUM(ph.ms_played), 0) FROM play_history ph {MUSIC}")))
        .fetch_one(pool).await.unwrap_or((0,));
    let (week_ms,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(SUM(ph.ms_played), 0) FROM play_history ph {MUSIC} AND ph.played_at >= ?")))
        .bind(n - 7 * 86_400).fetch_one(pool).await.unwrap_or((0,));
    let top_genre: Option<(String,)> = sqlx::query_as(
        "SELECT tm.genre FROM play_history ph
         JOIN track_meta tm ON tm.item_id = ph.item_id
         WHERE COALESCE(tm.is_audiobook, 0) = 0 AND tm.genre IS NOT NULL AND tm.genre != ''
         GROUP BY tm.genre ORDER BY COUNT(*) DESC LIMIT 1")
        .fetch_optional(pool).await.unwrap_or(None);
    // Distinct local days with a play, newest first.
    let days: Vec<(i64,)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT ph.played_at / 86400 FROM play_history ph {MUSIC} ORDER BY 1 DESC LIMIT 400")))
        .fetch_all(pool).await.unwrap_or_default();
    let today = n / 86_400;
    let mut streak = 0i64;
    let mut want = match days.first() {
        Some((d,)) if *d == today || *d == today - 1 => *d,
        _ => -1,
    };
    if want >= 0 {
        for (d,) in &days {
            if *d == want { streak += 1; want -= 1; } else if *d < want { break; }
        }
    }
    Ok(Stats { week_ms, total_ms, top_genre: top_genre.map(|(g,)| g), streak_days: streak })
}


/// One row of a listening breakdown.
#[derive(Debug, Clone, PartialEq)]
pub struct Tally {
    /// What it is: an artist's name, a genre, a track title.
    pub label: String,
    /// The row's own id, for a UI that wants to open it. 0 when the row is not
    /// a thing you can navigate to, as with an hour of the day.
    pub key: i64,
    pub ms: i64,
    pub plays: i64,
}

/// `AND ph.played_at >= ?` when a window was asked for, nothing when it was not.
///
/// The bind is applied by the caller in the same branch, so the two cannot
/// drift apart into a query with a placeholder and no value.
fn window(since: Option<i64>) -> &'static str {
    if since.is_some() { " AND ph.played_at >= ?" } else { "" }
}

async fn tally(pool: &SqlitePool, sql: String, since: Option<i64>, limit: i64) -> Result<Vec<Tally>> {
    let mut q = sqlx::query_as::<_, (String, i64, i64, i64)>(sqlx::AssertSqlSafe(&*sql));
    if let Some(s) = since {
        q = q.bind(s);
    }
    let rows = q.bind(limit).fetch_all(pool).await.unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|(label, key, ms, plays)| Tally { label, key, ms, plays })
        .collect())
}

/// Listening time by artist, most first. `since` is a unix timestamp, or `None`
/// for all time.
pub async fn by_artist(pool: &SqlitePool, since: Option<i64>, limit: i64) -> Result<Vec<Tally>> {
    tally(pool, format!(
        "SELECT ar.name, ar.id, COALESCE(SUM(ph.ms_played), 0), COUNT(*) \
         FROM play_history ph {MUSIC_JOIN} \
         JOIN artists ar ON ar.id = tm.artist_id \
         WHERE {MUSIC_WHERE}{} \
         GROUP BY ar.id ORDER BY 3 DESC LIMIT ?", window(since)), since, limit).await
}

/// Listening time by genre, most first.
///
/// Grouped case-insensitively — "Trip-Hop" and "trip-hop" are one genre — and
/// the label shown is whichever spelling the library uses most.
pub async fn by_genre(pool: &SqlitePool, since: Option<i64>, limit: i64) -> Result<Vec<Tally>> {
    tally(pool, format!(
        "SELECT tm.genre, 0, COALESCE(SUM(ph.ms_played), 0), COUNT(*) \
         FROM play_history ph {MUSIC} \
           AND tm.genre IS NOT NULL AND TRIM(tm.genre) != ''{} \
         GROUP BY LOWER(TRIM(tm.genre)) ORDER BY 3 DESC LIMIT ?", window(since)), since, limit).await
}

/// Listening time by track, most first — the songs actually on repeat.
pub async fn by_track(pool: &SqlitePool, since: Option<i64>, limit: i64) -> Result<Vec<Tally>> {
    tally(pool, format!(
        "SELECT COALESCE(NULLIF(TRIM(tm.title), ''), i.abs_path), tm.item_id, \
                COALESCE(SUM(ph.ms_played), 0), COUNT(*) \
         FROM play_history ph {MUSIC_JOIN} \
         JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
         WHERE {MUSIC_WHERE}{} \
         GROUP BY tm.item_id ORDER BY 3 DESC LIMIT ?", window(since)), since, limit).await
}

/// Listening time per hour of the day, midnight first.
///
/// Local hours, not UTC: "when do you listen" is a question about the user's
/// day, and a summary that puts their evening at 3am is wrong in the only way
/// that matters. SQLite reads the offset from the process timezone.
pub async fn by_hour(pool: &SqlitePool, since: Option<i64>) -> Result<[i64; 24]> {
    let sql = format!(
        "SELECT CAST(strftime('%H', ph.played_at, 'unixepoch', 'localtime') AS INTEGER), \
                COALESCE(SUM(ph.ms_played), 0) \
         FROM play_history ph {MUSIC}{} GROUP BY 1", window(since));
    let mut q = sqlx::query_as::<_, (i64, i64)>(sqlx::AssertSqlSafe(&*sql));
    if let Some(s) = since {
        q = q.bind(s);
    }
    let mut out = [0i64; 24];
    for (h, ms) in q.fetch_all(pool).await.unwrap_or_default() {
        if (0..24).contains(&h) {
            out[h as usize] += ms;
        }
    }
    Ok(out)
}

/// A play that got less than this far in was abandoned, not listened to.
const ABANDON_FRACTION: f64 = 0.6;

/// The records started over and over and never finished — ranked by how many
/// times, which is the figure that makes the point.
///
/// Needs a known duration to judge against, so a track with none never appears
/// here. `plays` counts the abandonments, not the plays.
pub async fn abandoned(pool: &SqlitePool, limit: i64) -> Result<Vec<Tally>> {
    let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(NULLIF(TRIM(tm.title), ''), i.abs_path), tm.item_id, \
                COALESCE(SUM(ph.ms_played), 0), COUNT(*) \
         FROM play_history ph {MUSIC_JOIN} \
         JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
         WHERE {MUSIC_WHERE} \
           AND tm.duration_s > 0 \
           AND ph.ms_played > 0 \
           AND ph.ms_played < tm.duration_s * 1000 * ? \
         GROUP BY tm.item_id HAVING COUNT(*) > 1 ORDER BY 4 DESC LIMIT ?")))
        .bind(ABANDON_FRACTION)
        .bind(limit)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|(label, key, ms, plays)| Tally { label, key, ms, plays })
        .collect())
}

/// The headline figures for one window, beside the breakdowns above.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Figures {
    pub plays: i64,
    pub ms: i64,
    /// Listening in the same length of time just before the window. 0 for all
    /// time, which has nothing before it.
    pub prev_ms: i64,
    /// Plays that got past [`ABANDON_FRACTION`] of their track, out of the
    /// plays whose track has a length to judge by.
    pub finished: i64,
    pub judged: i64,
    /// Artists whose first play ever falls inside the window.
    pub new_artists: i64,
}

/// [`Figures`] for the last `days` days, or all time when `days` is 0.
pub async fn figures(pool: &SqlitePool, days: i64) -> Result<Figures> {
    let since = (days > 0).then(|| now() - days * 86_400);

    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(ph.ms_played), 0) FROM play_history ph {MUSIC}{}",
        window(since));
    let mut q = sqlx::query_as::<_, (i64, i64)>(sqlx::AssertSqlSafe(&*sql));
    if let Some(s) = since {
        q = q.bind(s);
    }
    let (plays, ms) = q.fetch_one(pool).await.unwrap_or((0, 0));

    let prev_ms = match since {
        Some(s) => sqlx::query_as::<_, (i64,)>(sqlx::AssertSqlSafe(format!(
            "SELECT COALESCE(SUM(ph.ms_played), 0) FROM play_history ph {MUSIC} \
             AND ph.played_at >= ? AND ph.played_at < ?")))
            .bind(s - days * 86_400)
            .bind(s)
            .fetch_one(pool)
            .await
            .map(|(v,)| v)
            .unwrap_or(0),
        None => 0,
    };

    // Placeholders bind in the order they appear: the fraction, then the window.
    let sql = format!(
        "SELECT COALESCE(SUM(ph.ms_played >= tm.duration_s * 1000 * ?), 0), COUNT(*) \
         FROM play_history ph {MUSIC} AND tm.duration_s > 0{}",
        window(since));
    let mut q = sqlx::query_as::<_, (i64, i64)>(sqlx::AssertSqlSafe(&*sql)).bind(ABANDON_FRACTION);
    if let Some(s) = since {
        q = q.bind(s);
    }
    let (finished, judged) = q.fetch_one(pool).await.unwrap_or((0, 0));

    let (new_artists,): (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM ( \
           SELECT MIN(ph.played_at) AS first FROM play_history ph {MUSIC} \
             AND tm.artist_id IS NOT NULL GROUP BY tm.artist_id) \
         WHERE first >= ?")))
        .bind(since.unwrap_or(0))
        .fetch_one(pool)
        .await
        .unwrap_or((0,));

    Ok(Figures { plays, ms, prev_ms, finished, judged, new_artists })
}

/// Listening per local day for the last `days` days, oldest first and today
/// last. Local days for the reason [`by_hour`] uses local hours: a calendar that
/// files your Sunday evening under Monday is wrong where it shows.
pub async fn by_day(pool: &SqlitePool, days: i64) -> Result<Vec<i64>> {
    let days = days.max(1);
    let rows: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT CAST(julianday(date('now', 'localtime')) \
                   - julianday(date(ph.played_at, 'unixepoch', 'localtime')) AS INTEGER), \
                COALESCE(SUM(ph.ms_played), 0) \
         FROM play_history ph {MUSIC} AND ph.played_at >= ? GROUP BY 1")))
        .bind(now() - (days + 1) * 86_400)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let mut out = vec![0i64; days as usize];
    for (ago, ms) in rows {
        if (0..days).contains(&ago) {
            out[(days - 1 - ago) as usize] += ms;
        }
    }
    Ok(out)
}

/// What the shelves hold, for the Stats Center's library panel.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Shelf {
    pub tracks: i64,
    pub albums: i64,
    pub artists: i64,
    pub bytes: i64,
    pub secs: f64,
    /// Codec and how many tracks use it, most first. Every PCM flavour is one
    /// "WAV": pcm_s16le and pcm_s24le are the same answer to "what format".
    pub formats: Vec<(String, i64)>,
}

pub async fn shelf(pool: &SqlitePool) -> Result<Shelf> {
    const LIVE: &str = "FROM track_meta tm JOIN items i ON i.id = tm.item_id \
         WHERE i.missing_since IS NULL AND COALESCE(tm.is_audiobook, 0) = 0";
    let (tracks, albums, artists, bytes, secs): (i64, i64, i64, i64, f64) =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*), COUNT(DISTINCT tm.album_id), COUNT(DISTINCT tm.artist_id), \
                    COALESCE(SUM(i.size), 0), COALESCE(SUM(tm.duration_s), 0.0) {LIVE}")))
            .fetch_one(pool)
            .await
            .unwrap_or_default();
    let formats: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT CASE WHEN LOWER(tm.codec) LIKE 'pcm%' THEN 'WAV' \
                     ELSE UPPER(COALESCE(NULLIF(TRIM(tm.codec), ''), '?')) END, COUNT(*) \
         {LIVE} GROUP BY 1 ORDER BY 2 DESC")))
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    Ok(Shelf { tracks, albums, artists, bytes, secs, formats })
}


/// An album that was started and never finished.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumResume {
    pub album_id: i64,
    /// The track to start from: the one after the last you played.
    pub item_id: i64,
    /// How many tracks were already played, and how many there are. Position
    /// in the album order, not the tagged track number — an album missing its
    /// first two files still resumes in the right place.
    pub done: i64,
    pub total: i64,
    /// When the last of them was played.
    pub played_at: i64,
}

/// Albums left part-way through, most recently abandoned first.
///
/// The Books reader has offered this since it shipped; music records exactly
/// the same thing in `play_history` and never offered a way back in. An album
/// is resumable when the last track played from it was not its last track.
///
/// Single tracks filed under an album of one are skipped: a single you played
/// once is not an album you abandoned.
pub async fn resume_albums(pool: &SqlitePool, limit: i64) -> Result<Vec<AlbumResume>> {
    // The bare `ph.item_id` beside MAX() is SQLite's documented min/max bare
    // column rule: it comes from the same row the maximum did, which is exactly
    // the track that was played last. Any other engine would reject this.
    let recent: Vec<(i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT tm.album_id, ph.item_id, MAX(ph.played_at) \
         FROM play_history ph {MUSIC_JOIN} \
         JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
         WHERE {MUSIC_WHERE} AND tm.album_id IS NOT NULL \
         GROUP BY tm.album_id ORDER BY 3 DESC LIMIT ?")))
        .bind(limit * 4) // room to drop the finished ones and still fill the rail
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    let mut out = Vec::new();
    for (album_id, last_played, played_at) in recent {
        // Album order, not tag order: COALESCE so untagged files sort first
        // rather than vanishing, and the id breaks ties so the sequence is
        // stable between calls.
        let order: Vec<(i64,)> = sqlx::query_as(
            "SELECT tm.item_id FROM track_meta tm \
             JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
             WHERE tm.album_id = ? AND COALESCE(tm.is_audiobook, 0) = 0 \
             ORDER BY COALESCE(tm.disc_no, 0), COALESCE(tm.track_no, 0), tm.item_id",
        )
        .bind(album_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        let ids: Vec<i64> = order.into_iter().map(|(id,)| id).collect();
        if ids.len() < 2 {
            continue;
        }
        let Some(at) = ids.iter().position(|id| *id == last_played) else { continue };
        let Some(next) = ids.get(at + 1).copied() else { continue }; // finished
        out.push(AlbumResume {
            album_id,
            item_id: next,
            done: at as i64 + 1,
            total: ids.len() as i64,
            played_at,
        });
        if out.len() >= limit.max(0) as usize {
            break;
        }
    }
    Ok(out)
}

/// "2 days ago" / "just now". Coarse on purpose: the exact minute an album was
/// abandoned is not what anyone is asking.
pub fn fmt_ago(then: i64) -> String {
    let gap = now() - then;
    match gap {
        g if g < 90 => "just now".into(),
        g if g < 3_600 => format!("{} min ago", g / 60),
        g if g < 7_200 => "an hour ago".into(),
        g if g < 86_400 => format!("{} hours ago", g / 3_600),
        g if g < 172_800 => "yesterday".into(),
        g if g < 2_592_000 => format!("{} days ago", g / 86_400),
        g if g < 5_184_000 => "last month".into(),
        g => format!("{} months ago", g / 2_592_000),
    }
}

/// A unix timestamp as "2024-11-02".
///
/// Civil-from-days, Howard Hinnant's algorithm. No date dependency in this
/// crate for one date on one panel, and the arithmetic is shorter than the
/// line in Cargo.toml would be. UTC, not local: this labels when a file
/// entered the library, and a row that changes its date when you fly to
/// Tokyo is worse than one that is a few hours out.
pub fn fmt_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// "12h 30m" / "45m" / "—". Hours only once there is an hour to show.
pub fn fmt_listen(ms: i64) -> String {
    if ms <= 0 { return "—".into(); }
    let mins = ms / 60_000;
    if mins < 1 { return "<1m".into(); }
    let (h, m) = (mins / 60, mins % 60);
    if h == 0 { format!("{m}m") } else if m == 0 { format!("{h}h") } else { format!("{h}h {m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn stats_sum_and_streak() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        sqlx::query("UPDATE track_meta SET genre = 'Ghazal' WHERE item_id = ?")
            .bind(a).execute(&pool).await.unwrap();
        let today = now() / 86_400;
        // Three consecutive days ending today, then a gap, then one more.
        for (day, ms) in [(today, 600_000i64), (today - 1, 300_000), (today - 2, 60_000), (today - 9, 999)] {
            sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?, ?, ?)")
                .bind(a).bind(day * 86_400 + 100).bind(ms).execute(&pool).await.unwrap();
        }
        let s = stats(&pool).await.unwrap();
        assert_eq!(s.total_ms, 960_999);
        // The 9-day-old play is outside the 7-day window.
        assert_eq!(s.week_ms, 960_000);
        assert_eq!(s.top_genre.as_deref(), Some("Ghazal"));
        // The gap ends it at three, not four.
        assert_eq!(s.streak_days, 3);
    }

    #[tokio::test]
    async fn breakdowns_rank_and_respect_their_window() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO artists (id, name) VALUES (1, 'Burial'), (2, 'Tycho')")
            .execute(&pool).await.unwrap();
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        sqlx::query("UPDATE track_meta SET artist_id=1, genre='Dubstep', title='Archangel', duration_s=240 WHERE item_id=?")
            .bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET artist_id=2, genre='dubstep', title='Awake', duration_s=300 WHERE item_id=?")
            .bind(b).execute(&pool).await.unwrap();
        let n = now();
        // Every play here is under 60% of its track, which is what makes them
        // abandonments: 240s * 0.6 = 144s, 300s * 0.6 = 180s.
        for (id, at, ms) in [
            (a, n - 100, 100_000i64),
            (a, n - 200, 90_000),
            (b, n - 30 * 86_400, 90_000), // outside a one-week window
        ] {
            sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?,?,?)")
                .bind(id).bind(at).bind(ms).execute(&pool).await.unwrap();
        }

        let artists = by_artist(&pool, None, 10).await.unwrap();
        assert_eq!(artists[0].label, "Burial");
        assert_eq!(artists[0].ms, 190_000);
        assert_eq!(artists[0].plays, 2);
        assert_eq!(artists[0].key, 1, "the row carries the id it can be opened by");

        // Two spellings of one genre collapse into a single row.
        let genres = by_genre(&pool, None, 10).await.unwrap();
        assert_eq!(genres.len(), 1, "Dubstep and dubstep are one genre");
        assert_eq!(genres[0].ms, 280_000);

        let week = by_artist(&pool, Some(n - 7 * 86_400), 10).await.unwrap();
        assert_eq!(week.len(), 1, "the month-old play is outside the window");

        // Both of a's plays fell short of 60% of its 240s, and it has two of
        // them. b was abandoned once, and once is not a habit.
        let quit = abandoned(&pool, 10).await.unwrap();
        assert_eq!(quit.len(), 1);
        assert_eq!(quit[0].label, "Archangel");
        assert_eq!(quit[0].plays, 2);

        let hours = by_hour(&pool, None).await.unwrap();
        assert_eq!(hours.iter().sum::<i64>(), 280_000, "every play lands in some hour");
    }

    #[tokio::test]
    async fn a_window_counts_its_plays_its_finishes_and_its_new_artists() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO artists (id, name) VALUES (1, 'Burial'), (2, 'Tycho')")
            .execute(&pool).await.unwrap();
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        sqlx::query("UPDATE track_meta SET artist_id = 1, duration_s = 100 WHERE item_id = ?")
            .bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET artist_id = 2, duration_s = 100 WHERE item_id = ?")
            .bind(b).execute(&pool).await.unwrap();
        let n = now();
        for (id, at, ms) in [
            (a, n, 90_000i64),            // past 60% of 100s: finished
            (a, n, 10_000),               // abandoned
            (b, n - 10 * 86_400, 50_000), // the week before, so Tycho is not new
            (b, n, 50_000),
        ] {
            sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?,?,?)")
                .bind(id).bind(at).bind(ms).execute(&pool).await.unwrap();
        }

        let f = figures(&pool, 7).await.unwrap();
        assert_eq!((f.plays, f.ms), (3, 150_000));
        assert_eq!(f.prev_ms, 50_000, "the play ten days ago is in the week before");
        assert_eq!((f.finished, f.judged), (1, 3));
        assert_eq!(f.new_artists, 1, "Tycho was first played before the window");
        assert_eq!(figures(&pool, 0).await.unwrap().prev_ms, 0, "all time has no before");

        let days = by_day(&pool, 14).await.unwrap();
        assert_eq!(days.len(), 14);
        assert_eq!(days[13], 150_000, "today is the last day");
        assert_eq!(days.iter().sum::<i64>(), 200_000);

        let s = shelf(&pool).await.unwrap();
        assert_eq!((s.tracks, s.artists), (2, 2));
        assert!((s.secs - 200.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn an_album_resumes_after_the_last_track_played() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO albums (id, title) VALUES (1, 'Kid A'), (2, 'Single')")
            .execute(&pool).await.unwrap();
        let mut ids = Vec::new();
        for n in 1..=4 {
            let id = add_track(&pool, &format!("/m/kid{n}.flac")).await;
            sqlx::query("UPDATE track_meta SET album_id = 1, track_no = ? WHERE item_id = ?")
                .bind(n).bind(id).execute(&pool).await.unwrap();
            ids.push(id);
        }
        // A one-track album is not an album you abandoned.
        let solo = add_track(&pool, "/m/solo.flac").await;
        sqlx::query("UPDATE track_meta SET album_id = 2, track_no = 1 WHERE item_id = ?")
            .bind(solo).execute(&pool).await.unwrap();

        let n = now();
        for (id, at) in [(ids[0], n - 400), (ids[1], n - 300), (solo, n - 100)] {
            sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?,?,1000)")
                .bind(id).bind(at).execute(&pool).await.unwrap();
        }

        let r = resume_albums(&pool, 10).await.unwrap();
        assert_eq!(r.len(), 1, "only the four-track album is resumable");
        assert_eq!(r[0].album_id, 1);
        assert_eq!(r[0].item_id, ids[2], "resumes at track three");
        assert_eq!((r[0].done, r[0].total), (2, 4));

        // Play the last track and the album is finished, not resumable.
        sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?,?,1000)")
            .bind(ids[3]).bind(n - 50).execute(&pool).await.unwrap();
        assert!(resume_albums(&pool, 10).await.unwrap().is_empty());
    }

    #[test]
    fn dates_come_out_as_dates() {
        assert_eq!(fmt_date(0), "1970-01-01");
        assert_eq!(fmt_date(1_730_505_600), "2024-11-02");
        // Leap day, and the day after it, on a leap year that is also a
        // century divisible by 400 -- the case the /100 and /400 terms exist
        // for and the one a naive implementation gets wrong.
        assert_eq!(fmt_date(951_782_400), "2000-02-29");
        assert_eq!(fmt_date(951_868_800), "2000-03-01");
        // Before the epoch: div_euclid, not /, or this rounds towards zero and
        // lands a day late.
        assert_eq!(fmt_date(-1), "1969-12-31");
    }

    #[test]
    fn ago_reads_as_a_person_would_say_it() {
        let n = now();
        assert_eq!(fmt_ago(n), "just now");
        assert_eq!(fmt_ago(n - 600), "10 min ago");
        assert_eq!(fmt_ago(n - 5 * 3_600), "5 hours ago");
        assert_eq!(fmt_ago(n - 100_000), "yesterday");
        assert_eq!(fmt_ago(n - 3 * 86_400), "3 days ago");
    }

    #[tokio::test]
    async fn resume_pct_skips_finished_and_unstarted() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.m4b").await;
        let b = add_track(&pool, "/m/b.m4b").await;
        let c = add_track(&pool, "/m/c.m4b").await;
        sqlx::query("UPDATE track_meta SET duration_s = 1000 WHERE item_id IN (?, ?, ?)")
            .bind(a).bind(b).bind(c).execute(&pool).await.unwrap();
        for (id, pos, fin) in [(a, 250.0f64, 0), (b, 900.0, 1), (c, 0.0, 0)] {
            sqlx::query("INSERT INTO audiobook_progress (item_id, position_s, finished, updated) VALUES (?, ?, ?, 1)")
                .bind(id).bind(pos).bind(fin).execute(&pool).await.unwrap();
        }
        let got = resume_pct(&pool, 10).await.unwrap();
        assert_eq!(got, vec![(a, 25)]);
    }

    // The bug this file's `is_audiobook = 0` filters exist for: My Music's
    // Continue listening rail filling up with the book the user was halfway
    // through, because every long-form player writes to `audiobook_progress`.
    #[tokio::test]
    async fn resume_pct_leaves_books_on_the_book_shelf() {
        let (_t, pool) = open_pool().await;
        let song = add_track(&pool, "/m/song.flac").await;
        let chapter = add_track(&pool, "/m/book/ch1.m4b").await;
        sqlx::query("UPDATE track_meta SET duration_s = 1000 WHERE item_id IN (?, ?)")
            .bind(song).bind(chapter).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET is_audiobook = 1 WHERE item_id = ?")
            .bind(chapter).execute(&pool).await.unwrap();
        for id in [song, chapter] {
            sqlx::query("INSERT INTO audiobook_progress (item_id, position_s, finished, updated) VALUES (?, 250, 0, 1)")
                .bind(id).execute(&pool).await.unwrap();
        }
        assert_eq!(resume_pct(&pool, 10).await.unwrap(), vec![(song, 25)]);
    }

    #[test]
    fn listen_formatting() {
        assert_eq!(fmt_listen(0), "—");
        assert_eq!(fmt_listen(30_000), "<1m");
        assert_eq!(fmt_listen(45 * 60_000), "45m");
        assert_eq!(fmt_listen(120 * 60_000), "2h");
        assert_eq!(fmt_listen(150 * 60_000), "2h 30m");
    }

    #[tokio::test]
    async fn rails_reflect_state() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        sqlx::query("UPDATE track_meta SET play_count = 10, last_played = 500 WHERE item_id = ?").bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET play_count = 3, last_played = 200 WHERE item_id = ?").bind(b).execute(&pool).await.unwrap();
        assert_eq!(most_played(&pool, 10).await.unwrap(), vec![a, b]);
        assert_eq!(recently_played(&pool, 10).await.unwrap(), vec![a, b]);
        // freshly added => new this week
        sqlx::query("UPDATE items SET added = ? WHERE id = ?").bind(now()).bind(a).execute(&pool).await.unwrap();
        assert!(new_this_week(&pool, 10).await.unwrap().contains(&a));
    }
}
