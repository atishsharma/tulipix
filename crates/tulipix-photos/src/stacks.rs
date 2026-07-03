//! Photo stacks — single-cover groups over bursts + near-duplicates.
//!
//! A "stack" is the user-visible aggregation that hides clutter in the grid.
//! Two inputs are folded into one structure:
//!  * Burst groups from `burst::detect` (same camera, consecutive shots).
//!  * pHash clusters from `dedup` (perceptual near-dups).
//!
//! Stacks are persisted lightly: only the cover assignment + which photos are
//! "inside" a stack are stored, so the cluster tables remain authoritative.
//! Cover defaults to the starred member if any, else the first by `taken_at`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::burst::{detect as detect_bursts, BurstSet};

pub const STACKS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS photo_stacks (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind      TEXT    NOT NULL,        -- 'burst' | 'phash'
    cover_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    expanded  INTEGER NOT NULL DEFAULT 0,
    created   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS photo_stack_members (
    stack_id INTEGER NOT NULL REFERENCES photo_stacks(id) ON DELETE CASCADE,
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY (stack_id, item_id)
);
CREATE INDEX IF NOT EXISTS photo_stack_members_item_idx ON photo_stack_members(item_id);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackKind {
    Burst,
    Phash,
}

impl StackKind {
    fn as_str(self) -> &'static str {
        match self { Self::Burst => "burst", Self::Phash => "phash" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stack {
    pub id: i64,
    pub kind: StackKind,
    pub cover_id: i64,
    pub member_ids: Vec<i64>,
    pub expanded: bool,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(STACKS_SCHEMA).execute(pool).await?;
    Ok(())
}

/// Pick the cover for a member set: the starred member if any, else the
/// earliest by `taken_at`, falling back to the lowest id.
async fn pick_cover(pool: &SqlitePool, ids: &[i64]) -> Result<i64> {
    if ids.is_empty() { anyhow::bail!("empty stack"); }
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT items.id FROM items
         LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.id IN ({placeholders})
         ORDER BY COALESCE(photo_meta.starred, 0) DESC,
                  COALESCE(photo_meta.taken_at, items.added) ASC,
                  items.id ASC
         LIMIT 1",
    );
    let mut q = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids { q = q.bind(*id); }
    Ok(q.fetch_one(pool).await?)
}

/// Wipe existing stacks and recompute from burst + phash sources. Bursts win
/// on overlap — a member appearing in both joins the burst stack only.
pub async fn rebuild(pool: &SqlitePool) -> Result<usize> {
    apply_schema(pool).await?;
    sqlx::query("DELETE FROM photo_stacks").execute(pool).await?;

    let mut covered: HashSet<i64> = HashSet::new();
    let mut count = 0usize;

    let bursts: Vec<BurstSet> = detect_bursts(pool).await?;
    for b in &bursts {
        if b.item_ids.len() < 2 { continue; }
        let cover = pick_cover(pool, &b.item_ids).await?;
        let id = insert_stack(pool, StackKind::Burst, cover, &b.item_ids).await?;
        for m in &b.item_ids { covered.insert(*m); }
        count += 1;
        tracing::debug!(stack = id, kind = "burst", members = b.item_ids.len(), "stack created");
    }

    let phash: Vec<(i64, Vec<i64>)> = phash_clusters(pool).await?;
    for (_cid, members) in phash {
        let filtered: Vec<i64> = members.into_iter().filter(|m| !covered.contains(m)).collect();
        if filtered.len() < 2 { continue; }
        let cover = pick_cover(pool, &filtered).await?;
        for m in &filtered { covered.insert(*m); }
        let _ = insert_stack(pool, StackKind::Phash, cover, &filtered).await?;
        count += 1;
    }
    Ok(count)
}

async fn phash_clusters(pool: &SqlitePool) -> Result<Vec<(i64, Vec<i64>)>> {
    let cluster_ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM dedup_clusters WHERE kind = 'phash'",
    ).fetch_all(pool).await?;
    let mut out = Vec::new();
    for (cid,) in cluster_ids {
        let members: Vec<(i64,)> = sqlx::query_as(
            "SELECT item_id FROM dedup_members WHERE cluster_id = ?",
        ).bind(cid).fetch_all(pool).await?;
        out.push((cid, members.into_iter().map(|(x,)| x).collect()));
    }
    Ok(out)
}

async fn insert_stack(
    pool: &SqlitePool,
    kind: StackKind,
    cover: i64,
    members: &[i64],
) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO photo_stacks (kind, cover_id, created) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(kind.as_str()).bind(cover).bind(now())
    .fetch_one(pool).await?;
    for m in members {
        sqlx::query("INSERT OR IGNORE INTO photo_stack_members (stack_id, item_id) VALUES (?, ?)")
            .bind(id).bind(m).execute(pool).await?;
    }
    Ok(id)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Stack>> {
    apply_schema(pool).await?;
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT id, kind, cover_id, expanded FROM photo_stacks ORDER BY id",
    ).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, kind, cover_id, exp) in rows {
        let members: Vec<(i64,)> = sqlx::query_as(
            "SELECT item_id FROM photo_stack_members WHERE stack_id = ? ORDER BY item_id",
        ).bind(id).fetch_all(pool).await?;
        out.push(Stack {
            id,
            kind: if kind == "burst" { StackKind::Burst } else { StackKind::Phash },
            cover_id,
            member_ids: members.into_iter().map(|(x,)| x).collect(),
            expanded: exp != 0,
        });
    }
    Ok(out)
}

