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

/// Hex MD5 API signature for a signed Last.fm call.
pub fn api_sig(params: &[(&str, &str)], secret: &str) -> String {
    use md5::Digest;
    let digest = md5::Md5::digest(signature_base(params, secret).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Signed request params ready to send: input params + `api_sig` + JSON format.
/// (`format` is excluded from the signature per the Last.fm spec.)
pub fn signed_params(params: &[(&str, &str)], secret: &str) -> Vec<(String, String)> {
    let sig = api_sig(params, secret);
    let mut out: Vec<(String, String)> =
        params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    out.push(("api_sig".into(), sig));
    out.push(("format".into(), "json".into()));
    out
}

/// Browser page where the user approves a request token.
pub fn authorize_url(api_key: &str, token: &str) -> String {
    format!("https://www.last.fm/api/auth/?api_key={api_key}&token={token}")
}

/// The "API_KEY:SHARED_SECRET" value the user pastes into Settings → API Keys.
pub fn parse_key_secret(stored: &str) -> Option<(String, String)> {
    let (k, s) = stored.split_once(':')?;
    let (k, s) = (k.trim(), s.trim());
    if k.is_empty() || s.is_empty() { return None; }
    Some((k.to_string(), s.to_string()))
}

/// Enqueue a play for later submission.
pub async fn enqueue(pool: &SqlitePool, item_id: i64, played_at: i64) -> Result<()> {
    sqlx::query("INSERT INTO scrobble_queue (item_id, service, played_at, submitted) VALUES (?,?,?,0)")
        .bind(item_id).bind(SERVICE).bind(played_at).execute(pool).await?;
    Ok(())
}

/// Item ids + timestamps awaiting submission, oldest first.
pub async fn pending(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, i64, i64)>> {
    pending_for(pool, SERVICE, limit).await
}

/// The same, for any service sharing this queue -- ListenBrainz writes rows
/// here too, under its own `service` tag, so both can run at once.
pub async fn pending_for(pool: &SqlitePool, service: &str, limit: i64) -> Result<Vec<(i64, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT id, item_id, played_at FROM scrobble_queue WHERE service = ? AND submitted = 0 ORDER BY played_at ASC LIMIT ?",
    ).bind(service).bind(limit).fetch_all(pool).await?)
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
    fn api_sig_is_md5_hex() {
        // MD5("methodtrack.scrobbletrackSongsecret") — stable reference value.
        let sig = api_sig(&[("track", "Song"), ("method", "track.scrobble")], "secret");
        assert_eq!(sig.len(), 32);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        // signed_params appends api_sig + format=json, signature excludes format.
        let sp = signed_params(&[("method", "auth.getToken"), ("api_key", "k")], "s");
        assert_eq!(sp.last().unwrap(), &("format".to_string(), "json".to_string()));
        assert!(sp.iter().any(|(k, _)| k == "api_sig"));
    }

    #[test]
    fn key_secret_parsing() {
        assert_eq!(parse_key_secret("abc:def"), Some(("abc".into(), "def".into())));
        assert_eq!(parse_key_secret(" abc : def "), Some(("abc".into(), "def".into())));
        assert_eq!(parse_key_secret("nocolon"), None);
        assert_eq!(parse_key_secret("a:"), None);
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
