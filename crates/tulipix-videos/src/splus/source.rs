//! The provider contract, and the failover that walks it.
//!
//! One trait, three implementations. Adding a fourth provider is a new file and
//! one line in [`all`] — nothing else in Stream Plus knows how many there are.

use anyhow::Result;
use async_trait::async_trait;
use sqlx::SqlitePool;

use super::health;
use super::{Episode, EpisodeRef, Title};

/// How a resolved URL has to be handled downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayableKind {
    /// A progressive file. mpv plays it and the downloader streams it to disk.
    Mp4,
    /// An HLS playlist. mpv plays it too, but downloading needs a remux.
    M3u8,
}

impl PlayableKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::M3u8 => "m3u8",
        }
    }

    /// Not `FromStr`: this never fails — an unknown string is a progressive
    /// file, because that is what every provider that names nothing returns.
    pub fn parse(s: &str) -> Self {
        if s == "m3u8" { Self::M3u8 } else { Self::Mp4 }
    }
}

/// One playable file, as one provider resolved it.
#[derive(Debug, Clone, PartialEq)]
pub struct Playable {
    pub url: String,
    pub kind: PlayableKind,
    /// "1080p" — what the row shows on the left.
    pub label: String,
    /// Vertical resolution when known, for the preferred-quality pick.
    pub height: i32,
    /// Bytes, when the provider said. 0 means unknown, not empty.
    pub size: i64,
    pub codec: String,
    pub uploader: String,
    /// Which [`Source::id`] produced this.
    pub source: &'static str,
    /// Extra headers the URL only works with (referer, origin). Passed to mpv
    /// and to the downloader; empty for anything that plays bare.
    pub headers: Vec<(String, String)>,
}

impl Playable {
    pub fn new(url: impl Into<String>, kind: PlayableKind, source: &'static str) -> Self {
        Self {
            url: url.into(),
            kind,
            label: String::new(),
            height: 0,
            size: 0,
            codec: String::new(),
            uploader: String::new(),
            source,
            headers: Vec::new(),
        }
    }

    /// "412 MB · h264", or just the codec, or just the size.
    pub fn sub_label(&self) -> String {
        let size = (self.size > 0).then(|| human_bytes(self.size));
        match (size, self.codec.is_empty()) {
            (Some(s), false) => format!("{s} · {}", self.codec),
            (Some(s), true) => s,
            (None, false) => self.codec.clone(),
            (None, true) => self.kind.as_str().to_string(),
        }
    }
}

pub fn human_bytes(n: i64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n < K {
        format!("{n:.0} B")
    } else if n < K * K {
        format!("{:.0} KB", n / K)
    } else if n < K * K * K {
        format!("{:.0} MB", n / (K * K))
    } else {
        format!("{:.2} GB", n / (K * K * K))
    }
}

/// A place episodes and files come from.
///
/// Implementations are stateless — they hold an HTTP client and nothing else —
/// so [`all`] can rebuild the list on every resolve without cost.
#[async_trait]
pub trait Source: Send + Sync {
    /// Stable id. Used as the settings key, the health-table key and the label
    /// on a resolved row, so it must never change once shipped.
    fn id(&self) -> &'static str;

    /// What the user sees in the source order.
    fn label(&self) -> &'static str;

    /// True when this source only has anime. The TMDB lane skips those.
    fn anime_only(&self) -> bool {
        false
    }

    /// Episodes this source can offer for a title. An empty list is not an
    /// error — it means "not carried here", and failover moves on.
    async fn episodes(&self, title: &Title, audio: &str) -> Result<Vec<Episode>>;

    /// Playable files for one episode, best first.
    async fn resolve(&self, ep: &EpisodeRef) -> Result<Vec<Playable>>;

    /// A cheap request that proves the host answers, for the Test button.
    /// Returns the round trip in milliseconds.
    async fn ping(&self) -> Result<u32>;
}

/// Every provider Stream Plus knows about, in declaration order.
///
/// The *enabled* set and the *order* come from the health table and settings;
/// this is just the registry.
pub fn all() -> Vec<Box<dyn Source>> {
    let mut v: Vec<Box<dyn Source>> = vec![Box::new(super::allmanga::AllManga::new())];
    for host in super::vidsrc::hosts() {
        v.push(Box::new(super::vidsrc::VidSrc::new(host)));
    }
    v
}

/// Look one up by id.
pub fn by_id(id: &str) -> Option<Box<dyn Source>> {
    all().into_iter().find(|s| s.id() == id)
}

/// Walk the enabled sources in order and return the first non-empty result.
///
/// Records timing and failures as it goes, which is what later reorders the
/// list — a source that has failed three times running sinks to the bottom for
/// an hour and stops costing the user a timeout on every play.
pub async fn resolve_any(pool: &SqlitePool, ep: &EpisodeRef) -> Result<Vec<Playable>> {
    let order = health::order(pool, ep.title.anilist_id.is_some()).await;
    let mut last_err = None;
    for id in order {
        let Some(src) = by_id(&id) else { continue };
        let started = std::time::Instant::now();
        match src.resolve(ep).await {
            Ok(files) if !files.is_empty() => {
                let ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                health::note_ok(pool, src.id(), ms).await;
                return Ok(files);
            }
            Ok(_) => {
                health::note_fail(pool, src.id(), "no match for this episode").await;
            }
            Err(e) => {
                tracing::debug!(source = src.id(), error = %e, "splus: resolve failed");
                health::note_fail(pool, src.id(), &short_error(&e)).await;
                last_err = Some(e);
            }
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Ok(Vec::new()),
    }
}

/// Episodes for a title, from the first source that carries it.
pub async fn episodes_any(
    pool: &SqlitePool,
    title: &Title,
    audio: &str,
) -> Result<(Vec<Episode>, &'static str)> {
    let order = health::order(pool, title.anilist_id.is_some()).await;
    for id in order {
        let Some(src) = by_id(&id) else { continue };
        let started = std::time::Instant::now();
        match src.episodes(title, audio).await {
            Ok(eps) if !eps.is_empty() => {
                let ms = started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32;
                health::note_ok(pool, src.id(), ms).await;
                let id: &'static str = src.id();
                return Ok((eps, id));
            }
            Ok(_) => health::note_fail(pool, src.id(), "title not carried").await,
            Err(e) => {
                tracing::debug!(source = src.id(), error = %e, "splus: episode list failed");
                health::note_fail(pool, src.id(), &short_error(&e)).await;
            }
        }
    }
    Ok((Vec::new(), ""))
}

/// One line, no chain, suitable for a health chip.
pub fn short_error(e: &anyhow::Error) -> String {
    let s = e.to_string();
    match s.char_indices().nth(80) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}
