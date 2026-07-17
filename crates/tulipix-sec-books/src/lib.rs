//! Books section wiring extracted from tulipix-app: the Book Home page —
//! Continue Reading hero, the 2×2 stats strip, the filter chips (file types /
//! genres / quick filters), sort + search, and the paginated library grid.
//! Every `window.on_books_*` callback is registered here. tulipix-app calls
//! [`wire`] once at startup.

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

/// Books per grid page.
const PAGE_SIZE: usize = 24;

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
    // Read-aloud (TTS) state — Phase 1 timer-paced highlight; Phase 2 syncs to
    // Kokoro audio. Lives on the event-loop thread (all TTS callbacks + the
    // slint::Timer fire there).
    static TTS: RefCell<TtsState> = RefCell::new(TtsState::default());
}

#[derive(Default)]
struct TtsState {
    sentences: Vec<String>,
    active: usize,
    playing: bool,
    speed_idx: u8,
    voice: String,
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

/// Build the pagination [`Layout`] from the current prefs.
fn layout_from(p: &ReaderPrefs) -> Layout {
    Layout {
        font_px: p.font_px,
        line_height: LINE_HEIGHTS[p.line_idx as usize % 3],
        pad_x_px: MARGINS[p.margin_idx as usize % 3],
        glyph_em: typeface_glyph(p.typeface),
        ..Layout::default()
    }
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
    let sort_idx = Rc::new(Cell::new(0usize));

    // Restore the persisted grid/list view preference.
    window.set_books_view_mode(tulipix_books::prefs::load_view_mode().into());

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
            w.set_books_active_quick(k);
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
            w.set_books_page(1);
        }
        books_refresh(w.clone(), s.get());
    });

    // Local star rating.
    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_rating(move |id, rating| {
        books_set_rating(w.clone(), id as i64, rating as f64, s.get());
    });

    let w = window.as_weak();
    let s = sort_idx.clone();
    window.on_books_set_sort(move |label| {
        s.set(sort_index_of(&label));
        if let Some(w) = w.upgrade() { w.set_books_page(1); }
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

    // ── Reader (Phase 3) ─────────────────────────────────────────────────
    // Open a book from a grid tile.
    let w = window.as_weak();
    window.on_books_open_details(move |id| { books_open(w.clone(), id as i64); });
    // Hero "Continue reading" + cover click both resume the in-progress book.
    let w = window.as_weak();
    window.on_books_resume(move || { books_resume(w.clone()); });
    let w = window.as_weak();
    window.on_books_hero_details(move || { books_resume(w.clone()); });

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

    // Initial paint.
    books_refresh(window.as_weak(), 0);
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
        "reading" | "finished" | "unread" | "favorites" => quick,
        _ => String::new(), // "all"
    };
    Filter {
        format: w.get_books_active_file_type().to_string(),
        status,
        author: w.get_books_active_author().to_string(),
        genre: w.get_books_active_genre().to_string(),
        query: w.get_books_search().to_string(),
        sort: Sort::from_index(sort_idx),
    }
}

/// Reload the whole Book Home page: hero + stats from `home::load`, the grid
/// from `library::list` (filtered + paginated), and the chip rows.
fn books_refresh(weak: slint::Weak<MainWindow>, sort_idx: usize) {
    let filter = match weak.upgrade() {
        Some(w) => filter_from(&w, sort_idx),
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
        let home = tulipix_books::home::load(&pool).await.unwrap_or_default();
        let rows = library::list(&pool, &filter).await.unwrap_or_default();
        let formats = library::format_counts(&pool).await.unwrap_or_default();
        let genres = library::genre_counts(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            apply_home(&w, &home);
            apply_grid(&w, &rows);
            apply_chips(&w, &formats, &genres, &home);
        });
    });
}

/// Push hero + the 2×2 stats strip.
fn apply_home(w: &MainWindow, home: &HomeData) {
    let hero = match &home.continue_reading {
        Some((b, page, total)) => {
            let pct = if *total > 0 { *page as f32 / *total as f32 } else { b.percent as f32 };
            BooksHero {
                present: true,
                title: b.title.clone().into(),
                author: b.author.clone().into(),
                cover: load_cover(&b.cover_path),
                hue: cover_hue(&b.title),
                page: *page as i32,
                total: *total as i32,
                percent: pct.clamp(0.0, 1.0),
                time_left: time_left_label(*page, *total),
            }
        }
        None => BooksHero { present: false, ..Default::default() },
    };
    w.set_books_hero(hero);

    // Stat identities (icons/labels/colors) live in the .slint; only the four
    // values flow through, in order: Total Books, Authors, In Progress, Completed.
    let s = &home.stats;
    let val = |n: i64| BookStat { value: n.to_string().into(), ..Default::default() };
    let stats: Vec<BookStat> = vec![val(s.total), val(s.authors), val(s.in_progress), val(s.finished)];
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
            title: b.title.clone().into(),
            author: b.author.clone().into(),
            cover: load_cover(&b.cover_path),
            percent: b.percent as f32,
            favorite: b.favorite != 0,
            format: b.format.to_uppercase().into(),
            date: fmt_date(b.added_at),
            hue: cover_hue(&b.title),
            rating: b.rating as f32,
            missing: b.missing != 0,
        })
        .collect();
    w.set_books_tiles(VecModel::from_slice(&tiles));
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
    ];
    w.set_books_quick_filters(VecModel::from_slice(&quick));
}

