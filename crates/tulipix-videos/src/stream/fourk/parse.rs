//! 4KHDHub page parsing, mapped straight onto the Stream tab's own types.
//!
//! Ported from MovieBox-Tui `src/providers/fourkhdhub/parser.rs` (MIT OR
//! Apache-2.0, https://github.com/mesamirh/MovieBox-Tui). Two changes from
//! upstream:
//!
//! * Upstream renders its scraped records back into MovieBox's JSON envelope so
//!   the rest of its TUI can stay MovieBox-shaped. There is nothing to be
//!   MovieBox-shaped for here — the Stream tab has its own [`SearchHit`],
//!   [`Details`] and [`StreamFile`], so the parsers build those directly and the
//!   JSON round trip disappears.
//! * `scraper` is replaced by [`super::dom`] (see that module for why).

use super::dom::{self, El};
use crate::stream::{Details, EpisodeInfo, Season, SearchHit, StreamFile, human_size};
use std::collections::BTreeMap;

/// One scraped downloadable, before it is split into per-mirror
/// [`StreamFile`]s.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Release {
    pub filename: String,
    pub quality: Option<String>,
    pub codec: String,
    pub language: String,
    pub size_bytes: Option<u64>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// `(label, resolver url)`, in the order the page listed them.
    pub mirrors: Vec<(String, String)>,
}

// ---- search ----

/// Search results. `base_host` is the site's host; links pointing anywhere else
/// are dropped, because a scraped page is an untrusted source of URLs.
pub fn parse_search(html: &str, base_host: &str) -> Vec<SearchHit> {
    let mut out = Vec::new();
    for card in dom::find_all(html, "a", Some("movie-card")) {
        let Some(href) = dom::attr(card.attrs, "href") else { continue };
        let Some(path) = same_site_path(&href, base_host) else { continue };
        let title = dom::find(card.inner, "*", Some("movie-card-title"))
            .map(|e| dom::text(e.inner))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let meta = dom::find(card.inner, "*", Some("movie-card-meta"))
            .map(|e| dom::text(e.inner))
            .unwrap_or_default();
        out.push(SearchHit {
            id: path,
            title,
            year: first_year(&meta).unwrap_or_default(),
            cover: dom::find(card.inner, "img", None)
                .and_then(|e| dom::attr(e.attrs, "src"))
                .unwrap_or_default(),
            // The site encodes kind in the URL and nowhere else.
            is_series: href.contains("-series-"),
            // 4KHDHub keeps a show on one page, so there is never a set of
            // per-season subjects to fold together.
            season_subjects: Vec::new(),
        });
    }
    out
}

/// A same-site link reduced to its path. Anything absolute pointing at another
/// host, or unparseable, is refused.
fn same_site_path(href: &str, base_host: &str) -> Option<String> {
    let rest = href
        .strip_prefix("https://")
        .or_else(|| href.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            // `sub.host` is still the site; `evil-host.com` is not.
            let host = host.split('@').next_back().unwrap_or(host);
            let host = host.split(':').next().unwrap_or(host);
            (host == base_host || host.ends_with(&format!(".{base_host}")))
                .then(|| format!("/{path}"))
        }
        None if href.starts_with('/') => Some(href.to_string()),
        None if href.is_empty() => None,
        None => Some(format!("/{href}")),
    }
}

// ---- details ----

pub fn parse_details(id: &str, html: &str) -> Option<Details> {
    let raw_title = dom::find(html, "h1", None)
        .map(|e| dom::text(e.inner))
        .filter(|t| !t.is_empty())
        .or_else(|| meta_content(html, "og:title"))?;
    let seasons_map = episode_map(html);

    Some(Details {
        id: id.to_string(),
        title: strip_trailing_year(&raw_title),
        overview: dom::find(html, "*", Some("content-section"))
            .and_then(|s| dom::find(s.inner, "p", None).map(|p| dom::text(p.inner)))
            .or_else(|| meta_content(html, "description"))
            .unwrap_or_default(),
        cover: meta_content(html, "og:image").unwrap_or_default(),
        year: metadata(html, "Release:")
            .and_then(|v| first_year(&v))
            .or_else(|| metadata(html, "Last Air:").and_then(|v| first_year(&v)))
            .or_else(|| first_year(&raw_title))
            .unwrap_or_default(),
        rating: dom::find(html, "*", Some("imdb-score"))
            .map(|e| dom::text(e.inner))
            .and_then(|t| {
                t.split_whitespace().next().and_then(|n| n.parse::<f64>().ok())
            })
            .unwrap_or(0.0),
        genre: dom::find_all(html, "*", Some("badge-outline"))
            .iter()
            .filter_map(|e| dom::find(e.inner, "a", None).map(|a| dom::text(a.inner)))
            .filter(|g| is_genre(g))
            .collect::<Vec<_>>()
            .join(", "),
        country: String::new(),
        content_rating: String::new(),
        duration: String::new(),
        is_series: id.contains("-series-"),
        // The site carries one language line for the whole page rather than
        // separate subjects per dub, so there is nothing to list here.
        dubs: Vec::new(),
        seasons: seasons_map
            .iter()
            .map(|(se, eps)| Season {
                number: *se,
                max_ep: eps.iter().copied().max().unwrap_or(0),
            })
            .collect(),
        episodes: seasons_map
            .iter()
            .flat_map(|(se, eps)| {
                eps.iter().map(move |ep| EpisodeInfo {
                    season: *se,
                    number: *ep,
                    // Filenames are the only episode label the page has, and
                    // "S01E04 1080p WEB-DL" is not a title.
                    title: String::new(),
                    still: String::new(),
                })
            })
            .collect(),
    })
}

