//! Duplicate detection.
//!
//! Two stages:
//!  * SHA-256 — identifies byte-identical duplicates. Cheap, exact, but
//!    misses re-encodes/format-converts.
//!  * pHash (perceptual) — 64-bit DCT-II hash. 32×32 luma → 2-D DCT → keep the
//!    low-frequency top-left 8×8 block → bit = coefficient > median (DC term
//!    excluded from the median). A new photo joins an existing pHash cluster
//!    only if it is within `PHASH_RADIUS` Hamming of *every* member (no greedy
//!    chaining, so A~B + B~C can't drag unrelated A and C into one pile).
//!
//! Both stages populate `dedup_clusters` + `dedup_members` so the UI can
//! show each cluster as a single row with a Keep/Drop picker.

use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageReader};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::io::Read;
use std::path::Path;

// 64-bit DCT pHash: near-duplicates land at distance 0–6, genuinely different
// photos almost always exceed 16. 8 keeps re-encodes/crops together while the
// all-members join rule (below) blocks chain-merges of dissimilar shots.
pub const PHASH_RADIUS: u32 = 8;

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

const DCT_N: usize = 32;

/// 1-D DCT-II of a length-32 signal (unnormalised — scaling is irrelevant since
/// we only threshold coefficients against their own median).
fn dct_1d(input: &[f32; DCT_N]) -> [f32; DCT_N] {
    let mut out = [0f32; DCT_N];
    for (k, ok) in out.iter_mut().enumerate() {
        let mut s = 0f32;
        for (n, &x) in input.iter().enumerate() {
            s += x * (std::f32::consts::PI / DCT_N as f32 * (n as f32 + 0.5) * k as f32).cos();
        }
        *ok = s;
    }
    out
}

fn phash_bytes(luma: &image::GrayImage, _w: u32, _h: u32) -> [u8; 8] {
    // 32×32 luma → real separable 2-D DCT-II.
    let small = image::imageops::resize(luma, DCT_N as u32, DCT_N as u32, FilterType::Triangle);
    let mut f = [[0f32; DCT_N]; DCT_N];
    for y in 0..DCT_N {
        for x in 0..DCT_N {
            f[y][x] = small.get_pixel(x as u32, y as u32)[0] as f32;
        }
    }
    // DCT over rows, then over columns.
    let mut rows = [[0f32; DCT_N]; DCT_N];
    for y in 0..DCT_N {
        rows[y] = dct_1d(&f[y]);
    }
    let mut dct = [[0f32; DCT_N]; DCT_N];
    for x in 0..DCT_N {
        let col: [f32; DCT_N] = std::array::from_fn(|y| rows[y][x]);
        let c = dct_1d(&col);
        for y in 0..DCT_N {
            dct[y][x] = c[y];
        }
    }
    // Low-frequency top-left 8×8 block (index 0 = DC term).
    let mut vals = [0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            vals[v * 8 + u] = dct[v][u];
        }
    }
    // Median of the 63 AC coefficients (exclude DC so overall brightness
    // doesn't dominate the threshold).
    let mut ac: [f32; 63] = std::array::from_fn(|i| vals[i + 1]);
    ac.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = ac[ac.len() / 2];
    let mut bits: u64 = 0;
    for (i, &c) in vals.iter().enumerate() {
        if c > median {
            bits |= 1 << i;
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

    // Full rebuild on every Scan: wipe prior clusters/hashes so a hash-algorithm
    // change (or moved/edited files) can't leave stale false-positive pairs.
    sqlx::query("DELETE FROM dedup_members").execute(pool).await?;
    sqlx::query("DELETE FROM dedup_clusters").execute(pool).await?;
    sqlx::query("UPDATE photo_meta SET phash = NULL").execute(pool).await?;

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
            // Join an existing pHash cluster only if within radius of EVERY
            // member (not just the seed key) — kills chain-merging of dissimilar
            // photos. Pull every member's stored phash grouped by cluster.
            let member_rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT dm.cluster_id, pm.phash \
                 FROM dedup_members dm \
                 JOIN photo_meta pm ON pm.item_id = dm.item_id \
                 JOIN dedup_clusters dc ON dc.id = dm.cluster_id \
                 WHERE dc.kind = 'phash' AND pm.phash IS NOT NULL AND dm.item_id <> ?",
            ).bind(id).fetch_all(pool).await?;
            let mut by_cluster: std::collections::BTreeMap<i64, Vec<String>> = Default::default();
            for (cid, h) in member_rows { by_cluster.entry(cid).or_default().push(h); }
            let mut placed = false;
            for (cluster_id, hashes) in &by_cluster {
                let all_close = hashes.iter().all(|k|
                    hamming(&ph, k).map(|d| d <= PHASH_RADIUS).unwrap_or(false));
                if all_close {
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
