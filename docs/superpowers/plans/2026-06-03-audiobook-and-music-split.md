# Audiobook Section + Music Page Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the 329K `page_music.slint` monolith into a thin shell + shared module + 5 independent section files via a `global MusicState`, then rebuild the Audiobooks section with a podcast-style card/detail/chapter UI, a section-aware folder-add popup, and a placeholder cover for coverless books.

**Architecture:** Phase 1 is a `.slint`-only, hot-reloadable refactor: `MusicPage` keeps its exact external interface (so `main.slint` and ~907 Rust accessor calls stay untouched), and republishes its props/callbacks once into a `global MusicState` that the extracted section components read. Phase 2 adds the only Rust batch: a `set_folder_flag` helper + two `populate_audiobook_*` functions + add-popup callbacks, plus the new `section_audiobooks.slint` UI.

**Tech Stack:** Rust, Slint (UI, hot-reload via dev app), sqlx/SQLite, rfd (native folder picker), mpv (playback). Build under resource caps (`-j 1`, capped-build scope — see CLAUDE.md memories).

---

## Working rules (read before starting)

- **Hot-reload, don't rebuild, for `.slint` edits.** Keep the dev app alive; all
  of Phase 1 is verified by hot-reload render, no `cargo build`.
- **One Rust rebuild** at the end of Phase 2, using the capped-build incantation.
- Commits: author atishsharma, **no Claude co-author** (project rule). Use `rtk`
  for git. Branch `beta`.
- After each move task, the music page must render and navigate **identically**
  to before (all 5 tabs, bottom player, popups). If anything fails to render,
  revert that task's edit and re-do — do not stack a second move on a broken one.
- Slint has **no partial-file include**: a component lives in exactly one file.
  Cross-file use requires `export` at definition + `import` at use site.

---

## File Structure (target)

| File | Responsibility |
| --- | --- |
| `ui/music_common.slint` *(new)* | All shared `struct`s, `global MusicState`, shared components (buttons, chips, rows, grids, cards, viz, podcast item views). |
| `ui/music_players.slint` *(new)* | `MusicFullscreen`, `MusicMini`, `PodcastMini` (relocated as-is). |
| `ui/section_mymusic.slint` *(new)* | `MyMusicSection` — library grid, browse tabs, album/artist detail, lyrics, tag editor. |
| `ui/section_podcasts.slint` *(new)* | `PodcastsSection` — home/trends/subscribed/downloads + detail hero. |
| `ui/section_audiobooks.slint` *(new)* | `AudiobooksSection` — rebuilt: card grid + hero detail + chapters. |
| `ui/section_radio.slint` *(new)* | `RadioSection` — placeholder. |
| `ui/section_youtube.slint` *(new)* | `YouTubeSection` — placeholder. |
| `ui/page_music.slint` | thin `MusicPage` shell: header, bottom player, shared popups, `MusicState` bridge, `if MusicState.view==...` switch. |
| `crates/tulipix-music/src/audiobooks.rs` | + `set_folder_flag`, grouping query helpers, tests. |
| `crates/tulipix-app/src/main.rs` | + audiobook populate fns, add-popup callbacks, section-save flag sync, scan reconcile. |
| `ui/main.slint` | + new audiobook/add-popup props & callbacks; add-section dialog wiring. |

---

# PHASE 1 — Music page split (`.slint` only, hot-reload verified)

> No Rust rebuild in this phase. Keep the dev app running and hot-reloading.

## Task 1: Snapshot the baseline

**Files:** none (capture only)

- [ ] **Step 1: Confirm dev app is running and on the Music page**

Open the app to the Music page. Click each tab: My Music, Podcasts, Audiobooks,
Radio, YouTube. Open one album detail, the lyrics panel, the EQ popup, the viz
popup, a right-click context menu, the tag editor. Confirm all render.

- [ ] **Step 2: Record the MusicPage external interface**

Run: `grep -nE "^\s+(in|in-out|out) property|^\s+callback" ui/page_music.slint | sed -n '/925/,/3595/p'`
(Or open `ui/page_music.slint` and read lines 925–3595.)
Expected: the ~418 props/callbacks that form `MusicPage`'s interface. This list
is the source for the `MusicState` mirror in Task 3. Save it to a scratch file
`docs/superpowers/plans/.musicpage-iface.txt` (gitignored scratch, do not commit).

Run: `grep -nE "MusicState\.x|root\." -c ui/page_music.slint` is not needed —
just keep the interface list handy.

- [ ] **Step 3: Commit a clean starting point**

```bash
rtk git add -A && rtk git commit -m "chore: checkpoint before music page split"
```

---

## Task 2: Create `music_common.slint` with the shared structs

**Files:**
- Create: `ui/music_common.slint`
- Modify: `ui/page_music.slint` (remove moved structs, add import)

- [ ] **Step 1: Move the structs**

Cut these `export struct` blocks from `ui/page_music.slint` (currently lines
~10–101): `MusicSongRow`, `PodcastCard`, `PodcastTrendCard`,
`PodcastEpisodeRow`, `SongRowEx`, `MusicLyricLine`, `LyricsResult`,
`LyricsMgrRow`, `MetaMgrRow`. Paste them into a new `ui/music_common.slint`,
keeping the imports they need at the top of the new file (check the head of
`page_music.slint` for `import` lines the structs depend on — structs usually
depend on none, but copy `import { ... } from "theme.slint";` etc. if present).

- [ ] **Step 2: Import the structs back into page_music**

At the top of `ui/page_music.slint`, add:

```slint
import { MusicSongRow, PodcastCard, PodcastTrendCard, PodcastEpisodeRow, SongRowEx, MusicLyricLine, LyricsResult, LyricsMgrRow, MetaMgrRow } from "music_common.slint";
```

- [ ] **Step 3: Verify hot-reload render**

Save. The dev app hot-reloads. Confirm the Music page still renders and all 5
tabs navigate. If Slint reports a missing-type error, the import list in Step 2
is incomplete — add the missing struct name.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): move shared structs to music_common.slint"
```

---

## Task 3: Add the `global MusicState` to `music_common.slint`

**Files:**
- Modify: `ui/music_common.slint` (add the global)
- Modify: `ui/page_music.slint` (add the bridge block inside MusicPage)

- [ ] **Step 1: Declare the global**

In `ui/music_common.slint`, add a `global MusicState` whose members mirror every
prop/callback from the interface list captured in Task 1 Step 2. Convert each:
- `in property <T> name;`  → `in-out property <T> name;`
- `in-out property <T> name;` → `in-out property <T> name;`
- `out property <T> name;` → `in-out property <T> name;`
- `callback name(args) -> ret;` → `callback name(args) -> ret;`

(Globals can hold any access modifier; using `in-out` uniformly keeps the bridge
simple. Keep the same names and types.) Export it:

```slint
export global MusicState {
    in-out property <string> view: "mymusic";
    in-out property <[MusicSongRow]> audiobooks: [];
    // ... one line per interface member, same names/types ...
    callback song-play(int);
    // ... one line per callback ...
}
```

- [ ] **Step 2: Import the global into page_music**

Add `MusicState` to the import from `music_common.slint` at the top of
`ui/page_music.slint`.

- [ ] **Step 3: Add the bridge block inside MusicPage**

Inside the `MusicPage` component body (near the top, after its property
declarations), add one bridge line per interface member:
- `in` prop:      `MusicState.NAME: root.NAME;`
- `in-out` prop:  `MusicState.NAME <=> root.NAME;`
- `out` prop:     `root.NAME: MusicState.NAME;`
- callback:       `MusicState.NAME(a) => { root.NAME(a); }`

Generate this block mechanically from the captured interface list. Example:

```slint
MusicState.tiles: root.tiles;
MusicState.view <=> root.view;
MusicState.book-speed <=> root.book-speed;
MusicState.song-play(i) => { root.song-play(i); }
```

Do NOT yet change the section blocks — they still read `root.*`. This task only
adds the mirror; render must be unchanged.

- [ ] **Step 4: Verify hot-reload render**

Save. Confirm identical render + navigation. (The global is populated but not yet
consumed — no visual change expected.)

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): add global MusicState + bridge in MusicPage shell"
```

