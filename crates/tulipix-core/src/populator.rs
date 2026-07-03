//! Lazy populator for watched library locations.
//!
//! First-nav into a section triggers a walk of every registered Library for
//! that section: every file that survives the include/exclude glob filter
//! becomes a proxy row in `items`. Re-running the populator is idempotent —
//! existing rows are refreshed in place, vanished rows get `missing_since`.

use crate::fs::glob_match;
use crate::libraries::{LibrariesConfig, Library, Section};
use anyhow::Result;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;
use crate::util::unix_secs_i64 as now_secs;

pub const DEFAULT_EXCLUDES: &[&str] = &[
    "*/.git/*",
    "*/.DS_Store",
    "*/Thumbs.db",
    "*/__pycache__/*",
    "*/node_modules/*",
    "*/.cache/*",
];

#[derive(Debug, Clone, Copy, Default)]
pub struct PopulateStats {
    pub scanned: u64,
    pub inserted: u64,
    pub updated: u64,
    pub missing: u64,
    pub excluded: u64,
}

fn excluded(path: &Path, lib_globs: &[String]) -> bool {
    let p = path.to_string_lossy();
    for g in DEFAULT_EXCLUDES { if glob_match(g, &p) { return true; } }
    for g in lib_globs { if glob_match(g, &p) { return true; } }
    false
}

fn section_str(s: Section) -> &'static str {
    match s {
        Section::Photos => "photos",
        Section::Videos => "videos",
        Section::Music  => "music",
        Section::Books  => "books",
        Section::Cloud  => "cloud",
    }
}

#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino() as i64
}
#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> i64 { 0 }

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Populate one library into its section DB. Sweeps every file under
/// `lib.path`, applies excludes, upserts proxy rows. Files that were in the DB
/// but no longer on disk are marked with `missing_since`.
pub async fn populate(pool: &SqlitePool, lib: &Library) -> Result<PopulateStats> {
    let mut stats = PopulateStats::default();
    let section = section_str(lib.section);
    let now = now_secs();
    let mut seen: Vec<String> = Vec::new();

    for entry in WalkDir::new(&lib.path).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        stats.scanned += 1;
        let path = entry.path();
        if excluded(path, &lib.exclude_globs) { stats.excluded += 1; continue; }
        let Ok(meta) = entry.metadata() else { continue; };
        let abs = path.to_string_lossy().into_owned();
        seen.push(abs.clone());
        let inode = inode_of(&meta);
        let size = meta.len() as i64;
        let mtime = mtime_of(&meta);

        let existing: Option<(i64, i64, i64)> = sqlx::query_as(
            "SELECT id, size, mtime FROM items WHERE abs_path = ?",
        )
        .bind(&abs)
        .fetch_optional(pool)
        .await?;

        if let Some((id, prev_size, prev_mtime)) = existing {
            if prev_size != size || prev_mtime != mtime {
                sqlx::query(
                    "UPDATE items SET inode = ?, size = ?, mtime = ?, missing_since = NULL, updated = ? WHERE id = ?",
                )
                .bind(inode).bind(size).bind(mtime).bind(now).bind(id)
                .execute(pool).await?;
                stats.updated += 1;
            } else {
                sqlx::query("UPDATE items SET missing_since = NULL WHERE id = ?")
                    .bind(id).execute(pool).await?;
            }
        } else {
            sqlx::query(
                "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&abs).bind(inode).bind(size).bind(mtime)
            .bind(section).bind(now).bind(now)
            .execute(pool).await?;
            stats.inserted += 1;
        }
    }

    // Mark removed rows missing (only those rooted under this library).
    let prefix = format!("{}%", lib.path.to_string_lossy());
    let stale: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, abs_path FROM items WHERE section = ? AND missing_since IS NULL AND abs_path LIKE ?",
    )
    .bind(section)
    .bind(&prefix)
    .fetch_all(pool)
    .await?;
    let seen_set: std::collections::HashSet<_> = seen.into_iter().collect();
    for (id, path) in stale {
        if !seen_set.contains(&path) {
            sqlx::query("UPDATE items SET missing_since = ?, updated = ? WHERE id = ?")
                .bind(now).bind(now).bind(id).execute(pool).await?;
            stats.missing += 1;
        }
    }

    Ok(stats)
}

