//! `np.p4.music.scan` — populate `music.db` from scanned audio files.
//!
//! The shared scanner (tulipix-core) inserts the `items` row; this module
//! attaches the music overlay: a `track_meta` row plus get-or-create
//! `artists` / `albums` rows so browse views can group cheaply.

use anyhow::Result;
use sqlx::SqlitePool;
use std::path::Path;

use crate::tags::TrackTags;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Get the artist id by name, creating the row if absent.
pub async fn get_or_create_artist(pool: &SqlitePool, name: &str) -> Result<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM artists WHERE name = ?")
        .bind(name).fetch_optional(pool).await?
    {
        return Ok(id);
    }
    sqlx::query("INSERT INTO artists (name) VALUES (?)").bind(name).execute(pool).await?;
    Ok(sqlx::query_scalar::<_, i64>("SELECT id FROM artists WHERE name = ?")
        .bind(name).fetch_one(pool).await?)
}

/// Get the album id for (title, artist), creating the row if absent.
pub async fn get_or_create_album(pool: &SqlitePool, title: &str, artist_id: Option<i64>, year: Option<i64>) -> Result<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM albums WHERE title = ? AND artist_id IS ?")
        .bind(title).bind(artist_id).fetch_optional(pool).await?
    {
        return Ok(id);
    }
    sqlx::query("INSERT INTO albums (title, artist_id, year) VALUES (?, ?, ?)")
        .bind(title).bind(artist_id).bind(year).execute(pool).await?;
    Ok(sqlx::query_scalar::<_, i64>("SELECT id FROM albums WHERE title = ? AND artist_id IS ?")
        .bind(title).bind(artist_id).fetch_one(pool).await?)
}

/// Insert (or refresh) the `track_meta` row for an already-inserted `items`
/// row, wiring up artist/album foreign keys from the parsed tags.
/// What the tag reader knows how to extract, as a number that goes up.
///
/// Stored on every row it writes. A rescan re-reads anything below the current
/// value, which is what makes a new field reach a library that was already
/// scanned — without it, `read_missing_tags` only ever touches rows that were
/// never tagged at all, and a column added today would stay empty forever on
/// every existing install.
///
/// Bump this whenever [`TrackTags`] gains a field the reader fills.
///   1 — the original fourteen fields.
///   2 — composer, performer, producer, remixer, label, catalogue, release date.
pub const TAGS_VERSION: i64 = 2;

pub async fn upsert_track(pool: &SqlitePool, item_id: i64, abs_path: &str, t: &TrackTags) -> Result<()> {
    let artist_id = match &t.artist {
        Some(a) if !a.is_empty() => Some(get_or_create_artist(pool, a).await?),
        _ => None,
    };
    let album_id = match &t.album {
        Some(al) if !al.is_empty() => Some(get_or_create_album(pool, al, artist_id, t.year).await?),
        _ => None,
    };
    let folder = Path::new(abs_path).parent().map(|p| p.to_string_lossy().into_owned());
    sqlx::query(
        "INSERT INTO track_meta
            (item_id, title, artist_id, album_id, album_artist, genre, year,
             track_no, disc_no, duration_s, bitrate, sample_rate, channels,
             codec, container, folder,
             composer, performer, producer, remixer, label, catalog_no, release_date,
             tags_version)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET
            title=excluded.title, artist_id=excluded.artist_id,
            album_id=excluded.album_id, album_artist=excluded.album_artist,
            genre=excluded.genre, year=excluded.year, track_no=excluded.track_no,
            disc_no=excluded.disc_no, duration_s=excluded.duration_s,
            bitrate=excluded.bitrate, sample_rate=excluded.sample_rate,
            channels=excluded.channels, codec=excluded.codec,
            container=excluded.container, folder=excluded.folder,
            composer=excluded.composer, performer=excluded.performer,
            producer=excluded.producer, remixer=excluded.remixer,
            label=excluded.label, catalog_no=excluded.catalog_no,
            release_date=excluded.release_date,
            tags_version=excluded.tags_version",
    )
    .bind(item_id).bind(&t.title).bind(artist_id).bind(album_id)
    .bind(&t.album_artist).bind(&t.genre).bind(t.year)
    .bind(t.track_no).bind(t.disc_no).bind(t.duration_s)
    .bind(t.bitrate).bind(t.sample_rate).bind(t.channels)
    .bind(&t.codec).bind(&t.container).bind(folder)
    .bind(&t.composer).bind(&t.performer).bind(&t.producer).bind(&t.remixer)
    .bind(&t.label).bind(&t.catalog_no).bind(&t.release_date)
    .bind(TAGS_VERSION)
    .execute(pool).await?;
    Ok(())
}

/// Count of tracks currently visible (file present).
pub async fn track_count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE items.missing_since IS NULL",
    ).fetch_one(pool).await?)
}

#[allow(unused)]
fn touch_ts() -> i64 { now() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add_item(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap()
    }


    #[tokio::test]
    async fn a_written_row_carries_the_reader_version() {
        let (_t, pool) = open_pool().await;
        let id = add_item(&pool, "/m/x.flac").await;
        // A row from before the column existed reads as version 0, which is
        // what the rescan predicate looks for.
        sqlx::query("INSERT INTO track_meta (item_id, title) VALUES (?, 'Old')")
            .bind(id).execute(&pool).await.unwrap();
        let v: i64 = sqlx::query_scalar("SELECT tags_version FROM track_meta WHERE item_id = ?")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(v, 0, "an untouched row is behind the reader");
        assert!(v < TAGS_VERSION);

        upsert_track(&pool, id, "/m/x.flac", &TrackTags {
            title: Some("New".into()),
            composer: Some("Yorke".into()),
            ..Default::default()
        }).await.unwrap();
        let (v, composer): (i64, Option<String>) = sqlx::query_as(
            "SELECT tags_version, composer FROM track_meta WHERE item_id = ?")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(v, TAGS_VERSION);
        assert_eq!(composer.as_deref(), Some("Yorke"));
    }

    #[tokio::test]
    async fn upsert_wires_artist_and_album() {
        let (_t, pool) = open_pool().await;
        let id = add_item(&pool, "/m/song.flac").await;
        let tags = TrackTags { title: Some("Song".into()), artist: Some("Band".into()), album: Some("Disc".into()), year: Some(2020), ..Default::default() };
        upsert_track(&pool, id, "/m/song.flac", &tags).await.unwrap();
        assert_eq!(track_count(&pool).await.unwrap(), 1);
        let artist: String = sqlx::query_scalar(
            "SELECT artists.name FROM track_meta JOIN artists ON artists.id = track_meta.artist_id WHERE item_id = ?")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(artist, "Band");
        // re-scan is idempotent — no duplicate artist
        upsert_track(&pool, id, "/m/song.flac", &tags).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artists").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
    }
}
