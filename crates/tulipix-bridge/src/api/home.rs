//! Home: the landing page.
//!
//! One snapshot of every section at once -- counts, what is in progress, and
//! the newest few things in each library. Every figure is a count against that
//! section's own database, so Home and the section it names cannot disagree.
//!
//! Covers are not resolved here. Each row carries the id its section's own
//! lazy resolver takes (`photos_ensure_thumb`, `videos_ensure_thumb`,
//! `books_ensure_cover`, `music_ensure_art`), and the tile asks for one as it
//! scrolls in -- the same shape every grid in this port uses, and the reason a
//! cold Home paints immediately instead of after a hundred ffmpeg runs.

use flutter_rust_bridge::frb;
use anyhow::Result;
use sqlx::SqlitePool;

/// Cards in the Continue strip. Four fixed slots, as in the Slint page.
const CONTINUE_SLOTS: usize = 4;

/// Titles longer than this have nowhere to go on a card.
const TITLE_CHARS: usize = 50;

/// Rows per section shelf.
const SHELF: i64 = 12;

pub struct HomeCounts {
    pub photos: i64,
    pub photos_albums: i64,
    pub videos: i64,
    pub videos_shows: i64,
    pub songs: i64,
    pub podcasts: i64,
    pub audiobooks: i64,
    pub radio: i64,
    pub books: i64,
    pub books_reading: i64,
    pub cloud_remotes: i64,
    pub tools_running: i64,
    pub tools_queued: i64,
    pub finances_due: i64,
}

/// One in-progress item. `kind` is "book" | "podcast" | "audiobook" | "video",
/// and it decides both which resolver draws the cover and where a click goes.
pub struct HomeContinue {
    pub kind: String,
    pub title: String,
    pub author: String,
    pub sub: String,
    /// 0..1; negative means unknown, and the bar is hidden rather than drawn
    /// at zero under something that is clearly part-way through.
    pub frac: f64,
    pub id: i64,
    pub path: String,
}

/// One tile on a section shelf.
pub struct HomeTile {
    pub id: i64,
    pub label: String,
    pub sub: String,
}

pub struct HomeRemote {
    pub name: String,
    pub backend: String,
    pub usage: String,
}

pub struct HomeQuote {
    pub text: String,
    pub author: String,
}

/// The newest in-progress item, spelled out for Cinema's backdrop. Nothing
/// queries for this -- it is the first row the Continue strip already gathered,
/// so the hero and the rail cannot disagree about what you were last doing.
pub struct HomeHero {
    pub kind: String,
    pub title: String,
    pub kicker: String,
    pub meta: String,
    pub frac: f64,
    pub id: i64,
    pub path: String,
}

/// One row of the Stream layout's activity feed.
pub struct HomeEvent {
    /// "19:42".
    pub at: String,
    /// "11-08-26" -- the clock alone never said which day a row belonged to
    /// once the feed reached past yesterday.
    pub day: String,
    /// "NOW" | "TODAY" | "YESTERDAY" | "EARLIER".
    pub group: String,
    /// Draw the group caption above this row.
    pub head: bool,
    pub section: String,
    /// added | played | resumed | episode | job | sent | received | paid.
    pub kind: String,
    pub title: String,
    pub sub: String,
    /// Pill label; empty means no pill. Only set where there is a concrete
    /// thing to resume -- the row itself already opens the section, and two
    /// controls doing one job is noise.
    pub action: String,
    pub alarm: bool,
    pub id: i64,
    pub path: String,
}

/// One obligation on Cinema's money panel.
pub struct HomeDue {
    pub name: String,
    pub amount: String,
    /// DD-MM-YY, which is what the rest of this app's dates look like.
    pub due: String,
    pub late: bool,
}

pub struct HomeState {
    pub greeting: String,
    pub date_line: String,
    /// "12,304 items · 41 GB".
    pub library_line: String,
    pub quote: HomeQuote,
    pub counts: HomeCounts,
    /// "all" | "video" | "book" | "podcast" | "audiobook".
    pub continue_filter: String,
    pub continue_rows: Vec<HomeContinue>,
    pub recent_photos: Vec<HomeTile>,
    pub recent_videos: Vec<HomeTile>,
    pub recent_books: Vec<HomeTile>,
    pub remotes: Vec<HomeRemote>,
    pub recent_songs: Vec<HomeTile>,
    pub hero: HomeHero,
    /// Which layout is chosen: "classic" | "welcome" | "cinema" | "stream".
    pub layout: String,
    /// The card keys Settings left switched on. A layout draws only these.
    pub cards: Vec<String>,
    // Stream's feed.
    /// "all" | "media" | "money" | "devices".
    pub feed_filter: String,
    pub feed_note: String,
    pub events: Vec<HomeEvent>,
    // The figures Cinema's and Stream's panels print.
    pub tool_count: i64,
    pub transfer_inbox: String,
    pub fin_month: String,
    pub fin_spent: String,
    /// Twelve months of spend as 0..1 heights, oldest first.
    pub fin_months: Vec<f64>,
    pub fin_month_labels: Vec<String>,
    pub fin_dues: Vec<HomeDue>,
    pub busy: bool,
}

