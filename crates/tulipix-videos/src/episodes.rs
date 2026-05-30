//! Season / episode browser + Next Up rail.
//!
//! Episodes are joined to their parent show; the browser returns shows
//! ordered by latest-watched activity. Next Up picks, per show:
//!  * the first unfinished episode the user has progress on, OR
//!  * the next episode after the most recently finished one.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRow {
    pub item_id: i64,
    pub show_id: i64,
    pub season: i64,
    pub episode: i64,
    pub title: Option<String>,
    pub overview: Option<String>,
    pub still_path: Option<String>,
    pub runtime_min: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextUp {
    pub show_id: i64,
    pub show_title: String,
    pub episode: EpisodeRow,
    pub resume_position_s: Option<f64>,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn upsert(
    pool: &SqlitePool,
    item_id: i64,
    show_id: i64,
    season: i64,
    episode: i64,
    title: Option<&str>,
    overview: Option<&str>,
    air_date: Option<i64>,
    still_path: Option<&str>,
    runtime_min: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO episodes (item_id, show_id, season, episode, title, overview, air_date, still_path, runtime_min, updated)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            show_id = excluded.show_id, season = excluded.season, episode = excluded.episode,
            title = excluded.title, overview = excluded.overview, air_date = excluded.air_date,
            still_path = excluded.still_path, runtime_min = excluded.runtime_min,
            updated = excluded.updated",
    )
    .bind(item_id).bind(show_id).bind(season).bind(episode)
    .bind(title).bind(overview).bind(air_date).bind(still_path).bind(runtime_min)
    .bind(now())
    .execute(pool).await?;
    Ok(())
}

pub async fn seasons_for(pool: &SqlitePool, show_id: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT season FROM episodes WHERE show_id = ? ORDER BY season",
    ).bind(show_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}

pub async fn episodes_in_season(pool: &SqlitePool, show_id: i64, season: i64) -> Result<Vec<EpisodeRow>> {
    let rows: Vec<(i64, i64, i64, i64, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT item_id, show_id, season, episode, title, overview, still_path, runtime_min
         FROM episodes WHERE show_id = ? AND season = ? ORDER BY episode",
    ).bind(show_id).bind(season).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(item_id, show_id, season, episode, title, overview, still_path, runtime_min)| {
        EpisodeRow { item_id, show_id, season, episode, title, overview, still_path, runtime_min }
    }).collect())
}

/// Compute Next Up entries across every show. Limit caps the rail.
pub async fn next_up(pool: &SqlitePool, limit: i64) -> Result<Vec<NextUp>> {
    let shows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM shows ORDER BY updated DESC",
    ).fetch_all(pool).await?;
    let mut out = Vec::new();
    for (show_id, show_title) in shows {
        if let Some(ep) = next_up_for_show(pool, show_id).await? {
            let pos: Option<f64> = sqlx::query_scalar(
                "SELECT position_s FROM watch_progress WHERE item_id = ?",
            ).bind(ep.item_id).fetch_optional(pool).await?;
            out.push(NextUp { show_id, show_title, episode: ep, resume_position_s: pos });
        }
        if (out.len() as i64) >= limit { break; }
    }
    Ok(out)
}

