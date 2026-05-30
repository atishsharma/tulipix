//! `np.p4.sync.activity` — activity event feed.
//!
//! A reverse-chronological feed of who did what (uploaded / shared /
//! commented). Mirrors backend events locally so the feed renders offline;
//! supports keyset pagination (`before` cursor) for infinite scroll.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub actor: String,
    pub verb: String,
    pub object: Option<String>,
    pub at: i64,
}

pub async fn record(pool: &SqlitePool, actor: &str, verb: &str, object: Option<&str>, at: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO activity (actor, verb, object, at) VALUES (?,?,?,?) RETURNING id")
        .bind(actor).bind(verb).bind(object).bind(at).fetch_one(pool).await?)
}

/// Newest `limit` events with `at < before` (use `i64::MAX` for the first page).
pub async fn feed(pool: &SqlitePool, before: i64, limit: i64) -> Result<Vec<Event>> {
    let rows: Vec<(i64, String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, actor, verb, object, at FROM activity WHERE at < ? ORDER BY at DESC, id DESC LIMIT ?",
    ).bind(before).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, actor, verb, object, at)| Event { id, actor, verb, object, at }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn feed_paginates_newest_first() {
        let (_t, pool) = open_pool().await;
        record(&pool, "alice", "uploaded", Some("IMG_1.jpg"), 100).await.unwrap();
        record(&pool, "bob", "shared", Some("Album"), 200).await.unwrap();
        record(&pool, "carol", "commented", None, 300).await.unwrap();
        let page1 = feed(&pool, i64::MAX, 2).await.unwrap();
        assert_eq!(page1[0].actor, "carol"); // newest
        assert_eq!(page1.len(), 2);
        let page2 = feed(&pool, page1.last().unwrap().at, 2).await.unwrap();
        assert_eq!(page2[0].actor, "alice"); // older page
        assert_eq!(page2.len(), 1);
    }
}