pub enum HomeCmd {
    Refresh,
    SetContinueFilter { filter: String },
    /// Stream's feed key: all | media | money | devices.
    SetFeedFilter { filter: String },
    /// Hide one Continue card. Kept for the session only -- the Slint build
    /// does the same, because a dismissal that outlived the app would silently
    /// hide a book someone came back to a week later.
    DismissContinue { kind: String, id: i64, path: String },
}

pub async fn home_dispatch(cmd: HomeCmd) -> Result<HomeState> {
    match cmd {
        HomeCmd::Refresh => {}
        HomeCmd::SetContinueFilter { filter } => set_filter(filter),
        HomeCmd::SetFeedFilter { filter } => set_feed_filter(filter),
        HomeCmd::DismissContinue { kind, id, path } => {
            if let Ok(mut g) = dismissed().lock() {
                g.insert(dismiss_key(&kind, id, &path));
            }
        }
    }
    snapshot().await
}

async fn snapshot() -> Result<HomeState> {
    let (counts, rows, photos, videos, books, remotes, songs, events, money) = tokio::join!(
        counts(),
        continue_rows(),
        recent_photos(),
        recent_videos(),
        recent_books(),
        remotes(),
        recent_songs(),
        // Only the Stream layout draws this, and it is seven queries -- so it
        // is gathered only when that layout is the one on screen.
        feed(),
        money(),
    );
    let filter = filter();
    let settings = crate::api::shell::load();
    let shown = filtered(rows, &filter);
    Ok(HomeState {
        hero: hero(shown.first()),
        layout: match settings.text("home.layout") {
            l if l.is_empty() => "classic".into(),
            l => l,
        },
        cards: crate::api::settings::enabled_cards(&settings),
        feed_note: feed_note(&events),
        feed_filter: feed_filter(),
        events,
        recent_songs: songs,
        tool_count: crate::api::tools::op_count(),
        transfer_inbox: settings.text("transfer.inbox"),
        fin_month: money.month,
        fin_spent: money.spent,
        fin_months: money.months,
        fin_month_labels: money.labels,
        fin_dues: money.dues,
        greeting: greeting(),
        date_line: chrono::Local::now().format("%A, %B %-d").to_string(),
        library_line: library_line().await,
        quote: quote(),
        counts,
        continue_rows: shown,
        continue_filter: filter,
        recent_photos: photos,
        recent_videos: videos,
        recent_books: books,
        remotes,
        busy: false,
    })
}

// ── the header ──────────────────────────────────────────────────────────────

fn greeting() -> String {
    use chrono::Timelike;
    let name = crate::api::shell::load().text("profile.name");
    let who = match name.trim() {
        "" => "there".to_string(),
        n => n.to_string(),
    };
    match chrono::Local::now().hour() {
        5..=11 => format!("Good Morning, {who}"),
        12..=16 => format!("Good Afternoon, {who}"),
        _ => format!("Good Evening, {who}"),
    }
}

/// The quote shelf, shuffled once per run rather than per landing -- walking
/// back to Home should not restart the rotation.
fn quote() -> HomeQuote {
    use std::sync::OnceLock;
    static SHELF: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    let all = SHELF.get_or_init(tulipix_common::quotes::shuffled);
    if all.is_empty() {
        return HomeQuote { text: String::new(), author: String::new() };
    }
    // One a day, from a shelf that is already in a random order: the same quote
    // all day is a fixture you can come to like, and a new one per repaint is
    // noise.
    let day = (now_secs() / 86_400) as usize;
    let (text, author) = all[day % all.len()];
    HomeQuote { text: text.to_string(), author: author.to_string() }
}

pub(crate) fn now_secs() -> i64 {
    tulipix_core::util::unix_secs_i64()
}

/// "12,304 items · 41 GB", summed across every section that owns an `items`
/// table. The sidebar prints the same line under the profile name.
pub(crate) async fn library_line() -> String {
    let mut items = 0i64;
    let mut bytes = 0i64;
    for pool in section_pools().await {
        let (n, b) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM items WHERE missing_since IS NULL",
        )
        .fetch_one(pool)
        .await
        .unwrap_or((0, 0));
        items += n;
        bytes += b;
    }
    format!("{} items · {}", thousands(items), tulipix_core::util::human_bytes(bytes as u64))
}

/// The three databases that carry an `items` table. Music's four pools share
/// one: podcasts, radio and YouTube hold no library files.
async fn section_pools() -> Vec<&'static SqlitePool> {
    let mut out = Vec::new();
    for p in [
        crate::db::photos_pool().await,
        crate::db::videos_pool().await,
        crate::db::music_pool().await,
    ] {
        if let Ok(p) = p {
            out.push(p);
        }
    }
    out
}

