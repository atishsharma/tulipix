//! Subtitle search for Stream Plus — Wyzie and SubDL.
//!
//! Two more sources for the video section's existing subtitle handling, not a
//! second subtitle system: this returns URLs, and the caller hands them to mpv
//! or writes them beside a download exactly as the rest of the app does.

use anyhow::{Context, Result};
use serde_json::Value;

use super::{EpisodeRef, prefs};

const WYZIE: &str = "https://subs.wyzie.ru/search";
const SUBDL: &str = "https://api.subdl.com/api/v1/subtitles";

/// One subtitle the user can pick.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub label: String,
    /// ISO 639-1 where the source gave one.
    pub lang: String,
    pub url: String,
    pub source: &'static str,
    /// True for SDH / forced tracks, which are rarely what someone wants first.
    pub special: bool,
}

/// Everything both enabled sources know about this episode, preferred language
/// first. Never errors on one source being down — a missing subtitle is not a
/// reason to fail playback.
pub async fn search(ep: &EpisodeRef) -> Vec<Track> {
    let want = prefs::subs_lang();
    let mut out = Vec::new();
    if prefs::subs_wyzie() {
        match wyzie(ep, &want).await {
            Ok(mut t) => out.append(&mut t),
            Err(e) => tracing::debug!(error = %e, "splus: wyzie subtitle search failed"),
        }
    }
    if prefs::subs_subdl() {
        match subdl(ep, &want).await {
            Ok(mut t) => out.append(&mut t),
            Err(e) => tracing::debug!(error = %e, "splus: subdl subtitle search failed"),
        }
    }
    rank(out, &want)
}

/// Preferred language first, ordinary tracks before SDH, and no duplicate URLs.
fn rank(mut tracks: Vec<Track>, want: &str) -> Vec<Track> {
    tracks.sort_by_key(|t| {
        (
            u8::from(!t.lang.eq_ignore_ascii_case(want)),
            u8::from(t.special),
            t.label.to_ascii_lowercase(),
        )
    });
    tracks.dedup_by(|a, b| a.url == b.url);
    tracks
}

async fn wyzie(ep: &EpisodeRef, lang: &str) -> Result<Vec<Track>> {
    let Some(tmdb) = ep.title.tmdb_id else {
        // Wyzie indexes by TMDB id; an anime-lane title without one has nothing
        // to look up.
        return Ok(Vec::new());
    };
    let mut req = tulipix_core::net::http().get(WYZIE).query(&[
        ("id", tmdb.to_string()),
        ("language", lang.to_string()),
    ]);
    if ep.title.is_series() {
        req = req.query(&[
            ("season", ep.season.to_string()),
            ("episode", ep.episode.to_string()),
        ]);
    }
    let resp = req.send().await.context("wyzie: request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("wyzie: HTTP {}", resp.status().as_u16());
    }
    let v: Value = resp.json().await.context("wyzie: bad JSON")?;
    let arr = v.as_array().cloned().unwrap_or_default();
    Ok(arr
        .iter()
        .filter_map(|t| {
            let url = t.get("url").and_then(Value::as_str)?.to_string();
            let display = t
                .get("display")
                .or_else(|| t.get("language"))
                .and_then(Value::as_str)
                .unwrap_or("Subtitle")
                .to_string();
            let code = t
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or(lang)
                .to_string();
            Some(Track {
                special: is_special(&display),
                label: display,
                lang: code,
                url,
                source: "wyzie",
            })
        })
        .collect())
}

async fn subdl(ep: &EpisodeRef, lang: &str) -> Result<Vec<Track>> {
    let key = prefs::subdl_key();
    if key.is_empty() {
        // Enabled but unconfigured is a settings problem, not a search failure.
        return Ok(Vec::new());
    }
    let mut params: Vec<(String, String)> = vec![
        ("api_key".into(), key),
        ("languages".into(), lang.to_uppercase()),
        ("subs_per_page".into(), "20".into()),
    ];
    match ep.title.tmdb_id {
        Some(id) => params.push(("tmdb_id".into(), id.to_string())),
        None => params.push(("film_name".into(), ep.title.display_title().to_string())),
    }
    if ep.title.is_series() {
        params.push(("season_number".into(), ep.season.to_string()));
        params.push(("episode_number".into(), ep.episode.to_string()));
    }
    let resp = tulipix_core::net::http()
        .get(SUBDL)
        .query(&params)
        .send()
        .await
        .context("subdl: request failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("subdl: HTTP {}", resp.status().as_u16());
    }
    let v: Value = resp.json().await.context("subdl: bad JSON")?;
    let arr = v
        .get("subtitles")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(arr
        .iter()
        .filter_map(|t| {
            let raw = t.get("url").and_then(Value::as_str)?;
            // SubDL returns a site-relative download path.
            let url = if raw.starts_with("http") {
                raw.to_string()
            } else {
                format!("https://dl.subdl.com{}{raw}", if raw.starts_with('/') { "" } else { "/" })
            };
            let name = t
                .get("release_name")
                .or_else(|| t.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("Subtitle")
                .to_string();
            let code = t
                .get("lang")
                .or_else(|| t.get("language"))
                .and_then(Value::as_str)
                .unwrap_or(lang)
                .to_lowercase();
            Some(Track {
                special: is_special(&name),
                label: name,
                lang: code,
                url,
                source: "subdl",
            })
        })
        .collect())
}

fn is_special(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    l.contains("sdh") || l.contains("forced") || l.contains("hearing")
}

/// Download one track next to a video file, as `<stem>.<lang>.srt`.
pub async fn save_beside(track: &Track, video: &std::path::Path) -> Result<std::path::PathBuf> {
    let body = tulipix_core::net::http()
        .get(&track.url)
        .send()
        .await
        .context("subtitle download failed")?
        .bytes()
        .await
        .context("subtitle body was truncated")?;
    let stem = video.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let lang = if track.lang.is_empty() { "und".into() } else { track.lang.clone() };
    let dest = video.with_file_name(format!("{stem}.{lang}.srt"));
    tokio::fs::write(&dest, &body).await.context("could not write the subtitle")?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(label: &str, lang: &str, url: &str) -> Track {
        Track { special: is_special(label), label: label.into(), lang: lang.into(), url: url.into(), source: "wyzie" }
    }

    #[test]
    fn preferred_language_sorts_first() {
        let got = rank(vec![t("German", "de", "a"), t("English", "en", "b")], "en");
        assert_eq!(got[0].lang, "en");
    }

    #[test]
    fn sdh_sinks_below_a_plain_track_of_the_same_language() {
        let got = rank(vec![t("English SDH", "en", "a"), t("English", "en", "b")], "en");
        assert_eq!(got[0].label, "English");
    }

    #[test]
    fn duplicate_urls_collapse() {
        let got = rank(vec![t("English", "en", "same"), t("English", "en", "same")], "en");
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn forced_and_hearing_impaired_count_as_special() {
        assert!(is_special("English [Forced]"));
        assert!(is_special("English (Hearing Impaired)"));
        assert!(!is_special("English"));
    }
}
