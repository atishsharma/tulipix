//! SCRFD face detection + 160×160 crop pipeline.
//!
//! Two pieces:
//!   1. A `FaceDetector` trait — production binds to ONNX Runtime + SCRFD,
//!      a stub `NullDetector` is used until the model is downloaded. The
//!      trait keeps the rest of the pipeline testable without ML deps.
//!   2. `extract_crops` — given a detector + an image, runs detection,
//!      pads each bounding box by 25%, resizes to 160×160, and writes
//!      `face_thumbs/<item_id>/<face_id>.jpg`.
//!
//! `CROP_SIZE` is 160 because that is a decent face thumbnail for the People
//! tab — NOT because it is the recogniser's input, which this file used to
//! claim. ArcFace/buffalo_s takes 112×112, verified against the installed
//! blob, and `OrtFaceEmbedder` resizes on the way in.

use anyhow::{Context, Result};
use image::{imageops::FilterType, GenericImageView, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use tulipix_core::paths;

pub const CROP_SIZE: u32 = 160;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BBox {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub score: f32,
}

pub trait FaceDetector: Send + Sync {
    fn detect(&self, img: &image::DynamicImage) -> Result<Vec<BBox>>;
}

/// No-op detector — emits zero faces. Used when the SCRFD model is not
/// installed (e.g. local-basic tier) or in unit tests.
pub struct NullDetector;
impl FaceDetector for NullDetector {
    fn detect(&self, _img: &image::DynamicImage) -> Result<Vec<BBox>> { Ok(Vec::new()) }
}

/// Turns a 160×160 face crop into the 512-d vector clustering compares.
///
/// Separate from `FaceDetector` because they are two different models —
/// detection finds the boxes, recognition describes the face — and installing
/// one without the other is a normal state: boxes and crops with no embedding
/// yet. `face_clusters::load_embeddings` skips those rows, so the People tab
/// simply stays empty rather than clustering on nothing.
pub trait FaceEmbedder: Send + Sync {
    fn embed(&self, crop: &image::DynamicImage) -> Result<Vec<f32>>;
    /// Name+version of the weights, recorded so a model change can requeue.
    fn model_name(&self) -> &str;
}

/// No recognition model installed — errors rather than returning a zero vector,
/// which would cluster every face in the library into one person.
pub struct NullFaceEmbedder;
impl FaceEmbedder for NullFaceEmbedder {
    fn embed(&self, _: &image::DynamicImage) -> Result<Vec<f32>> {
        anyhow::bail!("no face-recognition model installed")
    }
    fn model_name(&self) -> &str { "none" }
}

pub fn face_thumbs_dir() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("face_thumbs"))
}

/// Fill `faces.embedding` for crops that do not have one, up to `limit` rows.
/// Returns how many were written.
///
/// `extract_crops` writes the box and the crop but leaves `embedding` NULL —
/// it runs the detector, not the recogniser. This is the second half, and
/// without it `faces` fills up while `load_embeddings` returns nothing and no
/// cluster is ever formed.
pub async fn embed_pending(
    pool: &SqlitePool,
    embedder: &dyn FaceEmbedder,
    limit: i64,
) -> Result<u32> {
    let root = face_thumbs_dir().context("no cache dir")?;
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, crop_path FROM faces
         WHERE embedding IS NULL AND crop_path <> ''
         ORDER BY id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut written = 0u32;
    for (face_id, rel) in rows {
        let path = root.join(&rel);
        let Ok(img) = ImageReader::open(&path).and_then(|r| r.with_guessed_format()) else {
            continue;
        };
        let Ok(img) = img.decode() else { continue };
        let vec = match embedder.embed(&img) {
            Ok(v) => v,
            // A model that cannot embed at all is the caller's problem to
            // report; stop rather than walk the whole table failing.
            Err(e) => return Err(e).with_context(|| format!("embed face {face_id}")),
        };
        if vec.len() != crate::ai::face_clusters::EMBED_DIM {
            anyhow::bail!("embedder returned {} dims, want {}", vec.len(), crate::ai::face_clusters::EMBED_DIM);
        }
        let blob: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
        sqlx::query("UPDATE faces SET embedding = ? WHERE id = ?")
            .bind(blob)
            .bind(face_id)
            .execute(pool)
            .await?;
        written += 1;
    }
    Ok(written)
}

/// Pad a bounding box outward by `factor` (e.g. 0.25 for +25%), clamped to
/// the original image dimensions. SCRFD outputs tight boxes — ArcFace needs
/// some forehead/chin context.
pub fn pad(bbox: &BBox, factor: f32, img_w: u32, img_h: u32) -> BBox {
    let dx = (bbox.w as f32 * factor) as i32;
    let dy = (bbox.h as f32 * factor) as i32;
    let x0 = (bbox.x - dx).max(0);
    let y0 = (bbox.y - dy).max(0);
    let x1 = ((bbox.x + bbox.w as i32) + dx).min(img_w as i32);
    let y1 = ((bbox.y + bbox.h as i32) + dy).min(img_h as i32);
    BBox {
        x: x0, y: y0,
        w: (x1 - x0).max(1) as u32,
        h: (y1 - y0).max(1) as u32,
        score: bbox.score,
    }
}

