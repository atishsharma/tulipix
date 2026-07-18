//! Books section wiring extracted from tulipix-app: the Book Home page —
//! Continue Reading hero, the 2×2 stats strip, the filter chips (file types /
//! genres / quick filters), sort + search, and the paginated library grid.
//! Every `window.on_books_*` callback is registered here. tulipix-app calls
//! [`wire`] once at startup.

mod hero_art;

use slint::{ComponentHandle, Model, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tulipix_books::annotations::{self, Annotation};
use tulipix_books::home::HomeData;
use tulipix_books::library::{self, BookRow, Filter, Sort};
use tulipix_books::paginate::{self, Layout, Page};
use tulipix_books::progress;
use tulipix_books::render;
use tulipix_books::toc::TocEntry;
use tulipix_common::pool_for;
use tulipix_ui::*;

/// Books per grid page (full-bleed 3×2 library grid / 1×6 list — must match
/// the `cols`/`rows` split in page_books.slint).
const PAGE_SIZE: usize = 6;

/// One-shot guard for the background 3D-cover pre-bake sweep.
static PREBAKE_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Live reader session, owned by the Slint event-loop thread. The async book
/// loader hands its result in via [`slint::Weak::upgrade_in_event_loop`], and
/// every reader callback runs on the same thread — so a thread-local `RefCell`
/// is all the synchronisation we need (no `Send` bound to fight).
#[derive(Default)]
struct ReaderState {
    book_id: i64,
    /// Raw chapter texts, kept so the Aa popover can re-paginate in place
    /// (no disk re-read) when font size changes.
    chapters: Vec<String>,
    /// Per-chapter heading (index-aligned with `chapters`), shown centered on
    /// the chapter's first page.
    chapter_titles: Vec<String>,
    layout: Layout,
    pages: Vec<Page>,
    toc: Vec<TocEntry>,
    /// Page-anchored notes for the current book, mirrored from the DB on load
    /// so the annotate panel can jump by char offset (repagination-safe).
    annots: Vec<Annotation>,
    /// Fixed-page (PDF/CBZ/CBR) mode: pages are rasterised images, not text.
    /// `pos` is the left page index; the text fields above stay empty.
    is_image: bool,
    img_path: std::path::PathBuf,
    img_format: String,
    img_total: usize,
    /// 0-based index of the left page of the current spread (even-aligned).
    pos: usize,
    /// Wall-clock start of the current reading burst, folded into `time_read`
    /// on each progress save.
    session_start: Option<std::time::Instant>,
}

/// Reader typography/appearance preferences — session-global so re-opening a
/// book keeps your settings. ponytail: not persisted to disk; add a books
/// setting file if it needs to survive restarts.
#[derive(Clone, Copy)]
struct ReaderPrefs {
    font_px: f32,
    line_idx: u8,  // 0 compact · 1 normal · 2 relaxed
    margin_idx: u8, // 0 narrow · 1 normal · 2 wide
    typeface: u8,  // 0 serif · 1 sans · 2 mono
    bold: bool,
    theme: u8,     // 0 light · 1 sepia · 2 dark · 3 night
    brightness: f32,
}

impl Default for ReaderPrefs {
    fn default() -> Self {
        Self { font_px: 17.0, line_idx: 1, margin_idx: 1, typeface: 0, bold: false, theme: 0, brightness: 1.0 }
    }
}

thread_local! {
    static READER: RefCell<ReaderState> = RefCell::new(ReaderState::default());
    static PREFS: Cell<ReaderPrefs> = const { Cell::new(ReaderPrefs {
        font_px: 17.0, line_idx: 1, margin_idx: 1, typeface: 0, bold: false, theme: 0, brightness: 1.0,
    }) };
    // Single-page view: nav steps by 1 and doesn't even-align spreads.
    static SINGLE: Cell<bool> = const { Cell::new(false) };
    // Real rendered page size (px), reported by the Slint PaperPage so pagination
    // matches the on-screen column. (0,0) until first report → Layout defaults.
    static PAGE_GEOM: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
    // Read-aloud (TTS) state — Phase 1 timer-paced highlight; Phase 2 syncs to
    // Kokoro audio. Lives on the event-loop thread (all TTS callbacks + the
    // slint::Timer fire there).
    static TTS: RefCell<TtsState> = RefCell::new(TtsState::default());
}

#[derive(Default)]
struct TtsState {
    sentences: Vec<String>,
    /// Screen-page index of each sentence (paginate space), so the reader spread
    /// follows along as the highlight advances. Index-aligned with `sentences`.
    sent_pages: Vec<usize>,
    active: usize,
    playing: bool,
    speed_idx: u8,
    voice: String,
    /// True when real audio (neural or robotic) drives playback; false = timer-
    /// paced highlight. Decided at `tts_start` from settings + availability.
    audio: bool,
    /// When `audio`: true = Kokoro neural, false = espeak robotic.
    neural: bool,
    /// Bumped on every (re)start/seek so stale scheduled timers self-cancel.
    generation: u64,
}

const TTS_SPEEDS: [f32; 4] = [0.75, 1.0, 1.25, 1.5];
/// Kokoro voices (label → id used by the Phase-2 synth).
const TTS_VOICES: [(&str, &str); 4] = [
    ("Heart (F)", "af_heart"),
    ("Bella (F)", "af_bella"),
    ("Michael (M)", "am_michael"),
    ("Adam (M)", "am_adam"),
];

/// Socket of the read-aloud mpv currently playing a sentence. The synth worker
/// (a blocking thread) stores it after spawning mpv; the event loop reads it to
/// send `quit` so a pause/seek/stop takes effect mid-sentence instead of at the
/// next sentence boundary. Cross-thread, so it can't live in the thread-local
/// `TTS` state.
static TTS_SOCK: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> =
    std::sync::OnceLock::new();
fn tts_sock() -> &'static std::sync::Mutex<Option<std::path::PathBuf>> {
    TTS_SOCK.get_or_init(|| std::sync::Mutex::new(None))
}

/// Quit the current read-aloud mpv over IPC — graceful (WirePlumber-safe:
/// SIGKILL-ing a PipeWire client mid link-activation wedges the session), so
/// pause/seek land instantly. No-op if nothing is playing.
fn tts_stop_audio() {
    let sock = tts_sock().lock().ok().and_then(|mut g| g.take());
    if let Some(s) = sock {
        use std::io::Write;
        if let Ok(mut c) = tulipix_common::mpv_ipc::connect(&s) {
            let _ = c.write_all(b"{\"command\":[\"quit\"]}\n");
        }
        tulipix_common::mpv_ipc::cleanup(&s);
    }
}

/// Kokoro voice id for a UI label (defaults to the first voice).
fn voice_id(label: &str) -> &'static str {
    TTS_VOICES.iter().find(|(l, _)| *l == label).map(|(_, id)| *id).unwrap_or(TTS_VOICES[0].1)
}

/// Pick the read-aloud engine for `voice_label` → (uses_audio, is_neural).
/// Honors the `books.tts.neural` setting, falling back to robotic espeak, then
/// to the timer-paced highlight when nothing is available.
fn decide_engine(voice_label: &str) -> (bool, bool) {
    let prefer_neural = tulipix_core::settings::Settings::load()
        .map(|s| s.flag("books.tts.neural", true))
        .unwrap_or(true);
    let neural_ok = prefer_neural && tulipix_books::kokoro::available(voice_id(voice_label));
    if neural_ok {
        (true, true)
    } else if tulipix_books::speech::espeak_available() {
        (true, false)
    } else {
        (false, false)
    }
}

/// Font-size bounds for the Aa stepper (px).
const FONT_MIN: f32 = 13.0;
const FONT_MAX: f32 = 26.0;
const FONT_STEP: f32 = 2.0;
const LINE_HEIGHTS: [f32; 3] = [1.30, 1.55, 1.90];
const MARGINS: [f32; 3] = [56.0, 96.0, 150.0];
const THEMES: [&str; 4] = ["light", "sepia", "dark", "night"];

/// UI font-family + pagination glyph-width for a typeface index.
fn typeface_family(t: u8) -> &'static str {
    match t {
        1 => "sans-serif",
        2 => "monospace",
        _ => "serif",
    }
}
fn typeface_glyph(t: u8) -> f32 {
    match t {
        2 => 0.60, // monospace is wider
        _ => 0.50,
    }
}

/// Build the pagination [`Layout`] from the current prefs. When a real page
/// size is known (`PAGE_GEOM`), use it so pagination matches the rendered
/// column instead of the 480×640 default (fixes text clipping / gaps).
fn layout_from(p: &ReaderPrefs) -> Layout {
    // Bold widens glyphs ~8%, so fewer chars fit per line — reflect that in the
    // width estimate or bold body text clips.
    let glyph = typeface_glyph(p.typeface) * if p.bold { 1.08 } else { 1.0 };
    let (w, h) = PAGE_GEOM.with(|c| c.get());
    let mut l = Layout {
        font_px: p.font_px,
        line_height: LINE_HEIGHTS[p.line_idx as usize % 3],
        pad_x_px: MARGINS[p.margin_idx as usize % 3],
        glyph_em: glyph,
        ..Layout::default()
    };
    if w > 1.0 && h > 1.0 {
        l.page_w_px = w;
        l.page_h_px = h;
        // PaperPage chrome: 40 top + 20 bottom + 24 folio ≈ 84px; small safety.
        l.pad_y_px = 92.0;
    }
    l
}

/// Push every appearance pref to the reader window props (visual + sheet state).
fn push_prefs(w: &MainWindow, p: &ReaderPrefs) {
    w.set_books_reader_font_px(p.font_px);
    w.set_books_reader_font_family(typeface_family(p.typeface).into());
    w.set_books_reader_bold(p.bold);
    w.set_books_reader_line_index(p.line_idx as i32);
    w.set_books_reader_margin_index(p.margin_idx as i32);
    w.set_books_reader_typeface(p.typeface as i32);
    w.set_books_reader_brightness(p.brightness);
    w.set_books_reader_theme(THEMES[p.theme as usize % 4].into());
}

/// Re-paginate the current book from the current prefs, keeping the reading
/// position (by char offset), then render the spread. No-op for image books.
fn reader_repaginate(w: &MainWindow, weak: slint::Weak<MainWindow>) {
    let prefs = PREFS.with(|c| c.get());
    let np = READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_image || r.chapters.is_empty() {
            return None;
        }
        let off = r.pages.get(r.pos).map(|p| p.char_start).unwrap_or(0);
        r.layout = layout_from(&prefs);
        r.pages = paginate::paginate(&r.chapters, r.layout);
        Some(paginate::page_at_offset(&r.pages, off))
    });
    if let Some(page) = np {
        nav_to(w, weak, page);
    }
}

