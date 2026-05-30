//! Copy-paste edits (Picasa parity).
//!
//! Picks up every op from a source photo's stack and applies it on top of N
//! target photos' stacks. Uses the `edit_clipboard` singleton table so the
//! clipboard survives panel switches. Doesn't paste `Crop`/`Text`/AI mask
//! ops by default — those are tied to the source photo's pixel space — but
//! callers can opt in via `PasteFlags`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::ops::{load, save, EditOp, EditStack};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PasteFlags {
    pub include_crop: bool,
    pub include_text: bool,
    pub include_ai_masks: bool,
}

fn is_pixel_local(op: &EditOp) -> bool {
    matches!(op, EditOp::Crop {..} | EditOp::Text {..} | EditOp::Heal {..} | EditOp::Sky {..})
}

pub async fn copy(pool: &SqlitePool, source: i64) -> Result<EditStack> {
    let stack = load(pool, source).await?;
    let body = serde_json::to_string(&stack.ops)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    sqlx::query(
        "INSERT INTO edit_clipboard (id, ops, copied) VALUES (1, ?, ?)
         ON CONFLICT(id) DO UPDATE SET ops = excluded.ops, copied = excluded.copied",
    ).bind(body).bind(now).execute(pool).await?;
    Ok(stack)
}

pub async fn clipboard(pool: &SqlitePool) -> Result<Option<Vec<EditOp>>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT ops FROM edit_clipboard WHERE id = 1")
        .fetch_optional(pool).await?;
    let Some((ops_json,)) = row else { return Ok(None); };
    let ops: Vec<EditOp> = serde_json::from_str(&ops_json).unwrap_or_default();
    Ok(Some(ops))
}

pub async fn paste(pool: &SqlitePool, targets: &[i64], flags: PasteFlags) -> Result<u64> {
    let Some(clip) = clipboard(pool).await? else { return Ok(0); };
    let filtered: Vec<EditOp> = clip.into_iter().filter(|op| {
        if !is_pixel_local(op) { return true; }
        match op {
            EditOp::Crop {..} => flags.include_crop,
            EditOp::Text {..} => flags.include_text,
            EditOp::Heal {..} | EditOp::Sky {..} => flags.include_ai_masks,
            _ => true,
        }
    }).collect();
    let mut count = 0u64;
    for &tid in targets {
        let mut stack = load(pool, tid).await?;
        for op in &filtered {
            stack.push(op.clone());
        }
        save(pool, tid, &stack).await?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use crate::editor::ops::EditOp;

    async fn seed(pool: &SqlitePool, n: usize) -> Vec<i64> {
        let mut ids = Vec::new();
        for i in 0..n {
            let p = format!("/p/{i}.jpg");
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(&p).execute(pool).await.unwrap();
            ids.push(sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(&p).fetch_one(pool).await.unwrap());
        }
        ids
    }

    #[tokio::test]
    async fn copy_then_paste_skips_crop_by_default() {
        let (_t, pool) = open_pool().await;
        let ids = seed(&pool, 3).await;
        // source has: Enhance + Crop
        let mut s = EditStack::new();
        s.push(EditOp::Enhance);
        s.push(EditOp::Crop { x: 0, y: 0, w: 10, h: 10, rotate_deg: 0.0, flip_h: false, flip_v: false });
        save(&pool, ids[0], &s).await.unwrap();

        copy(&pool, ids[0]).await.unwrap();
        let n = paste(&pool, &ids[1..], PasteFlags::default()).await.unwrap();
        assert_eq!(n, 2);
        let dest = load(&pool, ids[1]).await.unwrap();
        assert_eq!(dest.active_ops().len(), 1); // only Enhance
        assert!(matches!(dest.active_ops()[0], EditOp::Enhance));
    }
}
