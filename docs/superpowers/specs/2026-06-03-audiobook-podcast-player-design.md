# Audiobook section: podcast-style player + section-aware add + music page split

**Date:** 2026-06-03
**Status:** Approved for planning
**Area:** music page architecture, audiobooks, library add flow, settings

## Problem

Two intertwined goals:

1. **Feature** — the audiobook section is a flat `TrackRow` list. Give it the
   podcast UX (card grid → hero detail → chapter list → speed/bookmark
   transport), wire the "Audiobooks" folder-section so it actually populates the
   view, and show a placeholder thumb when a book lacks cover art.

2. **Refactor** — `ui/page_music.slint` is a ~329K monolith. `MusicPage` is one
   ~2,670-line component (lines 925–3595) holding all 5 sections as
   `if root.view == "..."` blocks. The 5 sections (My Music, Podcasts,
   Audiobooks, Radio, YouTube) are conceptually independent sub-apps sharing
   only the 2-line header and the bottom player. Split them into separate files.

## Decisions (from brainstorming)

Feature:
- **Full card + detail UX** for audiobooks (closest to podcasts).
- Add popup offers **music sub-sections only** (My Music / Podcasts /
  Audiobooks / Radio / YouTube). Choosing Audiobooks sets `is_audiobook` on the
  folder's tracks.
- **Group by folder**: one card per source folder; chapters = the audiobook
  tracks in that folder sorted by `track_no` then filename. A single `.m4b` is a
  one-track card; its internal mpv chapters drive chapter nav at play time.
- Add flow order: **pick directory first (rfd), then assign section (Slint
  dialog)**.

Refactor:
- **Full split** into per-section files + a thin shell.
- Shared state via a Slint **`global MusicState`**.
- The global is **fed by a `.slint` binding bridge inside the `MusicPage`
  shell** — Rust and main.slint are NOT migrated. (Rejected alt: rewrite all
  ~907 Rust music accessor calls to `global::<MusicState>()` — pure but one
  make-or-break rebuild, high risk on a 4c/7GB box.)

## Architecture — the split

### File layout (target)

| File | Holds |
| --- | --- |
| `music_common.slint` | All shared `struct`s, the `global MusicState`, and shared components (`PlayerBtn`, `GlyphBtn`, `Scrubber`, `SortChip`, `BrowseChip`, `TrackRow`, `PageNav`, `PageBtn`, `Marquee`, `MusicMenuRow`, `MusicTab`, `AlbumCard`, `ArtistCard`, `SongCard`, `TileGrid`, `EqBand`, `VizView`, `PodcastGrid`, `PodcastCardView`, `PodcastTrendCardView`, `PodcastEpisodeItem`, `DetailActionBtn`, `GradOutline`, `TagField`, etc.) |
| `section_mymusic.slint` | `MyMusicSection` component (library grid, browse tabs, detail, lyrics, tag editor, etc.) |
| `section_podcasts.slint` | `PodcastsSection` (home/trends/subscribed/downloads + detail hero) |
| `section_audiobooks.slint` | `AudiobooksSection` — **rebuilt** per the feature below |
| `section_radio.slint` | `RadioSection` (placeholder today) |
| `section_youtube.slint` | `YouTubeSection` (placeholder today) |
| `music_players.slint` | `MusicFullscreen`, `MusicMini`, `PodcastMini` (already standalone components, just relocated) |
| `page_music.slint` | thin `MusicPage` shell: header (2 lines) + bottom player + the `MusicState` bridge + `if MusicState.view == ... : XSection { }` switch |

Each section component reads/writes `MusicState.*` directly, so it needs **no
threaded property interface** — that is what makes them independent files.

### The `MusicState` bridge (zero Rust / main.slint churn)

`MusicPage` keeps its **existing external interface** — the ~418 props/callbacks
that main.slint already binds onto it stay byte-for-byte the same, so main.slint
and Rust are untouched. Inside the `MusicPage` shell, each is republished to the
global one time:

```slint
// inside MusicPage
MusicState.tiles: root.tiles;                 // `in`  -> one-way
MusicState.view <=> root.view;                // `in-out` -> two-way (UI writes back, Rust get_* still works)
MusicState.song-play(i) => { root.song-play(i); }   // callback -> forward
```