/// Season → sorted episode numbers, read off the download list. It is the only
/// place the page states which episodes exist.
fn episode_map(html: &str) -> BTreeMap<i64, Vec<i64>> {
    let mut map: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for item in dom::find_all(html, "*", Some("episode-download-item")) {
        let name = dom::find(item.inner, "*", Some("episode-file-title"))
            .map(|e| dom::text(e.inner))
            .unwrap_or_default();
        if let Some((se, ep)) = season_episode(&name) {
            let eps = map.entry(se).or_default();
            if !eps.contains(&ep) {
                eps.push(ep);
            }
        }
    }
    for eps in map.values_mut() {
        eps.sort_unstable();
    }
    map
}

// ---- releases ----

/// Downloadables for one episode, or for the whole page when `season` is 0.
///
/// Entries that name the same file are folded together and their mirrors
/// merged, because the page lists one row per host.
pub fn parse_releases(html: &str, season: i64, episode: i64) -> Vec<Release> {
    let (item_class, title_class) = if season > 0 {
        ("episode-download-item", "episode-file-title")
    } else {
        ("download-item", "file-title")
    };
    let page_language = metadata(html, "Audios:").and_then(|v| tidy_language(&v)).unwrap_or_default();

    let mut grouped: BTreeMap<String, Release> = BTreeMap::new();
    for item in dom::find_all(html, "*", Some(item_class)) {
        let filename = dom::find(item.inner, "*", Some(title_class))
            .map(|e| dom::text(e.inner))
            .unwrap_or_default();
        // A season pack is one enormous archive, not something to hand mpv.
        if filename.is_empty() || is_archive(&filename) {
            continue;
        }
        let stamped = season_episode(&filename);
        if season > 0 && stamped != Some((season, episode)) {
            continue;
        }
        let mirrors = release_mirrors(item);
        if mirrors.is_empty() {
            continue;
        }
        let size_bytes = dom::find_all(item.inner, "*", Some("badge-size"))
            .into_iter()
            .chain(dom::find_all(item.inner, "*", Some("badge")))
            .filter_map(|e| parse_size(&dom::text(e.inner)))
            .next();

        let entry = grouped.entry(normalise(&filename)).or_insert_with(|| Release {
            quality: quality_of(&filename),
            codec: codec_of(&filename).unwrap_or_default(),
            language: language_of(&filename).unwrap_or_else(|| page_language.clone()),
            size_bytes,
            season: stamped.map(|v| v.0),
            episode: stamped.map(|v| v.1),
            filename: filename.clone(),
            mirrors: Vec::new(),
        });
        for m in mirrors {
            if !entry.mirrors.iter().any(|(_, url)| *url == m.1) {
                entry.mirrors.push(m);
            }
        }
    }

    let mut out: Vec<Release> = grouped.into_values().collect();
    // Best quality first — the Stream tab's own sticky-quality pick then has
    // something meaningful to bind to.
    out.sort_by(|a, b| rung(b).cmp(&rung(a)));
    out
}

/// `(label, resolver url)` for every usable link on a download row.
fn release_mirrors(item: El<'_>) -> Vec<(String, String)> {
    dom::find_all(item.inner, "a", None)
        .into_iter()
        .filter_map(|link| {
            let href = dom::attr(link.attrs, "href")?;
            // Only https, and never the site's own session links.
            if !href.starts_with("https://") || href.contains("logout") {
                return None;
            }
            let label = dom::text(link.inner);
            Some((if label.is_empty() { "Source".into() } else { label }, href))
        })
        .collect()
}

