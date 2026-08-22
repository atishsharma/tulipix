//! Per-section SQLite pools.
//!
//! Deliberately not `tulipix_common::pool_for`, which is the same eight lines
//! plus a dependency on slint. Same DB, same schema application, same WAL
//! settings — `DbHandle` owns all of that.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::OnceCell;
use tulipix_core::db::DbHandle;

static PHOTOS: OnceCell<SqlitePool> = OnceCell::const_new();

/// Open (once) the photos DB and apply both schema layers.
///
/// Schema application is a write, and it is one-directional: a Flutter build
/// pointed at the real data directory can migrate photos.db forward under the
/// shipping Slint app. Run through `just --justfile justfile.flutter dev`,
/// which redirects XDG_* at a throwaway copy.
pub async fn photos_pool() -> Result<&'static SqlitePool> {
    PHOTOS
        .get_or_try_init(|| async {
            let pool = DbHandle::open("photos")?.init_pool().await?;
            tulipix_photos::schema::apply(&pool).await?;
            Ok(pool)
        })
        .await
}

static MUSIC: OnceCell<SqlitePool> = OnceCell::const_new();
static PODCASTS: OnceCell<SqlitePool> = OnceCell::const_new();
static RADIO: OnceCell<SqlitePool> = OnceCell::const_new();
static YOUTUBE: OnceCell<SqlitePool> = OnceCell::const_new();

/// Open (once) music.db and apply the music overlay on top of the shared
/// `items` proxy schema `init_pool` puts down.
///
/// Four pools rather than one, because the Slint side split them: none of the
/// podcast, radio or YouTube tables carries an `items` foreign key, so they
/// were moved out of music.db to keep it lean and let those pages open without
/// waiting on the library. Same file names here, so both builds read the same
/// four databases.
pub async fn music_pool() -> Result<&'static SqlitePool> {
    MUSIC
        .get_or_try_init(|| async {
            let pool = DbHandle::open("music")?.init_pool().await?;
            tulipix_music::schema::apply(&pool).await?;
            Ok(pool)
        })
        .await
}

pub async fn podcasts_pool() -> Result<&'static SqlitePool> {
    PODCASTS
        .get_or_try_init(|| async {
            let pool = DbHandle::open("podcasts")?.init_pool().await?;
            tulipix_music::podcasts::apply_schema(&pool).await?;
            Ok(pool)
        })
        .await
}

pub async fn radio_pool() -> Result<&'static SqlitePool> {
    RADIO
        .get_or_try_init(|| async {
            let pool = DbHandle::open("radio")?.init_pool().await?;
            tulipix_music::radio::apply_schema(&pool).await?;
            Ok(pool)
        })
        .await
}

pub async fn youtube_pool() -> Result<&'static SqlitePool> {
    YOUTUBE
        .get_or_try_init(|| async {
            let pool = DbHandle::open("youtube")?.init_pool().await?;
            tulipix_music::youtube::store::apply_schema(&pool).await?;
            Ok(pool)
        })
        .await
}
