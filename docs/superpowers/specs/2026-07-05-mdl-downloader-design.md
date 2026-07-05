# Music Downloader ("Downloader" tab) — Design

Date: 2026-07-05
Branch: beta
Status: Approved (design), pending implementation plan

## Goal

Add a **Downloader** tab to the My Music second row (the `home | songs | albums | artists | genres | playlists | folders` sub-tab strip) of the Music section. The user pastes a music URL from any supported streaming service; the app resolves the playlist/album/track metadata, downloads the audio, tags it, and the tracks appear in the music library in real time.

This is a **native Rust** port of the [`mdl`](https://github.com/labithiotis/mdl) CLI (studied in `mdl/packages/cli/`), not a bundled JS sidecar. Decision rationale: the app already owns the entire download half natively (bundled auto-updating yt-dlp + ffmpeg, `tulipix-music/yt_search.rs` `ytsearchN:` argv, watched-folder music library with a live-accumulated track model). The only capability `mdl` adds that the app lacks is the **provider metadata scrapers** (URL → track list, no API keys). Native keeps the binary small (no Bun runtime) and fits existing patterns.

## Decisions (locked)

- **Providers:** all 9 — Spotify, Apple Music, Amazon Music, YouTube Music, SoundCloud, Bandcamp, Qobuz, Deezer, Tidal.
- **Destination:** the Downloader page has a folder picker. Default = the system Music directory (e.g. `~/Music` / `$XDG_MUSIC_DIR`). Both the default system Music dir **and** any user-picked download folder are auto-added to the watched-folders set so downloads land in the library.
- **Real-time:** each completed track is pushed directly into the library track model as it finishes (instant, deterministic) — not reliant on a filesystem watch race.
- **Audio format:** Opus (matches the existing `-x --audio-format opus` path).

## Architecture

New library crate **`tulipix-mdl`** (no Slint, no UI):

```
crates/tulipix-mdl/
  src/
    lib.rs          // public API: detect_provider, resolve, DownloadJob, run
    types.rs        // Provider, Track, Playlist, Progress, Stage
    resolve.rs      // URL dispatch + short-link resolution
    download.rs     // per-track: yt-dlp search+download → tag → progress
    manifest.rs     // .mdl.json skip-already-downloaded / resync
    provider/
      mod.rs        // Provider trait + registry
      spotify.rs
      apple_music.rs
      amazon_music.rs
      youtube_music.rs
      soundcloud.rs
      bandcamp.rs
      qobuz.rs
      deezer.rs
      tidal.rs
```

**Consumers / reuse:**
- `tulipix-sec-music` — Slint wiring, folder picker, live push into the library model.
- `tulipix-core` — bundled tool path resolution (yt-dlp, ffmpeg), `paths::config_dir`.
- `tulipix-common::load_watched_folders` (extend with a save/add helper so a new dest folder persists into `watched_folders.json`).
- `tulipix-music/yt_search.rs` — `ytsearchN:` argv pattern for the download search.

**New dependencies:** `reqwest` (present), `serde_json` (present), `regex` (present). Add `lofty` (audio tag + cover-art write, cross-platform, avoids an extra ffmpeg tagging pass). Add a system-dirs helper (`dirs` or `directories`) for the default Music folder unless `tulipix_core::paths` already exposes it.

### Data model (ported from mdl `schemas.ts`)

```rust
struct Track {
    id: String,
    title: String,
    artists: Vec<String>,
    album: Option<String>,
    artwork_url: Option<String>,
    duration_ms: Option<u64>,
    source_url: Option<String>,
}
struct Playlist {
    id: String,
    title: String,
    owner: Option<String>,
    artwork_url: Option<String>,
    provider: Provider,
    source_url: String,
    tracks: Vec<Track>,
}
```

## Provider layer

```rust
trait Provider {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &str;
    fn short_link_hosts(&self) -> &[&str] { &[] }
    fn matches(&self, url: &Url) -> bool;
    async fn fetch(&self, url: &str) -> Result<Playlist>;
}
```

Registry mirrors `Providers.ts`: `detect_provider(url)`, `validate_provider_url` (including short-link redirect resolution for `spotify.link`, `apple.co`, etc.), and per-provider URL normalization (Apple keeps `?i=`, Amazon keeps `?trackAsin=`, YT Music keeps `v`/`list`).

Each scraper is a 1:1 port of its mdl counterpart — fetch the public/embed URL, extract the embedded JSON blob (`__NEXT_DATA__`, serialized `<script>` payloads) or `og:`/`twitter:` meta tags, normalize into `Track`s. Spotify uses the `/embed/{kind}/{id}` endpoint then enriches playlist tracks via per-track og-meta (album/artwork). Build order: framework + Spotify + Apple Music + YouTube Music first (usable), then the remaining six.

## Download pipeline (ported from mdl `sync.ts`)

Bounded to 5 concurrent workers. Per track:

1. `ytsearch1:"<artists joined> - <title>"` via yt-dlp; on an unavailable / age-gated / members-only candidate, advance to the next search hit (mdl's skippable-error logic).
2. Download `bestaudio` → extract Opus into the dest folder. Filename `NN - Artist - Title.opus` (sanitized, index-prefixed).
3. Write tags (title / artist / album) and embed cover art (fetched from `artwork_url`) via `lofty`.
4. Push the completed track into the library model + emit a progress event.

Retry each track up to 2× with a short delay (mdl constants). Skip a track if the manifest + file already exist (resync). A per-dest `.mdl.json` manifest records downloaded tracks for skip/resync; no resync UI surfaced in v1.

Progress stages ported from mdl `SyncStage`: `initializing → searching-youtube → downloading-audio → writing-metadata → writing-manifest → completed | skipped | failed`.

## Library integration (real-time)

- On download start, ensure both the system Music dir and any user-picked dest are present in `watched_folders.json` (add + persist). This makes them permanent library roots.
- As each track completes, push it directly into the accumulated tracks model in `tulipix-sec-music` so it appears in Home/Songs immediately. A `notify` watcher on the dest is an optional backstop, not the primary mechanism.

## UI — Downloader tab

- New pill `downloader` in the My Music second-row sub-tab strip (`mm-view`).
- Panel contents:
  - URL input + **Paste** button; live provider-detect badge ("Spotify ✓" / "Unsupported URL").
  - Folder row: current dest (default = system Music dir) + **Change…** → native folder dialog.
  - **Resolve** → playlist header (title/owner/artwork) + tracklist preview.
  - **Download** → per-track worker rows (stage, %, filename), overall progress with downloaded / skipped / failed counts, **Cancel**.
- Styling matches the app's music theme; worker rows mirror mdl's `SyncScreen` layout. `.slint` edits hot-reload; only the Rust glue needs a rebuild.

## Error handling

- Per-track retry 2× (mdl delays); skippable YouTube-candidate errors advance to the next candidate; failed tracks collect into a visible failed list with reasons.
- Provider/resolve errors surface inline at the URL input.
- Cancel via an atomic flag / cancellation token checked between stages and workers (mdl's `AbortSignal` equivalent).

## Testing

- **Provider scrapers:** port mdl's fixture-HTML + expected-track tests (`*.test.ts`) to Rust `#[test]` — offline, deterministic, one module per provider.
- **URL detection / normalization:** unit tests ported from `Providers.test.ts`.
- **Download path:** `#[ignore]` integration test (hits the network) exercising resolve → download → tag on one known-good URL.

## Out of scope (v1)

- Resync UI (manifest supports it; no button yet).
- Non-Opus formats (M4A/MP3) — pipeline is format-agnostic; only Opus wired.
- Provider API keys / authenticated content — public URLs only, as upstream.
- Account/sync features (per project's local-only decree).