/// One [`StreamFile`] per mirror.
///
/// Deliberately not one per release: resolving a mirror is a two-hop chain that
/// fails often, and a flat list is what lets the next one be tried by picking
/// the next row rather than by silently retrying behind a spinner.
pub fn releases_to_files(releases: &[Release]) -> Vec<StreamFile> {
    let mut out = Vec::new();
    for (i, r) in releases.iter().enumerate() {
        for (j, (label, url)) in r.mirrors.iter().enumerate() {
            out.push(StreamFile {
                url: url.clone(),
                resource_id: format!("fourk-{i}-{j}"),
                resolution: rung(r),
                codec: r.codec.clone(),
                size: r.size_bytes.map(|b| human_size(&b.to_string())).unwrap_or_default(),
                uploader: if r.language.is_empty() {
                    label.clone()
                } else {
                    format!("{label} · {}", r.language)
                },
                // The site ships no sidecar subtitles.
                captions: Vec::new(),
            });
        }
    }
    out
}

/// `"1080p"` → `1080`. Unknown quality sorts last rather than as 0-and-first.
fn rung(r: &Release) -> i64 {
    r.quality
        .as_deref()
        .and_then(|q| q.trim_end_matches('p').parse::<i64>().ok())
        .unwrap_or(0)
}

// ---- field readers ----

fn meta_content(html: &str, key: &str) -> Option<String> {
    dom::find_all(html, "meta", None)
        .into_iter()
        .find(|m| {
            dom::attr(m.attrs, "property").as_deref() == Some(key)
                || dom::attr(m.attrs, "name").as_deref() == Some(key)
        })
        .and_then(|m| dom::attr(m.attrs, "content"))
        .filter(|c| !c.is_empty())
}

/// The value beside a `.metadata-label` reading exactly `label`.
fn metadata(html: &str, label: &str) -> Option<String> {
    dom::find_all(html, "*", Some("metadata-item")).into_iter().find_map(|item| {
        let found = dom::find(item.inner, "*", Some("metadata-label")).map(|e| dom::text(e.inner))?;
        (found == label)
            .then(|| dom::find(item.inner, "*", Some("metadata-value")).map(|e| dom::text(e.inner)))?
    })
}

/// First plausible four-digit year in a string.
fn first_year(v: &str) -> Option<String> {
    v.as_bytes()
        .windows(4)
        .find(|w| w.iter().all(u8::is_ascii_digit) && matches!(w[0], b'1' | b'2'))
        .and_then(|w| std::str::from_utf8(w).ok())
        .map(str::to_string)
}

/// `"Dune (2021)"` → `"Dune"`. The year has its own field.
fn strip_trailing_year(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 6 {
        let tail = &t[t.len() - 6..];
        if tail.starts_with('(')
            && tail.ends_with(')')
            && tail[1..5].bytes().all(|b| b.is_ascii_digit())
        {
            return t[..t.len() - 6].trim_end().to_string();
        }
    }
    t.to_string()
}

/// `S01E04` anywhere in a filename, in any casing.
fn season_episode(v: &str) -> Option<(i64, i64)> {
    let upper = v.to_ascii_uppercase();
    let b = upper.as_bytes();
    for i in 0..b.len().saturating_sub(4) {
        if b[i] != b'S' {
            continue;
        }
        let se_end = b[i + 1..].iter().position(|c| !c.is_ascii_digit()).map(|o| i + 1 + o)?;
        if se_end == i + 1 || b.get(se_end) != Some(&b'E') {
            continue;
        }
        let ep_end = b[se_end + 1..]
            .iter()
            .position(|c| !c.is_ascii_digit())
            .map(|o| se_end + 1 + o)
            .unwrap_or(b.len());
        if ep_end == se_end + 1 {
            continue;
        }
        if let (Ok(se), Ok(ep)) =
            (upper[i + 1..se_end].parse(), upper[se_end + 1..ep_end].parse())
        {
            return Some((se, ep));
        }
    }
    None
}

/// `"2.1 GB"` → bytes.
fn parse_size(v: &str) -> Option<u64> {
    let n = v.replace(' ', "").to_ascii_uppercase();
    for (suffix, mul) in [("GB", 1024u64.pow(3)), ("MB", 1024u64.pow(2)), ("KB", 1024u64)] {
        if let Some(num) = n.strip_suffix(suffix)
            && let Ok(num) = num.parse::<f64>()
        {
            return Some((num * mul as f64) as u64);
        }
    }
    None
}

