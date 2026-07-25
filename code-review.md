# Tulipix — full code review

**Reviewed:** `beta` @ `6b65f1d`, v0.7.0 development · 2026-07-25
**Size:** 385 files · 5,602 graph nodes · ~116k lines of Rust + Slint · 24 crates

## Method and limits

This is a static review: the code knowledge graph, targeted searches for known
flaw classes, and direct reading of every site cited below. Runtime numbers in
§10 were measured against the live dev process (PID 620210, 1h uptime) and the
extracted v0.6.0 tarball. Nothing here is inferred from a file's name or a
comment's claim — where a comment asserted a behaviour, the behaviour was
checked, and several turned out to be false (§1, §11).

The dependency tree **is** now audited (§11, all 913 locked packages via OSV).
What this review still does **not** cover: runtime profiling under load,
GPU/render behaviour, fuzzing the feed/metadata parsers, and licence review.
Those are worth doing separately.

Where a count appears below, it is a triaged count, not a grep count — the two
differ by a lot, and the difference is where the actual findings live. `unwrap`
went 1,416 → 196 once test modules were excluded, then → 2 real bugs once
classified. Blocking IO went 351 → 37 once brace-tracked. Advisories went 50 →
effectively 0 once reachability was established. Reporting the raw numbers would
have been alarming and useless.

**Severity:**

| | |
|---|---|
| **P1** | Wrong behaviour, security exposure, or a process gap that lets regressions ship |
| **P2** | Robustness, performance, or a correctness risk that needs specific conditions |
| **P3** | Maintainability — no user-visible symptom today |

---

## 1. Build, CI and release · **weakest area**

### P1 — No CI runs the tests, and there are 1,047 of them · *fixed*

`.github/workflows/` contains exactly one file: `release.yml`. There is no
workflow running `cargo test`, `cargo clippy`, or `cargo fmt --check` on push
or pull request. The test suite is substantial and good (§9), and none of it
gates anything. A regression only surfaces when you notice it by hand.

This is the single highest-value fix in this document, because it changes the
odds on everything else.

```yaml
# .github/workflows/ci.yml
name: ci
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy, rustfmt }
      - uses: Swatinem/rust-cache@v2
      - run: sudo apt-get update && sudo apt-get install -y libmpv-dev
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

**Added** as `.github/workflows/ci.yml`, with two deliberate departures from the
sketch above:

- **No `cargo fmt --check`.** The workspace has 4,494 rustfmt diffs, nearly all
  of them default rustfmt wanting to explode deliberate one-line bodies into
  four. Gating on it means a permanently red job or a whole-workspace reformat —
  neither belongs in a CI file. A tuned `rustfmt.toml` is the way in, and that is
  a style decision to make on purpose.
- **Clippy is scoped, not `-D warnings`.** Clippy has never run here, so blanket
  denial would fail on the first run and train everyone to ignore the job.
  `-D clippy::correctness -D clippy::suspicious` are the groups that find bugs;
  style and pedantic are opinions this codebase has already answered. Widen once
  green.

It also adds a **windows + macos matrix job** that clippies and tests
`tulipix-platform` on those runners. That exists specifically because the shell
escaping in §7 is behind `cfg(target_os)`: a Linux build compiles none of it, so
without this the fixes could regress and nothing would notice.

### P1 — Packaging regressions ship silently · *fixed*

v0.6.0 shipped a tar.gz whose bundled mpv could not start: mpv's RUNPATH is
`$ORIGIN/../lib` (→ `resources/bin/lib`) while packaging put the libraries in
`<root>/lib`. Only `tulipix.sh` bridged it, and the shipped `.desktop` uses
`Exec=tulipix`. Every playback attempt did nothing, with no error surfaced.

Fixed this session (symlink + desktop rewrite + a `tool_bin` probe, §3), but
nothing would have *caught* it. Add a packaging smoke step after the tar.gz
build:

```bash
tar -xzf dist/tulipix-${VERSION}-linux-x86_64.tar.gz -C /tmp
env -u LD_LIBRARY_PATH /tmp/tulipix/resources/bin/linux-x86_64/mpv --version
for t in ffmpeg ffprobe yt-dlp rclone exiftool; do
  /tmp/tulipix/resources/bin/linux-x86_64/$t --version >/dev/null || exit 1
