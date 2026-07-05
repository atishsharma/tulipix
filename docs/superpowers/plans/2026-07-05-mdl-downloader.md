# Music Downloader Tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a native "Downloader" tab to the Music section's My Music sub-tab row that resolves a streaming URL (9 providers), downloads audio via the bundled yt-dlp, tags it, and surfaces the tracks in the library in real time.

**Architecture:** New pure-Rust library crate `tulipix-mdl` ports mdl's provider scrapers (URL → track list) and a download pipeline built on the app's existing bundled yt-dlp + ffmpeg. `tulipix-sec-music` provides the Slint glue: folder picker (default = system Music dir), watched-folder registration, and a silent library refresh after each completed track. `.slint` supplies the tab + panel UI.

**Tech Stack:** Rust, tokio + reqwest (async HTTP scraping), regex + serde_json (parse embedded JSON / meta tags), lofty (tag + cover write), yt-dlp (search + Opus extract) via `tulipix_core::thumbs::tool_bin`, rfd (folder dialog), Slint (UI).

**Reference source (port from):** `mdl/packages/cli/src/lib/` — providers in `providers/*.ts`, pipeline in `sync.ts`, tagging in `metadata.ts`, search/download in `youtube.ts`, schema in `schemas.ts`. Each `*.test.ts` carries inline HTML fixtures — port those fixtures verbatim into the Rust tests as the passing contract.

---

## File Structure

**New crate `crates/tulipix-mdl/`:**
- `Cargo.toml` — deps: reqwest, tokio, serde/serde_json, regex, url, lofty, anyhow/thiserror, sanitize-filename (or a local sanitizer), tracing.
- `src/lib.rs` — public API re-exports: `detect_provider`, `resolve_url`, `download_playlist`, and the `Progress` type.
- `src/types.rs` — `ProviderId`, `Track`, `Playlist`, `Stage`, `Progress`, `DownloadOptions`, `Summary`.
- `src/resolve.rs` — provider dispatch + short-link resolution + URL normalization (`Providers.ts`).
- `src/provider/mod.rs` — `Provider` trait + registry + `get_first_non_empty` helper (`utils.ts`).
- `src/provider/spotify.rs` … `tidal.rs` — one file per provider (9).
- `src/download.rs` — per-track search → download → tag → manifest → progress (`sync.ts` + `youtube.ts` + `metadata.ts`).
- `src/manifest.rs` — `.mdl.json` load/save/upsert (`manifest.ts`).

**Modified:**
- `Cargo.toml` (workspace) — add `crates/tulipix-mdl` to members.
- `crates/tulipix-sec-music/Cargo.toml` — depend on `tulipix-mdl`.
- `crates/tulipix-sec-music/src/lib.rs` — add `mdl.rs` glue module + `pub mod mdl;`.
- `crates/tulipix-sec-music/src/mdl.rs` (new) — Slint callback handlers: detect/resolve/download/pick-folder, watched-folder add, silent refresh per track, progress → model.
- `ui/page_music.slint` — add `downloader` sub-tab pill + Downloader panel; new properties + callbacks.
- `crates/tulipix-app/src/main.rs` — wire the new Slint callbacks to `tulipix_sec_music::mdl::*` (glob-import pattern already used).
- `crates/tulipix-common/src/lib.rs` — add `save_watched_folders` / `add_watched_folder` persist helper next to `load_watched_folders` (line 243).

**Note on rebuilds:** `.slint` edits hot-reload (no rebuild). Rust changes need one capped build per batch — follow the `capped-build-command` memory (`-j 1`, swap cap, watchdog). Never cold-build.

---

## Task 1: Scaffold the `tulipix-mdl` crate

**Files:**
- Create: `crates/tulipix-mdl/Cargo.toml`
- Create: `crates/tulipix-mdl/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create the crate manifest**

`crates/tulipix-mdl/Cargo.toml`:
```toml
[package]
name = "tulipix-mdl"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "fs", "time"] }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "gzip"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
regex = "1"
url = "2"
lofty = "0.21"
anyhow = "1"
thiserror = "1"
sanitize-filename = "0.5"
tracing = "0.1"

[dev-dependencies]
tokio = { version = "1", features = ["rt", "macros"] }
```
Match the workspace's existing reqwest TLS feature (check another crate's `Cargo.toml`; if the repo uses `native-tls`, mirror it instead of `rustls-tls`).

- [ ] **Step 2: Add the crate to the workspace**

In the root `Cargo.toml` `[workspace] members` list, add `"crates/tulipix-mdl"` (alphabetical with the other `crates/tulipix-*`).

- [ ] **Step 3: Minimal lib.rs**

`crates/tulipix-mdl/src/lib.rs`:
```rust
pub mod types;
pub mod provider;
pub mod resolve;
pub mod download;
pub mod manifest;

pub use resolve::{detect_provider, resolve_url};
pub use types::{Playlist, Progress, ProviderId, Stage, Track};
```
Create empty stub modules (`types.rs`, `provider/mod.rs`, `resolve.rs`, `download.rs`, `manifest.rs`) so it compiles; they are filled by later tasks. For now each stub can be a single line comment.

- [ ] **Step 4: Verify it builds**

Run (capped, per memory): `just check-dev-lite` (or the repo's capped `cargo check -p tulipix-mdl`).
Expected: compiles (empty modules).

- [ ] **Step 5: Commit**
```bash
git add crates/tulipix-mdl Cargo.toml
git commit -m "feat(mdl): scaffold tulipix-mdl crate"
```

---

## Task 2: Core types

**Files:**
- Modify: `crates/tulipix-mdl/src/types.rs`

- [ ] **Step 1: Define the data model** (port of `schemas.ts` + `types.ts`)

`crates/tulipix-mdl/src/types.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Spotify,
    AppleMusic,
    AmazonMusic,
    YoutubeMusic,
    Soundcloud,
    Bandcamp,
    Qobuz,
    Deezer,
    Tidal,
}

