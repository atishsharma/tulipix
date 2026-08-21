//! Source health, and the order it produces.
//!
//! The whole failover policy is here: a source that has failed
//! [`SINK_AFTER`] times in a row sinks below the healthy ones until
//! [`SINK_FOR_SECS`] has passed since its last failure. There is no scheduler —
//! the order is recomputed from two timestamps every time it is asked for.

use anyhow::Result;
use sqlx::{Row, SqlitePool};
use tulipix_core::util::unix_secs_i64 as now;

/// Consecutive failures before a source stops being tried first.
pub const SINK_AFTER: i64 = 3;
/// How long a sunk source stays sunk. Long enough that a rotated site does not
/// cost a timeout on every play; short enough to notice it came back.
pub const SINK_FOR_SECS: i64 = 60 * 60;

/// One row of the Settings health list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Health {
    pub source: String,
    pub label: String,
    pub enabled: bool,
    pub ok: bool,
    /// "1.1 s" / "403 forbidden" / "not tried yet"
    pub note: String,
    pub last_ms: i64,
    pub fail_streak: i64,
    /// True while the source is serving its hour at the bottom.
    pub sunk: bool,
}

pub async fn note_ok(pool: &SqlitePool, source: &str, ms: u32) {
    let _ = sqlx::query(
        "INSERT INTO splus_source_health (source, last_ok, fail_streak, last_ms, note)
         VALUES (?1, ?2, 0, ?3, '')
         ON CONFLICT(source) DO UPDATE SET
            last_ok = ?2, fail_streak = 0, last_ms = ?3, note = ''",
    )
    .bind(source)
    .bind(now())
    .bind(i64::from(ms))
    .execute(pool)
    .await;
}

pub async fn note_fail(pool: &SqlitePool, source: &str, note: &str) {
    let _ = sqlx::query(
        "INSERT INTO splus_source_health (source, last_fail, fail_streak, note)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(source) DO UPDATE SET
            last_fail = ?2, fail_streak = splus_source_health.fail_streak + 1, note = ?3",
    )
    .bind(source)
    .bind(now())
    .bind(note)
    .execute(pool)
    .await;
}

/// Clear every streak, for the Reset button.
pub async fn reset(pool: &SqlitePool) -> Result<()> {
    sqlx::query("UPDATE splus_source_health SET fail_streak = 0, note = ''")
        .execute(pool)
        .await?;
    Ok(())
}

/// The ids to try, in order.
///
/// `anime` selects the lane: an anime-only source is skipped entirely for a
/// TMDB title rather than being tried and failed, which would sink it for an
/// hour for no reason.
pub async fn order(pool: &SqlitePool, anime: bool) -> Vec<String> {
    let rows = super::source::all();
    let mut scored: Vec<(i64, usize, String)> = Vec::new();
    let t = now();
    for (i, src) in rows.iter().enumerate() {
        if src.anime_only() && !anime {
            continue;
        }
        if !enabled(pool, src.id()).await {
            continue;
        }
        let (streak, last_fail) = streak_of(pool, src.id()).await;
        // 0 keeps the configured position, 1 sinks below everything healthy.
        let sunk = i64::from(streak >= SINK_AFTER && t - last_fail < SINK_FOR_SECS);
        scored.push((sunk, position(pool, src.id(), i).await, src.id().to_string()));
    }
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, id)| id).collect()
}

/// Everything the Settings list needs, in the order the resolver would use.
pub async fn list(pool: &SqlitePool) -> Vec<Health> {
    let mut out = Vec::new();
    let t = now();
    for (i, src) in super::source::all().iter().enumerate() {
        let (streak, last_fail) = streak_of(pool, src.id()).await;
        let row = sqlx::query(
            "SELECT last_ok, last_ms, note FROM splus_source_health WHERE source = ?1",
        )
        .bind(src.id())
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let (last_ok, last_ms, note) = match row {
            Some(r) => (
                r.try_get::<i64, _>("last_ok").unwrap_or(0),
                r.try_get::<i64, _>("last_ms").unwrap_or(0),
                r.try_get::<String, _>("note").unwrap_or_default(),
            ),
            None => (0, 0, String::new()),
        };
        let sunk = streak >= SINK_AFTER && t - last_fail < SINK_FOR_SECS;
        let ok = last_ok > 0 && streak == 0;
        out.push(Health {
            source: src.id().to_string(),
            label: src.label().to_string(),
            enabled: enabled(pool, src.id()).await,
            ok,
            note: if !note.is_empty() {
                note
            } else if last_ms > 0 {
                format!("{:.1} s", last_ms as f64 / 1000.0)
            } else {
                "not tried yet".into()
            },
            last_ms,
            fail_streak: streak,
            sunk,
        });
        let _ = i;
    }
    out
}

async fn streak_of(pool: &SqlitePool, source: &str) -> (i64, i64) {
    let row = sqlx::query(
        "SELECT fail_streak, last_fail FROM splus_source_health WHERE source = ?1",
    )
    .bind(source)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(r) => (
            r.try_get::<i64, _>("fail_streak").unwrap_or(0),
            r.try_get::<i64, _>("last_fail").unwrap_or(0),
        ),
        None => (0, 0),
    }
}

async fn enabled(_pool: &SqlitePool, source: &str) -> bool {
    super::prefs::source_enabled(source)
}

/// Where the user dragged this source to, falling back to declaration order.
async fn position(_pool: &SqlitePool, source: &str, default: usize) -> usize {
    match super::prefs::source_order().iter().position(|s| s == source) {
        Some(i) => i,
        None => default + 100,
    }
}
