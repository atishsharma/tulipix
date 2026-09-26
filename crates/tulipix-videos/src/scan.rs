//! Videos-section recursive scan + notify-based live watcher.
//!
//! Mirrors `tulipix_photos::scan`: walks every Library, lets the core
//! populator place proxy rows, prunes the non-video rows from the videos DB,
//! and seeds `video_meta` for every survivor so the timeline join is cheap.

use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::Path;
use tulipix_core::libraries::Library;
use tulipix_core::populator::{populate, PopulateStats};
use tulipix_core::watcher::{apply_event, FsEvent};

use crate::ffprobe::{probe, VideoFacts};

pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv",
    "ts", "mts", "m2ts", "vob", "3gp", "3g2", "mpg", "mpeg",
    "ogv", "rm", "rmvb", "f4v", "divx",
];

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            VIDEO_EXTS.iter().any(|x| **x == e)
        })
        .unwrap_or(false)
}

pub async fn scan_library(pool: &SqlitePool, lib: &Library) -> Result<PopulateStats> {
    let stats = populate(pool, lib).await?;
    let prefix = format!("{}%", lib.path.to_string_lossy());
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, abs_path FROM items WHERE section = 'videos' AND abs_path LIKE ?",
    )
    .bind(&prefix)
    .fetch_all(pool)
    .await?;
    for (id, p) in rows {
        if !is_video(Path::new(&p)) {
            sqlx::query("DELETE FROM items WHERE id = ?").bind(id).execute(pool).await?;
        } else {
            sqlx::query("INSERT OR IGNORE INTO video_meta (item_id) VALUES (?)")
                .bind(id).execute(pool).await?;
        }
    }
    Ok(stats)
}

pub async fn apply_fs_event(pool: &SqlitePool, event: &FsEvent) -> Result<()> {
    apply_event(pool, event).await?;
    match event {
        FsEvent::Created(p) | FsEvent::Modified(p)
            if is_video(p) => {
                let abs = p.to_string_lossy().into_owned();
                let id: Option<i64> = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
                    .bind(&abs)
                    .fetch_optional(pool)
                    .await?;
                if let Some(id) = id {
                    sqlx::query("INSERT OR IGNORE INTO video_meta (item_id) VALUES (?)")
                        .bind(id).execute(pool).await?;
                }
            }
        _ => {}
    }
    Ok(())
}

/// Items missing `video_meta.duration_s` — the background indexer's worklist.
pub async fn unprocessed_ids(pool: &SqlitePool, limit: i64) -> Result<HashSet<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT items.id
         FROM items
         JOIN video_meta ON video_meta.item_id = items.id
         WHERE items.section = 'videos'
           AND items.missing_since IS NULL
           AND video_meta.duration_s IS NULL
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Run ffprobe for one item id, write the result into `video_meta`. Idempotent.
pub async fn ingest_meta(pool: &SqlitePool, item_id: i64) -> Result<()> {
    let path: Option<String> = sqlx::query_scalar(
        "SELECT abs_path FROM items WHERE id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some(path) = path else { return Ok(()) };
    // ffprobe is a child process waited on synchronously: on the blocking pool,
    // so a library's worth of them does not hold the async workers every other
    // section's calls run on.
    let facts = tokio::task::spawn_blocking(move || probe(Path::new(&path)))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    write_meta(pool, item_id, &facts).await
}

pub async fn write_meta(pool: &SqlitePool, item_id: i64, facts: &VideoFacts) -> Result<()> {
    sqlx::query(
        "INSERT INTO video_meta (
            item_id, duration_s, container, video_codec, audio_codec,
            width, height, fps, bitrate, hdr,
            color_primaries, color_transfer, audio_channels, audio_sample_hz)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            duration_s = excluded.duration_s,
            container = excluded.container,
            video_codec = excluded.video_codec,
            audio_codec = excluded.audio_codec,
            width = excluded.width,
            height = excluded.height,
            fps = excluded.fps,
            bitrate = excluded.bitrate,
            hdr = excluded.hdr,
            color_primaries = excluded.color_primaries,
            color_transfer = excluded.color_transfer,
            audio_channels = excluded.audio_channels,
            audio_sample_hz = excluded.audio_sample_hz",
    )
    .bind(item_id)
    .bind(facts.duration_s)
    .bind(facts.container.as_ref())
    .bind(facts.video_codec.as_ref())
    .bind(facts.audio_codec.as_ref())
    .bind(facts.width)
    .bind(facts.height)
    .bind(facts.fps)
    .bind(facts.bitrate)
    .bind(facts.hdr.as_ref())
    .bind(facts.color_primaries.as_ref())
    .bind(facts.color_transfer.as_ref())
    .bind(facts.audio_channels)
    .bind(facts.audio_sample_hz)
    .execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use std::path::PathBuf;
    use tulipix_core::libraries::{ScanCadence, Section};

    fn mk_lib(path: PathBuf) -> Library {
        Library {
            id: "lib1".into(),
            path, section: Section::Videos,
            last_scan: None, item_count: 0, size_bytes: 0,
            exclude_globs: vec![],
            cadence_override: Some(ScanCadence::Manual),
            realtime_notify: false,
        }
    }

    #[tokio::test]
    async fn scan_keeps_videos_drops_other() {
        let (_t, pool) = open_pool().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.mp4"), b"x").unwrap();
        std::fs::write(root.path().join("b.txt"), b"y").unwrap();
        let lib = mk_lib(root.path().to_path_buf());
        let _ = scan_library(&pool, &lib).await.unwrap();
        let videos: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items").fetch_one(&pool).await.unwrap();
        assert_eq!(videos, 1, "non-video rows pruned from videos DB");
        let meta: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_meta").fetch_one(&pool).await.unwrap();
        assert_eq!(meta, 1);
    }

    #[test]
    fn ext_filter_covers_common_containers() {
        for e in ["mp4", "MKV", "mov", "webm", "ts", "m2ts"] {
            let p = PathBuf::from(format!("/tmp/x.{e}"));
            assert!(is_video(&p), "{e} should be a video");
        }
        assert!(!is_video(Path::new("/tmp/x.txt")));
        assert!(!is_video(Path::new("/tmp/x.jpg")));
    }

    #[tokio::test]
    async fn write_meta_round_trip() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.mp4', 0, 1, 0, 'videos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/a.mp4'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO video_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        let facts = VideoFacts {
            duration_s: Some(60.0), video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()), width: Some(1920), height: Some(1080),
            fps: Some(30.0), bitrate: Some(5_000_000),
            ..VideoFacts::default()
        };
        write_meta(&pool, id, &facts).await.unwrap();
        let (dur, w, h): (Option<f64>, Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT duration_s, width, height FROM video_meta WHERE item_id = ?",
        ).bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(dur, Some(60.0));
        assert_eq!(w, Some(1920));
        assert_eq!(h, Some(1080));
    }
}