fn quality_of(v: &str) -> Option<String> {
    let lower = v.to_ascii_lowercase();
    ["2160p", "1080p", "720p", "480p"]
        .into_iter()
        .find(|q| lower.contains(q))
        .map(str::to_string)
}

fn codec_of(v: &str) -> Option<String> {
    let l = v.to_ascii_lowercase();
    if l.contains("av1") {
        Some("AV1".into())
    } else if l.contains("h.265") || l.contains("h265") || l.contains("x265") {
        Some("H.265".into())
    } else if l.contains("hevc") {
        Some("HEVC".into())
    } else if l.contains("h.264") || l.contains("h264") || l.contains("x264") {
        Some("H.264".into())
    } else if l.contains("remux") {
        Some("REMUX".into())
    } else {
        None
    }
}

fn language_of(v: &str) -> Option<String> {
    let l = v.to_ascii_lowercase();
    match (l.contains("hindi"), l.contains("english")) {
        (true, true) => Some("Hindi, English".into()),
        (true, false) => Some("Hindi".into()),
        (false, true) => Some("English".into()),
        _ if l.contains("dual audio") => Some("Dual Audio".into()),
        _ if l.contains("multi audio") || l.contains("multi-audio") => Some("Multi Audio".into()),
        _ => None,
    }
}

