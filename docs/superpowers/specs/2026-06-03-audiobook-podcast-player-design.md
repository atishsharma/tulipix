# Audiobook section: podcast-style player + section-aware add

**Date:** 2026-06-03
**Status:** Approved for planning
**Area:** music (audiobooks), library add flow, settings

## Problem

The audiobook section is a flat `TrackRow` list. The podcast section has a
richer UX: a card grid of shows, a hero detail page, a chapter/episode list,
and a speed/bookmark transport. Two gaps:

1. The audiobook view should adopt the podcast browsing/playing style.
2. Assigning a folder to the "Audiobooks" music section (settings dropdown)
   does **not** populate the audiobook view today. The view only shows tracks
   with `track_meta.is_audiobook = 1`, which is currently set by hand via the
   tag editor / right-click. The folder→section routing and the audiobook view
   are disconnected.
3. An audiobook with no embedded cover renders no thumb.

## Decisions (from brainstorming)

- **Full card + detail UX** for audiobooks (closest to podcasts).
- Add popup offers **music sub-sections only** (My Music / Podcasts /
  Audiobooks / Radio / YouTube). Choosing Audiobooks sets `is_audiobook` on the
  folder's tracks.
- **Group by folder**: one card per source folder. Chapters = the audiobook
  tracks in that folder sorted by `track_no` then filename. A single `.m4b` is a
  one-track card; its internal mpv chapters drive chapter nav at play time.
- Add flow order: **pick directory first (rfd), then assign section (Slint
  dialog)**.

## Architecture

Reuse existing music infrastructure. `track_meta.folder` (already indexed) is
the grouping key. `track_meta.is_audiobook` already exists. No schema change.

### Part A — Folder→section drives `is_audiobook`

New helper in `crates/tulipix-music/src/audiobooks.rs`:

```rust
/// Set/clear is_audiobook for every track under one folder.
pub async fn set_folder_flag(pool: &SqlitePool, folder: &str, on: bool) -> Result<u64>;
```

`UPDATE track_meta SET is_audiobook = ? WHERE folder = ?`. Returns rows
affected.

Reconcile rule (authoritative source = the `music_folder_sections.json` map):

- Folder mapped → `audiobooks`: set flag = 1 for its tracks.
- Folder mapped → any other section: set flag = 0 for its tracks.
- Folder **not in the map**: leave flag untouched (manual tag-editor marks on
  My Music tracks survive).

Call sites:

- `on_lib_row_save_section` (main.rs ~5218): when the saved key enters or leaves
  `audiobooks`, sync the affected folder's flag.
- The new add-popup confirm (Part D).
- After a music scan completes / on startup restore: run the reconcile over the
  folders present in the sections map so flags match assignments without
  clobbering unmapped folders.

### Part B — Card grid + detail page

Backend (main.rs), two new populate functions modelled on the podcast ones:

- `populate_audiobook_cards(w)` — groups `is_audiobook` tracks by `folder`:
  `BookCard { id, title, author, cover, chapters, total_time, resume_frac }`.
  - `title` = album tag if present else folder basename.
  - `author` = `album_artist` else `artist`.
  - `cover` = first chapter track's tile thumb (see Part C).
  - `id` = stable folder identifier (the folder path or its index in a
    parallel `Vec<String>` held like `music_ids`), so `audiobook-open(id)` can
    resolve the folder.
  - `resume_frac` = saved `audiobook_progress.position_s` / total_time for the
    most-recently-played chapter, for a progress bar on the card (0 if unseen).
- `populate_audiobook_detail(folder)` — chapter rows for one book sorted by
  `track_no` then filename: `ChapterRow { title, duration, index, played }`.
  `index` is the music library position so `audiobook-chapter-play(index)`
  reuses the existing `music_audiobook_play` path.

UI (`ui/page_music.slint`) — replace the flat list block (lines ~1913–1936):

- **Card grid** when `!audiobook-detail-open`: clone `PodcastGrid` /
  `PodcastTrendCardView` layout, blue accent (`#3b82f6`), one `BookCardView`
  per card, click → `audiobook-open(id)`.
