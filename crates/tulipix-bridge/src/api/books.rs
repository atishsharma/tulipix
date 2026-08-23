// The Books section: a library that is also a reader.
//
// Two halves that share one snapshot. The library half is a filtered, paged,
// sorted grid over `books.db` with collections, smart collections, a hero
// carousel of what you are part-way through, and reading statistics. The reader
// half opens one of those books and turns its pages — as laid-out text for
// EPUB/FB2/MOBI, and as rendered images for PDF/CBZ/CBR.
//
// They share a snapshot because they share a book. Turning a page writes
// progress, and progress is what the grid's percentage ring and the hero's
// "3 hours left" are drawn from; splitting them would mean two sources of truth
// for the same number and a stale one every time you closed the reader.
//
// Almost nothing here is logic. `tulipix-books` already knows how to parse an
// EPUB spine, paginate a chapter against a layout, rasterise a PDF page, hold a
// bookmark and score an FTS query. This file decides what the screen is looking
// at, asks that crate, and hands the answer to Dart in one struct.

use anyhow::Result;
use flutter_rust_bridge::frb;

use crate::frb_generated::StreamSink;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use tulipix_books::{annotations, covers, home, library, paginate, progress, render, scan, toc};

use crate::db::books_pool;

// ------------------------------------------------------------------- state ---

/// One book, as a tile or a row. `cover` is a path the UI can hand straight to
/// an image widget, empty when none has been extracted yet.
pub struct Book {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub series: String,
    pub genre: String,
    pub format: String,
    pub cover: String,
    pub published: String,
    /// 0..1 through the book.
    pub percent: f64,
    pub favorite: bool,
    pub finished: bool,
    pub missing: bool,
    pub trashed: bool,
    pub magazine: bool,
    pub rtl: bool,
    /// The user's own stars, 0..5.
    pub rating: f64,
    /// Open Library's average, 0..5, 0 when unknown.
    pub net_rating: f64,
    pub size_bytes: i64,
    pub added_at: i64,
    pub last_read: i64,
    pub time_read: i64,
}

/// A filter chip: a facet value and how many books carry it.
pub struct FacetChip {
    pub key: String,
    pub label: String,
    pub count: i64,
}

/// A collection, and — in the detail sheet — whether this book is in it.
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub member: bool,
}

/// The card at the top: what you are in the middle of.
pub struct HeroBook {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub cover: String,
    pub page: i64,
    pub total: i64,
    pub percent: f64,
    /// "about 3 h left", derived from this book's own reading pace.
    pub time_left: String,
}

/// The four numbers across the top of the library.
pub struct LibraryStats {
    pub total: i64,
    pub authors: i64,
    pub finished: i64,
    pub in_progress: i64,
    pub hours_read: f64,
    pub added_month: i64,
    pub authors_month: i64,
}

pub struct StatBook {
    pub title: String,
    pub time: String,
}

/// The reading-statistics panel.
pub struct ReadingStats {
    pub hours: f64,
    pub days: i64,
    pub finished: i64,
    pub started: i64,
    pub streak: i64,
    /// One bool per day for the last 365, oldest first — the heat strip.
    pub heat: Vec<bool>,
    pub books: Vec<StatBook>,
}

pub struct TocItem {
    pub label: String,
    pub depth: i64,
    /// Index into the spine, or -1 when the href is not in it.
    pub chapter: i64,
}

pub struct BookmarkRow {
    pub id: i64,
    pub page: i64,
    pub note: String,
    pub color: String,
    pub created_at: i64,
}

pub struct NoteRow {
    pub id: i64,
    pub page: i64,
    pub snippet: String,
    pub note: String,
    pub color: String,
    pub created_at: i64,
}

/// One hit from the in-book search: which screen page, and enough words around
/// the match to recognise it.
pub struct SearchHit {
    pub page: i64,
    pub snippet: String,
}

/// A page thumbnail in the strip, for fixed-page books.
pub struct ThumbRow {
    pub page: i64,
    pub path: String,
}

/// Typography and page appearance. Shared with the Slint build through the same
/// prefs file, so a font size chosen in one applies in the other.
pub struct ReaderPrefs {
    pub font_px: f64,
    /// 0 compact · 1 normal · 2 relaxed.
    pub line_index: i64,
    /// 0 narrow · 1 normal · 2 wide.
    pub margin_index: i64,
    /// 0 serif · 1 sans · 2 mono.
    pub typeface: i64,
    /// 0 left · 1 justify · 2 right.
    pub align: i64,
    pub bold: bool,
    /// light | sepia | dark.
    pub theme: String,
    /// 0.4..1.0 page dim.
    pub brightness: f64,
}

/// The reader, when one is open.
pub struct Reader {
    pub open: bool,
    pub id: i64,
    pub title: String,
    pub author: String,
    pub format: String,
    /// True for PDF/CBZ/CBR — pages are rendered images, not laid-out text.
    pub image_mode: bool,
    /// 1-based screen page (a spread counts as one).
    pub page: i64,
    pub page_count: i64,
    pub percent: f64,
    pub chapter_name: String,
    pub next_chapter_name: String,
    /// Text pages. Empty in image mode.
    pub left_text: String,
    pub right_text: String,
    /// Chapter title, on a chapter's opening page only.
    pub left_heading: String,
    pub right_heading: String,
    /// Rendered page images. Empty in text mode.
    pub left_image: String,
    pub right_image: String,
    pub left_folio: i64,
    pub right_folio: i64,
    /// Right-to-left page order (manga).
    pub rtl: bool,
    /// One page at a time rather than a spread.
    pub single: bool,
    /// Crop the scanned margins off a fixed page.
    pub trim: bool,
    pub bookmarked: bool,
    pub has_note: bool,
    pub toc: Vec<TocItem>,
    pub bookmarks: Vec<BookmarkRow>,
    pub notes: Vec<NoteRow>,
    pub search_query: String,
    pub search_results: Vec<SearchHit>,
    /// Whether searching is offered at all — a magazine has no text layer worth
    /// searching, and the user can say so per book.
    pub search_enabled: bool,
    pub thumbs: Vec<ThumbRow>,
    pub prefs: ReaderPrefs,
    /// The next book in the same series, offered at the end.
    pub next_title: String,
    pub next_id: i64,
    pub error: String,
}

/// Everything one screen of Books is looking at.
pub struct BooksState {
    /// library | trash.
    pub view: String,
    pub books: Vec<Book>,
    pub total: i64,
    pub filtered_total: i64,
    pub page: i64,
    pub page_count: i64,
    /// grid | list.
    pub view_mode: String,
    /// Index into the five sort options.
    pub sort_index: i64,
    pub query: String,
    /// Search the books' text, not just their titles.
    pub search_contents: bool,

    pub hero: Option<HeroBook>,
    pub slider: Vec<HeroBook>,
    pub stats: LibraryStats,

    pub formats: Vec<FacetChip>,
    pub genres: Vec<FacetChip>,
    pub series: Vec<FacetChip>,
    pub collections: Vec<CollectionRow>,
    pub smart: Vec<CollectionRow>,

    pub active_format: String,
    pub active_genre: String,
    pub active_series: String,
    pub active_author: String,
    pub active_quick: String,
    pub active_collection: i64,

