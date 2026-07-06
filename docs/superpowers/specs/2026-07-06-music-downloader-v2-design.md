# Music Downloader v2 — Design Spec

Date: 2026-07-06
Branch: beta
Scope: `tulipix-mdl`, `tulipix-sec-music`, `tulipix-app`, `ui/page_music.slint`, `ui/music_common.slint`

## Goal

Rework the Music Downloader tab from full-width worker rows into a frosted
card interface, fix the library-import pipeline so downloaded tracks appear
instantly with correct metadata and thumbnails, let the user pick the main
artist for multi-artist tracks, add configurable parallelism/threads, and add
Download-History and Search-History popups.

## Decisions (locked)

- **Library import**: direct DB ingest from provider metadata (not re-scan).
- **History storage**: `music.db` tables (`dl_history`, `dl_searches`).
- **Main-artist selector**: per-track dropdown + Per-Album / Per-Playlist bulk apply.
- **"Open in system"** = open the containing folder. **"Play in app"** = play the track in-app.
- **Themes**: must read correctly in all three — light / dark / oled.

## Problem analysis (current state)

- `crates/tulipix-sec-music/src/mdl.rs:216` hardcodes `parallelism: 5`, ignoring
  any setting. No threads-per-download flag exists on yt-dlp.
- On completion, `mdl.rs` invokes `music_dl_refresh_library` →
  `refresh_library_silent` (`main.rs:380`) → `add_folder_path` re-scans the
  folder by filename. Embedded tags are only re-read by `ingest_music_tags`
  (`lib.rs:3577`) via `ffprobe_tags` (`lib.rs:3485`), and thumbs settle on the
  next scan pass — so metadata/thumb do not show until a manual refresh + app
  restart. We already hold the exact provider metadata (title/artists/album/
  artwork URL) and cover bytes, so the round-trip is wasteful and lossy.
- **Composer-as-main-artist**: the library groups on `artists[0]`, whose order
  is whatever the provider scraper returned. Letting the user pick the main
  artist (and writing `album_artist` = that choice) fixes it.
- Frost primitives already exist: `ui/frost.slint` `PanelFrost` (cards) and
  `ModalFrost` (popups) resolve their fills from `Theme` tokens, so they render
  correctly in all three themes with no per-theme code.

## Architecture

### Data model

`crates/tulipix-mdl/src/types.rs`
- `DownloadOptions` gains `threads_per_download: usize` (yt-dlp
  `--concurrent-fragments N`). `parallelism` stays but is now driven by the UI.
- No new `Track` field: the chosen main artist is applied by reordering
  `track.artists` in place so the chosen name is `artists[0]`.

`ui/music_common.slint` — `DownloaderRow` gains:
- `thumb: image` (album art preview)
- `album: string`
- `artists: [string]` (dropdown model)
- `main-artist: string` (currently selected primary)

### Backend — `tulipix-mdl`

`download.rs::download_audio`
- Add `--concurrent-fragments {threads}` from `opts.threads_per_download`.
  Caveat: fragment threads only speed fragmented (DASH/HLS) streams; on a
  progressive audio stream it is a harmless no-op. This is the honest mapping
  of "threads per download".

`download.rs::download_playlist`
- Unchanged control flow; `opts.parallelism` and `opts.threads_per_download`
  now flow from the UI.

### Backend — `tulipix-sec-music`

`mdl.rs::start_download`
- Read `parallelism` (1–4, default 2) and `threads_per_download` (1–8, default
  2) from the new window props; drop the hardcoded `5`.
- Apply main-artist selection: reorder each selected track's `artists` so the
  chosen primary is first (per-row choice; bulk album/playlist already applied
  in the row state before Download).

New direct-ingest function (sec-music, mdl module):
- On each track `Stage::Completed`, given `abs_path` + the `Track` + cover
  bytes: insert/locate the `items` row, upsert `track_meta`
  (`title`, `artist` = `"Main, Second, …"`, `album`, `album_artist` = main),
  reuse `scan::get_or_create_artist` / `get_or_create_album`, and write the
  cover to `cover_path` (save cover bytes as a thumb file, or extract embedded
  art). Then `populate_music_views` on the UI thread — live, no restart, no
  filename round-trip. Replaces the `music_dl_refresh_library` call for the
  downloaded item (the watched-folder refresh may remain as a safety net but is
  no longer the primary path).

