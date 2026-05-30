//! `np.p4.cloud.mount` — FUSE (Linux/macOS) + WinFsp (Windows) + auto-remount.
//!
//! Builds the `rclone mount` argv (with VFS cache flags) and owns the
//! auto-remount backoff: when a mount drops and `auto_remount` is set, retry
//! with exponential backoff capped at a ceiling.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind { Fuse, WinFsp, Http }

impl FsKind {
    pub fn as_str(self) -> &'static str {
        match self { FsKind::Fuse => "fuse", FsKind::WinFsp => "winfsp", FsKind::Http => "http" }
    }
    /// Platform default backend for the current OS string.
    pub fn for_os(os: &str) -> FsKind {
        match os { "windows" => FsKind::WinFsp, _ => FsKind::Fuse }
    }
}

/// `rclone mount remote:path /mnt --vfs-cache-mode full ...` argv.
pub fn mount_args(remote: &str, mount_path: &str, kind: FsKind) -> Vec<String> {
    let mut a = vec![
        "mount".into(),
        format!("{remote}:"),
        mount_path.into(),
        "--vfs-cache-mode".into(), "full".into(),
        "--dir-cache-time".into(), "30s".into(),
    ];
    if kind == FsKind::WinFsp { a.push("--network-mode".into()); }
    a
}

/// Backoff (seconds) for the Nth consecutive remount attempt: 2^n capped at 60.
pub fn remount_backoff_s(attempt: u32) -> u64 {
    (1u64 << attempt.min(6)).min(60)
}

pub async fn record(pool: &SqlitePool, remote_id: i64, mount_path: &str, kind: FsKind, auto: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO mounts (remote_id, mount_path, fs_kind, auto_remount, status) VALUES (?,?,?,?,'mounted')
         ON CONFLICT(remote_id) DO UPDATE SET mount_path=excluded.mount_path, fs_kind=excluded.fs_kind,
            auto_remount=excluded.auto_remount, status='mounted'",
    ).bind(remote_id).bind(mount_path).bind(kind.as_str()).bind(auto as i64).execute(pool).await?;
    Ok(())
}

pub async fn set_status(pool: &SqlitePool, remote_id: i64, status: &str) -> Result<()> {
    sqlx::query("UPDATE mounts SET status = ? WHERE remote_id = ?").bind(status).bind(remote_id).execute(pool).await?;
    Ok(())
}

/// Mounts that dropped but want auto-remount.
pub async fn needs_remount(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT remote_id FROM mounts WHERE status != 'mounted' AND auto_remount = 1")
        .fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[test]
    fn argv_and_backoff() {
        let a = mount_args("gdrive", "/mnt/g", FsKind::Fuse);
        assert_eq!(a[1], "gdrive:");
        assert!(a.contains(&"--vfs-cache-mode".to_string()));
        assert!(mount_args("x", "Z:", FsKind::WinFsp).contains(&"--network-mode".to_string()));
        assert_eq!(remount_backoff_s(0), 1);
        assert_eq!(remount_backoff_s(3), 8);
        assert_eq!(remount_backoff_s(20), 60); // capped
        assert_eq!(FsKind::for_os("windows"), FsKind::WinFsp);
    }

    #[tokio::test]
    async fn remount_queue() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        record(&pool, r, "/mnt/g", FsKind::Fuse, true).await.unwrap();
        assert!(needs_remount(&pool).await.unwrap().is_empty());
        set_status(&pool, r, "error").await.unwrap();
        assert_eq!(needs_remount(&pool).await.unwrap(), vec![r]);
    }
}
