//! How a file's name and folders say what it is: an episode of a show, a
//! movie, or neither (a personal video, which stays in Local).
//!
//! One reader for both front ends. It used to be two copies of an `SxxEyy`
//! scanner -- one in the Slint build, one in the bridge -- that named the show
//! after the file's parent folder, so the ordinary `Show/Season 01/…` layout
//! made a show called "Season 01", and whose movie side took the first
//! year-like number it saw, so `Blade Runner 2049 (2017)` came out as
//! "Blade Runner" from 2049.
//!
//! The ten layouts each side reads are listed in the Videos tab's info popup
//! (`videos_naming.dart`); the tests at the bottom are those same twenty
//! examples, so the popup cannot promise a name this does not read.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// What an episode's path says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeName {
    pub show: String,
    pub year: Option<i64>,
    pub season: i64,
    pub episode: i64,
}

/// What a movie's path says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieName {
    pub title: String,
    pub year: Option<i64>,
}

// `S01E05`, `s1.e5`, `S01 E05`, and the first of `S01E01-E02`.
static TAG_SE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])s(\d{1,2})[ ._-]*e(\d{1,3})").unwrap()
});
// `1x05`. Not `1920x1080`: at most two digits before the x, at most three
// after, and a digit on neither side.
static TAG_X: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(\d{1,2})x(\d{1,3})(?:[^0-9]|$)").unwrap()
});
// `Season 2 Episode 5`, spelled out.
static TAG_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])season[ ._-]*(\d{1,2})[ ._,-]*episode[ ._-]*(\d{1,3})").unwrap()
});
// A season folder: `Season 01`, `Season 1`, `Series 2`, `S01`, `Specials`.
static SEASON_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(?:season|series|staffel|saison|temporada)[ ._-]*(\d{1,3})|s(\d{1,2})|(specials?))$")
        .unwrap()
});
// Inside a season folder, a file that names only its episode: `Episode 07`,
// `E07 - Title`, `07 - Title`.
static EP_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:episode|ep|e)?[ ._-]*(\d{1,3})(?:[^0-9]|$)").unwrap()
});
// A fansub release: `[Group] Show - 12 [1080p]`. The leading group tag is what
// makes a bare ` - 12` an episode rather than part of a title.
static ANIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[[^\]]+\][ ._]*(.+?)[ ._]+-[ ._]+(\d{1,3})(?:v\d)?(?:[ ._\[\(]|$)").unwrap()
});
// A year that is a year: four digits, 1900-2099, not inside a longer number.
static YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9])((?:19|20)\d{2})(?:[^0-9]|$)").unwrap()
});
// `{edition-Extended}`, `{tmdb-603}`, `[imdbid-tt0133093]`: labels, not title.
static LABELS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\{[^}]*\}|\[(?:imdb|tmdb|tvdb)(?:id)?-[^\]]*\]").unwrap()
});
// `- cd1`, `- part 2`, `.disc1` at the end: one film in pieces.
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[ ._-]+(?:cd|part|pt|disc|disk)[ ._-]*\d{1,2}$").unwrap()
});

/// Everything after these is release noise: resolution, source, codec. Only
/// cut from a name that has no year -- with one, everything after the year is
/// gone already, and a title can hold any of these as a word.
const NOISE: &[&str] = &[
    "2160p", "1080p", "720p", "480p", "4k", "uhd", "web-dl", "webdl", "webrip", "bluray",
    "blu-ray", "bdrip", "brrip", "dvdrip", "hdtv", "remux", "hdr", "hdr10", "dv", "x264", "x265",
    "h264", "h 264", "h265", "h 265", "hevc", "av1", "aac", "ac3", "dts", "ddp5", "dd5", "atmos",
    "10bit", "proper", "repack", "extended", "unrated",
];

/// File names that say nothing, so the folder has to: `movie.mkv`.
const GENERIC: &[&str] = &["movie", "film", "video", "feature", "main", "title", "sample"];

