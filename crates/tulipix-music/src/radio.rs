//! `np.p4.music.radio` — internet radio (Shoutcast/Icecast + radio-browser.info).
//!
//! Builds radio-browser.info search/click URLs, parses the station JSON, and
//! persists favourites locally. Recording is delegated to the Tools queue
//! (mpv `--stream-record`); the option builder lives here.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;

pub const RB_BASE: &str = "https://de1.api.radio-browser.info/json";

#[derive(Debug, Clone, Deserialize)]
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
}

/// Search-by-name URL.
pub fn search_url(name: &str, limit: u32) -> String {
    format!("{RB_BASE}/stations/byname/{}?limit={limit}&hidebroken=true", enc(name))
}

/// radio-browser asks clients to POST a "click" when a station is played, for
/// popularity stats.
pub fn click_url(uuid: &str) -> String { format!("{RB_BASE}/url/{uuid}") }

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

/// mpv options to record the live stream to `out` while playing.
pub fn record_options(out: &str) -> Vec<String> {
    vec![format!("--stream-record={out}")]
}

pub async fn add_favourite(pool: &SqlitePool, s: &Station) -> Result<()> {
    sqlx::query(
        "INSERT INTO radio_stations (station_uuid, name, url, favicon, country, tags, favourite) VALUES (?,?,?,?,?,?,1)
         ON CONFLICT(station_uuid) DO UPDATE SET favourite = 1, name=excluded.name, url=excluded.url",
    ).bind(&s.stationuuid).bind(&s.name).bind(&s.url).bind(&s.favicon).bind(&s.country).bind(&s.tags).execute(pool).await?;
    Ok(())
}

pub async fn remove_favourite(pool: &SqlitePool, uuid: &str) -> Result<()> {
    sqlx::query("UPDATE radio_stations SET favourite = 0 WHERE station_uuid = ?").bind(uuid).execute(pool).await?;
    Ok(())
}

pub async fn favourites(pool: &SqlitePool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as("SELECT station_uuid, name, url FROM radio_stations WHERE favourite = 1 ORDER BY name COLLATE NOCASE")
        .fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn url_builders() {
        assert!(search_url("BBC Radio", 10).contains("/byname/BBC%20Radio"));
        assert_eq!(click_url("abc"), format!("{RB_BASE}/url/abc"));
        assert_eq!(record_options("/tmp/r.mka"), vec!["--stream-record=/tmp/r.mka".to_string()]);
    }

    #[tokio::test]
    async fn favourite_toggle() {
        let (_t, pool) = open_pool().await;
        let s = Station { stationuuid: "u1".into(), name: "Jazz FM".into(), url: "http://s/1".into(), favicon: "".into(), country: "UK".into(), tags: "jazz".into() };
        add_favourite(&pool, &s).await.unwrap();
        assert_eq!(favourites(&pool).await.unwrap().len(), 1);
        remove_favourite(&pool, "u1").await.unwrap();
        assert!(favourites(&pool).await.unwrap().is_empty());
    }
}