/// Populate every library belonging to `section` via its dedicated pool.
pub async fn populate_section(
    pool: &SqlitePool,
    cfg: &LibrariesConfig,
    section: Section,
) -> Result<PopulateStats> {
    let mut total = PopulateStats::default();
    for lib in cfg.libraries.iter().filter(|l| l.section == section) {
        let s = populate(pool, lib).await?;
        total.scanned += s.scanned;
        total.inserted += s.inserted;
        total.updated += s.updated;
        total.missing += s.missing;
        total.excluded += s.excluded;
    }
    Ok(total)
}

/// Find candidate relink targets by sha256 — picks rows that share the same
/// content hash as the orphan but live at a different path.
pub async fn relink_candidates_by_hash(pool: &SqlitePool, sha256: &str) -> Result<Vec<PathBuf>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT abs_path FROM items WHERE sha256 = ? AND missing_since IS NULL",
    )
    .bind(sha256)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(p,)| PathBuf::from(p)).collect())
}

/// Apply a manual relink: update the orphan row's `abs_path` and clear missing.
pub async fn relink(pool: &SqlitePool, id: i64, new_path: &Path) -> Result<()> {
    let abs = new_path.to_string_lossy().into_owned();
    let meta = std::fs::metadata(new_path)?;
    let inode = inode_of(&meta);
    let size = meta.len() as i64;
    let mtime = mtime_of(&meta);
    let now = now_secs();
    sqlx::query(
        "UPDATE items SET abs_path = ?, inode = ?, size = ?, mtime = ?, missing_since = NULL, updated = ? WHERE id = ?",
    )
    .bind(&abs).bind(inode).bind(size).bind(mtime).bind(now).bind(id)
    .execute(pool).await?;
    Ok(())
}

/// Drop an orphan row from the index (does not touch any file on disk).
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM items WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_proxy_schema, DbHandle};
    use crate::libraries::{Library, ScanCadence};

    async fn open_test_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("photos.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let handle = DbHandle { section: "photos".into(), path, url };
        let pool = handle.pool().await.unwrap();
        apply_proxy_schema(&pool, "photos").await.unwrap();
        (tmp, pool)
    }

    fn mk_lib(path: PathBuf) -> Library {
        Library {
            id: "lib1".into(),
            path,
            section: Section::Photos,
            last_scan: None,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: vec![],
            cadence_override: Some(ScanCadence::Manual),
            realtime_notify: true,
        }
    }

    #[tokio::test]
    async fn populator_inserts_then_idempotent_then_misses() {
        let (_tmp, pool) = open_test_pool().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.jpg"), b"hello").unwrap();
        std::fs::write(root.path().join("b.png"), b"world").unwrap();
        let lib = mk_lib(root.path().to_path_buf());

        let s1 = populate(&pool, &lib).await.unwrap();
        assert_eq!(s1.inserted, 2);
        assert_eq!(s1.missing, 0);

        let s2 = populate(&pool, &lib).await.unwrap();
        assert_eq!(s2.inserted, 0);
        assert_eq!(s2.missing, 0);

        std::fs::remove_file(root.path().join("a.jpg")).unwrap();
        let s3 = populate(&pool, &lib).await.unwrap();
        assert_eq!(s3.missing, 1);
    }

    #[tokio::test]
    async fn excludes_filter_paths() {
        let (_tmp, pool) = open_test_pool().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join(".git").join("HEAD"), b"x").unwrap();
        std::fs::write(root.path().join("ok.jpg"), b"y").unwrap();
        let lib = mk_lib(root.path().to_path_buf());

        let s = populate(&pool, &lib).await.unwrap();
        assert_eq!(s.inserted, 1);
        assert!(s.excluded >= 1);
    }
}