    pub trashed_count: i64,
    pub folders: Vec<String>,

    /// The detail sheet's book, when open.
    pub detail: Option<Book>,
    pub detail_summary: String,
    pub detail_collections: Vec<CollectionRow>,

    pub reading_stats: Option<ReadingStats>,

    pub reader: Reader,

    /// A one-line note under the header — what the last command did.
    pub status: String,
}

// ---------------------------------------------------------------- commands ---

/// Everything the UI can ask for. One enum rather than eighty functions: the
/// answer is always the whole snapshot, so there is nothing for eighty return
/// types to differ about.
pub enum BooksCmd {
    Refresh,

    // --- library ---
    Search {
        text: String,
    },
    SetSearchContents {
        on: bool,
    },
    SetSort {
        index: i64,
    },
    SetViewMode {
        mode: String,
    },
    SetPage {
        page: i64,
    },
    SetFormat {
        format: String,
    },
    SetGenre {
        genre: String,
    },
    SetSeries {
        series: String,
    },
    SetAuthor {
        author: String,
    },
    /// all · reading · finished · unread · favorite · missing.
    SetQuick {
        quick: String,
    },
    SetCollection {
        id: i64,
    },
    ClearFilters,
    SetView {
        view: String,
    },

    // --- one book ---
    ToggleFavorite {
        id: i64,
    },
    ToggleMagazine {
        id: i64,
    },
    ToggleRtl {
        id: i64,
    },
    SetRating {
        id: i64,
        rating: f64,
    },
    Trash {
        id: i64,
    },
    Restore {
        id: i64,
    },
    DeletePerm {
        id: i64,
    },
    EmptyTrash,
    Relink {
        id: i64,
        path: String,
    },
    OpenDetail {
        id: i64,
    },
    CloseDetail,
    FetchSummary {
        id: i64,
    },

    // --- collections ---
    CollectionCreate {
        name: String,
    },
    CollectionRename {
        id: i64,
        name: String,
    },
    CollectionDelete {
        id: i64,
    },
    CollectionToggle {
        id: i64,
        book_id: i64,
    },
    SmartCreateFromFilters {
        name: String,
    },
    SmartApply {
        id: i64,
    },
    SmartDelete {
        id: i64,
    },

    // --- library maintenance ---
    AddFolder {
        path: String,
    },
    RemoveFolder {
        path: String,
    },
    Scan,
    BuildArt,
    IndexContents,

    // --- statistics ---
    OpenStats,
    CloseStats,

    // --- the reader ---
    OpenBook {
        id: i64,
    },
    CloseReader,
    ReaderNext,
    ReaderPrev,
    ReaderJump {
        page: i64,
    },
    ReaderChapter {
        index: i64,
    },
    ReaderNextChapter,
    ReaderPrevChapter,
    ReaderOpenNext,
    MarkFinished,
    ToggleBookmark,
    BookmarkJump {
        page: i64,
    },
    BookmarkRemove {
        id: i64,
    },
    AnnotAdd {
        snippet: String,
        note: String,
        color: String,
    },
    AnnotSetNote {
        id: i64,
        note: String,
    },
    AnnotRemove {
        id: i64,
    },
    ReaderSearch {
        query: String,
    },
    LoadThumbs,
    SetSingle {
        on: bool,
    },
    SetTrim {
        on: bool,
    },
    SetFontPx {
        px: f64,
    },
    SetLineSpacing {
        index: i64,
    },
    SetMargin {
        index: i64,
    },
    SetTypeface {
        index: i64,
    },
    SetAlign {
        index: i64,
    },
    ToggleBold,
    SetTheme {
        theme: String,
    },
    SetBrightness {
        value: f64,
    },
}

/// Out-of-band news. Everything else is a reply to a command.
pub enum BooksEvent {
    ScanProgress {
        done: i64,
        total: i64,
        name: String,
    },
    ScanFinished {
        added: i64,
        updated: i64,
        missing: i64,
    },
    Failed {
        message: String,
    },
}

// ----------------------------------------------------------------- session ---

/// What the screen is looking at, between commands. In Rust rather than Dart
/// because every query re-derives from it, and a filter that lived on the other
/// side would cross the bridge on every keystroke.
///
/// `frb(ignore)`: private state, not part of the contract.
#[frb(ignore)]
#[derive(Debug, Default)]
struct Session {
    view: String,
    query: String,
    search_contents: bool,
    sort_index: usize,
    view_mode: String,
    page: i64,
    format: String,
    genre: String,
    series: String,
    author: String,
    quick: String,
    collection: i64,

    detail_id: i64,
    stats_open: bool,

    reader: ReaderSession,
    status: String,
}

/// The open book. Held here rather than re-derived because paginating a novel
/// is not something to do on every page turn.
#[frb(ignore)]
#[derive(Debug, Default)]
struct ReaderSession {
    open: bool,
    id: i64,
    path: String,
    format: String,
    title: String,
    author: String,
    image_mode: bool,
    rtl: bool,
    single: bool,
    trim: bool,
    /// Laid-out text pages. Empty in image mode.
    pages: Vec<paginate::Page>,
    /// Chapter titles, indexed the same as the spine.
    chapter_titles: Vec<String>,
    /// Total leaves in a fixed-page book.
    image_pages: usize,
    /// 1-based screen page — a spread is one.
    screen: i64,
    toc: Vec<toc::TocEntry>,
    search_query: String,
    hits: Vec<(i64, String)>,
    thumbs: Vec<(i64, String)>,
    next_id: i64,
    next_title: String,
    error: String,
    prefs: Prefs,
}

#[frb(ignore)]
#[derive(Debug, Clone, Copy)]
struct Prefs {
    font_px: f32,
    line_index: u8,
    margin_index: u8,
    typeface: u8,
    align: u8,
    bold: bool,
    theme_index: u8,
    brightness: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            font_px: 17.0,
            line_index: 1,
            margin_index: 1,
            typeface: 0,
            align: 1,
            bold: false,
            theme_index: 0,
            brightness: 1.0,
        }
    }
}

impl Prefs {
    fn theme(self) -> &'static str {
        match self.theme_index {
            1 => "sepia",
            2 => "dark",
            _ => "light",
        }
    }

    /// The page geometry these preferences imply. Pagination is a function of
    /// this and nothing else, which is why changing any of it repaginates.
    fn layout(self) -> paginate::Layout {
        let line_height = match self.line_index {
            0 => 1.35,
            2 => 1.85,
            _ => 1.55,
        };
        let pad_x = match self.margin_index {
            0 => 64.0,
            2 => 160.0,
            _ => 96.0,
        };
        paginate::Layout {
            page_w_px: 520.0,
            page_h_px: 720.0,
            font_px: self.font_px,
            line_height,
            pad_x_px: pad_x,
            pad_y_px: 120.0,
            // Mono sets wider than serif, so the same column holds fewer
            // characters; getting this wrong is a page that overflows or one
            // that ends short.
            glyph_em: if self.typeface == 2 { 0.6 } else { 0.5 },
        }
    }
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            view: "library".into(),
            view_mode: tulipix_books::prefs::load_view_mode(),
            sort_index: tulipix_books::prefs::load_sort(),
            page: 1,
            quick: "all".into(),
            reader: ReaderSession {
                prefs: load_prefs(),
                ..Default::default()
            },
            ..Default::default()
        })
    })
}

