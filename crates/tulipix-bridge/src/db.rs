//! Per-section SQLite pools.
//!
//! Thin wrappers over `tulipix_common::pool_for`, which is the same cache the
//! Slint build and the CLI go through. This file used to open its own — same
//! file, same schema, same WAL settings — on the grounds that `pool_for` came
//! with a dependency on slint. It no longer does (see the note above the
//! domain-crate block in Cargo.toml), and the duplication was not free: the
//! bridge links `tulipix-status`, whose `collect()` reaches for `pool_for` on
//! ten databases, so nine of these files were open twice in one process. Two
//! pools, two sets of connections, and SQLite's page cache is per *connection*
//! — sixty-five `sqlx-sqlite-worker` threads and a quarter of a gigabyte of
//! cache for ten files that ten connections could serve.
//!
//! The cells stay: `pool_for` hands back a cloned `SqlitePool`, and these
//! return `&'static` so callers can hold one across an await without cloning.
//!
//! Schema application is a write, and it is one-directional: a Flutter build
//! pointed at the real data directory can migrate photos.db forward under the
//! shipping Slint app. Run through `just --justfile justfile.flutter dev`,
//! which redirects XDG_* at a throwaway copy.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::OnceCell;

static PHOTOS: OnceCell<SqlitePool> = OnceCell::const_new();
static MUSIC: OnceCell<SqlitePool> = OnceCell::const_new();
static PODCASTS: OnceCell<SqlitePool> = OnceCell::const_new();
static RADIO: OnceCell<SqlitePool> = OnceCell::const_new();
static YOUTUBE: OnceCell<SqlitePool> = OnceCell::const_new();
static BOOKS: OnceCell<SqlitePool> = OnceCell::const_new();
static CLOUD: OnceCell<SqlitePool> = OnceCell::const_new();
static TOOLS: OnceCell<SqlitePool> = OnceCell::const_new();
static FINANCES: OnceCell<SqlitePool> = OnceCell::const_new();
static VIDEOS: OnceCell<SqlitePool> = OnceCell::const_new();

pub async fn photos_pool() -> Result<&'static SqlitePool> {
    PHOTOS
        .get_or_try_init(|| tulipix_common::pool_for("photos"))
        .await
}

/// Four pools rather than one, because the Slint side split them: none of the
/// podcast, radio or YouTube tables carries an `items` foreign key, so they
/// were moved out of music.db to keep it lean and let those pages open without
/// waiting on the library. Same file names here, so both builds read the same
/// four databases.
pub async fn music_pool() -> Result<&'static SqlitePool> {
    MUSIC
        .get_or_try_init(|| tulipix_common::pool_for("music"))
        .await
}

/// The books library: one database, same file name as the Slint build's, so a
/// rating set in one shows in the other.
pub async fn books_pool() -> Result<&'static SqlitePool> {
    BOOKS
        .get_or_try_init(|| tulipix_common::pool_for("books"))
        .await
}

/// rclone's bookkeeping: which remotes exist, what is mounted, saved sync
/// jobs, share links, the recycle bin. rclone's own config file stays where
/// rclone keeps it — this is only what the app knows on top.
pub async fn cloud_pool() -> Result<&'static SqlitePool> {
    CLOUD
        .get_or_try_init(|| tulipix_common::pool_for("cloud"))
        .await
}

/// The tools job queue. Survives a restart, which is the point of it being a
/// table rather than a channel.
pub async fn tools_pool() -> Result<&'static SqlitePool> {
    TOOLS
        .get_or_try_init(|| tulipix_common::pool_for("tools"))
        .await
}

pub async fn podcasts_pool() -> Result<&'static SqlitePool> {
    PODCASTS
        .get_or_try_init(|| tulipix_common::pool_for("podcasts"))
        .await
}

pub async fn radio_pool() -> Result<&'static SqlitePool> {
    RADIO
        .get_or_try_init(|| tulipix_common::pool_for("radio"))
        .await
}

pub async fn youtube_pool() -> Result<&'static SqlitePool> {
    YOUTUBE
        .get_or_try_init(|| tulipix_common::pool_for("youtube"))
        .await
}

/// The money database.
///
/// The one pool here that does not go through `pool_for`, because `pool_for`
/// has no `finances` arm: `tulipix_finances::open` applies the schema, seeds
/// the default categories *and* seeds the sample data on a database that has
/// never held anything real. Opening it by hand would skip the last of those
/// and leave nine tabs of zeroes on a first run, which is indistinguishable
/// from nine broken tabs.
pub async fn finances_pool() -> Result<&'static SqlitePool> {
    FINANCES.get_or_try_init(tulipix_finances::open).await
}

/// The videos database.
///
/// Four schemas on one file, applied in the order the Slint build applies them.
/// `pool_for` puts down the section's own tables; the rest are the features
/// that shipped separately — Discover's feed cache, the Stream tab's five
/// (progress, bookmarks, resolved-stream cache, the download ledger and the
/// landing-feed cache), then Stream Plus's. They are separate `apply` functions
/// because the features shipped separately, not because they are separate
/// databases — the Slint build opens exactly this one file.
pub async fn videos_pool() -> Result<&'static SqlitePool> {
    VIDEOS
        .get_or_try_init(|| async {
            let pool = tulipix_common::pool_for("videos").await?;
            tulipix_videos::discover::apply_schema(&pool).await?;
            tulipix_videos::stream::progress::apply_schema(&pool).await?;
            tulipix_videos::stream::bookmarks::apply_schema(&pool).await?;
            tulipix_videos::stream::cache::apply_schema(&pool).await?;
            tulipix_videos::stream::downloads::apply_schema(&pool).await?;
            tulipix_videos::stream::feed_cache::apply_schema(&pool).await?;
            tulipix_videos::splus::schema::apply_schema(&pool).await?;
            Ok(pool)
        })
        .await
}