fn thousands(n: i64) -> String {
    let d = n.to_string();
    let b = d.as_bytes();
    let mut out = String::new();
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

// ── counts ──────────────────────────────────────────────────────────────────

async fn n(pool: &SqlitePool, q: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(q).fetch_one(pool).await.unwrap_or(0)
}

async fn counts() -> HomeCounts {
    let mut c = HomeCounts {
        photos: 0,
        photos_albums: 0,
        videos: 0,
        videos_shows: 0,
        songs: 0,
        podcasts: 0,
        audiobooks: 0,
        radio: 0,
        books: 0,
        books_reading: 0,
        cloud_remotes: 0,
        tools_running: 0,
        tools_queued: 0,
        finances_due: 0,
    };
    if let Ok(p) = crate::db::photos_pool().await {
        c.photos = n(p, "SELECT COUNT(*) FROM items WHERE section = 'photos' AND missing_since IS NULL").await;
        c.photos_albums = n(p, "SELECT COUNT(*) FROM albums").await;
    }
    if let Ok(p) = crate::db::videos_pool().await {
        c.videos = n(p, "SELECT COUNT(*) FROM video_meta WHERE deleted_at IS NULL AND archived = 0").await;
        c.videos_shows = n(p, "SELECT COUNT(*) FROM shows").await;
    }
    if let Ok(p) = crate::db::music_pool().await {
        c.songs = n(p, "SELECT COUNT(*) FROM items WHERE missing_since IS NULL").await;
        c.audiobooks = n(p, "SELECT COUNT(DISTINCT folder) FROM track_meta WHERE folder IS NOT NULL").await;
    }
    if let Ok(p) = crate::db::podcasts_pool().await {
        c.podcasts = n(p, "SELECT COUNT(*) FROM podcasts").await;
    }
    if let Ok(p) = crate::db::radio_pool().await {
        c.radio = n(p, "SELECT COUNT(*) FROM radio_stations").await;
    }
    if let Ok(p) = crate::db::books_pool().await {
        c.books = n(p, "SELECT COUNT(*) FROM books WHERE missing = 0").await;
        c.books_reading = n(
            p,
            "SELECT COUNT(*) FROM progress p JOIN books b ON b.id = p.book_id \
             WHERE b.finished = 0 AND b.missing = 0 \
             AND (p.page > 0 OR p.char_offset > 0 OR p.percent > 0)",
        )
        .await;
    }
    if let Ok(p) = crate::db::cloud_pool().await {
        c.cloud_remotes = n(p, "SELECT COUNT(*) FROM remotes").await;
    }
    if let Ok(p) = crate::db::tools_pool().await {
        c.tools_running = n(p, "SELECT COUNT(*) FROM jobs WHERE state = 'running'").await;
        c.tools_queued = n(p, "SELECT COUNT(*) FROM jobs WHERE state = 'queued'").await;
    }
    if let Ok(p) = crate::db::finances_pool().await {
        c.finances_due = n(
            p,
            "SELECT COUNT(*) FROM obligations WHERE status IN ('upcoming', 'due', 'overdue')",
        )
        .await;
    }
    c
}

// ── the section shelves ─────────────────────────────────────────────────────

async fn recent_photos() -> Vec<HomeTile> {
    let Ok(p) = crate::db::photos_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, COALESCE(abs_path, '') FROM items \
         WHERE section = 'photos' AND missing_since IS NULL \
         ORDER BY COALESCE(taken_at, added_at) DESC LIMIT ?",
    )
    .bind(SHELF)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, path)| HomeTile { id, label: file_name(&path), sub: String::new() })
    .collect()
}

async fn recent_videos() -> Vec<HomeTile> {
    let Ok(p) = crate::db::videos_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, Option<f64>)>(
        "SELECT vm.item_id, COALESCE(i.abs_path, ''), vm.duration_s \
         FROM video_meta vm JOIN items i ON i.id = vm.item_id \
         WHERE vm.deleted_at IS NULL AND vm.archived = 0 \
         ORDER BY i.added_at DESC LIMIT ?",
    )
    .bind(SHELF)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, path, dur)| HomeTile {
        id,
        label: file_name(&path),
        sub: dur.map(|d| hm(d)).unwrap_or_default(),
    })
    .collect()
}

async fn recent_books() -> Vec<HomeTile> {
    let Ok(p) = crate::db::books_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT id, COALESCE(title, ''), author FROM books \
         WHERE missing = 0 ORDER BY added_at DESC LIMIT ?",
    )
    .bind(SHELF)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, title, author)| HomeTile {
        id,
        label: title,
        sub: author.unwrap_or_default(),
    })
    .collect()
}

async fn remotes() -> Vec<HomeRemote> {
    let Ok(p) = crate::db::cloud_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (String, String)>(
        "SELECT name, COALESCE(backend, '') FROM remotes ORDER BY name LIMIT ?",
    )
    .bind(SHELF)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(name, backend)| HomeRemote { name, backend, usage: String::new() })
    .collect()
}

// ── the hero, the songs, the money ──────────────────────────────────────────

/// Cinema's backdrop, from the first Continue row. The kicker says which medium
/// it is in the words that medium uses; the meta line carries the detail a
/// 62px title has no room for.
fn hero(row: Option<&HomeContinue>) -> HomeHero {
    let Some(r) = row else {
        return HomeHero {
            kind: String::new(),
            title: String::new(),
            kicker: String::new(),
            meta: String::new(),
            frac: -1.0,
            id: -1,
            path: String::new(),
        };
    };
    let kicker = match r.kind.as_str() {
        "video" => "CONTINUE WATCHING",
        "book" => "CONTINUE READING",
        _ => "CONTINUE LISTENING",
    };
    HomeHero {
        kind: r.kind.clone(),
        title: r.title.clone(),
        kicker: match r.author.is_empty() {
            true => kicker.to_string(),
            false => format!("{kicker} · {}", r.author),
        },
        meta: match r.sub.is_empty() {
            true => r.author.clone(),
            false => r.sub.clone(),
        },
        frac: r.frac,
        id: r.id,
        path: r.path.clone(),
    }
}