/// Register every books callback on `window` and do the initial home refresh.
pub fn wire(window: &MainWindow) {
    // Sort index is not a UI property (it flows through set-sort by label), so
    // hold it here and rebuild the [`Filter`] from window props on each refresh.
    let sort_idx = Rc::new(Cell::new(tulipix_books::prefs::load_sort()));

    // Decode the embedded hero-art pack once (light + dark variants).
    load_hero_art();

    // Restore the persisted grid/list view preference + sort order.
    window.set_books_view_mode(tulipix_books::prefs::load_view_mode().into());
    window.set_books_sort_index(sort_idx.get() as i32);

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_search_changed(move |q| {
        if let Some(w) = w.upgrade() {
            w.set_books_search(q);
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_file_type(move |k| {
        // Accepts the rail key ("epub") or a dropdown label ("EPUB"/"All").
        let k = k.to_lowercase();
        if let Some(w) = w.upgrade() {
            let cur = w.get_books_active_file_type().to_string();
            // "all" clears; clicking the active format toggles it off.
            let next = if k == "all" || cur == k { String::new() } else { k };
            w.set_books_active_file_type(next.into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_quick_filter(move |k| {
        if let Some(w) = w.upgrade() {
            // Tabs are mutually exclusive: a status tab clears series/collection/author.
            w.set_books_active_quick(k);
            w.set_books_active_series("".into());
            w.set_books_active_collection(0);
            w.set_books_active_author("".into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_genre_filter(move |k| {
        if let Some(w) = w.upgrade() {
            let cur = w.get_books_active_genre().to_string();
            let next = if cur == k.as_str() { String::new() } else { k.to_string() };
            w.set_books_active_genre(next.into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    // Author page: filter the library to one author (empty string clears).
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_open_author(move |author| {
        if let Some(w) = w.upgrade() {
            w.set_books_active_author(author);
            w.set_books_active_quick("all".into());
            w.set_books_active_series("".into());
            w.set_books_active_collection(0);
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    // Series filter (toggle off by clicking the active one).
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_series_filter(move |name| {
        if let Some(w) = w.upgrade() {
            let cur = w.get_books_active_series().to_string();
            let next = if cur == name.as_str() { String::new() } else { name.to_string() };
            w.set_books_active_series(next.into());
            // Exclusive: clear status/collection/author so a series stands alone.
            w.set_books_active_quick("all".into());
            w.set_books_active_collection(0);
            w.set_books_active_author("".into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    // Collection filter (0 clears).
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_collection_filter(move |id| {
        if let Some(w) = w.upgrade() {
            let cur = w.get_books_active_collection();
            w.set_books_active_collection(if cur == id { 0 } else { id });
            // Exclusive: clear status/series/author.
            w.set_books_active_quick("all".into());
            w.set_books_active_series("".into());
            w.set_books_active_author("".into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_collection_create(move |name| {
        collection_create(w.clone(), name.to_string(), s.get());
    });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_collection_delete(move |id| {
        collection_delete(w.clone(), id as i64, s.get());
    });
    let w = window.as_weak();
    window.on_books_collection_toggle(move |book_id, cid| {
        collection_toggle(w.clone(), book_id as i64, cid as i64);
    });

    // Content (full-text) search toggle.
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_search_contents(move |on| {
        if let Some(w) = w.upgrade() {
            w.set_books_search_contents(on);
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    // Reading-stats panel.
    let w = window.as_weak();
    window.on_books_open_stats(move || { books_open_stats(w.clone()); });

    // Local star rating.
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_rating(move |id, rating| {
        books_set_rating(w.clone(), id as i64, rating as f64, s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_sort(move |label| {
        let idx = sort_index_of(&label);
        s.set(idx);
        tulipix_books::prefs::save_sort(idx);
        if let Some(w) = w.upgrade() { w.set_books_page(1); w.set_books_sort_index(idx as i32); }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    window.on_books_set_view_mode(move |m| {
        if let Some(w) = w.upgrade() { w.set_books_view_mode(m.clone()); }
        tulipix_books::prefs::save_view_mode(m.as_str());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_page(move |p| {
        if let Some(w) = w.upgrade() { w.set_books_page(p.max(1)); }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_stat_card(move |id| {
        // Stat cards act as quick jumps: finished / reading filters.
        if let Some(w) = w.upgrade() {
            let quick = match id.as_str() {
                "finished" => "finished",
                "reading" => "reading",
                _ => "all",
            };
            w.set_books_active_quick(quick.into());
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_toggle_favorite(move |id| {
        books_toggle_favorite(w.clone(), id as i64, s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_card_menu(move |id, action| {
        books_card_menu(w.clone(), id as i64, action.to_string(), s.get());
    });

    // Context-menu: open the file's folder, move to Trash, restore, purge.
    let w = window.as_weak();
    window.on_books_open_location(move |id| { books_open_location(w.clone(), id as i64); });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_trash(move |id| { books_trash(w.clone(), id as i64, s.get()); });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_restore(move |id| { books_restore(w.clone(), id as i64, s.get()); });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_delete_perm(move |id| { books_delete_perm(w.clone(), id as i64, s.get()); });

    // Book detail popup (R1): show metadata, fetch/cache online summary.
    let w = window.as_weak();
    window.on_books_show_detail(move |id| { books_show_detail(w.clone(), id as i64); });
    let w = window.as_weak();
    window.on_books_fetch_summary(move |id| { books_fetch_summary(w.clone(), id as i64); });

    // ── Reader (Phase 3) ─────────────────────────────────────────────────
    // Open a book from a grid tile.
    let w = window.as_weak();
    window.on_books_open_details(move |id| { books_open(w.clone(), id as i64); });
    // Hero "Continue reading" resumes the active slider slide.
    let w = window.as_weak();
    window.on_books_resume(move || {
        let id = SLIDER.with(|s| s.borrow().get(SLIDER_IX.with(|c| c.get())).map(|h| h.id as i64));
        match id {
            Some(id) => books_open(w.clone(), id),
            None => books_resume(w.clone()),
        }
    });
    let w = window.as_weak();
    window.on_books_hero_details(move || { books_resume(w.clone()); });
    // Hero slider: step ±1, wrapping, swapping the active slide (no DB hit).
    let w = window.as_weak();
    window.on_books_hero_slide(move |delta| {
        let Some(w) = w.upgrade() else { return };
        SLIDER.with(|s| {
            let s = s.borrow();
            let n = s.len();
            if n == 0 { return; }
            let cur = SLIDER_IX.with(|c| c.get());
            let ni = (cur as i64 + delta as i64).rem_euclid(n as i64) as usize;
            SLIDER_IX.with(|c| c.set(ni));
            w.set_books_hero(s[ni].clone());
            w.set_books_hero_index(ni as i32);
        });
    });

    // Reader chrome: back saves progress + refreshes the home hero.
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_reader_back(move || {
        persist_progress();
        tts_stop(w.clone());
        if let Some(w) = w.upgrade() { w.set_books_reader_open(false); w.set_books_reader_tts_open(false); }
        books_refresh(w.clone(), s.get());
    });

    // Page/spread navigation.
    let w = window.as_weak();
    window.on_books_reader_next_spread(move || {
        if let Some(w) = w.upgrade() {
            let cur = READER.with(|r| r.borrow().pos);
            nav_to(&w, w.as_weak(), cur + 2);
        }
    });
    let w = window.as_weak();
    window.on_books_reader_prev_spread(move || {
        if let Some(w) = w.upgrade() {
            let cur = READER.with(|r| r.borrow().pos);
            nav_to(&w, w.as_weak(), cur.saturating_sub(2));
        }
    });
    let w = window.as_weak();
    window.on_books_reader_scrub(move |screen_page| {
        if let Some(w) = w.upgrade() {
            nav_to(&w, w.as_weak(), (screen_page.max(1) - 1) as usize);
        }
    });
    // Single-page steps (±1) + view-mode sync (turns off spread even-alignment).
    let w = window.as_weak();
    window.on_books_reader_prev_page(move || {
        if let Some(w) = w.upgrade() {
            let cur = READER.with(|r| r.borrow().pos);
            nav_to(&w, w.as_weak(), cur.saturating_sub(1));
        }
    });
    let w = window.as_weak();
    window.on_books_reader_next_page(move || {
        if let Some(w) = w.upgrade() {
            let cur = READER.with(|r| r.borrow().pos);
            nav_to(&w, w.as_weak(), cur + 1);
        }
    });
    window.on_books_reader_set_single(move |s| { SINGLE.with(|c| c.set(s)); });

    // Real page geometry from the Slint PaperPage → drives pagination so text
    // fills the actual column (no clipping / gaps). Repaginate only on a material
    // size change to avoid thrash during a resize drag.
    let w = window.as_weak();
    window.on_books_reader_report_geom(move |pw, ph| {
        if pw < 60.0 || ph < 60.0 {
            return;
        }
        let (ow, oh) = PAGE_GEOM.with(|c| c.get());
        if (pw - ow).abs() < 4.0 && (ph - oh).abs() < 4.0 {
            return;
        }
        PAGE_GEOM.with(|c| c.set((pw, ph)));
        if let Some(w) = w.upgrade() {
            reader_repaginate(&w, w.as_weak());
        }
    });

    // Read-aloud (TTS).
    let w = window.as_weak();
    window.on_books_reader_tts_start(move || { tts_start(w.clone()); });
    let w = window.as_weak();
    window.on_books_reader_tts_stop(move || { tts_stop(w.clone()); });
    let w = window.as_weak();
    window.on_books_reader_tts_play_pause(move || { tts_play_pause(w.clone()); });
    let w = window.as_weak();
    window.on_books_reader_tts_set_speed(move |i| { tts_set_speed(w.clone(), i); });
    window.on_books_reader_tts_set_voice(move |v| { tts_set_voice(v.to_string()); });
    let w = window.as_weak();
    window.on_books_reader_tts_jump(move |i| { tts_jump(w.clone(), i); });

    // Define-a-word popup (R2): fetch definition/translation/wiki off-thread.
    let w = window.as_weak();
    window.on_books_reader_define_run(move |word, lang| {
        define_run(w.clone(), word.to_string(), lang.to_string());
    });
    window.on_books_open_url(move |url| { open_url(&url); });
    let w = window.as_weak();
    window.on_books_reader_next_chapter(move || {
        if let Some(w) = w.upgrade() { nav_chapter(&w, w.as_weak(), 1); }
    });
    let w = window.as_weak();
    window.on_books_reader_prev_chapter(move || {
        if let Some(w) = w.upgrade() { nav_chapter(&w, w.as_weak(), -1); }
    });
    let w = window.as_weak();
    window.on_books_reader_jump_chapter(move |chapter| {
        if let Some(w) = w.upgrade() {
            let np = READER.with(|r| paginate::page_of_chapter(&r.borrow().pages, chapter.max(0) as usize));
            nav_to(&w, w.as_weak(), np);
        }
    });

    // Bookmark toggle for the current page.
    let w = window.as_weak();
    window.on_books_reader_toggle_bookmark(move || { reader_toggle_bookmark(w.clone()); });

    // Sun button cycles the paper theme (light → sepia → dark → night).
    let w = window.as_weak();
    window.on_books_reader_brightness_cycle(move || {
        if let Some(w) = w.upgrade() {
            let p = PREFS.with(|c| {
                let mut p = c.get();
                p.theme = (p.theme + 1) % 4;
                c.set(p);
                p
            });
            w.set_books_reader_theme(THEMES[p.theme as usize].into());
        }
    });

    // Overlay-opening chrome (annotations / search / typography / more) lands
    // with its panels in a later pass; keep it harmless for now.
    // Notes (annotate): load on panel open, add on current page, edit, delete.
    let w = window.as_weak();
    window.on_books_reader_annotate(move || { reader_load_notes(w.clone()); });
    let w = window.as_weak();
    window.on_books_reader_annot_add(move || { reader_annot_add(w.clone()); });
    let w = window.as_weak();
    window.on_books_reader_annot_jump(move |id| {
        if let Some(w) = w.upgrade() {
            let np = READER.with(|r| {
                let r = r.borrow();
                r.annots
                    .iter()
                    .find(|a| a.id == id as i64)
                    .map(|a| paginate::page_at_offset(&r.pages, a.start_off as usize))
            });
            if let Some(p) = np { nav_to(&w, w.as_weak(), p); }
        }
    });
    let w = window.as_weak();
    window.on_books_reader_annot_set_note(move |id, note| {
        reader_annot_set_note(w.clone(), id as i64, note.to_string());
    });
    let w = window.as_weak();
    window.on_books_reader_annot_remove(move |id| { reader_annot_remove(w.clone(), id as i64); });
    let w = window.as_weak();
    window.on_books_reader_annot_export(move || { reader_annot_export(w.clone()); });
    // In-book search: scan the paginated pages (in memory) for the query.
    let w = window.as_weak();
    window.on_books_reader_search_run(move |q| { reader_search(w.clone(), q.to_string()); });
    // Aa reading-settings sheet.
    let w = window.as_weak();
    window.on_books_reader_font_dec(move || { reader_step_font(w.clone(), -FONT_STEP); });
    let w = window.as_weak();
    window.on_books_reader_font_inc(move || { reader_step_font(w.clone(), FONT_STEP); });
    let w = window.as_weak();
    window.on_books_reader_set_line_spacing(move |i| {
        PREFS.with(|c| { let mut p = c.get(); p.line_idx = i.clamp(0, 2) as u8; c.set(p); });
        if let Some(w) = w.upgrade() { push_prefs(&w, &PREFS.with(|c| c.get())); reader_repaginate(&w, w.as_weak()); }
    });
    let w = window.as_weak();
    window.on_books_reader_set_margin(move |i| {
        PREFS.with(|c| { let mut p = c.get(); p.margin_idx = i.clamp(0, 2) as u8; c.set(p); });
        if let Some(w) = w.upgrade() { push_prefs(&w, &PREFS.with(|c| c.get())); reader_repaginate(&w, w.as_weak()); }
    });
    let w = window.as_weak();
    window.on_books_reader_set_typeface(move |i| {
        PREFS.with(|c| { let mut p = c.get(); p.typeface = i.clamp(0, 2) as u8; c.set(p); });
        if let Some(w) = w.upgrade() { push_prefs(&w, &PREFS.with(|c| c.get())); reader_repaginate(&w, w.as_weak()); }
    });
    let w = window.as_weak();
    window.on_books_reader_toggle_bold(move || {
        PREFS.with(|c| { let mut p = c.get(); p.bold = !p.bold; c.set(p); });
        if let Some(w) = w.upgrade() { push_prefs(&w, &PREFS.with(|c| c.get())); }
    });
    let w = window.as_weak();
    window.on_books_reader_set_theme(move |t| {
        let idx = THEMES.iter().position(|x| *x == t.as_str()).unwrap_or(0) as u8;
        PREFS.with(|c| { let mut p = c.get(); p.theme = idx; c.set(p); });
        if let Some(w) = w.upgrade() { w.set_books_reader_theme(t); }
    });
    let w = window.as_weak();
    window.on_books_reader_set_brightness(move |v| {
        PREFS.with(|c| { let mut p = c.get(); p.brightness = v.clamp(0.4, 1.0); c.set(p); });
        if let Some(w) = w.upgrade() { w.set_books_reader_brightness(v.clamp(0.4, 1.0)); }
    });
    // ••• menu: mark the book finished, or remove it from the library.
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_reader_more_mark_finished(move || { reader_mark_finished(w.clone(), s.get()); });
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_reader_more_remove(move || { reader_remove_book(w.clone(), s.get()); });

    let w = window.as_weak();
    window.on_books_add(move || { books_add_folder(w.clone()); });

    // Initial paint + background metadata backfill (publication dates etc.).
    books_refresh(window.as_weak(), 0);
    books_backfill_metadata(window.as_weak());
}

/// Map a sort label from the dropdown back to a [`Sort`] index.
fn sort_index_of(label: &str) -> usize {
    match label {
        "Recently Read" => 1,
        "Title" => 2,
        "Author" => 3,
        "Rating" => 4,
        _ => 0,
    }
}

/// Build a [`Filter`] from the current window filter/search props + sort index.
fn filter_from(w: &MainWindow, sort_idx: usize) -> Filter {
    let quick = w.get_books_active_quick().to_string();
    let status = match quick.as_str() {
        "reading" | "finished" | "unread" | "favorites" | "missing" | "trashed" => quick,
        _ => String::new(), // "all"
    };
    Filter {
        format: w.get_books_active_file_type().to_string(),
        status,
        author: w.get_books_active_author().to_string(),
        genre: w.get_books_active_genre().to_string(),
        series: w.get_books_active_series().to_string(),
        collection: w.get_books_active_collection() as i64,
        query: w.get_books_search().to_string(),
        ids: Vec::new(), // filled by books_refresh for content search
        sort: Sort::from_index(sort_idx),
    }
}

/// Reload the whole Book Home page: hero + stats from `home::load`, the grid
/// from `library::list` (filtered + paginated), and the chip rows.
fn books_refresh(weak: slint::Weak<MainWindow>, sort_idx: usize) {
    let (mut filter, content) = match weak.upgrade() {
        Some(w) => (filter_from(&w, sort_idx), w.get_books_search_contents()),
        None => return,
    };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let pool = match pool_for("books").await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("books: pool_for failed: {e:#}");
                return;
            }
        };
        // Content search: resolve the query to matching book ids via FTS. A ran
        // search with no hits becomes the `-1` sentinel → empty grid.
        if content && !filter.query.trim().is_empty() {
            let mut ids = library::fts_search(&pool, &filter.query).await.unwrap_or_default();
            if ids.is_empty() {
                ids.push(-1);
            }
            filter.ids = ids;
            filter.query = String::new(); // ids already narrow it; skip LIKE
        }
        // One-shot background sweep: pre-bake 3D renditions for every existing
        // cover off-thread, then refresh once so tiles swap in. New books bake
        // at scan time — page flips themselves never bake.
        if !PREBAKE_DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let weak2 = weak.clone();
            let pool2 = pool.clone();
            tokio::spawn(async move {
                let paths: Vec<String> =
                    library::cover_paths(&pool2).await.unwrap_or_default();
                let weak_bl = weak2.clone();
                let baked_new = tokio::task::spawn_blocking(move || {
                    let mut any = false;
                    let mut since_refresh = 0u32;
                    for p in paths {
                        let cp = std::path::Path::new(&p);
                        if !cp.exists() {
                            continue;
                        }
                        for suffix in ["_book", "_hero"] {
                            if !tulipix_books::covers::baked_path(cp, suffix).exists() {
                                let _ = if suffix == "_hero" {
                                    tulipix_books::covers::bake_hero(cp)
                                } else {
                                    tulipix_books::covers::bake_book(cp)
                                };
                                any = true;
                                since_refresh += 1;
                            }
                        }
                        // Progressive: swap freshly-baked covers in every ~2
                        // pages instead of only when the whole sweep finishes.
                        if since_refresh >= 12 {
                            since_refresh = 0;
                            let w3 = weak_bl.clone();
                            let w4 = w3.clone();
                            let _ = w3.upgrade_in_event_loop(move |_w| {
                                books_refresh(w4, sort_idx);
                            });
                        }
                    }
                    any
                })
                .await
                .unwrap_or(false);
                if baked_new {
                    let weak3 = weak2.clone();
                    let _ = weak2.upgrade_in_event_loop(move |_w| {
                        books_refresh(weak3, sort_idx);
                    });
                }
            });
        }
        let home = tulipix_books::home::load(&pool).await.unwrap_or_default();
        let streak = current_streak(&library::reading_days(&pool).await.unwrap_or_default());
        let rows = library::list(&pool, &filter).await.unwrap_or_default();
        let formats = library::format_counts(&pool).await.unwrap_or_default();
        let genres = library::genre_counts(&pool).await.unwrap_or_default();
        let series = library::series_counts(&pool).await.unwrap_or_default();
        let colls = library::collections(&pool).await.unwrap_or_default();
        let trashed = library::trashed_count(&pool).await.unwrap_or(0);
        let _ = weak.upgrade_in_event_loop(move |w| {
            apply_home(&w, &home, streak);
            apply_grid(&w, &rows);
            apply_chips(&w, &formats, &genres, &home);
            apply_series_collections(&w, &series, &colls);
            w.set_books_trashed_count(trashed as i32);
        });
    });
}

/// Push the Series + Collections rail models.
fn apply_series_collections(
    w: &MainWindow,
    series: &[(String, i64)],
    colls: &[(i64, String, i64)],
) {
    let s: Vec<BookChip> = series
        .iter()
        .take(12)
        .map(|(name, n)| BookChip { key: name.clone().into(), label: name.clone().into(), count: *n as i32 })
        .collect();
    w.set_books_series(VecModel::from_slice(&s));
    let c: Vec<CollChip> = colls
        .iter()
        .map(|(id, name, n)| CollChip { id: *id as i32, name: name.clone().into(), count: *n as i32 })
        .collect();
    w.set_books_collections(VecModel::from_slice(&c));
}

/// Clamp a string to `max` characters, appending "…" when cut.
fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.truncate(out.trim_end().len());
        out.push('…');
        out
    }
}

/// Current reading streak: consecutive days ending today (or yesterday).
fn current_streak(read_days: &[i64]) -> i64 {
    let today = tulipix_books::schema::now() / 86400;
    let day_set: std::collections::HashSet<i64> = read_days.iter().copied().collect();
    let mut streak = 0i64;
    let mut d = if day_set.contains(&today) { today } else { today - 1 };
    while day_set.contains(&d) {
        streak += 1;
        d -= 1;
    }
    streak
}

/// Push hero + the 2×3 stats strip.
fn apply_home(w: &MainWindow, home: &HomeData, streak: i64) {
    // Build the hero-slider slides from the up-to-5 in-progress books.
    let arts = HERO_ART.with(|a| a.borrow().clone());
    let slides: Vec<BooksHero> = home
        .slider
        .iter()
        .enumerate()
        .map(|(i, (b, page, total))| {
            // Art pool: index 0 = the nook illustration (first slide), the
            // rest pick pseudo-randomly (stable per book) from the pack.
            let ix = if i == 0 || arts.len() <= 1 { 0 } else { 1 + (b.id as usize % (arts.len() - 1)) };
            let art = arts
                .get(ix)
                .cloned()
                .unwrap_or_else(|| (slint::Image::default(), slint::Image::default()));
            hero_from(b, *page, *total, art)
        })
        .collect();
    SLIDER.with(|s| *s.borrow_mut() = slides.clone());
    SLIDER_IX.with(|c| c.set(0));
    w.set_books_hero(slides.first().cloned().unwrap_or_default());
    w.set_books_hero_count(slides.len() as i32);
    w.set_books_hero_index(0);

    // Stat identities (icons/labels/colors) live in the .slint; only the four
    // values flow through, in order: Total Books, Authors, In Progress, Completed.
    let s = &home.stats;
    let stat = |n: i64, sub: String| BookStat {
        value: n.to_string().into(),
        sub: sub.into(),
        ..Default::default()
    };
    let month = |n: i64| if n > 0 { format!("+{n} this month") } else { String::new() };
    let text = |v: String, sub: String| BookStat {
        value: v.into(),
        sub: sub.into(),
        ..Default::default()
    };
    // Order matches the .slint 2×3 grid: total, authors, reading, finished,
    // time-read, streak (last two open the reading-stats panel).
    let stats: Vec<BookStat> = vec![
        stat(s.total, month(s.added_month)),
        stat(s.authors, month(s.authors_month)),
        stat(s.in_progress, format!("{} book{}", s.in_progress, if s.in_progress == 1 { "" } else { "s" })),
        stat(s.finished, "All time".into()),
        text(format!("{:.1}h", s.hours_read), "All time".into()),
        text(format!("{streak}d"), "in a row".into()),
    ];
    w.set_books_stats(VecModel::from_slice(&stats));
    w.set_books_count(home.stats.total as i32);
}

/// Format a unix timestamp (secs) as "Sep 12, 2016" — civil date from days,
/// no chrono dependency. Empty for non-positive input.
fn fmt_date(secs: i64) -> slint::SharedString {
    if secs <= 0 {
        return "".into();
    }
    let z = secs / 86400 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!("{} {}, {}", MON[(m - 1).clamp(0, 11) as usize], d, y).into()
}

/// Push the filtered library grid (paginated) + book / page counts.
fn apply_grid(w: &MainWindow, rows: &[BookRow]) {
    let total = rows.len();
    w.set_books_filtered_count(total as i32);
    let page_count = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = (w.get_books_page().max(1) as usize).min(page_count);
    w.set_books_page(page as i32);
    w.set_books_page_count(page_count as i32);

    let start = (page - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(total);
    let tiles: Vec<BookTile> = rows[start..end]
        .iter()
        .map(|b| BookTile {
            id: b.id as i32,
            title: clamp_chars(&b.title, 60).into(),
            author: clamp_chars(&b.author, 40).into(),
            author_full: b.author.clone().into(),
            cover: load_cover(&b.cover_path),
            book: load_baked(&b.cover_path, false),
            percent: b.percent as f32,
            favorite: b.favorite != 0,
            format: b.format.to_uppercase().into(),
            // Card date = fetched publication date (empty → tile shows "—").
            date: b.published.clone().into(),
            hue: cover_hue(&b.title),
            rating: b.rating as f32,
            net_rating: b.net_rating as f32,
            missing: b.missing != 0,
            trashed: b.trashed != 0,
        })
        .collect();
    // Update the existing VecModel in place (set_vec) instead of swapping in a
    // fresh model: the repeater then updates rows without tearing every tile
    // down. A full model swap deletes the GridTile that hosts a just-used ⋮
    // PopupWindow and trips slint#6426 ("accessing deleted parent") — the
    // move-to-trash crash.
    use slint::Model as _;
    let model = w.get_books_tiles();
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<BookTile>>() {
        vm.set_vec(tiles);
    } else {
        w.set_books_tiles(VecModel::from_slice(&tiles));
    }
}

/// Push the file-type / genre / quick-filter chip rows (with counts).
fn apply_chips(
    w: &MainWindow,
    formats: &[(String, i64)],
    genres: &[(String, i64)],
    home: &HomeData,
) {
    let file_types: Vec<BookChip> = formats
        .iter()
        .map(|(f, n)| BookChip { key: f.clone().into(), label: f.to_uppercase().into(), count: *n as i32 })
        .collect();
    w.set_books_file_types(VecModel::from_slice(&file_types));

    // Dropdown model: "All" + each format label (uppercase).
    let mut opts: Vec<slint::SharedString> = vec!["All".into()];
    opts.extend(formats.iter().map(|(f, _)| f.to_uppercase().into()));
    w.set_books_file_type_options(VecModel::from_slice(&opts));

    // Top genres only for the rail (long tail hides behind "View all").
    let genre_chips: Vec<BookChip> = genres
        .iter()
        .take(8)
        .map(|(g, n)| BookChip { key: g.clone().into(), label: g.clone().into(), count: *n as i32 })
        .collect();
    w.set_books_genres(VecModel::from_slice(&genre_chips));

    let quick: Vec<BookChip> = vec![
        BookChip { key: "all".into(), label: "All".into(), count: home.stats.total as i32 },
        BookChip { key: "reading".into(), label: "Reading now".into(), count: home.stats.in_progress as i32 },
        BookChip { key: "unread".into(), label: "Unread".into(), count: -1 },
        BookChip { key: "finished".into(), label: "Finished".into(), count: home.stats.finished as i32 },
        BookChip { key: "favorites".into(), label: "Favorites".into(), count: -1 },
        BookChip { key: "missing".into(), label: "Missing".into(), count: -1 },
    ];
    w.set_books_quick_filters(VecModel::from_slice(&quick));
}

/// Patch one visible tile in place (event-loop only) — instant UI feedback
/// before the DB write + full refresh land.
fn tile_patch(weak: &slint::Weak<MainWindow>, id: i64, f: impl Fn(&mut BookTile)) {
    let Some(w) = weak.upgrade() else { return };
    use slint::Model as _;
    let model = w.get_books_tiles();
    for i in 0..model.row_count() {
        if let Some(mut t) = model.row_data(i) {
            if t.id as i64 == id {
                f(&mut t);
                model.set_row_data(i, t);
                break;
            }
        }
    }
}

/// Set a book's local star rating, then refresh (keeps rating-sort order live).
fn books_set_rating(weak: slint::Weak<MainWindow>, id: i64, rating: f64, sort_idx: usize) {
    tile_patch(&weak, id, |t| t.rating = rating as f32);
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::set_rating(&pool, id, rating).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Human-readable byte size ("1.4 MB").
fn fmt_size(bytes: i64) -> String {
    if bytes <= 0 {
        return String::new();
    }
    let b = bytes as f64;
    const KB: f64 = 1024.0;
    if b < KB { format!("{bytes} B") }
    else if b < KB * KB { format!("{:.0} KB", b / KB) }
    else if b < KB * KB * KB { format!("{:.1} MB", b / (KB * KB)) }
    else { format!("{:.1} GB", b / (KB * KB * KB)) }
}

/// Populate + open the detail popup for a book (R1). Loads the row, pushes
/// metadata + any cached summary.
fn books_show_detail(weak: slint::Weak<MainWindow>, id: i64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let Ok(Some(b)) = library::get(&pool, id).await else { return };
        let colls = library::collections(&pool).await.unwrap_or_default();
        let mine = library::book_collections(&pool, id).await.unwrap_or_default();
        let need_fetch = b.summary_fetched_at == 0;
        let _ = weak.upgrade_in_event_loop(move |w| {
            push_detail_collections(&w, &colls, &mine);
            w.set_books_detail_id(b.id as i32);
            w.set_books_detail_title(b.title.clone().into());
            w.set_books_detail_author(b.author.clone().into());
            w.set_books_detail_cover(load_cover(&b.cover_path));
            w.set_books_detail_hue(cover_hue(&b.title));
            w.set_books_detail_format(b.format.to_uppercase().into());
            w.set_books_detail_genre(b.genre.clone().into());
            w.set_books_detail_series(b.series.clone().into());
            w.set_books_detail_size(fmt_size(b.size_bytes).into());
            w.set_books_detail_added(fmt_date(b.added_at));
            w.set_books_detail_published(b.published.clone().into());
            w.set_books_detail_rating(b.rating as f32);
            w.set_books_detail_net_rating(b.net_rating as f32);
            w.set_books_detail_percent(b.percent as f32);
            w.set_books_detail_summary(b.summary.clone().into());
            w.set_books_detail_summary_loading(false);
            w.set_books_detail_open(true);
        });
        // Lazily fetch metadata the first time this book's detail opens.
        if need_fetch {
            books_fetch_summary(weak, id);
        }
    });
}

/// Fetch online metadata (summary · publication date · avg rating) for the
/// detail popup across the 5-source chain, cache it in the DB, and show it.
/// Falls back to the local file's publication date when the web has none.
fn books_fetch_summary(weak: slint::Weak<MainWindow>, id: i64) {
    if let Some(w) = weak.upgrade() {
        if w.get_books_detail_id() as i64 == id {
            w.set_books_detail_summary_loading(true);
        }
    }
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let Ok(Some(b)) = library::get(&pool, id).await else { return };
        let mut m = tulipix_books::summary::fetch_meta(&b.title, &b.author).await;
        // Fall back to the file's own publication date (epub dc:date / pdf).
        if m.published.is_empty() {
            m.published = tulipix_books::metadata::extract(std::path::Path::new(&b.path), &b.format).published;
        }
        let _ = library::set_metadata(&pool, id, &m.summary, &m.published, m.rating).await;
        let summary = if m.summary.is_empty() { "No summary found online.".to_string() } else { m.summary };
        let published = m.published.clone();
        let net = m.rating as f32;
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Only update if the popup is still on this book.
            if w.get_books_detail_id() as i64 == id {
                w.set_books_detail_summary(summary.into());
                w.set_books_detail_published(published.into());
                w.set_books_detail_net_rating(net);
                w.set_books_detail_summary_loading(false);
            }
        });
        // Refresh the grid so the tile's publication-date pill updates too.
        books_refresh(weak, 0);
    });
}

/// Single-flight guard so overlapping triggers (startup + post-scan) don't run
/// concurrent backfill loops.
static BACKFILL_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Background: fetch online metadata for every book that has never been fetched,
/// in small throttled batches, refreshing the grid so publication-date pills
/// fill in progressively. Idempotent (each book is stamped after one attempt).
fn books_backfill_metadata(weak: slint::Weak<MainWindow>) {
    use std::sync::atomic::Ordering::SeqCst;
    if BACKFILL_RUNNING.swap(true, SeqCst) {
        return; // already draining the queue
    }
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            loop {
                let batch = library::needs_metadata(&pool, 8).await.unwrap_or_default();
                if batch.is_empty() {
                    break;
                }
                for (id, title, author) in batch {
                    let mut m = tulipix_books::summary::fetch_meta(&title, &author).await;
                    if m.published.is_empty() {
                        if let Ok(Some(b)) = library::get(&pool, id).await {
                            m.published = tulipix_books::metadata::extract(
                                std::path::Path::new(&b.path),
                                &b.format,
                            )
                            .published;
                        }
                    }
                    let _ = library::set_metadata(&pool, id, &m.summary, &m.published, m.rating).await;
                    // Be polite to the public APIs.
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                books_refresh(weak.clone(), 0);
            }
        }
        BACKFILL_RUNNING.store(false, SeqCst);
    });
}

// ── collections ───────────────────────────────────────────────────────────────

fn collection_create(weak: slint::Weak<MainWindow>, name: String, sort_idx: usize) {
    if name.trim().is_empty() {
        return;
    }
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::collection_create(&pool, &name).await;
        }
        books_refresh(weak, sort_idx);
    });
}

fn collection_delete(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::collection_delete(&pool, id).await;
        }
        // Clear the filter if it pointed at the deleted collection.
        let _ = weak.upgrade_in_event_loop(move |w| {
            if w.get_books_active_collection() as i64 == id {
                w.set_books_active_collection(0);
            }
        });
        books_refresh(weak, sort_idx);
    });
}

/// Add/remove the current detail book to/from a collection, then refresh the
/// popup's membership + the rail counts.
fn collection_toggle(weak: slint::Weak<MainWindow>, book_id: i64, cid: i64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let members = library::book_collections(&pool, book_id).await.unwrap_or_default();
        if members.contains(&cid) {
            let _ = library::collection_remove(&pool, cid, book_id).await;
        } else {
            let _ = library::collection_add(&pool, cid, book_id).await;
        }
        let colls = library::collections(&pool).await.unwrap_or_default();
        let mine = library::book_collections(&pool, book_id).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            push_detail_collections(&w, &colls, &mine);
        });
    });
}

/// Push the detail popup's collection rows (all collections + membership).
fn push_detail_collections(w: &MainWindow, colls: &[(i64, String, i64)], member_ids: &[i64]) {
    let rows: Vec<CollRow> = colls
        .iter()
        .map(|(id, name, _)| CollRow {
            id: *id as i32,
            name: name.clone().into(),
            member: member_ids.contains(id),
        })
        .collect();
    w.set_books_detail_collections(VecModel::from_slice(&rows));
}

// ── reading stats ─────────────────────────────────────────────────────────────

/// Load reading stats (totals + streak + 12-week heatmap + per-book time) and
/// open the stats panel.
fn books_open_stats(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let (secs, days, finished, started) =
            library::reading_totals(&pool).await.unwrap_or((0, 0, 0, 0));
        let read_days = library::reading_days(&pool).await.unwrap_or_default();
        let per_book = library::time_per_book(&pool, 5).await.unwrap_or_default();
        let today = tulipix_books::schema::now() / 86400;
        let day_set: std::collections::HashSet<i64> = read_days.iter().copied().collect();
        let streak = current_streak(&read_days);
        // 12-week heatmap (84 days, oldest→today).
        let heat: Vec<bool> = (0..84).rev().map(|i| day_set.contains(&(today - i))).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_books_stats_hours(((secs as f32) / 3600.0 * 10.0).round() / 10.0);
            w.set_books_stats_days(days as i32);
            w.set_books_stats_finished(finished as i32);
            w.set_books_stats_started(started as i32);
            w.set_books_stats_streak(streak as i32);
            let heat_model: Vec<bool> = heat;
            w.set_books_stats_heat(VecModel::from_slice(&heat_model));
            let rows: Vec<StatBookRow> = per_book
                .iter()
                .map(|(title, s)| StatBookRow {
                    title: clamp_chars(title, 70).into(),
                    time: fmt_duration(*s).into(),
                })
                .collect();
            w.set_books_stats_books(VecModel::from_slice(&rows));
            w.set_books_stats_open(true);
        });
    });
}

/// "3h 12m" / "45m" / "2m" from seconds.
fn fmt_duration(secs: i64) -> String {
    let m = secs / 60;
    if m >= 60 {
        format!("{}h {}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}

// ── notes export ──────────────────────────────────────────────────────────────

/// Export the current book's notes to a Markdown file (save dialog).
fn reader_annot_export(weak: slint::Weak<MainWindow>) {
    let (book_id, title, author) = match weak.upgrade() {
        Some(w) => (
            READER.with(|r| r.borrow().book_id),
            w.get_books_reader_title().to_string(),
            w.get_books_reader_author().to_string(),
        ),
        None => return,
    };
    if book_id == 0 {
        return;
    }
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let notes = annotations::for_book(&pool, book_id).await.unwrap_or_default();
        let mut md = format!("# Notes — {title}\n");
        if !author.is_empty() {
            md.push_str(&format!("*{author}*\n"));
        }
        md.push('\n');
        for n in &notes {
            md.push_str(&format!("## p{}\n", n.page));
            if !n.snippet.trim().is_empty() {
                md.push_str(&format!("> {}\n\n", n.snippet.trim()));
            }
            if !n.note.trim().is_empty() {
                md.push_str(&format!("{}\n\n", n.note.trim()));
            }
        }
        let default_name = format!("{}-notes.md", title.replace(['/', '\\'], "-"));
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_file_name(&default_name)
            .add_filter("Markdown", &["md"])
            .save_file()
            .await
        else {
            return;
        };
        let _ = std::fs::write(file.path(), md);
    });
}

/// Toggle a book's favorite flag, then refresh the grid.
fn books_toggle_favorite(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
    tile_patch(&weak, id, |t| t.favorite = !t.favorite);
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::toggle_favorite(&pool, id).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Card overflow-menu actions (favorite / remove for now).
fn books_card_menu(weak: slint::Weak<MainWindow>, id: i64, action: String, sort_idx: usize) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Let the source popup finish tearing down before rows change (slint#6426).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pool) = pool_for("books").await {
            match action.as_str() {
                "favorite" => { let _ = library::toggle_favorite(&pool, id).await; }
                // "Remove" everywhere = soft delete: flag → Trash tab, file kept.
                "remove" => { let _ = library::trash(&pool, id).await; }
                _ => {}
            }
        }
        books_refresh(weak, sort_idx);
    });
}

/// Open the folder holding a book's file in the OS file manager.
fn books_open_location(weak: slint::Weak<MainWindow>, id: i64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let Ok(Some(b)) = library::get(&pool, id).await else { return };
        let path = std::path::PathBuf::from(&b.path);
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or(path);
        let _ = weak; // no UI update needed
        open_url(&dir.to_string_lossy());
    });
}

/// Soft-delete: flag the row trashed — the book leaves the library grid and
/// shows in the Trash tab. Never touches the file on disk.
fn books_trash(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
    tokio::runtime::Handle::current().spawn(async move {
        // Let the ⋮ popup finish tearing down before rows change (slint#6426).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pool) = pool_for("books").await {
            let _ = library::trash(&pool, id).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Restore a trashed book: clear the trashed flag (no file to move back).
fn books_restore(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
    tokio::runtime::Handle::current().spawn(async move {
        // Let the ⋮ popup finish tearing down before rows change (slint#6426).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pool) = pool_for("books").await {
            let _ = library::restore(&pool, id).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Permanently delete a trashed book: remove its file + purge the DB row.
fn books_delete_perm(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
    tokio::runtime::Handle::current().spawn(async move {
        // Let the ⋮ popup finish tearing down before rows change (slint#6426).
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Ok(pool) = pool_for("books").await {
            if let Ok(Some(b)) = library::get(&pool, id).await {
                let _ = std::fs::remove_file(&b.path);
            }
            let _ = library::remove(&pool, id).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// One-row books scan-progress model for the shared ScanProgressCard.
fn books_scan_row(label: &str, total: i32, added: i32, active: bool) -> ScanProgress {
    ScanProgress {
        section: "books".into(),
        label: label.into(),
        total,
        added,
        failed: 0,
        active,
        last_error: "".into(),
    }
}

/// "Add books" → folder picker → scan into the library, showing the shared
/// scan-progress popup (same dialog other media get when adding a folder).
fn books_add_folder(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else { return };
        let path = dir.path().to_string_lossy().to_string();
        // Show the loading popup (indeterminate until the walk yields a total).
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_scan_progress(VecModel::from_slice(&[books_scan_row("Scanning…", 0, 0, true)]));
            w.set_scan_active(true);
        });
        let mut added = 0i32;
        if let Ok(pool) = pool_for("books").await {
            let _ = tulipix_books::scan::add_folder(&pool, &path).await;
            // Stream per-file progress into the shared popup (bar + current title),
            // throttled to ~1% steps so big libraries don't flood the event loop.
            let weak_cb = weak.clone();
            let report = tulipix_books::scan::scan_all_progress(&pool, move |total, done, title| {
                let step = (total / 100).max(1);
                if done % step == 0 || done == total {
                    let title = title.to_string();
                    let _ = weak_cb.upgrade_in_event_loop(move |w| {
                        w.set_scan_progress(VecModel::from_slice(&[books_scan_row(
                            &title, total as i32, done as i32, true,
                        )]));
                        w.set_scan_active(true);
                    });
                }
            })
            .await;
            added = report.map(|r| r.added as i32).unwrap_or(0);
            books_refresh(weak.clone(), 0);
            // Fetch online metadata for the newly-added books in the background.
            books_backfill_metadata(weak.clone());
            // Index contents for library-wide search in the background (slow for
            // big libraries; resumable so a partial run picks up next time).
            let _ = tulipix_books::scan::index_contents(&pool).await;
        }
        // Mark done; keep the popup up briefly with the count, then auto-hide.
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_scan_progress(VecModel::from_slice(&[books_scan_row(
                "Done", added.max(1), added, false,
            )]));
            w.set_scan_active(false);
            let weak2 = w.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(4), move || {
                if let Some(w) = weak2.upgrade() {
                    let empty: &[ScanProgress] = &[];
                    w.set_scan_progress(VecModel::from_slice(empty));
                    w.set_scan_active(false);
                }
            });
        });
        books_refresh(weak, 0);
    });
}

/// Deterministic cover color for a book without artwork — the .slint draws a
/// generated title/author card on this hue (see `Cover`).
fn cover_hue(title: &str) -> slint::Color {
    const HUES: [(u8, u8, u8); 10] = [
        (108, 77, 246), (224, 81, 143), (47, 191, 113), (245, 166, 35), (58, 134, 255),
        (239, 71, 111), (17, 138, 178), (131, 56, 236), (255, 107, 107), (32, 201, 151),
    ];
    let h = title.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = HUES[h as usize % HUES.len()];
    slint::Color::from_rgb_u8(r, g, b)
}

thread_local! {
    /// Decoded cover cache, keyed by path. Every `books_refresh` rebuilds the
    /// whole tile model; without this each rebuild re-decodes every visible
    /// cover (flicker + wasted work when you just rated/favorited a book).
    static COVER_CACHE: RefCell<std::collections::HashMap<String, slint::Image>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Decode a cover file into a slint image (empty → default placeholder),
/// memoised by path so repeated grid rebuilds don't re-decode.
fn load_cover(path: &str) -> slint::Image {
    if path.is_empty() {
        return slint::Image::default();
    }
    if let Some(img) = COVER_CACHE.with(|c| c.borrow().get(path).cloned()) {
        return img;
    }
    let img = slint::Image::load_from_path(std::path::Path::new(path)).unwrap_or_default();
    COVER_CACHE.with(|c| c.borrow_mut().insert(path.to_string(), img.clone()));
    img
}

/// Cover art baked onto a book mockup (grid hardcover or hero flat-book).
/// Read-only on the event loop: serves the disk cache when the background
/// pre-bake (scan-time or the one-shot sweep in `books_refresh`) has produced
/// it; empty image otherwise (UI falls back to frame + flat overlay). Misses
/// are NOT memoised so tiles pick the bake up as soon as it lands.
fn load_baked(path: &str, hero: bool) -> slint::Image {
    if path.is_empty() {
        return slint::Image::default();
    }
    let key = format!("{path}#{}", if hero { "hero" } else { "book" });
    if let Some(img) = COVER_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return img;
    }
    let suffix = if hero { "_hero" } else { "_book" };
    let baked = tulipix_books::covers::baked_path(std::path::Path::new(path), suffix);
    if !baked.exists() {
        return slint::Image::default();
    }
    let img = slint::Image::load_from_path(&baked).unwrap_or_default();
    COVER_CACHE.with(|c| c.borrow_mut().insert(key, img.clone()));
    img
}
thread_local! {
    /// The hero slider's built slides (up to 5 in-progress books) + which is
    /// active, so `hero-slide` can swap `books-hero` with no DB round-trip.
    static SLIDER: RefCell<Vec<BooksHero>> = const { RefCell::new(Vec::new()) };
    static SLIDER_IX: Cell<usize> = const { Cell::new(0) };
    /// Decorative hero illustrations `(light, dark)` per slide, rotated by index.
    /// Filled once from the embedded base64 pack; slider renders fine if empty.
    static HERO_ART: RefCell<Vec<(slint::Image, slint::Image)>> = const { RefCell::new(Vec::new()) };
}

/// Decode the embedded base64 PNG hero-art pack into `(light, dark)` images.
fn load_hero_art() {
    let to_img = |b64: &str| -> slint::Image {
        let Some((rgba, w, h)) = tulipix_books::decode_png_b64(b64) else {
            return slint::Image::default();
        };
        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
        for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
            let o = i * 4;
            *p = slint::Rgba8Pixel { r: rgba[o], g: rgba[o + 1], b: rgba[o + 2], a: rgba[o + 3] };
        }
        slint::Image::from_rgba8(buf)
    };
    let arts: Vec<(slint::Image, slint::Image)> = hero_art::HERO_ART_LIGHT
        .iter()
        .zip(hero_art::HERO_ART_DARK.iter())
        .map(|(l, d)| (to_img(l), to_img(d)))
        .collect();
    HERO_ART.with(|a| *a.borrow_mut() = arts);
}

/// Build one hero-slider slide from a book row + saved position. `percent` is
/// 0‥100 so the card's `%` label reads true (the bar divides by 100).
fn hero_from(
    b: &BookRow,
    page: i64,
    total: i64,
    art: (slint::Image, slint::Image),
) -> BooksHero {
    let frac = if total > 0 { page as f32 / total as f32 } else { b.percent as f32 };
    BooksHero {
        id: b.id as i32,
        present: true,
        title: b.title.clone().into(),
        author: b.author.clone().into(),
        cover: load_cover(&b.cover_path),
        book: load_baked(&b.cover_path, true),
        hue: cover_hue(&b.title),
        page: page as i32,
        total: total as i32,
        percent: (frac * 100.0).clamp(0.0, 100.0),
        time_left: time_left_label(page, total),
        art_light: art.0,
        art_dark: art.1,
    }
}

// ── Reader (Phase 3) ─────────────────────────────────────────────────────────

/// Open a book in the reader: parse + paginate off-thread, restore the saved
/// position, then render the first spread.
fn books_open(weak: slint::Weak<MainWindow>, id: i64) {
    // PREFS is thread-local to the event-loop thread — read here (callbacks fire
    // on this thread), then build the layout off-thread.
    let prefs = PREFS.with(|c| c.get());
    let layout = layout_from(&prefs);
    if let Some(w) = weak.upgrade() {
        push_prefs(&w, &prefs);
        w.set_books_reader_open(true);
        w.set_books_reader_loading(true);
        w.set_books_reader_error("".into());
    }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let pool = match pool_for("books").await {
            Ok(p) => p,
            Err(e) => return reader_fail(weak, format!("{e:#}")),
        };
        let Ok(Some(book)) = library::get(&pool, id).await else {
            return reader_fail(weak, "Book not found".into());
        };
        let prog = progress::get(&pool, id).await.ok().flatten().unwrap_or_default();

        // Fixed-page formats (PDF/CBZ/CBR) render as page images, not reflowed
        // text — take the image path entirely.
        if matches!(book.format.as_str(), "pdf" | "cbz" | "cbr") {
            return open_image_book(weak, book, prog.page.max(0) as usize).await;
        }

        let (path, format) = (std::path::PathBuf::from(&book.path), book.format.clone());
        // Parse + layout are CPU-bound; keep them off the async pool.
        let (chapters, titles, pages, toc, start) = tokio::task::spawn_blocking(move || {
            let ct = tulipix_books::book_chapters(&path, &format);
            let titles: Vec<String> = ct.iter().map(|(t, _)| t.clone()).collect();
            let chapters: Vec<String> = ct.into_iter().map(|(_, x)| x).collect();
            let pages = paginate::paginate(&chapters, layout);
            let toc = tulipix_books::toc::load(&path).unwrap_or_default();
            let start = paginate::page_at_offset(&pages, prog.char_offset as usize);
            (chapters, titles, pages, toc, start)
        })
        .await
        .unwrap_or_default();
        if pages.is_empty() {
            return reader_fail(weak, format!("No readable text in this {} file", book.format));
        }
        let full_text = chapters.join("\n\n");
        // Index this book's contents for library-wide search (cheap — text in hand).
        let _ = library::fts_index(&pool, id, &full_text).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            READER.with(|r| {
                *r.borrow_mut() = ReaderState {
                    book_id: id,
                    chapters,
                    chapter_titles: titles,
                    layout,
                    pages,
                    toc,
                    annots: Vec::new(),
                    pos: 0,
                    session_start: Some(std::time::Instant::now()),
                    ..Default::default()
                };
            });
            w.set_books_reader_title(book.title.clone().into());
            w.set_books_reader_author(book.author.clone().into());
            w.set_books_reader_full_text(full_text.into());
            w.set_books_reader_image_mode(false);
            w.set_books_reader_search_query("".into());
            w.set_books_reader_search_results(VecModel::from_slice(&[]));
            w.set_books_reader_notes(VecModel::from_slice(&[]));
            w.set_books_reader_toc(Rc::new(VecModel::<TocRow>::default()).into());
            w.set_books_reader_loading(false);
            nav_to(&w, w.as_weak(), start);
            // Load notes up front so the per-page note indicator is accurate.
            reader_load_notes(w.as_weak());
        });
    });
}

/// Open a fixed-page book (PDF/CBZ/CBR): count pages off-thread, seed the image
/// reader state, then render the first spread. Already inside the async task.
async fn open_image_book(weak: slint::Weak<MainWindow>, book: BookRow, start_page: usize) {
    let path = std::path::PathBuf::from(&book.path);
    let format = book.format.clone();
    let (path, format, total) = tokio::task::spawn_blocking(move || {
        // Bound the rendered-page cache before adding more pages to it.
        render::prune_page_cache(512 * 1024 * 1024);
        let total = render::page_count(&path, &format).unwrap_or(0);
        (path, format, total)
    })
    .await
    .unwrap_or((std::path::PathBuf::new(), String::new(), 0));
    if total == 0 {
        return reader_fail(weak, format!("Could not open this {} file", book.format));
    }
    let start = start_page.min(total.saturating_sub(1));
    let _ = weak.upgrade_in_event_loop(move |w| {
        READER.with(|r| {
            *r.borrow_mut() = ReaderState {
                book_id: book.id,
                is_image: true,
                img_path: path,
                img_format: format,
                img_total: total,
                pos: 0,
                session_start: Some(std::time::Instant::now()),
                ..Default::default()
            };
        });
        w.set_books_reader_title(book.title.clone().into());
        w.set_books_reader_author(book.author.clone().into());
        w.set_books_reader_full_text("".into());
        w.set_books_reader_image_mode(true);
        w.set_books_reader_search_query("".into());
        w.set_books_reader_search_results(VecModel::from_slice(&[]));
        w.set_books_reader_notes(VecModel::from_slice(&[]));
        w.set_books_reader_toc(Rc::new(VecModel::<TocRow>::default()).into());
        w.set_books_reader_loading(false);
        nav_to(&w, w.as_weak(), start);
    });
}

/// Resume the hero's in-progress book (Home "Continue reading").
fn books_resume(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let Ok(home) = tulipix_books::home::load(&pool).await else { return };
        let Some((book, _, _)) = home.continue_reading else { return };
        let _ = weak.upgrade_in_event_loop(move |w| books_open(w.as_weak(), book.id));
    });
}

/// Surface a reader load failure on the overlay.
fn reader_fail(weak: slint::Weak<MainWindow>, msg: String) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_books_reader_loading(false);
        w.set_books_reader_error(msg.into());
    });
}

/// Render the image spread whose left page is `page` (even-aligned): update
/// folio/percent, rasterise + load the two page images off-thread, save the
/// page position. Fixed-page (PDF/CBZ/CBR) counterpart to [`nav_to`].
fn nav_image(w: &MainWindow, weak: slint::Weak<MainWindow>, page: usize) {
    let snap = READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.img_total == 0 {
            return None;
        }
        let last = r.img_total - 1;
        let pos = if SINGLE.with(|c| c.get()) { page.min(last) } else { page.min(last) & !1 };
        r.pos = pos;
        Some((r.book_id, r.img_path.clone(), r.img_format.clone(), pos, r.img_total))
    });
    let Some((book_id, path, format, pos, total)) = snap else { return };

    w.set_books_reader_left_folio(pos as i32 + 1);
    w.set_books_reader_right_folio(if pos + 1 < total { pos as i32 + 2 } else { 0 });
    w.set_books_reader_page(pos as i32 + 1);
    w.set_books_reader_page_count(total as i32);
    w.set_books_reader_percent(((pos + 1) as f32 / total as f32).clamp(0.0, 1.0));
    // Text/chapter chrome is meaningless for fixed-page books.
    w.set_books_reader_chapter_name("".into());
    w.set_books_reader_next_chapter_name("".into());
    w.set_books_reader_bookmarked(false);

    // Rasterise the two pages off-thread (poppler/unrar); decode on UI thread.
    let has_right = pos + 1 < total;
    tokio::runtime::Handle::current().spawn(async move {
        let render_at = |idx: usize| {
            let (p, f) = (path.clone(), format.clone());
            async move {
                tokio::task::spawn_blocking(move || render::page_image(&p, &f, idx).ok())
                    .await
                    .ok()
                    .flatten()
            }
        };
        // Render both pages of the spread concurrently (was sequential .await).
        let (left, right) = tokio::join!(render_at(pos), async {
            if has_right { render_at(pos + 1).await } else { None }
        });
        let _ = weak.upgrade_in_event_loop(move |w| {
            let load = |o: Option<std::path::PathBuf>| {
                o.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default()
            };
            w.set_books_reader_left_image(load(left));
            w.set_books_reader_right_image(load(right));
        });
        // Prefetch the next spread's pages into the disk cache so forward paging
        // is instant (page_image returns early on a cache hit).
        for nxt in [pos + 2, pos + 3] {
            if nxt < total {
                let (p, f) = (path.clone(), format.clone());
                tokio::task::spawn_blocking(move || {
                    let _ = render::page_image(&p, &f, nxt);
                });
            }
        }
    });

    // Persist page position (image books have no char offset / session fold).
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = progress::save(&pool, book_id, pos as i64, total as i64, pos as i64, 0).await;
        }
    });
}

// ── read-aloud (TTS) ──────────────────────────────────────────────────────────

/// Begin read-aloud: segment the current book text, seed the sentence list +
/// voices, and start the highlight timer.
fn tts_start(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return };
    // Image books (PDF/CBZ/CBR) have no reflow text → nothing to read aloud.
    if w.get_books_reader_image_mode() {
        return;
    }
    // Segment per screen-page so each sentence carries its page index (paginate
    // space) — the highlight can then drive the reader spread with no offset-space
    // mismatch. ponytail: cap the list — the read-along view isn't virtualised, so
    // thousands of Text rows would stall Slint. Phase 2 windows around the cursor.
    let (sents, sent_pages): (Vec<String>, Vec<usize>) = READER.with(|r| {
        let r = r.borrow();
        let mut texts = Vec::new();
        let mut pages = Vec::new();
        for (pi, page) in r.pages.iter().enumerate() {
            for s in tulipix_books::tts::sentences(&page.text) {
                texts.push(s.text);
                pages.push(pi);
                if texts.len() >= 1500 {
                    return (texts, pages);
                }
            }
        }
        (texts, pages)
    });
    if sents.is_empty() {
        return;
    }
    let g = TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.generation += 1;
        t.sentences = sents.clone();
        t.sent_pages = sent_pages;
        t.active = 0;
        t.playing = !sents.is_empty();
        if t.voice.is_empty() {
            t.voice = TTS_VOICES[0].0.to_string();
        }
        // Neural (Kokoro) / robotic (espeak) / timer, per setting + availability.
        let (audio, neural) = decide_engine(&t.voice);
        t.audio = audio;
        t.neural = neural;
        t.generation
    });
    let model: Vec<slint::SharedString> = sents.iter().map(|s| s.as_str().into()).collect();
    w.set_books_reader_tts_sentences(VecModel::from_slice(&model));
    let voices: Vec<slint::SharedString> = TTS_VOICES.iter().map(|(l, _)| (*l).into()).collect();
    w.set_books_reader_tts_voices(VecModel::from_slice(&voices));
    w.set_books_reader_tts_voice(TTS.with(|t| t.borrow().voice.clone()).into());
    w.set_books_reader_tts_active(0);
    w.set_books_reader_tts_playing(TTS.with(|t| t.borrow().playing));
    tts_resume(weak, g);
}

