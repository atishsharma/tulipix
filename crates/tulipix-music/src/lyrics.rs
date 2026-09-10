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


/// One track whose words contain the search, and where in it they are sung.
#[derive(Debug, Clone, PartialEq)]
pub struct WordHit {
    pub item_id: i64,
    /// The matching line, with any LRC timestamp stripped off the front.
    pub line: String,
    /// Where the line is sung, in milliseconds. -1 when the stored lyrics are
    /// plain text with no timings — the hit is still worth showing, it just
    /// cannot be seeked to.
    pub ms: i64,
}

/// Escape a user's text for a `LIKE` pattern.
///
/// Without this a search for `50%` matches every song in the library, and one
/// for `_` matches every song with any character at that position.
fn like_escape(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 8);
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Tracks whose lyrics contain `query`, with the line and its timestamp.
///
/// A `LIKE` scan rather than an FTS index. A personal library has thousands of
/// lyric rows, not millions, and SQLite reads all of them faster than the
/// keystroke that asked; an FTS5 table would be a second copy of the text, a
/// migration, and a trigger to keep it in step — for a search that already
/// returns instantly. If someone ever has a library where this is slow, the
/// index is the fix and this is the thing to replace.
///
/// One hit per track: the first line that matches. A chorus repeating the
/// phrase eight times should be one result, not eight.
pub async fn search(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<WordHit>> {
    let needle = query.trim();
    // Two characters would match most of the library and be no use as a search.
    if needle.chars().count() < 3 {
        return Ok(Vec::new());
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT item_id, content FROM lyrics \
         WHERE content LIKE ? ESCAPE '\\' LIMIT ?",
    )
    .bind(format!("%{}%", like_escape(needle)))
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let lower = needle.to_lowercase();
    Ok(rows
        .into_iter()
        .filter_map(|(item_id, content)| {
            // Timed lines first, so a synced sheet reports where to seek to.
            // `parse_lrc` returns nothing for plain text, and the raw scan below
            // still finds the line.
            let timed = parse_lrc(&content);
            if let Some((ms, line)) = timed
                .iter()
                .find(|(_, l)| l.to_lowercase().contains(&lower))
            {
                return Some(WordHit { item_id, line: line.clone(), ms: *ms });
            }
            let line = content
                .lines()
                .map(str::trim)
                .find(|l| l.to_lowercase().contains(&lower))?;
            Some(WordHit { item_id, line: line.to_string(), ms: -1 })
        })
        .collect())
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

    #[tokio::test]
    async fn searching_the_words_finds_the_line_and_the_second() {
        let (_t, pool) = open_pool().await;
        let synced = add_track(&pool, "/m/take-on-me.flac").await;
        let plain = add_track(&pool, "/m/other.flac").await;
        let quiet = add_track(&pool, "/m/instrumental.flac").await;
        store(&pool, synced, "[00:10.00]Talking away\n[01:14.50]So we put our hands together\n[01:20.00]So we put our hands together", true, "lrclib").await.unwrap();
        store(&pool, plain, "Hands together in the dark", false, "manual").await.unwrap();
        store(&pool, quiet, "[00:05.00]La la la", true, "lrclib").await.unwrap();

        let hits = search(&pool, "hands together", 20).await.unwrap();
        assert_eq!(hits.len(), 2, "the instrumental does not match");

        let timed = hits.iter().find(|h| h.item_id == synced).unwrap();
        assert_eq!(timed.line, "So we put our hands together");
        assert_eq!(timed.ms, 74_500, "seeks to 1:14.5, not to the repeat");

        // Plain lyrics still hit; they just have nowhere to seek to.
        let flat = hits.iter().find(|h| h.item_id == plain).unwrap();
        assert_eq!(flat.ms, -1);

        // Case does not matter, and two characters is not a search.
        assert_eq!(search(&pool, "HANDS TOGETHER", 20).await.unwrap().len(), 2);
        assert!(search(&pool, "ha", 20).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_wildcard_is_text_not_a_pattern() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        store(&pool, a, "Nothing to see here", false, "manual").await.unwrap();
        store(&pool, b, "Give me 100% of it", false, "manual").await.unwrap();
        // Unescaped, "100%" as a LIKE pattern would also match the first row.
        let hits = search(&pool, "100%", 20).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_id, b);
    }

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
