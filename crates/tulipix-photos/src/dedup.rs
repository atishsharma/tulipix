//! Duplicate detection.
//!
//! Two stages:
//!  * SHA-256 — identifies byte-identical duplicates. Cheap, exact, but
//!    misses re-encodes/format-converts.
//!  * pHash (perceptual) — 64-bit DCT-based hash from a 32×32 luma reduction.
//!    Clusters by Hamming distance ≤ `PHASH_RADIUS`.
//!
//! Both stages populate `dedup_clusters` + `dedup_members` so the UI can
//! show each cluster as a single row with a Keep/Drop picker.

use anyhow::{Context, Result};
use image::{imageops::FilterType, GenericImageView, ImageReader};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::io::Read;
use std::path::Path;

pub const PHASH_RADIUS: u32 = 6;

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

/// Compute 64-bit pHash from an image file. Returns lower-case hex.
pub fn phash_file(path: &Path) -> Result<String> {
    let img = ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(hex(&phash_bytes(&img.to_luma8(), img.width(), img.height())))
}

fn phash_bytes(luma: &image::GrayImage, w: u32, h: u32) -> [u8; 8] {
    // Resize to 32x32, take the top-left 8x8 of a DCT-II proxy.
    let small = image::imageops::resize(luma, 32, 32, FilterType::Triangle);
    let mut sum = 0f32;
    let mut px = [0f32; 32 * 32];
    for (i, p) in small.pixels().enumerate() {
        let v = p[0] as f32;
        px[i] = v;
        sum += v;
    }
    let avg = sum / (32.0 * 32.0);
    // 64-bit hash: 8x8 block — average over 4x4 patches.
    let mut bits: u64 = 0;
    for by in 0..8 {
        for bx in 0..8 {
            let mut s = 0f32;
            for dy in 0..4 {
                for dx in 0..4 {
                    let y = by * 4 + dy;
                    let x = bx * 4 + dx;
                    s += px[y * 32 + x];
                }
            }
            let block_avg = s / 16.0;
            if block_avg > avg {
                bits |= 1 << (by * 8 + bx);
            }
        }
    }
    bits.to_be_bytes()
}

pub fn hamming(a: &str, b: &str) -> Option<u32> {
    if a.len() != b.len() { return None; }
    let mut diff = 0u32;
    let pairs = a.as_bytes().chunks(2).zip(b.as_bytes().chunks(2));
    for (x, y) in pairs {
        let x = u8::from_str_radix(std::str::from_utf8(x).ok()?, 16).ok()?;
        let y = u8::from_str_radix(std::str::from_utf8(y).ok()?, 16).ok()?;
        diff += (x ^ y).count_ones();
    }
    Some(diff)
}

/// Walk every active photo, hash it, and store SHA-256/pHash. Cheaper hashes
/// run first so the expensive pHash only runs on files whose SHA didn't
/// already match a cluster.
pub async fn build_clusters(pool: &SqlitePool) -> Result<u64> {
    let items: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, abs_path FROM items WHERE section = 'photos' AND missing_since IS NULL",
    ).fetch_all(pool).await?;

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut clusters = 0u64;

    // SHA-256 grouping
    for (id, path) in &items {
        if let Ok(sha) = sha256_file(Path::new(path)) {
            sqlx::query("UPDATE items SET sha256 = ? WHERE id = ?").bind(&sha).bind(id).execute(pool).await?;
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM dedup_clusters WHERE kind='sha256' AND key=?",
            ).bind(&sha).fetch_optional(pool).await?;
            let cluster_id = if let Some(c) = existing { c } else {
                let c: i64 = sqlx::query_scalar(
                    "INSERT INTO dedup_clusters (kind, key, created) VALUES ('sha256', ?, ?) RETURNING id",
                ).bind(&sha).bind(now).fetch_one(pool).await?;
                clusters += 1;
                c
            };
            sqlx::query("INSERT OR IGNORE INTO dedup_members (cluster_id, item_id) VALUES (?, ?)")
                .bind(cluster_id).bind(id).execute(pool).await?;
        }
    }

    // pHash grouping — only for items without a SHA-cluster duplicate.
    for (id, path) in &items {
        // skip if this item already shares a SHA cluster with another
        let in_sha_dup: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dedup_members WHERE cluster_id IN
             (SELECT cluster_id FROM dedup_members WHERE item_id = ?)
             AND item_id <> ?",
        ).bind(id).bind(id).fetch_one(pool).await?;
        if in_sha_dup > 0 { continue; }

        if let Ok(ph) = phash_file(Path::new(path)) {
            sqlx::query("UPDATE photo_meta SET phash = ? WHERE item_id = ?")
                .bind(&ph).bind(id).execute(pool).await?;
            // try to place into an existing pHash cluster within radius
            let candidates: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, key FROM dedup_clusters WHERE kind = 'phash'",
            ).fetch_all(pool).await?;
            let mut placed = false;
            for (cluster_id, key) in candidates {
                if hamming(&ph, &key).map(|d| d <= PHASH_RADIUS).unwrap_or(false) {
                    sqlx::query("INSERT OR IGNORE INTO dedup_members (cluster_id, item_id) VALUES (?, ?)")
                        .bind(cluster_id).bind(id).execute(pool).await?;
                    placed = true;
                    break;
                }
            }
            if !placed {
                let c: i64 = sqlx::query_scalar(
                    "INSERT INTO dedup_clusters (kind, key, created) VALUES ('phash', ?, ?) RETURNING id",
                ).bind(&ph).bind(now).fetch_one(pool).await?;
                clusters += 1;
                sqlx::query("INSERT INTO dedup_members (cluster_id, item_id) VALUES (?, ?)")
                    .bind(c).bind(id).execute(pool).await?;
            }
        }
    }
    // Drop singleton clusters (those don't represent duplicates).
    sqlx::query("DELETE FROM dedup_clusters WHERE id IN (
                    SELECT cluster_id FROM dedup_members GROUP BY cluster_id HAVING COUNT(*) < 2)")
        .execute(pool).await?;
    Ok(clusters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn hamming_matches_known_pairs() {
        assert_eq!(hamming("00", "00"), Some(0));
        assert_eq!(hamming("0f", "00"), Some(4));
        assert_eq!(hamming("ff", "00"), Some(8));
        assert_eq!(hamming("ff", "ffff"), None);
    }
    #[test]
    fn sha_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let h = sha256_file(&p).unwrap();
        assert_eq!(h, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }
    #[tokio::test]
    async fn cluster_dup_files() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.bin");
        let b = tmp.path().join("b.bin");
        std::fs::write(&a, b"same").unwrap();
        std::fs::write(&b, b"same").unwrap();
        for p in [&a, &b] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 4, 0, 'photos', 0, 0)")
                .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        }
        let _ = build_clusters(&pool).await.unwrap();
        let dups: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dedup_clusters WHERE kind='sha256'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(dups, 1);
    }
}