/// Continue playback from the current sentence — neural audio when `audio` is
/// set, else the timer-paced highlight.
fn tts_resume(weak: slint::Weak<MainWindow>, generation: u64) {
    if TTS.with(|t| t.borrow().audio) {
        tts_audio_step(weak, generation);
    } else {
        tts_schedule(weak, generation);
    }
}

/// Neural-audio step: synth the current sentence (Kokoro, off-thread), play it
/// through mpv, then advance the highlight + spread and recurse. The mpv runs
/// with an IPC socket stored in `tts_sock`, so pause/seek/stop quit it
/// mid-sentence (see [`tts_stop_audio`]) instead of waiting for the boundary.
fn tts_audio_step(weak: slint::Weak<MainWindow>, generation: u64) {
    let snap = TTS.with(|t| {
        let t = t.borrow();
        if t.generation != generation || !t.playing || t.active >= t.sentences.len() {
            return None;
        }
        Some((
            t.sentences[t.active].clone(),
            voice_id(&t.voice).to_string(),
            TTS_SPEEDS[t.speed_idx as usize % 4],
            t.neural,
        ))
    });
    let Some((text, voice, speed, neural)) = snap else { return };
    tokio::runtime::Handle::current().spawn(async move {
        // Render the sentence to a temp WAV off the async worker (CPU-bound):
        // Kokoro neural synth, or robotic espeak, per the chosen engine.
        let wav = tokio::task::spawn_blocking(move || {
            let path = std::env::temp_dir()
                .join(format!("tulipix-tts-{}-{}.wav", std::process::id(), generation));
            if neural {
                let pcm = tulipix_books::kokoro::synth(&text, &voice, speed).ok()?;
                if pcm.is_empty() {
                    return None;
                }
                tulipix_books::kokoro::write_wav(&pcm, &path).ok()?;
            } else {
                // espeak base rate ~175 wpm, scaled by the speed multiplier.
                let wpm = (175.0 * speed).round() as u32;
                let female = voice.as_bytes().get(1) == Some(&b'f');
                if !tulipix_books::speech::espeak_to_wav(&text, female, wpm, &path) {
                    return None;
                }
            }
            Some(path)
        })
        .await
        .ok()
        .flatten();
        // Play it and wait for the end (blocking) on a blocking worker. Spawn
        // mpv with an IPC socket (unique per generation so a seek's new mpv
        // can't collide with the outgoing one) and stash it so pause/seek can
        // `quit` it mid-sentence.
        if let Some(path) = wav.clone() {
            let _ = tokio::task::spawn_blocking(move || {
                let sock = tulipix_common::mpv_ipc::endpoint(&format!("tulipix-tts-{generation}"));
                tulipix_common::mpv_ipc::cleanup(&sock);
                let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
                cmd.args(["--no-video", "--really-quiet", "--no-terminal"])
                    .arg(format!("--input-ipc-server={}", sock.display()))
                    .arg(&path);
                tulipix_common::mpv_die_with_parent(&mut cmd);
                if let Ok(mut child) = cmd.spawn() {
                    if let Ok(mut g) = tts_sock().lock() {
                        *g = Some(sock.clone());
                    }
                    let _ = child.wait();
                    // Clear only if still ours (a newer step may have replaced it).
                    if let Ok(mut g) = tts_sock().lock() {
                        if g.as_deref() == Some(sock.as_path()) {
                            *g = None;
                        }
                    }
                }
                tulipix_common::mpv_ipc::cleanup(&sock);
                let _ = std::fs::remove_file(&path);
            })
            .await;
        }
        // Advance on the event-loop thread (unless cancelled/paused meanwhile).
        let _ = weak.upgrade_in_event_loop(move |w| {
            let (active, playing) = TTS.with(|t| {
                let mut t = t.borrow_mut();
                if t.generation != generation || !t.playing {
                    return (t.active, false);
                }
                t.active += 1;
                if t.active >= t.sentences.len() {
                    t.active = t.sentences.len().saturating_sub(1);
                    t.playing = false;
                }
                (t.active, t.playing)
            });
            w.set_books_reader_tts_active(active as i32);
            w.set_books_reader_tts_playing(playing);
            tts_follow_page(&w, w.as_weak(), active);
            if playing {
                tts_audio_step(w.as_weak(), generation);
            }
        });
    });
}