Rules:
- `in` props (Rust→UI only: tiles, statuses): one-way `MusicState.x: root.x;`
- `in-out` props (UI also writes; Rust reads via `get_*`): `MusicState.x <=> root.x;`
- callbacks (UI→Rust): `MusicState.x(args) => { root.x(args); }`

This block is mechanical and lives entirely in `page_music.slint`. The whole
split is `.slint`-only and **hot-reloadable** — no Rust rebuild to bring it up.

### Migration order (safe, mostly hot-reload)

1. Create `music_common.slint`: move the structs + shared components there;
   declare the (initially empty) `global MusicState`. Add imports back into
   `page_music.slint`. Verify hot-reload still renders.
2. Populate `MusicState` with the props/callbacks and add the bridge block in
   `MusicPage`. Switch shared components + section blocks to read `MusicState.*`
   incrementally (one section at a time), verifying render after each.
3. Extract each section's block into its own `*Section` component file; replace
   the inline block in the shell with `if MusicState.view == "x": XSection {}`.
   Do this one section at a time, hot-reloading between each.
4. Relocate the fullscreen/mini players to `music_players.slint`.
5. Only then layer the audiobook feature (Rust + `section_audiobooks.slint`).

No step except the final audiobook feature requires a Rust rebuild.

## Architecture — the audiobook feature

`track_meta.folder` (already indexed) is the grouping key.
`track_meta.is_audiobook` already exists. No schema change.

### Part A — Folder→section drives `is_audiobook`

New helper in `crates/tulipix-music/src/audiobooks.rs`:

```rust
/// Set/clear is_audiobook for every track under one folder. Returns rows hit.
pub async fn set_folder_flag(pool: &SqlitePool, folder: &str, on: bool) -> Result<u64>;
```

`UPDATE track_meta SET is_audiobook = ? WHERE folder = ?`.

Reconcile rule (authoritative source = `music_folder_sections.json`):
- Folder mapped → `audiobooks`: flag = 1 for its tracks.
- Folder mapped → other section: flag = 0 for its tracks.
- Folder **not in the map**: untouched (manual tag-editor marks survive).

Call sites:
- `on_lib_row_save_section` (main.rs ~5218): on enter/leave `audiobooks`, sync.
- The add-popup confirm (Part D).
- After a music scan / on startup restore: reconcile over folders in the map.

### Part B — Card grid + detail page (`section_audiobooks.slint`)

Backend (main.rs), modelled on the podcast populate fns:
- `populate_audiobook_cards(w)` — groups `is_audiobook` tracks by `folder`:
  `BookCard { id, title, author, cover, chapters, total_time, resume_frac }`.
  - `title` = album tag else folder basename; `author` = `album_artist` else
    `artist`; `cover` = first chapter's tile thumb (Part C).
  - `id` = stable folder key (path, or index into a parallel `Vec<String>` held
    like `music_ids`) so `audiobook-open(id)` resolves the folder.
  - `resume_frac` = saved `audiobook_progress.position_s` / total for the
    most-recent chapter (0 if unseen) → card progress bar.
- `populate_audiobook_detail(folder)` — chapter rows sorted by `track_no` then
  filename: `ChapterRow { title, duration, index, played }`; `index` is the
  music library position so `audiobook-chapter-play(index)` reuses the existing
  `music_audiobook_play` path.

UI (`section_audiobooks.slint`), blue accent `#3b82f6`:
- **Card grid** when `!MusicState.audiobook-detail-open`: a `BookCardView`
  cloned from `PodcastTrendCardView`/`PodcastGrid`; click → `audiobook-open(id)`.
- **Hero detail** when open: cloned from the podcast detail hero
  (page_music.slint ~1817–1911): cover + info card (title / author /
  "N chapters" / total time) + Resume button + the speed/chapter/bookmark/
  skip-silence transport (reuse `book-speed`, `book-chapter`, `book-bookmark`,
  `book-skip-silence`, `book-bookmarks`) + big round Back → `audiobook-back`.
  Chapter list below clones `PodcastEpisodeItem`, plays via
  `audiobook-chapter-play(index)`.

