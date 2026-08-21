//! Stream Plus — the anime lane and the second provider stack.
//!
//! Deliberately separate from `stream::`. The Stream tab is a MovieBox client
//! and stays one; this module talks to a different set of services and shares
//! nothing with it but the app's HTTP client, its DB file and mpv. Two tabs, one
//! app: the *plumbing* is shared, the *providers* are not.
//!
//! Everything here is written against each service's own public API. No code
//! is derived from any GPL implementation of the same idea — see
//! `.planning/specs/stream-plus.md` §2.
//!
//! Layering, matching `stream/`:
//!   * this crate is typed and UI-free — it returns data, never `SharedString`
//!   * `tulipix_sec_videos::splus` owns the window callbacks and the models
//!   * `ui/page_stream_plus.slint` owns the pixels and holds no logic

pub mod allmanga;
pub mod anilist;
pub mod aniskip;
pub mod downloads;
pub mod epgroup;
pub mod health;
pub mod prefs;
pub mod library;
pub mod schema;
pub mod source;
pub mod subs;
pub mod vidsrc;

pub use source::{Playable, PlayableKind, Source, resolve_any};

/// A title as Stream Plus knows it, whatever provider produced it.
///
/// `anilist_id` is `Some` for the anime lane and `None` for the TMDB lane; the
/// two never mix, and the id that is present decides which metadata path a
/// detail view takes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Title {
    pub anilist_id: Option<i64>,
    pub mal_id: Option<i64>,
    pub tmdb_id: Option<i64>,
    /// Romaji for anime, the catalogue title otherwise. This is what the
    /// providers are searched by, so it is never empty.
    pub title: String,
    pub english: Option<String>,
    pub native: Option<String>,
    pub year: Option<i64>,
    pub overview: String,
    pub cover_url: String,
    pub banner_url: String,
    /// "TV" | "MOVIE" | "OVA" | "SPECIAL" | "ONA"
    pub format: String,
    pub episodes: Option<i64>,
    /// 0..=100 as the services report it; `None` when unrated.
    pub score: Option<i64>,
    pub genres: Vec<String>,
    /// Certification for the age gate, e.g. "PG-13" / "FSK 16". Empty when the
    /// provider gave none — which the gate treats as "above the limit".
    pub certification: String,
    pub adult: bool,
    /// Set when the show is still airing: the next episode and when it lands.
    pub next_episode: Option<i64>,
    pub next_airing_at: Option<i64>,
}

impl Title {
    /// What to draw on a card. English where the user is likely to recognise it,
    /// romaji otherwise — never the native script, which is unreadable to most
    /// of the people looking at the grid.
    pub fn display_title(&self) -> &str {
        self.english.as_deref().filter(|s| !s.is_empty()).unwrap_or(&self.title)
    }

    pub fn is_series(&self) -> bool {
        !matches!(self.format.as_str(), "MOVIE")
    }
}

/// One episode of a title, as a provider listed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Episode {
    pub number: i64,
    pub season: i64,
    pub title: String,
    /// 0.0..=1.0 of how far through it the user got, filled from `library`.
    pub progress: f32,
    pub thumb_url: String,
}

/// What a detail view is looking at right now: a title, a season, an episode and
/// an audio track. Providers take this rather than six loose arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EpisodeRef {
    pub title: Title,
    pub season: i64,
    pub episode: i64,
    /// "sub" | "dub" — AllManga keys its releases on this.
    pub audio: String,
}

impl EpisodeRef {
    /// Stable key for progress, history and download rows.
    ///
    /// Prefixed per-lane so an AniList 154587 and a TMDB 154587 can never
    /// collide in the same table.
    pub fn key(&self) -> String {
        let base = match (self.title.anilist_id, self.title.tmdb_id) {
            (Some(a), _) => format!("al:{a}"),
            (None, Some(t)) => format!("tmdb:{t}"),
            _ => format!("q:{}", self.title.title),
        };
        if self.title.is_series() {
            format!("{base}:s{}e{}", self.season, self.episode)
        } else {
            base
        }
    }

    /// "S01E13" for a series, "" for a film — the label every row appends.
    pub fn label(&self) -> String {
        if self.title.is_series() {
            format!("S{:02}E{:02}", self.season, self.episode)
        } else {
            String::new()
        }
    }
}

/// Preferred audio, as the Settings dropdown stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPref {
    SubThenDub,
    DubThenSub,
}

impl AudioPref {
    pub fn from_setting(s: &str) -> Self {
        if s == "dub_then_sub" { Self::DubThenSub } else { Self::SubThenDub }
    }

    /// The order to try, first choice first.
    pub fn order(self) -> [&'static str; 2] {
        match self {
            Self::SubThenDub => ["sub", "dub"],
            Self::DubThenSub => ["dub", "sub"],
        }
    }
}