/// Schedule the highlight advance for the current sentence (delay ∝ length /
/// speed). Recurses until stopped/paused/end. ponytail: Phase-1 timing estimate
/// — Phase 2 replaces the delay with the real Kokoro audio-end callback.
fn tts_schedule(weak: slint::Weak<MainWindow>, generation: u64) {
    let dur = TTS.with(|t| {
        let t = t.borrow();
        if t.generation != generation || !t.playing || t.active >= t.sentences.len() {
            return None;
        }
        let sp = TTS_SPEEDS[t.speed_idx as usize % 4];
        Some(tulipix_books::tts::speak_ms(&t.sentences[t.active], 200.0, sp))
    });
    let Some(ms) = dur else { return };
    slint::Timer::single_shot(std::time::Duration::from_millis(ms), move || {
        let (active, playing) = TTS.with(|t| {
            let mut t = t.borrow_mut();
            if t.generation != generation || !t.playing {
                return (t.active, false);
            }
            t.active += 1;
            if t.active >= t.sentences.len() {
                t.active = t.sentences.len().saturating_sub(1);
                t.playing = false;
            }
            (t.active, t.playing)
        });
        if let Some(w) = weak.upgrade() {
            w.set_books_reader_tts_active(active as i32);
            w.set_books_reader_tts_playing(playing);
            tts_follow_page(&w, weak.clone(), active);
        }
        if playing {
            tts_schedule(weak.clone(), generation);
        }
    });
}