/// Detect faces on `path`, write 160×160 crops, and insert one `faces` row
/// per detection. Returns the new face IDs.
pub async fn extract_crops(
    pool: &SqlitePool,
    item_id: i64,
    path: &Path,
    detector: &dyn FaceDetector,
) -> Result<Vec<i64>> {
    let img = ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let (w, h) = img.dimensions();
    let boxes = detector.detect(&img)?;
    let dir = face_thumbs_dir().context("no cache dir")?.join(format!("{item_id}"));
    if !boxes.is_empty() { std::fs::create_dir_all(&dir)?; }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut face_ids = Vec::with_capacity(boxes.len());
    for b in boxes {
        let padded = pad(&b, 0.25, w, h);
        let crop = img.crop_imm(padded.x as u32, padded.y as u32, padded.w, padded.h);
        let small = crop.resize_exact(CROP_SIZE, CROP_SIZE, FilterType::Triangle);
        let face_id: i64 = sqlx::query_scalar(
            "INSERT INTO faces (item_id, person_id, bbox_x, bbox_y, bbox_w, bbox_h, confidence, crop_path, confirmed, created)
             VALUES (?, NULL, ?, ?, ?, ?, ?, '', 0, ?) RETURNING id",
        )
        .bind(item_id)
        .bind(b.x).bind(b.y).bind(b.w).bind(b.h)
        .bind(b.score)
        .bind(now)
        .fetch_one(pool).await?;
        let out_path = dir.join(format!("{face_id}.jpg"));
        small.save_with_format(&out_path, ImageFormat::Jpeg)?;
        let rel = out_path.strip_prefix(face_thumbs_dir().unwrap()).unwrap_or(&out_path);
        sqlx::query("UPDATE faces SET crop_path = ? WHERE id = ?")
            .bind(rel.to_string_lossy().into_owned()).bind(face_id)
            .execute(pool).await?;
        face_ids.push(face_id);
    }
    Ok(face_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    /// Embeds to a fixed direction so clustering behaviour is deterministic.
    struct FixedEmbedder(f32);
    impl FaceEmbedder for FixedEmbedder {
        fn embed(&self, _: &image::DynamicImage) -> Result<Vec<f32>> {
            let mut v = vec![0.0f32; crate::ai::face_clusters::EMBED_DIM];
            v[0] = self.0;
            v[1] = 1.0 - self.0;
            Ok(v)
        }
        fn model_name(&self) -> &str { "fixed" }
    }

    #[tokio::test]
    async fn embed_pending_fills_the_column_clustering_reads() {
        let (_t, pool) = open_pool().await;
        unsafe { std::env::set_var("XDG_CACHE_HOME", tempfile::tempdir().unwrap().keep()); }
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("face.jpg");
        image::RgbImage::from_pixel(200, 200, image::Rgb([200, 100, 50])).save(&p).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(p.to_string_lossy().as_ref()).fetch_one(&pool).await.unwrap();
        let det = FixedDetector(vec![BBox { x: 40, y: 40, w: 80, h: 80, score: 0.99 }]);
        extract_crops(&pool, id, &p, &det).await.unwrap();

        // Detection alone leaves nothing for the clusterer to read.
        assert!(crate::ai::face_clusters::load_embeddings(&pool).await.unwrap().is_empty());

        let n = embed_pending(&pool, &FixedEmbedder(1.0), 100).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(crate::ai::face_clusters::load_embeddings(&pool).await.unwrap().len(), 1);

        // Idempotent: a second pass finds nothing still NULL.
        assert_eq!(embed_pending(&pool, &FixedEmbedder(1.0), 100).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn null_embedder_refuses_rather_than_returning_zeros() {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(160, 160));
        // A zero vector would make every face in the library one person.
        assert!(NullFaceEmbedder.embed(&img).is_err());
    }

    struct FixedDetector(Vec<BBox>);
    impl FaceDetector for FixedDetector {
        fn detect(&self, _: &image::DynamicImage) -> Result<Vec<BBox>> { Ok(self.0.clone()) }
    }

    #[test]
    fn pad_clamps_to_image_bounds() {
        let b = BBox { x: 0, y: 0, w: 10, h: 10, score: 0.9 };
        let p = pad(&b, 0.5, 12, 12);
        assert_eq!(p.x, 0);
        assert_eq!(p.y, 0);
        assert!(p.w <= 12);
        assert!(p.h <= 12);
    }

    #[tokio::test]
    async fn null_detector_produces_no_rows() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.jpg");
        image::RgbImage::from_pixel(32, 32, image::Rgb([0, 0, 0])).save(&p).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p.to_string_lossy().as_ref()).fetch_one(&pool).await.unwrap();
        let ids = extract_crops(&pool, id, &p, &NullDetector).await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn fixed_detector_writes_crops() {
        let (_t, pool) = open_pool().await;
        unsafe { std::env::set_var("XDG_CACHE_HOME", tempfile::tempdir().unwrap().keep()); }
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("face.jpg");
        image::RgbImage::from_pixel(200, 200, image::Rgb([200, 100, 50])).save(&p).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p.to_string_lossy().as_ref()).fetch_one(&pool).await.unwrap();
        let det = FixedDetector(vec![BBox { x: 40, y: 40, w: 80, h: 80, score: 0.99 }]);
        let ids = extract_crops(&pool, id, &p, &det).await.unwrap();
        assert_eq!(ids.len(), 1);
        let crop_path: String = sqlx::query_scalar("SELECT crop_path FROM faces WHERE id = ?").bind(ids[0]).fetch_one(&pool).await.unwrap();
        assert!(crop_path.ends_with(".jpg"));
    }
}