async fn next_up_for_show(pool: &SqlitePool, show_id: i64) -> Result<Option<EpisodeRow>> {
    // 1) Episode the user is actively in the middle of.
    let active: Option<(i64, i64, i64, i64, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT e.item_id, e.show_id, e.season, e.episode, e.title, e.overview, e.still_path, e.runtime_min
         FROM episodes e
         JOIN watch_progress w ON w.item_id = e.item_id
         WHERE e.show_id = ? AND w.finished = 0 AND w.position_s > 0
         ORDER BY w.updated DESC LIMIT 1",
    ).bind(show_id).fetch_optional(pool).await?;
    if let Some(r) = active { return Ok(Some(row_to(r))); }

    // 2) Episode right after the last finished one.
    let after_finished: Option<(i64, i64, i64, i64, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "WITH last_done AS (
            SELECT e.season, e.episode
            FROM episodes e
            JOIN watch_progress w ON w.item_id = e.item_id
            WHERE e.show_id = ? AND w.finished = 1
            ORDER BY e.season DESC, e.episode DESC LIMIT 1
         )
         SELECT e.item_id, e.show_id, e.season, e.episode, e.title, e.overview, e.still_path, e.runtime_min
         FROM episodes e, last_done d
         WHERE e.show_id = ?
           AND ((e.season = d.season AND e.episode > d.episode) OR (e.season > d.season))
         ORDER BY e.season, e.episode LIMIT 1",
    ).bind(show_id).bind(show_id).fetch_optional(pool).await?;
    if let Some(r) = after_finished { return Ok(Some(row_to(r))); }

    // 3) First episode if nothing watched yet.
    let first: Option<(i64, i64, i64, i64, Option<String>, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT item_id, show_id, season, episode, title, overview, still_path, runtime_min
         FROM episodes WHERE show_id = ? ORDER BY season, episode LIMIT 1",
    ).bind(show_id).fetch_optional(pool).await?;
    Ok(first.map(row_to))
}

fn row_to(r: (i64, i64, i64, i64, Option<String>, Option<String>, Option<String>, Option<i64>)) -> EpisodeRow {
    let (item_id, show_id, season, episode, title, overview, still_path, runtime_min) = r;
    EpisodeRow { item_id, show_id, season, episode, title, overview, still_path, runtime_min }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn make_show(pool: &SqlitePool, title: &str) -> i64 {
        sqlx::query("INSERT INTO shows (title, updated) VALUES (?, ?)")
            .bind(title).bind(now()).execute(pool).await.unwrap();
        sqlx::query_scalar("SELECT id FROM shows WHERE title = ?").bind(title).fetch_one(pool).await.unwrap()
    }

    async fn make_ep(pool: &SqlitePool, path: &str, show_id: i64, s: i64, e: i64) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'videos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        upsert(pool, id, show_id, s, e, Some(&format!("S{s}E{e}")), None, None, None, Some(45)).await.unwrap();
        id
    }

    #[tokio::test]
    async fn season_and_episode_listing() {
        let (_t, pool) = open_pool().await;
        let show = make_show(&pool, "Test").await;
        make_ep(&pool, "/a.mkv", show, 1, 1).await;
        make_ep(&pool, "/b.mkv", show, 1, 2).await;
        make_ep(&pool, "/c.mkv", show, 2, 1).await;
        assert_eq!(seasons_for(&pool, show).await.unwrap(), vec![1, 2]);
        let s1 = episodes_in_season(&pool, show, 1).await.unwrap();
        assert_eq!(s1.len(), 2);
        assert_eq!(s1[0].episode, 1);
        assert_eq!(s1[1].episode, 2);
    }

    #[tokio::test]
    async fn next_up_picks_after_finished() {
        let (_t, pool) = open_pool().await;
        let show = make_show(&pool, "Test").await;
        let e1 = make_ep(&pool, "/a.mkv", show, 1, 1).await;
        let _e2 = make_ep(&pool, "/b.mkv", show, 1, 2).await;
        sqlx::query("INSERT INTO watch_progress (item_id, position_s, finished, updated) VALUES (?, 45.0, 1, ?)")
            .bind(e1).bind(now()).execute(&pool).await.unwrap();
        let n = next_up(&pool, 10).await.unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].episode.episode, 2);
    }

    #[tokio::test]
    async fn next_up_first_episode_when_nothing_watched() {
        let (_t, pool) = open_pool().await;
        let show = make_show(&pool, "Test").await;
        make_ep(&pool, "/a.mkv", show, 1, 1).await;
        make_ep(&pool, "/b.mkv", show, 1, 2).await;
        let n = next_up(&pool, 10).await.unwrap();
        assert_eq!(n[0].episode.episode, 1);
    }
}