/// Turn the reader spread to the page holding sentence `active`, if it isn't
/// already visible — so read-aloud scrolls the book as it speaks.
fn tts_follow_page(w: &MainWindow, weak: slint::Weak<MainWindow>, active: usize) {
    let target = TTS.with(|t| t.borrow().sent_pages.get(active).copied());
    let Some(target) = target else { return };
    // Only move when the sentence's page isn't within the current spread.
    let cur = READER.with(|r| r.borrow().pos);
    let visible = target == cur || (!SINGLE.with(|c| c.get()) && target == cur + 1);
    if !visible {
        nav_to(w, weak, target);
    }
}

fn tts_play_pause(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return };
    let (playing, g) = TTS.with(|t| {
        let mut t = t.borrow_mut();
        if t.sentences.is_empty() {
            return (false, t.generation);
        }
        t.playing = !t.playing;
        if t.playing {
            t.generation += 1;
        }
        (t.playing, t.generation)
    });
    // Quit the in-flight sentence's mpv: on pause it stops now; on resume the
    // bumped generation gets a fresh mpv anyway.
    tts_stop_audio();
    w.set_books_reader_tts_playing(playing);
    if playing {
        tts_resume(weak, g);
    }
}

fn tts_set_speed(weak: slint::Weak<MainWindow>, idx: i32) {
    let Some(w) = weak.upgrade() else { return };
    let (playing, g) = TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.speed_idx = idx.clamp(0, 3) as u8;
        if t.playing {
            t.generation += 1;
        }
        (t.playing, t.generation)
    });
    tts_stop_audio(); // re-synth the current sentence at the new speed now
    w.set_books_reader_tts_speed_index(idx.clamp(0, 3));
    if playing {
        tts_resume(weak, g);
    }
}

