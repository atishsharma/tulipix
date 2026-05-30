//! Agglomerative cosine-distance clustering over face embeddings.
//!
//! Embeddings are 512-d fp32 (ArcFace output). Distance metric is `1 -
//! cosine_similarity`. The clusterer is an O(N²) agglomerative pass with a
//! distance threshold (default 0.45) — fine for the per-user face counts we
//! see (low thousands). Above 5k faces the indexer batches and re-runs only
//! over new + unconfirmed.

use anyhow::Result;
use sqlx::SqlitePool;

pub const DEFAULT_THRESHOLD: f32 = 0.45;
pub const EMBED_DIM: usize = 512;

#[derive(Debug, Clone)]
pub struct FaceEmbedding {
    pub face_id: i64,
    pub vec: Vec<f32>,
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na  += a[i] * a[i];
        nb  += b[i] * b[i];
    }
    let denom = (na.sqrt() * nb.sqrt()).max(f32::MIN_POSITIVE);
    dot / denom
}

pub fn distance(a: &[f32], b: &[f32]) -> f32 {
    1.0 - cosine(a, b)
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub members: Vec<i64>,
    pub centroid: Vec<f32>,
}

impl Cluster {
    fn new(face_id: i64, vec: Vec<f32>) -> Self {
        Self { members: vec![face_id], centroid: vec }
    }
    fn merge(&mut self, other: Cluster) {
        let total = self.members.len() + other.members.len();
        let a_w = self.members.len() as f32 / total as f32;
        let b_w = other.members.len() as f32 / total as f32;
        for i in 0..self.centroid.len() {
            self.centroid[i] = self.centroid[i] * a_w + other.centroid[i] * b_w;
        }
        self.members.extend(other.members);
    }
}

/// Agglomerative single-linkage clustering. Returns a list of `Cluster`s.
pub fn cluster(embeddings: Vec<FaceEmbedding>, threshold: f32) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = embeddings
        .into_iter()
        .map(|e| Cluster::new(e.face_id, e.vec))
        .collect();
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let d = distance(&clusters[i].centroid, &clusters[j].centroid);
                if best.map(|(_, _, bd)| d < bd).unwrap_or(true) {
                    best = Some((i, j, d));
                }
            }
        }
        match best {
            Some((i, j, d)) if d <= threshold => {
                let other = clusters.remove(j);
                clusters[i].merge(other);
            }
            _ => break,
        }
    }
    clusters
}

/// Read all face embeddings from the DB. Faces without an embedding blob are
/// skipped (no model installed yet → no clustering input).
pub async fn load_embeddings(pool: &SqlitePool) -> Result<Vec<FaceEmbedding>> {
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT id, embedding FROM faces WHERE embedding IS NOT NULL",
    ).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, bytes) in rows {
        if bytes.len() != EMBED_DIM * 4 { continue; }
        let mut vec = Vec::with_capacity(EMBED_DIM);
        for chunk in bytes.chunks_exact(4) {
            vec.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        out.push(FaceEmbedding { face_id: id, vec });
    }
    Ok(out)
}

/// Persist a clustering — creates one `people` row per cluster, then writes
/// `faces.person_id` for each member. Existing people rows are reused when
/// the cluster contains an already-named face.
pub async fn persist_clusters(pool: &SqlitePool, clusters: &[Cluster]) -> Result<u64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut affected = 0u64;
    for c in clusters {
        // Reuse an existing person if any member already has one assigned.
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT person_id FROM faces WHERE id IN (
                SELECT value FROM json_each(?)
             ) AND person_id IS NOT NULL LIMIT 1",
        )
        .bind(serde_json::to_string(&c.members)?)
        .fetch_optional(pool).await?;

        let pid = match existing {
            Some(p) => p,
            None => sqlx::query_scalar::<_, i64>(
                "INSERT INTO people (created, updated) VALUES (?, ?) RETURNING id",
            ).bind(now).bind(now).fetch_one(pool).await?,
        };
        for face_id in &c.members {
            let r = sqlx::query("UPDATE faces SET person_id = ? WHERE id = ?")
                .bind(pid).bind(face_id).execute(pool).await?;
            affected += r.rows_affected();
        }
    }
    Ok(affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn norm(v: &mut [f32]) {
        let n: f32 = v.iter().map(|x| x*x).sum::<f32>().sqrt();
        for x in v { *x /= n; }
    }

    #[test]
    fn cosine_identical_is_one() {
        let mut a = vec![1.0, 0.0, 0.0];
        let mut b = vec![1.0, 0.0, 0.0];
        norm(&mut a); norm(&mut b);
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cluster_merges_near_vectors() {
        let mut a = vec![1.0, 0.05, 0.0];
        let mut b = vec![0.95, 0.0, 0.05];
        let mut c = vec![0.0, 0.0, 1.0];
        norm(&mut a); norm(&mut b); norm(&mut c);
        let embeds = vec![
            FaceEmbedding { face_id: 1, vec: a },
            FaceEmbedding { face_id: 2, vec: b },
            FaceEmbedding { face_id: 3, vec: c },
        ];
        let cs = cluster(embeds, 0.3);
        assert_eq!(cs.len(), 2);
        let large = cs.iter().find(|x| x.members.len() == 2).unwrap();
        let small = cs.iter().find(|x| x.members.len() == 1).unwrap();
        assert!(large.members.contains(&1) && large.members.contains(&2));
        assert!(small.members.contains(&3));
    }

    #[tokio::test]
    async fn persist_creates_people() {
        let (_t, pool) = open_pool().await;
        // seed two faces in a single cluster
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path='/p.jpg'").fetch_one(&pool).await.unwrap();
        for _ in 0..2 {
            sqlx::query("INSERT INTO faces (item_id, bbox_x, bbox_y, bbox_w, bbox_h, created) VALUES (?, 0, 0, 1, 1, 0)").bind(id).execute(&pool).await.unwrap();
        }
        let face_ids: Vec<(i64,)> = sqlx::query_as("SELECT id FROM faces").fetch_all(&pool).await.unwrap();
        let cluster = Cluster {
            members: face_ids.iter().map(|x| x.0).collect(),
            centroid: vec![1.0, 0.0],
        };
        let _ = persist_clusters(&pool, &[cluster]).await.unwrap();
        let people: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM people").fetch_one(&pool).await.unwrap();
        assert_eq!(people, 1);
    }
}