done
```

**Added** to `release.yml` as *Smoke-test the tarball*, run on the artifact that
is about to be published, with `LD_LIBRARY_PATH` unset so a layout mistake cannot
be papered over. The version-flag fallback chain (`-version`, then `--version`)
was checked against the real bundled binaries: ffmpeg and ffprobe take the first,
yt-dlp, rclone and exiftool the second. Thirty seconds of CI that would have
caught a shipped bug.

### P2 — Release notes are hardcoded in the workflow

The v0.6.0 body lives inline in `release.yml`. It has to be hand-edited every
release and silently goes stale — it currently still describes v0.6.0 while the
workspace is on 0.7.0. Move it to `CHANGELOG.md` and have the publish step read
the top section, or generate from the tag range.

### P2 — Installer size

The Linux tar.gz is **402.5 MB**; the AppImage 379 MB. Contents: static ffmpeg
+ ffprobe (152 MB), whisper model (74 MB), rclone (75 MB), yt-dlp (38 MB), mpv
+ libs. The old "< 400 MB" budget row in Advanced was already breached — I
removed the row this session rather than leave a false claim, but the *size*
question stands.

Worth considering: ship the whisper model and rclone as on-demand downloads
(the app already has a resumable, SHA-verified downloader in
`tulipix-ai/src/onboarding_models.rs`). That alone is ~150 MB.

---

## 2. App shell — `tulipix-app`

### P2 — `main()` is 3,421 lines; `main.rs` is 10,071

| Function | Lines |
|---|---|
| `main` | 3,421 |
| `wire_youtube` | 1,284 |
| `wire_music_metadata` | 861 |
| `wire_music_podcasts` | 510 |
| `wire_music_playlist` | 492 |
| `wire_music_p5a` | 476 |
| `wire_music_p6` | 436 |

The `sec-*` crate extraction already established the pattern and worked. The
music wiring (~4,000 lines across six `wire_music_*` functions) is the obvious
next candidate for `tulipix-sec-music`, where most of its callees already live.

This is also a build-cost issue: one 10k-line crate root is a serialisation
point for compile time and the peak-RAM problem that has repeatedly broken CI.

### P2 — Startup blocks on a full filesystem walk

`main.rs:3483` restores watched folders synchronously before `window.run()`,
and each `add_folder_path` → `classify_folder` (`main.rs:5315`) does a complete
recursive `WalkDir`, counting every file by extension, on the main thread.

Measured on this machine: 464 files, **4 ms** — harmless today. On a
50k–100k-file library over a network mount it is seconds of blank window before
first paint. The counts only drive initial row labels; move the walk to a task
and seed the rows when it returns.

### P2 — "Startup to ready" measured uptime *(fixed, uncommitted)*

`startup_ms()` returned `APP_START.elapsed()` evaluated when the settings panel
was built, and `seed_settings_panels` is called from 11 places. There was no
"ready" marker at all, so an hour-old session reported a 3.6-million-ms startup
and lit the warning light permanently. Now frozen via `READY_MS` on the first
turn of the event loop.

---

## 3. Core — `tulipix-core`

### Positive: database configuration is right

`db.rs:55-71` — WAL journal, `synchronous=Normal`, foreign keys on, 5 s busy
timeout, `max_connections(8)`. This is what you want for an embedded
multi-reader workload; no changes suggested.

### The 196 production `unwrap`/`expect` sites, triaged

Counted with `#[cfg(test)]` regions excluded, then classified:

| Kind | Count | Verdict |
|---|---|---|
| Mutex/RwLock guard | 152 | the lock-poisoning class below |
| Needs judgement | 31 | read individually — see below |
| `expect(msg)` | 6 | deliberate, with a message |
| Slint `row_data().unwrap()` | 4 | index derived from `row_count()` |
| `Regex::new(…).unwrap()` | 3 | compile-time constant patterns |