/// `S01E05` (and `1x05`, `Season 1 Episode 5`) in a file name, with where the
/// tag starts so the text before it can name the show.
pub fn season_episode(name: &str) -> Option<(i64, i64)> {
    tag(name).map(|(s, e, _)| (s, e))
}

/// The season, the episode, and where the tag begins -- everything before it
/// is the show's name, when the file carries one.
fn tag(name: &str) -> Option<(i64, i64, usize)> {
    for re in [&*TAG_SE, &*TAG_X, &*TAG_WORDS] {
        if let Some(c) = re.captures(name) {
            let s = c[1].parse().ok()?;
            let e = c[2].parse().ok()?;
            // The whole match, which starts on the separator before the tag
            // (or at the very start): `Fleabag Season 2 Episode 5` is
            // "Fleabag", not "Fleabag Season".
            return Some((s, e, c.get(0)?.start()));
        }
    }
    None
}

/// `Season 02` -> 2, `Specials` -> 0.
fn season_dir(name: &str) -> Option<i64> {
    let c = SEASON_DIR.captures(name.trim())?;
    if c.get(3).is_some() {
        return Some(0);
    }
    c.get(1).or(c.get(2))?.as_str().parse().ok()
}

fn stem(path: &Path) -> &str {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("")
}

fn dir_name(path: Option<&Path>) -> Option<&str> {
    path?.file_name()?.to_str()
}

