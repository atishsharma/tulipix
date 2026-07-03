//! Chapter extraction via ffprobe `-show_chapters`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;
use std::process::Command;
use tulipix_core::thumbs::tool_bin;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Chapter {
    pub idx: i64,
    pub title: Option<String>,
    pub start_s: f64,
    pub end_s: Option<f64>,
}

pub fn extract(path: &Path) -> Result<Vec<Chapter>> {
    let bin = tool_bin("ffprobe");
    let out = Command::new(&bin)
        .args(["-loglevel", "error", "-print_format", "json", "-show_chapters"])
        .arg(path)
        .output()
        .with_context(|| format!("spawn {}", bin.display()))?;
    if !out.status.success() {
        anyhow::bail!("ffprobe exit {} — {}", out.status, String::from_utf8_lossy(&out.stderr));
    }
    parse_json(&out.stdout)
}

pub fn parse_json(bytes: &[u8]) -> Result<Vec<Chapter>> {
    let v: serde_json::Value = serde_json::from_slice(bytes).context("decode ffprobe chapters json")?;
    let arr = v.get("chapters").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.len());
    for (i, c) in arr.iter().enumerate() {
        let start = c.get("start_time").and_then(|x| x.as_str()).and_then(|s| s.parse().ok());
        let end   = c.get("end_time").and_then(|x| x.as_str()).and_then(|s| s.parse().ok());
        let title = c.get("tags").and_then(|t| t.get("title")).and_then(|x| x.as_str()).map(str::to_string);
        if let Some(s) = start {
            out.push(Chapter { idx: i as i64, title, start_s: s, end_s: end });
        }
    }
    Ok(out)
}

pub async fn store(pool: &SqlitePool, item_id: i64, chapters: &[Chapter]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM chapters WHERE item_id = ?").bind(item_id).execute(&mut *tx).await?;
    for c in chapters {
        sqlx::query(
            "INSERT INTO chapters (item_id, idx, title, start_s, end_s) VALUES (?, ?, ?, ?, ?)",
        ).bind(item_id).bind(c.idx).bind(&c.title).bind(c.start_s).bind(c.end_s)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn ingest(pool: &SqlitePool, item_id: i64) -> Result<usize> {
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id).fetch_optional(pool).await?;
    let Some(path) = path else { return Ok(0); };
    let chapters = extract(Path::new(&path)).unwrap_or_default();
    let n = chapters.len();
    store(pool, item_id, &chapters).await?;
    Ok(n)
}

pub async fn list_for(pool: &SqlitePool, item_id: i64) -> Result<Vec<Chapter>> {
    let rows: Vec<(i64, Option<String>, f64, Option<f64>)> = sqlx::query_as(
        "SELECT idx, title, start_s, end_s FROM chapters WHERE item_id = ? ORDER BY idx",
    ).bind(item_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(idx, title, start_s, end_s)| Chapter { idx, title, start_s, end_s }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn parse_handles_typical_ffprobe_output() {
        let json = br#"{
            "chapters": [
                { "start_time": "0.000", "end_time": "60.500", "tags": { "title": "Intro" } },
                { "start_time": "60.500", "end_time": "1234.000", "tags": { "title": "Chapter 1" } }
            ]
        }"#;
        let v = parse_json(json).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].title.as_deref(), Some("Intro"));
        assert!((v[0].start_s - 0.0).abs() < 1e-9);
        assert!((v[1].start_s - 60.5).abs() < 1e-9);
    }

    #[test]
    fn parse_empty_when_no_chapters() {
        let v = parse_json(br#"{"chapters": []}"#).unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn store_and_list_round_trip() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.mkv', 0, 1, 0, 'videos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/a.mkv'").fetch_one(&pool).await.unwrap();
        let chapters = vec![
            Chapter { idx: 0, title: Some("Intro".into()), start_s: 0.0, end_s: Some(60.0) },
            Chapter { idx: 1, title: None, start_s: 60.0, end_s: Some(120.0) },
        ];
        store(&pool, id, &chapters).await.unwrap();
        let back = list_for(&pool, id).await.unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].title.as_deref(), Some("Intro"));
        // Re-storing replaces, not appends.
        store(&pool, id, &chapters[..1]).await.unwrap();
        assert_eq!(list_for(&pool, id).await.unwrap().len(), 1);
    }
}