/// A poisoned session is recoverable: the state it holds is a view, not an
/// invariant, and refusing to open the library because a previous command
/// panicked would be the worse failure.
fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn load_prefs() -> Prefs {
    match tulipix_books::prefs::load_reader_prefs() {
        Some((
            font_px,
            line_index,
            margin_index,
            typeface,
            align,
            bold,
            theme_index,
            brightness,
        )) => Prefs {
            font_px,
            line_index,
            margin_index,
            typeface,
            align,
            bold,
            theme_index,
            brightness,
        },
        None => Prefs::default(),
    }
}

fn save_prefs(p: Prefs) {
    tulipix_books::prefs::save_reader_prefs(
        p.font_px,
        p.line_index,
        p.margin_index,
        p.typeface,
        p.align,
        p.bold,
        p.theme_index,
        p.brightness,
    );
}

// ---------------------------------------------------------------- exported ---

/// Do one thing, then describe everything. The reply is the whole snapshot
/// because a command rarely changes only what it names — trashing a book moves
/// a count, a chip, a page total and possibly the hero.
pub async fn books_dispatch(cmd: BooksCmd) -> Result<BooksState> {
    apply(cmd).await?;
    snapshot().await
}

/// Scan progress and failures. Everything else is a reply.
#[frb(sync)]
pub fn books_events(sink: StreamSink<BooksEvent>) {
    let _ = events().set(sink);
}

/// A book's cover, extracted on demand. Separate from the snapshot for the same
/// reason Photos' thumbnails are: unpacking a cover out of every EPUB in a
/// freshly scanned library, inside the query that draws the first screen, is how
/// a library takes a minute to open instead of a moment.
pub async fn books_ensure_cover(id: i64) -> Result<Option<String>> {
    let pool = books_pool().await?;
    let Some(row) = library::get(pool, id).await? else {
        return Ok(None);
    };
    if !row.cover_path.is_empty() && Path::new(&row.cover_path).exists() {
        return Ok(Some(row.cover_path));
    }
    let path = PathBuf::from(&row.path);
    if !path.exists() {
        return Ok(None);
    }
    match covers::extract(&path, &row.format) {
        Ok(p) => {
            let s = p.to_string_lossy().to_string();
            let _ = sqlx::query("UPDATE books SET cover_path = ? WHERE id = ?")
                .bind(&s)
                .bind(id)
                .execute(pool)
                .await;
            Ok(Some(s))
        }
        Err(_) => Ok(None),
    }
}

/// One page thumbnail of a fixed-page book, rendered on demand.
pub async fn books_page_thumb(id: i64, page: i64) -> Result<Option<String>> {
    let pool = books_pool().await?;
    let Some(row) = library::get(pool, id).await? else {
        return Ok(None);
    };
    Ok(
        render::page_thumb(Path::new(&row.path), &row.format, page.max(1) as usize - 1)
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
    )
}

fn events() -> &'static OnceLock<StreamSink<BooksEvent>> {
    static E: OnceLock<StreamSink<BooksEvent>> = OnceLock::new();
    &E
}

fn emit(e: BooksEvent) {
    if let Some(sink) = events().get() {
        let _ = sink.add(e);
    }
}

// ------------------------------------------------------------------ filter ---

/// The session's filters, as the query the domain crate understands.
fn filter_of(s: &Session) -> library::Filter {
    let status = match s.quick.as_str() {
        "reading" | "finished" | "unread" | "missing" => s.quick.clone(),
        _ => String::new(),
    };
    library::Filter {
        format: s.format.clone(),
        status: if s.view == "trash" {
            "trashed".into()
        } else {
            status
        },
        author: s.author.clone(),
        genre: s.genre.clone(),
        series: s.series.clone(),
        collection: s.collection,
        // A contents search restricts by id instead: the FTS table answers
        // "which books contain this", and the ordinary filter does the rest.
        query: if s.search_contents {
            String::new()
        } else {
            s.query.clone()
        },
        ids: Vec::new(),
        sort: library::Sort::from_index(s.sort_index),
    }
}

// ------------------------------------------------------------------ reader ---

/// Formats whose pages are pictures rather than text.
fn is_image_format(format: &str) -> bool {
    matches!(format, "pdf" | "cbz" | "cbr")
}

/// How many leaves the open book has — text pages or rendered images.
fn leaf_count(r: &ReaderSession) -> usize {
    if r.image_mode {
        r.image_pages
    } else {
        r.pages.len()
    }
}

/// Leaves per screen. One in single mode, two otherwise — and always one when
/// there is only one leaf to show.
fn per_screen(r: &ReaderSession) -> usize {
    if r.single { 1 } else { 2 }
}

fn screen_count(r: &ReaderSession) -> i64 {
    let leaves = leaf_count(r);
    if leaves == 0 {
        return 0;
    }
    ((leaves + per_screen(r) - 1) / per_screen(r)) as i64
}

/// The 0-based leaves shown on a 1-based screen page.
fn leaves_of(r: &ReaderSession, screen: i64) -> (Option<usize>, Option<usize>) {
    let leaves = leaf_count(r);
    if leaves == 0 {
        return (None, None);
    }
    let step = per_screen(r);
    let first = ((screen.max(1) - 1) as usize) * step;
    if first >= leaves {
        return (None, None);
    }
    if step == 1 {
        return (Some(first), None);
    }
    let second = if first + 1 < leaves {
        Some(first + 1)
    } else {
        None
    };
    // Right-to-left books put the lower-numbered leaf on the right; the reader
    // draws left and right, so the swap belongs here rather than in the widget.
    if r.rtl {
        (second, Some(first))
    } else {
        (Some(first), second)
    }
}

/// Load a book into the session. Everything expensive about opening a book
/// happens here, once, rather than on every page turn.
async fn open_book(id: i64) -> Result<()> {
    let pool = books_pool().await?;
    let Some(row) = library::get(pool, id).await? else {
        anyhow::bail!("book {id} is not in the library");
    };
    let path = PathBuf::from(&row.path);
    if !path.exists() {
        let mut s = lock();
        s.reader = ReaderSession {
            open: true,
            id,
            title: row.title.clone(),
            error: format!("The file has moved: {}", row.path),
            prefs: s.reader.prefs,
            ..Default::default()
        };
        return Ok(());
    }

    let image_mode = is_image_format(&row.format);
    let prefs = lock().reader.prefs;

    // The remembered view: -1 unset, 2 single. Anything else is a spread.
    let single = row.reader_view == 2;

    let mut r = ReaderSession {
        open: true,
        id,
        path: row.path.clone(),
        format: row.format.clone(),
        title: row.title.clone(),
        author: row.author.clone(),
        image_mode,
        rtl: row.rtl != 0,
        single,
        trim: false,
        screen: 1,
        prefs,
        ..Default::default()
    };

    if image_mode {
        r.image_pages = render::page_count(&path, &row.format).unwrap_or(0);
        r.toc = toc::pdf_outline(&path);
    } else {
        let chapters = tulipix_books::epub::load_chapters(&path).unwrap_or_default();
        r.chapter_titles = chapters.iter().map(|c| c.title.clone()).collect();
        let texts: Vec<String> = chapters.into_iter().map(|c| c.text).collect();
        r.pages = paginate::paginate(&texts, prefs.layout());
        r.toc = toc::load(&path).unwrap_or_default();
    }

    // Resume where the book was left. Progress stores a leaf, not a screen, so
    // that the same position survives switching between single and spread.
    if let Ok(Some(p)) = progress::get(pool, id).await {
        let leaves = leaf_count(&r);
        if leaves > 0 {
            let leaf = (p.page.max(0) as usize).min(leaves - 1);
            r.screen = (leaf / per_screen(&r)) as i64 + 1;
        }
    }

    if let Ok(Some(next)) = library::next_in_series(pool, id).await {
        r.next_id = next.id;
        r.next_title = next.title;
    }

    lock().reader = r;
    Ok(())
}