---

## Task 4: Move shared components into `music_common.slint`

**Files:**
- Modify: `ui/music_common.slint` (receive components)
- Modify: `ui/page_music.slint` (remove them, import them)

Shared components to move (defined ~lines 102–924): `AlbumCard`, `MiniCalendar`,
`PlayerBtn`, `GlyphBtn`, `Scrubber`, `EqBand`, `VizView`, `ArtistCard`,
`TileGrid`, `SortChip`, `Marquee`, `MusicMenuRow`, `TrackRow`, `SongCard`,
`MusicTab`, `BrowseChip`, `ColorChip`, `FancyCount`, `PageBtn`, `PageNav`,
`DetailActionBtn`, `GradOutline`, `TagField`, `PodcastCardView`,
`PodcastTrendCardView`, `PodcastGrid`, `PodcastEpisodeItem`.

- [ ] **Step 1: Move in dependency order**

Cut each component block from `page_music.slint` and paste into
`music_common.slint`. `export` each one (`export component PlayerBtn ...`).
Keep their relative order (a component must be defined before another that
instantiates it). Copy any `import` lines they rely on (`theme.slint`,
`icons.slint`, `tokens.slint`, `section_header.slint`, etc.) into the head of
`music_common.slint` if not already present.

If a shared component reads page state, change those reads to `MusicState.*`
(e.g. `PodcastEpisodeItem` referencing `root.podcast-dl-id` becomes
`MusicState.podcast-dl-id`). Components that only use their own `in`/`callback`
interface need no change.

- [ ] **Step 2: Import the components into page_music**

Add all moved component names to the `music_common.slint` import in
`page_music.slint`.

- [ ] **Step 3: Verify hot-reload render**

Save. Confirm identical render: cards, chips, rows, podcast grid, player buttons,
EQ band, viz all still appear and behave. Fix any missing import or
forward-reference order error reported by Slint.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): move shared components to music_common.slint"
```

---

## Task 5: Relocate the fullscreen/mini players to `music_players.slint`

**Files:**
- Create: `ui/music_players.slint`
- Modify: `ui/page_music.slint` (remove the three components)
- Modify: `ui/main.slint` (re-point the import)

- [ ] **Step 1: Move the components**

Cut `MusicFullscreen`, `MusicMini`, `PodcastMini` (the three `export component`s
at the end of `page_music.slint`, ~lines 3596–end) into a new
`ui/music_players.slint`. Copy needed `import` lines (theme/icons/common) to its
head, including `import { MusicState, <components/structs they use> } from "music_common.slint";`.
Change any `root.*` page-state reads in them to `MusicState.*`.

- [ ] **Step 2: Re-point imports**

In `ui/main.slint`, change the existing import (line ~11) so
`MusicFullscreen, MusicMini, PodcastMini` come from `music_players.slint` and the
structs come from `music_common.slint`; keep `MusicPage` from `page_music.slint`.
Add `import { MusicFullscreen, MusicMini, PodcastMini } from "music_players.slint";`

- [ ] **Step 3: Verify hot-reload render**

Save. Open the fullscreen player, the mini player, and (on the podcast view) the
podcast mini. Confirm all render and controls work.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): move fullscreen/mini players to music_players.slint"
```

---

## Task 6: Extract `RadioSection` and `YouTubeSection` (smallest first)

**Files:**
- Create: `ui/section_radio.slint`, `ui/section_youtube.slint`
- Modify: `ui/page_music.slint`

These two are the placeholder block (`if root.view == "radio" || root.view == "youtube"`, ~lines 1691–1701) — the simplest extraction, done first to prove the pattern.

- [ ] **Step 1: Create RadioSection**

Create `ui/section_radio.slint`:

```slint
import { MusicState } from "music_common.slint";
import { Icons, AppIcon } from "icons.slint";
import { MusicTheme } from "theme.slint";

export component RadioSection inherits Rectangle {
    vertical-stretch: 1; horizontal-stretch: 1;
    VerticalLayout {
        alignment: center; spacing: 14px;
        HorizontalLayout { alignment: center;
            Rectangle { width: 96px; height: 96px; border-radius: 48px; background: #ec489922;
                Text { text: MusicState.view-glyph("radio"); font-size: 40px; horizontal-alignment: center; vertical-alignment: center; } } }
        Text { text: MusicState.view-title("radio"); color: MusicTheme.fg; font-size: 22px; font-weight: 700; horizontal-alignment: center; }
        Text { text: MusicState.view-msg("radio"); color: MusicTheme.fg-dim2; font-size: 13px; horizontal-alignment: center; }
    }
}
```

(Confirm the actual import paths/names for `MusicTheme`, `Icons` by checking the
existing head of `page_music.slint`; `view-glyph/title/msg` are existing
`MusicPage` callbacks — they must exist on `MusicState`, which they do after
Task 3.)

- [ ] **Step 2: Create YouTubeSection**

Create `ui/section_youtube.slint`, identical but `"youtube"` in the three calls
and accent — copy the same block, swapping the string argument.

- [ ] **Step 3: Swap the inline block for the components**

In `page_music.slint`, replace the `if root.view == "radio" || root.view == "youtube": Rectangle { ... }`
block with:

```slint
if MusicState.view == "radio": RadioSection { }
if MusicState.view == "youtube": YouTubeSection { }
```

Add imports at the top of `page_music.slint`:

```slint
import { RadioSection } from "section_radio.slint";
import { YouTubeSection } from "section_youtube.slint";
```

- [ ] **Step 4: Verify hot-reload render**

Save. Click Radio and YouTube tabs. Confirm the placeholder renders for both.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): extract RadioSection + YouTubeSection"
```

---

## Task 7: Extract `PodcastsSection`

**Files:**
- Create: `ui/section_podcasts.slint`
- Modify: `ui/page_music.slint`

The podcast blocks are: `if root.view == "podcasts" && !root.podcast-detail-open`
(~1704–1815) and `if root.view == "podcasts" && root.podcast-detail-open`
(~1817–1911).

- [ ] **Step 1: Create the component**

Create `ui/section_podcasts.slint`:

```slint
import { MusicState, PodcastGrid, PodcastEpisodeItem, PodcastTrendCardView, SortChip, BrowseChip, PageNav } from "music_common.slint";
import { MusicTheme } from "theme.slint";
import { Icons, AppIcon } from "icons.slint";

