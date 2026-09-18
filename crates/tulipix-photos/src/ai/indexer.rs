//! Running one indexing stage over one batch of photos.
//!
//! The loop that decides *when* to do this stays in each front end — the Slint
//! build ticks it on idle, the Flutter build drives it from a Run button — but
//! the work itself is the same work and lived in `tulipix-app/src/main.rs`,
//! where only Slint could reach it. That is why the Flutter build's People tab
//! and Things tab stayed empty however many models were downloaded: the models
//! landed, and nothing on that side knew how to use them.
//!
//! Every item in a batch is marked considered whether or not the stage produced
//! anything — see the note on `background::pending_for`. The one exception is a
//! stage whose model is missing, which returns early and marks nothing.

use anyhow::Result;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

use super::background::{self, Stage};

/// How many files a stage reads at once. The library scan picks its own number
/// off the core count; this one is deliberately smaller because indexing is the
/// background errand and the user is not waiting for it.
fn concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(2, 8))
        .unwrap_or(4)
}

/// Run `stage` over `batch`, returning how many items it marked considered.
///
/// `Ok(0)` from a model stage means the model is not installed — the batch is
/// left in the queue for when it is.
pub async fn run_stage(pool: &SqlitePool, stage: Stage, batch: Vec<(i64, String)>) -> Result<usize> {
    match stage {
        Stage::Exif => exif_stage(pool, batch).await,
        Stage::Fts => fts_stage(pool, batch).await,
        Stage::Tags => tags_stage(pool, batch).await,
        Stage::Faces => faces_stage(pool, batch).await,
        // CLIP is the one stage with no implementation behind it: `ai/clip.rs`
        // ships only its Null embedder. Running it would mark every photo
        // considered while embedding nothing. Kept exhaustive so wiring the
        // real embedder is a compile error here rather than a silent no-op.
        Stage::Clip => Ok(0),
    }
}

/// Claim the next batch for `stage` and run it. Returns how many were done.
///
/// The single call a driver needs: `while run_batch(..)? > 0 {}` drains a
/// stage, and one call is one yield point.
pub async fn run_batch(pool: &SqlitePool, stage: Stage, limit: i64) -> Result<usize> {
    let batch = background::next_batch(pool, stage, limit).await?;
    if batch.is_empty() {
        return Ok(0);
    }
    run_stage(pool, stage, batch).await
}

// ------------------------------------------------------------------ stages --

/// Re-read EXIF for photos that never got a `photo_meta` row — a library
/// scanned by an older build, or a file whose read failed.
async fn exif_stage(pool: &SqlitePool, batch: Vec<(i64, String)>) -> Result<usize> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency()));
    let handles: Vec<_> = batch
        .into_iter()
        .map(|(id, path)| {
            let sem = sem.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire().await.ok();
                let facts = tokio::task::spawn_blocking(move || {
                    crate::exif::read(Path::new(&path)).unwrap_or_default()
                })
                .await
                .ok();
                (id, facts)
            })
        })
        .collect();
    let mut done = 0;
    for h in handles {
        let Ok((id, Some(facts))) = h.await else { continue };
        if let Err(e) = crate::exif::write_facts(pool, id, &facts).await {
            tracing::warn!(item = id, error = %e, "indexer exif write");
            continue;
        }
        background::mark_done(pool, id, Stage::Exif, "").await?;
        done += 1;
    }
    Ok(done)
}

/// The full-text index was only ever built by "Rebuild search index" in
/// Settings, so a freshly scanned library had an empty `photo_fts`. Keeping it
/// current here is what makes search work without the user knowing that button
/// exists.
async fn fts_stage(pool: &SqlitePool, batch: Vec<(i64, String)>) -> Result<usize> {
    let mut done = 0;
    for (id, _) in batch {
        if let Err(e) = crate::search::index_item(pool, id).await {
            tracing::warn!(item = id, error = %e, "indexer fts");
            continue;
        }
        background::mark_done(pool, id, Stage::Fts, "").await?;
        done += 1;
    }
    Ok(done)
}

async fn tags_stage(pool: &SqlitePool, batch: Vec<(i64, String)>) -> Result<usize> {
    // No detector installed is not the same as "detected nothing".
    let Some(tagger) = super::load::make_tagger() else { return Ok(0) };
    let mut done = 0;
    for (id, path) in batch {
        let p = PathBuf::from(&path);
        match super::tags::ingest_predictions(pool, id, &p, tagger.as_ref(), 0.35).await {
            Ok(n) => tracing::debug!(item = id, tags = n, "tagged"),
            // A file that cannot be decoded will never tag; mark it considered
            // so it stops coming back round.
            Err(e) => tracing::debug!(item = id, error = %e, "tag skipped"),
        }
        background::mark_done(pool, id, Stage::Tags, tagger.source()).await?;
        done += 1;
    }
    Ok(done)
}

async fn faces_stage(pool: &SqlitePool, batch: Vec<(i64, String)>) -> Result<usize> {
    let Some((detector, embedder)) = super::load::make_face_models() else { return Ok(0) };
    let (mut done, mut found) = (0usize, 0usize);
    for (id, path) in batch {
        let p = PathBuf::from(&path);
        match super::faces::extract_crops(pool, id, &p, detector.as_ref()).await {
            Ok(ids) => found += ids.len(),
            Err(e) => tracing::debug!(item = id, error = %e, "face detect skipped"),
        }
        background::mark_done(pool, id, Stage::Faces, embedder.model_name()).await?;
        done += 1;
    }
    if found > 0 {
        // Embed the crops just written, then re-cluster. Clustering is over the
        // whole library by nature — a new face can merge two existing piles —
        // so it runs once per batch, not per photo.
        super::faces::embed_pending(pool, embedder.as_ref(), found as i64 * 2).await?;
        let n = super::face_clusters::recluster(pool, super::face_clusters::DEFAULT_THRESHOLD).await?;
        tracing::debug!(faces = found, people = n, "reclustered");
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_stays_in_its_lane() {
        // Indexing is the errand nobody is waiting on: it must never claim
        // every core, and must never be so small it cannot overlap IO.
        let n = concurrency();
        assert!((2..=8).contains(&n), "{n}");
    }

    #[tokio::test]
    async fn an_empty_batch_is_not_an_error() {
        // `run_batch` on a drained stage is the steady state — a driver loops
        // on it, so it has to answer 0 rather than fail.
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::schema::apply(&pool).await.unwrap();
        assert_eq!(run_batch(&pool, Stage::Fts, 32).await.unwrap(), 0);
    }
}
