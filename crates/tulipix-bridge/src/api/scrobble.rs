//! Submitting listens — the half `tulipix_music::{scrobble, listenbrainz}` did
//! not have.
//!
//! Both modules were complete and unreferenced: payload builders, Last.fm
//! request signing, and a shared `scrobble_queue` that survives being offline.
//! What was missing on both sides was the part that decides a play counts and
//! the part that posts it. That is all this is.
//!
//! ListenBrainz only, for now. Its auth is a user token pasted into Settings;
//! Last.fm needs a token→session browser round trip that has nowhere to happen
//! yet, and queueing rows for a service that cannot submit them would grow a
//! table nothing ever drains.

use anyhow::Result;

use crate::db::music_pool;
use tulipix_music::{listenbrainz, scrobble};

/// How many queued listens one flush will send. ListenBrainz has no rate limit
/// worth fearing, but a hundred POSTs from a laptop coming back online is a
/// burst nobody asked for.
const BATCH: i64 = 25;

fn setting(key: &str, default: &str) -> String {
    tulipix_core::settings::Settings::load()
        .map(|s| s.text(key))
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The user's ListenBrainz token, when the service is switched on and one has
/// been entered. `None` is the normal state and means: do nothing at all.
fn lb_token() -> Option<String> {
    let on = tulipix_core::settings::Settings::load()
        .map(|s| s.flag("api.listenbrainz", false))
        .unwrap_or(false);
    if !on {
        return None;
    }
    let t = setting("api.listenbrainz-token", "");
    (!t.trim().is_empty()).then(|| t.trim().to_string())
}

/// Queue a finished play if it qualifies, then try to send it.
///
/// Called from the track-change path, which is the only place that knows how
/// much of the outgoing track was actually heard. Silent on every failure: a
/// scrobble is a nicety, and a listener should never be shown an error because
/// a website was down.
pub(crate) async fn on_play_finished(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    played_s: f64,
    duration_s: f64,
) {
    if lb_token().is_none() {
        return;
    }
    // Last.fm's rule, which ListenBrainz also recommends: half the track, or
    // four minutes, and nothing under thirty seconds.
    if !scrobble::qualifies(played_s, duration_s) {
        return;
    }
    let at = crate::api::home::now_secs();
    if let Err(e) = listenbrainz::enqueue(pool, item_id, at).await {
        tracing::debug!(item_id, error = %e, "listenbrainz enqueue");
        return;
    }
    flush().await;
}

/// Send whatever is queued. Safe to call at any time; does nothing when the
/// service is off, unconfigured, or the queue is empty.
pub async fn scrobble_flush() -> Result<i64> {
    Ok(flush().await)
}

/// Returns how many listens went out.
async fn flush() -> i64 {
    let Some(token) = lb_token() else { return 0 };
    let Ok(pool) = music_pool().await else { return 0 };

    let rows = match scrobble::pending_for(pool, listenbrainz::SERVICE, BATCH).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "listenbrainz pending");
            return 0;
        }
    };
    if rows.is_empty() {
        return 0;
    }

    let client = tulipix_core::net::http().clone();
    let auth = listenbrainz::auth_header(&token);
    let mut sent = Vec::new();

    for (queue_id, item_id, played_at) in rows {
        // The tags as they are now, not as they were when it played: a track
        // retagged since is better described by the new value.
        let meta: Option<(String, String, String)> = sqlx::query_as(
            "SELECT COALESCE(tm.title, ''), COALESCE(ar.name, ''), COALESCE(al.title, '') \
             FROM track_meta tm \
             LEFT JOIN artists ar ON ar.id = tm.artist_id \
             LEFT JOIN albums al ON al.id = tm.album_id \
             WHERE tm.item_id = ?",
        )
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        let Some((title, artist, album)) = meta else {
            // The track was deleted since it played. The row can never be
            // submitted, so retire it rather than retrying it forever.
            sent.push(queue_id);
            continue;
        };
        // ListenBrainz rejects a listen with no artist or no title outright.
        if title.trim().is_empty() || artist.trim().is_empty() {
            sent.push(queue_id);
            continue;
        }

        let body = listenbrainz::single_listen(
            &artist,
            &title,
            (!album.trim().is_empty()).then_some(album.as_str()),
            played_at,
        );
        let res = client
            .post(listenbrainz::SUBMIT_URL)
            .header(reqwest::header::AUTHORIZATION, auth.as_str())
            .json(&body)
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => sent.push(queue_id),
            // A rejected listen is not a retryable one -- a bad token or a
            // malformed payload will fail identically forever, and only a 5xx
            // or a dead connection is worth keeping in the queue.
            Ok(r) if r.status().is_client_error() => {
                tracing::warn!(status = %r.status(), "listenbrainz rejected a listen");
                sent.push(queue_id);
            }
            Ok(r) => {
                tracing::debug!(status = %r.status(), "listenbrainz server error; will retry");
                break;
            }
            Err(e) => {
                // Offline. Stop here and keep the rest queued.
                tracing::debug!(error = %e, "listenbrainz unreachable; will retry");
                break;
            }
        }
    }

    if sent.is_empty() {
        return 0;
    }
    if let Err(e) = scrobble::mark_submitted(pool, &sent).await {
        tracing::warn!(error = %e, "listenbrainz mark_submitted");
    }
    sent.len() as i64
}

/// How many listens are waiting, for the Settings row to report.
pub async fn scrobble_pending_count() -> Result<i64> {
    let pool = music_pool().await?;
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM scrobble_queue WHERE service = ? AND submitted = 0",
    )
    .bind(listenbrainz::SERVICE)
    .fetch_one(pool)
    .await
    .unwrap_or(0))
}