async fn recent_songs() -> Vec<HomeTile> {
    let Ok(p) = crate::db::music_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, Option<String>, Option<String>, String)>(
        "SELECT i.id, tm.title, tm.artist, COALESCE(i.abs_path, '') \
         FROM items i LEFT JOIN track_meta tm ON tm.item_id = i.id \
         WHERE i.missing_since IS NULL ORDER BY i.added DESC LIMIT ?",
    )
    .bind(SHELF)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, title, artist, path)| HomeTile {
        id,
        label: title.filter(|t| !t.is_empty()).unwrap_or_else(|| file_name(&path)),
        sub: artist.unwrap_or_default(),
    })
    .collect()
}

#[frb(ignore)]
struct Money {
    month: String,
    spent: String,
    months: Vec<f64>,
    labels: Vec<String>,
    dues: Vec<HomeDue>,
}

/// This month's spend, twelve months of it as bar heights, and the three
/// obligations that want attention -- overdue first, because a fourth row would
/// push Cinema's shelf off the page.
async fn money() -> Money {
    let mut out = Money {
        month: String::new(),
        spent: String::new(),
        months: Vec::new(),
        labels: Vec::new(),
        dues: Vec::new(),
    };
    let Ok(p) = crate::db::finances_pool().await else { return out };

    let today = chrono::Local::now().date_naive();
    out.month = today.format("%B").to_string();

    // Twelve months back, oldest first. One query per month is twelve tiny
    // scans over an indexed column, which is cheaper than pulling every
    // transaction in a year across the bridge to bucket it in Dart.
    let mut spends: Vec<i64> = Vec::new();
    for back in (0..12).rev() {
        let m = month_start(today, back);
        let next = month_start(today, back - 1);
        let total: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(base_minor), 0) FROM transactions \
             WHERE kind = 'expense' AND created_at >= ? AND created_at < ?",
        )
        .bind(m.0)
        .bind(next.0)
        .fetch_one(p)
        .await
        .unwrap_or(0);
        spends.push(total.abs());
        out.labels.push(m.1);
    }
    let peak = spends.iter().copied().max().unwrap_or(0).max(1);
    out.months = spends.iter().map(|v| *v as f64 / peak as f64).collect();
    out.spent = minor(*spends.last().unwrap_or(&0));

    let dues = sqlx::query_as::<_, (String, i64, String, String)>(
        "SELECT name, COALESCE(estimate_minor, 0), COALESCE(due_on, ''), status \
         FROM obligations WHERE status IN ('upcoming', 'due', 'overdue') \
         ORDER BY due_on LIMIT 12",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default();
    let mut rows: Vec<HomeDue> = dues
        .into_iter()
        .map(|(name, minor_amount, due_on, status)| HomeDue {
            name,
            amount: minor(minor_amount),
            due: dd_mm_yy(&due_on),
            late: status == "overdue",
        })
        .collect();
    rows.sort_by_key(|d| !d.late);
    rows.truncate(3);
    out.dues = rows;
    out
}

/// The unix second a month starts, `back` months before the one `today` is in,
/// and that month's long name.
fn month_start(today: chrono::NaiveDate, back: i64) -> (i64, String) {
    use chrono::Datelike;
    let months = today.year() as i64 * 12 + today.month0() as i64 - back;
    let (y, m) = ((months.div_euclid(12)) as i32, (months.rem_euclid(12)) as u32 + 1);
    let d = chrono::NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(today);
    let secs = d.and_hms_opt(0, 0, 0).map(|t| t.and_utc().timestamp()).unwrap_or(0);
    (secs, d.format("%B").to_string())
}

/// Minor units as a plain figure. No currency symbol: the section owns that,
/// and guessing one here would put a second answer on screen.
fn minor(v: i64) -> String {
    format!("{}.{:02}", v / 100, (v % 100).abs())
}

/// `2026-08-15` -> `15-08-26`. Anything that is not an ISO date comes back
/// unchanged, so a humanised string still prints.
fn dd_mm_yy(iso: &str) -> String {
    let p: Vec<&str> = iso.split('-').collect();
    match p.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => {
            format!("{d}-{m}-{}", &y[2..])
        }
        _ => iso.to_string(),
    }
}

// ── the Continue strip ──────────────────────────────────────────────────────

fn dismissed() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static D: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    D.get_or_init(Default::default)
}

fn dismiss_key(kind: &str, id: i64, path: &str) -> String {
    format!("{kind}:{id}:{path}")
}

fn filter_cell() -> &'static std::sync::Mutex<String> {
    static F: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    F.get_or_init(|| std::sync::Mutex::new("all".into()))
}

fn filter() -> String {
    filter_cell().lock().map(|g| g.clone()).unwrap_or_else(|_| "all".into())
}

fn set_filter(v: String) {
    if let Ok(mut g) = filter_cell().lock() {
        *g = v;
    }
}

