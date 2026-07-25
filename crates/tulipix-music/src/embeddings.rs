//! `np.p4.music.embeddings` — CLAP/PANNs ONNX embeddings → "Sonic Similar".
//!
//! Stores per-track audio embeddings as little-endian f32 BLOBs and finds the
//! nearest tracks by cosine similarity. Inference is upstream (ONNX in the
//! worker); this owns serialization and the similarity search.

use anyhow::Result;
use sqlx::SqlitePool;

pub fn to_bytes(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for f in vec { out.extend_from_slice(&f.to_le_bytes()); }
    out
}

pub fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 0.0; }
    let mut dot = 0.0; let mut na = 0.0; let mut nb = 0.0;
    for i in 0..a.len() { dot += a[i] * b[i]; na += a[i] * a[i]; nb += b[i] * b[i]; }
    if na == 0.0 || nb == 0.0 { return 0.0; }
    dot / (na.sqrt() * nb.sqrt())
}

pub async fn store(pool: &SqlitePool, item_id: i64, model: &str, vec: &[f32]) -> Result<()> {
    sqlx::query(
        "INSERT INTO track_embeddings (item_id, dim, model, vec) VALUES (?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET dim=excluded.dim, model=excluded.model, vec=excluded.vec",
    ).bind(item_id).bind(vec.len() as i64).bind(model).bind(to_bytes(vec)).execute(pool).await?;
    Ok(())
}

/// Top-`k` most sonically-similar tracks to `seed`, excluding the seed itself.
/// Only compares embeddings from the seed's own `model` — cosine across
/// different embedding spaces (dsp-v1 vs CLAP) is meaningless.
pub async fn similar(pool: &SqlitePool, seed: i64, k: usize) -> Result<Vec<(i64, f32)>> {
    let seed_row: Option<(Vec<u8>, String)> = sqlx::query_as("SELECT vec, model FROM track_embeddings WHERE item_id = ?")
        .bind(seed).fetch_optional(pool).await?;
    let Some((seed_bytes, model)) = seed_row else { return Ok(vec![]); };
    let seed = (seed, from_bytes(&seed_bytes));
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT item_id, vec FROM track_embeddings WHERE item_id != ? AND model = ?")
        .bind(seed.0).bind(&model).fetch_all(pool).await?;
    let mut scored: Vec<(i64, f32)> = rows.into_iter()
        .map(|(id, b)| (id, cosine(&seed.1, &from_bytes(&b)))).collect();
    // total_cmp, not partial_cmp().unwrap(): a stored vector is raw bytes from
    // the DB and every bit pattern decodes to a valid f32, so a corrupt row can
    // yield NaN. partial_cmp then returns None and the unwrap aborts the process
    // (release builds use panic = "abort"). total_cmp orders NaN instead.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(k);
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn nan_scores_sort_without_panicking() {
        // A corrupt embedding blob decodes to NaN — every bit pattern is a valid
        // f32 — and NaN through partial_cmp().unwrap() used to abort the process.
        let mut scored: Vec<(i64, f32)> = vec![(1, 0.5), (2, f32::NAN), (3, 0.9)];
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        assert_eq!(scored.len(), 3);
        // Real scores still rank correctly among themselves.
        let real: Vec<i64> = scored.iter().filter(|(_, s)| !s.is_nan()).map(|(i, _)| *i).collect();
        assert_eq!(real, vec![3, 1]);
    }

    #[test]
    fn bytes_roundtrip_and_cosine() {
        let v = vec![1.0f32, 2.0, 3.0];
        assert_eq!(from_bytes(&to_bytes(&v)), v);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[tokio::test]
    async fn similar_ranks_closest_first() {
        let (_t, pool) = open_pool().await;
        let seed = add_track(&pool, "/m/seed.flac").await;
        let near = add_track(&pool, "/m/near.flac").await;
        let far = add_track(&pool, "/m/far.flac").await;
        store(&pool, seed, "clap", &[1.0, 0.0, 0.0]).await.unwrap();
        store(&pool, near, "clap", &[0.9, 0.1, 0.0]).await.unwrap();
        store(&pool, far, "clap", &[0.0, 0.0, 1.0]).await.unwrap();
        let r = similar(&pool, seed, 2).await.unwrap();
        assert_eq!(r[0].0, near);
        assert_eq!(r.len(), 2);
    }
}
