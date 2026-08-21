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