- **Hero detail** when `audiobook-detail-open`: clone the podcast detail hero
  (lines ~1817–1911): cover + outlined info card (title / author / "N chapters"
  / total time) + a Resume button (plays the resume chapter at saved position)
  + the speed / chapter / bookmark / skip-silence transport (reuse existing
  `book-speed`, `book-chapter`, `book-bookmark`, `book-skip-silence`,
  `book-bookmarks`) + a big round Back button → `audiobook-back`. Chapter list
  below clones `PodcastEpisodeItem` → each row plays via
  `audiobook-chapter-play(index)`.

New Slint state on the music page:

- `in property <[BookCard]> audiobook-cards`
- `in-out property <bool> audiobook-detail-open`
- detail fields: `ab-d-title`, `ab-d-author`, `ab-d-cover`, `ab-d-total`,
  `ab-d-chapters: [ChapterRow]` (+ any needed scalars)
- callbacks: `audiobook-open(string)`, `audiobook-back()`,
  `audiobook-chapter-play(int)`, `audiobook-resume()`

The existing `audiobooks: [MusicSongRow]` / `audiobook-play(int)` are removed or
left unused once the new view lands.

### Part C — Thumb placeholder

Card and hero cover bind to the first chapter's embedded thumb. When that image
is empty (`.width == 0`), render the podcast-style fallback: a rounded rect
filled with the blue audiobook tint (`#3b82f6` at low alpha) and a centered 📚
glyph — the same pattern podcasts use with 🎙 at page_music.slint:1832. No
file generation; pure Slint fallback.

### Part D — Add popup with section pick

New Slint dialog (top-layer, consistent with the existing music dialogs):
radio list of the 5 music sections + Add / Cancel. State:

- `in-out property <bool> music-add-section-open`
- `in-out property <string> music-add-section-choice` (key, default `mymusic`)
- callbacks `music-add-folder-pick()`, `music-add-confirm(string)`,
  `music-add-cancel()`

Flow:

1. `+ Add` button (music, non-podcast) → `music-add-folder-pick()`.
2. Rust: open `rfd::FileDialog::pick_folder()`. On cancel, stop. On pick, store
   the path in a static (`OnceLock<Mutex<Option<PathBuf>>>`), seed
   `music-add-section-choice` from the existing sections map (or `mymusic`), set
   `music-add-section-open = true`.
3. User picks a section, hits Add → `music-add-confirm(key)`.
4. Rust: read stored path; `set_folder_section(path, key)`;
   `add_folder_path(window, path)` (persists watched folder + scans); if
   `key == "audiobooks"` call `audiobooks::set_folder_flag(pool, path, true)`,
   else clear; close popup; refresh music views.

The podcast `+ Add` path (RSS URL dialog) is unchanged. Photos/videos/books
keep their current direct pick — the section popup is music-only.

## Files touched

- `crates/tulipix-music/src/audiobooks.rs` — `set_folder_flag`, folder-grouping
  query helper(s), unit tests.
- `crates/tulipix-app/src/main.rs` — `populate_audiobook_cards`,
  `populate_audiobook_detail`, add-flow callbacks + stored-path static,
  section-save flag sync, post-scan reconcile.
- `ui/page_music.slint` — `BookCard`/`ChapterRow` structs, card grid + hero
  detail + placeholder, new props/callbacks; remove the flat list block.
- `ui/main.slint` — wire the new props/callbacks and the add-section dialog.

## Testing

- `audiobooks.rs` unit tests: `set_folder_flag` toggles `is_audiobook` for all
  tracks of a folder and only that folder; reconcile sets/clears mapped folders
  and leaves an unmapped folder's manual mark intact.
- Existing speed/progress/bookmark tests stay green.
- Manual: add a folder as Audiobooks via the new popup → it appears as a card →
  open → chapter list → Resume restores position and speed; a coverless book
  shows the 📚 placeholder; reassigning the folder to My Music in settings
  empties the audiobook card.

## Out of scope

- No new metadata fetching for audiobooks (author/series lookup).
- No `.m4b` chapter-splitting into separate library tracks (internal mpv
  chapters already handled at play time).
- Section popup for non-music media.