/// "All" leads with the newest of each kind for variety, then backfills the
/// empty slots with the next-newest rows whatever their kind -- a library with
/// only books in progress used to show one card and three empty wells.
fn filtered(mut rows: Vec<(HomeContinue, i64)>, filter: &str) -> Vec<HomeContinue> {
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    if filter != "all" {
        return rows
            .into_iter()
            .filter(|(r, _)| r.kind == filter)
            .map(|(r, _)| r)
            .take(CONTINUE_SLOTS)
            .collect();
    }
    let mut out: Vec<HomeContinue> = Vec::new();
    let mut used = vec![false; rows.len()];
    for kind in ["video", "book", "podcast", "audiobook"] {
        if out.len() >= CONTINUE_SLOTS {
            break;
        }
        if let Some(i) = (0..rows.len()).find(|&i| !used[i] && rows[i].0.kind == kind) {
            used[i] = true;
            out.push(std::mem::replace(&mut rows[i].0, blank()));
        }
    }
    for i in 0..rows.len() {
        if out.len() >= CONTINUE_SLOTS {
            break;
        }
        if !used[i] {
            used[i] = true;
            out.push(std::mem::replace(&mut rows[i].0, blank()));
        }
    }
    out
}

fn blank() -> HomeContinue {
    HomeContinue {
        kind: String::new(),
        title: String::new(),
        author: String::new(),
        sub: String::new(),
        frac: -1.0,
        id: 0,
        path: String::new(),
    }
}

/// Every in-progress item across four sections, each with the timestamp that
/// orders it. Gathered concurrently -- these are four different databases.
async fn continue_rows() -> Vec<(HomeContinue, i64)> {
    let (books, podcasts, audiobooks, videos) =
        tokio::join!(cont_books(), cont_podcasts(), cont_audiobooks(), cont_videos());
    let mut rows: Vec<(HomeContinue, i64)> = Vec::new();
    rows.extend(books);
    rows.extend(podcasts);
    rows.extend(audiobooks);
    rows.extend(videos);
    let gone = dismissed().lock().map(|g| g.clone()).unwrap_or_default();
    rows.retain(|(r, _)| !gone.contains(&dismiss_key(&r.kind, r.id, &r.path)));
    rows
}

async fn cont_books() -> Vec<(HomeContinue, i64)> {
    let Ok(p) = crate::db::books_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, Option<String>, String, f64, i64, i64)>(
        "SELECT b.id, COALESCE(b.title, ''), b.author, COALESCE(b.abs_path, ''), \
                COALESCE(pr.percent, 0.0), COALESCE(pr.page, 0), COALESCE(pr.updated_at, 0) \
         FROM progress pr JOIN books b ON b.id = pr.book_id \
         WHERE b.finished = 0 AND b.missing = 0 \
           AND (pr.page > 0 OR pr.char_offset > 0 OR pr.percent > 0) \
         ORDER BY pr.updated_at DESC LIMIT ?",
    )
    .bind(CONTINUE_SLOTS as i64)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, title, author, path, percent, page, ts)| {
        let frac = (percent / 100.0).clamp(0.0, 1.0);
        (
            HomeContinue {
                kind: "book".into(),
                title: clip(&title),
                author: author.unwrap_or_default(),
                sub: match percent > 0.0 {
                    true => format!("{}%", percent.round() as i64),
                    false => format!("page {page}"),
                },
                frac: if percent > 0.0 { frac } else { -1.0 },
                id,
                path,
            },
            ts,
        )
    })
    .collect()
}

async fn cont_podcasts() -> Vec<(HomeContinue, i64)> {
    let Ok(p) = crate::db::podcasts_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, String, f64, Option<f64>, i64)>(
        "SELECT e.id, COALESCE(e.title, ''), COALESCE(p.title, ''), e.position_s, e.duration_s, \
                COALESCE(e.published, 0) \
         FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id \
         WHERE e.position_s > 5 \
           AND (e.duration_s IS NULL OR e.duration_s <= 0 OR e.position_s < e.duration_s * 0.95) \
         ORDER BY COALESCE(e.published, 0) DESC LIMIT ?",
    )
    .bind(CONTINUE_SLOTS as i64)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .filter(|(_, title, ..)| !title.is_empty())
    .map(|(id, title, show, pos, dur, ts)| {
        let (frac, sub) = left(pos, dur);
        (
            HomeContinue {
                kind: "podcast".into(),
                title: clip(&title),
                author: show,
                sub,
                frac,
                id,
                path: String::new(),
            },
            ts,
        )
    })
    .collect()
}

async fn cont_audiobooks() -> Vec<(HomeContinue, i64)> {
    let Ok(p) = crate::db::music_pool().await else { return Vec::new() };
    let items = sqlx::query_as::<_, (String, i64, f64, i64)>(
        "SELECT tm.folder, ap.item_id, ap.position_s, COALESCE(ap.updated, 0) \
         FROM audiobook_progress ap JOIN track_meta tm ON tm.item_id = ap.item_id \
         WHERE ap.finished = 0 AND ap.position_s > 0 AND tm.folder IS NOT NULL \
         ORDER BY ap.updated DESC LIMIT 12",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (folder, item_id, pos, ts) in items {
        // One card per book, not one per chapter: the newest chapter resume is
        // the book's position.
        if !seen.insert(folder.clone()) {
            continue;
        }
        if out.len() >= CONTINUE_SLOTS {
            break;
        }
        let title = std::path::Path::new(&folder)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.clone());
        out.push((
            HomeContinue {
                kind: "audiobook".into(),
                title: clip(&title),
                author: "Audiobook".into(),
                sub: format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60),
                frac: -1.0,
                id: item_id,
                path: folder,
            },
            ts,
        ));
    }
    out
}