History tables (music.db migration in the music schema):
- `dl_history(id INTEGER PK, title, artists, album, provider, abs_path,
  downloaded_at)` — inserted per completion.
- `dl_searches(id INTEGER PK, url, kind, title, provider, searched_at)` —
  inserted per successful resolve. `kind` ∈ {track, album, playlist}.
- Paginated fetchers, 20 rows/page, newest first.
- Play-in-app: resolve `abs_path` → `items.id` → existing play path.
- Open-in-system: open the containing folder (reveal).
- Search "Use": push the stored URL back into the `dl-url` field.

### UI — `page_music.slint`

Replace the current downloader `VerticalLayout` (lines ~4008–4075) with:

- **Header**: `AppIcon { source: Icons.download }` + "Music Downloader" title;
  right-aligned **History** and **Search History** buttons (open popups).
- **`PanelFrost` cards** (accent-tinted, theme-aware):
  1. **Source** — URL input + provider badge, Save-to folder, Resolve /
     Download / Cancel actions.
  2. **Options** — Format combo, Name combo, **Parallel** stepper (1–4, default
     2), **Threads** stepper (1–8, default 2), Select all / None.
  3. **Queue** — **overall progress pill** (album/playlist: `done/total`,
     gradient fill, color shifts by aggregate stage) + bulk main-artist bar
     (Per-Album / Per-Playlist dropdown + Apply), then per-track **cards**:
     thumb + title + album + **main-artist dropdown** + a **color-changing
     fill** behind the content whose width = `percent` and whose color tracks
     the stage (queued grey → downloading violet/blue → done green → failed red).
- **Popups** (`ModalFrost` overlays):
  - **Download History** — 20/page; each entry: thumb, title, artists, "when";
    actions **Open in system** (open folder) + **Play in app**; pagination.
  - **Search History** — each entry: URL, `kind` tag, title, "when"; action
    **Use** (refills the URL field); pagination.

### New slint props / callbacks

- `music-dl-parallel: int = 2`, `music-dl-threads: int = 2` (+ setters).
- `DownloaderRow`: `thumb`, `album`, `artists: [string]`, `main-artist`.
- `dl-set-main-artist(int row, string artist)`,
  `dl-bulk-main-artist(string scope, string artist)` (scope ∈ album|playlist).
- Overall pill: `dl-total` + computed fraction from `dl-done`.
- Download history: `dl-history-open: bool`, `dl-history-rows: [...]`,
  `dl-history-page`, `dl-history-pages`, callbacks open/close/page/play/reveal.
- Search history: `dl-search-open: bool`, `dl-search-rows: [...]`, page props,
  callbacks open/close/page/use.

## Error handling

- Direct ingest failure (DB error, cover decode) logs a warning and falls back
  to the existing watched-folder refresh so the track is never lost.
- History/search inserts are best-effort (log on failure); never block a
  download.
- Threads/parallel steppers clamp to their ranges.

## Testing

Rust unit tests:
- `--concurrent-fragments N` present in the yt-dlp argv for `threads > 1`.
- Main-artist reorder puts the chosen name at `artists[0]`; name stem reflects
  the new order.
- Direct-ingest builds `track_meta` with `album_artist` = main and
  `artist` = joined-with-main-first.
- History insert + 20/page pagination (newest first); search insert + `Use`
  returns the stored URL.

Manual (hot-reload dev app, per `hot-reload-dev` skill):
- Small playlist → tracks appear live with correct thumb + main artist, no
  restart.
- Parallel/threads steppers respected.
- Per-track dropdown + Per-Album / Per-Playlist bulk apply.
- Download-History popup: Open in system opens folder, Play in app plays.
- Search-History popup: Use refills the URL field.
- Verify frost + pills + text legibility in light, dark, oled.

## Build order (phases)

1. Config: parallel + threads (backend + Options card wiring).
2. Direct DB ingest + main-artist reorder (core bug + composer fix).
3. History tables + queries + play/reveal/use.
4. Card UI: frost cards, overall pill, per-track cards, dropdowns, steppers.
5. History + Search popups.
6. Three-theme polish.

## Out of scope

- AI/metadata enrichment beyond provider data.
- New providers.
- Changing the resolve/scrape logic itself.