fn tts_set_voice(name: String) {
    let (audio, neural) = decide_engine(&name);
    TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.audio = audio;
        t.neural = neural;
        t.voice = name;
    });
}

// ── define-a-word (R2) ────────────────────────────────────────────────────────

/// Fetch a word's definition/translation/wiki and push the result to the popup.
fn define_run(weak: slint::Weak<MainWindow>, word: String, lang: String) {
    let word = word.trim().to_string();
    if word.is_empty() {
        return;
    }
    if let Some(w) = weak.upgrade() {
        w.set_books_reader_define_loading(true);
        w.set_books_reader_define_definition("".into());
        w.set_books_reader_define_pos("".into());
        w.set_books_reader_define_translation("".into());
        w.set_books_reader_define_wiki("".into());
        w.set_books_reader_define_wiki_url("".into());
    }
    tokio::runtime::Handle::current().spawn(async move {
        let r = tulipix_books::lookup::lookup(&word, &lang).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_books_reader_define_loading(false);
            w.set_books_reader_define_definition(r.definition.into());
            w.set_books_reader_define_pos(r.part_of_speech.into());
            w.set_books_reader_define_translation(r.translation.into());
            w.set_books_reader_define_wiki(r.wiki_extract.into());
            w.set_books_reader_define_wiki_url(r.wiki_url.into());
        });
    });
}

