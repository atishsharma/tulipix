//! `np.p4.music.listenbrainz` — ListenBrainz scrobble alternative.
//!
//! Open-data, no rate limits, token auth (no signing). Builds the
//! `submit-listens` JSON payload and shares the `scrobble_queue` table with
//! Last.fm under a distinct `service` tag so both can run at once.

use anyhow::Result;
use serde_json::json;
use sqlx::SqlitePool;

pub const SERVICE: &str = "listenbrainz";
pub const SUBMIT_URL: &str = "https://api.listenbrainz.org/1/submit-listens";

/// Build a `single` listen submission payload.
pub fn single_listen(artist: &str, title: &str, album: Option<&str>, listened_at: i64) -> serde_json::Value {
    let mut track = json!({ "artist_name": artist, "track_name": title });
    if let Some(al) = album {
        track["release_name"] = json!(al);
    }
    json!({
        "listen_type": "single",
        "payload": [ { "listened_at": listened_at, "track_metadata": track } ]
    })
}

/// Build a `playing_now` payload (no timestamp, not persisted).
pub fn playing_now(artist: &str, title: &str) -> serde_json::Value {
    json!({
        "listen_type": "playing_now",
        "payload": [ { "track_metadata": { "artist_name": artist, "track_name": title } } ]
    })
}

/// Authorization header value for a user token.
pub fn auth_header(token: &str) -> String { format!("Token {token}") }

pub async fn enqueue(pool: &SqlitePool, item_id: i64, played_at: i64) -> Result<()> {
    sqlx::query("INSERT INTO scrobble_queue (item_id, service, played_at, submitted) VALUES (?,?,?,0)")
        .bind(item_id).bind(SERVICE).bind(played_at).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn single_payload_shape() {
        let v = single_listen("Band", "Song", Some("Disc"), 1700);
        assert_eq!(v["listen_type"], "single");
        assert_eq!(v["payload"][0]["listened_at"], 1700);
        assert_eq!(v["payload"][0]["track_metadata"]["release_name"], "Disc");
    }

    #[test]
    fn playing_now_has_no_timestamp() {
        let v = playing_now("Band", "Song");
        assert_eq!(v["listen_type"], "playing_now");
        assert!(v["payload"][0].get("listened_at").is_none());
        assert_eq!(auth_header("abc"), "Token abc");
    }

    #[tokio::test]
    async fn enqueue_uses_own_service() {
        let (_t, pool) = open_pool().await;
        let id = add_track(&pool, "/m/a.flac").await;
        enqueue(&pool, id, 10).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scrobble_queue WHERE service = 'listenbrainz'").fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
    }
}