Of the 31, most are safe **by construction**, and it is worth recording why,
because the raw grep suggests otherwise:

- `bpm_from_beats` filters `d > 0.0`, which excludes NaN, before sorting
- `read_u64_sum` uses `chunks_exact(8)`, so `try_into::<[u8;8]>` cannot fail
- `redact_token` guards `split_once('@')` with `contains('@')`
- `onboarding::index` searches `ORDER`, which contains every variant
- the HMAC `expect` is documented: that construction accepts any key length

**Two were real, and are fixed** (see below). This is the useful outcome of the
triage: the count is not the finding, the two sites are.

### P1 — Two NaN sorts could abort the process · *fixed*

`sort_by(|a, b| a.partial_cmp(b).unwrap())` panics when a comparison yields
`None`, and release builds use `panic = "abort"` — so one NaN takes the whole app
down rather than skewing a number.

- `music/embeddings.rs` `similar()` sorts cosine scores. `cosine` guards
  zero-norm vectors, but vectors come back from the DB via `from_bytes`, and
  **every** bit pattern decodes to a valid `f32` — a corrupt row yields NaN.
  Reachable from data at rest.
- `music/dr_meter.rs` `dr_score()` is `pub` and takes an arbitrary `&[f64]`.

Both now use `total_cmp`, a total order that includes NaN: identical results for
real data, no panic. Verified both directions — the `total_cmp` path passes, and
the old form demonstrably panics on the same input. Tests added for each.
`bpm_key` got the same treatment defensively; its filter makes it safe today,
but the safety is a side effect of a filter that could change.

### P2 — 152 `lock().unwrap()` / `read().unwrap()` sites

Release builds set `panic = "abort"`, so a mutex can never actually be poisoned
there — but **dev builds unwind**, and in a dev build one panicking thread turns
every subsequent `lock().unwrap()` into a second panic, hiding the original
fault behind a cascade. Since dev builds are where you debug, this actively
works against you.

Worst concentration: `crates/tulipix-sec-music/src/mdl.rs` (45 sites).

A poison-tolerant helper is a small change with a real debugging payoff:

```rust
/// Lock, recovering from a poisoned mutex. A panic elsewhere should not turn
/// every later access into a second panic that hides the first.
fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
```

**Deliberately not applied**, and this is a judgement call worth stating rather
than burying. The payoff is real but narrow: release builds cannot poison a mutex
at all (`panic = "abort"` never unwinds), so this only improves *dev-build*
debugging. Against that, mechanically rewriting 152 sites across fifteen crates
is a wide diff touching concurrency code, with each site needing the helper in
scope. A change that can only help debugging should not be the change most likely
to introduce a bug. Worth doing incrementally — as files are touched for other
reasons — starting with `sec-music/src/mdl.rs`, which holds 45 of them.

### Fixed this session: `tool_bin` could hand back a binary that cannot start

`tool_bin` checked only `exists()` + ELF magic. A bundled tool with missing
shared libraries passes both, execs successfully, and dies in the dynamic
loader *after* `spawn()` has returned `Ok` — so the caller sees a tool that
launched and vanished, and a working `/usr/bin/mpv` on PATH is never consulted.
That is exactly how v0.6.0 shipped with dead playback. Now probed once per
binary and cached, treating only could-not-execute statuses (127 on unix,
`STATUS_DLL_NOT_FOUND` / `DLL_INIT_FAILED` on Windows) as broken.

### Removed this session: the entire `perf` module was dead

`gpu_budget`, `vrr`, `prefetch`, `io_throttle`, `offthread`, `jank`, `memory` —
679 lines, **zero** references outside themselves. Five rows in Advanced
reported on them as if they were running. Notably `perf::memory` was the RSS
watchdog meant to warn on sustained overshoot; it never ran, which is why
nothing flagged the 1.1 GB heap in §10.

---

## 4. Videos and Stream

### Positive: the Stream HTTP client is the reference implementation

`stream/client.rs:74-80` — 12 s total timeout, 3 s connect timeout, TCP
keepalive, idle pool per host. Every other HTTP call site in the codebase
should look like this. It does not (§5).

