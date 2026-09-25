<p align="center">
  <img src="resources/appicons/Tulipix-Logo.png" alt="Tulipix" width="160"/>
</p>

<h1 align="center">Tulipix</h1>

<p align="center">
A private, native, all-in-one desktop media and life manager — <b>Photos · Videos · Stream · Music · Podcasts · Audiobooks · Radio · YouTube · Books · Feeds · Journal · Kitchen · Papers · Finances · Cloud · Transfer · Tools</b> — a Flutter app over a Rust core.<br/>
No webview, no accounts, no telemetry: everything lives in local SQLite databases on your machine.
</p>

<p align="center">
<a href="https://github.com/atishsharma/tulipix/releases"><b>Download</b></a> ·
<a href="#features">Features</a> ·
<a href="#build-from-source">Build from source</a> ·
<a href="#architecture">Architecture</a> ·
<a href="#privacy-and-security">Privacy</a>
</p>

---

## Contents

- [Install](#install)
- [Build from source](#build-from-source)
- [Features](#features)
- [Privacy and security](#privacy-and-security)
- [Architecture](#architecture)
- [Where your data lives](#where-your-data-lives)
- [Development notes](#development-notes)
- [Releases](#releases)

---

## Install

### Grab a release (recommended)

Download the latest build from [**Releases**](https://github.com/atishsharma/tulipix/releases):

| OS | Artifact | Notes |
|---|---|---|
| Linux | `tulipix-<ver>-linux-x86_64.tar.gz` | portable — needs `libmpv` from your package manager (Arch: `pacman -S mpv`, Debian/Ubuntu: `apt install libmpv2`) |
| Windows | `tulipix-<ver>-windows-x86_64-portable.zip` | portable zip — everything bundled, unzip and run |
| macOS | `tulipix-<ver>-macos-aarch64.dmg` | Apple Silicon, signed ad hoc — right-click › Open on first launch |

Bundled with every build: **ffmpeg**, **yt-dlp**, **whisper.cpp**, **rclone** and **exiftool**, each pinned by SHA-256 in `resources/binaries.toml`. yt-dlp checks for its own update once a week in the background.

On Linux, run `just desktop` (or copy the bundled `.desktop` file) once so the taskbar picks up the app icon — Wayland takes the icon from the installed desktop entry and nowhere else.

---

## Build from source

### Prerequisites

| Tool | Why |
|---|---|
| Rust (stable, `rustup default stable`) | the core, the bridge and every domain crate |
| Flutter **3.47.5** | the app ([install](https://docs.flutter.dev/get-started/install)) |
| [`just`](https://github.com/casey/just) (`cargo install just`) | every workflow goes through the root `justfile` |
| `flutter_rust_bridge_codegen` 2.13.0 | `just install-codegen` installs the version the bridge crate expects |
| libmpv | playback (Linux: from your package manager; bundled on Windows and macOS) |
| OpenSSL headers (Linux: `libssl-dev`) | SQLCipher database encryption in profile and release builds |
| Perl (Windows only) | builds the vendored OpenSSL that SQLCipher links on Windows |

Linux build packages (Debian/Ubuntu names): `clang cmake ninja-build pkg-config libgtk-3-dev liblzma-dev libmpv-dev libdbus-1-dev libwayland-dev libxkbcommon-dev libssl-dev`.

### First run

```bash
just install-codegen   # flutter_rust_bridge_codegen, matching the bridge crate
just scaffold          # fill in the desktop runners Flutter owns (safe to re-run)
just fetch             # download the bundled tools into resources/bin/, SHA-256 checked
just dev               # generate bindings, seed a sandbox, run a debug build
```

### Everyday recipes

| Recipe | What it does |
|---|---|
| `just` | list every recipe |
| `just dev` | debug run against a sandboxed **copy** of your data |
| `just fast` | profile build against the sandbox — DevTools still works; use it for any performance number |
| `just fast release` | release-mode run: what the app actually feels like |
| `just gen` | regenerate the Dart ⇄ Rust bindings (after any change under `crates/tulipix-bridge/src/api/`) |
| `just check` | `gen`, then `cargo check` on the bridge and `flutter analyze` |
| `just test` | `gen`, then the bridge tests and `flutter test` |
| `just test-crate <name>` | tests for one workspace crate, e.g. `just test-crate tulipix-photos` |
| `just fmt` | `cargo fmt` + `dart format` |
| `just reseed` | throw the sandbox away and copy your data in fresh |
| `just seed` | copy your real databases into `.flutter-sandbox/` (only if it is empty) |
| `just desktop` | install the `.desktop` entry and icons (Linux) |
| `just cli <args>` | run the headless `tulipix-cli` |

`just dev` and `just fast` **never open your real data**. Section schemas migrate forward on open, so a dev build pointed at the real data directory could upgrade it under the installed app's feet. `just seed` copies the databases into `.flutter-sandbox/` and the app runs against that copy; nothing is ever copied back.

`CARGO_BUILD_JOBS` defaults to 2 in the justfile, and `sccache` is used when it is installed. Raise the job count on a machine with more than 8 GB of RAM.

---

## Features

Fourteen sections in one window, each with its own database, and a shell that ties them together. Every section can be hidden in **Settings › Sections**, or picked as a set from a preset.

### 🏠 Home

- **Four layouts** over one snapshot of every section, chosen in Settings › You & Home:
  - **Classic**: counts, Continue, then shelves.
  - **Welcome**: a greeting, the launchers, then everything offered as cards.
  - **Cinema**: the newest in-progress item, full bleed, with Resume on it.
  - **Stream**: one timeline of everything that happened, newest first.
- Continue watching / listening / reading across sections, and a card per section you can dismiss or reorder.

### 🖼 Photos

- Folder scanner with EXIF extraction, thumbnail pipeline, timeline, map, people, places and album views.
- **Albums**, manual and **smart**. A smart album is a saved rule: tag, person, camera, a range of years, starred only. It fills and updates itself.
- Built-in editor: adjust, curves, crop/rotate/flip, filters, tint, text, red-eye, enhance, sharpen and resize. AI ops: heal, sky, upscale and colorize.
- Slideshow with hero transitions, ambient screensaver, duplicates finder, starred / archived / trash tabs.
- **On-device AI** (ONNX Runtime, models downloaded on demand in Settings › AI, SHA-256 pinned):
  - CLIP search.
  - Face detection and grouping (SCRFD).
  - Object detection (YOLO).
  - Inpainting (LaMa) and SAM segmentation.
  - Real-ESRGAN upscale and DeOldify colorize.

  Nothing leaves your machine.

### 🎬 Videos

- Library scanner with TMDB / TVDB metadata, posters, backdrops and subtitle support. Anime falls back to AniList / AniDB when TMDB has nothing.
- Embedded libmpv playback (media_kit), PiP floating player, chapters, subtitle word search, per-item resume.
- **The Videos deck**: hero rails, Continue Watching, movie covers, starred / archived / trash tabs, and a file-naming guide for the scanner.
- Optional **Trakt** sync of what you watched.

### 📡 Videos — Stream

A remote catalogue browsed from inside the section, with the same players and queues as everything local.

- **Search with suggestions**, paged results, and a landing screen with Continue Watching, trending titles and trending vertical dramas. All of it is cached and refreshed twice a day.
- **Stream Plus**, an anime lane with its own provider stack.
- **Detail pane** per title: seasons, episodes, audio cuts, resolutions and subtitle tracks, each as its own column.
- **Sticky quality**, **resume and autoplay**: the next episode starts on its own and the one after is resolved in the background.
- **Downloads** that survive a restart: retry, cancel, reveal, delete, a live bar per job, and the chosen subtitle fetched alongside.
- **Bookmarks** with new-episode badges, a **History** page, **Cast** to DLNA, trailers, copy link, and a host health check with fastest-first sorting.

### 🎵 Music

Five modes in one section: **My Music, Podcasts, Audiobooks, Radio and YouTube**.

- **Library**: a scanner with full tag support (symphonia). Albums, artists, genres, folders and songs, with sortable views, grid density and pagination.
- **Playlists**:
  - Manual playlists, with reordering and custom covers.
  - Smart playlists: Loved and Recently Added.
  - M3U and PLS import and export.
- **Song analysis**: BPM, musical key and dynamics, for one song from its menu or for the whole library.
- **Sonic Similar** recommendations from the current track, plus instant mix and auto-DJ.
- **Lyrics**: synced LRC from LRCLIB with a library-wide sync manager, a karaoke view and an inline ticker.
- **Metadata**: tags fetched from MusicBrainz, with Discogs and Spotify as optional fallbacks.
- **Playback**: a 10-band equalizer with saved profiles, sleep timer, volume to 130% and an output-device picker that follows a headset being plugged in.
- **Scrobbling**: optional, to Last.fm and ListenBrainz. It is off by default.
- **Music downloader** (`tulipix-mdl`): native scrapers that write straight into the library.

**Podcasts**:
- RSS subscriptions, a Trends directory and a hero show page.
- A sequential offline download queue.
- Show notes and transcripts, played tracking, and pitch-corrected speed from 0.75× to 2×.

**Audiobooks**:
- Each folder is one book, with chapters and its cover.
- Resume per book.
- Speed up to 3×, skip-silence and bookmarks.

**Internet Radio**:
- 15 curated India-first categories from radio-browser.info, cached offline.
- Universal search, favourites and recents.
- Custom stream URLs, ICY now-playing, station art and recording through Tools.

**YouTube**:
- Home, Subscriptions, Trends, Downloads and Search, backed by Piped and yt-dlp.
- Channel pages and a daily recommendation cache.
- Audio through the same transport; Watch video hands off to the player.

**Player stack**:
- A draggable, resizable mini player (vinyl, queue and lyrics faces) that docks to a side bubble.
- A bottom glass player bar.
- **Zen** full screen: ambient art wash, 5 visualizers, inline lyrics and full keyboard shortcuts.

### 📖 Books

- **Ten formats**: EPUB (including fixed-layout manga and magazines), PDF, DjVu, CBZ, CBR, CB7, CBT, MOBI, AZW3 and FB2.
- **Two reader engines**:
  - A reflowing paginator for prose: two-page spread, single page or a continuous column.
  - A fixed-page reader for anything rasterised.
- **Comics and manga**:
  - Double-page spreads shown whole.
  - Webtoon mode for long strips.
  - Right-to-left order, taken from ComicInfo.xml.
- **Reading tools**:
  - Sharpening zoom and margin trim.
  - Page thumbnails.
  - Page-anchored notes that export as Markdown.
- **Read-aloud** with Kokoro neural TTS or espeak, with sentence highlighting.
- **Library**:
  - Series with natural ordering.
  - Smart collections and duplicate detection.
  - Calibre import.
  - Full-text search inside books.
  - Reading stats with streaks and a heatmap.
  - Baked 3D covers.
- **Genesis**, a sub-page of Books: search LibGen mirrors, which are ranked by health, and download straight into the library.

### 📰 Feeds

- Sites, blogs and newsletters in one reader.
- **Today** is a morning brief: one story told by several sources, then the rest, then newsletters.
- **Unread** (sources · list · reader), **Read later** and **Highlights**.
- Read aloud in the Books reader's voice.

### 📔 Journal

- A daily log that **gathers the day from the other sections**: photos taken, music played, money spent, videos watched and pages read.
- Moods, places, and **voice notes transcribed locally with whisper.cpp**.
- **Calendar**, **On this day**, a question for tonight, streaks and **Insights**.

### 🍳 Kitchen

- **Recipes** from a link (schema.org JSON-LD), from a photo, or by hand.
- **Scaling** and a **pantry**.
- **Cook mode**: one step at a time in large type, with step timers, keep-awake, arrow-key navigation and read-aloud.
- **Plan** the week's lunches and dinners.
- **Shopping list** by aisle, as a page you open on your phone.
- The shop is logged as an expense in Finances.

### 🗂 Papers

- **Receipts, bills, IDs and manuals**, read on this computer with pdftotext and tesseract OCR.
- Filed with suggestions and searchable by their text.
- Matched to Finances transactions.
- **Reminders** 30 and 7 days before anything expires.
- **Vault** sealed with ChaCha20-Poly1305 behind the app PIN. It re-locks after five minutes.
- **Scans sent from a phone** over Transfer.

### 💰 Finances

- Accounts, loans and **lending** (money lent is tracked as a claim, not an account balance).
- Budgets, bills, and statement import.
- **Summary cards and the table use the same filter**, so a card's total always matches the rows below it.
- Charts drawn natively: donut, bars and trends.

### ☁️ Cloud

- An rclone-driven remote manager:
  - Remote create, edit and delete.
  - A tree browser.
  - Sync jobs, selective sync and share links.
  - Versions and snapshots.
  - Streaming preview.
- An encrypted vault and a ransomware guard.
- **Mounts** that the app tracks. It checks them against the system on refresh and stops them on quit.
- Credentials stay local. There are no accounts.

### 📲 Transfer

- A **Wi-Fi transfer server**: pair a phone with a QR code, then send files both ways from the phone's browser. No app is needed on the phone.
- Trust-on-first-use certificates by default, SSH-style. You can load a real certificate instead.
- A **Recent transfers** ledger, sortable across the whole history.

### 🛠 Tools

- A background job queue with live progress:
  - Compress and convert video, audio and photos.
  - Trim, resize, merge, split and watermark.
  - Hash files, compare folders and batch rename.
- **Downloads** with yt-dlp: videos, playlists and live streams.
- **Transcription** with whisper.cpp, and stream recording.
- An **Update** button on every bundled tool.

### 📊 Status

- Every section's health on one page:
  - A health band with the week's growth.
  - A card per section that opens a details drawer.
  - Scans, what happened today and a timeline.
- The same numbers are served on a loopback HTML dashboard.

### 🤖 AI

- **Chat** with a local or configured LLM.
- **Photo captions** and on-device vision.
- **First-run model downloads**, pinned by hash.
- An **MCP server**: `tulipix-cli mcp` speaks JSON-RPC on stdin/stdout, so an agent can query your library. It is off unless you turn it on, and no port is ever opened.

### ⚙️ Settings and the shell

- **Four design languages** that restyle the whole app:
  - Standard: frosted glass.
  - Neumorphism.
  - Glassmorphism: lit by the cover that is playing.
  - Material 3 Expressive.
- **Themes and profile**: light, dark and extra-dark, reduce motion, font scale and accent colour. Profile photo and cover with a pan-and-zoom cropper, and a choice of sidebar logo.
- **Nine Settings tabs** with search across every row, and a Restore defaults button.
- **Libraries**:
  - Watched folders, with **live filesystem watching**: new, moved and deleted files show up without a rescan.
  - Scheduled rescans and exclusions.
- **Data**:
  - Backups.
  - A log viewer over rolling JSONL logs, kept for 14 days.
  - Crash reports, kept on this computer. **Report** opens a pre-filled issue for you to review.
- **Service integrations**, each off until you add a key:
  - AniList, AniDB, Discogs, Spotify, Trakt, Last.fm and ListenBrainz.
  - These only answer when the primary source (MusicBrainz, TMDB, OpenSubtitles, yt-dlp) came back empty.
- **Everywhere in the app**:
  - A command palette.
  - A ten-card first-run onboarding.
  - Tray panel, menus and notifications per OS.
  - A desktop widget pin.
  - A scan-progress HUD.
- **Lock screen**: PIN-gated and ambient, with a slideshow and now-playing.

---

## Privacy and security

- **Local-first.** Every section writes to its own SQLite database in your data directory. There are no accounts and no telemetry, and no network call happens unless a feature you use needs one: a metadata lookup, a stream, a feed refresh.
- **Database encryption** (release builds): SQLCipher over the library index, switched on in Settings › Security. The encryption covers what Tulipix knows about your files. The files themselves are never modified.
- **App lock**:
  - A PIN.
  - A **fingerprint** through fprintd, or a **security key**.
  - Profiles.
- **Papers vault**: ChaCha20-Poly1305, unlocked with the app PIN and re-locked after five minutes.
- **Capabilities**: `resources/capabilities.toml` says what each tier may do and sets the daily quotas for the app's own API keys. Anything it does not list is denied. You can override it:
  - with `capabilities.local.toml` beside your data, or
  - with the file named by `TULIPIX_CAPS_OVERRIDE`.
- **Plugins** run sandboxed in WASM (wasmtime) or Lua (mlua).
- **Bundled binaries** are all verified against SHA-256 hashes in `resources/binaries.toml`, and **AI models** against `resources/ai-models.toml`.

---

## Architecture

```
┌────────────────────────── app_flutter (Dart) ──────────────────────────┐
│ shell · design tokens · 4 design languages · one page+controller per   │
│ section · media_kit (libmpv) playback                                  │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │ flutter_rust_bridge 2.13 (generated)
┌──────────────────────────────────▼─────────────────────────────────────┐
│ crates/tulipix-bridge  — api/<section>.rs: State · Cmd · dispatch ·    │
│ event stream. A separate Cargo workspace, built by cargokit.           │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │ plain Rust calls
┌──────────────────────────────────▼─────────────────────────────────────┐
│ domain crates (root workspace): core · common · photos · videos ·      │
│ music · books · cloud · finances · genesis · transfer · status · ai ·  │
│ whisper · tools · mdl · platform · plugins · cli                       │
└────────────────────────────────────────────────────────────────────────┘
```

Each section follows the same pattern:
- Rust keeps the session state.
- Dart sends a command, such as `PhotosCmd.albumNewSmart(...)`.
- Rust runs it and emits an event on the section's stream.
- The controller asks for a fresh snapshot and redraws.

### Workspace layout

```
app_flutter/            the Flutter app
  lib/shell/            window, sidebar, header, tray panel, command palette
  lib/design/           tokens, themes, the four design languages
  lib/sections/<name>/  page, controller and views per section
  lib/src/rust/         generated bindings (do not edit — `just gen`)
crates/
  tulipix-bridge/       flutter_rust_bridge API over the crates below (own workspace + Cargo.lock)
  tulipix-core/         db pool, settings, keyring, caps, watcher, logging, crash, db encryption
  tulipix-common/       playback core, shared singletons, data dirs
  tulipix-photos/       photos.db, scanner, EXIF, editor, smart albums, AI hooks
  tulipix-videos/       videos.db, scanner, TMDB/TVDB/AniList, subs, Stream catalogue
  tulipix-music/        music / podcasts / radio / youtube dbs, scanners, tags, analysis
  tulipix-books/        books.db, ten-format parsers, ComicVine, folder watch
  tulipix-genesis/      LibGen search, mirror ranking, downloads
  tulipix-cloud/        cloud.db, rclone driver
  tulipix-finances/     finances.db: accounts, budgets, bills, statement import
  tulipix-transfer/     Wi-Fi transfer server, pairing, phone web app
  tulipix-status/       the Status snapshot every dashboard renders
  tulipix-ai/           LLM chat, captions, model catalogue
  tulipix-whisper/      whisper.cpp subprocess + FIFO queue
  tulipix-tools/        background job queue
  tulipix-mdl/          music downloader
  tulipix-platform/     per-OS menus, tray, notifications
  tulipix-plugins/      WASM + Lua extension host (excluded from the root workspace)
  tulipix-cli/          headless CLI + MCP server
tools/fetch-resources/  binary fetcher with SHA-256 verification
resources/              binaries.toml, ai-models.toml, capabilities.toml, icons, app logos
.github/workflows/      ci.yml (crate checks), flutter-release.yml (release builds)
```

Feeds, Journal, Kitchen and Papers have their stores inside the bridge crate (`api/<section>.rs`) rather than a crate of their own.

---

## Where your data lives

| OS | Directory |
|---|---|
| Linux | `$XDG_CONFIG_HOME/Tulipix` (usually `~/.config/Tulipix`) |
| macOS | `~/Library/Application Support/Tulipix` |
| Windows | `%APPDATA%\Tulipix` |

One `.db` file per section (`photos.db`, `music.db`, `books.db`, `papers.db`, …), plus these folders:
- `logs/`: rolling JSONL.
- `crashes/`: local crash dumps.
- Thumbnail caches.
- Downloaded AI models.

Back it up by copying the folder, or use Settings › Data › Backups.

---

## Development notes

- **Changing the bridge API.** Adding a function, a `Cmd` variant or a field under `crates/tulipix-bridge/src/api/` needs `just gen` before Dart can see it. `just check`, `just dev` and `just test` all run `gen` first.
- **Two lockfiles.** The root workspace (library crates plus `tools/fetch-resources`) and the bridge each have their own `Cargo.lock`. A dependency bump may need both.
- **Feature flags** for app builds are in `crates/tulipix-bridge/cargokit.yaml`. Profile and release builds enable `books-tts`, `ai-onnx` and `db-encrypt`.
- **Tests.** For the Rust side, `just test-crate <crate>` covers one crate and `just test` covers the bridge plus the Flutter widget tests.
- **CI.** `.github/workflows/ci.yml` runs `cargo check` and tests on the domain crates for pushes and pull requests.

---

## Releases

Push a `flutter-v<version>` tag to build and publish Linux, Windows and macOS artifacts with `.github/workflows/flutter-release.yml`:

```bash
git tag flutter-v1.0.1 && git push origin flutter-v1.0.1
```

The version comes from `[workspace.package]` in `Cargo.toml` and `version:` in `app_flutter/pubspec.yaml`. Keep them in step.

---

<p align="center">
Tulipix © 2026 — Developed by <b>Atish Ak Sharma</b><br/>
<sub>Built with Flutter, Rust, flutter_rust_bridge, Tokio, SQLite / SQLCipher, sqlx, libmpv (media_kit), FFmpeg, yt-dlp, rclone, whisper.cpp, ONNX Runtime, symphonia, image-rs, wasmtime, mlua, ExifTool, Tesseract, Serde and reqwest.</sub>
</p>
