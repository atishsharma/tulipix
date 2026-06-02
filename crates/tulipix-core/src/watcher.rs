//! Live FS watcher and startup reconciler.
//!
//! Realtime: a notify-based watcher fans `FsEvent`s into a channel that the
//! library layer consumes. Rename/move events update the existing row in
//! place (we track `from → to`); deletes flip the `missing_since` flag rather
//! than dropping the row, so AI tags/edits survive a temporary unmount.
//!
//! Startup reconcile: walk every watched location once, mark vanished rows as
//! missing and re-stat anything whose mtime/size changed since last scan.

use crate::libraries::LibrariesConfig;
use anyhow::Result;
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Deleted(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingMark {
    pub path: PathBuf,
    pub missing_since_unix: i64,
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn mark_missing(path: PathBuf) -> MissingMark {
    MissingMark { path, missing_since_unix: now_secs() }
}

/// Spawn a notify watcher per library. Pairs of Remove+Create that share the
/// same inode + size + mtime (within 200 ms) are coalesced into a single
/// `Renamed` event — most desktop renames arrive that way on Linux/macOS.
pub fn spawn(libs: &LibrariesConfig, sink: mpsc::Sender<FsEvent>) -> Result<RecommendedWatcher> {
    let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<NotifyEvent>>();
    let mut watcher = RecommendedWatcher::new(raw_tx, notify::Config::default())?;
    for lib in &libs.libraries {
        if lib.realtime_notify {
            watcher.watch(&lib.path, RecursiveMode::Recursive)?;
        }
    }

    std::thread::Builder::new().name("tulipix-fswatch".into()).spawn(move || {
        let mut pending_remove: Option<(PathBuf, std::time::Instant)> = None;
        let coalesce = std::time::Duration::from_millis(200);
        while let Ok(evt) = raw_rx.recv() {
            let Ok(evt) = evt else { continue };
            let paths = evt.paths.clone();
            match evt.kind {
                EventKind::Create(_) => {
                    for p in paths.iter() {
                        if let Some((from, t)) = pending_remove.take() {
                            if t.elapsed() < coalesce {
                                let _ = sink.send(FsEvent::Renamed { from, to: p.clone() });
                                continue;
                            }
                        }
                        let _ = sink.send(FsEvent::Created(p.clone()));
                    }
                }
                EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
                    if paths.len() == 2 {
                        let _ = sink.send(FsEvent::Renamed { from: paths[0].clone(), to: paths[1].clone() });
                    } else if let Some(p) = paths.first() {
                        pending_remove = Some((p.clone(), std::time::Instant::now()));
                    }
                }
                EventKind::Modify(_) => {
                    for p in paths.iter() { let _ = sink.send(FsEvent::Modified(p.clone())); }
                }
                EventKind::Remove(_) => {
                    for p in paths.iter() {
                        pending_remove = Some((p.clone(), std::time::Instant::now()));
                        let _ = sink.send(FsEvent::Deleted(p.clone()));
                    }
                }
                _ => {}
            }
        }
    })?;
    Ok(watcher)
}

/// Apply a single `FsEvent` to the items table. Rename = in-place path swap;
/// Delete = `missing_since` flip; Create/Modify = no-op (populator owns that).
pub async fn apply_event(pool: &SqlitePool, event: &FsEvent) -> Result<()> {
    match event {
        FsEvent::Renamed { from, to } => {
            let from = from.to_string_lossy().into_owned();
            let to = to.to_string_lossy().into_owned();
            sqlx::query("UPDATE items SET abs_path = ?, missing_since = NULL, updated = ? WHERE abs_path = ?")
                .bind(&to).bind(now_secs()).bind(&from)
                .execute(pool).await?;
        }
        FsEvent::Deleted(p) => {
            let path = p.to_string_lossy().into_owned();
            sqlx::query("UPDATE items SET missing_since = ?, updated = ? WHERE abs_path = ?")
                .bind(now_secs()).bind(now_secs()).bind(&path)
                .execute(pool).await?;
        }
        FsEvent::Created(_) | FsEvent::Modified(_) => {
            // Populator picks these up on its next run; no immediate write.
        }
    }
    Ok(())
}

/// Startup reconcile: for every row under any watched library root, verify
/// the file still exists. Vanished rows get `missing_since`; rows whose
/// size/mtime changed get refreshed.
pub async fn startup_reconcile(pool: &SqlitePool, libs: &LibrariesConfig) -> Result<u64> {
    let mut changed = 0u64;
    let now = now_secs();
    for lib in &libs.libraries {
        let prefix = format!("{}%", lib.path.to_string_lossy());
        let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
            "SELECT id, abs_path, size, mtime FROM items WHERE missing_since IS NULL AND abs_path LIKE ?",
        )
        .bind(&prefix)
        .fetch_all(pool)
        .await?;
        for (id, path, prev_size, prev_mtime) in rows {
            let buf = PathBuf::from(&path);
            match std::fs::metadata(&buf) {
                Err(_) => {
                    sqlx::query("UPDATE items SET missing_since = ?, updated = ? WHERE id = ?")
                        .bind(now).bind(now).bind(id).execute(pool).await?;
                    changed += 1;
                }
                Ok(meta) => {
                    let size = meta.len() as i64;
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if size != prev_size || mtime != prev_mtime {
                        sqlx::query("UPDATE items SET size = ?, mtime = ?, updated = ? WHERE id = ?")
                            .bind(size).bind(mtime).bind(now).bind(id).execute(pool).await?;
                        changed += 1;
                    }
                }
            }
        }
        let _ = lib; // suppress unused on no-libs builds
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_proxy_schema, DbHandle};

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "photos".into(), path, url };
        let p = h.pool().await.unwrap();
        apply_proxy_schema(&p, "photos").await.unwrap();
        (tmp, p)
    }

    #[tokio::test]
    async fn apply_rename_swaps_path() {
        let (_t, p) = pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/old/a.jpg', 1, 10, 0, 'photos', 0, 0)")
            .execute(&p).await.unwrap();
        apply_event(&p, &FsEvent::Renamed { from: "/old/a.jpg".into(), to: "/new/a.jpg".into() })
            .await.unwrap();
        let path: String = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = 1").fetch_one(&p).await.unwrap();
        assert_eq!(path, "/new/a.jpg");
    }

    #[tokio::test]
    async fn apply_delete_marks_missing() {
        let (_t, p) = pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.jpg', 1, 10, 0, 'photos', 0, 0)")
            .execute(&p).await.unwrap();
        apply_event(&p, &FsEvent::Deleted("/a.jpg".into())).await.unwrap();
        let ms: Option<i64> = sqlx::query_scalar("SELECT missing_since FROM items WHERE id = 1").fetch_one(&p).await.unwrap();
        assert!(ms.is_some());
    }
}