### P1 — Subtitle and download fetches have no timeouts

| Site | Call |
|---|---|
| `sec-videos/src/stream/play.rs:220` | `reqwest::Client::new()` — subtitle track fetch |
| `sec-videos/src/stream/play.rs:249` | `reqwest::Client::new()` — sidecar subtitle save |
| `sec-videos/src/stream/download.rs:418` | `reqwest::Client::new()` — the file download itself |

`Client::new()` has **no timeout of any kind**. A host that accepts the
connection and then goes silent hangs the task forever; the download queue has
no way to notice and move on.

One nuance that matters: do **not** put a total `.timeout()` on the download at
line 418 — that would abort legitimate large transfers. The right shape there
is `connect_timeout` plus a read timeout:

```rust
reqwest::Client::builder()
    .connect_timeout(Duration::from_secs(10))
    .read_timeout(Duration::from_secs(30))   // per-read, not whole-body
    .build()
```

For the subtitle fetches (small, bounded) a plain 15 s `.timeout()` is correct.

### P2 — Split-show season grouping is still partial *(partly fixed)*

Grouping is built only while parsing search results, so a show opened from
Bookmarks/History/Continue Watching arrived ungrouped. Fixed this session by
re-searching the base title on open. Remaining hole: when the catalogue leaves
season 1 unsuffixed ("Person of Interest") while suffixing the rest, `parse_search`
pass 3 (`videos/src/stream/mod.rs:313-325`) deliberately drops a lone season, so
that S1 subject stays ungrouped. Consistent with search behaviour, so not a
regression — but it is a real gap if you want it closed.

---

## 5. Music

### P1 — 62 HTTP call sites with no timeout and no connection reuse · *fixed*

`reqwest::Client::new()` appears 62 times across the workspace — 21 in
`main.rs`, 20 in `sec-music/src/lib.rs`, 5 in `mdl.rs`. Each call builds a
*fresh* client, meaning a new connection pool and TLS session every request:
no keepalive, no reuse, and no timeout.

The fix is one shared client per crate, built once:

```rust
fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_default())
}
```

This is both a robustness fix (no unbounded hangs) and a latency win on the
metadata paths, which issue many small requests to the same hosts.

**Done** as `tulipix_core::net`, with **two** clients rather than one, because a
single timeout policy does not fit both shapes:

| | policy | for |
|---|---|---|
| `net::http()` | 20 s total, 8 s connect | API calls, metadata, artwork, subtitles |
| `net::http_stream()` | 10 s connect, 30 s **read**, no total | file transfers |

The split matters: a total timeout on a download would abort a large but
perfectly healthy transfer. What should be bounded there is *silence*, not
length. Three sites needed the streaming client — the podcast episode chunk loop
(`sec-music/src/lib.rs`), the Stream video download (`stream/download.rs`), and
the client handed to the mdl audio engine (`sec-music/src/mdl.rs`). All 61
production sites were rewritten; the one in a `#[cfg(test)]` e2e download was
left alone.

### P3 — `sec-music/src/lib.rs` is 6,490 lines

With the ~4,000 lines of `wire_music_*` still in `main.rs` (§2), music is by
far the largest surface in the app. Worth splitting by concern
(library / playlists / podcasts / radio / downloader) rather than growing.

---

## 6. Books, Photos, Tools, Cloud

### P3 — Large wiring functions

`sec-books/src/lib.rs` 3,897 lines with a 641-line `wire()`;
`sec-tools/src/lib.rs` with a 419-line `wire()`. Same pattern as §2 — these are
long because they are sequences of callback registrations, which is
comprehensible but hostile to review and to compile-time.

### P1 — Four glue crates have zero tests

See §9. `sec-tools`, `sec-photos`, `sec-cloud`, `plugins`, `cli`, `ui`, `hot`
have no tests at all.

---

## 7. Platform integration — **security** · *fixed*

**Status: all five sites fixed this session** (uncommitted). The first pass of
this review found two; auditing every shell invocation in the workspace turned
up three more of the same class, including one on macOS. None affect Linux,
which is why they went unnoticed.