async fn cont_videos() -> Vec<(HomeContinue, i64)> {
    let Ok(p) = crate::db::videos_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, f64, Option<f64>, i64)>(
        "SELECT vm.item_id, COALESCE(i.abs_path, ''), COALESCE(wp.position_s, 0.0), \
                COALESCE(wp.duration_s, vm.duration_s), COALESCE(vm.last_accessed, 0) \
         FROM video_meta vm JOIN items i ON i.id = vm.item_id \
         LEFT JOIN watch_progress wp ON wp.item_id = vm.item_id \
         WHERE vm.deleted_at IS NULL AND vm.archived = 0 AND vm.last_accessed IS NOT NULL \
           AND COALESCE(wp.finished, 0) = 0 AND COALESCE(wp.position_s, 0) > 0 \
         ORDER BY vm.last_accessed DESC LIMIT ?",
    )
    .bind(CONTINUE_SLOTS as i64)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, path, pos, dur, ts)| {
        let (frac, sub) = left(pos, dur);
        (
            HomeContinue {
                kind: "video".into(),
                title: clip(&file_name(&path)),
                author: "Video".into(),
                sub,
                frac,
                id,
                path,
            },
            ts,
        )
    })
    .collect()
}

/// "18m left / 1h 12m", or the plain position when the duration is unknown.
fn left(pos: f64, dur: Option<f64>) -> (f64, String) {
    match dur {
        Some(d) if d > 1.0 => (
            (pos / d).clamp(0.0, 1.0),
            format!("{}m left / {}", (((d - pos) / 60.0).ceil() as i64).max(1), hm(d)),
        ),
        _ => (-1.0, format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60)),
    }
}

