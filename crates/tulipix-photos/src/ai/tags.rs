//! YOLOv8 COCO-80 tagging.
//!
//! Production binds to ONNX Runtime with the YOLOv8n weight blob from the
//! model registry (`tulipix_core::ai_models::Manifest`). This module keeps
//! the contract — a `Tagger` trait, a COCO-80 label table, and an
//! `ingest_predictions` writer — so the rest of the section ships even
//! without ML deps compiled in.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path;

/// COCO-80 class labels in canonical YOLO output order.
pub const COCO80: [&str; 80] = [
    "person","bicycle","car","motorcycle","airplane","bus","train","truck","boat","traffic light",
    "fire hydrant","stop sign","parking meter","bench","bird","cat","dog","horse","sheep","cow",
    "elephant","bear","zebra","giraffe","backpack","umbrella","handbag","tie","suitcase","frisbee",
    "skis","snowboard","sports ball","kite","baseball bat","baseball glove","skateboard","surfboard","tennis racket","bottle",
    "wine glass","cup","fork","knife","spoon","bowl","banana","apple","sandwich","orange",
    "broccoli","carrot","hot dog","pizza","donut","cake","chair","couch","potted plant","bed",
    "dining table","toilet","tv","laptop","mouse","remote","keyboard","cell phone","microwave","oven",
    "toaster","sink","refrigerator","book","clock","vase","scissors","teddy bear","hair drier","toothbrush",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Detection {
    pub class: usize,        // index into COCO80
    pub confidence: f32,
    pub bbox: [f32; 4],      // x,y,w,h, normalised 0..1
}

pub fn label(class: usize) -> Option<&'static str> {
    COCO80.get(class).copied()
}

pub trait Tagger: Send + Sync {
    fn predict(&self, img: &image::DynamicImage) -> Result<Vec<Detection>>;

    /// Provenance written to `item_tags.source`, so a tag can be traced to what
    /// produced it and one detector's output can be dropped without touching
    /// the user's own tags.
    ///
    /// Defaulted rather than required: every COCO-80 detector fits `predict`,
    /// and the column used to be hardcoded to `'yolov8'` by a module that has
    /// never run a detector of any kind.
    fn source(&self) -> &str { "object-detect" }
}

/// No-op tagger — emits zero detections. Used when YOLOv8 weights are not
/// installed (Settings → AI Models row is greyed out) or in tests.
pub struct NullTagger;
impl Tagger for NullTagger {
    fn predict(&self, _: &image::DynamicImage) -> Result<Vec<Detection>> { Ok(Vec::new()) }
}

/// Run `tagger` on `path`, insert `item_tags` rows with `source = 'yolov8'`.
/// De-dupes per (item, class): the highest-confidence detection wins.
pub async fn ingest_predictions(
    pool: &SqlitePool,
    item_id: i64,
    path: &Path,
    tagger: &dyn Tagger,
    min_confidence: f32,
) -> Result<u32> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    let dets = tagger.predict(&img)?;
    let mut best: std::collections::BTreeMap<usize, f32> = std::collections::BTreeMap::new();
    for d in dets {
        if d.confidence < min_confidence { continue; }
        let cur = best.get(&d.class).copied().unwrap_or(0.0);
        if d.confidence > cur { best.insert(d.class, d.confidence); }
    }
    let mut inserted = 0u32;
    for (class, conf) in best {
        let Some(name) = label(class) else { continue; };
        // Ensure tag exists.
        let tag_id: i64 = sqlx::query_scalar(
            "INSERT INTO tags (name) VALUES (?) ON CONFLICT(name) DO UPDATE SET name = name RETURNING id",
        ).bind(name).fetch_one(pool).await?;
        let r = sqlx::query(
            "INSERT INTO item_tags (item_id, tag_id, confidence, source) VALUES (?, ?, ?, ?)
             ON CONFLICT(item_id, tag_id) DO UPDATE SET confidence = excluded.confidence WHERE excluded.confidence > item_tags.confidence",
        )
        .bind(item_id).bind(tag_id).bind(conf as f64).bind(tagger.source())
        .execute(pool).await?;
        inserted += r.rows_affected() as u32;
    }
    Ok(inserted)
}

/// Aggregate tags for the Things tab — every tag whose item count > 0, with
/// the count.
pub async fn things(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT t.name, COUNT(it.item_id) AS c
         FROM tags t
         JOIN item_tags it ON it.tag_id = t.id
         JOIN items i ON i.id = it.item_id
         JOIN photo_meta pm ON pm.item_id = i.id
         WHERE i.missing_since IS NULL AND pm.deleted_at IS NULL AND pm.archived = 0
         GROUP BY t.id HAVING c > 0
         ORDER BY c DESC",
    ).fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    struct FixedTagger(Vec<Detection>);
    impl Tagger for FixedTagger { fn predict(&self, _: &image::DynamicImage) -> Result<Vec<Detection>> { Ok(self.0.clone()) } }

    #[test]
    fn coco_table_has_80_entries() {
        assert_eq!(COCO80.len(), 80);
        assert_eq!(label(0), Some("person"));
        assert_eq!(label(79), Some("toothbrush"));
        assert_eq!(label(80), None);
    }

    #[tokio::test]
    async fn ingest_writes_distinct_tags() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.jpg");
        image::RgbImage::from_pixel(32, 32, image::Rgb([0, 0, 0])).save(&p).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)").bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        let t = FixedTagger(vec![
            Detection { class: 16, confidence: 0.8, bbox: [0.0; 4] }, // dog
            Detection { class: 16, confidence: 0.5, bbox: [0.0; 4] }, // dog (lower) — dropped
            Detection { class: 17, confidence: 0.9, bbox: [0.0; 4] }, // horse
            Detection { class: 0,  confidence: 0.1, bbox: [0.0; 4] }, // below threshold
        ]);
        let n = ingest_predictions(&pool, id, &p, &t, 0.2).await.unwrap();
        assert!(n >= 2);
        let things = things(&pool).await.unwrap();
        let names: Vec<_> = things.iter().map(|(n,_)| n.as_str()).collect();
        assert!(names.contains(&"dog"));
        assert!(names.contains(&"horse"));
        assert!(!names.contains(&"person"));
    }
}
