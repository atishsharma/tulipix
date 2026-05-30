//! `np.p4.music.scrobble` — Last.fm scrobbling (opt-in).
//!
//! Last.fm signs every call: sort params by name, concat `name+value`, append
//! the shared secret, MD5 the result. That ordering is the bug-prone part and
//! is unit-tested here. Scrobbles are queued in `scrobble_queue` so offline
//! plays submit on reconnect (Last.fm's "now playing" vs "scrobble" split).

use anyhow::Result;
use sqlx::SqlitePool;

pub const SERVICE: &str = "lastfm";
pub const API_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
/// Last.fm requires ≥ half the track (or 4 min) played before a scrobble counts.
pub const MIN_PLAY_FRACTION: f64 = 0.5;
pub const MIN_PLAY_SECS: f64 = 240.0;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Does this play qualify as a scrobble per Last.fm rules?
pub fn qualifies(played_s: f64, duration_s: f64) -> bool {
    duration_s > 30.0 && (played_s >= duration_s * MIN_PLAY_FRACTION || played_s >= MIN_PLAY_SECS)
}

/// Build the string to MD5 for an API signature: params sorted by key, each
/// `key+value` concatenated, then the secret. Callers MD5 this.
pub fn signature_base(params: &[(&str, &str)], secret: &str) -> String {
    let mut p: Vec<(&str, &str)> = params.to_vec();
    p.sort_by(|a, b| a.0.cmp(b.0));
    let mut s = String::new();
    for (k, v) in p { s.push_str(k); s.push_str(v); }
    s.push_str(secret);
    s
}

/// Enqueue a play for later submission.
pub async fn enqueue(pool: &SqlitePool, item_id: i64, played_at: i64) -> Result<()> {
    sqlx::query("INSERT INTO scrobble_queue (item_id, service, played_at, submitted) VALUES (?,?,?,0)")
        .bind(item_id).bind(SERVICE).bind(played_at).execute(pool).await?;
    Ok(())
}

/// Item ids + timestamps awaiting submission, oldest first.
pub async fn pending(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT id, item_id, played_at FROM scrobble_queue WHERE service = ? AND submitted = 0 ORDER BY played_at ASC LIMIT ?",
    ).bind(SERVICE).bind(limit).fetch_all(pool).await?)
}

/// Mark queue rows submitted after a successful batch.
pub async fn mark_submitted(pool: &SqlitePool, ids: &[i64]) -> Result<()> {
    for id in ids {
        sqlx::query("UPDATE scrobble_queue SET submitted = 1 WHERE id = ?").bind(id).execute(pool).await?;
    }
    Ok(())
}

#[allow(unused)]
fn touch() -> i64 { now() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn signature_sorts_params() {
        // 'method' before 'track' alphabetically, regardless of input order.
        let base = signature_base(&[("track", "Song"), ("method", "track.scrobble")], "secret");
        assert_eq!(base, "methodtrack.scrobbletrackSongsecret");
    }

    #[test]
    fn scrobble_threshold() {
        assert!(qualifies(150.0, 200.0));   // 75% played
        assert!(!qualifies(40.0, 200.0));   // only 20%
        assert!(qualifies(250.0, 1000.0));  // long track, 4 min rule
        assert!(!qualifies(20.0, 25.0));    // too short to ever scrobble
    }

    #[tokio::test]
    async fn queue_roundtrip() {
        let (_t, pool) = open_pool().await;
        let id = add_track(&pool, "/m/a.flac").await;
        enqueue(&pool, id, 1000).await.unwrap();
        let p = pending(&pool, 10).await.unwrap();
        assert_eq!(p.len(), 1);
        mark_submitted(&pool, &[p[0].0]).await.unwrap();
        assert!(pending(&pool, 10).await.unwrap().is_empty());
    }
}
