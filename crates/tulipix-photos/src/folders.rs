//! Folder-tree browse view.
//!
//! 1:1 mirror of the on-disk hierarchy under every watched library. Cover for
//! a folder is the first photo (by `taken_at`, else by mtime). Breadcrumb nav
//! is derived from `abs_path` so no extra table is needed.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    pub child_count: i64,
    pub photo_count: i64,
    pub cover_item_id: Option<i64>,
    pub cover_abs_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breadcrumb {
    pub label: String,
    pub path:  PathBuf,
}

/// Direct children of `root`. Both subfolders and photos in that folder are
/// counted, but only subfolders are returned as nodes (photos belong to the
/// page view, not the tree view).
pub async fn children(pool: &SqlitePool, root: &Path) -> Result<Vec<FolderNode>> {
    let root_str = root.to_string_lossy().into_owned();
    let prefix = if root_str.ends_with('/') { root_str.clone() } else { format!("{root_str}/") };
    let like = format!("{prefix}%");

    // Pull every descendant photo path, bucket by immediate child dirname.
    let rows: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.taken_at
         FROM items
         LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.missing_since IS NULL
           AND items.section = 'photos'
           AND items.abs_path LIKE ?",
    )
    .bind(&like)
    .fetch_all(pool)
    .await?;

    let mut buckets: BTreeMap<String, FolderBucket> = BTreeMap::new();
    for (id, abs, taken) in rows {
        let rel = abs.strip_prefix(&prefix).unwrap_or(&abs);
        let head = rel.split('/').next().unwrap_or("").to_string();
        if head.is_empty() || head == rel {
            // file directly in root — skip (those aren't subfolders)
            continue;
        }
        let bucket = buckets.entry(head.clone()).or_default();
        bucket.photo_count += 1;
        let rank = taken.unwrap_or(i64::MAX);
        if bucket.cover_rank.map(|r| rank < r).unwrap_or(true) {
            bucket.cover_rank = Some(rank);
            bucket.cover_id   = Some(id);
            bucket.cover_path = Some(abs.clone());
        }
    }
    Ok(buckets.into_iter().map(|(name, b)| FolderNode {
        path: PathBuf::from(prefix.clone()).join(&name),
        name,
        child_count: 0,                 // populated lazily on expand
        photo_count: b.photo_count,
        cover_item_id: b.cover_id,
        cover_abs_path: b.cover_path,
    }).collect())
}

#[derive(Default)]
struct FolderBucket {
    photo_count: i64,
    cover_id:    Option<i64>,
    cover_path:  Option<String>,
    cover_rank:  Option<i64>,
}

/// Breadcrumb from a library root to a target subpath. The first entry is the
/// root, then each subdir up to and including `target`.
pub fn breadcrumb(library_root: &Path, target: &Path) -> Vec<Breadcrumb> {
    let mut out = Vec::new();
    out.push(Breadcrumb {
        label: library_root.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        path:  library_root.to_path_buf(),
    });
    let Ok(rel) = target.strip_prefix(library_root) else { return out; };
    let mut acc = library_root.to_path_buf();
    for part in rel.iter() {
        acc.push(part);
        out.push(Breadcrumb {
            label: part.to_string_lossy().into_owned(),
            path:  acc.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn children_groups_by_first_segment() {
        let (_t, pool) = open_pool().await;
        for p in ["/root/2024/Italy/a.jpg", "/root/2024/Italy/b.jpg", "/root/2023/x.jpg", "/root/loose.jpg"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(p).execute(&pool).await.unwrap();
        }
        let kids = children(&pool, Path::new("/root")).await.unwrap();
        let names: Vec<_> = kids.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"2024"));
        assert!(names.contains(&"2023"));
        assert!(!names.contains(&"loose.jpg")); // loose files aren't subfolders
        let italy = kids.iter().find(|n| n.name == "2024").unwrap();
        assert_eq!(italy.photo_count, 2);
    }

    #[test]
    fn breadcrumb_walks_segments() {
        let crumbs = breadcrumb(Path::new("/lib"), Path::new("/lib/2024/Italy/Rome"));
        let labels: Vec<_> = crumbs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["lib", "2024", "Italy", "Rome"]);
    }
}
