//! Metadata agent priority chain.
//!
//! Users configure the order of metadata sources in Settings → Sources.
//! Default: `TMDB → OMDb → filename-parser`. The chain walks each agent in
//! order, stops at the first match, and records which agent answered (so a
//! manual re-scan can be limited to a specific tier — e.g. "rescan anything
//! that fell back to the filename parser").
//!
//! All agents implement `MovieAgent` / `ShowAgent`. The TMDB client from
//! `crate::tmdb` already implements `MetadataProvider`; we expose a thin
//! adapter so it slots in without a second implementation.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::tmdb::{MetadataProvider, MovieMeta, ShowMeta};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentId {
    Tmdb,
    Omdb,
    FilenameParser,
    Custom(u32),
}

impl AgentId {
    pub fn as_str(self) -> String {
        match self {
            AgentId::Tmdb => "tmdb".into(),
            AgentId::Omdb => "omdb".into(),
            AgentId::FilenameParser => "filename".into(),
            AgentId::Custom(n) => format!("custom-{n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentHit<T> {
    pub agent: String,
    pub meta: T,
}

#[async_trait]
pub trait MovieAgent: Send + Sync {
    fn id(&self) -> AgentId;
    async fn lookup(&self, title: &str, year: Option<i64>) -> Result<Option<MovieMeta>>;
}

#[async_trait]
pub trait ShowAgent: Send + Sync {
    fn id(&self) -> AgentId;
    async fn lookup(&self, title: &str) -> Result<Option<ShowMeta>>;
}

/// Adapter so any `MetadataProvider` (TMDB, OMDb-rs, plugins) is usable as
/// both movie + show agent.
pub struct ProviderAgent<P> {
    pub id: AgentId,
    pub inner: P,
}

#[async_trait]
impl<P> MovieAgent for ProviderAgent<P>
where
    P: MetadataProvider,
{
    fn id(&self) -> AgentId {
        self.id
    }
    async fn lookup(&self, title: &str, year: Option<i64>) -> Result<Option<MovieMeta>> {
        self.inner.search_movie(title, year).await
    }
}

#[async_trait]
impl<P> ShowAgent for ProviderAgent<P>
where
    P: MetadataProvider,
{
    fn id(&self) -> AgentId {
        self.id
    }
    async fn lookup(&self, title: &str) -> Result<Option<ShowMeta>> {
        self.inner.search_show(title).await
    }
}

/// Last-resort filename agent — parses release title regex (Plex/Sonarr style)
/// and returns a MovieMeta that only has title + year filled in. Lets the UI
/// render an unknown movie correctly even when TMDB is down.
pub struct FilenameAgent;

#[async_trait]
impl MovieAgent for FilenameAgent {
    fn id(&self) -> AgentId {
        AgentId::FilenameParser
    }
    async fn lookup(&self, title: &str, _year: Option<i64>) -> Result<Option<MovieMeta>> {
        Ok(Some(parse_filename(title)))
    }
}

#[async_trait]
impl ShowAgent for FilenameAgent {
    fn id(&self) -> AgentId {
        AgentId::FilenameParser
    }
    async fn lookup(&self, title: &str) -> Result<Option<ShowMeta>> {
        let m = parse_filename(title);
        Ok(Some(ShowMeta {
            tmdb_id: None,
            tvdb_id: None,
            title: m.title,
            year: m.year,
            overview: None,
            poster_path: None,
            backdrop_path: None,
        }))
    }
}

pub fn parse_filename(raw: &str) -> MovieMeta {
    let stem = raw.trim_end_matches(".mkv")
        .trim_end_matches(".mp4")
        .trim_end_matches(".avi")
        .trim_end_matches(".mov");
    let stem = stem.replace(['.', '_'], " ");
    let (title, year) = extract_year(&stem);
    MovieMeta {
        tmdb_id: None,
        title: trim_garbage(&title),
        year,
        overview: None,
        poster_path: None,
        backdrop_path: None,
        runtime_min: None,
    }
}

fn extract_year(s: &str) -> (String, Option<i64>) {
    // Walk from the right looking for a (19xx) or (20xx) token.
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let token = &s[i..i + 4];
            if let Ok(y) = token.parse::<i64>() {
                if (1900..=2100).contains(&y) {
                    let prefix = s[..i].trim_end_matches(['(', ' ', '[']).to_string();
                    return (prefix, Some(y));
                }
            }
        }
        i += 1;
    }
    (s.to_string(), None)
}

fn trim_garbage(s: &str) -> String {
    let mut out = s.trim().to_string();
    for tag in [
        " 1080p", " 720p", " 2160p", " 4K", " WEB-DL", " WEBDL", " BluRay", " BDRip",
        " HDR", " x264", " x265", " H264", " H265", " HEVC", " DDP5 1", " DD5 1", " AAC",
    ] {
        if let Some(idx) = out.to_lowercase().find(tag.to_lowercase().as_str()) {
            out.truncate(idx);
        }
    }
    out.trim().to_string()
}

/// Walk the configured chain. Returns the first agent that produced a match,
/// alongside the meta it returned. `None` only when every agent declined — in
/// practice the filename parser is always last and always answers, so the
/// `Option` exists for chains that omit it.
pub async fn resolve_movie(
    chain: &[Box<dyn MovieAgent>],
    title: &str,
    year: Option<i64>,
) -> Result<Option<AgentHit<MovieMeta>>> {
    for agent in chain {
        if let Some(meta) = agent.lookup(title, year).await? {
            return Ok(Some(AgentHit {
                agent: agent.id().as_str(),
                meta,
            }));
        }
    }
    Ok(None)
}

pub async fn resolve_show(
    chain: &[Box<dyn ShowAgent>],
    title: &str,
) -> Result<Option<AgentHit<ShowMeta>>> {
    for agent in chain {
        if let Some(meta) = agent.lookup(title).await? {
            return Ok(Some(AgentHit {
                agent: agent.id().as_str(),
                meta,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct StubMovieAgent {
        id: AgentId,
        out: Option<MovieMeta>,
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl MovieAgent for StubMovieAgent {
        fn id(&self) -> AgentId {
            self.id
        }
        async fn lookup(&self, _: &str, _: Option<i64>) -> Result<Option<MovieMeta>> {
            *self.calls.lock().unwrap() += 1;
            Ok(self.out.clone())
        }
    }

    #[test]
    fn filename_parser_extracts_title_and_year() {
        let m = parse_filename("The.Matrix.1999.1080p.BluRay.x264.mkv");
        assert_eq!(m.title.trim(), "The Matrix");
        assert_eq!(m.year, Some(1999));
    }

    #[test]
    fn filename_parser_handles_no_year() {
        let m = parse_filename("Some.Random.Clip.mkv");
        assert_eq!(m.year, None);
        assert!(m.title.contains("Some"));
    }

    #[test]
    fn filename_parser_strips_release_tags() {
        let m = parse_filename("Dune.Part.Two.2024.2160p.WEB-DL.DDP5.1.x265.mkv");
        assert_eq!(m.year, Some(2024));
        assert!(!m.title.to_lowercase().contains("2160p"));
        assert!(!m.title.to_lowercase().contains("x265"));
    }

    #[tokio::test]
    async fn chain_short_circuits_on_first_hit() {
        let first = StubMovieAgent {
            id: AgentId::Tmdb,
            out: Some(MovieMeta {
                title: "Matrix".into(),
                year: Some(1999),
                ..MovieMeta::default()
            }),
            calls: Mutex::new(0),
        };
        let second = StubMovieAgent {
            id: AgentId::Omdb,
            out: Some(MovieMeta::default()),
            calls: Mutex::new(0),
        };
        let chain: Vec<Box<dyn MovieAgent>> = vec![Box::new(first), Box::new(second)];
        let hit = resolve_movie(&chain, "matrix", None).await.unwrap().unwrap();
        assert_eq!(hit.agent, "tmdb");
        assert_eq!(hit.meta.title, "Matrix");
    }

    #[tokio::test]
    async fn chain_falls_through_to_filename() {
        let no_hit = StubMovieAgent {
            id: AgentId::Tmdb,
            out: None,
            calls: Mutex::new(0),
        };
        let chain: Vec<Box<dyn MovieAgent>> =
            vec![Box::new(no_hit), Box::new(FilenameAgent)];
        let hit = resolve_movie(&chain, "Inception.2010.mkv", None).await.unwrap().unwrap();
        assert_eq!(hit.agent, "filename");
        assert_eq!(hit.meta.year, Some(2010));
    }

    #[tokio::test]
    async fn empty_chain_returns_none() {
        let chain: Vec<Box<dyn MovieAgent>> = vec![];
        let hit = resolve_movie(&chain, "x", None).await.unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn agent_id_string_round_trip() {
        assert_eq!(AgentId::Tmdb.as_str(), "tmdb");
        assert_eq!(AgentId::Omdb.as_str(), "omdb");
        assert_eq!(AgentId::FilenameParser.as_str(), "filename");
        assert_eq!(AgentId::Custom(7).as_str(), "custom-7");
    }
}
