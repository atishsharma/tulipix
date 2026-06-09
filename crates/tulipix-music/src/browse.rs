//! `np.p4.music.browse` — Artist / Album / Genre / Year browse views.
//!
//! The cover-art grid's data source: grouped, counted lists backed by the
//! `artists` / `albums` / `track_meta` tables, filtered to present files.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlbumRow {
    pub album_id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<i64>,
    pub cover_path: Option<String>,
    pub track_count: i64,
}

// My Music browse views exclude audiobook-flagged tracks — those live only in
// the Audiobooks section (np.p5.music.audiobook-detect).
const PRESENT: &str =
    " JOIN items ON items.id = track_meta.item_id \
      WHERE items.missing_since IS NULL AND track_meta.is_audiobook = 0 ";

pub async fn albums(pool: &SqlitePool) -> Result<Vec<AlbumRow>> {
    let rows: Vec<(i64, String, Option<String>, Option<i64>, Option<String>, i64)> = sqlx::query_as(
        "SELECT albums.id, albums.title, artists.name, albums.year, albums.cover_path, COUNT(track_meta.item_id)
         FROM albums
         JOIN track_meta ON track_meta.album_id = albums.id
         JOIN items ON items.id = track_meta.item_id AND items.missing_since IS NULL
            AND track_meta.is_audiobook = 0
         LEFT JOIN artists ON artists.id = albums.artist_id
         GROUP BY albums.id ORDER BY albums.title COLLATE NOCASE",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(album_id, title, artist, year, cover_path, track_count)| AlbumRow {
        album_id, title, artist, year, cover_path, track_count,
    }).collect())
}

/// Distinct genres with track counts, busiest first.
pub async fn genres(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        &format!("SELECT genre, COUNT(*) FROM track_meta {PRESENT} AND genre IS NOT NULL AND genre != '' GROUP BY genre ORDER BY COUNT(*) DESC"),
    ).fetch_all(pool).await?)
}

/// Distinct release years with track counts, newest first.
pub async fn years(pool: &SqlitePool) -> Result<Vec<(i64, i64)>> {
    Ok(sqlx::query_as(
        &format!("SELECT year, COUNT(*) FROM track_meta {PRESENT} AND year IS NOT NULL GROUP BY year ORDER BY year DESC"),
    ).fetch_all(pool).await?)
}

/// Artists with their album + track counts.
pub async fn artists(pool: &SqlitePool) -> Result<Vec<(i64, String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT artists.id, artists.name, COUNT(track_meta.item_id)
         FROM artists
         JOIN track_meta ON track_meta.artist_id = artists.id
         JOIN items ON items.id = track_meta.item_id AND items.missing_since IS NULL
            AND track_meta.is_audiobook = 0
         GROUP BY artists.id ORDER BY artists.name COLLATE NOCASE",
    ).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use crate::{scan, tags::TrackTags};

    async fn seed(pool: &SqlitePool, path: &str, t: TrackTags) {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        scan::upsert_track(pool, id, path, &t).await.unwrap();
    }

    #[tokio::test]
    async fn grouped_views() {
        let (_t, pool) = open_pool().await;
        seed(&pool, "/m/1.flac", TrackTags { title: Some("A".into()), artist: Some("Band".into()), album: Some("Disc".into()), genre: Some("Rock".into()), year: Some(2001), ..Default::default() }).await;
        seed(&pool, "/m/2.flac", TrackTags { title: Some("B".into()), artist: Some("Band".into()), album: Some("Disc".into()), genre: Some("Rock".into()), year: Some(2001), ..Default::default() }).await;
        let al = albums(&pool).await.unwrap();
        assert_eq!(al.len(), 1);
        assert_eq!(al[0].track_count, 2);
        assert_eq!(genres(&pool).await.unwrap()[0], ("Rock".to_string(), 2));
        assert_eq!(years(&pool).await.unwrap()[0], (2001, 2));
        assert_eq!(artists(&pool).await.unwrap()[0].2, 2);
    }
}