/// Dots and underscores to spaces, labels out, trailing dashes off -- and, for
/// a name with no year to end it, release noise off too.
fn tidy(raw: &str, noise: bool) -> String {
    let s = LABELS.replace_all(raw, " ");
    let s = s.replace(['.', '_'], " ");
    let lower = s.to_ascii_lowercase();
    let mut cut = s.len();
    let words: &[&str] = if noise { NOISE } else { &[] };
    for word in words {
        // Whole words only: "dv" must not cut "Dvorak".
        let mut from = 0;
        while let Some(i) = lower[from..].find(word) {
            let i = from + i;
            let before = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
            let end = i + word.len();
            let after = end >= lower.len() || !lower.as_bytes()[end].is_ascii_alphanumeric();
            if before && after && i > 0 {
                cut = cut.min(i);
                break;
            }
            from = end;
        }
    }
    let s = &s[..cut];
    s.trim().trim_end_matches(['-', '(', '[', ' ']).trim().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title and the year it carries, if any.
///
/// The year is the one in brackets when there is one -- `Blade Runner 2049
/// (2017)` is from 2017 -- and otherwise the last year-like number that is not
/// the whole title, so `1917.2019.1080p` is "1917" from 2019 and `1917.mkv` is
/// just "1917".
fn title_year(raw: &str) -> (String, Option<i64>) {
    let raw = LABELS.replace_all(raw, " ");
    let raw = PART.replace(raw.trim(), "");
    // Resumed from the end of each year rather than the end of each match:
    // a match eats the separator after its year, and in `1917.2019` that
    // separator is the one the second year needs before it.
    let mut years: Vec<(usize, i64, bool)> = Vec::new();
    let mut from = 0;
    while let Some(c) = YEAR.captures_at(&raw, from) {
        let Some(m) = c.get(1) else { break };
        from = m.end();
        let bracketed =
            raw[..m.start()].ends_with(['(', '[']) && raw[m.end()..].starts_with([')', ']']);
        if let Ok(y) = m.as_str().parse() {
            // Not a year if it is the whole title: `1917.mkv`.
            if !tidy(&raw[..m.start()], false).is_empty() {
                years.push((m.start(), y, bracketed));
            }
        }
    }
    let pick = years.iter().find(|y| y.2).or(years.last());
    match pick {
        Some(&(at, year, _)) => (tidy(&raw[..at], false), Some(year)),
        None => (tidy(&raw, true), None),
    }
}

/// An episode, from the file name and the two folders above it.
pub fn parse_episode(path: &Path) -> Option<EpisodeName> {
    let name = stem(path);
    let parent = path.parent();
    let folder_season = dir_name(parent).and_then(season_dir);
    // The show's folder: the one above a season folder, else the parent.
    let show_dir = if folder_season.is_some() {
        dir_name(parent.and_then(Path::parent))
    } else {
        dir_name(parent)
    };

    let (season, episode, prefix) = if let Some((s, e, at)) = tag(name) {
        (s, e, name[..at].to_string())
    } else if let Some(c) = ANIME.captures(name) {
        (folder_season.unwrap_or(1), c[2].parse().ok()?, c[1].to_string())
    } else if let Some(fs) = folder_season {
        let c = EP_ONLY.captures(name)?;
        (fs, c[1].parse().ok()?, String::new())
    } else {
        return None;
    };

    // A season folder means the layout is deliberate, so the folder names the
    // show. Otherwise the file does, when there is text before its tag; a bare
    // `S01E01.mkv` is named by the folder it sits in.
    let from_prefix = title_year(&prefix);
    let (show, year) = match show_dir {
        Some(d) if folder_season.is_some() || from_prefix.0.is_empty() => title_year(d),
        _ => from_prefix,
    };
    if show.is_empty() {
        return None;
    }
    Some(EpisodeName { show, year, season, episode })
}

/// A movie, or `None` when nothing about the path says it is one.
///
/// It is a movie when it carries a year, or when it lives under a folder
/// called Movies or Films. Anything else -- `VID_20250814.mp4` in Camera --
/// is a personal video and stays in Local.
pub fn parse_movie(path: &Path) -> Option<MovieName> {
    if parse_episode(path).is_some() {
        return None;
    }
    let (mut title, mut year) = title_year(stem(path));
    // `Title (Year)/movie.mkv`, or a file with no year in a folder with one:
    // the folder is the name.
    if let Some(dir) = dir_name(path.parent()) {
        let generic = GENERIC.contains(&title.to_ascii_lowercase().as_str());
        let (dt, dy) = title_year(dir);
        if dy.is_some() && (generic || year.is_none()) {
            title = dt;
            year = dy;
        }
    }
    let in_movies = path.ancestors().skip(1).filter_map(|a| a.file_name()?.to_str()).any(|d| {
        matches!(d.to_ascii_lowercase().as_str(), "movies" | "films" | "movie" | "film")
    });
    if title.is_empty() || (year.is_none() && !in_movies) {
        return None;
    }
    Some(MovieName { title, year })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(p: &str) -> Option<(String, Option<i64>, i64, i64)> {
        parse_episode(Path::new(p)).map(|e| (e.show, e.year, e.season, e.episode))
    }
    fn mv(p: &str) -> Option<(String, Option<i64>)> {
        parse_movie(Path::new(p)).map(|m| (m.title, m.year))
    }
    fn some(show: &str, year: Option<i64>, s: i64, e: i64) -> Option<(String, Option<i64>, i64, i64)> {
        Some((show.into(), year, s, e))
    }

    /// The ten show layouts the info popup lists, in its order.
    #[test]
    fn the_ten_show_layouts() {
        let v = "/home/me/Videos/TV";
        assert_eq!(ep(&format!("{v}/Severance/Season 01/Severance - S01E04 - The You You Are.mkv")),
            some("Severance", None, 1, 4));
        assert_eq!(ep(&format!("{v}/Severance/Season 1/S01E04.mkv")), some("Severance", None, 1, 4));
        assert_eq!(ep(&format!("{v}/Severance.S01E04.1080p.WEB-DL.x265-GROUP.mkv")),
            some("Severance", None, 1, 4));
        assert_eq!(ep(&format!("{v}/Doctor Who (2005)/Season 04/Doctor Who (2005) - S04E10.mkv")),
            some("Doctor Who", Some(2005), 4, 10));
        assert_eq!(ep(&format!("{v}/The Bear/Season 01/The Bear - 1x05 - Sheridan.mkv")),
            some("The Bear", None, 1, 5));
        assert_eq!(ep(&format!("{v}/Dark/Season 01/Dark - S01E01-E02.mkv")), some("Dark", None, 1, 1));
        assert_eq!(ep(&format!("{v}/Sherlock/Specials/Sherlock - S00E01 - The Abominable Bride.mkv")),
            some("Sherlock", None, 0, 1));
        assert_eq!(ep(&format!("{v}/Chernobyl/Season 1/Episode 03.mkv")), some("Chernobyl", None, 1, 3));
        assert_eq!(ep(&format!("{v}/Frieren/[SubsPlease] Sousou no Frieren - 12 [1080p].mkv")),
            some("Sousou no Frieren", None, 1, 12));
        assert_eq!(ep(&format!("{v}/Fleabag/Fleabag Season 2 Episode 5.mp4")), some("Fleabag", None, 2, 5));
    }

    /// The ten movie layouts the info popup lists, in its order.
    #[test]
    fn the_ten_movie_layouts() {
        let m = "/home/me/Videos/Movies";
        let some = |t: &str, y: i64| Some((t.to_string(), Some(y)));
        assert_eq!(mv(&format!("{m}/Arrival (2016)/Arrival (2016).mkv")), some("Arrival", 2016));
        assert_eq!(mv(&format!("{m}/Arrival (2016).mkv")), some("Arrival", 2016));
        assert_eq!(mv("/dl/Arrival.2016.1080p.BluRay.x264-GROUP.mkv"), some("Arrival", 2016));
        assert_eq!(mv("/dl/Dune Part Two (2024) [2160p HDR].mkv"), some("Dune Part Two", 2024));
        assert_eq!(mv("/dl/Parasite [2019].mkv"), some("Parasite", 2019));
        assert_eq!(mv("/dl/Aliens (1986) {edition-Director's Cut}.mkv"), some("Aliens", 1986));
        assert_eq!(mv("/dl/Kill Bill Vol 1 (2003) - cd2.mkv"), some("Kill Bill Vol 1", 2003));
        assert_eq!(mv("/dl/Blade Runner 2049 (2017).mkv"), some("Blade Runner 2049", 2017));
        assert_eq!(mv(&format!("{m}/Spirited Away.mkv")), Some(("Spirited Away".into(), None)));
        assert_eq!(mv(&format!("{m}/Interstellar (2014)/movie.mkv")), some("Interstellar", 2014));
    }

    #[test]
    fn what_is_neither() {
        // A phone clip, a screen recording and a resolution are not episodes
        // or movies.
        assert_eq!(mv("/home/me/Videos/Camera/VID_20250814_183012.mp4"), None);
        assert_eq!(ep("/home/me/Videos/Camera/VID_20250814_183012.mp4"), None);
        assert_eq!(season_episode("Trip 1920x1080.mp4"), None);
        assert_eq!(mv("/home/me/Videos/Screen recordings/bug repro.webm"), None);
        // A title that is also noise keeps its word when a year ends it.
        assert_eq!(mv("/dl/Charlotte's Web (2006).mkv"), Some(("Charlotte's Web".into(), Some(2006))));
        // Numbers in a title stay in the title.
        assert_eq!(mv("/dl/1917.2019.1080p.mkv"), Some(("1917".into(), Some(2019))));
        assert_eq!(mv("/home/me/Movies/1917.mkv"), Some(("1917".into(), None)));
        // An episode is never also a movie.
        assert_eq!(mv("/dl/Severance.S01E04.2022.mkv"), None);
    }

    #[test]
    fn the_old_scanner_still_reads() {
        // The shapes the two old copies were tested on.
        assert_eq!(season_episode("Show.S01E05.1080p.mkv"), Some((1, 5)));
        assert_eq!(season_episode("show s2 e12.mp4"), Some((2, 12)));
        assert_eq!(season_episode("Show_s01_e105.mkv"), Some((1, 105)));
        assert_eq!(season_episode("Dune Part Two 2024.mkv"), None);
        assert_eq!(season_episode("seasons.mkv"), None);
    }
}
