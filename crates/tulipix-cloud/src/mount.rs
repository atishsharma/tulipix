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

/// `np.p5.cloud.mount-cache` — VFS cache mode for a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsCache { Off, Minimal, Writes, Full }

impl VfsCache {
    pub fn as_str(self) -> &'static str {
        match self { VfsCache::Off => "off", VfsCache::Minimal => "minimal", VfsCache::Writes => "writes", VfsCache::Full => "full" }
    }
    pub fn parse(s: &str) -> Option<VfsCache> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "off" => VfsCache::Off, "minimal" => VfsCache::Minimal,
            "writes" => VfsCache::Writes, "full" => VfsCache::Full, _ => return None,
        })
    }
}

/// `rclone mount` argv with an explicit VFS cache mode and a cache-cleanup
/// max-age (`--vfs-cache-max-age`, e.g. "1h", "24h"; empty = rclone default).
/// `mount_args` is the always-full convenience wrapper over this.
///
/// Streaming/playback tuning baked in:
/// - `--vfs-cache-max-size 10G` bounds the disk the full-mode cache can eat
///   (uncapped, a few movie nights fill the drive);
/// - `--vfs-read-ahead 256M` keeps video playback fed past the mpv demuxer;
/// - `--vfs-read-chunk-size 32M` (+ 2G limit) makes seeks cheap at the start
///   and sequential reads cheap later;
/// - `--buffer-size 32M` per-file kernel-side buffer;
/// - `--vfs-fast-fingerprint` skips slow hash fingerprints on backends where
///   size+modtime is enough (Drive/OneDrive), speeding cache revalidation.
pub fn mount_args_cached(remote: &str, mount_path: &str, kind: FsKind, cache: VfsCache, max_age: &str) -> Vec<String> {
    let mut a = vec![
        "mount".into(),
        format!("{remote}:"),
        mount_path.into(),
        "--vfs-cache-mode".into(), cache.as_str().into(),
        "--dir-cache-time".into(), "60s".into(),
        "--vfs-read-chunk-size".into(), "32M".into(),
        "--vfs-read-chunk-size-limit".into(), "2G".into(),
        "--buffer-size".into(), "32M".into(),
        "--vfs-fast-fingerprint".into(),
    ];
    if cache != VfsCache::Off {
        a.push("--vfs-cache-max-size".into()); a.push("10G".into());
        a.push("--vfs-cache-poll-interval".into()); a.push("1m".into());
        a.push("--vfs-read-ahead".into()); a.push("256M".into());
        let ma = max_age.trim();
        if !ma.is_empty() {
            a.push("--vfs-cache-max-age".into()); a.push(ma.to_string());
        }
    }
    if kind == FsKind::WinFsp { a.push("--network-mode".into()); }
    a
}

/// Persist a remote's VFS cache preference (mode + max-age) so it survives
/// restarts — the mount-options dialog used to keep it in process memory only,
/// and every startup mount silently fell back to the defaults.
pub async fn set_vfs(pool: &SqlitePool, remote_id: i64, mount_path: &str, kind: FsKind, cache: &str, max_age: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO mounts (remote_id, mount_path, fs_kind, status, vfs_cache, vfs_max_age)
         VALUES (?,?,?,'unmounted',?,?)
         ON CONFLICT(remote_id) DO UPDATE SET vfs_cache = excluded.vfs_cache, vfs_max_age = excluded.vfs_max_age",
    ).bind(remote_id).bind(mount_path).bind(kind.as_str()).bind(cache).bind(max_age).execute(pool).await?;
    Ok(())
}

/// Every persisted VFS preference: (remote name, cache mode, max age).
pub async fn vfs_prefs(pool: &SqlitePool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT r.name, COALESCE(m.vfs_cache, 'full'), COALESCE(m.vfs_max_age, '')
         FROM mounts m JOIN remotes r ON r.id = m.remote_id",
    ).fetch_all(pool).await?)
}

/// Mounts flagged to auto-mount on app startup (`auto_remount = 1`).
pub async fn startup_mounts(pool: &SqlitePool) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as("SELECT remote_id, mount_path FROM mounts WHERE auto_remount = 1")
        .fetch_all(pool).await?)
}

/// Toggle mount-on-startup for a remote (creates the row if missing).
pub async fn set_auto(pool: &SqlitePool, remote_id: i64, mount_path: &str, kind: FsKind, auto: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO mounts (remote_id, mount_path, fs_kind, auto_remount, status) VALUES (?,?,?,?,'unmounted')
         ON CONFLICT(remote_id) DO UPDATE SET auto_remount = excluded.auto_remount",
    ).bind(remote_id).bind(mount_path).bind(kind.as_str()).bind(auto as i64).execute(pool).await?;
    Ok(())
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

    #[test]
    fn cached_argv_modes() {
        let a = mount_args_cached("g", "/mnt", FsKind::Fuse, VfsCache::Writes, "1h");
        assert!(a.windows(2).any(|w| w == ["--vfs-cache-mode", "writes"]));
        assert!(a.windows(2).any(|w| w == ["--vfs-cache-max-age", "1h"]));
        // off mode drops max-age
        let b = mount_args_cached("g", "/mnt", FsKind::Fuse, VfsCache::Off, "1h");
        assert!(b.windows(2).any(|w| w == ["--vfs-cache-mode", "off"]));
        assert!(!b.iter().any(|s| s == "--vfs-cache-max-age"));
        assert_eq!(VfsCache::parse("full"), Some(VfsCache::Full));
    }

    #[tokio::test]
    async fn auto_startup_list() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        set_auto(&pool, r, "/mnt/g", FsKind::Fuse, true).await.unwrap();
        assert_eq!(startup_mounts(&pool).await.unwrap(), vec![(r, "/mnt/g".to_string())]);
        set_auto(&pool, r, "/mnt/g", FsKind::Fuse, false).await.unwrap();
        assert!(startup_mounts(&pool).await.unwrap().is_empty());
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
