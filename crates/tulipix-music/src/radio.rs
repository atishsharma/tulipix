//! `np.p4.music.radio` — internet radio (Shoutcast/Icecast + radio-browser.info).
//!
//! Builds radio-browser.info search/click URLs, parses the station JSON, and
//! persists favourites + recents locally. Recording is delegated to the Tools
//! queue (mpv `--stream-record`); the option builder lives here.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;
use tulipix_core::util::url_encode as enc;

pub const RB_BASE: &str = "https://de1.api.radio-browser.info/json";

/// Schema for the standalone `radio.db` section. Self-contained (just saved
/// stations), so it lives in its own file rather than bloating `music.db`.
pub const RADIO_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS radio_stations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_uuid TEXT UNIQUE,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    favicon      TEXT,
    country      TEXT,
    tags         TEXT,
    favourite    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS radio_fav_idx ON radio_stations(favourite);

-- Browse cache: one snapshot of every curated preset's station list, written
-- by the explicit Refresh action so category pages open instantly offline.
CREATE TABLE IF NOT EXISTS radio_cache (
    preset       TEXT NOT NULL,
    pos          INTEGER NOT NULL,
    station_uuid TEXT NOT NULL,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    favicon      TEXT,
    country      TEXT,
    tags         TEXT,
    codec        TEXT,
    bitrate      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (preset, pos)
);
"#;

/// Apply the radio schema to a (radio.db) pool. Idempotent.
pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(RADIO_SCHEMA).execute(pool).await?;
    // v2 — recents need a play timestamp. ALTER fails once the column exists;
    // that's the idempotence (sqlite has no ADD COLUMN IF NOT EXISTS).
    let _ = sqlx::query("ALTER TABLE radio_stations ADD COLUMN last_played INTEGER")
        .execute(pool).await;
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Station {
    pub stationuuid: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub favicon: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub votes: i64,
}

/// Curated browse presets for the Radio home — label + radio-browser
/// `/stations/search` query fragment. India-first: every major Indian
/// language/genre with real coverage on radio-browser, then a couple of
/// global staples. Results are always ordered by votes (see `browse_url`).
pub const PRESETS: &[(&str, &str)] = &[
    ("⭐ Top India",   "countrycode=IN"),
    ("🎬 Bollywood",   "tag=bollywood"),
    ("🎙 Hindi",       "language=hindi"),
    ("🕰 Retro 90s",   "name=retro&language=hindi"),
    ("🥁 Punjabi",     "language=punjabi"),
    ("🌴 Tamil",       "language=tamil"),
    ("🎥 Telugu",      "language=telugu"),
    ("🌊 Malayalam",   "language=malayalam"),
    ("🏵 Kannada",     "language=kannada"),
    ("🎭 Marathi",     "language=marathi"),
    ("🎼 Bengali",     "language=bengali"),
    ("🌙 Urdu·Ghazal", "language=urdu"),
    ("🎻 Classical",   "tag=classical&countrycode=IN"),
    ("☕ Lofi·Chill",  "tag=lofi"),
    ("🌍 Global Hits", "tag=pop"),
];

/// Browse URL for a preset/query fragment — top-voted working stations first.
pub fn browse_url(query: &str, limit: u32) -> String {
    format!("{RB_BASE}/stations/search?{query}&order=votes&reverse=true&hidebroken=true&limit={limit}")
}

/// Search-by-name URL (free-text search box).
pub fn search_url(name: &str, limit: u32) -> String {
    format!("{RB_BASE}/stations/search?name={}&order=votes&reverse=true&hidebroken=true&limit={limit}", enc(name))
}

/// radio-browser asks clients to POST a "click" when a station is played, for
/// popularity stats.
pub fn click_url(uuid: &str) -> String { format!("{RB_BASE}/url/{uuid}") }

/// mpv options to record the live stream to `out` while playing.
pub fn record_options(out: &str) -> Vec<String> {
    vec![format!("--stream-record={out}")]
}

/// Upsert a station row without touching its favourite flag (used by recents).
async fn upsert(pool: &SqlitePool, s: &Station) -> Result<()> {
    sqlx::query(
        "INSERT INTO radio_stations (station_uuid, name, url, favicon, country, tags) VALUES (?,?,?,?,?,?)
         ON CONFLICT(station_uuid) DO UPDATE SET name=excluded.name, url=excluded.url, favicon=excluded.favicon",
    ).bind(&s.stationuuid).bind(&s.name).bind(&s.url).bind(&s.favicon).bind(&s.country).bind(&s.tags).execute(pool).await?;
    Ok(())
}

pub async fn add_favourite(pool: &SqlitePool, s: &Station) -> Result<()> {
    upsert(pool, s).await?;
    sqlx::query("UPDATE radio_stations SET favourite = 1 WHERE station_uuid = ?")
        .bind(&s.stationuuid).execute(pool).await?;
    Ok(())
}

pub async fn remove_favourite(pool: &SqlitePool, uuid: &str) -> Result<()> {
    sqlx::query("UPDATE radio_stations SET favourite = 0 WHERE station_uuid = ?").bind(uuid).execute(pool).await?;
    Ok(())
}

