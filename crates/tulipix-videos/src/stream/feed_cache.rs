//! Persistent cache for the landing row's picks.
//!
//! The browse feed barely moves — it is an editorial front page, not a live
//! ticker — so refetching it on every tab visit costs a request and a poster
//! batch to arrive at the same six cards. The picks are kept in the videos DB
//! and only refreshed once they have aged past [`TTL_SECS`].
//!
//! One row per kind ("trending", "vertical"), holding the already-chosen hits
//! rather than the raw payload: the selection rules are what is expensive to
//! reproduce, and re-running them on stale JSON would be a second source of
//! truth for what the row shows.

use anyhow::Result;
use sqlx::SqlitePool;

use super::SearchHit;

/// How long a stored answer is served before the servers are asked again.
pub const TTL_SECS: i64 = 12 * 60 * 60;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS stream_feed (
    kind        TEXT    PRIMARY KEY,
    payload     TEXT    NOT NULL,
    fetched_at  INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Cached {
    pub hits: Vec<SearchHit>,
    pub fetched_at: i64,
}

impl Cached {
    /// Old enough to be worth another request.
    pub fn is_stale(&self, now: i64) -> bool {
        now.saturating_sub(self.fetched_at) > TTL_SECS
    }
}

use tulipix_core::util::unix_secs_i64 as now_secs;

/// Stored picks for `kind`, if any survive and still parse.
pub async fn load(pool: &SqlitePool, kind: &str) -> Option<Cached> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT payload, fetched_at FROM stream_feed WHERE kind = ?")
            .bind(kind)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let (payload, fetched_at) = row?;
    // A payload that no longer deserializes is treated as a miss; the next
    // refresh overwrites it.
    let hits: Vec<SearchHit> = serde_json::from_str(&payload).ok()?;
    (!hits.is_empty()).then_some(Cached { hits, fetched_at })
}

/// Replace the stored picks for `kind`. An empty list is not stored — it would
/// serve an empty row for twelve hours after one bad answer.
pub async fn store(pool: &SqlitePool, kind: &str, hits: &[SearchHit]) -> Result<()> {
    if hits.is_empty() {
        return Ok(());
    }
    let payload = serde_json::to_string(hits)?;
    sqlx::query(
        "INSERT INTO stream_feed (kind, payload, fetched_at) VALUES (?, ?, ?)
         ON CONFLICT(kind) DO UPDATE SET payload = excluded.payload,
                                         fetched_at = excluded.fetched_at",
    )
    .bind(kind)
    .bind(&payload)
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(())
}

/// Is anything at all worth refetching right now?
pub fn needs_refresh(cached: Option<&Cached>) -> bool {
    match cached {
        None => true,
        Some(c) => c.is_stale(now_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn hit(id: &str) -> SearchHit {
        SearchHit {
            id: id.into(),
            title: format!("Title {id}"),
            year: "2024".into(),
            cover: format!("https://img/{id}.jpg"),
            is_series: true,
            season_subjects: Vec::new(),
        }
    }

    #[tokio::test]
    async fn round_trips_per_kind() {
        let (_t, pool) = open_pool().await;
        assert!(load(&pool, "trending").await.is_none());

        store(&pool, "trending", &[hit("a"), hit("b")]).await.unwrap();
        store(&pool, "vertical", &[hit("v")]).await.unwrap();

        let t = load(&pool, "trending").await.unwrap();
        assert_eq!(t.hits.len(), 2);
        assert_eq!(t.hits[0].id, "a");
        assert_eq!(t.hits[0].cover, "https://img/a.jpg");
        // The kinds do not tread on each other.
        assert_eq!(load(&pool, "vertical").await.unwrap().hits[0].id, "v");

        // A later write replaces rather than accumulating.
        store(&pool, "trending", &[hit("c")]).await.unwrap();
        assert_eq!(load(&pool, "trending").await.unwrap().hits.len(), 1);
    }

    #[tokio::test]
    async fn an_empty_answer_never_replaces_a_good_one() {
        let (_t, pool) = open_pool().await;
        store(&pool, "trending", &[hit("a")]).await.unwrap();
        store(&pool, "trending", &[]).await.unwrap();
        assert_eq!(load(&pool, "trending").await.unwrap().hits.len(), 1);
    }

    #[test]
    fn staleness_follows_the_ttl() {
        let fresh = Cached { hits: vec![hit("a")], fetched_at: 1_000 };
        assert!(!fresh.is_stale(1_000 + TTL_SECS));
        assert!(fresh.is_stale(1_001 + TTL_SECS));
        // Nothing stored at all always needs fetching.
        assert!(needs_refresh(None));
    }
}
