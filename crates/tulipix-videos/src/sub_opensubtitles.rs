//! OpenSubtitles REST API integration.
//!
//! Two search paths:
//!  * **Hash search** — the canonical OpenSubtitles file hash (Tomte/OSDb
//!    algorithm: first 64 KiB XOR last 64 KiB + size). Most accurate.
//!  * **Query search** — by title + season/episode (+ optional year).
//!
//! Downloads write the chosen subtitle next to the video as a sibling file
//! so the local autopick (`sub_local`) sees it on next launch.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const OS_BASE: &str = "https://api.opensubtitles.com/api/v1";
pub const OS_HASH_CHUNK: u64 = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsCredentials {
    pub api_key: String,
    pub user_agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsSubtitle {
    pub file_id: i64,
    pub language: String,
    pub release: Option<String>,
    pub download_count: Option<i64>,
    pub from_trusted: bool,
}

/// OSDb file hash — 64-bit checksum of first + last 64 KiB plus the file
/// length. Matches what OpenSubtitles expects in the `moviehash` field.
pub fn osdb_hash(path: &Path) -> Result<(u64, u64)> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let len = f.metadata()?.len();
    if len < OS_HASH_CHUNK { anyhow::bail!("file too small for OSDb hash"); }

    let mut hash: u64 = len;
    hash = hash.wrapping_add(read_u64_sum(&mut f, 0, OS_HASH_CHUNK)?);
    hash = hash.wrapping_add(read_u64_sum(&mut f, len - OS_HASH_CHUNK, OS_HASH_CHUNK)?);
    Ok((hash, len))
}

fn read_u64_sum(f: &mut std::fs::File, offset: u64, len: u64) -> Result<u64> {
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf)?;
    let mut sum: u64 = 0;
    for chunk in buf.chunks_exact(8) {
        let bytes: [u8; 8] = chunk.try_into().unwrap();
        sum = sum.wrapping_add(u64::from_le_bytes(bytes));
    }
    Ok(sum)
}

pub struct OpenSubtitlesClient {
    creds: OsCredentials,
    http: reqwest::Client,
}

impl OpenSubtitlesClient {
    pub fn new(creds: OsCredentials) -> Self {
        Self { creds, http: reqwest::Client::new() }
    }

    /// Search by OSDb hash. Empty list = no match.
    pub async fn search_by_hash(&self, hash: u64, language: &str) -> Result<Vec<OsSubtitle>> {
        let url = format!("{}/subtitles", OS_BASE);
        let resp = self.http.get(url)
            .header("Api-Key", &self.creds.api_key)
            .header("User-Agent", &self.creds.user_agent)
            .query(&[("moviehash", format!("{hash:016x}")), ("languages", language.into())])
            .send().await?
            .error_for_status()?;
        parse_search(&resp.json::<serde_json::Value>().await?)
    }

    pub async fn search_by_query(&self, query: &str, language: &str, season: Option<i64>, episode: Option<i64>) -> Result<Vec<OsSubtitle>> {
        let url = format!("{}/subtitles", OS_BASE);
        let mut req = self.http.get(url)
            .header("Api-Key", &self.creds.api_key)
            .header("User-Agent", &self.creds.user_agent)
            .query(&[("query", query), ("languages", language)]);
        if let Some(s) = season { req = req.query(&[("season_number", s.to_string())]); }
        if let Some(e) = episode { req = req.query(&[("episode_number", e.to_string())]); }
        let resp = req.send().await?.error_for_status()?;
        parse_search(&resp.json::<serde_json::Value>().await?)
    }

    /// Step 1 of download: trade `file_id` for a temporary download URL.
    pub async fn download_link(&self, file_id: i64) -> Result<String> {
        let url = format!("{}/download", OS_BASE);
        let resp = self.http.post(url)
            .header("Api-Key", &self.creds.api_key)
            .header("User-Agent", &self.creds.user_agent)
            .json(&serde_json::json!({ "file_id": file_id }))
            .send().await?
            .error_for_status()?;
        let v: serde_json::Value = resp.json().await?;
        v.get("link").and_then(|x| x.as_str()).map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("missing `link` in download response"))
    }

    /// Step 2 of download: fetch the bytes, write to a sibling path.
    pub async fn save_as_sibling(&self, video: &Path, link: &str, language: &str, ext: &str) -> Result<PathBuf> {
        let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
        let parent = video.parent().unwrap_or(Path::new("."));
        let out = parent.join(format!("{stem}.{language}.{ext}"));
        let bytes = reqwest::get(link).await?.error_for_status()?.bytes().await?;
        std::fs::write(&out, &bytes).with_context(|| format!("write {}", out.display()))?;
        Ok(out)
    }
}

fn parse_search(v: &serde_json::Value) -> Result<Vec<OsSubtitle>> {
    let arr = v.get("data").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.len());
    for entry in arr {
        let attrs = entry.get("attributes").cloned().unwrap_or_default();
        let language = attrs.get("language").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let release  = attrs.get("release").and_then(|x| x.as_str()).map(str::to_string);
        let download_count = attrs.get("download_count").and_then(|x| x.as_i64());
        let from_trusted = attrs.get("from_trusted").and_then(|x| x.as_bool()).unwrap_or(false);
        let file_id = attrs.get("files").and_then(|f| f.as_array())
            .and_then(|a| a.first())
            .and_then(|f| f.get("file_id"))
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        if file_id != 0 {
            out.push(OsSubtitle { file_id, language, release, download_count, from_trusted });
        }
    }
    // Trusted first, then by download_count desc.
    out.sort_by(|a, b| b.from_trusted.cmp(&a.from_trusted)
        .then(b.download_count.unwrap_or(0).cmp(&a.download_count.unwrap_or(0))));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osdb_hash_matches_reference_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.bin");
        // 128 KiB file with predictable content.
        let buf = vec![0xAAu8; 128 * 1024];
        std::fs::write(&p, &buf).unwrap();
        let (h, len) = osdb_hash(&p).unwrap();
        assert_eq!(len, 128 * 1024);
        // 0xAA repeated 8 times → 0xAAAAAAAAAAAAAAAA, summed 2 * (65536/8) = 16384 times.
        let single: u64 = 0xAAAA_AAAA_AAAA_AAAA;
        let per_chunk: u64 = single.wrapping_mul(8192);
        let expected = (128u64 * 1024).wrapping_add(per_chunk).wrapping_add(per_chunk);
        assert_eq!(h, expected);
    }

    #[test]
    fn small_file_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("small.bin");
        std::fs::write(&p, b"too small").unwrap();
        let r = osdb_hash(&p);
        assert!(r.is_err());
    }

    #[test]
    fn parse_search_orders_trusted_first() {
        let v = serde_json::json!({
            "data": [
                { "attributes": { "language": "en", "from_trusted": false, "download_count": 1000,
                                  "files": [{"file_id": 1}] } },
                { "attributes": { "language": "en", "from_trusted": true,  "download_count": 5,
                                  "files": [{"file_id": 2}] } }
            ]
        });
        let r = parse_search(&v).unwrap();
        assert_eq!(r[0].file_id, 2);
        assert_eq!(r[1].file_id, 1);
    }

    #[test]
    fn parse_search_skips_entries_without_file_id() {
        let v = serde_json::json!({"data": [ { "attributes": { "language": "en" } } ]});
        let r = parse_search(&v).unwrap();
        assert!(r.is_empty());
    }
}