/// Stamp a station as just-played (drives the Recent tab). Inserts the row if
/// the station was never saved before. Recents are capped at 50 — older play
/// stamps are cleared (the row survives if it's also a favourite).
pub async fn touch_played(pool: &SqlitePool, s: &Station, now: i64) -> Result<()> {
    upsert(pool, s).await?;
    sqlx::query("UPDATE radio_stations SET last_played = ? WHERE station_uuid = ?")
        .bind(now).bind(&s.stationuuid).execute(pool).await?;
    sqlx::query(
        "UPDATE radio_stations SET last_played = NULL WHERE last_played IS NOT NULL
         AND station_uuid NOT IN (SELECT station_uuid FROM radio_stations
             WHERE last_played IS NOT NULL ORDER BY last_played DESC LIMIT 50)")
        .execute(pool).await?;
    Ok(())
}

/// Wipe the Recent list. Non-favourite rows are deleted outright (they only
/// existed to carry the play stamp); favourites just lose their stamp.
pub async fn clear_recents(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM radio_stations WHERE favourite = 0 AND last_played IS NOT NULL")
        .execute(pool).await?;
    sqlx::query("UPDATE radio_stations SET last_played = NULL").execute(pool).await?;
    Ok(())
}

fn row_to_station(r: (String, String, String, String, String, String)) -> Station {
    Station { stationuuid: r.0, name: r.1, url: r.2, favicon: r.3, country: r.4, tags: r.5, ..Default::default() }
}

pub async fn favourites(pool: &SqlitePool) -> Result<Vec<Station>> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT station_uuid, name, url, COALESCE(favicon,''), COALESCE(country,''), COALESCE(tags,'')
         FROM radio_stations WHERE favourite = 1 ORDER BY name COLLATE NOCASE")
        .fetch_all(pool).await?;
    Ok(rows.into_iter().map(row_to_station).collect())
}

pub async fn recents(pool: &SqlitePool, limit: u32) -> Result<Vec<Station>> {
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT station_uuid, name, url, COALESCE(favicon,''), COALESCE(country,''), COALESCE(tags,'')
         FROM radio_stations WHERE last_played IS NOT NULL ORDER BY last_played DESC LIMIT ?")
        .bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(row_to_station).collect())
}

/// Replace one preset's cached station list (Refresh writes all presets).
pub async fn save_cache(pool: &SqlitePool, preset: &str, stations: &[Station]) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM radio_cache WHERE preset = ?").bind(preset).execute(&mut *tx).await?;
    for (i, s) in stations.iter().enumerate() {
        sqlx::query(
            "INSERT INTO radio_cache (preset, pos, station_uuid, name, url, favicon, country, tags, codec, bitrate)
             VALUES (?,?,?,?,?,?,?,?,?,?)")
            .bind(preset).bind(i as i64).bind(&s.stationuuid).bind(&s.name).bind(&s.url)
            .bind(&s.favicon).bind(&s.country).bind(&s.tags).bind(&s.codec).bind(s.bitrate as i64)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// One preset's cached list, in fetch order (votes-desc at refresh time).
pub async fn load_cache(pool: &SqlitePool, preset: &str) -> Result<Vec<Station>> {
    let rows: Vec<(String, String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT station_uuid, name, url, COALESCE(favicon,''), COALESCE(country,''), COALESCE(tags,''),
                COALESCE(codec,''), bitrate
         FROM radio_cache WHERE preset = ? ORDER BY pos")
        .bind(preset).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| Station {
        stationuuid: r.0, name: r.1, url: r.2, favicon: r.3, country: r.4, tags: r.5,
        codec: r.6, bitrate: r.7 as u32, ..Default::default()
    }).collect())
}

/// Universal search across the WHOLE cache (every preset) + saved stations —
/// name/tag match, deduped by uuid then by name, name-ordered. This is what
/// the radio search box hits first, so search covers all sections offline.
pub async fn search_cache(pool: &SqlitePool, q: &str) -> Result<Vec<Station>> {
    let like = format!("%{}%", q.trim());
    let rows: Vec<(String, String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT station_uuid, name, url, COALESCE(favicon,''), COALESCE(country,''), COALESCE(tags,''),
                COALESCE(codec,''), bitrate
         FROM radio_cache WHERE name LIKE ? OR tags LIKE ?
         UNION
         SELECT station_uuid, name, url, COALESCE(favicon,''), COALESCE(country,''), COALESCE(tags,''),
                '', 0
         FROM radio_stations WHERE name LIKE ? OR tags LIKE ?
         ORDER BY name COLLATE NOCASE, bitrate DESC")
        .bind(&like).bind(&like).bind(&like).bind(&like)
        .fetch_all(pool).await?;
    let mut seen_uuid = std::collections::HashSet::new();
    let mut seen_name = std::collections::HashSet::new();
    Ok(rows.into_iter()
        .filter(|r| seen_uuid.insert(r.0.clone()) && seen_name.insert(r.1.trim().to_lowercase()))
        .map(|r| Station {
            stationuuid: r.0, name: r.1, url: r.2, favicon: r.3, country: r.4, tags: r.5,
            codec: r.6, bitrate: r.7 as u32, ..Default::default()
        }).collect())
}

/// Station count per cached preset (drives the category tile badges).
pub async fn cache_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as("SELECT preset, COUNT(*) FROM radio_cache GROUP BY preset")
        .fetch_all(pool).await?)
}