/// Open a URL in the OS default browser (best-effort, cross-platform).
fn open_url(url: &str) {
    let prog = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(prog).arg(url).spawn();
}

fn tts_jump(weak: slint::Weak<MainWindow>, i: i32) {
    let Some(w) = weak.upgrade() else { return };
    let g = TTS.with(|t| {
        let mut t = t.borrow_mut();
        if t.sentences.is_empty() {
            return t.generation;
        }
        t.active = (i.max(0) as usize).min(t.sentences.len() - 1);
        t.playing = true;
        t.generation += 1;
        t.generation
    });
    let active = TTS.with(|t| t.borrow().active);
    tts_stop_audio(); // stop the outgoing sentence so the jump is instant
    w.set_books_reader_tts_active(active as i32);
    w.set_books_reader_tts_playing(true);
    tts_follow_page(&w, weak.clone(), active);
    tts_resume(weak, g);
}

fn tts_stop(weak: slint::Weak<MainWindow>) {
    TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.playing = false;
        t.active = 0;
        t.generation += 1;
    });
    tts_stop_audio();
    if let Some(w) = weak.upgrade() {
        w.set_books_reader_tts_playing(false);
        w.set_books_reader_tts_active(0);
    }
}

/// Chapter title to show on page `pos` — only on the chapter's first page,
/// empty otherwise (so it renders once, at the chapter opener).
fn page_heading(r: &ReaderState, pos: usize) -> String {
    let Some(p) = r.pages.get(pos) else { return String::new() };
    if paginate::page_of_chapter(&r.pages, p.chapter) != pos {
        return String::new();
    }
    r.chapter_titles.get(p.chapter).cloned().unwrap_or_default()
}

/// Drop a leading body line that duplicates the heading (EPUB flattens `<h1>`
/// into the text stream, so a chapter's first line is often its title).
/// Normalised (alphanumeric, lowercased) compare; no match → body unchanged.
fn strip_heading(body: &str, heading: &str) -> String {
    let norm = |s: &str| {
        s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect::<String>()
    };
    let h = norm(heading);
    if h.is_empty() {
        return body.to_string();
    }
    let mut lines = body.lines();
    match lines.next() {
        Some(first) if norm(first) == h => {
            lines.collect::<Vec<_>>().join("\n").trim_start_matches('\n').to_string()
        }
        _ => body.to_string(),
    }
}