export component PodcastsSection inherits Rectangle {
    vertical-stretch: 1; horizontal-stretch: 1;
    // (1) the non-detail ScrollView block, then (2) the detail ScrollView block.
    // Paste both blocks here, replacing every `root.` with `MusicState.`.
}
```

Move both podcast blocks verbatim into the component body, replacing every
`root.` with `MusicState.`. Add to the import list any shared component the
blocks instantiate that isn't already listed.

- [ ] **Step 2: Swap inline blocks**

In `page_music.slint`, delete the two podcast `if root.view == "podcasts" ...`
blocks and add in their place:

```slint
if MusicState.view == "podcasts": PodcastsSection { }
```

Add `import { PodcastsSection } from "section_podcasts.slint";` at the top.

- [ ] **Step 3: Verify hot-reload render**

Save. Click Podcasts. Exercise: Home, Trends, Subscribed, Downloads tabs; open a
show detail; hit Back; play an episode (mini player appears). Confirm all work.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): extract PodcastsSection"
```

---

## Task 8: Extract `MyMusicSection`

**Files:**
- Create: `ui/section_mymusic.slint`
- Modify: `ui/page_music.slint`

My Music owns the largest surface: the library grid (`if root.view == "mymusic" && tiles.length > 0`, ~1938+), the browse tabs, album/artist detail, lyrics view, tag editor, and the empty state. These all belong to My Music.

- [ ] **Step 1: Identify the My-Music-owned blocks**

Run: `grep -nE 'if root\.view == "mymusic"|if root\.(detail|lyrics|tag|meta|lib)-' ui/page_music.slint`
List every block gated on `mymusic` or on a My-Music-only sub-state (album/artist
detail, lyrics view/manager, tag editor, metadata manager). These move together.

- [ ] **Step 2: Create the component**

Create `ui/section_mymusic.slint` importing `MusicState` + every shared component
the blocks use (from `music_common.slint`) + theme/icons. Paste all My-Music
blocks into the component body, replacing every `root.` with `MusicState.`.

- [ ] **Step 3: Swap inline blocks**

Replace the moved blocks in `page_music.slint` with:

```slint
if MusicState.view == "mymusic": MyMusicSection { }
```

Add `import { MyMusicSection } from "section_mymusic.slint";`. Keep shared popups
(EQ, viz, sleep timer, right-click context menu) in the shell if they are invoked
from the bottom player; move them into the section only if they are exclusively
My-Music. When unsure, leave a popup in the shell — it can read `MusicState.*`.

- [ ] **Step 4: Verify hot-reload render**

Save. Click My Music. Exercise: library grid, browse tabs (albums/artists/
genres/folders/playlists), open album detail, open artist, lyrics panel, tag
editor, metadata manager, pagination. Confirm all render and act.

- [ ] **Step 5: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): extract MyMusicSection"
```

---

## Task 9: Stub `AudiobooksSection` (flat list moved out as-is)

**Files:**
- Create: `ui/section_audiobooks.slint`
- Modify: `ui/page_music.slint`

This moves the current flat-list audiobook block out unchanged, so Phase 1 ends
with the shell fully sectioned. Phase 2 rewrites this file's body.

- [ ] **Step 1: Create the component**

Create `ui/section_audiobooks.slint`:

```slint
import { MusicState, TrackRow, BrowseChip } from "music_common.slint";
import { MusicTheme } from "theme.slint";

export component AudiobooksSection inherits Rectangle {
    vertical-stretch: 1; horizontal-stretch: 1;
    // Paste the current `if root.view == "audiobooks": ScrollView { ... }` body
    // here (without the outer `if`), replacing every `root.` with `MusicState.`.
}
```

- [ ] **Step 2: Swap inline block**

In `page_music.slint` replace `if root.view == "audiobooks": ScrollView { ... }`
(~1913–1936) with:

```slint
if MusicState.view == "audiobooks": AudiobooksSection { }
```

Add `import { AudiobooksSection } from "section_audiobooks.slint";`.

- [ ] **Step 3: Verify hot-reload render**

Save. Click Audiobooks. Confirm the flat list + speed chips render as before.

- [ ] **Step 4: Commit**

```bash
rtk git add -A && rtk git commit -m "refactor(music): extract AudiobooksSection (flat list, pre-rewrite)"
```

---

## Task 10: Convert remaining shell `root.*` reads + final verification

**Files:**
- Modify: `ui/page_music.slint`

- [ ] **Step 1: Confirm the shell is thin**

Run: `wc -l ui/page_music.slint`
Expected: dramatically smaller than 329K (the shell = header + bottom player +
shared popups + bridge + section switch).

- [ ] **Step 2: Full regression pass**

In the dev app, walk every section + the bottom player + fullscreen/mini once
more. Everything must render and behave as on the Task 1 baseline.

- [ ] **Step 3: Commit (if any cleanup edits)**

```bash
rtk git add -A && rtk git commit -m "refactor(music): finalize thin MusicPage shell"
```

---

# PHASE 2 — Audiobook feature (Rust batch + section rebuild)

## Task 11: `set_folder_flag` helper + tests

**Files:**
- Modify: `crates/tulipix-music/src/audiobooks.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/tulipix-music/src/audiobooks.rs`:

```rust
    #[tokio::test]
    async fn folder_flag_scopes_to_one_folder() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/music/song.mp3").await;
        // Tracks need their folder column set for the WHERE clause.
        for (id, folder) in [(a, "/books/dune"), (b, "/books/dune"), (c, "/music")] {
            sqlx::query("UPDATE track_meta SET folder = ? WHERE item_id = ?")
                .bind(folder).bind(id).execute(&pool).await.unwrap();
        }
        let n = set_folder_flag(&pool, "/books/dune", true).await.unwrap();
        assert_eq!(n, 2);
        let flagged: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE is_audiobook = 1 ORDER BY item_id")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(flagged, vec![a, b]);
        // Clearing scopes too.
        let n2 = set_folder_flag(&pool, "/books/dune", false).await.unwrap();
        assert_eq!(n2, 2);
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(still, 0);
    }
```

Check `crate::schema::tests::add_track` sets `folder`; if it already does, drop
the manual `UPDATE` loop. If `add_track` has a different signature, adapt the
calls (read the helper first).

- [ ] **Step 2: Run test, verify it fails**

Run: `rtk cargo test -p tulipix-music folder_flag_scopes -j 1`
Expected: FAIL — `set_folder_flag` not found.

- [ ] **Step 3: Implement**

Add to `crates/tulipix-music/src/audiobooks.rs`:

```rust
/// Set (or clear) `is_audiobook` for every track whose `folder` matches.
/// Returns the number of track_meta rows updated.
pub async fn set_folder_flag(pool: &SqlitePool, folder: &str, on: bool) -> Result<u64> {
    let res = sqlx::query("UPDATE track_meta SET is_audiobook = ? WHERE folder = ?")
        .bind(if on { 1 } else { 0 })
        .bind(folder)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
```

- [ ] **Step 4: Run test, verify it passes**

Run: `rtk cargo test -p tulipix-music folder_flag_scopes -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/tulipix-music/src/audiobooks.rs
rtk git commit -m "feat(music): set_folder_flag toggles is_audiobook by folder"
```

---

## Task 12: Book-grouping query helper + tests

**Files:**
- Modify: `crates/tulipix-music/src/audiobooks.rs`
- Test: same file

- [ ] **Step 1: Write the failing test**

```rust
    #[tokio::test]
    async fn book_folders_groups_and_counts() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/books/hobbit/all.m4b").await;
        for (id, folder, on) in [(a, "/books/dune", true), (b, "/books/dune", true), (c, "/books/hobbit", true)] {
            sqlx::query("UPDATE track_meta SET folder = ?, is_audiobook = ? WHERE item_id = ?")
                .bind(folder).bind(on as i32).bind(id).execute(&pool).await.unwrap();
        }
        let books = book_folders(&pool).await.unwrap();
        // Sorted by folder; (folder, chapter_count).
        assert_eq!(books, vec![
            ("/books/dune".to_string(), 2),
            ("/books/hobbit".to_string(), 1),
        ]);
    }