| Site | Flaw |
|---|---|
| `fm.rs` `open_default` | `cmd /C start "" <path>` — path metacharacters |
| `fm.rs` `move_to_trash` (win) | path in a single-quoted PowerShell literal |
| `fm.rs` `move_to_trash` (mac) | path in a double-quoted AppleScript literal |
| `fm.rs` `enumerate_open_with` | `assoc .{ext}` / `ftype {progid}` through cmd |
| `main.rs` `on_open_url` | `cmd /C start "" <url>` — URL metacharacters |

### P1 — PowerShell injection via filename in the recycle-bin path

`crates/tulipix-platform/src/fm.rs:238-242`:

```rust
let ps = format!(
    "…::DeleteFile('{}', 'OnlyErrorDialogs', 'SendToRecycleBin')",
    path.display()
);
Command::new("powershell").args(["-NoProfile", "-Command", &ps]).status()?
```

The path is interpolated into a **single-quoted PowerShell string** with no
escaping. A file named `x'; calc; '.mp3` closes the quote and runs arbitrary
commands. Filenames are attacker-controllable in the ordinary case — a
downloaded torrent, a shared drive, a synced cloud folder.

**Fixed** via `powershell_quote()` — a single-quoted PowerShell string expands
nothing, so the quote is the only metacharacter and doubling it is the complete
escape. `SHFileOperationW` with `FOF_ALLOWUNDO` would remove the shell entirely
and is noted in the code as the upgrade path; it needs the `Win32_UI_Shell`
feature on the `windows` crate (already a dependency on that target) plus
double-NUL buffer handling.

### P1 — AppleScript injection via filename in the macOS trash path