/// Render the spread whose left page is `page` (clamped, even-aligned), then
/// refresh folio/percent/chapter/toc/bookmark state and save progress.
fn nav_to(w: &MainWindow, weak: slint::Weak<MainWindow>, page: usize) {
    if READER.with(|r| r.borrow().is_image) {
        return nav_image(w, weak, page);
    }
    struct Spread {
        book_id: i64,
        pos: usize,
        total: usize,
        left: Page,
        right: Option<Page>,
        left_heading: String,
        right_heading: String,
        chapter_changed: bool,
    }
    let s = READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.pages.is_empty() {
            return None;
        }
        let last = r.pages.len() - 1;
        let pos = if SINGLE.with(|c| c.get()) { page.min(last) } else { page.min(last) & !1 };
        let chapter_changed = r.pages[pos].chapter != r.pages[r.pos.min(last)].chapter;
        r.pos = pos;
        Some(Spread {
            book_id: r.book_id,
            pos,
            total: r.pages.len(),
            left: r.pages[pos].clone(),
            right: r.pages.get(pos + 1).cloned(),
            left_heading: page_heading(&r, pos),
            right_heading: page_heading(&r, pos + 1),
            chapter_changed,
        })
    });
    let Some(s) = s else { return };

    // A chapter's first page shows its title as a centered heading; strip a
    // duplicate leading title line from the body so it isn't shown twice.
    w.set_books_reader_left_text(strip_heading(&s.left.text, &s.left_heading).into());
    w.set_books_reader_left_heading(s.left_heading.clone().into());
    w.set_books_reader_right_text(
        s.right
            .as_ref()
            .map(|p| strip_heading(&p.text, &s.right_heading))
            .unwrap_or_default()
            .into(),
    );
    w.set_books_reader_right_heading(s.right_heading.clone().into());
    w.set_books_reader_left_folio(s.pos as i32 + 1);
    w.set_books_reader_right_folio(if s.right.is_some() { s.pos as i32 + 2 } else { 0 });
    w.set_books_reader_page(s.pos as i32 + 1);
    w.set_books_reader_page_count(s.total as i32);
    w.set_books_reader_percent(((s.pos + 1) as f32 / s.total as f32).clamp(0.0, 1.0));

    READER.with(|r| {
        let r = r.borrow();
        let cur = s.left.chapter as i32;
        let name_of = |ch: i32| {
            r.toc
                .iter()
                .find(|t| t.chapter == ch)
                .map(|t| t.label.clone())
                .unwrap_or_default()
        };
        w.set_books_reader_chapter_name(name_of(cur).into());
        w.set_books_reader_next_chapter_name(name_of(cur + 1).into());
        // Rebuild the contents list only when the chapter (and so the active
        // row) changes, so the panel keeps its scroll position on page turns.
        if s.chapter_changed || w.get_books_reader_toc().row_count() == 0 {
            let rows: Vec<TocRow> = r
                .toc
                .iter()
                .map(|t| TocRow {
                    label: t.label.clone().into(),
                    page: paginate::page_of_chapter(&r.pages, t.chapter.max(0) as usize) as i32
                        + 1,
                    depth: t.depth,
                    chapter: t.chapter,
                    active: t.chapter == cur,
                })
                .collect();
            w.set_books_reader_toc(Rc::new(VecModel::from(rows)).into());
        }
    });

    // Note indicator: does any annotation land on the current spread?
    let has_note = READER.with(|r| {
        let r = r.borrow();
        r.annots.iter().any(|a| {
            let ap = paginate::page_at_offset(&r.pages, a.start_off as usize);
            ap == s.pos || (s.right.is_some() && ap == s.pos + 1)
        })
    });
    w.set_books_reader_has_note(has_note);

    // Bookmark indicator for the new left page.
    let handle = tokio::runtime::Handle::current();
    let (book_id, pos) = (s.book_id, s.pos);
    handle.spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let on = progress::is_bookmarked(&pool, book_id, pos as i64)
            .await
            .unwrap_or(false);
        let _ = weak.upgrade_in_event_loop(move |w| w.set_books_reader_bookmarked(on));
    });

    persist_progress();
}

/// Jump to the first page of the previous/next chapter relative to `pos`.
fn nav_chapter(w: &MainWindow, weak: slint::Weak<MainWindow>, delta: i32) {
    let target = READER.with(|r| {
        let r = r.borrow();
        if r.pages.is_empty() {
            return None;
        }
        let cur = r.pages[r.pos.min(r.pages.len() - 1)].chapter as i32;
        Some(paginate::page_of_chapter(&r.pages, (cur + delta).max(0) as usize))
    });
    if let Some(p) = target {
        nav_to(w, weak, p);
    }
}

/// Toggle a bookmark on the current left page; updates the ribbon state.
fn reader_toggle_bookmark(weak: slint::Weak<MainWindow>) {
    let snap = READER.with(|r| {
        let r = r.borrow();
        if r.pages.is_empty() {
            return None;
        }
        let pos = r.pos.min(r.pages.len() - 1);
        Some((r.book_id, pos as i64, r.pages[pos].char_start as i64))
    });
    let Some((book_id, page, off)) = snap else { return };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let on = progress::toggle_bookmark(&pool, book_id, page, off)
            .await
            .unwrap_or(false);
        let _ = weak.upgrade_in_event_loop(move |w| w.set_books_reader_bookmarked(on));
    });
}

// ── notes (annotate) ─────────────────────────────────────────────────────────

/// Build annotate-panel rows: screen page is re-derived from the char offset so
/// it stays correct across font-size repagination.
fn annot_rows(annots: &[Annotation], pages: &[Page]) -> Vec<AnnotRow> {
    annots
        .iter()
        .map(|a| AnnotRow {
            id: a.id as i32,
            page: paginate::page_at_offset(pages, a.start_off as usize) as i32 + 1,
            snippet: a.snippet.clone().into(),
            note: a.note.clone().into(),
        })
        .collect()
}

/// Load the current book's notes from the DB, mirror them into `ReaderState`,
/// and push the panel rows. Must be called on the event-loop thread.
fn reader_load_notes(weak: slint::Weak<MainWindow>) {
    let book_id = READER.with(|r| r.borrow().book_id);
    if book_id == 0 {
        return;
    }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("books").await else { return };
        let list = annotations::for_book(&pool, book_id).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let (rows, has_note) = READER.with(|r| {
                let mut r = r.borrow_mut();
                r.annots = list;
                let pos = r.pos;
                let has = r.annots.iter().any(|a| {
                    paginate::page_at_offset(&r.pages, a.start_off as usize) == pos
                });
                (annot_rows(&r.annots, &r.pages), has)
            });
            w.set_books_reader_notes(VecModel::from_slice(&rows));
            w.set_books_reader_has_note(has_note);
        });
    });
}

/// Add a note anchored to the current page (whole-page anchor; snippet is the
/// page's opening text), then reload the panel.
fn reader_annot_add(weak: slint::Weak<MainWindow>) {
    let snap = READER.with(|r| {
        let r = r.borrow();
        if r.pages.is_empty() {
            return None;
        }
        let pos = r.pos.min(r.pages.len() - 1);
        let p = &r.pages[pos];
        let snippet: String = p.text.replace('\n', " ").chars().take(60).collect();
        Some((r.book_id, (pos + 1) as i64, p.char_start as i64, snippet.trim().to_string()))
    });
    let Some((book_id, page, off, snippet)) = snap else { return };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = annotations::add(&pool, book_id, page, off, off, &snippet, "amber").await;
        }
        let _ = weak.upgrade_in_event_loop(|w| reader_load_notes(w.as_weak()));
    });
}

/// Persist an edited note (and update the in-memory mirror so a reload keeps it).
fn reader_annot_set_note(_weak: slint::Weak<MainWindow>, id: i64, note: String) {
    READER.with(|r| {
        if let Some(a) = r.borrow_mut().annots.iter_mut().find(|a| a.id == id) {
            a.note = note.clone();
        }
    });
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = annotations::set_note(&pool, id, &note).await;
        }
    });
}

/// Delete a note, then reload the panel.
fn reader_annot_remove(weak: slint::Weak<MainWindow>, id: i64) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = annotations::remove(&pool, id).await;
        }
        let _ = weak.upgrade_in_event_loop(|w| reader_load_notes(w.as_weak()));
    });
}

// ── ••• menu ─────────────────────────────────────────────────────────────────

/// Mark the current book finished (progress at last page → `books.finished`),
/// then refresh Home so the hero/stats update. Reader stays open.
fn reader_mark_finished(weak: slint::Weak<MainWindow>, sort_idx: usize) {
    let snap = READER.with(|r| {
        let r = r.borrow();
        if r.pages.is_empty() {
            return None;
        }
        let last = r.pages.len() - 1;
        Some((r.book_id, r.pages.len() as i64, r.pages[last].char_start as i64))
    });
    let Some((book_id, total, off)) = snap else { return };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = progress::save(&pool, book_id, total - 1, total, off, 0).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Remove the current book from the library, close the reader, refresh Home.
fn reader_remove_book(weak: slint::Weak<MainWindow>, sort_idx: usize) {
    let book_id = READER.with(|r| r.borrow().book_id);
    if book_id == 0 {
        return;
    }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::remove(&pool, book_id).await;
        }
        let _ = weak.upgrade_in_event_loop(|w| w.set_books_reader_open(false));
        books_refresh(weak, sort_idx);
    });
}

/// In-book search: case-insensitive scan of the current book's pages, pushing
/// up to 200 hits (1-based page + a snippet around the first match per page)
/// to the search panel. Runs on the event-loop thread — pages are already in
/// memory, so no async is needed.
fn reader_search(weak: slint::Weak<MainWindow>, query: String) {
    let Some(w) = weak.upgrade() else { return };
    let q = query.trim().to_lowercase();
    if q.len() < 2 {
        w.set_books_reader_search_results(VecModel::from_slice(&[]));
        return;
    }
    let rows: Vec<SearchRow> = READER.with(|r| {
        r.borrow()
            .pages
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let hay = p.text.to_lowercase();
                let pos = hay.find(&q)?;
                Some(SearchRow {
                    page: i as i32 + 1,
                    snippet: search_snippet(&hay, pos, q.len()).into(),
                })
            })
            .take(200)
            .collect()
    });
    w.set_books_reader_search_results(VecModel::from_slice(&rows));
}

/// ~30 chars of context on each side of a byte-offset match, on char
/// boundaries, newlines flattened, ellipsised when clipped.
fn search_snippet(s: &str, byte_pos: usize, match_len: usize) -> String {
    let start = s[..byte_pos]
        .char_indices()
        .rev()
        .take(30)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end_byte = (byte_pos + match_len).min(s.len());
    let end = s[end_byte..]
        .char_indices()
        .take(40)
        .last()
        .map(|(i, _)| end_byte + i)
        .unwrap_or(s.len());
    let body = s[start..end].replace('\n', " ");
    let body = body.trim();
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        body,
        if end < s.len() { "…" } else { "" },
    )
}

/// Aa stepper: change the reader font size by `delta` px (clamped), then
/// re-paginate in place (position kept by char offset).
fn reader_step_font(weak: slint::Weak<MainWindow>, delta: f32) {
    let Some(w) = weak.upgrade() else { return };
    let new_px = (PREFS.with(|c| c.get().font_px) + delta).clamp(FONT_MIN, FONT_MAX);
    if PREFS.with(|c| c.get().font_px) == new_px {
        return;
    }
    PREFS.with(|c| { let mut p = c.get(); p.font_px = new_px; c.set(p); });
    w.set_books_reader_font_px(new_px);
    reader_repaginate(&w, w.as_weak());
}

/// Save the current position and fold the elapsed reading burst into
/// `time_read_secs`; restarts the session clock so time isn't double-counted.
fn persist_progress() {
    let snap = READER.with(|r| {
        let mut r = r.borrow_mut();
        if r.pages.is_empty() {
            return None;
        }
        // Fold the reading burst into total time — but cap it: a gap longer than
        // IDLE_CAP means the reader sat open unattended, not active reading.
        const IDLE_CAP: i64 = 300; // 5 min
        let secs = r
            .session_start
            .replace(std::time::Instant::now())
            .map(|t| (t.elapsed().as_secs() as i64).min(IDLE_CAP))
            .unwrap_or(0);
        let pos = r.pos.min(r.pages.len() - 1);
        Some((
            r.book_id,
            pos as i64,
            r.pages.len() as i64,
            r.pages[pos].char_start as i64,
            secs,
        ))
    });
    let Some((book_id, page, total, off, secs)) = snap else { return };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = progress::save(&pool, book_id, page, total, off, secs).await;
            // Log today for the reading streak/heatmap.
            let _ = library::mark_read_today(&pool).await;
        }
    });
}

/// Rough "N min left" label from remaining pages (~2 min/page).
fn time_left_label(page: i64, total: i64) -> slint::SharedString {
    let remaining = (total - page).max(0);
    if total <= 0 || remaining == 0 {
        return "".into();
    }
    let mins = remaining * 2;
    if mins >= 60 {
        format!("{}h {}m left", mins / 60, mins % 60).into()
    } else {
        format!("{mins}m left").into()
    }
}