impl ProviderId {
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Spotify => "Spotify",
            ProviderId::AppleMusic => "Apple Music",
            ProviderId::AmazonMusic => "Amazon Music",
            ProviderId::YoutubeMusic => "YouTube Music",
            ProviderId::Soundcloud => "SoundCloud",
            ProviderId::Bandcamp => "Bandcamp",
            ProviderId::Qobuz => "Qobuz",
            ProviderId::Deezer => "Deezer",
            ProviderId::Tidal => "Tidal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
    pub provider: ProviderId,
    pub source_url: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Initializing,
    SearchingYoutube,
    DownloadingAudio,
    WritingMetadata,
    WritingManifest,
    Skipped,
    Failed,
    Completed,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub track_index: usize,
    pub total: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub stage: Stage,
    pub percent: f32,
    pub message: String,
    pub title: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub dest_dir: std::path::PathBuf,
    pub parallelism: usize, // default 5
}

#[derive(Debug, Default)]
pub struct Summary {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: Vec<(Track, String)>,
}
```

- [ ] **Step 2: Verify build**

Run: capped `cargo check -p tulipix-mdl`.
Expected: compiles.

- [ ] **Step 3: Commit**
```bash
git add crates/tulipix-mdl/src/types.rs
git commit -m "feat(mdl): core types (Track/Playlist/Progress)"
```

---

## Task 3: Provider trait + registry + helpers

**Files:**
- Modify: `crates/tulipix-mdl/src/provider/mod.rs`

- [ ] **Step 1: Define the trait and shared helpers** (port of `Providers.ts` interface + `utils.ts` `getFirstNonEmptyString`)

`crates/tulipix-mdl/src/provider/mod.rs`:
```rust
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub mod spotify;
pub mod apple_music;
pub mod amazon_music;
pub mod youtube_music;
pub mod soundcloud;
pub mod bandcamp;
pub mod qobuz;
pub mod deezer;
pub mod tidal;

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn short_link_hosts(&self) -> &'static [&'static str] { &[] }
    fn matches(&self, url: &Url) -> bool;
    /// Normalize a matched URL (strip query/hash, preserving provider-specific params).
    fn normalize(&self, url: &Url) -> String { strip_query_and_hash(url) }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist>;
}

pub fn registry() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(spotify::Spotify),
        Box::new(apple_music::AppleMusic),
        Box::new(amazon_music::AmazonMusic),
        Box::new(youtube_music::YoutubeMusic),
        Box::new(soundcloud::Soundcloud),
        Box::new(bandcamp::Bandcamp),
        Box::new(qobuz::Qobuz),
        Box::new(deezer::Deezer),
        Box::new(tidal::Tidal),
    ]
}

pub fn get_first_non_empty(candidates: &[Option<&str>]) -> Option<String> {
    candidates.iter().flatten().map(|s| s.trim()).find(|s| !s.is_empty()).map(String::from)
}

pub fn strip_query_and_hash(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}
```
Add `async-trait = "0.1"` to `Cargo.toml` `[dependencies]`.

- [ ] **Step 2: Add empty provider stub files** so the module tree compiles.

For each of the 9 provider files (`spotify.rs` … `tidal.rs`), create a stub that defines the unit struct and a `todo!()` fetch, e.g. `spotify.rs`:
```rust
use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct Spotify;

