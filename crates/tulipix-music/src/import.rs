//! `np.p4.music.import` — library import from iTunes/Apple Music XML +
//! foobar2000 db (preserves play counts + ratings).
//!
//! Parses the iTunes `Library.xml` plist enough to recover the fields users
//! care about losing — play counts and 0–5 star ratings — keyed by file path.
//! Merge matches existing `track_meta` rows by absolute path so analysis done
//! in Tulipix isn't overwritten.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTrack {
    pub path: String,
    pub name: Option<String>,
    pub play_count: i64,
    pub stars: u8, // 0..5
}

/// Value text of the plist element immediately following `<key>key</key>`.
fn plist_value(seg: &str, key: &str) -> Option<String> {
    let k = format!("<key>{key}</key>");
    let after = &seg[seg.find(&k)? + k.len()..];
    let lt = after.find('<')?;
    let rest = &after[lt..];
    let open_end = rest.find('>')?;
    let close = rest[open_end..].find("</")? + open_end;
    Some(rest[open_end + 1..close].to_string())
}

/// iTunes ratings are 0..100 in steps of 20. Convert to 0..5 stars.
pub fn rating_to_stars(itunes_rating: i64) -> u8 {
    ((itunes_rating.clamp(0, 100) + 10) / 20) as u8
}

/// Decode a `file://localhost/...` (percent-encoded) iTunes Location.
pub fn decode_location(loc: &str) -> String {
    let stripped = loc.trim_start_matches("file://localhost").trim_start_matches("file://");
    let bytes = stripped.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&stripped[i + 1..i + 3], 16) {
                out.push(b); i += 3; continue;
            }
        }
        out.push(bytes[i]); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse all track records out of an iTunes Library.xml.
pub fn parse_itunes(xml: &str) -> Vec<ImportedTrack> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<key>Track ID</key>") {
        let after = &rest[s..];
        let end = after[1..].find("<key>Track ID</key>").map(|e| e + 1).unwrap_or(after.len());
        let seg = &after[..end];
        if let Some(loc) = plist_value(seg, "Location") {
            let play_count = plist_value(seg, "Play Count").and_then(|v| v.parse().ok()).unwrap_or(0);
            let stars = plist_value(seg, "Rating").and_then(|v| v.parse::<i64>().ok()).map(rating_to_stars).unwrap_or(0);
            out.push(ImportedTrack {
                path: decode_location(&loc),
                name: plist_value(seg, "Name"),
                play_count, stars,
            });
        }
        rest = &after[end..];
    }
    out
}

/// Merge imported play counts + ratings into matching `track_meta` rows
/// (matched by abs_path). Returns how many rows were updated.
pub async fn merge(pool: &SqlitePool, tracks: &[ImportedTrack]) -> Result<u64> {
    let mut updated = 0;
    for t in tracks {
        let r = sqlx::query(
            "UPDATE track_meta SET play_count = MAX(play_count, ?), rating = MAX(rating, ?)
             WHERE item_id = (SELECT id FROM items WHERE abs_path = ?)",
        ).bind(t.play_count).bind(t.stars as i64).bind(&t.path).execute(pool).await?;
        updated += r.rows_affected();
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    const XML: &str = r#"<plist><dict><key>Tracks</key><dict>
      <key>101</key><dict>
        <key>Track ID</key><integer>101</integer>
        <key>Name</key><string>Hello</string>
        <key>Play Count</key><integer>42</integer>
        <key>Rating</key><integer>100</integer>
        <key>Location</key><string>file://localhost/music/a%20b.flac</string>
      </dict>
      <key>102</key><dict>
        <key>Track ID</key><integer>102</integer>
        <key>Name</key><string>World</string>
        <key>Location</key><string>file:///music/c.mp3</string>
      </dict>
    </dict></dict></plist>"#;

    #[test]
    fn parses_tracks_with_counts() {
        let t = parse_itunes(XML);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].path, "/music/a b.flac");
        assert_eq!(t[0].play_count, 42);
        assert_eq!(t[0].stars, 5);
        assert_eq!(t[1].path, "/music/c.mp3");
        assert_eq!(t[1].play_count, 0);
    }

    #[test]
    fn rating_conversion() {
        assert_eq!(rating_to_stars(100), 5);
        assert_eq!(rating_to_stars(60), 3);
        assert_eq!(rating_to_stars(0), 0);
    }

    #[tokio::test]
    async fn merge_updates_matching_paths() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/music/a b.flac', 0, 1, 0, 'music', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/music/a b.flac'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO track_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        let n = merge(&pool, &parse_itunes(XML)).await.unwrap();
        assert_eq!(n, 1);
        let pc: i64 = sqlx::query_scalar("SELECT play_count FROM track_meta WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(pc, 42);
    }
}