/// Write where we are. Called on every turn: a reader that only saves on close
/// loses the page when the process does.
async fn save_position() -> Result<()> {
    let (id, leaf, leaves, offset) = {
        let s = lock();
        let r = &s.reader;
        if !r.open || r.id == 0 {
            return Ok(());
        }
        let (a, b) = leaves_of(r, r.screen);
        let leaf = a.or(b).unwrap_or(0);
        let offset = r.pages.get(leaf).map(|p| p.char_start as i64).unwrap_or(0);
        (r.id, leaf as i64, leaf_count(r) as i64, offset)
    };
    let pool = books_pool().await?;
    // Session seconds are not tracked here yet — the Slint build counts them
    // from a timer that has no counterpart on this side. Zero adds nothing
    // rather than inventing time that was not read.
    progress::save(pool, id, leaf, leaves, offset, 0).await?;
    library::mark_read_today(pool).await.ok();
    Ok(())
}

/// Re-lay the text after a typography change, keeping the reader on the same
/// words rather than the same page number — the page number means something
/// different once the type has changed size.
fn repaginate(s: &mut Session) {
    let r = &mut s.reader;
    if !r.open || r.image_mode || r.pages.is_empty() {
        return;
    }
    let (a, b) = leaves_of(r, r.screen);
    let anchor = a
        .or(b)
        .and_then(|i| r.pages.get(i))
        .map(|p| p.char_start)
        .unwrap_or(0);

    // Re-splitting needs the chapter texts back. They are the pages' text,
    // concatenated per chapter — cheaper to keep than to re-open the zip.
    let mut chapters: Vec<String> = Vec::new();
    for page in &r.pages {
        if page.chapter >= chapters.len() {
            chapters.resize(page.chapter + 1, String::new());
        }
        if !chapters[page.chapter].is_empty() {
            chapters[page.chapter].push(' ');
        }
        chapters[page.chapter].push_str(&page.text);
    }
    r.pages = paginate::paginate(&chapters, r.prefs.layout());
    let leaf = paginate::page_at_offset(&r.pages, anchor);
    r.screen = (leaf / per_screen(r)) as i64 + 1;
}

/// The chapter a screen page is in, and the one after it.
fn chapter_names(r: &ReaderSession) -> (String, String) {
    if r.image_mode {
        // A fixed-page book's "chapter" is whichever outline entry is at or
        // before this leaf; the outline stores spine indices, so for a PDF the
        // nearest preceding entry is the honest answer.
        let leaf = leaves_of(r, r.screen).0.unwrap_or(0) as i32;
        let mut here = String::new();
        let mut next = String::new();
        for e in &r.toc {
            if e.chapter <= leaf {
                here = e.label.clone();
            } else {
                next = e.label.clone();
                break;
            }
        }
        return (here, next);
    }
    let leaf = leaves_of(r, r.screen).0.unwrap_or(0);
    let ch = r.pages.get(leaf).map(|p| p.chapter).unwrap_or(0);
    let here = r.chapter_titles.get(ch).cloned().unwrap_or_default();
    let next = r.chapter_titles.get(ch + 1).cloned().unwrap_or_default();
    (here, next)
}

