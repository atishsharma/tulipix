//! Persistent cache for resolved streams.
//!
//! Resolving one episode costs a request per resolution rung, and the answers
//! barely change between sessions — so results are kept in the videos DB keyed
//! by exactly what identifies them: `(subject, season, episode, resolution)`.
//!
//! [`store`] rewrites a row only when the payload actually differs, so a refresh
//! that finds nothing new costs a single timestamp update instead of churning
//! the table.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::StreamFile;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS stream_cache (
    subject_id  TEXT    NOT NULL,
    season      INTEGER NOT NULL,
    episode     INTEGER NOT NULL,
    resolution  TEXT    NOT NULL,
    payload     TEXT    NOT NULL,
    fetched_at  INTEGER NOT NULL,
    PRIMARY KEY (subject_id, season, episode, resolution)
);
CREATE INDEX IF NOT EXISTS stream_cache_fetched_idx ON stream_cache(fetched_at);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// How long a cached answer is served without going back to the servers.
pub const TTL_SECS: i64 = 6 * 60 * 60;

/// Identifies one resolved stream list. `resolution` is the rung that was
/// requested ("1080", "720", …) or `""` for the merged all-rungs answer, so the
/// two never overwrite each other.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub subject_id: String,
    pub season: i64,
    pub episode: i64,
    pub resolution: String,
}

impl Key {
    pub fn new(subject_id: &str, season: i64, episode: i64, resolution: &str) -> Self {
        Self {
            subject_id: subject_id.to_string(),
            season,
            episode,
            resolution: resolution.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cached {
    pub files: Vec<StreamFile>,
    pub fetched_at: i64,
}

impl Cached {
    /// Old enough that a background refresh is worth the request.
    pub fn is_stale(&self, now: i64) -> bool {
        now.saturating_sub(self.fetched_at) > TTL_SECS
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Cached answer for `key`, if one was stored and still parses.
pub async fn load(pool: &SqlitePool, key: &Key) -> Option<Cached> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT payload, fetched_at FROM stream_cache
         WHERE subject_id = ? AND season = ? AND episode = ? AND resolution = ?",
    )
    .bind(&key.subject_id)
    .bind(key.season)
    .bind(key.episode)
    .bind(&key.resolution)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (payload, fetched_at) = row?;
    // A payload that no longer deserializes (struct changed under it) is simply
    // treated as a miss — the next fetch overwrites it.
    let files: Vec<StreamFile> = serde_json::from_str(&payload).ok()?;
    Some(Cached { files, fetched_at })
}

/// Persist `files` for `key`. Returns `true` when the stored list actually
/// changed — an unchanged refresh only moves the timestamp forward.
pub async fn store(pool: &SqlitePool, key: &Key, files: &[StreamFile]) -> Result<bool> {
    let payload = serde_json::to_string(files)?;
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT payload FROM stream_cache
         WHERE subject_id = ? AND season = ? AND episode = ? AND resolution = ?",
    )
    .bind(&key.subject_id)
    .bind(key.season)
    .bind(key.episode)
    .bind(&key.resolution)
    .fetch_optional(pool)
    .await?;

    if existing.as_deref() == Some(payload.as_str()) {
        sqlx::query(
            "UPDATE stream_cache SET fetched_at = ?
             WHERE subject_id = ? AND season = ? AND episode = ? AND resolution = ?",
        )
        .bind(now_secs())
        .bind(&key.subject_id)
        .bind(key.season)
        .bind(key.episode)
        .bind(&key.resolution)
        .execute(pool)
        .await?;
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO stream_cache (subject_id, season, episode, resolution, payload, fetched_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(subject_id, season, episode, resolution)
         DO UPDATE SET payload = excluded.payload, fetched_at = excluded.fetched_at",
    )
    .bind(&key.subject_id)
    .bind(key.season)
    .bind(key.episode)
    .bind(&key.resolution)
    .bind(&payload)
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(true)
}

/// Drop everything older than `max_age_secs`. Called on nothing automatically —
/// exposed so a maintenance action can reclaim the space.
pub async fn prune(pool: &SqlitePool, max_age_secs: i64) -> Result<u64> {
    let cutoff = now_secs().saturating_sub(max_age_secs);
    let r = sqlx::query("DELETE FROM stream_cache WHERE fetched_at < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn file(res: i64, url: &str) -> StreamFile {
        StreamFile {
            url: url.into(),
            resource_id: "r".into(),
            resolution: res,
            codec: "hevc".into(),
            size: "1.0 GB".into(),
            uploader: "Alice".into(),
            captions: Vec::new(),
        }
    }

    #[tokio::test]
    async fn round_trips_a_stream_list() {
        let (_t, pool) = open_pool().await;
        let key = Key::new("subj", 1, 3, "1080");
        assert!(load(&pool, &key).await.is_none());

        let files = vec![file(1080, "https://v/a"), file(720, "https://v/b")];
        assert!(store(&pool, &key, &files).await.unwrap());

        let got = load(&pool, &key).await.unwrap();
        assert_eq!(got.files, files);
        assert!(!got.is_stale(now_secs()));
        assert!(got.is_stale(now_secs() + TTL_SECS + 1));
    }

    /// The whole point of the cache: an unchanged refresh must not rewrite.
    #[tokio::test]
    async fn unchanged_payload_reports_no_change() {
        let (_t, pool) = open_pool().await;
        let key = Key::new("subj", 1, 1, "720");
        let files = vec![file(720, "https://v/a")];

        assert!(store(&pool, &key, &files).await.unwrap(), "first write is a change");
        assert!(!store(&pool, &key, &files).await.unwrap(), "identical write is not");

        let changed = vec![file(720, "https://v/a"), file(720, "https://v/c")];
        assert!(store(&pool, &key, &changed).await.unwrap());
        assert_eq!(load(&pool, &key).await.unwrap().files.len(), 2);
    }

    /// Every part of the key has to separate rows, or episodes bleed together.
    #[tokio::test]
    async fn each_key_field_is_a_separate_row() {
        let (_t, pool) = open_pool().await;
        let base = Key::new("subj", 1, 1, "1080");
        store(&pool, &base, &[file(1080, "https://v/base")]).await.unwrap();

        for other in [
            Key::new("other", 1, 1, "1080"),
            Key::new("subj", 2, 1, "1080"),
            Key::new("subj", 1, 2, "1080"),
            Key::new("subj", 1, 1, "720"),
        ] {
            assert!(load(&pool, &other).await.is_none(), "{other:?} collided with {base:?}");
        }
        assert_eq!(load(&pool, &base).await.unwrap().files.len(), 1);
    }

    #[tokio::test]
    async fn prune_drops_only_the_old() {
        let (_t, pool) = open_pool().await;
        let key = Key::new("subj", 1, 1, "");
        store(&pool, &key, &[file(0, "https://v/a")]).await.unwrap();
        assert_eq!(prune(&pool, TTL_SECS).await.unwrap(), 0);
        assert_eq!(prune(&pool, -1).await.unwrap(), 1);
        assert!(load(&pool, &key).await.is_none());
    }
}