```

- [ ] **Step 2: Run test, verify it fails**

Run: `rtk cargo test -p tulipix-music book_folders_groups -j 1`
Expected: FAIL — `book_folders` not found.

- [ ] **Step 3: Implement**

```rust
/// Distinct audiobook folders with chapter counts, ordered by folder path.
/// One row per book card (np.p5.music.audiobook-chapters).
pub async fn book_folders(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT folder, COUNT(*) AS n FROM track_meta \
         WHERE is_audiobook = 1 AND folder IS NOT NULL AND folder <> '' \
         GROUP BY folder ORDER BY folder")
        .fetch_all(pool).await?;
    Ok(rows)
}

/// item_ids of one book's chapters, ordered by track_no then item_id.
pub async fn book_chapters(pool: &SqlitePool, folder: &str) -> Result<Vec<i64>> {
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT item_id FROM track_meta \
         WHERE is_audiobook = 1 AND folder = ? \
         ORDER BY COALESCE(track_no, 1000000), item_id")
        .bind(folder).fetch_all(pool).await?;
    Ok(rows)
}
```

- [ ] **Step 4: Run test, verify it passes**

Run: `rtk cargo test -p tulipix-music book_folders_groups -j 1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/tulipix-music/src/audiobooks.rs
rtk git commit -m "feat(music): book_folders + book_chapters grouping queries"
```

---

## Task 13: New Slint structs + MusicState members for the audiobook UI

**Files:**
- Modify: `ui/music_common.slint` (structs + MusicState members)
- Modify: `ui/main.slint` (MainWindow props/callbacks)
- Modify: `ui/page_music.slint` (bridge lines for the new members)

- [ ] **Step 1: Add structs to `music_common.slint`**

```slint
export struct BookCard {
    id: string,          // folder path (stable key)
    title: string,
    author: string,
    cover: image,
    chapters: int,
    total-time: string,  // pre-formatted, e.g. "8h 12m"
    resume-frac: float,  // 0..1 progress for the card bar
}
export struct ChapterRow {
    title: string,
    duration: string,
    index: int,          // music library position for audiobook-chapter-play
    played: bool,
}
```

- [ ] **Step 2: Add members to `global MusicState`**

```slint
    in-out property <[BookCard]> audiobook-cards: [];
    in-out property <bool> audiobook-detail-open: false;
    in-out property <string> ab-d-title;
    in-out property <string> ab-d-author;
    in-out property <image> ab-d-cover;
    in-out property <string> ab-d-total;
    in-out property <[ChapterRow]> ab-d-chapters: [];
    callback audiobook-open(string);
    callback audiobook-back();
    callback audiobook-chapter-play(int);
    callback audiobook-resume();
```

- [ ] **Step 3: Add matching props/callbacks to MainWindow in `main.slint`**

Add the same members (same names, with the `music-` style already used for the
section, or plain — match the existing convention; the MusicPage instance must
forward them). Concretely, add to the `MusicPage { ... }` instantiation in
`main.slint` the bindings and to MainWindow the backing props:

```slint
// MainWindow properties:
in property <[BookCard]> music-audiobook-cards: [];
in-out property <bool> music-audiobook-detail-open: false;
in-out property <string> music-ab-d-title;
in-out property <string> music-ab-d-author;
in property <image> music-ab-d-cover;
in-out property <string> music-ab-d-total;
in property <[ChapterRow]> music-ab-d-chapters: [];
callback music-audiobook-open(string);
callback music-audiobook-back();
callback music-audiobook-chapter-play(int);
callback music-audiobook-resume();
```

Import `BookCard, ChapterRow` from `music_common.slint` in `main.slint`. Bind on
the `MusicPage { }` instance:

```slint
audiobook-cards: root.music-audiobook-cards;
audiobook-detail-open <=> root.music-audiobook-detail-open;
ab-d-title <=> root.music-ab-d-title;
ab-d-author <=> root.music-ab-d-author;
ab-d-cover: root.music-ab-d-cover;
ab-d-total <=> root.music-ab-d-total;
ab-d-chapters: root.music-ab-d-chapters;
audiobook-open(id) => { root.music-audiobook-open(id); }
audiobook-back() => { root.music-audiobook-back(); }
audiobook-chapter-play(i) => { root.music-audiobook-chapter-play(i); }
audiobook-resume() => { root.music-audiobook-resume(); }
```

- [ ] **Step 4: Add bridge lines in `MusicPage` (page_music.slint)**

`MusicPage` now needs the matching `in/in-out property` + callback declarations
(to receive from main.slint) and bridge lines to MusicState:

```slint
// declarations on MusicPage:
in property <[BookCard]> audiobook-cards: [];
in-out property <bool> audiobook-detail-open: false;
in-out property <string> ab-d-title;
in-out property <string> ab-d-author;
in property <image> ab-d-cover;
in-out property <string> ab-d-total;
in property <[ChapterRow]> ab-d-chapters: [];
callback audiobook-open(string);
callback audiobook-back();
callback audiobook-chapter-play(int);
callback audiobook-resume();
// bridge:
MusicState.audiobook-cards: root.audiobook-cards;
MusicState.audiobook-detail-open <=> root.audiobook-detail-open;
MusicState.ab-d-title <=> root.ab-d-title;
MusicState.ab-d-author <=> root.ab-d-author;
MusicState.ab-d-cover: root.ab-d-cover;
MusicState.ab-d-total <=> root.ab-d-total;
MusicState.ab-d-chapters: root.ab-d-chapters;
MusicState.audiobook-open(id) => { root.audiobook-open(id); }
MusicState.audiobook-back() => { root.audiobook-back(); }
MusicState.audiobook-chapter-play(i) => { root.audiobook-chapter-play(i); }
MusicState.audiobook-resume() => { root.audiobook-resume(); }
```

Import `BookCard, ChapterRow` in `page_music.slint` if not already.

- [ ] **Step 5: Verify hot-reload render**

Save. App still renders (new members unused yet). No Rust rebuild yet — these are
pure Slint additions; the Rust setters added in Task 15 must exist before the app
*restarts*, but hot-reload of `.slint` tolerates unset props (defaults).

- [ ] **Step 6: Commit**

```bash
rtk git add ui/music_common.slint ui/main.slint ui/page_music.slint
rtk git commit -m "feat(music): audiobook BookCard/ChapterRow structs + MusicState members"
```

---

## Task 14: Rebuild `section_audiobooks.slint` — card grid + hero detail + placeholder

**Files:**
- Modify: `ui/section_audiobooks.slint`

- [ ] **Step 1: Replace the body with the card grid + detail**

Replace the stub body from Task 9 with:

```slint
import { MusicState, BookCard, ChapterRow, PageNav } from "music_common.slint";
import { MusicTheme } from "theme.slint";
import { Icons, AppIcon } from "icons.slint";