/// True when this leaf opens its chapter — the only place the reader draws a
/// chapter title and its divider.
fn heading_for(r: &ReaderSession, leaf: Option<usize>) -> String {
    let Some(i) = leaf else { return String::new() };
    if r.image_mode {
        return String::new();
    }
    let Some(page) = r.pages.get(i) else {
        return String::new();
    };
    let opens = i == 0 || r.pages.get(i - 1).map(|p| p.chapter) != Some(page.chapter);
    if opens {
        r.chapter_titles
            .get(page.chapter)
            .cloned()
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// The rendered image for one leaf, at the current theme and trim.
fn leaf_image(r: &ReaderSession, leaf: Option<usize>) -> String {
    let Some(i) = leaf else { return String::new() };
    if !r.image_mode {
        return String::new();
    }
    let night = r.prefs.theme() == "dark";
    render::page_image_view(Path::new(&r.path), &r.format, i, night, r.trim)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

// ----------------------------------------------------------------- commands ---

async fn apply(cmd: BooksCmd) -> Result<()> {
    match cmd {
        BooksCmd::Refresh => {}

        // --- library ---
        BooksCmd::Search { text } => {
            let mut s = lock();
            s.query = text;
            s.page = 1;
        }
        BooksCmd::SetSearchContents { on } => {
            let mut s = lock();
            s.search_contents = on;
            s.page = 1;
        }
        BooksCmd::SetSort { index } => {
            let idx = index.clamp(0, 4) as usize;
            lock().sort_index = idx;
            tulipix_books::prefs::save_sort(idx);
        }
        BooksCmd::SetViewMode { mode } => {
            lock().view_mode = mode.clone();
            tulipix_books::prefs::save_view_mode(&mode);
        }
        BooksCmd::SetPage { page } => lock().page = page.max(1),
        BooksCmd::SetFormat { format } => {
            let mut s = lock();
            // Clicking the chip that is already on clears it — a filter row
            // with no way back out is a trap.
            s.format = if s.format == format {
                String::new()
            } else {
                format
            };
            s.page = 1;
        }
        BooksCmd::SetGenre { genre } => {
            let mut s = lock();
            s.genre = if s.genre == genre {
                String::new()
            } else {
                genre
            };
            s.page = 1;
        }
        BooksCmd::SetSeries { series } => {
            let mut s = lock();
            s.series = if s.series == series {
                String::new()
            } else {
                series
            };
            s.page = 1;
        }
        BooksCmd::SetAuthor { author } => {
            let mut s = lock();
            s.author = if s.author == author {
                String::new()
            } else {
                author
            };
            s.page = 1;
        }
        BooksCmd::SetQuick { quick } => {
            let mut s = lock();
            s.quick = quick;
            s.page = 1;
        }
        BooksCmd::SetCollection { id } => {
            let mut s = lock();
            s.collection = if s.collection == id { 0 } else { id };
            s.page = 1;
        }
        BooksCmd::ClearFilters => {
            let mut s = lock();
            s.format.clear();
            s.genre.clear();
            s.series.clear();
            s.author.clear();
            s.collection = 0;
            s.quick = "all".into();
            s.query.clear();
            s.page = 1;
        }
        BooksCmd::SetView { view } => {
            let mut s = lock();
            s.view = view;
            s.page = 1;
        }

        // --- one book ---
        BooksCmd::ToggleFavorite { id } => {
            library::toggle_favorite(books_pool().await?, id).await?;
        }
        BooksCmd::ToggleMagazine { id } => {
            library::toggle_magazine(books_pool().await?, id).await?;
        }
        BooksCmd::ToggleRtl { id } => {
            let on = library::toggle_rtl(books_pool().await?, id).await?;
            let mut s = lock();
            if s.reader.open && s.reader.id == id {
                s.reader.rtl = on;
            }
        }
        BooksCmd::SetRating { id, rating } => {
            library::set_rating(books_pool().await?, id, rating.clamp(0.0, 5.0)).await?;
        }
        BooksCmd::Trash { id } => {
            library::trash(books_pool().await?, id).await?;
            lock().status = "Moved to trash.".into();
        }
        BooksCmd::Restore { id } => {
            library::restore(books_pool().await?, id).await?;
            lock().status = "Restored.".into();
        }
        BooksCmd::DeletePerm { id } => {
            library::remove(books_pool().await?, id).await?;
            let mut s = lock();
            if s.detail_id == id {
                s.detail_id = 0;
            }
            s.status = "Removed from the library.".into();
        }
        BooksCmd::EmptyTrash => {
            let pool = books_pool().await?;
            let f = library::Filter {
                status: "trashed".into(),
                ..Default::default()
            };
            let (rows, _) = library::list_page(pool, &f, 10_000, 0).await?;
            let n = rows.len();
            for row in rows {
                library::remove(pool, row.id).await.ok();
            }
            lock().status = format!("Emptied the trash — {n} removed.");
        }
        BooksCmd::Relink { id, path } => {
            library::relink(books_pool().await?, id, path.trim()).await?;
            lock().status = "Relinked.".into();
        }
        BooksCmd::OpenDetail { id } => lock().detail_id = id,
        BooksCmd::CloseDetail => lock().detail_id = 0,
        BooksCmd::FetchSummary { id } => {
            let pool = books_pool().await?;
            let Some(row) = library::get(pool, id).await? else {
                anyhow::bail!("book {id} is not in the library");
            };
            let meta = tulipix_books::summary::fetch_meta(&row.title, &row.author).await;
            library::set_metadata(pool, id, &meta.summary, &meta.published, meta.rating).await?;
            lock().status = if meta.summary.is_empty() {
                "No description found.".into()
            } else {
                "Description updated.".into()
            };
        }

        // --- collections ---
        BooksCmd::CollectionCreate { name } => {
            library::collection_create(books_pool().await?, &name).await?;
        }
        BooksCmd::CollectionRename { id, name } => {
            library::collection_rename(books_pool().await?, id, &name).await?;
        }
        BooksCmd::CollectionDelete { id } => {
            let pool = books_pool().await?;
            library::collection_delete(pool, id).await?;
            let mut s = lock();
            if s.collection == id {
                s.collection = 0;
            }
        }
        BooksCmd::CollectionToggle { id, book_id } => {
            let pool = books_pool().await?;
            let mine = library::book_collections(pool, book_id)
                .await
                .unwrap_or_default();
            if mine.contains(&id) {
                library::collection_remove(pool, id, book_id).await?;
            } else {
                library::collection_add(pool, id, book_id).await?;
            }
        }
        BooksCmd::SmartCreateFromFilters { name } => {
            let f = filter_of(&lock());
            library::smart_create(books_pool().await?, &name, &f).await?;
            lock().status = "Saved as a smart collection.".into();
        }
        BooksCmd::SmartApply { id } => {
            let smart = library::smart_list(books_pool().await?).await?;
            if let Some((_, _, f)) = smart.into_iter().find(|(sid, _, _)| *sid == id) {
                let mut s = lock();
                s.format = f.format;
                s.genre = f.genre;
                s.series = f.series;
                s.author = f.author;
                s.collection = f.collection;
                s.query = f.query;
                s.quick = if f.status.is_empty() {
                    "all".into()
                } else {
                    f.status
                };
                s.page = 1;
            }
        }
        BooksCmd::SmartDelete { id } => {
            library::smart_delete(books_pool().await?, id).await?;
        }

        // --- library maintenance ---
        BooksCmd::AddFolder { path } => {
            let pool = books_pool().await?;
            scan::add_folder(pool, path.trim()).await?;
            run_scan(pool).await?;
        }
        BooksCmd::RemoveFolder { path } => {
            scan::remove_folder(books_pool().await?, path.trim()).await?;
            lock().status = "Folder removed.".into();
        }
        BooksCmd::Scan => {
            let pool = books_pool().await?;
            run_scan(pool).await?;
        }
        BooksCmd::BuildArt => {
            let n = scan::build_art(books_pool().await?, 200).await?;
            lock().status = format!("Built cover art for {n} books.");
        }
        BooksCmd::IndexContents => {
            scan::index_contents(books_pool().await?).await?;
            lock().status = "Indexed book contents for search.".into();
        }

        // --- statistics ---
        BooksCmd::OpenStats => lock().stats_open = true,
        BooksCmd::CloseStats => lock().stats_open = false,

        // --- the reader ---
        BooksCmd::OpenBook { id } => open_book(id).await?,
        BooksCmd::CloseReader => {
            save_position().await.ok();
            let mut s = lock();
            s.reader = ReaderSession {
                prefs: s.reader.prefs,
                ..Default::default()
            };
        }
        BooksCmd::ReaderNext => {
            {
                let mut s = lock();
                let last = screen_count(&s.reader);
                if s.reader.screen < last {
                    s.reader.screen += 1;
                }
            }
            save_position().await.ok();
        }
        BooksCmd::ReaderPrev => {
            {
                let mut s = lock();
                if s.reader.screen > 1 {
                    s.reader.screen -= 1;
                }
            }
            save_position().await.ok();
        }
        BooksCmd::ReaderJump { page } => {
            {
                let mut s = lock();
                let last = screen_count(&s.reader).max(1);
                s.reader.screen = page.clamp(1, last);
            }
            save_position().await.ok();
        }
        BooksCmd::ReaderChapter { index } => {
            {
                let mut s = lock();
                let step = per_screen(&s.reader);
                if s.reader.image_mode {
                    let leaf = index.max(0) as usize;
                    s.reader.screen = (leaf / step) as i64 + 1;
                } else {
                    let leaf = paginate::page_of_chapter(&s.reader.pages, index.max(0) as usize);
                    s.reader.screen = (leaf / step) as i64 + 1;
                }
            }
            save_position().await.ok();
        }
        BooksCmd::ReaderNextChapter => {
            step_chapter(true);
            save_position().await.ok();
        }
        BooksCmd::ReaderPrevChapter => {
            step_chapter(false);
            save_position().await.ok();
        }
        BooksCmd::ReaderOpenNext => {
            let next = lock().reader.next_id;
            if next != 0 {
                save_position().await.ok();
                open_book(next).await?;
            }
        }
        BooksCmd::MarkFinished => {
            let id = lock().reader.id;
            if id != 0 {
                sqlx::query("UPDATE books SET finished = 1 WHERE id = ?")
                    .bind(id)
                    .execute(books_pool().await?)
                    .await?;
                lock().status = "Marked as finished.".into();
            }
        }
        BooksCmd::ToggleBookmark => {
            let (id, leaf, offset) = {
                let s = lock();
                let leaf = leaves_of(&s.reader, s.reader.screen).0.unwrap_or(0);
                let offset = s
                    .reader
                    .pages
                    .get(leaf)
                    .map(|p| p.char_start as i64)
                    .unwrap_or(0);
                (s.reader.id, leaf as i64, offset)
            };
            if id != 0 {
                progress::toggle_bookmark(books_pool().await?, id, leaf, offset).await?;
            }
        }
        BooksCmd::BookmarkJump { page } => {
            {
                let mut s = lock();
                let step = per_screen(&s.reader);
                s.reader.screen = (page.max(0) as usize / step) as i64 + 1;
            }
            save_position().await.ok();
        }
        BooksCmd::BookmarkRemove { id } => {
            progress::remove_bookmark(books_pool().await?, id).await?;
        }
        BooksCmd::AnnotAdd {
            snippet,
            note,
            color,
        } => {
            let (id, leaf, start) = {
                let s = lock();
                let leaf = leaves_of(&s.reader, s.reader.screen).0.unwrap_or(0);
                let start = s
                    .reader
                    .pages
                    .get(leaf)
                    .map(|p| p.char_start as i64)
                    .unwrap_or(0);
                (s.reader.id, leaf as i64, start)
            };
            if id != 0 {
                let pool = books_pool().await?;
                let end = start + snippet.chars().count() as i64;
                let aid = annotations::add(pool, id, leaf, start, end, &snippet, &color).await?;
                if !note.trim().is_empty() {
                    annotations::set_note(pool, aid, note.trim()).await?;
                }
            }
        }
        BooksCmd::AnnotSetNote { id, note } => {
            annotations::set_note(books_pool().await?, id, &note).await?;
        }
        BooksCmd::AnnotRemove { id } => {
            annotations::remove(books_pool().await?, id).await?;
        }
        BooksCmd::ReaderSearch { query } => {
            let mut s = lock();
            s.reader.search_query = query.clone();
            s.reader.hits = search_pages(&s.reader, &query);
        }
        BooksCmd::LoadThumbs => {
            let (path, format, total) = {
                let s = lock();
                (
                    s.reader.path.clone(),
                    s.reader.format.clone(),
                    s.reader.image_pages,
                )
            };
            if !path.is_empty() {
                // Capped: a 900-page scan would otherwise render nine hundred
                // JPEGs before the panel drew anything.
                let mut out = Vec::new();
                for i in 0..total.min(THUMB_CAP) {
                    if let Ok(p) = render::page_thumb(Path::new(&path), &format, i) {
                        out.push((i as i64 + 1, p.to_string_lossy().to_string()));
                    }
                }
                lock().reader.thumbs = out;
            }
        }
        BooksCmd::SetSingle { on } => {
            let id = {
                let mut s = lock();
                let leaf = leaves_of(&s.reader, s.reader.screen).0.unwrap_or(0);
                s.reader.single = on;
                let step = per_screen(&s.reader);
                s.reader.screen = (leaf / step) as i64 + 1;
                s.reader.id
            };
            if id != 0 {
                library::set_reader_view(books_pool().await?, id, if on { 2 } else { 0 }).await?;
            }
        }
        BooksCmd::SetTrim { on } => lock().reader.trim = on,
        BooksCmd::SetFontPx { px } => {
            let mut s = lock();
            s.reader.prefs.font_px = (px as f32).clamp(12.0, 32.0);
            save_prefs(s.reader.prefs);
            repaginate(&mut s);
        }
        BooksCmd::SetLineSpacing { index } => {
            let mut s = lock();
            s.reader.prefs.line_index = index.clamp(0, 2) as u8;
            save_prefs(s.reader.prefs);
            repaginate(&mut s);
        }
        BooksCmd::SetMargin { index } => {
            let mut s = lock();
            s.reader.prefs.margin_index = index.clamp(0, 2) as u8;
            save_prefs(s.reader.prefs);
            repaginate(&mut s);
        }
        BooksCmd::SetTypeface { index } => {
            let mut s = lock();
            s.reader.prefs.typeface = index.clamp(0, 2) as u8;
            save_prefs(s.reader.prefs);
            repaginate(&mut s);
        }
        BooksCmd::SetAlign { index } => {
            let mut s = lock();
            s.reader.prefs.align = index.clamp(0, 2) as u8;
            save_prefs(s.reader.prefs);
        }
        BooksCmd::ToggleBold => {
            let mut s = lock();
            s.reader.prefs.bold = !s.reader.prefs.bold;
            save_prefs(s.reader.prefs);
        }
        BooksCmd::SetTheme { theme } => {
            let mut s = lock();
            s.reader.prefs.theme_index = match theme.as_str() {
                "sepia" => 1,
                "dark" => 2,
                _ => 0,
            };
            save_prefs(s.reader.prefs);
        }
        BooksCmd::SetBrightness { value } => {
            let mut s = lock();
            s.reader.prefs.brightness = (value as f32).clamp(0.4, 1.0);
            save_prefs(s.reader.prefs);
        }
    }
    Ok(())
}

/// Move to the start of the neighbouring chapter.
fn step_chapter(forward: bool) {
    let mut s = lock();
    let step = per_screen(&s.reader);
    let leaf = leaves_of(&s.reader, s.reader.screen).0.unwrap_or(0);
    let here = s.reader.pages.get(leaf).map(|p| p.chapter).unwrap_or(0);
    let target = if forward {
        here + 1
    } else {
        here.saturating_sub(1)
    };
    let to = paginate::page_of_chapter(&s.reader.pages, target);
    s.reader.screen = (to / step) as i64 + 1;
}

/// Thumbnails are rendered eagerly when the panel opens; this is where that
/// stops being reasonable.
const THUMB_CAP: usize = 200;

pub(crate) async fn run_scan(pool: &sqlx::SqlitePool) -> Result<()> {
    let report = scan::scan_all_progress(pool, |done, total, name| {
        emit(BooksEvent::ScanProgress {
            done: done as i64,
            total: total as i64,
            name: name.to_string(),
        });
    })
    .await?;
    scan::reconcile_missing(pool).await.ok();
    emit(BooksEvent::ScanFinished {
        added: report.added as i64,
        updated: report.updated as i64,
        missing: report.missing as i64,
    });
    lock().status = format!(
        "Scan finished — {} added, {} updated, {} missing.",
        report.added, report.updated, report.missing
    );
    Ok(())
}

/// Find a phrase in the laid-out pages. Case-insensitive, and it reports the
/// screen page rather than the leaf, because that is what a click has to jump
/// to.
fn search_pages(r: &ReaderSession, query: &str) -> Vec<(i64, String)> {
    let needle = query.trim().to_lowercase();
    if needle.len() < 2 || r.image_mode {
        return Vec::new();
    }
    let step = per_screen(r);
    let mut out = Vec::new();
    for (i, page) in r.pages.iter().enumerate() {
        let hay = page.text.to_lowercase();
        let Some(at) = hay.find(&needle) else {
            continue;
        };
        // Snippet by chars, not bytes: slicing a multi-byte character in half
        // panics, and book text is full of them.
        let chars: Vec<char> = page.text.chars().collect();
        let approx = page.text[..at].chars().count();
        let from = approx.saturating_sub(40);
        let to = (approx + needle.chars().count() + 60).min(chars.len());
        let snippet: String = chars[from..to].iter().collect();
        out.push(((i / step) as i64 + 1, snippet.trim().to_string()));
        if out.len() >= SEARCH_HITS {
            break;
        }
    }
    out
}

const SEARCH_HITS: usize = 100;

// ---------------------------------------------------------------- snapshot ---

/// One screenful of the grid. Books are big tiles; a hundred of them is a lot
/// of covers to hold and none of them is on screen.
const PAGE_SIZE: i64 = 24;

fn to_book(r: library::BookRow) -> Book {
    Book {
        id: r.id,
        title: r.title,
        author: r.author,
        series: r.series,
        genre: r.genre,
        format: r.format,
        cover: r.cover_path,
        published: r.published,
        // The column is 0..100; every ring and bar that draws it wants 0..1.
        percent: (r.percent / 100.0).clamp(0.0, 1.0),
        favorite: r.favorite != 0,
        finished: r.finished != 0,
        missing: r.missing != 0,
        trashed: r.trashed != 0,
        magazine: r.magazine != 0,
        rtl: r.rtl != 0,
        rating: r.rating,
        net_rating: r.net_rating,
        size_bytes: r.size_bytes,
        added_at: r.added_at,
        last_read: r.last_read,
        time_read: r.time_read,
    }
}

/// "about 3 h left" — from this book's own pace where there is one, and from a
/// reader's-average page rate where there is not.
fn time_left(page: i64, total: i64, secs_read: i64) -> String {
    if total <= 0 || page >= total {
        return String::new();
    }
    let left = total - page;
    let per_page = if page > 3 && secs_read > 0 {
        secs_read as f64 / page as f64
    } else {
        90.0
    };
    let secs = (left as f64 * per_page) as i64;
    if secs < 3600 {
        format!("about {} min left", (secs / 60).max(1))
    } else {
        format!("about {:.1} h left", secs as f64 / 3600.0)
    }
}

fn to_hero(row: library::BookRow, page: i64, total: i64) -> HeroBook {
    let secs = row.time_read;
    HeroBook {
        id: row.id,
        title: row.title,
        author: row.author,
        cover: row.cover_path,
        page,
        total,
        percent: (row.percent / 100.0).clamp(0.0, 1.0),
        time_left: time_left(page, total, secs),
    }
}

async fn snapshot() -> Result<BooksState> {
    let pool = books_pool().await?;
    let s = {
        let g = lock();
        Session {
            view: g.view.clone(),
            query: g.query.clone(),
            search_contents: g.search_contents,
            sort_index: g.sort_index,
            view_mode: g.view_mode.clone(),
            page: g.page,
            format: g.format.clone(),
            genre: g.genre.clone(),
            series: g.series.clone(),
            author: g.author.clone(),
            quick: g.quick.clone(),
            collection: g.collection,
            detail_id: g.detail_id,
            stats_open: g.stats_open,
            status: g.status.clone(),
            reader: ReaderSession::default(),
        }
    };

    let mut filter = filter_of(&s);
    // "favorite" is not a status the domain filter knows; it is a column, and
    // the cheapest honest way to express it is an id restriction.
    if s.quick == "favorite" {
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM books WHERE favorite = 1 AND trashed = 0")
                .fetch_all(pool)
                .await
                .unwrap_or_default();
        filter.ids = if ids.is_empty() { vec![-1] } else { ids };
    }
    if s.search_contents && !s.query.trim().is_empty() {
        let ids = library::fts_search(pool, s.query.trim())
            .await
            .unwrap_or_default();
        filter.ids = if ids.is_empty() { vec![-1] } else { ids };
    }

    let offset = (s.page - 1).max(0) * PAGE_SIZE;
    let (rows, filtered_total) = library::list_page(pool, &filter, PAGE_SIZE, offset).await?;
    let page_count = ((filtered_total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);

    let home = home::load(pool).await.unwrap_or(home::HomeData {
        continue_reading: None,
        slider: Vec::new(),
        recently_added: Vec::new(),
        in_progress: Vec::new(),
        stats: home::Stats::default(),
    });

    let chips = |v: Vec<(String, i64)>| -> Vec<FacetChip> {
        v.into_iter()
            .filter(|(k, _)| !k.is_empty())
            .map(|(k, n)| FacetChip {
                key: k.clone(),
                label: k,
                count: n,
            })
            .collect()
    };

    let collections: Vec<CollectionRow> = library::collections(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, count)| CollectionRow {
            id,
            name,
            count,
            member: false,
        })
        .collect();

    let smart: Vec<CollectionRow> = library::smart_list(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        // Count is not knowable without running each saved filter; -1 says so
        // rather than showing a zero that looks like an empty collection.
        .map(|(id, name, _)| CollectionRow {
            id,
            name,
            count: -1,
            member: false,
        })
        .collect();

    // The detail sheet, and which collections its book is already in.
    let (detail, detail_summary, detail_collections) = if s.detail_id != 0 {
        let row = library::get(pool, s.detail_id).await.ok().flatten();
        let mine = library::book_collections(pool, s.detail_id)
            .await
            .unwrap_or_default();
        let with_membership = collections
            .iter()
            .map(|c| CollectionRow {
                id: c.id,
                name: c.name.clone(),
                count: c.count,
                member: mine.contains(&c.id),
            })
            .collect();
        let summary = row.as_ref().map(|r| r.summary.clone()).unwrap_or_default();
        (row.map(to_book), summary, with_membership)
    } else {
        (None, String::new(), Vec::new())
    };

    let reading_stats = if s.stats_open {
        Some(reading_stats(pool).await)
    } else {
        None
    };

    let reader = reader_view(pool).await;

    Ok(BooksState {
        view: s.view,
        books: rows.into_iter().map(to_book).collect(),
        total: home.stats.total,
        filtered_total,
        page: s.page,
        page_count,
        view_mode: s.view_mode,
        sort_index: s.sort_index as i64,
        query: s.query,
        search_contents: s.search_contents,

        hero: home
            .continue_reading
            .map(|(row, page, total)| to_hero(row, page, total)),
        slider: home
            .slider
            .into_iter()
            .map(|(row, page, total)| to_hero(row, page, total))
            .collect(),
        stats: LibraryStats {
            total: home.stats.total,
            authors: home.stats.authors,
            finished: home.stats.finished,
            in_progress: home.stats.in_progress,
            hours_read: home.stats.hours_read,
            added_month: home.stats.added_month,
            authors_month: home.stats.authors_month,
        },

        formats: chips(library::format_counts(pool).await.unwrap_or_default()),
        genres: chips(library::genre_counts(pool).await.unwrap_or_default()),
        series: chips(library::series_counts(pool).await.unwrap_or_default()),
        collections,
        smart,

        active_format: s.format,
        active_genre: s.genre,
        active_series: s.series,
        active_author: s.author,
        active_quick: s.quick,
        active_collection: s.collection,

        trashed_count: library::trashed_count(pool).await.unwrap_or(0),
        folders: scan::folders(pool).await.unwrap_or_default(),

        detail,
        detail_summary,
        detail_collections,

        reading_stats,
        reader,
        status: s.status,
    })
}

/// The statistics panel: totals, a year of reading days, and where the hours
/// went.
async fn reading_stats(pool: &sqlx::SqlitePool) -> ReadingStats {
    let (secs, _days, finished, started) =
        library::reading_totals(pool).await.unwrap_or((0, 0, 0, 0));
    let days = library::reading_days(pool).await.unwrap_or_default();
    let books = library::time_per_book(pool, 8)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(title, secs)| StatBook {
            title,
            time: fmt_duration(secs),
        })
        .collect();

    let today = tulipix_books::schema::now() / 86400;
    let set: std::collections::HashSet<i64> = days.iter().map(|d| d / 86400).collect();
    let heat: Vec<bool> = (0..365).map(|i| set.contains(&(today - 364 + i))).collect();

    // A streak is consecutive days ending today or yesterday — missing today
    // before bedtime should not read as a broken streak.
    let mut streak = 0i64;
    let mut cursor = if set.contains(&today) {
        today
    } else {
        today - 1
    };
    while set.contains(&cursor) {
        streak += 1;
        cursor -= 1;
    }

    ReadingStats {
        hours: secs as f64 / 3600.0,
        days: set.len() as i64,
        finished,
        started,
        streak,
        heat,
        books,
    }
}

fn fmt_duration(secs: i64) -> String {
    if secs < 3600 {
        format!("{} min", (secs / 60).max(1))
    } else {
        format!("{:.1} h", secs as f64 / 3600.0)
    }
}

/// The reader half of the snapshot. Cheap when nothing is open, and never more
/// than the two leaves on screen when something is.
async fn reader_view(pool: &sqlx::SqlitePool) -> Reader {
    let (r_open, id) = {
        let g = lock();
        (g.reader.open, g.reader.id)
    };
    if !r_open {
        let prefs = lock().reader.prefs;
        return empty_reader(prefs);
    }

    let bookmarks: Vec<BookmarkRow> = progress::bookmarks(pool, id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|b| BookmarkRow {
            id: b.id,
            page: b.page,
            note: b.note,
            color: b.color,
            created_at: b.created_at,
        })
        .collect();

    let notes: Vec<NoteRow> = annotations::for_book(pool, id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| NoteRow {
            id: a.id,
            page: a.page,
            snippet: a.snippet,
            note: a.note,
            color: a.color,
            created_at: a.created_at,
        })
        .collect();

    let magazine = library::get(pool, id)
        .await
        .ok()
        .flatten()
        .map(|b| b.magazine != 0)
        .unwrap_or(false);

    let g = lock();
    let r = &g.reader;
    let (left, right) = leaves_of(r, r.screen);
    let (chapter_name, next_chapter_name) = chapter_names(r);
    let leaves = leaf_count(r) as i64;
    let first_leaf = left.or(right).unwrap_or(0) as i64;

    Reader {
        open: true,
        id: r.id,
        title: r.title.clone(),
        author: r.author.clone(),
        format: r.format.clone(),
        image_mode: r.image_mode,
        page: r.screen,
        page_count: screen_count(r),
        percent: if leaves > 0 {
            ((first_leaf + 1) as f64 / leaves as f64).clamp(0.0, 1.0)
        } else {
            0.0
        },
        chapter_name,
        next_chapter_name,
        left_text: left
            .and_then(|i| r.pages.get(i))
            .map(|p| p.text.clone())
            .unwrap_or_default(),
        right_text: right
            .and_then(|i| r.pages.get(i))
            .map(|p| p.text.clone())
            .unwrap_or_default(),
        left_heading: heading_for(r, left),
        right_heading: heading_for(r, right),
        left_image: leaf_image(r, left),
        right_image: leaf_image(r, right),
        left_folio: left.map(|i| i as i64 + 1).unwrap_or(0),
        right_folio: right.map(|i| i as i64 + 1).unwrap_or(0),
        rtl: r.rtl,
        single: r.single,
        trim: r.trim,
        bookmarked: bookmarks.iter().any(|b| b.page == first_leaf),
        has_note: notes.iter().any(|n| n.page == first_leaf),
        toc: r
            .toc
            .iter()
            .map(|e| TocItem {
                label: e.label.clone(),
                depth: e.depth as i64,
                chapter: e.chapter as i64,
            })
            .collect(),
        bookmarks,
        notes,
        search_query: r.search_query.clone(),
        search_results: r
            .hits
            .iter()
            .map(|(page, snippet)| SearchHit {
                page: *page,
                snippet: snippet.clone(),
            })
            .collect(),
        // A magazine has no text worth searching, and the user says which books
        // those are.
        search_enabled: !r.image_mode && !magazine,
        thumbs: r
            .thumbs
            .iter()
            .map(|(page, path)| ThumbRow {
                page: *page,
                path: path.clone(),
            })
            .collect(),
        prefs: ReaderPrefs {
            font_px: r.prefs.font_px as f64,
            line_index: r.prefs.line_index as i64,
            margin_index: r.prefs.margin_index as i64,
            typeface: r.prefs.typeface as i64,
            align: r.prefs.align as i64,
            bold: r.prefs.bold,
            theme: r.prefs.theme().to_string(),
            brightness: r.prefs.brightness as f64,
        },
        next_title: r.next_title.clone(),
        next_id: r.next_id,
        error: r.error.clone(),
    }
}

fn empty_reader(prefs: Prefs) -> Reader {
    Reader {
        open: false,
        id: 0,
        title: String::new(),
        author: String::new(),
        format: String::new(),
        image_mode: false,
        page: 0,
        page_count: 0,
        percent: 0.0,
        chapter_name: String::new(),
        next_chapter_name: String::new(),
        left_text: String::new(),
        right_text: String::new(),
        left_heading: String::new(),
        right_heading: String::new(),
        left_image: String::new(),
        right_image: String::new(),
        left_folio: 0,
        right_folio: 0,
        rtl: false,
        single: false,
        trim: false,
        bookmarked: false,
        has_note: false,
        toc: Vec::new(),
        bookmarks: Vec::new(),
        notes: Vec::new(),
        search_query: String::new(),
        search_results: Vec::new(),
        search_enabled: false,
        thumbs: Vec::new(),
        prefs: ReaderPrefs {
            font_px: prefs.font_px as f64,
            line_index: prefs.line_index as i64,
            margin_index: prefs.margin_index as i64,
            typeface: prefs.typeface as i64,
            align: prefs.align as i64,
            bold: prefs.bold,
            theme: prefs.theme().to_string(),
            brightness: prefs.brightness as f64,
        },
        next_title: String::new(),
        next_id: 0,
        error: String::new(),
    }
}