/// The set of favourited uuids — to flag hearts in fetched browse lists.
pub async fn favourite_uuids(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT station_uuid FROM radio_stations WHERE favourite = 1")
        .fetch_all(pool).await?)
}

/// A user-pasted custom station (direct stream URL, e.g. an AIR/Akashvani
/// mount not in radio-browser). Keyed by its URL so re-adding dedupes.
pub fn custom_station(name: &str, url: &str) -> Station {
    Station {
        stationuuid: format!("custom:{url}"),
        name: name.trim().to_string(),
        url: url.trim().to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn url_builders() {
        assert!(search_url("BBC Radio", 10).contains("name=BBC%20Radio"));
        assert!(browse_url("countrycode=IN", 60).contains("countrycode=IN&order=votes"));
        assert_eq!(click_url("abc"), format!("{RB_BASE}/url/abc"));
        assert_eq!(record_options("/tmp/r.mka"), vec!["--stream-record=/tmp/r.mka".to_string()]);
        // Every preset query must be a bare fragment (no leading ?/&).
        for (label, q) in PRESETS {
            assert!(!q.starts_with('?') && !q.starts_with('&'), "bad preset {label}");
        }
    }

    #[test]
    fn custom_station_dedupe_key() {
        let s = custom_station(" AIR Vividh Bharati ", " http://air.pc.cdn.bitgravity.com/air/live/pbaudio001/playlist.m3u8 ");
        assert_eq!(s.name, "AIR Vividh Bharati");
        assert!(s.stationuuid.starts_with("custom:"));
    }

    #[tokio::test]
    async fn browse_cache_roundtrip() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let a = Station { stationuuid: "a".into(), name: "A FM".into(), url: "http://a".into(), codec: "MP3".into(), bitrate: 128, ..Default::default() };
        let b = Station { stationuuid: "b".into(), name: "B FM".into(), url: "http://b".into(), ..Default::default() };
        save_cache(&pool, "tag=bollywood", &[a.clone(), b]).await.unwrap();
        let got = load_cache(&pool, "tag=bollywood").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "A FM");
        assert_eq!(got[0].bitrate, 128);
        // Replace semantics: re-saving with one station drops the other.
        save_cache(&pool, "tag=bollywood", &[a]).await.unwrap();
        assert_eq!(load_cache(&pool, "tag=bollywood").await.unwrap().len(), 1);
        assert_eq!(cache_counts(&pool).await.unwrap(), vec![("tag=bollywood".to_string(), 1)]);
        assert!(load_cache(&pool, "language=hindi").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cache_search_is_universal() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let a = Station { stationuuid: "a".into(), name: "Mirchi Tamil".into(), url: "http://a".into(), bitrate: 128, ..Default::default() };
        let b = Station { stationuuid: "b".into(), name: "Mirchi Hindi".into(), url: "http://b".into(), tags: "bollywood".into(), ..Default::default() };
        save_cache(&pool, "language=tamil", &[a.clone()]).await.unwrap();
        save_cache(&pool, "language=hindi", &[b.clone()]).await.unwrap();
        // Matches across BOTH presets…
        assert_eq!(search_cache(&pool, "mirchi").await.unwrap().len(), 2);
        // …by tag too, and favourites join in (deduped by uuid/name).
        assert_eq!(search_cache(&pool, "bollywood").await.unwrap().len(), 1);
        add_favourite(&pool, &a).await.unwrap();
        let hits = search_cache(&pool, "Mirchi Tamil").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].bitrate, 128, "cache row (with quality) wins the dedupe");
    }

    #[tokio::test]
    async fn favourite_toggle_and_recents() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let s = Station { stationuuid: "u1".into(), name: "Jazz FM".into(), url: "http://s/1".into(), ..Default::default() };
        add_favourite(&pool, &s).await.unwrap();
        assert_eq!(favourites(&pool).await.unwrap().len(), 1);
        assert_eq!(favourite_uuids(&pool).await.unwrap(), vec!["u1".to_string()]);
        // Recents: a non-favourite station played once shows up, ordered by time.
        let s2 = Station { stationuuid: "u2".into(), name: "Mirchi".into(), url: "http://s/2".into(), ..Default::default() };
        touch_played(&pool, &s2, 100).await.unwrap();
        touch_played(&pool, &s, 200).await.unwrap();
        let r = recents(&pool, 10).await.unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].stationuuid, "u1");
        // Unfavourite keeps the row (it may still be a recent).
        remove_favourite(&pool, "u1").await.unwrap();
        assert!(favourites(&pool).await.unwrap().is_empty());
        assert_eq!(recents(&pool, 10).await.unwrap().len(), 2);
    }
}
