//! `np.p4.music.lyrics` — LRCLIB fetch + synchronized scroll.
//!
//! Fetches lyrics from LRCLIB (free, no key) and parses the LRC timestamp
//! format into `(ms, line)` rows the UI scrolls through. Persists to the
//! `lyrics` table. The scroll-position lookup (active line for playhead `t`)
//! lives here too since it's the hot path during playback.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;

pub const LRCLIB_BASE: &str = "https://lrclib.net/api";

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Deserialize, Default)]
pub struct LrclibHit {
    #[serde(rename = "syncedLyrics")]
    pub synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics")]
    pub plain_lyrics: Option<String>,
}

/// LRCLIB `get` URL by exact track signature.
pub fn get_url(artist: &str, title: &str, album: &str, duration_s: f64) -> String {
    format!(
        "{LRCLIB_BASE}/get?artist_name={}&track_name={}&album_name={}&duration={}",
        enc(artist), enc(title), enc(album), duration_s.round() as i64
    )
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

/// Parse one `[mm:ss.xx]` (or `[mm:ss]`) tag → milliseconds.
pub fn parse_lrc_ts(tag: &str) -> Option<i64> {
    let t = tag.trim_start_matches('[').trim_end_matches(']');
    let (m, rest) = t.split_once(':')?;
    let mins: i64 = m.trim().parse().ok()?;
    let secs: f64 = rest.trim().parse().ok()?;
    Some(mins * 60_000 + (secs * 1000.0).round() as i64)
}

/// Parse a full LRC document → sorted `(ms, line)` pairs (timed lines only).
pub fn parse_lrc(lrc: &str) -> Vec<(i64, String)> {
    let mut out = Vec::new();
    for line in lrc.lines() {
        let mut rest = line;
        let mut stamps = Vec::new();
        while rest.starts_with('[') {
            if let Some(end) = rest.find(']') {
                let tag = &rest[..=end];
                if let Some(ms) = parse_lrc_ts(tag) { stamps.push(ms); }
                rest = &rest[end + 1..];
            } else { break; }
        }
        let text = rest.trim().to_string();
        for ms in stamps { out.push((ms, text.clone())); }
    }
    out.sort_by_key(|(ms, _)| *ms);
    out
}

/// Active line index for playhead `t_ms` over parsed lines (last line whose
/// timestamp ≤ t). `None` before the first line.
pub fn active_line(lines: &[(i64, String)], t_ms: i64) -> Option<usize> {
    lines.iter().rposition(|(ms, _)| *ms <= t_ms)
}

pub async fn store(pool: &SqlitePool, item_id: i64, content: &str, synced: bool, source: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO lyrics (item_id, synced, content, source, updated) VALUES (?,?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET synced=excluded.synced, content=excluded.content, source=excluded.source, updated=excluded.updated",
    ).bind(item_id).bind(synced as i64).bind(content).bind(source).bind(now()).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn ts_parse() {
        assert_eq!(parse_lrc_ts("[01:23.45]"), Some(83_450));
        assert_eq!(parse_lrc_ts("[00:05]"), Some(5_000));
    }

    #[test]
    fn parse_and_scroll() {
        let lrc = "[00:01.00]First\n[00:03.50]Second\n[bad]ignored\n[00:05.00]Third";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], (3500, "Second".into()));
        assert_eq!(active_line(&lines, 0), None);
        assert_eq!(active_line(&lines, 4000), Some(1));
        assert_eq!(active_line(&lines, 99_000), Some(2));
    }

    #[tokio::test]
    async fn store_roundtrip() {
        let (_t, pool) = open_pool().await;
        let id = add_track(&pool, "/m/a.flac").await;
        store(&pool, id, "[00:01.00]Hi", true, "lrclib").await.unwrap();
        let synced: i64 = sqlx::query_scalar("SELECT synced FROM lyrics WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(synced, 1);
    }
}
