//! `np.p4.cloud.selective` — per-folder selective sync mode.
//!
//! Each remote folder gets a mode: mount-only (stream, no local copy), full
//! sync (bidirectional), mirror-up (local→remote one-way), or ignore. This
//! persists the mapping and resolves the effective mode for a path by walking
//! up to the nearest configured ancestor.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Mount, Full, MirrorUp, Ignore }

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self { Mode::Mount => "mount", Mode::Full => "full", Mode::MirrorUp => "mirror_up", Mode::Ignore => "ignore" }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s { "mount" => Mode::Mount, "full" => Mode::Full, "mirror_up" => Mode::MirrorUp, "ignore" => Mode::Ignore, _ => return None })
    }
}

pub async fn set_mode(pool: &SqlitePool, remote_id: i64, folder: &str, mode: Mode) -> Result<()> {
    sqlx::query(
        "INSERT INTO selective_sync (remote_id, folder_path, mode) VALUES (?,?,?)
         ON CONFLICT(remote_id, folder_path) DO UPDATE SET mode = excluded.mode",
    ).bind(remote_id).bind(folder).bind(mode.as_str()).execute(pool).await?;
    Ok(())
}

/// Effective mode for `path`: the mode of the deepest configured ancestor,
/// defaulting to `Full` if nothing matches.
pub async fn effective_mode(pool: &SqlitePool, remote_id: i64, path: &str) -> Result<Mode> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT folder_path, mode FROM selective_sync WHERE remote_id = ?",
    ).bind(remote_id).fetch_all(pool).await?;
    let mut best: Option<(usize, Mode)> = None;
    for (folder, mode) in rows {
        if path == folder || path.starts_with(&format!("{}/", folder.trim_end_matches('/'))) {
            let depth = folder.len();
            if best.map_or(true, |(d, _)| depth > d) {
                if let Some(m) = Mode::parse(&mode) { best = Some((depth, m)); }
            }
        }
    }
    Ok(best.map(|(_, m)| m).unwrap_or(Mode::Full))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[tokio::test]
    async fn deepest_ancestor_wins() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        set_mode(&pool, r, "Photos", Mode::Mount).await.unwrap();
        set_mode(&pool, r, "Photos/2024", Mode::Full).await.unwrap();
        assert_eq!(effective_mode(&pool, r, "Photos/2024/jan/a.jpg").await.unwrap(), Mode::Full);
        assert_eq!(effective_mode(&pool, r, "Photos/2023/x.jpg").await.unwrap(), Mode::Mount);
        assert_eq!(effective_mode(&pool, r, "Docs/x.txt").await.unwrap(), Mode::Full); // default
    }
}