Not in the first pass of this review; found by auditing every shell
invocation. `fm.rs` `move_to_trash` interpolated the path into a
**double-quoted AppleScript literal**, where both `"` and `\` are live. A file
named `x" & (do shell script "…") & "` closes the literal and runs commands.

**Fixed** via `applescript_quote()` — backslashes first, then quotes, or the
escaping escapes its own escapes.

### P1 — `cmd /C start` URL injection

`crates/tulipix-app/src/main.rs:8897`:

```rust
Command::new("cmd").args(["/C", "start", "", &url]).spawn()
```

`cmd.exe` re-parses its arguments, so `&`, `|`, `^` in the URL break out into
new commands. `on_open_url` is a general-purpose callback — the risk depends
entirely on whether any URL reaching it originates from remote data (stream
metadata, podcast feeds, book metadata all plausibly do).

**Fixed** two ways, because either alone is insufficient: the scheme is now
required to be `http`/`https` (without it, a value like `C:\payload.exe` would
be launched as readily as a link), and the launch goes through `explorer.exe`,
which receives the argument via `CreateProcess` with no shell to reinterpret
it. `open_default` had the identical flaw with a *file path* — even more
attacker-controlled — and got the same treatment.

`assoc` / `ftype` are cmd builtins, so cmd cannot be avoided there and no
escaping survives its re-parse; both interpolated values are now required to be
inert tokens (`cmd_token_ok`) instead.

### Tests

The three escaping helpers live outside the `cfg` branches and are **tested on
every platform**, including a payload that demonstrably breaks out unescaped —
an escaping bug is a command-execution bug and should not be discoverable only
on the OS that ships it. `cargo test -p tulipix-platform` covers them.

---

## 8. UI / Slint

### P2 — 3 of the 26 model swaps repeat a crash this codebase already had · *fixed*

All 26 sites audited by what triggers them, because the trigger is what decides
the risk. A model swap tears down every repeater tile; that is harmless for a
page-level load, and dangerous when the swap is fired **by a row**, because it
deletes the tile hosting the control that fired it — slint#6426, "accessing
deleted parent", the crash the books grid hit on move-to-trash.

| Trigger | Sites | Risk |
|---|---|---|
| Page-level seed / populate / refresh | 23 | none — settings panels, timeline, people, things, map, downloader lists |
| **Per-row button** | **3** | `dedupe_keep`, `dedupe_both`, `dedupe_trash` |

The three dedupe row actions each rebuilt the whole `photo_dedupe_groups` model
from the button living inside the row being removed — the same shape as the
books crash, in a section that had not hit it yet only because dedupe sees less
use. **Fixed** with a `replace_rows` helper that mutates the existing `VecModel`
in place and hands the rows back when the property does not hold one yet (first
population). `dedupe_scan` keeps its swap: page-level, and not row-triggered.

### Remaining: in-place updates are the exception, not the rule

`ModelRc::new(VecModel::from(..))` appears 26 times; `set_vec` once. The books
trash crash (slint#6426) was caused precisely by replacing a model while its
rows were still referenced by a live view — the fix there was to mutate in
place with `set_vec` instead of swapping.

Not every swap is a bug: replacing a model for a view that is not currently
mounted is fine. But the 26 sites have not been audited against that rule, and
the crash mode is a hard abort. Worth a pass over the ones that fire while
their page is visible (the Stream downloads/bookmarks lists and the music
views are the likely candidates).

### P3 — Slint file sizes

`page_music.slint` 7,692 lines, `main.slint` 4,588, `page_videos.slint` 3,717.
`page_music` in particular is large enough that the live-preview reparse cost
shows up in dev memory (§10).

---

## 9. Testing

| Crate | Tests | | Crate | Tests |
|---|---|---|---|---|
| tulipix-videos | 227 | | tulipix-platform | 29 |
| tulipix-core | 159 | | tulipix-whisper | 28 |
| tulipix-photos | 138 | | tulipix-books | 28 |
| tulipix-music | 137 | | tulipix-mdl | 22 |
| tulipix-player | 95 | | tulipix-sec-videos | 21 |
| tulipix-tools | 72 | | tulipix-ai | 21 |
| tulipix-cloud | 57 | | tulipix-sec-books | 6 |
| | | | **sec-music / app** | **3 each** |
| | | | **sec-tools, sec-photos, sec-cloud, ui, plugins, hot, cli** | **0** |

### P2 — CI immediately found a test that had been failing · *fixed*

The first green run of the new `test` job failed on
`tulipix-books::tts::tests::char_start_maps_back` — a test in code this review
never touched, failing before any of these changes. Nothing had run it.

`sentences()` returns each sentence's `text` **trimmed** but its `char_start`
taken **before** the trim, so the two fields disagreed: for `"One. Two."` the
second sentence reported index 4 (the space) instead of 5 (the `T`). Read-aloud
highlighting keyed off `char_start` therefore began one character early — and
further off after a paragraph break, where more whitespace is trimmed. The test
was right and the code was wrong; `flush` now advances past the same leading
whitespace `trim` removes. Verified against all three tts tests: the other two
are unaffected.

That is the argument for §1 in one bug: a wrong result, in shipped code, sitting
in a test that already described the correct behaviour.

**1,047 tests total** — and the domain crates are genuinely well covered. The
Stream parsers are a good example: `per_season_subjects_fold_into_one_card`,
`a_lone_season_one_is_not_treated_as_a_split_show`, and the
`split_season_suffix` set pin down real edge cases, and they caught my own
mistake earlier this session.

The gap is structural, not cultural: **pure logic is tested, glue is not.** The
`sec-*` crates hold decision logic worth testing even without a UI — which
caption to pick for a language, which season a subject maps to, how a progress
fraction becomes a label. Extracting those into free functions would make them
testable without touching Slint.

Combined with §1 (nothing runs the tests), the practical coverage today is
"whatever was run by hand most recently".

---

## 10. Performance and memory — measured

Against the live dev process (debug build, `SLINT_LIVE_PREVIEW=1`, 1h uptime):

| | |
|---|---|
| RSS | 304 MB |
| Swapped | 970 MB |
| **Total anonymous** | **~1.27 GB** |
| **mimalloc arenas** | **1.14 GB** (2 × 1 GB reserved; 985 MB + 160 MB touched) |
| Everything else | 28 MB heap · 27 MB Mesa · 20 MB binary |

So it is the app's own heap, not textures or libraries — and the library is 464
files, which cannot need 1.1 GB live. Three contributors, in likely order:

1. **Debug build** — unoptimized, and every allocation path is fatter.
2. **`SLINT_LIVE_PREVIEW=1`** — keeps the Slint compiler and interpreter
   in-process and reparses a very large `.slint` set on every hot-reload.
3. **mimalloc is untuned** — `crates/tulipix-app/Cargo.toml:125` takes the
   crate defaults, which retain freed pages rather than returning them. Pages
   sitting in swap rather than released is exactly that signature.

**Experiments, cheapest first** (each is one restart, no rebuild):

| Test | How |
|---|---|
| Release baseline | extract the tarball, run `./tulipix.sh`, read `/proc/<pid>/smaps_rollup` |
| Live-preview cost | drop `--setenv=SLINT_LIVE_PREVIEW=1` from `run-dev-lite` |
| mimalloc retention | add `--setenv=MIMALLOC_PURGE_DELAY=0` |

### P2 — 37 blocking filesystem calls inside async bodies

The 351 figure quoted earlier was a file-level grep and overstated the problem.
Tracking braces to find calls genuinely inside an `async fn` body gives **37**,
each of which blocks a tokio worker thread while it runs. Concentrated in
`photos/src/export_html.rs` (11 in one function, writing a file per photo),
`core/src/populator.rs`, `core/src/watcher.rs` and
`videos/src/sub_opensubtitles.rs`. Use `tokio::fs`, or wrap the loop in
`spawn_blocking`; the HTML export is the one most likely to stall the runtime
noticeably.

If the third moves the number, the permanent fix is `mi_option_set` at the top
of `main` (needs `libmimalloc-sys` exposed — the `mimalloc` crate alone has no
option setters).

Until the release baseline is measured, **no code change should be made for
memory**: the 300 MB threshold in Advanced was written for a release build, and
optimising against a debug + live-preview number would be optimising noise.

---

## 11. Dependencies — advisory audit

913 locked packages checked against the OSV database (`cargo-audit` is not
installed here; querying OSV directly reads `Cargo.lock` and needs no build).
**50 advisories matched — and the count is close to meaningless without
reachability.** Working out what actually compiles is the whole job:

| Crate | Advisories | Reachable? |
|---|---|---|
| **wasmtime 26.0.1** | **27**, incl. sandbox escapes | **No** — see below |
| quick-xml 0.39.4 | 2 (DoS) | **No** — build-time only |
| quinn-proto 0.11.14 | 2 (remote memory exhaustion) | No — not in the resolved graph |
| memmap2 0.9.10 | 1 (unsound pointer offset) | Yes, via `fontique` (slint fonts) |
| glib 0.18.5 | 2 (unsound iterators) | Yes, via GTK file dialogs |
| crossbeam-epoch, anyhow | 2 (unsoundness) | Only via `Debug`-printing an `Atomic` / a specific downcast misuse |
| gtk/atk/gdk 0.18, paste, encoding, async-std, bincode, rustybuzz, ttf-parser | 11 | **Unmaintained notices, not vulnerabilities** |

**wasmtime is the headline that isn't.** 27 advisories including guest sandbox
escapes, and it is in `Cargo.lock` — but `tulipix-plugins` declares
`default = []` with `wasm = ["dep:wasmtime"]`, and **no crate enables `wasm`**.
It never compiles, not even in the `full,ai-onnx` release build (`cargo tree -i
wasmtime` finds no match at all). So the exposure today is zero — and it becomes
P1 the day someone turns that feature on, because a WASM plugin engine's entire
value is the sandbox those advisories break. Bump it before enabling, not after.

**quick-xml matters less than it looks**, which is the opposite of what I first
assumed. Both advisories are runtime DoS on attacker-supplied XML, and the app
does parse untrusted remote XML (podcast feeds) — but not with this crate. Its
only consumer is `wayland-scanner`, a proc-macro that parses wayland protocol
files at *build* time. Our code never calls `quick_xml`; feeds go through a
hand-rolled scanner in `music/podcasts.rs`.

That scanner deserves a note of its own: it is a `find`/`split` state machine over
untrusted input, which is precisely where panics hide. I traced `parse_opml` — every
index derives from an ASCII match so all slices land on char boundaries, and a
missing `>` ends the loop instead of spinning. It holds up. The rest of the file
is the natural fuzz target if you ever want one.

**Worth acting on:** nothing urgent. The reachable items are unsoundness in
third-party code with no known exploit path here. The unmaintained gtk-rs 0.18
stack is a slow-moving liability rather than a bug — it arrives through `rfd`
file dialogs.

## 12. Honesty of the Advanced panel *(fixed this session)*

Nine rows asserted behaviour the app did not have — five pointing at the dead
`perf` module, plus "Localisation: Fluent .ftl · en" (no `fluent` dependency
and no `.ftl` file exists), "RTL layout: Auto-mirror per locale" (no
layout-direction handling), "cargo-bloat CI gate" (nothing in `.github/`), and
an installer budget already breached. All removed.

The general lesson is worth keeping: a diagnostics panel that prints static
claims is worse than a short one, because it is indistinguishable from a panel
that measures. Every remaining row is now a real measurement, a real probe, or
a real setting.

---

## Prioritised plan

| # | Change | Section | Effort | Why now |
|---|---|---|---|---|
| ~~1~~ | ~~Add `ci.yml`~~ — **done** | §1 | — | test + scoped clippy + per-OS matrix |
| ~~2~~ | ~~Fix the shell injections~~ — **done** | §7 | — | 5 sites fixed + tested |
| ~~3~~ | ~~Packaging smoke test~~ — **done** | §1 | — | verified against the real bundled tools |
| ~~4~~ | ~~Shared HTTP client + timeouts~~ — **done** | §4, §5 | — | 61 sites, two timeout policies |
| ~~5~~ | ~~Audit the 26 model swaps~~ — **done** | §8 | — | 3 row-triggered sites fixed |
| ~~6~~ | ~~Dependency advisory audit~~ — **done** | §11 | — | 913 packages; no reachable exploit |
| ~~7~~ | ~~Triage the 196 `unwrap`s~~ — **done** | §3 | — | 2 NaN aborts found and fixed |
| 8 | Measure release RSS, then decide | §10 | 15 min | Prevents optimising a debug-only number |
| 9 | `tokio::fs` for the 37 blocking calls | §10 | 2–3 h | Each one stalls a runtime worker |
| 10 | Move `classify_folder` off the main thread | §2 | 1–2 h | Startup scales with library size |
| 11 | Carve `wire_music_*` out of `main.rs` | §2, §5 | 1–2 d | Build RAM, compile time, reviewability |
| 12 | On-demand whisper model / rclone | §1 | 1 d | ~150 MB off the installer |
| 13 | Poison-tolerant lock helper, incrementally | §3 | — | Dev-build ergonomics only; see the note |
| 14 | Bump wasmtime *before* enabling `wasm` | §11 | 1 h | 27 advisories go live with that feature |

Items 1–4 are done. Each closed a path by which a defect had *already* reached a
release: untested code, an unescaped shell, an unverified package, an unbounded
request.

The remaining items are ordered by value, not by ease. Item 5 is deliberately
first: measuring before optimising is the difference between fixing memory and
guessing at it.

---

## Fixed during this review

Uncommitted in the working tree:

- Five shell-injection sites — Windows and macOS — plus tests (§7)
- Two NaN sorts that could abort the process, plus tests (§3)
- `char_start` disagreeing with `text` in the sentence splitter (§9)
- Three row-triggered model swaps repeating a known crash shape (§8)
- CI: test, scoped lints, per-OS jobs; tarball smoke test (§1)
- Shared HTTP clients with timeouts, replacing 61 untimed ones (§5)
- Startup metric measured uptime, not startup (§2)
- `tool_bin` handed callers binaries that cannot start (§3) — the v0.6.0 dead-playback root cause
- Tarball lib layout + `.desktop` pointing past the launcher (§1)
- Dead `perf` module removed, 679 lines (§3)
- Nine false Advanced rows removed (§11)
- Stream season grouping outside search (§4)