fn hm(secs: f64) -> String {
    let s = secs as i64;
    match s / 3600 {
        0 => format!("{}m", (s % 3600) / 60),
        h => format!("{h}h {}m", (s % 3600) / 60),
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

fn clip(title: &str) -> String {
    if title.chars().count() <= TITLE_CHARS {
        return title.to_string();
    }
    let head: String = title.chars().take(TITLE_CHARS).collect();
    format!("{}...", head.trim_end())
}

// ── the Stream layout's activity feed ───────────────────────────────────────

/// Rows kept after the merge. Fifty is a long scroll and a short query.
const EVENTS_MAX: usize = 50;

/// Runs of the same section and kind inside this window fold into one row.
const EVENT_WINDOW: i64 = 15 * 60;

/// A gathered row, before it knows its clock string or its group caption.
#[frb(ignore)]
struct Raw {
    at: i64,
    section: &'static str,
    kind: &'static str,
    /// Written as "one|many": the collapse picks the half it needs once it
    /// knows how many rows folded together, so neither the query nor the page
    /// has to guess the plural.
    title: String,
    sub: String,
    action: &'static str,
    alarm: bool,
    id: i64,
    path: String,
}

fn feed_cell() -> &'static std::sync::Mutex<String> {
    static F: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    F.get_or_init(|| std::sync::Mutex::new("all".into()))
}

fn feed_filter() -> String {
    feed_cell().lock().map(|g| g.clone()).unwrap_or_else(|_| "all".into())
}

fn set_feed_filter(v: String) {
    if let Ok(mut g) = feed_cell().lock() {
        *g = v;
    }
}

fn in_filter(section: &str, filter: &str) -> bool {
    match filter {
        "media" => matches!(section, "photos" | "videos" | "music" | "books"),
        "money" => section == "finances",
        "devices" => matches!(section, "transfer" | "cloud" | "tools"),
        _ => true,
    }
}

/// An empty today is not an empty page: the feed simply reaches further back,
/// and says so.
fn feed_note(events: &[HomeEvent]) -> String {
    match events.first() {
        Some(e) if e.group != "NOW" && e.group != "TODAY" => {
            "Nothing today — showing what came before.".into()
        }
        _ => String::new(),
    }
}

/// Gather every source, merge, collapse, cap, then stamp each row with its
/// clock and its group caption.
async fn feed() -> Vec<HomeEvent> {
    // Independent databases, so they run concurrently: the feed lands in
    // about max(source) rather than the sum.
    let (photos, videos, books, music, pods, tools, money) = tokio::join!(
        ev_items("photos", "Photo added", "photos added"),
        ev_items("videos", "Video added", "videos added"),
        ev_books(),
        ev_music(),
        ev_podcasts(),
        ev_tools(),
        ev_money(),
    );
    let mut all: Vec<Raw> = Vec::new();
    for src in [photos, videos, books, music, pods, tools, money] {
        all.extend(src);
    }
    all.sort_by(|a, b| b.at.cmp(&a.at));

    let filter = feed_filter();
    all.retain(|r| in_filter(r.section, &filter));
    let mut rows = collapse(all);
    rows.truncate(EVENTS_MAX);

    let now = now_secs();
    let today = chrono::Local::now().date_naive();
    let mut last = String::new();
    rows.into_iter()
        .map(|r| {
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(r.at, 0)
                .map(|t| t.with_timezone(&chrono::Local));
            let group = if now - r.at < 1800 {
                "NOW"
            } else {
                match dt.map(|d| (today - d.date_naive()).num_days()) {
                    Some(0) => "TODAY",
                    Some(1) => "YESTERDAY",
                    _ => "EARLIER",
                }
            };
            let head = group != last;
            last = group.to_string();
            HomeEvent {
                at: dt.map(|d| d.format("%H:%M").to_string()).unwrap_or_default(),
                day: dt.map(|d| d.format("%d-%m-%y").to_string()).unwrap_or_default(),
                group: group.to_string(),
                head,
                section: r.section.to_string(),
                kind: r.kind.to_string(),
                title: r.title,
                sub: r.sub,
                action: r.action.to_string(),
                alarm: r.alarm,
                id: r.id,
                path: r.path,
            }
        })
        .collect()
}

/// Fold runs of the same section and kind, and resolve the "one|many" titles.
fn collapse(rows: Vec<Raw>) -> Vec<Raw> {
    let mut out: Vec<Raw> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for r in rows {
        let fold = out
            .last()
            .is_some_and(|p| p.section == r.section && p.kind == r.kind && p.at - r.at <= EVENT_WINDOW);
        if fold {
            if let Some(n) = counts.last_mut() {
                *n += 1;
            }
            continue;
        }
        out.push(r);
        counts.push(1);
    }
    for (r, n) in out.iter_mut().zip(counts) {
        match r.title.split_once('|') {
            Some((one, many)) => {
                let (one, many) = (one.to_string(), many.to_string());
                r.title = if n > 1 { format!("{n} {many}") } else { one };
                if n > 1 {
                    r.sub = format!("{} +{} more", r.sub, n - 1);
                }
            }
            None if n > 1 => r.sub = format!("{} +{} more", r.sub, n - 1),
            None => {}
        }
    }
    out
}

/// Newly scanned files in a section that keeps the common `items` table.
async fn ev_items(
    section: &'static str,
    one: &'static str,
    many: &'static str,
) -> Vec<Raw> {
    let pool = match section {
        "photos" => crate::db::photos_pool().await,
        _ => crate::db::videos_pool().await,
    };
    let Ok(p) = pool else { return Vec::new() };
    sqlx::query_as::<_, (String, i64)>(
        "SELECT COALESCE(abs_path, ''), added FROM items WHERE missing_since IS NULL \
         ORDER BY added DESC LIMIT 30",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(path, at)| Raw {
        at,
        section,
        kind: "added",
        title: format!("{one}|{many}"),
        sub: file_name(&path),
        action: "",
        alarm: false,
        id: -1,
        path,
    })
    .collect()
}

/// Books: what was added, and what you were reading. Progress is the one resume
/// source with a timestamp of its own, which is why videos only report
/// additions.
async fn ev_books() -> Vec<Raw> {
    let Ok(p) = crate::db::books_pool().await else { return Vec::new() };
    let mut out: Vec<Raw> = sqlx::query_as::<_, (String, Option<String>, i64)>(
        "SELECT COALESCE(title, ''), author, added_at FROM books WHERE missing = 0 \
         ORDER BY added_at DESC LIMIT 12",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(title, author, at)| Raw {
        at,
        section: "books",
        kind: "added",
        title: "Book added|books added".into(),
        sub: match author.filter(|a| !a.is_empty()) {
            Some(a) => format!("{title} · {a}"),
            None => title,
        },
        action: "",
        alarm: false,
        id: -1,
        path: String::new(),
    })
    .collect();
    out.extend(
        sqlx::query_as::<_, (i64, String, f64, i64)>(
            "SELECT b.id, COALESCE(b.title, ''), COALESCE(p.percent, 0.0), COALESCE(p.updated_at, 0) \
             FROM progress p JOIN books b ON b.id = p.book_id \
             WHERE b.finished = 0 AND b.missing = 0 ORDER BY p.updated_at DESC LIMIT 8",
        )
        .fetch_all(p)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, title, pct, at)| Raw {
            at,
            section: "books",
            kind: "resumed",
            title: format!("Reading {title}"),
            sub: format!("{}% through", pct.clamp(0.0, 100.0).round() as i64),
            action: "Resume",
            alarm: false,
            id,
            path: String::new(),
        }),
    );
    out
}

/// What played. `play_history` is written by the player itself, so this is the
/// one source that needs no interpretation.
async fn ev_music() -> Vec<Raw> {
    let Ok(p) = crate::db::music_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (Option<String>, String, i64)>(
        "SELECT tm.title, COALESCE(i.abs_path, ''), h.played_at FROM play_history h \
         JOIN items i ON i.id = h.item_id \
         LEFT JOIN track_meta tm ON tm.item_id = h.item_id \
         ORDER BY h.played_at DESC LIMIT 30",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(title, path, at)| Raw {
        at,
        section: "music",
        kind: "played",
        title: "Track played|tracks played".into(),
        sub: title.filter(|t| !t.is_empty()).unwrap_or_else(|| file_name(&path)),
        action: "",
        alarm: false,
        id: -1,
        path: String::new(),
    })
    .collect()
}

