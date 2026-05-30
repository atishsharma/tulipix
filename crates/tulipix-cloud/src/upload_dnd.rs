//! `np.p4.cloud.upload-dnd` — drag-and-drop upload to the remote tree.
//!
//! Files dropped from the native file manager queue as `uploads` rows and run
//! via `rclone copyto` (resumable: rclone resumes partial transfers). This owns
//! the dest-path mapping (drop onto a tree node → remote dest), the argv, and
//! progress bookkeeping the UI bar reads.

use anyhow::Result;
use sqlx::SqlitePool;
use std::path::Path;

/// Remote destination for a dropped file: `dest_dir` + the file's basename.
pub fn dest_for(dest_dir: &str, src: &str) -> String {
    let base = Path::new(src).file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let dir = dest_dir.trim_end_matches('/');
    if dir.is_empty() { base.to_string() } else { format!("{dir}/{base}") }
}

/// `rclone copyto src remote:dest` argv (resumable via `--retries`).
pub fn copy_args(src: &str, remote: &str, dest: &str) -> Vec<String> {
    vec![
        "copyto".into(), src.into(), format!("{remote}:{dest}"),
        "--retries".into(), "5".into(),
        "--low-level-retries".into(), "20".into(),
    ]
}

pub async fn enqueue(pool: &SqlitePool, remote_id: i64, src: &str, dest: &str, bytes_total: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO uploads (remote_id, src_path, dest_path, bytes_total, bytes_done, state) VALUES (?,?,?,?,0,'queued') RETURNING id",
    ).bind(remote_id).bind(src).bind(dest).bind(bytes_total).fetch_one(pool).await?)
}

pub async fn progress(pool: &SqlitePool, id: i64, bytes_done: i64) -> Result<()> {
    sqlx::query("UPDATE uploads SET bytes_done = ?, state = 'running' WHERE id = ?").bind(bytes_done).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn finish(pool: &SqlitePool, id: i64, ok: bool) -> Result<()> {
    sqlx::query("UPDATE uploads SET state = ? WHERE id = ?").bind(if ok { "done" } else { "error" }).bind(id).execute(pool).await?;
    Ok(())
}

/// Overall progress fraction across queued/running uploads (0..1).
pub async fn overall_fraction(pool: &SqlitePool) -> Result<f64> {
    let (done, total): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(bytes_done),0), COALESCE(SUM(bytes_total),0) FROM uploads WHERE state IN ('queued','running')",
    ).fetch_one(pool).await?;
    Ok(if total <= 0 { 0.0 } else { done as f64 / total as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[test]
    fn dest_appends_basename() {
        assert_eq!(dest_for("Photos/2024", "/home/me/pic.jpg"), "Photos/2024/pic.jpg");
        assert_eq!(dest_for("", "/home/me/pic.jpg"), "pic.jpg");
        assert!(copy_args("/a", "gdrive", "b").contains(&"--retries".to_string()));
    }

    #[tokio::test]
    async fn progress_tracking() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        let id = enqueue(&pool, r, "/a.bin", "up/a.bin", 1000).await.unwrap();
        progress(&pool, id, 250).await.unwrap();
        assert!((overall_fraction(&pool).await.unwrap() - 0.25).abs() < 1e-6);
        finish(&pool, id, true).await.unwrap();
        // done uploads excluded from active fraction
        assert_eq!(overall_fraction(&pool).await.unwrap(), 0.0);
    }
}
