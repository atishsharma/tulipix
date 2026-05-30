//! `np.p4.music.video-link` — music-videos sub-section.
//!
//! Links a music track row to a video file row (in the Videos section) via
//! `music_video_link`, so the player can switch between audio-only and video
//! modes for the same song. The link stores the foreign video `item_id`
//! (which lives in the videos DB) as an opaque id.

use anyhow::Result;
use sqlx::SqlitePool;

/// Link (or re-point) a track to its music-video item.
pub async fn link(pool: &SqlitePool, track_item_id: i64, video_item_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO music_video_link (item_id, video_item_id) VALUES (?, ?)
         ON CONFLICT(item_id) DO UPDATE SET video_item_id = excluded.video_item_id",
    ).bind(track_item_id).bind(video_item_id).execute(pool).await?;
    Ok(())
}

pub async fn unlink(pool: &SqlitePool, track_item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM music_video_link WHERE item_id = ?").bind(track_item_id).execute(pool).await?;
    Ok(())
}

/// The linked video item id for a track, if any.
pub async fn video_for(pool: &SqlitePool, track_item_id: i64) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT video_item_id FROM music_video_link WHERE item_id = ?")
        .bind(track_item_id).fetch_optional(pool).await?)
}

/// Whether a track has a music video (drives the audio/video toggle button).
pub async fn has_video(pool: &SqlitePool, track_item_id: i64) -> Result<bool> {
    Ok(video_for(pool, track_item_id).await?.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn link_unlink_roundtrip() {
        let (_t, pool) = open_pool().await;
        let track = add_track(&pool, "/m/song.flac").await;
        assert!(!has_video(&pool, track).await.unwrap());
        link(&pool, track, 7777).await.unwrap();
        assert_eq!(video_for(&pool, track).await.unwrap(), Some(7777));
        // re-link re-points instead of erroring
        link(&pool, track, 8888).await.unwrap();
        assert_eq!(video_for(&pool, track).await.unwrap(), Some(8888));
        unlink(&pool, track).await.unwrap();
        assert!(!has_video(&pool, track).await.unwrap());
    }
}