/// Episodes that arrived. They have a `published` date but no local timestamp,
/// so this reports the feed's clock, which is the honest one.
async fn ev_podcasts() -> Vec<Raw> {
    let Ok(p) = crate::db::podcasts_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, Option<String>, String, Option<i64>)>(
        "SELECT e.id, e.title, COALESCE(p.title, ''), e.published FROM podcast_episodes e \
         JOIN podcasts p ON p.id = e.podcast_id \
         WHERE e.published IS NOT NULL ORDER BY e.published DESC LIMIT 8",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .filter_map(|(id, ep, show, published)| {
        Some(Raw {
            at: published?,
            section: "music",
            kind: "episode",
            title: format!("New episode — {}", ep.unwrap_or_else(|| show.clone())),
            sub: show,
            action: "Play",
            alarm: false,
            id,
            path: String::new(),
        })
    })
    .collect()
}

/// Finished and failed Tools jobs, from the executor's own table.
async fn ev_tools() -> Vec<Raw> {
    let Ok(p) = crate::db::tools_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (String, String, i64, Option<String>)>(
        "SELECT kind, state, updated, message FROM jobs \
         WHERE state IN ('done', 'error') ORDER BY updated DESC LIMIT 10",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(kind, state, at, msg)| Raw {
        at,
        section: "tools",
        kind: "job",
        title: match state.as_str() {
            "error" => format!("Job failed — {kind}"),
            _ => format!("Job finished — {kind}"),
        },
        sub: msg.filter(|m| !m.is_empty()).unwrap_or_else(|| "no message".into()),
        action: "",
        alarm: state == "error",
        id: -1,
        path: String::new(),
    })
    .collect()
}

/// Money that moved.
async fn ev_money() -> Vec<Raw> {
    let Ok(p) = crate::db::finances_pool().await else { return Vec::new() };
    sqlx::query_as::<_, (i64, String, i64, String, String)>(
        "SELECT t.created_at, t.kind, t.base_minor, COALESCE(t.description, ''), \
                COALESCE(a.name, '') \
         FROM transactions t LEFT JOIN accounts a ON a.id = t.account_id \
         ORDER BY t.created_at DESC LIMIT 10",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(at, kind, base, desc, account)| Raw {
        at,
        section: "finances",
        kind: "paid",
        title: match kind.as_str() {
            "income" => format!("Received {}", minor(base.abs())),
            _ => format!("Spent {}", minor(base.abs())),
        },
        sub: match (desc.is_empty(), account.is_empty()) {
            (false, false) => format!("{desc} · {account}"),
            (false, true) => desc,
            (true, false) => account,
            (true, true) => kind,
        },
        action: "",
        alarm: false,
        id: -1,
        path: String::new(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &str, id: i64, ts: i64) -> (HomeContinue, i64) {
        (
            HomeContinue {
                kind: kind.into(),
                title: kind.into(),
                author: String::new(),
                sub: String::new(),
                frac: -1.0,
                id,
                path: String::new(),
            },
            ts,
        )
    }

    /// "All" leads with one of each kind, then backfills — a library with only
    /// books in progress must fill all four slots with books, not show one card
    /// and three empty wells.
    #[test]
    fn the_all_filter_varies_then_backfills() {
        let mixed = vec![row("book", 1, 90), row("video", 2, 80), row("book", 3, 70)];
        let kinds: Vec<String> =
            filtered(mixed, "all").into_iter().map(|r| r.kind).collect();
        assert_eq!(kinds, ["video", "book", "book"]);

        let only_books =
            vec![row("book", 1, 4), row("book", 2, 3), row("book", 3, 2), row("book", 4, 1), row("book", 5, 0)];
        assert_eq!(filtered(only_books, "all").len(), CONTINUE_SLOTS);
    }

    #[test]
    fn a_kind_filter_shows_only_that_kind() {
        let mixed = vec![row("book", 1, 3), row("video", 2, 2), row("book", 3, 1)];
        let ids: Vec<i64> = filtered(mixed, "book").into_iter().map(|r| r.id).collect();
        assert_eq!(ids, [1, 3]);
    }

    /// A part-way item with no duration must hide its bar rather than draw one
    /// at zero under something clearly in progress.
    #[test]
    fn an_unknown_duration_hides_the_bar() {
        assert_eq!(left(90.0, None).0, -1.0);
        assert_eq!(left(90.0, Some(0.0)).0, -1.0);
        let (frac, sub) = left(90.0, Some(180.0));
        assert!((frac - 0.5).abs() < 1e-9);
        assert!(sub.starts_with("2m left"), "{sub}");
    }

    #[test]
    fn a_long_title_is_cut_not_wrapped() {
        assert_eq!(clip("short"), "short");
        assert!(clip(&"x".repeat(80)).ends_with("..."));
        assert_eq!(clip(&"x".repeat(80)).chars().count(), TITLE_CHARS + 3);
    }

    #[test]
    fn item_totals_read_with_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(12304), "12,304");
        assert_eq!(thousands(1000000), "1,000,000");
    }
}
