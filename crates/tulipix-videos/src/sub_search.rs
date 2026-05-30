//! Word-level FTS over whisper JSON.
//!
//! whisper.cpp's `--output-json` emits per-segment + per-token timing. We
//! flatten that into one row per word in the `sub_words` FTS5 table so the
//! UI can `search("the cake is")` → `(item_id, start_ms, end_ms)` and seek
//! the player directly. The index keys on `item_id` so deletion/refresh is
//! cheap.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub item_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub word: String,
}

/// Parse the JSON whisper.cpp writes (`--output-json` or `--output-json-full`).
/// Handles both the modern `transcription[*].offsets.from/to + text` schema
/// and the older `tokens[*]` form.
pub fn parse_whisper_json(bytes: &[u8]) -> Result<Vec<Word>> {
    let v: serde_json::Value = serde_json::from_slice(bytes).context("decode whisper json")?;
    let mut out = Vec::new();
    if let Some(arr) = v.get("transcription").and_then(|x| x.as_array()) {
        for seg in arr {
            let text = seg.get("text").and_then(|x| x.as_str()).unwrap_or("").trim();
            if text.is_empty() { continue; }
            let from = seg.pointer("/offsets/from").and_then(|x| x.as_i64()).unwrap_or(0);
            let to   = seg.pointer("/offsets/to").and_then(|x| x.as_i64()).unwrap_or(from);
            // If per-token timing is absent, distribute the segment evenly.
            let tokens: Vec<&str> = text.split_whitespace().collect();
            if tokens.is_empty() { continue; }
            let span = (to - from).max(1);
            let step = span / tokens.len() as i64;
            for (i, w) in tokens.iter().enumerate() {
                out.push(Word {
                    word: (*w).to_string(),
                    start_ms: from + step * i as i64,
                    end_ms: from + step * (i as i64 + 1),
                });
            }
        }
        return Ok(out);
    }
    // Older payload: top-level `tokens` array.
    if let Some(toks) = v.get("tokens").and_then(|x| x.as_array()) {
        for t in toks {
            let word = t.get("text").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if word.is_empty() { continue; }
            let start_ms = t.get("offsets").and_then(|o| o.get("from")).and_then(|x| x.as_i64()).unwrap_or(0);
            let end_ms   = t.get("offsets").and_then(|o| o.get("to")).and_then(|x| x.as_i64()).unwrap_or(start_ms);
            out.push(Word { word, start_ms, end_ms });
        }
    }
    Ok(out)
}

pub async fn delete_for(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM sub_words WHERE item_id = ?").bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn index_words(pool: &SqlitePool, item_id: i64, words: &[Word]) -> Result<u64> {
    delete_for(pool, item_id).await?;
    let mut tx = pool.begin().await?;
    let mut n = 0u64;
    for w in words {
        sqlx::query(
            "INSERT INTO sub_words (item_id, start_ms, end_ms, word) VALUES (?, ?, ?, ?)",
        )
        .bind(item_id).bind(w.start_ms).bind(w.end_ms).bind(&w.word)
        .execute(&mut *tx).await?;
        n += 1;
    }
    tx.commit().await?;
    Ok(n)
}

pub async fn ingest_json_file(pool: &SqlitePool, item_id: i64, path: &Path) -> Result<u64> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let words = parse_whisper_json(&bytes)?;
    index_words(pool, item_id, &words).await
}

pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    // Pass user query through sqlite's MATCH operator as a column-qualified
    // phrase so quotes etc. behave like the rest of the app's FTS calls.
    let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "SELECT item_id, start_ms, end_ms, word FROM sub_words
         WHERE sub_words MATCH ? ORDER BY rank LIMIT ?",
    )
    .bind(format!("word:{query}"))
    .bind(limit)
    .fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(item_id, start_ms, end_ms, word)| SearchHit {
        item_id, start_ms, end_ms, word,
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn parse_modern_transcription_schema() {
        let json = br#"{
            "transcription": [
                { "offsets": { "from": 0, "to": 2000 }, "text": "the cake is" },
                { "offsets": { "from": 2000, "to": 4000 }, "text": "a lie" }
            ]
        }"#;
        let w = parse_whisper_json(json).unwrap();
        assert_eq!(w.len(), 5);
        assert_eq!(w[0].word, "the");
        assert!(w[1].start_ms > 0 && w[1].start_ms < 2000);
    }

    #[test]
    fn parse_legacy_tokens_schema() {
        let json = br#"{
            "tokens": [
                { "text": "hello", "offsets": { "from": 100, "to": 500 } },
                { "text": "world", "offsets": { "from": 500, "to": 900 } }
            ]
        }"#;
        let w = parse_whisper_json(json).unwrap();
        assert_eq!(w.len(), 2);
        assert_eq!(w[1].word, "world");
    }

    #[tokio::test]
    async fn index_then_search() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.mkv', 0, 1, 0, 'videos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/a.mkv'").fetch_one(&pool).await.unwrap();
        let words = vec![
            Word { word: "hello".into(), start_ms: 1000, end_ms: 1500 },
            Word { word: "world".into(), start_ms: 1500, end_ms: 2000 },
        ];
        let n = index_words(&pool, id, &words).await.unwrap();
        assert_eq!(n, 2);
        let hits = search(&pool, "hello", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_id, id);
        assert_eq!(hits[0].start_ms, 1000);
    }

    #[tokio::test]
    async fn reindex_replaces_rather_than_appends() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.mkv', 0, 1, 0, 'videos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/a.mkv'").fetch_one(&pool).await.unwrap();
        let w1 = vec![Word { word: "foo".into(), start_ms: 0, end_ms: 100 }];
        let w2 = vec![Word { word: "bar".into(), start_ms: 0, end_ms: 100 }];
        index_words(&pool, id, &w1).await.unwrap();
        index_words(&pool, id, &w2).await.unwrap();
        let hits = search(&pool, "foo", 10).await.unwrap();
        assert!(hits.is_empty());
        let hits = search(&pool, "bar", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
    }
}