pub async fn set_cover(pool: &SqlitePool, stack_id: i64, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE photo_stacks SET cover_id = ? WHERE id = ?
                 AND EXISTS (SELECT 1 FROM photo_stack_members WHERE stack_id = ? AND item_id = ?)")
        .bind(item_id).bind(stack_id).bind(stack_id).bind(item_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_expanded(pool: &SqlitePool, stack_id: i64, expanded: bool) -> Result<()> {
    sqlx::query("UPDATE photo_stacks SET expanded = ? WHERE id = ?")
        .bind(if expanded { 1 } else { 0 }).bind(stack_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn unstack(pool: &SqlitePool, stack_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM photo_stacks WHERE id = ?")
        .bind(stack_id).execute(pool).await?;
    Ok(())
}

/// IDs of items that are *inside* a stack but not the cover — the grid hides
/// these by default to give the single-cover effect.
pub async fn hidden_ids(pool: &SqlitePool) -> Result<HashSet<i64>> {
    apply_schema(pool).await?;
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT m.item_id FROM photo_stack_members m
         JOIN photo_stacks s ON s.id = m.stack_id
         WHERE m.item_id <> s.cover_id AND s.expanded = 0",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(x,)| x).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, seed_photo as seed};

    #[tokio::test]
    async fn rebuild_creates_a_burst_stack() {
        let (_t, pool) = open_pool().await;
        for i in 0..3 { seed(&pool, &format!("/p/{i}.jpg"), 1000 + i, "C1").await; }
        let n = rebuild(&pool).await.unwrap();
        assert_eq!(n, 1);
        let s = list(&pool).await.unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, StackKind::Burst);
        assert_eq!(s[0].member_ids.len(), 3);
        // hidden ids = members minus cover
        let h = hidden_ids(&pool).await.unwrap();
        assert_eq!(h.len(), 2);
        assert!(!h.contains(&s[0].cover_id));
    }

    #[tokio::test]
    async fn cover_change_then_unstack() {
        let (_t, pool) = open_pool().await;
        let ids: Vec<i64> = {
            let mut v = Vec::new();
            for i in 0..3 { v.push(seed(&pool, &format!("/p/{i}.jpg"), 2000 + i, "C1").await); }
            v
        };
        rebuild(&pool).await.unwrap();
        let s = list(&pool).await.unwrap();
        let sid = s[0].id;
        let new_cover = ids[2];
        set_cover(&pool, sid, new_cover).await.unwrap();
        let s2 = list(&pool).await.unwrap();
        assert_eq!(s2[0].cover_id, new_cover);
        unstack(&pool, sid).await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
