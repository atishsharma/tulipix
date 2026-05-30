//! CLIP image + text embeddings.
//!
//! Both directions hit the same ViT-B/32 trunk (image vs text encoder
//! branches; one ONNX file per branch). Vectors are 512-d fp32 L2-normalised
//! so cosine similarity reduces to a dot product. Production binds via the
//! shared ORT session; tests use deterministic Null embedders.
//!
//! Persistence: `clip_embeddings` table — one row per (item, model). Rebuild
//! is idempotent: same model name + same item replaces in place.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

pub const DIM: usize = 512;

pub trait ImageEmbedder: Send + Sync {
    fn embed(&self, image_path: &Path) -> Result<Vec<f32>>;
    fn model_name(&self) -> &str;
}

pub trait TextEmbedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn model_name(&self) -> &str;
}

/// Null embedder for unit tests / no-model installs — returns a vector
/// deterministically derived from the input bytes. Always normalised.
pub struct NullEmbedder { pub model: String }
impl NullEmbedder {
    pub fn new(model: &str) -> Self { Self { model: model.into() } }
    fn derive(&self, seed: &[u8]) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        let mut x = 0x12345678u32;
        for b in seed {
            x = x.wrapping_mul(1664525).wrapping_add(*b as u32).wrapping_add(1013904223);
            v[(x as usize) % DIM] += ((x >> 16) as i16 as f32) / 32_768.0;
        }
        l2_normalise(&mut v);
        v
    }
}
impl ImageEmbedder for NullEmbedder {
    fn embed(&self, p: &Path) -> Result<Vec<f32>> {
        let bytes = std::fs::read(p)?;
        Ok(self.derive(&bytes))
    }
    fn model_name(&self) -> &str { &self.model }
}
impl TextEmbedder for NullEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.derive(text.as_bytes()))
    }
    fn model_name(&self) -> &str { &self.model }
}

pub fn l2_normalise(v: &mut [f32]) {
    let n: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::MIN_POSITIVE);
    for x in v { *x /= n; }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipHit {
    pub item_id: i64,
    pub score: f32,
    pub abs_path: String,
}

/// Encode + persist one photo. Idempotent on (item_id, model).
pub async fn index_image(pool: &SqlitePool, item_id: i64, embedder: &dyn ImageEmbedder) -> Result<()> {
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id).fetch_optional(pool).await?;
    let Some(path) = path else { return Ok(()); };
    let mut vec = embedder.embed(Path::new(&path))?;
    if vec.len() != DIM { anyhow::bail!("embedder returned wrong dim ({})", vec.len()); }
    l2_normalise(&mut vec);
    let blob = vec.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    sqlx::query(
        "INSERT INTO clip_embeddings (item_id, model, vec, updated) VALUES (?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET model = excluded.model, vec = excluded.vec, updated = excluded.updated",
    )
    .bind(item_id).bind(embedder.model_name()).bind(blob).bind(now)
    .execute(pool).await?;
    Ok(())
}

/// Encode a query string, score every persisted image embedding by cosine
/// similarity (after L2 normalisation both are unit vectors → dot product),
/// return top-K hits.
pub async fn search_text(
    pool: &SqlitePool,
    query: &str,
    embedder: &dyn TextEmbedder,
    top_k: usize,
) -> Result<Vec<ClipHit>> {
    let mut q_vec = embedder.embed(query)?;
    l2_normalise(&mut q_vec);
    let rows: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, clip_embeddings.vec
         FROM clip_embeddings
         JOIN items ON items.id = clip_embeddings.item_id
         WHERE clip_embeddings.model = ?
           AND items.missing_since IS NULL",
    )
    .bind(embedder.model_name()).fetch_all(pool).await?;
    let mut scored: Vec<ClipHit> = Vec::with_capacity(rows.len());
    for (id, path, blob) in rows {
        if blob.len() != DIM * 4 { continue; }
        let mut v = Vec::with_capacity(DIM);
        for c in blob.chunks_exact(4) {
            v.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        let s = cosine(&q_vec, &v);
        scored.push(ClipHit { item_id: id, score: s, abs_path: path });
    }
    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn null_embedder_roundtrip_and_search() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let p_dog = tmp.path().join("dog.jpg");
        let p_cat = tmp.path().join("cat.jpg");
        std::fs::write(&p_dog, b"dog dog dog").unwrap();
        std::fs::write(&p_cat, b"cat cat cat").unwrap();
        for p in [&p_dog, &p_cat] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        }
        let img_ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM items ORDER BY id").fetch_all(&pool).await.unwrap();
        let embed = NullEmbedder::new("clip-vit-b-32");
        for id in &img_ids {
            index_image(&pool, *id, &embed).await.unwrap();
        }
        let hits = search_text(&pool, "dog dog dog", &embed, 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        // top hit should be the dog photo (same input bytes → same embedding)
        assert!(hits[0].abs_path.ends_with("dog.jpg"));
    }

    #[test]
    fn cosine_of_normalised_is_dot() {
        let mut a = vec![1.0, 2.0, 2.0];
        let mut b = vec![0.0, 1.0, 0.0];
        l2_normalise(&mut a); l2_normalise(&mut b);
        let c = cosine(&a, &b);
        assert!((c - (2.0_f32 / 3.0)).abs() < 1e-6);
    }
}