New `MusicState`/`MusicPage`/MainWindow props + callbacks (declared on
MainWindow so Rust sets them, bridged through as in the split):
- `in property <[BookCard]> audiobook-cards`
- `in-out property <bool> audiobook-detail-open`
- detail scalars `ab-d-title/author/cover/total` + `ab-d-chapters: [ChapterRow]`
- callbacks `audiobook-open(string)`, `audiobook-back()`,
  `audiobook-chapter-play(int)`, `audiobook-resume()`

The old `audiobooks: [MusicSongRow]` / `audiobook-play(int)` flat-list path is
removed.

### Part C — Thumb placeholder

Card + hero cover bind to the first chapter's embedded thumb; when empty
(`.width == 0`), render the podcast-style fallback: rounded rect in the blue
audiobook tint (`#3b82f6` low alpha) + centered 📚 glyph — same pattern
podcasts use with 🎙 at page_music.slint:1832. Pure Slint fallback, no file gen.

### Part D — Add popup with section pick

New Slint dialog (top-layer, like the existing music dialogs): radio list of the
5 music sections + Add / Cancel. State on MainWindow (bridged to `MusicState`):
- `in-out property <bool> music-add-section-open`
- `in-out property <string> music-add-section-choice` (key, default `mymusic`)
- callbacks `music-add-folder-pick()`, `music-add-confirm(string)`,
  `music-add-cancel()`

Flow:
1. `+ Add` (music, non-podcast) → `music-add-folder-pick()`.
2. Rust: `rfd::FileDialog::pick_folder()`. On cancel, stop. On pick, store path
   in a static (`OnceLock<Mutex<Option<PathBuf>>>`), seed
   `music-add-section-choice` from the sections map (or `mymusic`), set
   `music-add-section-open = true`.
3. User picks section, Add → `music-add-confirm(key)`.
4. Rust: read stored path; `set_folder_section(path, key)`;
   `add_folder_path(window, path)`; if `key == "audiobooks"`
   `audiobooks::set_folder_flag(pool, path, true)` else clear; close popup;
   refresh music views.

Podcast `+ Add` (RSS URL) unchanged. Photos/videos/books keep direct pick — the
section popup is music-only.

## Files touched

- `ui/music_common.slint` *(new)* — structs, `global MusicState`, shared components.
- `ui/section_mymusic.slint`, `ui/section_podcasts.slint`,
  `ui/section_audiobooks.slint`, `ui/section_radio.slint`,
  `ui/section_youtube.slint` *(new)*.
- `ui/music_players.slint` *(new)* — fullscreen + mini players.
- `ui/page_music.slint` — reduced to the `MusicPage` shell + bridge.
- `ui/main.slint` — only the new audiobook + add-popup props/callbacks (split
  itself leaves it unchanged); audiobook add-section dialog if hosted at app top
  layer.
- `crates/tulipix-music/src/audiobooks.rs` — `set_folder_flag`, grouping
  helpers, unit tests.
- `crates/tulipix-app/src/main.rs` — `populate_audiobook_cards`,
  `populate_audiobook_detail`, add-flow callbacks + stored-path static,
  section-save flag sync, post-scan reconcile.

## Testing

- `audiobooks.rs` unit tests: `set_folder_flag` toggles `is_audiobook` for all
  tracks of one folder and only that folder; reconcile sets/clears mapped
  folders and leaves an unmapped folder's manual mark intact.
- Existing speed/progress/bookmark tests stay green.
- Split verification: after each migration step the music page hot-reloads and
  renders identically (all 5 sections navigable, player works).
- Manual feature check: add a folder as Audiobooks via the new popup → card
  appears → open → chapter list → Resume restores position + speed; a coverless
  book shows the 📚 placeholder; reassigning the folder to My Music in settings
  empties the card grid.

## Out of scope

- No new audiobook metadata fetching (author/series lookup).
- No `.m4b` chapter-splitting into separate library tracks (mpv internal
  chapters already handled at play time).
- Section popup for non-music media.
- Rewriting the ~907 Rust music accessor calls (explicitly rejected above).