fn tidy_language(v: &str) -> Option<String> {
    let joined = v
        .split(['|', '/', '+'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let lower = joined.to_ascii_lowercase();
    (!joined.is_empty() && joined.len() <= 80 && !matches!(lower.as_str(), "n/a" | "na" | "unknown"))
        .then_some(joined)
}

fn is_archive(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    l.ends_with(".zip") || l.contains("complete season") || l.contains("season pack")
}

/// Group key: the same file listed under two hosts differs only in punctuation
/// and spacing.
fn normalise(v: &str) -> String {
    v.to_ascii_lowercase().chars().filter(char::is_ascii_alphanumeric).collect()
}

fn is_genre(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "action"
            | "adventure"
            | "animation"
            | "comedy"
            | "crime"
            | "documentary"
            | "drama"
            | "family"
            | "fantasy"
            | "history"
            | "horror"
            | "music"
            | "mystery"
            | "romance"
            | "science fiction"
            | "sci-fi"
            | "thriller"
            | "war"
            | "western"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = r#"
      <a class="movie-card" href="https://4khdhub.one/movie/dune-2021">
        <img src="https://img/p.jpg">
        <div class="movie-card-title">Dune</div>
        <div class="movie-card-meta">2021 &bull; Action</div>
      </a>
      <a class="movie-card" href="/tv-series-loki-2021">
        <div class="movie-card-title">Loki</div>
        <div class="movie-card-meta">2021 - S02</div>
      </a>
      <a class="movie-card" href="https://evil.example/steal">
        <div class="movie-card-title">Nope</div>
      </a>"#;

    #[test]
    fn search_reads_cards_and_refuses_off_site_links() {
        let hits = parse_search(SEARCH, "4khdhub.one");
        assert_eq!(hits.len(), 2, "the off-site card is dropped");
        assert_eq!(hits[0].title, "Dune");
        assert_eq!(hits[0].id, "/movie/dune-2021", "absolute same-site link reduces to its path");
        assert_eq!(hits[0].year, "2021");
        assert!(!hits[0].is_series);
        assert!(hits[1].is_series, "`-series-` in the path is the only kind marker");
    }

    const DETAILS: &str = r#"
      <html><head>
        <meta property="og:image" content="https://img/cover.jpg">
        <meta name="description" content="fallback blurb">
      </head><body>
        <h1>Loki (2021)</h1>
        <div class="content-section"><p>A trickster steals a tesseract.</p></div>
        <div class="imdb-score">8.2 / 10</div>
        <div class="badge-outline"><a>Drama</a></div>
        <div class="badge-outline"><a>4K</a></div>
        <div class="metadata-item">
          <span class="metadata-label">Release:</span>
          <span class="metadata-value">June 2021</span>
        </div>
        <div id="episodes">
          <div class="episode-download-item">
            <div class="episode-file-title">Loki.S01E01.1080p.WEB-DL.x265.mkv</div>
            <a href="https://hubcloud.one/drive/a">HubCloud</a>
            <span class="badge-size">2.1 GB</span>
          </div>
          <div class="episode-download-item">
            <div class="episode-file-title">Loki.S01E02.2160p.WEB-DL.HEVC.Hindi.English.mkv</div>
            <a href="https://hubdrive.one/file/b">HubDrive</a>
            <a href="https://hubcloud.one/drive/b">HubCloud</a>
          </div>
          <div class="episode-download-item">
            <div class="episode-file-title">Loki.S02E01.720p.mkv</div>
            <a href="https://hubcloud.one/drive/c">HubCloud</a>
          </div>
        </div>
      </body></html>"#;

    #[test]
    fn details_pull_the_real_fields_not_the_fallbacks() {
        let d = parse_details("/tv-series-loki-2021", DETAILS).unwrap();
        assert_eq!(d.title, "Loki", "the trailing year has its own field");
        assert_eq!(d.year, "2021");
        assert_eq!(d.overview, "A trickster steals a tesseract.");
        assert_eq!(d.cover, "https://img/cover.jpg");
        assert_eq!(d.rating, 8.2);
        assert_eq!(d.genre, "Drama", "`4K` is a badge, not a genre");
        assert!(d.is_series);
    }

    #[test]
    fn seasons_are_derived_from_the_download_list() {
        // It is the only place the page states which episodes exist.
        let d = parse_details("/tv-series-loki-2021", DETAILS).unwrap();
        assert_eq!(d.seasons.len(), 2);
        assert_eq!((d.seasons[0].number, d.seasons[0].max_ep), (1, 2));
        assert_eq!((d.seasons[1].number, d.seasons[1].max_ep), (2, 1));
        assert_eq!(d.episodes.len(), 3);
    }

    #[test]
    fn releases_filter_to_the_requested_episode() {
        let r = parse_releases(DETAILS, 1, 2);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].quality.as_deref(), Some("2160p"));
        assert_eq!(r[0].codec, "HEVC");
        assert_eq!(r[0].language, "Hindi, English");
        assert_eq!(r[0].mirrors.len(), 2, "both hosts are kept as alternatives");
        assert!(parse_releases(DETAILS, 9, 9).is_empty());
    }

    #[test]
    fn a_release_becomes_one_file_per_mirror() {
        let files = releases_to_files(&parse_releases(DETAILS, 1, 2));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].resolution, 2160);
        assert!(files[0].uploader.contains("Hindi"), "the audio track is worth naming");
        assert_ne!(files[0].url, files[1].url);
    }

    #[test]
    fn size_badges_become_the_human_string_the_ui_expects() {
        let files = releases_to_files(&parse_releases(DETAILS, 1, 1));
        assert_eq!(files[0].size, "2.1 GB");
    }

    #[test]
    fn season_packs_and_archives_are_not_offered() {
        let html = r#"<div class="download-item">
              <div class="file-title">Loki.Complete.Season.1.zip</div>
              <a href="https://hubcloud.one/drive/z">HubCloud</a>
            </div>
            <div class="download-item">
              <div class="file-title">Dune.2021.1080p.mkv</div>
              <a href="https://hubcloud.one/drive/d">HubCloud</a>
            </div>"#;
        let r = parse_releases(html, 0, 0);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].filename, "Dune.2021.1080p.mkv");
    }

    #[test]
    fn the_same_file_under_two_hosts_is_one_release() {
        let html = r#"<div class="download-item">
              <div class="file-title">Dune 2021 1080p.mkv</div>
              <a href="https://hubcloud.one/drive/a">A</a>
            </div>
            <div class="download-item">
              <div class="file-title">dune.2021.1080p.mkv</div>
              <a href="https://hubdrive.one/file/a">B</a>
            </div>"#;
        let r = parse_releases(html, 0, 0);
        assert_eq!(r.len(), 1, "punctuation and case are not a different file");
        assert_eq!(r[0].mirrors.len(), 2);
    }

    #[test]
    fn best_quality_comes_first() {
        let html = r#"<div class="download-item"><div class="file-title">a.720p.mkv</div>
              <a href="https://hubcloud.one/drive/a">A</a></div>
            <div class="download-item"><div class="file-title">b.2160p.mkv</div>
              <a href="https://hubcloud.one/drive/b">B</a></div>"#;
        let r = parse_releases(html, 0, 0);
        assert_eq!(r[0].quality.as_deref(), Some("2160p"));
    }

    #[test]
    fn season_stamps_are_found_in_any_casing() {
        assert_eq!(season_episode("Show.s01e04.mkv"), Some((1, 4)));
        assert_eq!(season_episode("Show S2E11 1080p"), Some((2, 11)));
        assert_eq!(season_episode("Movie.2021.1080p.mkv"), None);
    }
}