#[async_trait::async_trait]
impl Provider for Spotify {
    fn id(&self) -> ProviderId { ProviderId::Spotify }
    fn matches(&self, _url: &Url) -> bool { false }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("not implemented")
    }
}
```
Repeat with the matching struct name for each file (`AppleMusic`, `AmazonMusic`, `YoutubeMusic`, `Soundcloud`, `Bandcamp`, `Qobuz`, `Deezer`, `Tidal`), returning the correct `ProviderId`.

- [ ] **Step 3: Verify build**

Run: capped `cargo check -p tulipix-mdl`.
Expected: compiles.

- [ ] **Step 4: Commit**
```bash
git add crates/tulipix-mdl/src/provider crates/tulipix-mdl/Cargo.toml
git commit -m "feat(mdl): provider trait + registry + stubs"
```

---

## Task 4: URL detection + normalization + short links

**Files:**
- Modify: `crates/tulipix-mdl/src/resolve.rs`

- [ ] **Step 1: Write failing tests** (port of `Providers.test.ts`)

At the bottom of `crates/tulipix-mdl/src/resolve.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProviderId;

    #[test]
    fn detects_spotify_album() {
        assert_eq!(
            detect_provider("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3"),
            Some(ProviderId::Spotify)
        );
    }

    #[test]
    fn detects_apple_music_playlist() {
        assert_eq!(
            detect_provider("https://music.apple.com/us/playlist/foo/pl.u-abc123"),
            Some(ProviderId::AppleMusic)
        );
    }

    #[test]
    fn rejects_unknown() {
        assert_eq!(detect_provider("https://example.com/song/1"), None);
    }
}
```
Port additional per-provider positive/negative cases from each `providers/*.test.ts` `matchesUrl` block as the providers are implemented (Tasks 5–13); add them here.

- [ ] **Step 2: Run tests, verify fail**

Run: capped `cargo test -p tulipix-mdl resolve`.
Expected: FAIL (`detect_provider` not defined).

- [ ] **Step 3: Implement detection + normalization**

Top of `crates/tulipix-mdl/src/resolve.rs`:
```rust
use crate::provider::{registry, Provider};
use crate::types::{Playlist, ProviderId};
use anyhow::{anyhow, Result};
use url::Url;

fn parse_http(url: &str) -> Option<Url> {
    let u = Url::parse(url.trim()).ok()?;
    matches!(u.scheme(), "http" | "https").then_some(u)
}

pub fn detect_provider(url: &str) -> Option<ProviderId> {
    let u = parse_http(url)?;
    let reg = registry();
    if let Some(p) = reg.iter().find(|p| p.matches(&u)) {
        return Some(p.id());
    }
    // short-link host match (resolution happens in resolve_url)
    let host = u.host_str()?.to_lowercase();
    reg.iter()
        .find(|p| p.short_link_hosts().iter().any(|h| *h == host))
        .map(|p| p.id())
}

/// Resolve a URL to a Playlist. Follows short links first (HTTP redirect),
/// then dispatches to the matching provider.
pub async fn resolve_url(client: &reqwest::Client, url: &str) -> Result<Playlist> {
    let u = parse_http(url).ok_or_else(|| anyhow!("Invalid URL. Provide a full http(s) music URL."))?;
    let reg = registry();

    if let Some(p) = reg.iter().find(|p| p.matches(&u)) {
        let normalized = p.normalize(&u);
        return p.fetch(client, &normalized).await;
    }

    // Short link: follow redirects, re-detect.
    let host = u.host_str().unwrap_or("").to_lowercase();
    let is_short = reg.iter().any(|p| p.short_link_hosts().iter().any(|h| *h == host));
    if !is_short {
        return Err(anyhow!("Unsupported music URL."));
    }
    let resp = client.get(u.as_str()).header("user-agent", "mdl/0.1").send().await?;
    let final_url = resp.url().clone();
    let fu = Url::parse(final_url.as_str())?;
    let p = reg.iter().find(|p| p.matches(&fu))
        .ok_or_else(|| anyhow!("Could not resolve the short link to a supported music URL."))?;
    let normalized = p.normalize(&fu);
    p.fetch(client, &normalized).await
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: capped `cargo test -p tulipix-mdl resolve`.
Expected: PASS (the three tests; provider-specific matches implemented in later tasks).
Note: `detects_apple_music_playlist` passes only once `AppleMusic::matches` is real (Task 6). Until then keep only the Spotify + unknown cases, add the Apple case in Task 6.

- [ ] **Step 5: Commit**
```bash
git add crates/tulipix-mdl/src/resolve.rs
git commit -m "feat(mdl): URL detection + short-link resolution"
```

---

## Task 5: Spotify provider (exemplar — full port)

**Files:**
- Modify: `crates/tulipix-mdl/src/provider/spotify.rs`

Port `mdl/packages/cli/src/lib/providers/Spotify.ts`. Strategy: hit `https://open.spotify.com/embed/{kind}/{id}`, extract the `__NEXT_DATA__` `<script>` JSON, deserialize `props.pageProps.state.data.entity`, normalize into tracks. Playlist enrichment (per-track album/artwork from og-meta) is optional for v1 — implement the embed parse first (covers album/track fully and playlists at reduced metadata).

- [ ] **Step 1: Write failing test** (fixture ported verbatim from `Spotify.test.ts`)

Bottom of `spotify.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_from_embed() {
        let html = r#"<!DOCTYPE html><html><body>
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"type":"track","name":"Kookaburra Sits","uri":"spotify:track:1eJdXVLxLoMWu1TkaeSL18","id":"1eJdXVLxLoMWu1TkaeSL18","title":"Kookaburra Sits","artists":[{"name":"ABC Kids","uri":"spotify:artist:6l7J2uM3bM2BCh0tIPhWx8"}],"duration":57720,"visualIdentity":{"image":[{"url":"https://image-cdn-fa.spotifycdn.com/image/track-art"}]}}}}}}}</script>
</body></html>"#;
        let pl = Spotify::parse_collection_html(html, "https://open.spotify.com/track/1eJdXVLxLoMWu1TkaeSL18").unwrap();
        assert_eq!(pl.id, "1eJdXVLxLoMWu1TkaeSL18");
        assert_eq!(pl.title, "Kookaburra Sits");
        assert_eq!(pl.owner.as_deref(), Some("ABC Kids"));
        assert_eq!(pl.artwork_url.as_deref(), Some("https://image-cdn-fa.spotifycdn.com/image/track-art"));
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.tracks[0].title, "Kookaburra Sits");
        assert_eq!(pl.tracks[0].artists, vec!["ABC Kids".to_string()]);
        assert_eq!(pl.tracks[0].duration_ms, Some(57720));
    }

    #[test]
    fn matches_spotify_urls() {
        use url::Url;
        assert!(Spotify.matches(&Url::parse("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3").unwrap()));
        assert!(!Spotify.matches(&Url::parse("https://open.spotify.com/artist/abc").unwrap()));
    }
}
```
Port the remaining `Spotify.test.ts` cases (playlist `trackList`, album) the same way.

- [ ] **Step 2: Run test, verify fail**

Run: capped `cargo test -p tulipix-mdl spotify`.
Expected: FAIL (`parse_collection_html` not defined).

- [ ] **Step 3: Implement** the port. Full structure:
```rust
use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use url::Url;

pub struct Spotify;

#[derive(Deserialize)]
struct Payload { props: Option<Props> }
#[derive(Deserialize)]
struct Props { #[serde(rename = "pageProps")] page_props: Option<PageProps> }
#[derive(Deserialize)]
struct PageProps { state: Option<State> }
#[derive(Deserialize)]
struct State { data: Option<Data> }
#[derive(Deserialize)]
struct Data { entity: Option<Entity> }
#[derive(Deserialize)]
struct Entity {
    #[serde(rename = "type")] kind: Option<String>,
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    title: Option<String>,
    subtitle: Option<String>,
    duration: Option<u64>,
    artists: Option<Vec<Artist>>,
    #[serde(rename = "trackList")] track_list: Option<Vec<TrackItem>>,
    #[serde(rename = "coverArt")] cover_art: Option<CoverArt>,
    #[serde(rename = "visualIdentity")] visual_identity: Option<VisualIdentity>,
}
#[derive(Deserialize)] struct Artist { name: Option<String> }
#[derive(Deserialize)] struct TrackItem { title: Option<String>, subtitle: Option<String>, uri: Option<String>, duration: Option<u64> }
#[derive(Deserialize)] struct CoverArt { sources: Option<Vec<ImgSrc>> }
#[derive(Deserialize)] struct VisualIdentity { image: Option<Vec<ImgSrc>> }
#[derive(Deserialize)] struct ImgSrc { url: Option<String> }

impl Spotify {
    fn collection_kind(url: &str) -> &'static str {
        let path = Url::parse(url).map(|u| u.path().to_string()).unwrap_or_default();
        if path.contains("/album/") { "album" }
        else if path.contains("/track/") { "track" }
        else { "playlist" }
    }

    fn extract_id(value: &str, kind: &str) -> Option<String> {
        let re = Regex::new(&format!(r"(?:{kind}/|spotify:{kind}:)([A-Za-z0-9]+)")).ok()?;
        re.captures(value).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
    }

    pub fn parse_collection_html(html: &str, source_url: &str) -> Result<Playlist> {
        let re = Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap();
        let json = re.captures(html).and_then(|c| c.get(1))
            .ok_or_else(|| anyhow!("Could not find Spotify collection data in the page."))?;
        let payload: Payload = serde_json::from_str(json.as_str().trim())?;
        let entity = payload.props.and_then(|p| p.page_props).and_then(|p| p.state)
            .and_then(|s| s.data).and_then(|d| d.entity)
            .ok_or_else(|| anyhow!("Could not parse Spotify collection data."))?;

        let kind = Self::collection_kind(source_url);
        let title = get_first_non_empty(&[entity.title.as_deref(), entity.name.as_deref()])
            .unwrap_or_else(|| format!("Spotify {kind}"));
        let artwork = get_first_non_empty(&[
            entity.cover_art.as_ref().and_then(|c| c.sources.as_ref()).and_then(|s| s.first()).and_then(|s| s.url.as_deref()),
            entity.visual_identity.as_ref().and_then(|v| v.image.as_ref()).and_then(|i| i.first()).and_then(|i| i.url.as_deref()),
        ]);
        let owner = get_first_non_empty(&[entity.subtitle.as_deref()]).or_else(|| {
            entity.artists.as_ref().map(|a| a.iter().filter_map(|x| x.name.as_deref()).collect::<Vec<_>>().join(", "))
                .filter(|s| !s.is_empty())
        });

        let tracks: Vec<Track> = if kind == "track" {
            let artists: Vec<String> = entity.artists.as_ref().map(|a| a.iter().filter_map(|x| x.name.as_ref().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
            let id = entity.id.clone().or_else(|| entity.uri.as_deref().and_then(|u| Self::extract_id(u, "track")));
            match (title.is_empty(), artists.is_empty(), id) {
                (false, false, Some(id)) => vec![Track {
                    id, title: title.clone(), artists, album: None,
                    artwork_url: artwork.clone(), duration_ms: entity.duration,
                    source_url: Some(source_url.to_string()),
                }],
                _ => vec![],
            }
        } else {
            entity.track_list.unwrap_or_default().into_iter().filter_map(|t| {
                let ttitle = t.title.as_ref()?.trim().to_string();
                if ttitle.is_empty() { return None; }
                let artists: Vec<String> = t.subtitle.as_deref().unwrap_or("").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if artists.is_empty() { return None; }
                let id = t.uri.as_deref().and_then(|u| Self::extract_id(u, "track")).unwrap_or_else(|| format!("{}-{}", artists.join(","), ttitle));
                Some(Track { id, title: ttitle, artists, album: Some(title.clone()), artwork_url: artwork.clone(), duration_ms: t.duration, source_url: t.uri.as_deref().and_then(|u| Self::extract_id(u, "track")).map(|id| format!("https://open.spotify.com/track/{id}")) })
            }).collect()
        };

        if tracks.is_empty() { bail!("No tracks were found in the Spotify {kind}."); }
        let id = entity.id.filter(|s| !s.trim().is_empty())
            .or_else(|| entity.uri.as_deref().and_then(|u| Self::extract_id(u, kind)))
            .or_else(|| Self::extract_id(source_url, kind))
            .unwrap_or_else(|| format!("{kind}-spotify"));
        Ok(Playlist { id, title, owner, artwork_url: artwork, provider: ProviderId::Spotify, source_url: source_url.to_string(), tracks })
    }
}

#[async_trait::async_trait]
impl Provider for Spotify {
    fn id(&self) -> ProviderId { ProviderId::Spotify }
    fn short_link_hosts(&self) -> &'static [&'static str] { &["spotify.link", "spotify.app.link"] }
    fn matches(&self, url: &Url) -> bool {
        if url.host_str().map(|h| h.to_lowercase()) != Some("open.spotify.com".into()) { return false; }
        let path = url.path().trim_end_matches('/');
        let pats = [
            r"^/album/[A-Za-z0-9]+$", r"^/playlist/[A-Za-z0-9]+$", r"^/track/[A-Za-z0-9]+$",
            r"^/intl-[a-z]{2}/album/[A-Za-z0-9]+$", r"^/intl-[a-z]{2}/track/[A-Za-z0-9]+$",
            r"^/user/[^/]+/playlist/[A-Za-z0-9]+$", r"^/intl-[a-z]{2}/playlist/[A-Za-z0-9]+$",
        ];
        pats.iter().any(|p| Regex::new(p).unwrap().is_match(path))
    }
    fn normalize(&self, url: &Url) -> String { strip_query_and_hash(url) }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let kind = Self::collection_kind(url);
        let id = Self::extract_id(url, kind).ok_or_else(|| anyhow!("Could not determine the Spotify collection id."))?;
        let embed = format!("https://open.spotify.com/embed/{kind}/{id}");
        let html = client.get(&embed).header("user-agent", "Mozilla/5.0").send().await?.error_for_status()?.text().await?;
        Self::parse_collection_html(&html, url)
    }
}
```

- [ ] **Step 4: Run tests, verify pass**

Run: capped `cargo test -p tulipix-mdl spotify`.
Expected: PASS.

- [ ] **Step 5: Add the Apple case back to resolve tests? No — commit Spotify.**
```bash
git add crates/tulipix-mdl/src/provider/spotify.rs
git commit -m "feat(mdl): Spotify provider (embed scrape)"
```

---

## Tasks 6–13: Remaining 8 providers (parallel port tasks)

Each task follows the **exact same shape as Task 5**. One task per provider, in this order (usability first): Apple Music, YouTube Music, SoundCloud, Deezer, Amazon Music, Qobuz, Bandcamp, Tidal.

**For each provider `X`:**

**Files:** Modify `crates/tulipix-mdl/src/provider/<x>.rs`.

- [ ] **Step 1: Port the tests.** Copy every inline HTML/JSON fixture and every `matchesUrl` assertion from `mdl/packages/cli/src/lib/providers/X.test.ts` into a `#[cfg(test)] mod tests` block, translating `expect(...)` → `assert_eq!(...)`. These fixtures are the passing contract — do not invent new ones.
- [ ] **Step 2: Run, verify fail.** `cargo test -p tulipix-mdl <x>` → FAIL.
- [ ] **Step 3: Port the scraper** from `mdl/packages/cli/src/lib/providers/X.ts`:
  - Translate `matchesUrl` regexes 1:1 (Rust `regex` crate; JS `(?:...)?` groups map directly).
  - Translate `fetch` + parse: for JSON-in-`<script>` providers (Apple `serialized-server-data`, YouTube Music `ytInitialData`, Amazon, Tidal, Qobuz, Deezer API JSON) use a `regex` to extract the blob + `serde_json` structs mirroring the TS `type` declarations at the bottom of each file. For meta-tag providers (Bandcamp `data-tralbum`, SoundCloud hydration) mirror the TS extraction.
  - Reuse `get_first_non_empty`, `strip_query_and_hash`. Implement provider-specific `normalize` where the TS `normalizeProviderUrl` preserves params (Apple `i`, Amazon `trackAsin`, YouTube Music `v`/`list`) — override `normalize` accordingly.
  - Set the correct `ProviderId` and `short_link_hosts` (Apple `apple.co`).
- [ ] **Step 4: Run, verify pass.** `cargo test -p tulipix-mdl <x>` → PASS.
- [ ] **Step 5: Add that provider's positive/negative URL case to `resolve.rs` tests; run `cargo test -p tulipix-mdl resolve` → PASS.**
- [ ] **Step 6: Commit** `git commit -m "feat(mdl): <X> provider"`.

Task 6 Apple Music · Task 7 YouTube Music · Task 8 SoundCloud · Task 9 Deezer · Task 10 Amazon Music · Task 11 Qobuz · Task 12 Bandcamp · Task 13 Tidal.

**Provider-specific porting notes (from source review):**
- **Apple Music** (`AppleMusic.ts`, 311 LOC): parses `<script id="serialized-server-data">` JSON + `<meta>` fallbacks; keeps `?i=` for single-song URLs. `short_link_hosts`: `apple.co`.
- **YouTube Music** (`YouTubeMusic.ts`, 317): extracts `ytInitialData` blob; preserve `v` + `list` query params in `normalize`.
- **Bandcamp** (`Bandcamp.ts`, 529 — largest): parses `data-tralbum` attribute JSON; album vs track handling.
- **Tidal** (`Tidal.ts`, 500) / **Qobuz** (298) / **Deezer** (217): public JSON API / embedded JSON.
- **Amazon Music** (`AmazonMusic.ts`, 351): keeps `?trackAsin=`; embedded app-state JSON.
- **SoundCloud** (`SoundCloud.ts`, 183 — smallest, good second port): hydration JSON in page.

---

## Task 14: Manifest (skip / resync)

**Files:** Modify `crates/tulipix-mdl/src/manifest.rs`

- [ ] **Step 1: Write failing test**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upsert_replaces_by_source_id() {
        let mut m = Manifest::new(crate::types::ProviderId::Spotify, "pid", "Title", "url");
        m.upsert(ManifestTrack { source_track_id: "t1".into(), file_name: "a.opus".into(), relative_path: "a.opus".into() });
        m.upsert(ManifestTrack { source_track_id: "t1".into(), file_name: "b.opus".into(), relative_path: "b.opus".into() });
        assert_eq!(m.tracks.len(), 1);
        assert_eq!(m.tracks[0].file_name, "b.opus");
    }
}
```
- [ ] **Step 2: Run, verify fail.** `cargo test -p tulipix-mdl manifest` → FAIL.
- [ ] **Step 3: Implement** (port of `manifest.ts`): `Manifest` struct (serde) with `version:1`, provider, playlist id/title/url, `generated_at`, `Vec<ManifestTrack>` (`source_track_id`, `file_name`, `relative_path`, `downloaded_at`, + track meta). Fns: `MANIFEST_FILE_NAME = ".mdl.json"`, `load(dir) -> Option<Manifest>`, `save(dir, &Manifest)`, `upsert(&mut self, track)`, `contains(&self, source_track_id) -> Option<&ManifestTrack>`.
- [ ] **Step 4: Run, verify pass.** → PASS.
- [ ] **Step 5: Commit** `git commit -m "feat(mdl): sync manifest"`.

---

## Task 15: Download pipeline

**Files:** Modify `crates/tulipix-mdl/src/download.rs`

Ports `sync.ts` (orchestration) + `youtube.ts` (search/download via yt-dlp instead of youtubei.js) + `metadata.ts` (tagging via lofty).

- [ ] **Step 1: Write failing test** (filename builder — the pure unit worth testing offline)
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Track;
    #[test]
    fn builds_sanitized_filename() {
        let t = Track { id: "1".into(), title: "Song/Name".into(), artists: vec!["A".into(), "B".into()], album: None, artwork_url: None, duration_ms: None, source_url: None };
        assert_eq!(track_file_stem(3, &t), "03 - A, B - SongName");
    }
}
```
- [ ] **Step 2: Run, verify fail.** `cargo test -p tulipix-mdl download` → FAIL.
- [ ] **Step 3: Implement**
```rust
use crate::manifest::{Manifest, ManifestTrack, MANIFEST_FILE_NAME};
use crate::types::{DownloadOptions, Playlist, Progress, Stage, Summary, Track};
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub fn track_file_stem(index: usize, track: &Track) -> String {
    let raw = format!("{:02} - {} - {}", index, track.artists.join(", "), track.title);
    sanitize_filename::sanitize(raw)
}

/// yt-dlp query "artist - title".
fn search_query(track: &Track) -> String {
    format!("{} - {}", track.artists.join(", "), track.title)
}

/// Resolve the bundled yt-dlp path (same helper the YouTube section uses).
fn ytdlp_bin() -> std::ffi::OsString { tulipix_core::thumbs::tool_bin("yt-dlp") }

/// Download one track's audio into `dest` as Opus, return the produced file path.
async fn download_audio(track: &Track, index: usize, dest: &Path) -> Result<PathBuf> {
    let stem = track_file_stem(index, track);
    let out_tmpl = dest.join(format!("{stem}.%(ext)s"));
    let query = format!("ytsearch1:{}", search_query(track));
    let status = tokio::process::Command::new(ytdlp_bin())
        .arg(&query)
        .args(["-f", "bestaudio", "-x", "--audio-format", "opus"])
        .args(["--no-playlist", "--no-progress", "-o"])
        .arg(&out_tmpl)
        .output().await?;
    if !status.status.success() {
        return Err(anyhow!("yt-dlp failed: {}", String::from_utf8_lossy(&status.stderr)));
    }
    let path = dest.join(format!("{stem}.opus"));
    if !path.exists() { return Err(anyhow!("expected output file missing: {}", path.display())); }
    Ok(path)
}

/// Write title/artist/album tags + cover art via lofty.
async fn write_tags(path: &Path, track: &Track, client: &reqwest::Client) -> Result<()> {
    let cover = match &track.artwork_url {
        Some(u) => client.get(u).send().await.ok().and_then(|r| r.error_for_status().ok())
            .and_then(|r| futures_lite::future::block_on(r.bytes()).ok()) // see note
            .map(|b| b.to_vec()),
        None => None,
    };
    let path = path.to_path_buf();
    let (title, artist, album) = (track.title.clone(), track.artists.join(", "), track.album.clone());
    tokio::task::spawn_blocking(move || -> Result<()> {
        use lofty::{prelude::*, probe::Probe, tag::Tag, picture::{Picture, MimeType, PictureType}};
        let mut tagged = Probe::open(&path)?.read()?;
        let tag = tagged.primary_tag_mut().map(|t| t as &mut Tag)
            .ok_or_else(|| anyhow!("no primary tag"))?;
        // NOTE: exact lofty 0.21 API for insert_text/first_tag — adapt to the
        // version resolved. Set TITLE, ARTIST, ALBUM item keys.
        tag.set_title(title);
        tag.set_artist(artist);
        if let Some(al) = album { tag.set_album(al); }
        if let Some(bytes) = cover {
            let pic = Picture::new_unchecked(PictureType::CoverFront, Some(MimeType::Jpeg), None, bytes);
            tag.push_picture(pic);
        }
        tagged.save_to_path(&path)?;
        Ok(())
    }).await??;
    Ok(())
}
```
Implementation note for the porter: the cover fetch above must be `async` — replace the `block_on` sketch with a proper `let cover = ...; let bytes = resp.bytes().await?;` before the `spawn_blocking`, passing the `Vec<u8>` in. lofty setter names (`set_title` etc.) must match the resolved lofty 0.21 API; if the convenience setters differ, use `Tag::insert_text(ItemKey::TrackTitle, ...)`. Verify against `mcp__context7` lofty docs during implementation.

- [ ] **Step 4: Implement the orchestrator**
```rust
/// Download a whole playlist. Calls `on_progress` for each stage transition.
pub async fn download_playlist<F>(
    client: &reqwest::Client,
    playlist: &Playlist,
    opts: &DownloadOptions,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    on_progress: F,
) -> Summary
where F: Fn(Progress) + Send + Sync + 'static {
    let on_progress = Arc::new(on_progress);
    tokio::fs::create_dir_all(&opts.dest_dir).await.ok();
    let mut manifest = crate::manifest::load(&opts.dest_dir)
        .unwrap_or_else(|| Manifest::new(playlist.provider, &playlist.id, &playlist.title, &playlist.source_url));
    let total = playlist.tracks.len();
    let sem = Arc::new(Semaphore::new(opts.parallelism.max(1)));
    let counters = Arc::new(std::sync::Mutex::new((0usize, 0usize, Vec::<(Track, String)>::new()))); // downloaded, skipped, failed

    let mut handles = Vec::new();
    for (i, track) in playlist.tracks.iter().cloned().enumerate() {
        let (sem, client, dest, cancel, on_progress, counters) =
            (sem.clone(), client.clone(), opts.dest_dir.clone(), cancel.clone(), on_progress.clone(), counters.clone());
        let existing = manifest.contains(&track.id).map(|m| dest.join(&m.relative_path)).filter(|p| p.exists()).is_some();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            if cancel.load(std::sync::atomic::Ordering::Relaxed) { return None; }
            let idx = i + 1;
            let emit = |stage, percent, msg: String, file: Option<String>| on_progress(Progress {
                track_index: idx, total, downloaded: 0, skipped: 0, failed: 0,
                stage, percent, message: msg, title: track.title.clone(), file_name: file });
            if existing {
                { let mut c = counters.lock().unwrap(); c.1 += 1; }
                emit(Stage::Skipped, 100.0, "Already downloaded".into(), None);
                return None;
            }
            emit(Stage::SearchingYoutube, 20.0, "Searching YouTube".into(), None);
            match download_audio(&track, idx, &dest).await {
                Ok(path) => {
                    emit(Stage::WritingMetadata, 90.0, "Embedding metadata".into(), None);
                    let _ = write_tags(&path, &track, &client).await;
                    let file_name = path.file_name().unwrap().to_string_lossy().to_string();
                    { let mut c = counters.lock().unwrap(); c.0 += 1; }
                    emit(Stage::Completed, 100.0, file_name.clone(), Some(file_name.clone()));
                    Some((track, file_name))
                }
                Err(e) => {
                    { let mut c = counters.lock().unwrap(); c.2.push((track.clone(), e.to_string())); }
                    emit(Stage::Failed, 100.0, e.to_string(), None);
                    None
                }
            }
        }));
    }

    for h in handles {
        if let Ok(Some((track, file_name))) = h.await {
            manifest.upsert(ManifestTrack::from_track(&track, &file_name));
            let _ = crate::manifest::save(&opts.dest_dir, &manifest);
        }
    }
    let (downloaded, skipped, failed) = { let c = counters.lock().unwrap(); (c.0, c.1, c.2.clone()) };
    Summary { downloaded, skipped, failed }
}
```
Add `ManifestTrack::from_track(&Track, &file_name)` to `manifest.rs`. Add `futures`/`tokio` features as needed. Add `tulipix-core` + `tulipix-common` as path deps in `tulipix-mdl/Cargo.toml` for `tool_bin`.

- [ ] **Step 5: Run tests, verify pass.** `cargo test -p tulipix-mdl` (all) → PASS.
- [ ] **Step 6: Commit** `git commit -m "feat(mdl): download pipeline (yt-dlp + lofty tagging)"`.

---

## Task 16: Watched-folder persist helper

**Files:** Modify `crates/tulipix-common/src/lib.rs` (near line 243)

- [ ] **Step 1: Write failing test** (in `tulipix-common` tests, tmp-dir based)
```rust
#[test]
fn add_watched_folder_persists_unique() {
    // set config dir to a tmp path via existing test hook if present; else test the
    // pure dedup helper `merge_watched(existing, new) -> Vec<PathBuf>`.
    let out = merge_watched(vec!["/a".into()], std::path::Path::new("/b"));
    assert_eq!(out, vec![std::path::PathBuf::from("/a"), "/b".into()]);
    let same = merge_watched(vec!["/a".into()], std::path::Path::new("/a"));
    assert_eq!(same, vec![std::path::PathBuf::from("/a")]);
}
```
- [ ] **Step 2: Run, verify fail.** capped `cargo test -p tulipix-common merge_watched` → FAIL.
- [ ] **Step 3: Implement** next to `load_watched_folders`:
```rust
pub fn merge_watched(mut existing: Vec<PathBuf>, add: &std::path::Path) -> Vec<PathBuf> {
    if !existing.iter().any(|p| p == add) { existing.push(add.to_path_buf()); }
    existing
}

pub fn save_watched_folders(folders: &[PathBuf]) {
    if let Some(p) = watched_folders_path() {
        let list: Vec<String> = folders.iter().map(|p| p.display().to_string()).collect();
        if let Ok(body) = serde_json::to_string_pretty(&list) { let _ = std::fs::write(p, body); }
    }
}

/// Add `dir` to the watched set (idempotent) and persist. Returns true if newly added.
pub fn add_watched_folder(dir: &std::path::Path) -> bool {
    let existing = load_watched_folders();
    let had = existing.iter().any(|p| p == dir);
    if !had { save_watched_folders(&merge_watched(existing, dir)); }
    !had
}
```
- [ ] **Step 4: Run, verify pass.** → PASS.
- [ ] **Step 5: Commit** `git commit -m "feat(common): add_watched_folder persist helper"`.

---

## Task 17: sec-music glue module

**Files:**
- Create: `crates/tulipix-sec-music/src/mdl.rs`
- Modify: `crates/tulipix-sec-music/src/lib.rs` (add `pub mod mdl;`)
- Modify: `crates/tulipix-sec-music/Cargo.toml` (add `tulipix-mdl` path dep)

- [ ] **Step 1: Default system Music dir helper**

In `mdl.rs`:
```rust
use slint::Weak;

/// System Music dir (XDG_MUSIC_DIR / ~/Music), falling back to ~/Music.
pub fn default_music_dir() -> std::path::PathBuf {
    dirs::audio_dir().unwrap_or_else(|| {
        dirs::home_dir().unwrap_or_default().join("Music")
    })
}
```
Add `dirs = "5"` to `tulipix-sec-music/Cargo.toml` if not already a workspace dep.

- [ ] **Step 2: Detect handler** (called on URL input change; updates a badge property)
```rust
pub fn detect(url: &str) -> String {
    match tulipix_mdl::detect_provider(url) {
        Some(p) => p.display_name().to_string(),
        None => String::new(),
    }
}
```

- [ ] **Step 3: Resolve + download driver.** Spawn a tokio task (reuse the section's existing runtime handle — follow how `tulipix-music` piped calls spawn async; if the app uses a shared `tokio::runtime::Handle`, use it, else `std::thread` + a current-thread runtime). On progress, marshal back to the UI thread with `slint::invoke_from_event_loop` and update the Downloader model rows + counters. After each `Stage::Completed`, call the app's silent library refresh so the new file appears (invoke the existing `refresh_library_silent` path — expose it via a callback the glue can trigger, see Task 19).
```rust
pub fn start_download(win: Weak<MainWindow>, url: String, dest: std::path::PathBuf) {
    // 1. add_watched_folder(default_music_dir()); add_watched_folder(&dest);
    // 2. spawn: client = reqwest::Client; playlist = resolve_url(url).await?;
    //    push tracklist preview to model via invoke_from_event_loop.
    // 3. download_playlist(..., on_progress = |p| invoke_from_event_loop(update row)).
    // 4. on each Completed -> trigger silent refresh callback.
}
```
Exact model field names come from the Slint properties defined in Task 18 — fill them in once those exist. Keep this module free of business logic beyond marshalling; the pipeline lives in `tulipix-mdl`.

- [ ] **Step 4: Verify build.** capped `cargo check -p tulipix-sec-music`.
- [ ] **Step 5: Commit** `git commit -m "feat(music): mdl glue module"`.

---

## Task 18: Slint UI — Downloader tab (hot-reload, no rebuild)

**Files:** Modify `ui/page_music.slint`

Follow the `slint-layout-width-distribution` and `ui-scroll-start-top` memories. Consult the `slint-reviewer` agent after editing.

- [ ] **Step 1: Add the sub-tab pill.** In the My Music second-row sub-tab strip (where `home | songs | albums | …` pills render, near the `mm-view` property at line ~1496), add a `downloader` pill matching the existing pill component/pattern. Wire it to set `mm-view = "downloader"`.

- [ ] **Step 2: Add Downloader properties + callbacks** on the page root:
```slint
in-out property <string> dl-url;
in-out property <string> dl-provider-badge;   // "" = unsupported
in-out property <string> dl-dest;             // folder path shown
in-out property <string> dl-status;           // idle | resolving | downloading | done | error
in-out property <[DownloaderRow]> dl-rows;    // per-track rows
in-out property <int> dl-done;
in-out property <int> dl-skipped;
in-out property <int> dl-failed;
callback dl-url-changed(string);
callback dl-pick-folder();
callback dl-resolve();
callback dl-download();
callback dl-cancel();
```
Define `struct DownloaderRow { title: string, stage: string, percent: float, file: string }` in the page's struct block (or `ui/tokens.slint` if structs live there).

- [ ] **Step 3: Add the Downloader panel** shown when `mm-view == "downloader"`: URL input bound to `dl-url` (on edited → `dl-url-changed`), a Paste button, provider badge (`dl-provider-badge`), a folder row (`dl-dest` + "Change…" → `dl-pick-folder`), Resolve + Download + Cancel buttons, an overall counts line (`dl-done`/`dl-skipped`/`dl-failed`), and a `for row in dl-rows` list of worker rows (title + stage + progress bar bound to `row.percent`). Reuse existing music-theme button/progress components.

- [ ] **Step 4: Verify in the live dev app.** Per the `hot-reload-dev` skill: keep the dev app running, save the `.slint`, confirm the tab + panel render (no rebuild needed).

- [ ] **Step 5: Commit** `git commit -m "feat(music): Downloader tab UI"`.

---

## Task 19: Wire callbacks in main.rs

**Files:** Modify `crates/tulipix-app/src/main.rs`

- [ ] **Step 1: Wire the four callbacks** using the existing glob-import pattern (`tulipix_sec_music::mdl::*`). Near the other music callback wiring:
```rust
window.on_dl_url_changed({
    let w = window.as_weak();
    move |url| { if let Some(win) = w.upgrade() { win.set_dl_provider_badge(mdl::detect(&url).into()); } }
});
window.on_dl_pick_folder({
    let w = window.as_weak();
    move || {
        if let Some(path) = rfd::FileDialog::new().set_title("Choose download folder").pick_folder() {
            if let Some(win) = w.upgrade() { win.set_dl_dest(path.display().to_string().into()); }
        }
    }
});
window.on_dl_resolve({ let w = window.as_weak(); move || { /* resolve preview via mdl::start_resolve */ } });
window.on_dl_download({ let w = window.as_weak(); move || {
    if let Some(win) = w.upgrade() {
        let url = win.get_dl_url().to_string();
        let dest = { let d = win.get_dl_dest().to_string(); if d.is_empty() { mdl::default_music_dir() } else { d.into() } };
        mdl::start_download(w.clone(), url, dest);
    }
}});
window.on_dl_cancel({ /* set the shared AtomicBool */ });
```
- [ ] **Step 2: Set the initial `dl-dest`** to `mdl::default_music_dir()` at startup (where other music defaults are seeded).
- [ ] **Step 3: Expose silent refresh** to the glue: pass a callback (or reuse an existing invoke) so `mdl::start_download` can trigger `refresh_library_silent` after each completed track. Simplest: after `download_playlist` finishes AND on each Completed, `slint::invoke_from_event_loop` a closure that calls the section's silent rescan.
- [ ] **Step 4: Build (capped) + verify.** Follow `capped-build-command` memory: `just run-dev-lite` (or capped build). App launches; Downloader tab appears.
- [ ] **Step 5: Commit** `git commit -m "feat(app): wire Downloader callbacks"`.

---

## Task 20: End-to-end verification

**Files:** none (manual + one ignored integration test)

- [ ] **Step 1: Ignored integration test** in `crates/tulipix-mdl/src/download.rs`:
```rust
#[tokio::test]
#[ignore] // hits the network; run manually with --ignored
async fn resolve_and_download_one() {
    let client = reqwest::Client::new();
    let pl = crate::resolve_url(&client, "https://open.spotify.com/track/1eJdXVLxLoMWu1TkaeSL18").await.unwrap();
    assert!(!pl.tracks.is_empty());
    let dir = std::env::temp_dir().join("mdl-e2e");
    let opts = crate::types::DownloadOptions { dest_dir: dir.clone(), parallelism: 1 };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sum = crate::download::download_playlist(&client, &pl, &opts, cancel, |_| {}).await;
    assert!(sum.downloaded + sum.skipped >= 1);
}
```
Run manually: `cargo test -p tulipix-mdl --ignored resolve_and_download_one`.

- [ ] **Step 2: Manual app verification** (per `verify` skill): launch dev app → Music → Downloader tab → paste a real Spotify track/album URL → badge shows "Spotify" → Resolve shows tracklist → Download → rows progress → track appears in Songs list (default Music folder now watched). Confirm the file has title/artist/album tags + cover (open in a player or `ffprobe`).
- [ ] **Step 3: Commit** any fixups `git commit -m "test(mdl): e2e ignored integration + verification fixes"`.

---

## Self-Review Notes

- **Spec coverage:** all 9 providers (Tasks 5–13), default=system Music + user-picked folder both auto-watched (Tasks 16–17, 19), real-time via silent refresh per completed track (Tasks 17, 19), Opus (Task 15), folder picker (Tasks 18–19), manifest/resync skip (Task 14), UI tab (Task 18), error/failed list + cancel (Tasks 15, 18, 19), tests ported from mdl fixtures (Tasks 4–13).
- **Port contract:** scraper Rust is not hand-written blind — each provider's inline `.test.ts` fixtures are the passing tests, ported first (TDD). The TS source is the reference; regexes and JSON shapes translate directly.
- **Known adaptation points flagged inline:** lofty 0.21 setter API (verify via context7), the async cover-fetch in `write_tags` (replace the `block_on` sketch), the section's tokio runtime handle, and the silent-refresh callback wiring — all called out where they occur.
- **Build discipline:** every Rust step uses the capped build per memory; `.slint` steps hot-reload with no rebuild.