/// Set a book's local star rating, then refresh (keeps rating-sort order live).
fn books_set_rating(weak: slint::Weak<MainWindow>, id: i64, rating: f64, sort_idx: usize) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = library::set_rating(&pool, id, rating).await;
        }
        books_refresh(weak, sort_idx);
    });
}

/// Toggle a book's favorite flag, then refresh the grid.
fn books_toggle_favorite(weak: slint::Weak<MainWindow>, id: i64, sort_idx: usize) {
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
        if let Ok(pool) = pool_for("books").await {
            match action.as_str() {
                "favorite" => { let _ = library::toggle_favorite(&pool, id).await; }
                "remove" => { let _ = library::remove(&pool, id).await; }
                _ => {}
            }
        }
        books_refresh(weak, sort_idx);
    });
}

/// "Add books" → folder picker → scan into the library.
fn books_add_folder(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await else { return };
        let path = dir.path().to_string_lossy().to_string();
        if let Ok(pool) = pool_for("books").await {
            let _ = tulipix_books::scan::add_folder(&pool, &path).await;
            let _ = tulipix_books::scan::scan_all(&pool).await;
        }
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

/// Decode a cover file into a slint image (empty → default placeholder).
fn load_cover(path: &str) -> slint::Image {
    if path.is_empty() {
        return slint::Image::default();
    }
    slint::Image::load_from_path(std::path::Path::new(path)).unwrap_or_default()
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
        });
    });
}

/// Open a fixed-page book (PDF/CBZ/CBR): count pages off-thread, seed the image
/// reader state, then render the first spread. Already inside the async task.
async fn open_image_book(weak: slint::Weak<MainWindow>, book: BookRow, start_page: usize) {
    let path = std::path::PathBuf::from(&book.path);
    let format = book.format.clone();
    let (path, format, total) = tokio::task::spawn_blocking(move || {
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
        let left = render_at(pos).await;
        let right = if has_right { render_at(pos + 1).await } else { None };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let load = |o: Option<std::path::PathBuf>| {
                o.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default()
            };
            w.set_books_reader_left_image(load(left));
            w.set_books_reader_right_image(load(right));
        });
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
    let full = w.get_books_reader_full_text().to_string();
    // ponytail: cap the sentence list — the read-along view isn't virtualised, so
    // thousands of Text rows would stall Slint. Phase 2 windows around the cursor.
    let sents: Vec<String> = tulipix_books::tts::sentences(&full)
        .into_iter()
        .map(|s| s.text)
        .take(1500)
        .collect();
    let g = TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.generation += 1;
        t.sentences = sents.clone();
        t.active = 0;
        t.playing = !sents.is_empty();
        if t.voice.is_empty() {
            t.voice = TTS_VOICES[0].0.to_string();
        }
        t.generation
    });
    let model: Vec<slint::SharedString> = sents.iter().map(|s| s.as_str().into()).collect();
    w.set_books_reader_tts_sentences(VecModel::from_slice(&model));
    let voices: Vec<slint::SharedString> = TTS_VOICES.iter().map(|(l, _)| (*l).into()).collect();
    w.set_books_reader_tts_voices(VecModel::from_slice(&voices));
    w.set_books_reader_tts_voice(TTS.with(|t| t.borrow().voice.clone()).into());
    w.set_books_reader_tts_active(0);
    w.set_books_reader_tts_playing(TTS.with(|t| t.borrow().playing));
    tts_schedule(weak, g);
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
        }
        if playing {
            tts_schedule(weak.clone(), generation);
        }
    });
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
    w.set_books_reader_tts_playing(playing);
    if playing {
        tts_schedule(weak, g);
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
    w.set_books_reader_tts_speed_index(idx.clamp(0, 3));
    if playing {
        tts_schedule(weak, g);
    }
}

fn tts_set_voice(name: String) {
    TTS.with(|t| t.borrow_mut().voice = name);
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
    w.set_books_reader_tts_active(TTS.with(|t| t.borrow().active) as i32);
    w.set_books_reader_tts_playing(true);
    tts_schedule(weak, g);
}

fn tts_stop(weak: slint::Weak<MainWindow>) {
    TTS.with(|t| {
        let mut t = t.borrow_mut();
        t.playing = false;
        t.active = 0;
        t.generation += 1;
    });
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
            let rows = READER.with(|r| {
                let mut r = r.borrow_mut();
                r.annots = list;
                annot_rows(&r.annots, &r.pages)
            });
            w.set_books_reader_notes(VecModel::from_slice(&rows));
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
        let secs = r
            .session_start
            .replace(std::time::Instant::now())
            .map(|t| t.elapsed().as_secs() as i64)
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