// One book cover with the 📚 placeholder fallback.
component BookCover inherits Rectangle {
    in property <image> cover;
    in property <length> glyph-size: 40px;
    border-radius: 12px; clip: true;
    background: MusicTheme.surf-thumb;
    if root.cover.width == 0: Rectangle {
        background: #3b82f61f;
        Text { text: "📚"; font-size: root.glyph-size; horizontal-alignment: center; vertical-alignment: center; }
    }
    if root.cover.width > 0: Image { source: root.cover; image-fit: cover; width: 100%; height: 100%; }
}

component BookCardView inherits Rectangle {
    in property <BookCard> card;
    callback open();
    border-radius: 14px;
    background: ta.has-hover ? MusicTheme.hov2 : MusicTheme.surf00;
    border-width: 1px; border-color: MusicTheme.line;
    animate background { duration: 120ms; }
    ta := TouchArea { mouse-cursor: pointer; clicked => { root.open(); } }
    VerticalLayout { padding: 10px; spacing: 8px;
        BookCover { cover: root.card.cover; height: self.width; }
        Text { text: root.card.title; color: MusicTheme.fg; font-size: 13px; font-weight: 700; overflow: elide; }
        if root.card.author != "": Text { text: root.card.author; color: MusicTheme.fg-dim2; font-size: 11px; overflow: elide; }
        HorizontalLayout { spacing: 6px;
            Text { text: root.card.chapters + " ch"; color: MusicTheme.fg-dim; font-size: 11px; }
            Rectangle { horizontal-stretch: 1; }
            Text { text: root.card.total-time; color: MusicTheme.fg-dim; font-size: 11px; } }
        // Resume progress bar.
        Rectangle { height: 4px; border-radius: 2px; background: MusicTheme.line;
            Rectangle { x: 0; width: parent.width * Math.clamp(root.card.resume-frac, 0, 1);
                height: 100%; border-radius: 2px; background: #3b82f6; } }
    }
}

component ChapterItem inherits Rectangle {
    in property <ChapterRow> row;
    in property <int> n;
    callback play();
    height: 40px; border-radius: 8px;
    background: cta.has-hover ? MusicTheme.hov2 : transparent;
    cta := TouchArea { mouse-cursor: pointer; clicked => { root.play(); } }
    HorizontalLayout { padding-left: 12px; padding-right: 12px; spacing: 10px;
        VerticalLayout { alignment: center; Text { text: root.n + "."; color: MusicTheme.fg-dim; font-size: 12px; width: 28px; } }
        VerticalLayout { alignment: center; horizontal-stretch: 1;
            Text { text: root.row.title; color: root.row.played ? MusicTheme.fg-dim2 : MusicTheme.fg; font-size: 13px; font-weight: 600; overflow: elide; } }
        VerticalLayout { alignment: center; Text { text: root.row.duration; color: MusicTheme.fg-dim; font-size: 12px; } }
    }
}

export component AudiobooksSection inherits Rectangle {
    vertical-stretch: 1; horizontal-stretch: 1;

    // ── GRID ────────────────────────────────────────────────────────────
    if !MusicState.audiobook-detail-open: ScrollView {
        VerticalLayout { padding: 36px; padding-top: 20px; spacing: 18px;
            Text { text: "Audiobooks"; color: MusicTheme.fg; font-size: 26px; font-weight: 800; }
            if MusicState.audiobook-cards.length == 0: Text {
                text: "No audiobooks yet — use + Add (top-right) and assign the folder to Audiobooks, or mark a track via its tag editor.";
                color: MusicTheme.fg-dim2; font-size: 13px; }
            Rectangle { horizontal-stretch: 1;
                property <int> cols: 6;
                property <length> cw: self.width / cols;
                property <length> ch: cw + 92px;
                height: Math.ceil(MusicState.audiobook-cards.length / cols) * ch;
                for c[idx] in MusicState.audiobook-cards: BookCardView {
                    x: Math.mod(idx, cols) * cw + 7px;
                    y: Math.floor(idx / cols) * ch;
                    width: cw - 14px; height: ch - 14px;
                    card: c;
                    open => { MusicState.audiobook-open(c.id); } } }
        }
    }

    // ── DETAIL ──────────────────────────────────────────────────────────
    if MusicState.audiobook-detail-open: ScrollView {
        VerticalLayout { padding: 0px; spacing: 18px; alignment: start;
            // Hero.
            Rectangle { height: 320px; horizontal-stretch: 1; clip: true;
                Rectangle { width: parent.width; height: parent.height;
                    background: @linear-gradient(180deg, #3b82f659, MusicTheme.surf-page); }
                HorizontalLayout { padding-left: 36px; padding-right: 36px; padding-top: 18px; padding-bottom: 16px; spacing: 22px;
                    BookCover { width: 280px; height: 280px; glyph-size: 56px; cover: MusicState.ab-d-cover;
                        y: (parent.height - self.height)/2; border-width: 1px; border-color: MusicTheme.line;
                        drop-shadow-blur: 24px; drop-shadow-color: #000000aa; }
                    Rectangle { horizontal-stretch: 0; min-width: 460px; max-width: 620px; height: 280px;
                        y: (parent.height - self.height)/2; border-radius: 14px;
                        background: MusicTheme.surf00; border-width: 1px; border-color: MusicTheme.line;
                        VerticalLayout { padding: 18px; spacing: 7px; alignment: start;
                            Text { text: "AUDIOBOOK"; color: MusicTheme.fg-dim; font-size: 11px; font-weight: 700; letter-spacing: 2px; }
                            Text { text: MusicState.ab-d-title; color: MusicTheme.fg; font-size: 28px; font-weight: 800; overflow: elide; }
                            if MusicState.ab-d-author != "": Text { text: MusicState.ab-d-author; color: MusicTheme.fg3; font-size: 14px; overflow: elide; }
                            HorizontalLayout { spacing: 8px; alignment: start;
                                Rectangle { height: 24px; border-radius: 12px; background: MusicTheme.surf1; border-width: 1px; border-color: MusicTheme.line; horizontal-stretch: 0;
                                    HorizontalLayout { padding-left: 10px; padding-right: 10px;
                                        Text { text: MusicState.ab-d-chapters.length + " chapters · " + MusicState.ab-d-total; color: MusicTheme.fg2; font-size: 12px; font-weight: 700; vertical-alignment: center; } } } }
                            // Resume + speed/chapter/bookmark transport.
                            HorizontalLayout { spacing: 8px; alignment: start;
                                Rectangle { height: 30px; border-radius: 15px; horizontal-stretch: 0; background: rb.has-hover ? #2563eb : #3b82f6;
                                    rb := TouchArea { mouse-cursor: pointer; clicked => { MusicState.audiobook-resume(); } }
                                    HorizontalLayout { padding-left: 14px; padding-right: 14px;
                                        Text { text: "▶ Resume"; color: #ffffff; font-size: 12px; font-weight: 700; vertical-alignment: center; } } } }
                            Rectangle { vertical-stretch: 1; }
                            HorizontalLayout { spacing: 8px; alignment: start;
                                for sp[i] in [0.75, 1.0, 1.25, 1.5, 2.0]: BrowseChip {
                                    label: MusicState.book-speed == sp ? "● " + sp + "×" : sp + "×"; clicked => { MusicState.set-book-speed(sp); } } }
                            HorizontalLayout { spacing: 8px; alignment: start;
                                BrowseChip { label: "⏮ Chapter"; clicked => { MusicState.book-chapter(-1); } }
                                BrowseChip { label: "Chapter ⏭"; clicked => { MusicState.book-chapter(1); } }
                                BrowseChip { label: MusicState.book-skip-silence-on ? "● Skip silence" : "Skip silence"; clicked => { MusicState.book-skip-silence(); } }
                                BrowseChip { label: "+ Bookmark"; clicked => { MusicState.book-bookmark(); } } }
                        } }
                    // Big round Back.
                    Rectangle { width: 280px; height: 280px; border-radius: 140px; horizontal-stretch: 0;
                        y: (parent.height - self.height)/2;
                        background: bkp.has-hover ? #3b82f6 : MusicTheme.surf1;
                        border-width: 2px; border-color: bkp.has-hover ? #3b82f6 : MusicTheme.line;
                        drop-shadow-blur: bkp.has-hover ? 34px : 0px; drop-shadow-color: #3b82f6aa;
                        animate background, drop-shadow-blur, border-color { duration: 180ms; easing: ease-out; }
                        bkp := TouchArea { mouse-cursor: pointer; clicked => { MusicState.audiobook-back(); } }
                        VerticalLayout { alignment: center; spacing: 4px;
                            HorizontalLayout { alignment: center;
                                AppIcon { source: Icons.chevron-left; tint: bkp.has-hover ? #ffffff : MusicTheme.fg2; width: 48px; height: 48px; } }
                            Text { text: "Back"; color: bkp.has-hover ? #ffffff : MusicTheme.fg2; font-size: 16px; font-weight: 700; horizontal-alignment: center; } } }
                    Rectangle { horizontal-stretch: 1; } } }
            // Chapter list.
            VerticalLayout { padding-left: 36px; padding-right: 36px; padding-bottom: 24px; spacing: 2px;
                if MusicState.ab-d-chapters.length == 0: Text { text: "No chapters."; color: MusicTheme.fg-dim2; font-size: 13px; }
                for ch[i] in MusicState.ab-d-chapters: ChapterItem {
                    row: ch; n: i + 1; play => { MusicState.audiobook-chapter-play(ch.index); } }
                // Bookmarks.
                if MusicState.book-bookmarks.length > 0: HorizontalLayout { spacing: 8px; alignment: start;
                    Text { text: "Bookmarks"; color: MusicTheme.fg-mute; font-size: 12px; }
                    for b[i] in MusicState.book-bookmarks: BrowseChip { label: b; clicked => { MusicState.book-bookmark-jump(i); } } }
            }
        }
    }
}
```

Confirm the names `MusicTheme.surf-thumb`, `.surf00`, `.surf1`, `.hov2`,
`.fg/.fg2/.fg3/.fg-dim/.fg-dim2/.fg-mute`, `.line`, `.surf-page` exist (they are
used by the podcast detail this clones). Confirm `BrowseChip` is exported from
`music_common.slint` and import it if the grid/detail uses it.

- [ ] **Step 2: Verify hot-reload render**

Save. Click Audiobooks. With no data yet the grid shows the empty message. If
Slint errors on a missing theme token or unexported component, fix the
import/name. (Detail is unreachable until Task 15 wires `audiobook-open`.)

- [ ] **Step 3: Commit**

```bash
rtk git add ui/section_audiobooks.slint
rtk git commit -m "feat(music): audiobook card grid + hero detail + 📚 placeholder"
```

---

## Task 15: Rust — populate cards/detail + open/back/play/resume wiring

**Files:**
- Modify: `crates/tulipix-app/src/main.rs`

- [ ] **Step 1: Replace `populate_audiobooks` with `populate_audiobook_cards`**

Replace the body of `populate_audiobooks` (main.rs ~9848–9875) — keep it callable
from the view dispatch (line ~1124) but rename/repurpose to fill the new cards:

```rust
/// Fill the audiobook card grid: one BookCard per folder of is_audiobook tracks
/// (np.p5.music.audiobook-chapters).
fn populate_audiobook_cards(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let books = tulipix_music::audiobooks::book_folders(&pool).await.unwrap_or_default();
        // For each folder grab first chapter (for cover/title) + total duration + resume.
        let mut cards: Vec<(String, String, String, i64, f64, f64)> = Vec::new();
        // (folder, title, author, chapters, total_s, resume_s)
        for (folder, n) in &books {
            let ids = tulipix_music::audiobooks::book_chapters(&pool, folder).await.unwrap_or_default();
            let first = ids.first().copied().unwrap_or(-1);
            let (title, author): (Option<String>, Option<String>) = sqlx::query_as(
                "SELECT al.title, tm.album_artist FROM track_meta tm \
                 LEFT JOIN albums al ON al.id = tm.album_id WHERE tm.item_id = ?")
                .bind(first).fetch_optional(&pool).await.ok().flatten().unwrap_or((None, None));
            let total_s: f64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(duration_s),0) FROM track_meta WHERE is_audiobook=1 AND folder=?")
                .bind(folder).fetch_one(&pool).await.unwrap_or(0.0);
            // Resume = most recent chapter's saved position.
            let resume_s: f64 = sqlx::query_scalar(
                "SELECT ap.position_s FROM audiobook_progress ap \
                 JOIN track_meta tm ON tm.item_id = ap.item_id \
                 WHERE tm.is_audiobook=1 AND tm.folder=? ORDER BY ap.updated DESC LIMIT 1")
                .bind(folder).fetch_optional(&pool).await.ok().flatten().unwrap_or(0.0);
            let base = std::path::Path::new(folder).file_name().and_then(|s| s.to_str()).unwrap_or(folder).to_string();
            cards.push((
                folder.clone(),
                title.filter(|s| !s.is_empty()).unwrap_or(base),
                author.unwrap_or_default(),
                *n,
                total_s,
                resume_s,
            ));
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
            let tiles = w.get_music_tiles();
            let rows: Vec<BookCard> = cards.iter().map(|(folder, title, author, n, total_s, resume_s)| {
                // Cover = first chapter's tile thumb (empty image → 📚 placeholder in UI).
                let cover = {
                    // first chapter item_id → library pos → tile thumb
                    let cover_img = slint::Image::default();
                    cover_img
                };
                let _ = &pos_of; let _ = &tiles; // cover lookup below replaces this
                BookCard {
                    id: folder.clone().into(),
                    title: title.clone().into(),
                    author: author.clone().into(),
                    cover,
                    chapters: *n as i32,
                    total_time: fmt_hms(*total_s).into(),
                    resume_frac: if *total_s > 0.0 { (*resume_s / *total_s) as f32 } else { 0.0 },
                }
            }).collect();
            w.set_music_audiobook_cards(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}
```

Replace the placeholder `cover` block with a real lookup: resolve the folder's
first chapter `item_id` → `pos_of` → `tiles.row_data(pos).thumb`. To do that,
carry the first `item_id` in the `cards` tuple. Adjust the tuple to include
`first_id: i64` and compute:

```rust
let cover = pos_of.get(&first_id).copied()
    .filter(|p| *p >= 0 && (*p as usize) < tiles.row_count())
    .and_then(|p| tiles.row_data(p as usize).map(|t| t.thumb))
    .unwrap_or_default();
```

Add a `fmt_hms` helper near `fmt_clock` if none exists:

```rust
/// "8h 12m" / "47m" from seconds, for audiobook totals.
fn fmt_hms(s: f64) -> String {
    let m = (s / 60.0).round() as i64;
    if m >= 60 { format!("{}h {}m", m / 60, m % 60) } else { format!("{}m", m) }
}
```

Update the dispatch at main.rs ~1124 (`"audiobooks" => populate_audiobooks(&w)`)
to call `populate_audiobook_cards(&w)`.

- [ ] **Step 2: Add `populate_audiobook_detail` + open/back/play/resume callbacks**

In the music callbacks setup area (near the other `window.on_music_*` handlers,
e.g. around the audiobook play handler ~3287), add:

```rust
/// Fill the audiobook detail (chapters + hero meta) for one folder.
fn populate_audiobook_detail(w: &MainWindow, folder: String) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
        let (title, author): (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT al.title, tm.album_artist FROM track_meta tm \
             LEFT JOIN albums al ON al.id = tm.album_id WHERE tm.item_id = ?")
            .bind(ids.first().copied().unwrap_or(-1)).fetch_optional(&pool).await.ok().flatten().unwrap_or((None, None));
        let total_s: f64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(duration_s),0) FROM track_meta WHERE is_audiobook=1 AND folder=?")
            .bind(&folder).fetch_one(&pool).await.unwrap_or(0.0);
        let _ = weak.upgrade_in_event_loop(move |w| {
            let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
            let by_pos: std::collections::HashMap<i32, (String, f64)> = music_songs().lock()
                .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.duration_s))).collect()).unwrap_or_default();
            let tiles = w.get_music_tiles();
            let base = std::path::Path::new(&folder).file_name().and_then(|s| s.to_str()).unwrap_or(&folder).to_string();
            let rows: Vec<ChapterRow> = ids.iter().enumerate().filter_map(|(_, id)| {
                let pos = *pos_of.get(id)?;
                let (t, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
                Some(ChapterRow {
                    title: if t.is_empty() { "Chapter".into() } else { t.into() },
                    duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
                    index: pos,
                    played: false,
                })
            }).collect();
            let cover = ids.first().and_then(|id| pos_of.get(id).copied())
                .filter(|p| *p >= 0 && (*p as usize) < tiles.row_count())
                .and_then(|p| tiles.row_data(p as usize).map(|t| t.thumb))
                .unwrap_or_default();
            w.set_music_ab_d_title(title.filter(|s| !s.is_empty()).unwrap_or(base).into());
            w.set_music_ab_d_author(author.unwrap_or_default().into());
            w.set_music_ab_d_cover(cover);
            w.set_music_ab_d_total(fmt_hms(total_s).into());
            w.set_music_ab_d_chapters(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_music_audiobook_detail_open(true);
        });
    });
}
```

Wire the callbacks (use a static for the open book folder so resume/play know it):

```rust
static AB_OPEN_FOLDER: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn ab_open_folder() -> &'static std::sync::Mutex<String> { AB_OPEN_FOLDER.get_or_init(|| std::sync::Mutex::new(String::new())) }

let w = window.as_weak();
window.on_music_audiobook_open(move |id| {
    if let Ok(mut g) = ab_open_folder().lock() { *g = id.to_string(); }
    if let Some(w) = w.upgrade() { populate_audiobook_detail(&w, id.to_string()); }
});
let w = window.as_weak();
window.on_music_audiobook_back(move || {
    if let Some(w) = w.upgrade() { w.set_music_audiobook_detail_open(false); }
});
// Chapter play reuses the existing audiobook play path (pitch-preserving speed + resume).
let w = window.as_weak();
window.on_music_audiobook_chapter_play(move |pos| {
    if let Some(w) = w.upgrade() { w.invoke_music_audiobook_play(pos); }
});
// Resume = play the most-recently-updated chapter of the open book.
let w = window.as_weak();
window.on_music_audiobook_resume(move || {
    let Some(w) = w.upgrade() else { return; };
    let folder = ab_open_folder().lock().map(|g| g.clone()).unwrap_or_default();
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let last: Option<i64> = sqlx::query_scalar(
            "SELECT ap.item_id FROM audiobook_progress ap JOIN track_meta tm ON tm.item_id=ap.item_id \
             WHERE tm.is_audiobook=1 AND tm.folder=? ORDER BY ap.updated DESC LIMIT 1")
            .bind(&folder).fetch_optional(&pool).await.ok().flatten();
        let target = if let Some(id) = last { id }
            else { tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default().first().copied().unwrap_or(-1) };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
            if let Some(&pos) = pos_of.get(&target) { w.invoke_music_audiobook_play(pos); }
        });
    });
});
```

Confirm `invoke_music_audiobook_play` is the generated name for the existing
`music-audiobook-play(int)` callback; if the callback is named differently, match
it. Remove the now-dead `populate_audiobooks` flat-list fn and its
`set_music_audiobooks` usage.

- [ ] **Step 3: Wire the section-save flag sync**

In `on_lib_row_save_section` (main.rs ~5218), after `set_folder_section`, sync the
flag:

```rust
let folder = row.path.to_string();
let on = key == "audiobooks";
tokio::runtime::Handle::current().spawn(async move {
    if let Ok(pool) = pool_for("music").await {
        let _ = tulipix_music::audiobooks::set_folder_flag(&pool, &folder, on).await;
    }
});
```

(Place it so `folder`/`key` are still in scope; `key` is `&'static str`.)

- [ ] **Step 4: Reconcile flags after a music scan (back-fill pre-assigned folders)**

So folders already mapped to `audiobooks` before this feature get
`is_audiobook=1` without a manual re-save, add a reconcile that runs after the
music views populate. Put this helper near `populate_audiobook_cards`:

```rust
/// Sync is_audiobook to the folder→section map: mapped→audiobooks set 1,
/// mapped→other set 0, unmapped folders left alone (manual marks survive).
fn reconcile_audiobook_flags() {
    let map = load_folder_sections();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        for (folder, key) in map.iter() {
            let _ = tulipix_music::audiobooks::set_folder_flag(&pool, folder, key == "audiobooks").await;
        }
    });
}
```

Call `reconcile_audiobook_flags();` immediately before
`populate_audiobook_cards(&w)` in the `"audiobooks"` view dispatch (main.rs
~1124), and once after the main scan completes in `populate_music_views` (so the
flags are right before the cards query runs). Keep it idempotent — it only
touches folders present in the map.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/tulipix-app/src/main.rs
rtk git commit -m "feat(music): populate audiobook cards/detail + open/back/play/resume + flag sync + reconcile"
```

---

## Task 16: Add-folder section popup (rfd → dialog → confirm)

**Files:**
- Modify: `ui/main.slint` (dialog + props/callbacks)
- Modify: `ui/page_music.slint` + `ui/music_common.slint` (the `+ Add` button path)
- Modify: `crates/tulipix-app/src/main.rs` (callbacks + stored path)

- [ ] **Step 1: Add dialog state + component to `main.slint`**

On MainWindow add:

```slint
in-out property <bool> music-add-section-open: false;
in-out property <string> music-add-section-choice: "mymusic";
callback music-add-folder-pick();
callback music-add-confirm(string);
callback music-add-cancel();
```

Add a top-layer dialog (near the other app-level dialogs) shown when
`music-add-section-open`:

```slint
if root.music-add-section-open: Rectangle {
    background: #000000aa;
    TouchArea { clicked => { root.music-add-cancel(); } }
    Rectangle { width: 320px; height: 360px; border-radius: 16px; background: #1b1b1f; border-width: 1px; border-color: #ffffff1f;
        VerticalLayout { padding: 20px; spacing: 10px;
            Text { text: "Add as…"; color: #ffffff; font-size: 18px; font-weight: 800; }
            Text { text: "Classify this folder into a music section."; color: #ffffff99; font-size: 12px; }
            for opt[i] in [{k: "mymusic", l: "My Music"}, {k: "podcasts", l: "Podcasts"}, {k: "audiobooks", l: "Audiobooks"}, {k: "radio", l: "Radio"}, {k: "youtube", l: "YouTube"}]: Rectangle {
                height: 38px; border-radius: 10px;
                background: root.music-add-section-choice == opt.k ? #3b82f6 : #ffffff10;
                TouchArea { clicked => { root.music-add-section-choice = opt.k; } }
                HorizontalLayout { padding-left: 12px;
                    Text { text: opt.l; color: #ffffff; font-size: 13px; font-weight: 600; vertical-alignment: center; } } }
            Rectangle { vertical-stretch: 1; }
            HorizontalLayout { spacing: 8px;
                Rectangle { height: 38px; horizontal-stretch: 1; border-radius: 10px; background: #ffffff14;
                    TouchArea { clicked => { root.music-add-cancel(); } }
                    Text { text: "Cancel"; color: #ffffff; font-size: 13px; horizontal-alignment: center; vertical-alignment: center; } }
                Rectangle { height: 38px; horizontal-stretch: 1; border-radius: 10px; background: #3b82f6;
                    TouchArea { clicked => { root.music-add-confirm(root.music-add-section-choice); } }
                    Text { text: "Add"; color: #ffffff; font-size: 13px; font-weight: 700; horizontal-alignment: center; vertical-alignment: center; } } }
        } } }
```

- [ ] **Step 2: Route the `+ Add` button through the picker callback**

The music `+ Add` button currently calls `root.add-folder()` for non-podcast
(page_music.slint ~1497, now in the shell header). Change that branch to invoke a
new MusicState callback `music-add-folder-pick()`:
- Add `callback music-add-folder-pick();` to `MusicState` (music_common.slint),
  to `MusicPage` (page_music.slint) with bridge `MusicState.music-add-folder-pick() => { root.music-add-folder-pick(); }`,
  and bind on the MusicPage instance in main.slint: `music-add-folder-pick() => { root.music-add-folder-pick(); }`.
- In the header `+ Add` handler, replace the non-podcast branch
  `root.add-folder()` with `MusicState.music-add-folder-pick()`.

- [ ] **Step 3: Implement the Rust callbacks**

```rust
static ADD_PICK_PATH: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> = std::sync::OnceLock::new();
fn add_pick_path() -> &'static std::sync::Mutex<Option<std::path::PathBuf>> { ADD_PICK_PATH.get_or_init(|| std::sync::Mutex::new(None)) }

let w = window.as_weak();
window.on_music_add_folder_pick(move || {
    let Some(w) = w.upgrade() else { return; };
    let Some(path) = rfd::FileDialog::new().set_title("Add music folder").pick_folder() else { return; };
    let key = load_folder_sections().get(&path.display().to_string()).cloned().unwrap_or_else(|| "mymusic".into());
    if let Ok(mut g) = add_pick_path().lock() { *g = Some(path); }
    w.set_music_add_section_choice(key.into());
    w.set_music_add_section_open(true);
});
let w = window.as_weak();
window.on_music_add_cancel(move || {
    if let Some(w) = w.upgrade() { w.set_music_add_section_open(false); }
    if let Ok(mut g) = add_pick_path().lock() { *g = None; }
});
let w = window.as_weak();
window.on_music_add_confirm(move |key| {
    let Some(w) = w.upgrade() else { return; };
    let Some(path) = add_pick_path().lock().ok().and_then(|mut g| g.take()) else { return; };
    let path_str = path.display().to_string();
    let key = key.to_string();
    set_folder_section(&path_str, &key);
    persist_watched_folder(&path);
    set_scan_silent(false);
    add_folder_path(&w, path);
    let on = key == "audiobooks";
    let folder = path_str.clone();
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("music").await {
            let _ = tulipix_music::audiobooks::set_folder_flag(&pool, &folder, on).await;
        }
    });
    w.set_music_add_section_open(false);
});
```

`set_folder_section` takes `&str` keys; `music_section_key` is not needed here
since the dialog already yields section keys. Confirm `persist_watched_folder` +
`set_scan_silent` are in scope (used by `pick_and_append`).

- [ ] **Step 4: Commit**

```bash
rtk git add ui/main.slint ui/page_music.slint ui/music_common.slint crates/tulipix-app/src/main.rs
rtk git commit -m "feat(music): add-folder section popup wires folder→section + is_audiobook"
```

---

## Task 17: Build, run tests, manual verification

**Files:** none (verify)

- [ ] **Step 1: Run the music crate tests**

Run: `rtk cargo test -p tulipix-music -j 1`
Expected: PASS (new `folder_flag_*`, `book_folders_*`, plus existing
speed/progress/bookmark tests).

- [ ] **Step 2: Capped build of the app**

Use the capped-build incantation (CLAUDE.md memory `capped-build-command`):
`systemd-run --scope -p MemoryMax=3000M ... cargo build -j 1` for the
`tulipix` binary. Expected: compiles clean.

- [ ] **Step 3: Restart the app and verify the feature end-to-end**

- Click `+ Add` on a music section → native folder picker → choose a folder of
  audio → section dialog appears → pick **Audiobooks** → Add.
- The folder scans; switch to **Audiobooks** → a book card appears (cover or 📚
  placeholder), chapter count + total time + resume bar.
- Click the card → hero detail with chapters → click a chapter (plays) → **Resume**
  restores position + speed; speed/chapter/bookmark chips act.
- In Settings → Libraries, reassign the same folder to **My Music** + Save → the
  Audiobooks grid empties; the tracks return to My Music.
- A coverless book shows the 📚 placeholder on both card and hero.

- [ ] **Step 4: Final commit**

```bash
rtk git add -A && rtk git commit -m "chore: audiobook feature + music split complete"
```

---

## Self-review notes (author)

- **Spec coverage:** Part A → Tasks 11, 15(step3), 16(step3). Part B → Tasks
  12,13,14,15. Part C → Task 14 (`BookCover` fallback). Part D → Task 16. Split →
  Tasks 1–10. Reconcile-on-scan (spec Part A third bullet) is partially covered:
  save-section + add-confirm sync flags directly; the post-scan reconcile is
  Task 15 Step 4 (`reconcile_audiobook_flags`).
- **Type consistency:** `BookCard`/`ChapterRow` field names match across
  music_common.slint, main.slint props, and the Rust struct literals.
  `audiobook-chapter-play(int)` reuses `music-audiobook-play`. `id` = folder path
  throughout.
