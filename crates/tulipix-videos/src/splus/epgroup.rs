//! TMDB episode-group correction.
//!
//! Some shows are numbered one way by TMDB and another by everyone else —
//! split-cour seasons counted absolutely, recap episodes included or not. TMDB
//! publishes the alternate orderings as *episode groups*; when one exists we
//! cache the mapping and translate before asking a provider for an episode.
//!
//! Only shows that actually disagree get a row, so the common case costs one
//! lookup that misses.

use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

const TMDB: &str = "https://api.themoviedb.org/3";

/// Translate a season/episode pair into what the providers expect.
///
/// Returns the input unchanged when nothing is cached, which is the answer for
/// most shows.
pub async fn map(pool: &SqlitePool, tmdb_id: i64, season: i64, episode: i64) -> (i64, i64) {
    let row = sqlx::query(
        "SELECT mapped_season, mapped_ep FROM splus_episode_group
         WHERE tmdb_id = ?1 AND season = ?2 AND episode = ?3",
    )
    .bind(tmdb_id)
    .bind(season)
    .bind(episode)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some(r) => (
            r.try_get("mapped_season").unwrap_or(season),
            r.try_get("mapped_ep").unwrap_or(episode),
        ),
        None => (season, episode),
    }
}

/// Fetch and cache a show's absolute ordering, if TMDB publishes one.
///
/// Called once per show, the first time its detail view opens. A show with no
/// alternate ordering writes no rows and is not asked about again this session.
pub async fn ensure(pool: &SqlitePool, tmdb_id: i64, api_key: &str) -> Result<usize> {
    if tmdb_id <= 0 || api_key.is_empty() {
        return Ok(0);
    }
    let already = sqlx::query("SELECT 1 FROM splus_episode_group WHERE tmdb_id = ?1 LIMIT 1")
        .bind(tmdb_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if already.is_some() {
        return Ok(0);
    }

    let list: Value = tulipix_core::net::http()
        .get(format!("{TMDB}/tv/{tmdb_id}/episode_groups"))
        .query(&[("api_key", api_key)])
        .send()
        .await
        .context("tmdb: episode groups request failed")?
        .json()
        .await
        .context("tmdb: bad JSON")?;

    // Type 2 is TMDB's "Absolute" ordering — the one that disagrees with the
    // season/episode split, which is exactly the disagreement we are fixing.
    let Some(group_id) = list
        .get("results")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|g| g.get("type").and_then(Value::as_i64) == Some(2))
                .or_else(|| a.first())
        })
        .and_then(|g| g.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(0);
    };

    let detail: Value = tulipix_core::net::http()
        .get(format!("{TMDB}/tv/episode_group/{group_id}"))
        .query(&[("api_key", api_key)])
        .send()
        .await
        .context("tmdb: episode group detail failed")?
        .json()
        .await
        .context("tmdb: bad JSON")?;

    let rows = flatten(&detail);
    for (season, episode, mapped_season, mapped_ep) in &rows {
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO splus_episode_group
             (tmdb_id, season, episode, mapped_season, mapped_ep) VALUES (?1,?2,?3,?4,?5)",
        )
        .bind(tmdb_id)
        .bind(season)
        .bind(episode)
        .bind(mapped_season)
        .bind(mapped_ep)
        .execute(pool)
        .await;
    }
    Ok(rows.len())
}

/// `(season, episode) -> (mapped_season, mapped_ep)` for every entry that
/// actually differs. Identical pairs are not stored — a row that says "1,4 maps
/// to 1,4" costs a write and answers nothing.
fn flatten(detail: &Value) -> Vec<(i64, i64, i64, i64)> {
    let mut out = Vec::new();
    let Some(groups) = detail.get("groups").and_then(Value::as_array) else {
        return out;
    };
    for (gi, g) in groups.iter().enumerate() {
        let group_season = g
            .get("order")
            .and_then(Value::as_i64)
            .unwrap_or(gi as i64 + 1);
        let Some(eps) = g.get("episodes").and_then(Value::as_array) else {
            continue;
        };
        for (ei, e) in eps.iter().enumerate() {
            let real_season = e.get("season_number").and_then(Value::as_i64).unwrap_or(1);
            let real_ep = e.get("episode_number").and_then(Value::as_i64).unwrap_or(0);
            let group_ep = ei as i64 + 1;
            if real_season == group_season && real_ep == group_ep {
                continue;
            }
            out.push((group_season, group_ep, real_season, real_ep));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_disagreements_are_stored() {
        let detail = serde_json::json!({"groups":[
          {"order":1,"episodes":[
            {"season_number":1,"episode_number":1},
            {"season_number":1,"episode_number":2}
          ]},
          {"order":2,"episodes":[
            {"season_number":1,"episode_number":13},
            {"season_number":1,"episode_number":14}
          ]}
        ]});
        let got = flatten(&detail);
        // Season 1 agrees entirely; season 2 maps onto season 1's back half.
        assert_eq!(got, vec![(2, 1, 1, 13), (2, 2, 1, 14)]);
    }

    #[test]
    fn a_group_with_no_episodes_is_skipped() {
        assert!(flatten(&serde_json::json!({"groups":[{"order":1}]})).is_empty());
        assert!(flatten(&serde_json::json!({})).is_empty());
    }
}
