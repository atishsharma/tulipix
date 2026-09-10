//! One pass over every section database, rendered as the JSON the status page
//! draws.
//!
//! Two rules hold this file together.
//!
//! **Nothing is invented.** Every number here is the answer to a query, a file
//! stat, or a value the app pushed into [`Live`]. A status page that reports a
//! figure with nothing behind it teaches its reader to stop reading it, so a
//! fact we cannot get is a row that is not emitted — never a plausible-looking
//! constant.
//!
//! **The shape is the contract.** The page renders whatever cards, rows,
//! groups and tables arrive; it has no per-section knowledge. Adding a section
//! is a new `Value` in [`cards`], not an edit to the HTML.

use serde_json::{json, Value};
use sqlx::SqlitePool;
use tulipix_common::pool_for;

/// Health of the whole app, worst-of across the sections. Drives the sidebar
/// LED as well as the page header, so both always agree.
pub const OK: &str = "ok";
pub const BUSY: &str = "busy";
pub const PROBLEM: &str = "problem";
pub const UNKNOWN: &str = "unknown";

/// Rank for worst-of folding. `busy` deliberately outranks nothing — work in
/// progress is not a problem, and colouring it like one would make the LED
/// amber most of the day.
fn rank(level: &str) -> u8 {
    match level {
        PROBLEM => 3,
        BUSY => 2,
        OK => 1,
        _ => 0,
    }
}

fn worse<'a>(a: &'a str, b: &'a str) -> &'a str {
    if rank(b) > rank(a) { b } else { a }
}

/// What only the running app knows: in-process services with no database
/// behind them (the transfer listener, the whisper queue) and build facts the
/// status crate cannot see from here.
///
/// Set by `tulipix-app` on the same tick that refreshes the snapshot. Left at
/// its default it reports "not started" rather than inventing a state — which
/// is why every field has an honest zero.
#[derive(Default, Clone, Debug)]
pub struct Live {
    pub transfer_running: bool,
    pub transfer_port: u16,
    /// The LAN address the phone URL is built from — the half of "where do I
    /// point a browser" that a port on its own does not answer.
    pub transfer_ip: String,
    pub transfer_devices: i64,
    pub transfer_inflight: i64,
    /// Cumulative bytes this run, out and in. Rates are derived from the
    /// difference between two collections; see [`wire_rate`].
    pub transfer_sent: u64,
    pub transfer_recv: u64,
    /// Model files installed under the app's models directory.
    pub ai_models: i64,
    /// Bytes those model files take up.
    pub ai_models_bytes: i64,
    pub app_version: String,
    pub renderer: String,
    /// Seconds this process has been up.
    pub uptime_s: i64,
    /// Live per-section scan counters, straight off the app's own progress
    /// overlay. Empty when nothing is indexing — which is what makes the page's
    /// progress bar disappear rather than sit at 100%.
    pub scan: Vec<ScanRow>,
}

/// One section's indexing progress, mirroring the app's scan overlay so the
/// page and the overlay cannot disagree about how far along a rescan is.
#[derive(Default, Clone, Debug)]
pub struct ScanRow {
    pub section: String,
    pub total: i64,
    pub added: i64,
    pub failed: i64,
    pub active: bool,
}

pub struct Snapshot {
    pub level: String,
    pub note: String,
    pub json: Value,
}

// ── small query helpers ──────────────────────────────────────────────────────
// Every one of these swallows its error into a zero. A status page that fails
// to render because one table is mid-migration is worse than a status page with
// one zero on it, and the section's own `level` is what reports the trouble.

async fn n(pool: &SqlitePool, q: &'static str) -> i64 {
    sqlx::query_scalar::<_, i64>(q).fetch_one(pool).await.unwrap_or(0)
}

async fn n_at(pool: &SqlitePool, q: &'static str, bind: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(q).bind(bind).fetch_one(pool).await.unwrap_or(0)
}

async fn two(pool: &SqlitePool, q: &'static str) -> (i64, i64) {
    sqlx::query_as::<_, (i64, i64)>(q).fetch_one(pool).await.unwrap_or((0, 0))
}

/// `COUNT(*)` and `SUM(size)` over the live rows of a section's `items` table.
/// Every media section shares this schema (`tulipix_core::db`), so the library
/// totals are one query shape repeated, not three different ones.
async fn items(pool: &SqlitePool) -> (i64, i64) {
    two(pool, "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM items WHERE missing_since IS NULL").await
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── formatting ───────────────────────────────────────────────────────────────

/// Human bytes. Binary steps, one decimal below 100 — "1.4 TB" reads, and
/// "1,542,003,238,912" does not.
pub fn bytes(b: i64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b.max(0) as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} B", b.max(0))
    } else if v < 100.0 {
        format!("{v:.1} {}", U[i])
    } else {
        format!("{v:.0} {}", U[i])
    }
}

/// Thousands separators, the long way round because the number is i64 and the
/// alternative is a formatting dependency for eleven lines.
pub fn num(v: i64) -> String {
    let s = v.abs().to_string();
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    if v < 0 {
        out.push('-');
    }
    for (i, c) in b.iter().enumerate() {
        if i > 0 && (b.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

/// "4h 12m" / "12m" / "—".
pub fn dur(secs: i64) -> String {
    if secs <= 0 {
        return "—".into();
    }
    let m = secs / 60;
    let (h, m) = (m / 60, m % 60);
    if h == 0 {
        format!("{m}m")
    } else {
        format!("{h}h {m:02}m")
    }
}

/// Wall-clock "3:25 PM" for a unix timestamp, local time, without pulling in a
/// date library: the page only ever shows today's events, so the day never has
/// to be rendered and the offset can come from the OS once.
pub fn clock(ts: i64) -> String {
    if ts <= 0 {
        return "—".into();
    }
    let local = ts + local_offset();
    let secs_today = local.rem_euclid(86_400);
    let (h24, mi) = (secs_today / 3600, (secs_today % 3600) / 60);
    let ampm = if h24 < 12 { "AM" } else { "PM" };
    let h = match h24 % 12 {
        0 => 12,
        h => h,
    };
    format!("{h}:{mi:02} {ampm}")
}

/// Seconds east of UTC. Derived once from the difference between local and UTC
/// broken-down time as reported by the OS through `SystemTime`; on a platform
/// where that is unavailable the page shows UTC, which is wrong by a known
/// amount rather than wrong by an unknown one.
fn local_offset() -> i64 {
    use std::sync::OnceLock;
    static OFF: OnceLock<i64> = OnceLock::new();
    *OFF.get_or_init(|| {
        // `date +%z` is present on every platform this ships to and costs one
        // process at startup. Parsing "+0530" is four bytes of arithmetic.
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let s = out
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        let s = s.trim();
        if s.len() < 5 {
            return 0;
        }
        let sign = if s.starts_with('-') { -1 } else { 1 };
        let h: i64 = s[1..3].parse().unwrap_or(0);
        let m: i64 = s[3..5].parse().unwrap_or(0);
        sign * (h * 3600 + m * 60)
    })
}

// ── payload builders ─────────────────────────────────────────────────────────

/// One `key: value` line on a card or inside a popup group. `cls` tints the
/// value — "g" good, "o" attention, "rd" bad, "mut" muted, "" plain.
fn row(k: &str, v: impl Into<String>, cls: &str) -> Value {
    json!({ "k": k, "v": v.into(), "cls": cls })
}

fn grp(h: &str, rows: Vec<Value>) -> Value {
    json!({ "h": h, "rows": rows })
}

fn table(h: &str, cols: &[&str], rows: Vec<Value>) -> Value {
    json!({ "h": h, "table": { "cols": cols, "rows": rows } })
}

/// A status pill inside a table cell.
fn tag(text: &str, cls: &str) -> Value {
    json!({ "tag": text, "cls": cls })
}

fn kpi(v: impl Into<String>, k: &str) -> Value {
    json!([v.into(), k])
}

#[allow(clippy::too_many_arguments)]
fn card(
    key: &str,
    name: &str,
    accent: &str,
    level: &str,
    status: &str,
    rows: Vec<Value>,
    kpis: Vec<Value>,
    groups: Vec<Value>,
) -> Value {
    json!({
        "key": key, "name": name, "accent": accent, "level": level, "status": status,
        "rows": rows,
        "detail": { "kpis": kpis, "groups": groups },
    })
}

// ── the pass ─────────────────────────────────────────────────────────────────

/// Build the whole snapshot. One call per refresh tick; the result is cached by
/// the server, so however many browser tabs are open this runs once.
pub async fn collect(live: &Live) -> Snapshot {
    let t = now();
    let week = t - 7 * 86_400;

    // Sections are independent databases, so they are queried concurrently and
    // the whole pass costs about as long as the slowest one rather than the sum.
    let (photos, videos, music, books, cloud, tools, finances, extra, xfer_today) = tokio::join!(
        photos_card(week),
        videos_card(week),
        music_card(week),
        books_card(),
        cloud_card(),
        tools_card(t),
        finances_card(),
        extra_counts(week),
        transfer_today(),
    );

    let (podcasts, radio, lib_items, lib_bytes, lib_week, spark) = extra;

    let transfer = transfer_card(live, t, xfer_today);
    let ai = ai_card(live);
    let system = system_card(live, lib_bytes);

    // Everything derived from the section results is computed HERE, before the
    // `vec![…]` below moves each `card` field out. After that move the `Sec`s
    // are partially moved, and a whole-struct borrow (`&tools`) is no longer
    // allowed — only individual surviving fields are.

    // Worst-of across everything the page shows.
    let mut level = OK;
    for s in [
        photos.level.as_str(), videos.level.as_str(), music.level.as_str(),
        books.level.as_str(), cloud.level.as_str(), tools.level.as_str(),
        finances.level.as_str(), transfer.level.as_str(), ai.level.as_str(),
        system.level.as_str(),
    ] {
        level = worse(level, s);
    }

    let cloud_status = cloud.card["status"].as_str().unwrap_or("—").to_string();
    // Files on the wire count as work in progress. Without them the page said
    // "0 jobs running" underneath a headline that had gone busy *because* of a
    // transfer — the one card that can be busy without owning a row in `jobs`.
    let jobs_running = tools.running + live.transfer_inflight;
    let jobs_failed = tools.failed;
    let metrics_v = metrics(live, lib_bytes, jobs_running, jobs_failed, &cloud_status,
        podcasts, radio);
    let timeline_v = timeline(&tools.events, &transfer_events().await);
    let today_v = today(music.today, photos.today, tools.today, finances.today);

    let cards = vec![
        music.card, books.card, photos.card, videos.card, cloud.card,
        finances.card, transfer.card, tools.card, ai.card, system.card,
    ];

    let online = 10 - cards.iter().filter(|c| c["level"] == UNKNOWN).count() as i64;

    // Built before the `json!` below, which moves `cards` — and this reads the
    // section figures back out of them.
    let library_v = library(&cards, lib_items, lib_bytes, &root_sections().await);
    let scan_v = scan(&live.scan);

    // The headline names the single most useful fact, in the order someone
    // scanning the sidebar would want it: a real problem first, then work in
    // progress, then the all-clear.
    //
    // A problem quotes the card's own `why` rather than its name. "Finances
    // reporting a problem" is a redirect — the reader still has to go and find
    // out what. The card already knows, so it says so here.
    let (headline, note) = if level == PROBLEM {
        let told: Vec<String> = cards
            .iter()
            .filter(|c| c["level"] == PROBLEM)
            .map(|c| {
                let name = c["name"].as_str().unwrap_or("A section");
                match c["why"].as_str() {
                    Some(w) if !w.is_empty() => format!("{name}: {w}"),
                    _ => format!("{name} reporting a problem"),
                }
            })
            .collect();
        ("Attention Needed".to_string(), told.join(" · "))
    } else if level == BUSY {
        // Same headline as the idle case on purpose: a scan running is the app
        // doing its job, not a state the reader has to act on. What changes is
        // the line under it, which says how much work is in flight.
        (
            "All Systems Operational".to_string(),
            format!("{} Tasks Running · {online} sections responding", num(jobs_running)),
        )
    } else {
        (
            "All Systems Operational".to_string(),
            format!("{online} sections responding · no failed jobs"),
        )
    };

    // The short line under "Status" in the sidebar. Same facts, fewer words —
    // the rail is 190px wide.
    let short = if level == PROBLEM {
        note.clone()
    } else if level == BUSY {
        format!("{} jobs running", num(jobs_running))
    } else {
        "All systems operational".into()
    };

    let json = json!({
        "level": level,
        "headline": headline,
        "note": note,
        "updated": t,
        "hero": {
            "sections_online": online,
            "jobs_running": jobs_running,
            "jobs_failed": jobs_failed,
            "library_bytes": bytes(lib_bytes),
            "library_items": num(lib_items),
            "library_week": lib_week,
            "spark": spark,
        },
        "metrics": metrics_v,
        "cards": cards,
        "timeline": timeline_v,
        "today": today_v,
        "library": library_v,
        "scan": scan_v,
    });

    Snapshot { level: level.to_string(), note: short, json }
}

/// Per-section result: the card itself plus the few figures the header, the
/// metric strip and the "Today" column need to reuse.
struct Sec {
    card: Value,
    level: String,
    running: i64,
    failed: i64,
    today: i64,
    events: Vec<(i64, String, String, String)>,
}

impl Sec {
    fn new(card: Value, level: &str) -> Self {
        Self { card, level: level.into(), running: 0, failed: 0, today: 0, events: Vec::new() }
    }

    /// Why this card is not OK, in one line specific enough to act on.
    ///
    /// "Finances reporting a problem" is not a status, it is a promise to look
    /// somewhere else. Whatever a card knows about its own trouble it says here
    /// and the page header quotes it verbatim — so the top of the page names the
    /// thing rather than pointing at the card that names the thing.
    fn why(mut self, why: impl Into<String>) -> Self {
        let why = why.into();
        if !why.is_empty() {
            self.card["why"] = json!(why);
        }
        self
    }
}

async fn photos_card(week: i64) -> Sec {
    let Ok(pool) = pool_for("photos").await else {
        return Sec::new(card("photos", "Photos", "violet", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };
    let (n_items, size) = items(&pool).await;
    let albums = n(&pool, "SELECT COUNT(*) FROM albums").await;
    let missing = n(&pool, "SELECT COUNT(*) FROM items WHERE missing_since IS NOT NULL").await;
    let added_week = n_at(&pool, "SELECT COUNT(*) FROM items WHERE added >= ?", week).await;
    let added_today = n_at(&pool, "SELECT COUNT(*) FROM items WHERE added >= ?", now() - 86_400).await;
    let newest = n(&pool, "SELECT COALESCE(MAX(added), 0) FROM items").await;

    let level = if missing > 0 { PROBLEM } else { OK };
    let status = if missing > 0 { format!("{missing} missing") } else { "Operational".into() };

    let mut s = Sec::new(
        card("photos", "Photos", "violet", level, &status,
            vec![
                row("Items", num(n_items), ""),
                row("Albums", num(albums), ""),
                row("Size", bytes(size), ""),
                row("Added this week", num(added_week), ""),
                row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
            ],
            vec![kpi(num(n_items), "Items"), kpi(bytes(size), "On disk"), kpi(num(albums), "Albums")],
            vec![
                grp("Library", vec![
                    row("Live items", num(n_items), ""),
                    row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
                    row("Albums", num(albums), ""),
                    row("Newest import", clock(newest), "mut"),
                ]),
                grp("Recent", vec![
                    row("Added today", num(added_today), ""),
                    row("Added this week", num(added_week), ""),
                    row("Average size", if n_items > 0 { bytes(size / n_items) } else { "—".into() }, "mut"),
                ]),
            ]),
        level,
    ).why(missing_why("photo", missing));
    s.today = added_today;
    s
}

/// The one sentence every media section says when files it indexed are no
/// longer where it indexed them. Same wording in four places because it is the
/// same fact, and a rescan is the same answer to all four.
fn missing_why(noun: &str, missing: i64) -> String {
    match missing {
        0 => String::new(),
        1 => format!("1 {noun} file is indexed but no longer on disk — rescan to clear it"),
        n => format!("{} {noun} files are indexed but no longer on disk — rescan to clear them",
            num(n)),
    }
}

async fn videos_card(week: i64) -> Sec {
    let Ok(pool) = pool_for("videos").await else {
        return Sec::new(card("videos", "Videos", "indigo", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };
    let (n_items, size) = items(&pool).await;
    let shows = n(&pool, "SELECT COUNT(*) FROM shows").await;
    let missing = n(&pool, "SELECT COUNT(*) FROM items WHERE missing_since IS NOT NULL").await;
    let added_week = n_at(&pool, "SELECT COUNT(*) FROM items WHERE added >= ?", week).await;

    let level = if missing > 0 { PROBLEM } else { OK };
    let status = if missing > 0 { format!("{missing} missing") } else { "Operational".into() };

    Sec::new(
        card("videos", "Videos", "indigo", level, &status,
            vec![
                row("Items", num(n_items), ""),
                row("Shows", num(shows), ""),
                row("Size", bytes(size), ""),
                row("Added this week", num(added_week), ""),
                row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
            ],
            vec![kpi(num(n_items), "Items"), kpi(bytes(size), "On disk"), kpi(num(shows), "Shows")],
            vec![grp("Library", vec![
                row("Live items", num(n_items), ""),
                row("Shows", num(shows), ""),
                row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
                row("Added this week", num(added_week), ""),
                row("Average size", if n_items > 0 { bytes(size / n_items) } else { "—".into() }, "mut"),
            ])]),
        level,
    ).why(missing_why("video", missing))
}

async fn music_card(week: i64) -> Sec {
    let Ok(pool) = pool_for("music").await else {
        return Sec::new(card("music", "Music", "pink", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };
    let (_, size) = items(&pool).await;
    let songs = n(&pool, "SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 0").await;
    let audiobooks =
        n(&pool, "SELECT COUNT(DISTINCT folder) FROM track_meta WHERE is_audiobook = 1").await;
    // `track_meta` keys these by id, so the counts come from the dimension
    // tables rather than a DISTINCT over a column that does not exist there.
    let artists = n(&pool, "SELECT COUNT(*) FROM artists").await;
    let albums = n(&pool, "SELECT COUNT(*) FROM albums").await;
    let missing = n(&pool, "SELECT COUNT(*) FROM items WHERE missing_since IS NOT NULL").await;
    let added_week = n_at(&pool, "SELECT COUNT(*) FROM items WHERE added >= ?", week).await;

    let st = tulipix_music::dashboard::stats(&pool).await.unwrap_or_default();
    let plays_today = n_at(&pool, "SELECT COUNT(*) FROM play_history WHERE played_at >= ?",
        now() - 86_400).await;

    let level = if missing > 0 { PROBLEM } else { OK };
    let status = if missing > 0 { format!("{missing} missing") } else { "Operational".into() };

    let mut s = Sec::new(
        card("music", "Music", "pink", level, &status,
            vec![
                row("Tracks", num(songs), ""),
                row("Audiobooks", num(audiobooks), ""),
                row("This week", tulipix_music::dashboard::fmt_listen(st.week_ms), "g"),
                row("Streak", format!("{} days", st.streak_days), ""),
                row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
            ],
            vec![
                kpi(num(songs), "Tracks"),
                kpi(tulipix_music::dashboard::fmt_listen(st.week_ms), "Played this week"),
                kpi(num(st.streak_days), "Day streak"),
            ],
            vec![
                grp("Library", vec![
                    row("Albums", num(albums), ""),
                    row("Artists", num(artists), ""),
                    row("Audiobooks", format!("{} folders", num(audiobooks)), ""),
                    row("On disk", bytes(size), ""),
                    row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
                    row("Added this week", num(added_week), ""),
                ]),
                grp("Listening", vec![
                    row("This week", tulipix_music::dashboard::fmt_listen(st.week_ms), ""),
                    row("All time", tulipix_music::dashboard::fmt_listen(st.total_ms), ""),
                    row("Plays today", num(plays_today), ""),
                    row("Day streak", format!("{} days", st.streak_days), ""),
                    row("Top genre", st.top_genre.clone().unwrap_or_else(|| "—".into()), "mut"),
                ]),
            ]),
        level,
    ).why(missing_why("music", missing));
    s.today = plays_today;
    s
}

async fn books_card() -> Sec {
    let Ok(pool) = pool_for("books").await else {
        return Sec::new(card("books", "Books", "emerald", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };
    let (lib, size) = two(&pool,
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM books WHERE missing = 0").await;
    let missing = n(&pool, "SELECT COUNT(*) FROM books WHERE missing = 1").await;
    let finished = n(&pool, "SELECT COUNT(*) FROM books WHERE finished = 1 AND missing = 0").await;
    let reading = n(&pool,
        "SELECT COUNT(*) FROM progress p JOIN books b ON b.id = p.book_id \
         WHERE b.finished = 0 AND b.missing = 0 \
               AND (p.page > 0 OR p.char_offset > 0 OR p.percent > 0)").await;
    let favorites = n(&pool, "SELECT COUNT(*) FROM books WHERE favorite = 1 AND missing = 0").await;

    // Format mix, biggest first — the one thing about a book library that is
    // hard to answer anywhere else in the app.
    let fmts = sqlx::query_as::<_, (String, i64)>(
        "SELECT UPPER(format), COUNT(*) FROM books WHERE missing = 0 \
         GROUP BY UPPER(format) ORDER BY 2 DESC LIMIT 6")
        .fetch_all(&pool).await.unwrap_or_default();
    let fmt_rows: Vec<Value> = fmts.iter().map(|(f, c)| row(f, num(*c), "")).collect();

    let level = if missing > 0 { PROBLEM } else { OK };
    let status = if missing > 0 { format!("{missing} missing") } else { "Operational".into() };

    Sec::new(
        card("books", "Books", "emerald", level, &status,
            vec![
                row("Library", num(lib), ""),
                row("In progress", num(reading), "g"),
                row("Finished", num(finished), ""),
                row("Size", bytes(size), ""),
                row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
            ],
            vec![kpi(num(lib), "Library"), kpi(num(reading), "In progress"), kpi(num(finished), "Finished")],
            vec![
                grp("Library", vec![
                    row("On disk", bytes(size), ""),
                    row("Favorites", num(favorites), ""),
                    row("Missing files", num(missing), if missing > 0 { "rd" } else { "g" }),
                    row("Average size", if lib > 0 { bytes(size / lib) } else { "—".into() }, "mut"),
                ]),
                grp("Formats", fmt_rows),
            ]),
        level,
    ).why(missing_why("book", missing))
}

async fn cloud_card() -> Sec {
    let Ok(pool) = pool_for("cloud").await else {
        return Sec::new(card("cloud", "Cloud", "teal", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };

    // Every remote with the mount row it may or may not have. LEFT JOIN, not
    // JOIN: a remote that was never mounted still exists and still belongs on
    // the list — inner-joining it away would quietly under-report the count.
    let all = sqlx::query_as::<_, (String, String, Option<String>, Option<String>)>(
        "SELECT r.name, r.backend, m.mount_path, m.status \
         FROM remotes r LEFT JOIN mounts m ON m.remote_id = r.id \
         ORDER BY r.name")
        .fetch_all(&pool).await.unwrap_or_default();

    let total = all.len() as i64;
    let mounted = all.iter().filter(|r| r.3.as_deref() == Some("mounted")).count() as i64;
    let errored = all.iter().filter(|r| r.3.as_deref() == Some("error")).count() as i64;

    let level = if errored > 0 { PROBLEM } else { OK };
    let status = if errored > 0 {
        format!("{errored} unreachable")
    } else if total == 0 {
        "No remotes".into()
    } else {
        format!("{mounted} of {total} mounted")
    };
    // Deliberately not folded into `level`: a queue row that errored is a file
    // that did not copy, which the queue group reports by name. Turning the
    // whole app's LED red for it would make a retryable copy look like an
    // unreachable remote.

    // The card shows the three most useful remotes, the popup shows all of
    // them: anything that is not simply mounted comes first, because a card
    // with room for three lines should spend them on the ones that need
    // looking at.
    let mut ranked: Vec<&(String, String, Option<String>, Option<String>)> = all.iter().collect();
    ranked.sort_by_key(|r| match r.3.as_deref() {
        Some("error") => 0,
        None | Some("unmounted") => 1,
        _ => 2,
    });

    // The transfer queue. `uploads` is rclone's own work list — a row per file
    // with a direction implied by which side `dest_path` names, so the queue is
    // reported by state rather than split into two lists the schema does not
    // actually keep apart.
    let q = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "SELECT state, COUNT(*), COALESCE(SUM(bytes_total), 0), COALESCE(SUM(bytes_done), 0) \
         FROM uploads GROUP BY state")
        .fetch_all(&pool).await.unwrap_or_default();
    let qn = |st: &str| q.iter().find(|r| r.0 == st).map(|r| r.1).unwrap_or(0);
    let (queued, running, q_err) = (qn("queued"), qn("running"), qn("error"));
    let pending = queued + running;
    let (q_total, q_done): (i64, i64) = q
        .iter()
        .filter(|r| r.0 == "queued" || r.0 == "running")
        .fold((0, 0), |(a, b), r| (a + r.2, b + r.3));

    let queue_tbl: Vec<Value> = sqlx::query_as::<_, (String, String, i64, i64, String)>(
        "SELECT u.src_path, u.dest_path, u.bytes_total, u.bytes_done, u.state \
         FROM uploads u WHERE u.state IN ('queued', 'running', 'error') \
         ORDER BY CASE u.state WHEN 'running' THEN 0 WHEN 'error' THEN 1 ELSE 2 END, u.id \
         LIMIT 20")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(src, dest, total, done, state)| {
            let pct = if total > 0 { done * 100 / total } else { 0 };
            json!([
                file_name(&src),
                dest,
                bytes(total),
                tag(&match state.as_str() {
                    "running" => format!("{pct}%"),
                    "error" => "Error".to_string(),
                    _ => "Queued".to_string(),
                }, match state.as_str() {
                    "running" => "",
                    "error" => "b",
                    _ => "i",
                }),
            ])
        })
        .collect();

    let mut rows = vec![row("Remotes",
        if total > 3 { format!("{total} · 3 shown") } else { num(total) }, "")];
    rows.push(row("Transfer queue",
        if pending > 0 {
            format!("{} · {} left", num(pending), bytes(q_total - q_done))
        } else if q_err > 0 {
            format!("{} failed", num(q_err))
        } else {
            "Empty".into()
        },
        if q_err > 0 { "rd" } else if pending > 0 { "o" } else { "mut" }));
    for r in ranked.iter().take(3) {
        let (label, cls) = mount_state(r.3.as_deref());
        rows.push(row(&r.0, label, cls));
    }

    let table_rows: Vec<Value> = all.iter().map(|(name, backend, path, st)| {
        let (label, cls) = mount_state(st.as_deref());
        json!([
            name,
            backend,
            path.clone().unwrap_or_else(|| "—".into()),
            tag(label, cls),
        ])
    }).collect();

    // Named, because "1 unreachable" out of five remotes is a question, and the
    // answer is already in the row we just filtered.
    let broken: Vec<&str> = all.iter()
        .filter(|r| r.3.as_deref() == Some("error"))
        .map(|r| r.0.as_str())
        .collect();
    let why = if broken.is_empty() {
        String::new()
    } else {
        format!("rclone cannot reach {} — check the remote's credentials or network",
            broken.join(", "))
    };

    let mut sec = Sec::new(
        card("cloud", "Cloud", "teal", level, &status, rows,
            vec![kpi(num(total), "Remotes"), kpi(num(mounted), "Mounted"),
                 kpi(num(pending), "Queued transfers")],
            vec![
                table("Upload / download queue",
                    &["File", "Destination", "Size", "State"], queue_tbl),
                grp("Queue", vec![
                    row("Running", num(running), if running > 0 { "o" } else { "mut" }),
                    row("Waiting", num(queued), ""),
                    row("Failed", num(q_err), if q_err > 0 { "rd" } else { "g" }),
                    row("Bytes remaining", bytes(q_total - q_done),
                        if pending > 0 { "" } else { "mut" }),
                ]),
                table("All remotes", &["Remote", "Type", "Mount", "Status"], table_rows),
                grp("rclone", vec![
                    row("Binary", if tulipix_common::bundled_present("rclone") || which("rclone") {
                        "Found"
                    } else {
                        "Not found"
                    }, if tulipix_common::bundled_present("rclone") || which("rclone") { "g" } else { "rd" }),
                    row("Remotes configured", num(total), ""),
                ]),
            ]),
        level,
    ).why(why);
    // The button says what the popup actually adds. Phrasing it here rather
    // than deriving it in the page keeps the page free of per-section wording.
    if pending > 0 {
        sec.card["more"] = json!(format!("View queue ({})", num(pending)));
    } else if total > 3 {
        sec.card["more"] = json!(format!("View all {total} remotes"));
    }
    sec
}

/// Mount status → (label, css class). `None` is a remote with no mount row at
/// all, which is a remote that has never been mounted rather than one that
/// failed — different news, different colour.
fn mount_state(st: Option<&str>) -> (&'static str, &'static str) {
    match st {
        Some("mounted") => ("Mounted", "g"),
        Some("error") => ("Error", "b"),
        Some("unmounted") => ("Unmounted", "i"),
        _ => ("Not mounted", "i"),
    }
}

/// Last path component, for a table cell that would otherwise be a full
/// absolute path in a 200px column. The whole path is still one row down in the
/// destination column, so nothing is hidden — only unstacked.
fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(path).to_string()
}

/// Is a binary reachable on PATH. Cheap enough to call per refresh, and the
/// answer genuinely changes at runtime when someone installs rclone.
fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| {
            d.join(bin).is_file() || d.join(format!("{bin}.exe")).is_file()
        }))
        .unwrap_or(false)
}

async fn tools_card(t: i64) -> Sec {
    let Ok(pool) = pool_for("tools").await else {
        return Sec::new(card("tools", "Tools", "cyan", UNKNOWN, "Unavailable",
            vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
    };
    let running = n(&pool, "SELECT COUNT(*) FROM jobs WHERE state = 'running'").await;
    let queued = n(&pool, "SELECT COUNT(*) FROM jobs WHERE state = 'queued'").await;
    let day = t - 86_400;
    let done = n_at(&pool, "SELECT COUNT(*) FROM jobs WHERE state = 'done' AND updated >= ?", day).await;
    let failed = n_at(&pool, "SELECT COUNT(*) FROM jobs WHERE state = 'error' AND updated >= ?", day).await;

    let live_rows = sqlx::query_as::<_, (String, f64, i64, Option<String>)>(
        "SELECT kind, progress, created, message FROM jobs WHERE state = 'running' \
         ORDER BY created LIMIT 12")
        .fetch_all(&pool).await.unwrap_or_default();
    let running_tbl: Vec<Value> = live_rows.iter().map(|(kind, prog, created, msg)| {
        json!([
            kind,
            msg.clone().unwrap_or_else(|| "—".into()),
            clock(*created),
            tag(&format!("{}%", (prog * 100.0).round() as i64), ""),
        ])
    }).collect();

    // Recently finished, for the page's activity timeline.
    let recent = sqlx::query_as::<_, (String, String, i64, Option<String>)>(
        "SELECT kind, state, updated, message FROM jobs \
         WHERE state IN ('done', 'error') AND updated >= ? ORDER BY updated DESC LIMIT 8")
        .bind(day).fetch_all(&pool).await.unwrap_or_default();
    let events = recent.into_iter().map(|(kind, state, at, msg)| {
        let title = if state == "error" {
            format!("Tools job failed — {kind}")
        } else {
            format!("Tools job finished — {kind}")
        };
        (at, "cyan".to_string(), title, msg.unwrap_or_else(|| "no message".into()))
    }).collect();

    let level = if failed > 0 { PROBLEM } else if running > 0 { BUSY } else { OK };
    let status = if failed > 0 {
        format!("{failed} failed")
    } else if running > 0 {
        format!("{running} running")
    } else {
        "Idle".into()
    };

    // The job's own message where it left one — it is the only place the actual
    // cause is written down, and a status page that omits it makes someone open
    // the section to read a line we already have in hand.
    let first_error = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT kind, message FROM jobs WHERE state = 'error' AND updated >= ? \
         ORDER BY updated DESC LIMIT 1")
        .bind(day).fetch_optional(&pool).await.ok().flatten();
    let why = match (failed, &first_error) {
        (0, _) => String::new(),
        (n, Some((kind, msg))) => {
            let detail = msg.as_deref().filter(|m| !m.is_empty()).unwrap_or("no message");
            if n > 1 {
                format!("{n} jobs failed in 24h, latest {kind}: {detail}")
            } else {
                format!("{kind} job failed: {detail}")
            }
        }
        (n, None) => format!("{n} jobs failed in the last 24 hours"),
    };

    // Off the runtime thread: this spawns `yt-dlp --version`, and a PyInstaller
    // binary takes a couple of hundred milliseconds to start. The other rows
    // here only stat PATH, which is why they can stay inline. Cached after the
    // first read, so the cost is once per session.
    let ytdlp_version = tokio::task::spawn_blocking(tulipix_core::ytdlp::installed_version)
        .await
        .ok()
        .flatten();

    let mut s = Sec::new(
        card("tools", "Tools", "cyan", level, &status,
            vec![
                row("Running", num(running), if running > 0 { "o" } else { "mut" }),
                row("Queued", num(queued), ""),
                row("Done 24h", num(done), "g"),
                row("Failed 24h", num(failed), if failed > 0 { "rd" } else { "g" }),
                row("ffmpeg", if which("ffmpeg") { "Found" } else { "Not found" },
                    if which("ffmpeg") { "g" } else { "rd" }),
            ],
            vec![kpi(num(running), "Running"), kpi(num(queued), "Queued"), kpi(num(done), "Done 24h")],
            vec![
                table("Running now", &["Job", "Message", "Started", "Progress"], running_tbl),
                grp("Queue", vec![
                    row("Waiting", num(queued), ""),
                    row("Done 24h", num(done), "g"),
                    row("Failed 24h", num(failed), if failed > 0 { "rd" } else { "g" }),
                ]),
                grp("Binaries", vec![
                    row("ffmpeg", if which("ffmpeg") { "On PATH" } else { "Not found" },
                        if which("ffmpeg") { "g" } else { "rd" }),
                    // The version, not "On PATH". This row exists to answer
                    // "why did my download fail", and the answer is nearly
                    // always a binary some weeks old — which "On PATH" never
                    // said. It also asked the wrong question: the app resolves
                    // its own copy first and only falls back to PATH.
                    match &ytdlp_version {
                        Some(v) => row("yt-dlp", v.clone(), "g"),
                        None => row("yt-dlp", "Not found", "rd"),
                    },
                    row("rclone", if which("rclone") { "On PATH" } else { "Not found" },
                        if which("rclone") { "g" } else { "rd" }),
                    row("mpv", if which("mpv") { "On PATH" } else { "Not found" },
                        if which("mpv") { "g" } else { "rd" }),
                ]),
            ]),
        level,
    ).why(why);
    s.running = running;
    s.failed = failed;
    s.today = done;
    s.events = events;
    s
}

async fn finances_card() -> Sec {
    let pool = match finances_pool().await {
        Some(p) => p,
        None => {
            return Sec::new(card("finances", "Finances", "lime", UNKNOWN, "Unavailable",
                vec![row("Database", "Could not open", "rd")], vec![], vec![]), UNKNOWN);
        }
    };

    let accounts = n(&pool, "SELECT COUNT(*) FROM accounts WHERE closed = 0").await;
    let txns = n(&pool, "SELECT COUNT(*) FROM transactions").await;
    let today_str = tulipix_finances::date::today();
    let txn_today = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM transactions WHERE occurred_on = ?")
        .bind(today_str.to_string()).fetch_one(&pool).await.unwrap_or(0);

    let lead = tulipix_finances::lead_days();
    let due = tulipix_finances::obligations::badge_count(&pool, today_str, lead).await.unwrap_or(0);
    // Bills and subscriptions live in `obligations`, which carries its own
    // status. (`dues` is a different thing entirely — money between you and a
    // named person — and counting it here would report IOUs as overdue bills.)
    // By name and by date, not just counted. "1 overdue" sends someone to open
    // the section to find out which; "Housekeeping, 1 day late" does not.
    let late = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT name, due_on, COALESCE(actual_minor, estimate_minor, 0) \
         FROM obligations WHERE status = 'overdue' ORDER BY due_on")
        .fetch_all(&pool).await.unwrap_or_default();
    let overdue = late.len() as i64;
    let uncategorized = n(&pool, "SELECT COUNT(*) FROM transactions WHERE category_id IS NULL").await;

    let days_late = |due: &str| {
        tulipix_finances::date::parse(due)
            .map(|d| tulipix_finances::date::days_between(d, today_str))
            .unwrap_or(0)
    };
    let late_rows: Vec<Value> = late.iter().map(|(name, due, minor)| {
        let d = days_late(due);
        row(name, format!("{:.2} · {} late", *minor as f64 / 100.0, plural_days(d)), "rd")
    }).collect();

    let upcoming = sqlx::query_as::<_, (String, String, i64, String)>(
        "SELECT name, due_on, COALESCE(actual_minor, estimate_minor, 0), status \
         FROM obligations WHERE status IN ('upcoming', 'due', 'overdue') \
         ORDER BY due_on LIMIT 10")
        .fetch_all(&pool).await.unwrap_or_default();
    let due_tbl: Vec<Value> = upcoming.iter().map(|(name, on, minor, status)| {
        let late = status == "overdue";
        json!([name, on, tag(&format!("{:.2}", *minor as f64 / 100.0), if late { "b" } else { "w" })])
    }).collect();

    let level = if overdue > 0 { PROBLEM } else if due > 0 { BUSY } else { OK };
    let status = if overdue > 0 {
        format!("{overdue} overdue")
    } else if due > 0 {
        format!("{due} due soon")
    } else {
        "Operational".into()
    };

    // What is wrong, in the words the card would use if it had room: the two
    // names and how late they are, not "1 overdue".
    let why = late.iter().take(3).map(|(name, due, _)| {
        format!("{name} ({} late)", plural_days(days_late(due)))
    }).collect::<Vec<_>>().join(", ");
    let why = match overdue {
        0 => String::new(),
        n if n > 3 => format!("Unpaid bills: {why}, and {} more", n - 3),
        _ => format!("Unpaid bills: {why}"),
    };

    let mut s = Sec::new(
        card("finances", "Finances", "lime", level, &status,
            vec![
                row("Accounts", num(accounts), ""),
                row("Transactions", format!("{} today", num(txn_today)), ""),
                row(&format!("Due ≤ {lead}d"), num(due), if due > 0 { "o" } else { "g" }),
                row("Overdue", num(overdue), if overdue > 0 { "rd" } else { "g" }),
                row("Uncategorized", num(uncategorized), if uncategorized > 0 { "o" } else { "g" }),
            ],
            vec![kpi(num(accounts), "Accounts"), kpi(num(due), "Due soon"), kpi(num(overdue), "Overdue")],
            vec![
                // First, above everything else: this is the group that explains
                // why the whole page is red.
                grp("Overdue — pay or reschedule these", late_rows),
                table("Coming due", &["Item", "Due", "Amount"], due_tbl),
                grp("Ledger", vec![
                    row("Transactions", num(txns), ""),
                    row("Posted today", num(txn_today), ""),
                    row("Uncategorized", num(uncategorized), if uncategorized > 0 { "o" } else { "g" }),
                    row("Open accounts", num(accounts), ""),
                ]),
                grp("Engine", vec![
                    row("Alert lead", format!("{lead} days"), "mut"),
                    row("Today", today_str.to_string(), "mut"),
                ]),
            ]),
        level,
    ).why(why);
    s.today = txn_today;
    s
}

/// "1 day" / "6 days" / "today". Used wherever a lateness is spelled out, so
/// nothing on the page ever reads "1 days".
fn plural_days(d: i64) -> String {
    match d {
        d if d <= 0 => "today".into(),
        1 => "1 day".into(),
        d => format!("{d} days"),
    }
}

/// `finances.db` is not in `pool_for` — it opens through its own crate, which
/// also seeds demo rows on a first-ever open. Cached here so a refresh every
/// few seconds does not repeat that work.
async fn finances_pool() -> Option<SqlitePool> {
    static POOL: tokio::sync::OnceCell<SqlitePool> = tokio::sync::OnceCell::const_new();
    POOL.get_or_try_init(tulipix_finances::open).await.ok().cloned()
}

/// Bytes per second, from the distance between this reading of a cumulative
/// counter and the last one.
///
/// The sampler lives here rather than in the transfer service because only this
/// side knows how often it looks — and the interval is not fixed: the refresh
/// tick is 2s with a page open and 30s without. Dividing by the *measured*
/// elapsed time is what keeps the number honest across that switch.
///
/// A counter that went backwards means the server restarted; that reads as zero
/// rather than as a negative rate.
fn wire_rate(slot: &std::sync::Mutex<(u64, i64)>, total: u64, t: i64) -> i64 {
    let mut last = match slot.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let (prev_total, prev_t) = *last;
    *last = (total, t);
    // First reading of a run has nothing to subtract from, and a rate over a
    // zero-length interval is not a number.
    if prev_t == 0 || t <= prev_t || total < prev_total {
        return 0;
    }
    ((total - prev_total) as i64) / (t - prev_t)
}

fn transfer_rates(live: &Live, t: i64) -> (i64, i64) {
    use std::sync::{Mutex, OnceLock};
    static SENT: OnceLock<Mutex<(u64, i64)>> = OnceLock::new();
    static RECV: OnceLock<Mutex<(u64, i64)>> = OnceLock::new();
    (
        wire_rate(SENT.get_or_init(|| Mutex::new((0, 0))), live.transfer_sent, t),
        wire_rate(RECV.get_or_init(|| Mutex::new((0, 0))), live.transfer_recv, t),
    )
}

/// Today's ledger, split by direction: `(files out, bytes out, files in, bytes in)`.
/// Midnight is the local one — a "today" that rolls over at 5:30am is not today.
async fn transfer_today() -> (i64, i64, i64, i64) {
    let Ok(pool) = pool_for("transfers").await else { return (0, 0, 0, 0) };
    let midnight = local_midnight();
    const Q: &str = "SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM transfers \
                     WHERE direction = ? AND status = 'ok' AND at >= ?";
    let out = sqlx::query_as::<_, (i64, i64)>(Q)
        .bind("out").bind(midnight)
        .fetch_one(&pool).await.unwrap_or((0, 0));
    let inn = sqlx::query_as::<_, (i64, i64)>(Q)
        .bind("in").bind(midnight)
        .fetch_one(&pool).await.unwrap_or((0, 0));
    (out.0, out.1, inn.0, inn.1)
}

/// Unix seconds of the most recent local midnight.
fn local_midnight() -> i64 {
    let off = local_offset();
    let local = now() + off;
    local - local.rem_euclid(86_400) - off
}

fn transfer_card(live: &Live, t: i64, today: (i64, i64, i64, i64)) -> Sec {
    let (sent_rate, recv_rate) = transfer_rates(live, t);
    let (out_files, out_bytes, in_files, in_bytes) = today;

    let level = if live.transfer_inflight > 0 { BUSY } else { OK };
    let status = if !live.transfer_running {
        "Not sharing".to_string()
    } else if live.transfer_inflight > 0 {
        format!("{} in flight", live.transfer_inflight)
    } else {
        "Listening".into()
    };

    // The address is the whole point of the row: a port on its own is not
    // something anyone can type into a phone.
    let server = if !live.transfer_running {
        ("Stopped".to_string(), "mut")
    } else if live.transfer_ip.is_empty() {
        (format!(":{}", live.transfer_port), "g")
    } else {
        (format!("{}:{}", live.transfer_ip, live.transfer_port), "g")
    };

    // Only ever "moving" or nothing. A steady "0 B/s" on an idle server is a
    // number that says less than a blank does.
    let rate = |b: i64| if b > 0 { format!("{}/s", bytes(b)) } else { "—".into() };
    let moving = sent_rate > 0 || recv_rate > 0;

    Sec::new(
        card("transfer", "Transfer", "orange", level, &status,
            vec![
                row("Server", server.0.clone(), server.1),
                row("Sending", rate(sent_rate), if sent_rate > 0 { "o" } else { "mut" }),
                row("Sent today", format!("{} · {}", num(out_files), bytes(out_bytes)),
                    if out_files > 0 { "g" } else { "mut" }),
                row("Paired devices", num(live.transfer_devices), ""),
                row("In flight", num(live.transfer_inflight),
                    if live.transfer_inflight > 0 { "o" } else { "mut" }),
            ],
            vec![
                kpi(bytes(out_bytes), "Sent today"),
                kpi(if moving { rate(sent_rate) } else { "Idle".into() }, "Sending now"),
                kpi(num(live.transfer_devices), "Paired devices"),
            ],
            vec![
                grp("Server", vec![
                    row("Address", server.0, server.1),
                    row("Paired devices", num(live.transfer_devices), ""),
                    row("Transfers in flight", num(live.transfer_inflight), ""),
                ]),
                grp("Right now", vec![
                    row("Sending", rate(sent_rate), if sent_rate > 0 { "o" } else { "mut" }),
                    row("Receiving", rate(recv_rate), if recv_rate > 0 { "o" } else { "mut" }),
                ]),
                grp("Today", vec![
                    row("Files sent", num(out_files), ""),
                    row("Bytes sent", bytes(out_bytes), ""),
                    row("Files received", num(in_files), ""),
                    row("Bytes received", bytes(in_bytes), ""),
                ]),
                // Deliberately absent: a lifetime total. The ledger prunes, so
                // "all time" here would mean "since the oldest row we kept".
            ]),
        level,
    )
}

/// Installed AI capability — not a job queue.
///
/// There is no transcription *queue* in this app: `whisper-cli` is spawned per
/// request and waited on. A "0 queued / idle" row would imply a background
/// worker that does not exist, so this card reports the only thing that is
/// actually true between runs — what is installed and reachable.
fn ai_card(live: &Live) -> Sec {
    let cli = which("whisper-cli") || tulipix_common::bundled_present("whisper-cli");
    let ready = live.ai_models > 0 && cli;
    let status = if ready {
        "Ready".to_string()
    } else if live.ai_models == 0 {
        "No models installed".into()
    } else {
        "whisper-cli missing".into()
    };

    Sec::new(
        card("ai", "AI & Models", "brand", OK, &status,
            vec![
                row("Models installed", num(live.ai_models),
                    if live.ai_models > 0 { "" } else { "mut" }),
                row("Models size", bytes(live.ai_models_bytes), ""),
                row("whisper-cli", if cli { "Found" } else { "Not found" },
                    if cli { "g" } else { "o" }),
                row("Transcription", if ready { "Available" } else { "Unavailable" },
                    if ready { "g" } else { "o" }),
            ],
            vec![
                kpi(num(live.ai_models), "Models"),
                kpi(bytes(live.ai_models_bytes), "On disk"),
                kpi(if ready { "Yes" } else { "No" }, "Transcription ready"),
            ],
            vec![grp("Installed", vec![
                row("Model files", num(live.ai_models), ""),
                row("Total size", bytes(live.ai_models_bytes), ""),
                row("whisper-cli", if cli { "Found" } else { "Not found" },
                    if cli { "g" } else { "o" }),
                // Deliberately absent: a queue depth and an "in flight" row.
                // Transcription here is a blocking spawn per request, and a
                // queue that is always empty reads as a queue that is broken.
            ])]),
        OK,
    )
}

fn system_card(live: &Live, lib_bytes: i64) -> Sec {
    let dbs = db_sizes();
    let db_total: i64 = dbs.iter().map(|(_, s)| *s).sum();
    let db_tbl: Vec<Value> = dbs.iter().map(|(name, size)| {
        json!([name, bytes(*size), tag(if *size > 0 { "Present" } else { "Empty" },
            if *size > 0 { "" } else { "i" })])
    }).collect();

    let rss = rss_bytes();

    Sec::new(
        card("system", "System", "slate", OK, "Healthy",
            vec![
                row("Version", live.app_version.clone(), ""),
                row("Session", dur(live.uptime_s), ""),
                row("Memory (RSS)", if rss > 0 { bytes(rss) } else { "—".into() },
                    if rss > 0 { "" } else { "mut" }),
                row("Database size", bytes(db_total), ""),
                row("Renderer", live.renderer.clone(), "mut"),
            ],
            vec![
                kpi(live.app_version.clone(), "Version"),
                kpi(if rss > 0 { bytes(rss) } else { "—".into() }, "Memory (RSS)"),
                kpi(dur(live.uptime_s), "Session"),
            ],
            vec![
                grp("Build", vec![
                    row("Version", live.app_version.clone(), ""),
                    row("Renderer", live.renderer.clone(), ""),
                    row("Platform", format!("{} · {}", std::env::consts::OS,
                        std::env::consts::ARCH), "mut"),
                ]),
                table("Databases", &["File", "Size", "State"], db_tbl),
                grp("Storage", vec![
                    row("Library total", bytes(lib_bytes), ""),
                    row("Databases", bytes(db_total), ""),
                ]),
            ]),
        OK,
    )
}

/// On-disk size of every section database. Names come from the backup manifest
/// so this list cannot drift from what a backup actually contains.
fn db_sizes() -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for name in tulipix_core::backup::INCLUDED_DIRS {
        if !name.ends_with(".db") {
            continue;
        }
        let section = name.trim_end_matches(".db");
        let size = tulipix_core::paths::db_path(section)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        out.push(((*name).to_string(), size));
    }
    // Not in the backup list but very much on disk.
    for extra in ["tools", "finances", "youtube"] {
        let size = tulipix_core::paths::db_path(extra)
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        if size > 0 {
            out.push((format!("{extra}.db"), size));
        }
    }
    out
}

/// Resident set size of this process. `/proc/self/statm` field 2 is resident
/// pages.
///
/// Two functions rather than one with a `cfg` block inside: an attribute on a
/// tail *expression* is not stable Rust, and the version that compiles on Linux
/// by falling through a `return` does not type-check anywhere else.
#[cfg(target_os = "linux")]
fn rss_bytes() -> i64 {
    let s = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: i64 = s.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0);
    pages * 4096
}

/// No cheap equivalent on Windows or macOS without a process-info dependency,
/// so the row reads "—". A wrong number would be worse than no number.
#[cfg(not(target_os = "linux"))]
fn rss_bytes() -> i64 {
    0
}

/// The counts that belong to no single card: the other two music databases,
/// and the library totals + 14-day sparkline the hero draws.
async fn extra_counts(week: i64) -> (i64, i64, i64, i64, i64, Vec<i64>) {
    let podcasts = match pool_for("podcasts").await {
        Ok(p) => n(&p, "SELECT COUNT(*) FROM podcasts").await,
        Err(_) => 0,
    };
    let radio = match pool_for("radio").await {
        Ok(p) => n(&p, "SELECT COUNT(*) FROM radio_stations").await,
        Err(_) => 0,
    };

    // Library totals and the growth curve, summed across every section that
    // shares the `items` schema.
    let day0 = (now() - 13 * 86_400) / 86_400;
    let mut buckets = vec![0i64; 14];
    let (mut total_items, mut total_bytes, mut added_week) = (0, 0, 0);

    for section in ["photos", "videos", "music"] {
        let Ok(pool) = pool_for(section).await else { continue };
        let (c, b) = items(&pool).await;
        total_items += c;
        total_bytes += b;
        added_week += n_at(&pool, "SELECT COUNT(*) FROM items WHERE added >= ?", week).await;

        let rows = sqlx::query_as::<_, (i64, i64)>(
            "SELECT added / 86400, COUNT(*) FROM items WHERE added >= ? GROUP BY 1")
            .bind(day0 * 86_400).fetch_all(&pool).await.unwrap_or_default();
        for (day, c) in rows {
            let idx = (day - day0).clamp(0, 13) as usize;
            buckets[idx] += c;
        }
    }
    // Books live outside the `items` schema but are still library.
    if let Ok(pool) = pool_for("books").await {
        let (c, b) = two(&pool,
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM books WHERE missing = 0").await;
        total_items += c;
        total_bytes += b;
        added_week += n_at(&pool, "SELECT COUNT(*) FROM books WHERE added_at >= ?", week).await;
    }

    // Cumulative, so the hero line only ever climbs — a per-day bar chart of
    // imports is a different (and much noisier) question than "is the library
    // growing", which is what the hero is answering.
    let mut running = 0;
    let spark = buckets.iter().map(|c| { running += c; running }).collect();

    (podcasts, radio, total_items, total_bytes, added_week, spark)
}

fn metrics(
    live: &Live,
    lib_bytes: i64,
    jobs_running: i64,
    jobs_failed: i64,
    cloud_status: &str,
    podcasts: i64,
    radio: i64,
) -> Value {
    let dbs = db_sizes();
    let db_total: i64 = dbs.iter().map(|(_, s)| *s).sum();
    let rss = rss_bytes();

    // Every tile carries its own explanation. A dashboard number with no
    // definition gets read as whatever the reader assumes it means, and
    // "Library 1.4 TB" is exactly the kind of figure two people would define
    // differently — hence `is` (what is counted) and `for` (why anyone cares).
    let db_rows: Vec<Value> = dbs.iter().map(|(n, s)| row(n, bytes(*s), "")).collect();

    json!([
        { "label": "Library", "value": bytes(lib_bytes), "note": "across all sections",
          "accent": "emerald",
          "help": {
            "is": "Total bytes of the media files Tulipix has indexed — photos, \
                   videos, music and books added together. Files still on disk \
                   only; anything the index has flagged as missing is left out.",
            "for": "It is the size of what you would lose, and what a backup or a \
                    move to a bigger disk has to carry. It grows when you add \
                    folders, never because Tulipix itself stored something.",
            "rows": vec![
                row("Indexed size", bytes(lib_bytes), ""),
                row("Where it lives", "your watched folders, untouched", "mut"),
            ],
          } },
        { "label": "Databases", "value": format!("{} files", dbs.len()),
          "note": bytes(db_total), "accent": "indigo",
          "help": {
            "is": "Tulipix's own index files — one SQLite database per section, \
                   holding metadata, thumbnails' bookkeeping, play history, \
                   ratings and settings. Not your media, which is never copied \
                   into them.",
            "for": "This is the part that a reset erases and a rescan rebuilds. \
                    It is normally a rounding error next to the library; if it \
                    is not, something is retaining far more history than it should.",
            "rows": db_rows,
          } },
        { "label": "Job queue", "value": num(jobs_running),
          "note": format!("{} failed 24h", num(jobs_failed)),
          "accent": "cyan",
          "help": {
            "is": "Background work from the Tools section — conversions, \
                   transcodes, downloads — plus any file moving over Transfer. \
                   The big number is running right now; the line under it is \
                   what errored in the last 24 hours.",
            "for": "A queue that never drains, or a failure count that keeps \
                    climbing, is the first sign a bundled binary went missing.",
            "rows": vec![
                row("Running now", num(jobs_running), if jobs_running > 0 { "o" } else { "mut" }),
                row("Failed 24h", num(jobs_failed), if jobs_failed > 0 { "rd" } else { "g" }),
            ],
          } },
        { "label": "Cloud", "value": cloud_status,
          "note": "rclone remotes", "accent": "teal",
          "help": {
            "is": "How many configured rclone remotes are currently mounted and \
                   reachable.",
            "for": "A remote that is configured but not mounted is a folder that \
                    silently is not there — which looks exactly like an empty \
                    folder until you go looking for a file.",
            "rows": vec![row("Remotes", cloud_status, "")],
          } },
        { "label": "Podcasts · Radio", "value": format!("{} · {}", num(podcasts), num(radio)),
          "note": "subscribed · stations", "accent": "violet",
          "help": {
            "is": "Podcast feeds you subscribe to, and saved radio stations. Both \
                   live outside the music library — they are subscriptions, not files.",
            "for": "They are the two things in Music that survive a library rescan, \
                    because nothing about them is on your disk to rescan.",
            "rows": vec![
                row("Podcast feeds", num(podcasts), ""),
                row("Radio stations", num(radio), ""),
            ],
          } },
        { "label": "Session", "value": dur(live.uptime_s),
          "note": if rss > 0 { format!("RSS {}", bytes(rss)) } else { "—".into() },
          "accent": "orange",
          "help": {
            "is": "How long this Tulipix process has been running, and how much \
                   memory it currently holds (resident set size).",
            "for": "Memory that only ever climbs across a long session is a leak; \
                    memory that settles is not. One reading tells you nothing — \
                    the point of showing it with an uptime is the pair.",
            "rows": vec![
                row("Uptime", dur(live.uptime_s), ""),
                row("Memory (RSS)", if rss > 0 { bytes(rss) } else { "—".into() },
                    if rss > 0 { "" } else { "mut" }),
            ],
          } },
    ])
}

/// What the Library popup shows before it offers to change anything: the
/// watched roots on disk, and what each media section actually found in them.
///
/// The per-section figures are lifted back out of the cards rather than
/// re-queried. They were built from the same tables one function ago, and two
/// passes over ten databases to print the same numbers twice is two chances for
/// the popup and the card behind it to disagree.
fn library(
    cards: &[Value],
    items: i64,
    size: i64,
    used_by: &std::collections::HashMap<String, Vec<String>>,
) -> Value {
    let roots: Vec<Value> = tulipix_common::load_watched_folders()
        .into_iter()
        .map(|p| {
            let exists = p.exists();
            let key = p.display().to_string();
            let secs = used_by.get(&key).cloned().unwrap_or_default();
            json!({
                "path": key,
                "state": if exists { "On disk" } else { "Missing" },
                "cls": if exists { "" } else { "b" },
                // What the folder is *for*, answered rather than assumed: every
                // section scans every root, so a root belongs to whichever ones
                // actually found something in it. Empty while a first scan is
                // still running, which is the honest state.
                "sections": secs,
            })
        })
        .collect();

    // One row per media section, pulled off the card it belongs to.
    let sections: Vec<Value> = ["photos", "videos", "music", "books"]
        .iter()
        .filter_map(|key| cards.iter().find(|c| c["key"] == **key))
        .map(|c| {
            // Card rows first, then the popup's groups. Music is the reason for
            // the second half: it states its size once, as "On disk" inside a
            // group, and never on the card face — so a lookup that read the
            // front rows alone came back empty for exactly one section, and the
            // fallback it then hit was the second KPI, which for music is a
            // listening time and not a size at all.
            let pick = |k: &str| {
                let hit = |rows: &Value| {
                    rows.as_array().and_then(|rows| {
                        rows.iter()
                            .find(|r| r["k"] == k)
                            .and_then(|r| r["v"].as_str())
                            .map(str::to_string)
                    })
                };
                hit(&c["rows"]).or_else(|| {
                    c["detail"]["groups"]
                        .as_array()
                        .and_then(|gs| gs.iter().find_map(|g| hit(&g["rows"])))
                })
            };
            json!({
                "name": c["name"],
                "key": c["key"],
                "accent": c["accent"],
                "count": pick("Items").or_else(|| pick("Tracks")).or_else(|| pick("Library"))
                    .unwrap_or_else(|| "—".into()),
                "size": pick("Size").or_else(|| pick("On disk")).unwrap_or_else(|| "—".into()),
                "missing": pick("Missing files").unwrap_or_else(|| "0".into()),
                "status": c["status"],
                "level": c["level"],
            })
        })
        .collect();

    json!({
        "roots": roots,
        "sections": sections,
        "items": num(items),
        "bytes": bytes(size),
        "databases": db_sizes().len(),
    })
}

/// Watched root → the section names that indexed something under it.
///
/// One `EXISTS` per section per root. Roots number a handful and `abs_path` is
/// indexed, so this is cheap; the alternative — tagging folders with a section
/// when they are added — would be a guess, and wrong the moment a folder holds
/// both photos and video, which is what a phone's camera roll is.
async fn root_sections() -> std::collections::HashMap<String, Vec<String>> {
    let roots = tulipix_common::load_watched_folders();
    let mut out: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    if roots.is_empty() {
        return out;
    }
    // Books is the odd one: it never adopted the shared `items` table and keeps
    // its own `books(path)`. Asking it the `items` question returns an error
    // that reads exactly like "nothing here", so the query is named per section
    // rather than assumed to be one shape.
    for (key, name, q) in [
        ("photos", "Photos", "SELECT 1 FROM items WHERE abs_path LIKE ? || '%' LIMIT 1"),
        ("videos", "Videos", "SELECT 1 FROM items WHERE abs_path LIKE ? || '%' LIMIT 1"),
        ("music",  "Music",  "SELECT 1 FROM items WHERE abs_path LIKE ? || '%' LIMIT 1"),
        ("books",  "Books",  "SELECT 1 FROM books WHERE path LIKE ? || '%' LIMIT 1"),
    ] {
        let Ok(pool) = pool_for(key).await else { continue };
        for p in &roots {
            // Trailing separator so `/media/Films` cannot claim `/media/Films2`.
            let mut prefix = p.display().to_string();
            if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
                prefix.push(std::path::MAIN_SEPARATOR);
            }
            let hit: Option<i64> = sqlx::query_scalar(q)
                .bind(&prefix)
                .fetch_optional(&pool).await.ok().flatten();
            if hit.is_some() {
                out.entry(p.display().to_string()).or_default().push(name.into());
            }
        }
    }
    out
}

/// Live indexing progress, one row per section that is scanning or has just
/// finished. Empty when nothing is running — the page hides the bar entirely
/// rather than parking it at 100%, because a full bar that never goes away
/// reads as a stuck job.
fn scan(rows: &[ScanRow]) -> Value {
    // The counters are not cleared when a scan ends — the app's own overlay
    // hides itself on a timer instead. So the whole group is gated on something
    // being active right now: once the last section finishes, the bars go, and a
    // section that finished early keeps its full bar until then.
    let running = rows.iter().any(|r| r.active);
    if !running {
        return json!({ "rows": [], "running": false });
    }
    let out: Vec<Value> = rows
        .iter()
        .filter(|r| r.active || r.total > 0)
        .map(|r| {
            let seen = r.added + r.failed;
            json!({
                "section": r.section,
                "total": r.total,
                "done": seen,
                "failed": r.failed,
                "active": r.active,
                // Clamped: a scan that finds more files than it first counted
                // would otherwise draw a bar past the end of its track.
                "pct": if r.total > 0 { (seen * 100 / r.total).clamp(0, 100) } else { 0 },
            })
        })
        .collect();
    json!({ "rows": out, "running": true })
}

/// Everything that happened, newest first — not only Tools jobs.
///
/// Three sources, because there is no general event table and three different
/// places keep the record: finished jobs (Tools' own database), completed
/// transfers (the ledger), and the app's activity log, which is where the
/// events with no table of their own go — a folder watched, a folder dropped,
/// a rescan started, from the app or from this page. Merged in Rust rather
/// than unioned in SQL: two of the three are separate databases and the third
/// is a file.
fn timeline(
    jobs: &[(i64, String, String, String)],
    transfers: &[(i64, String, String, String)],
) -> Value {
    let mut all: Vec<(i64, String, String, String)> = Vec::new();
    all.extend_from_slice(jobs);
    all.extend_from_slice(transfers);
    all.extend(tulipix_common::load_activity());
    all.sort_by(|a, b| b.0.cmp(&a.0));
    all.truncate(12);

    let mut out: Vec<Value> = all.iter().map(|(at, accent, title, desc)| {
        json!({ "at": clock(*at), "accent": accent, "title": title, "desc": desc })
    }).collect();
    if out.is_empty() {
        out.push(json!({
            "at": clock(now()), "accent": "slate",
            "title": "Nothing to report",
            "desc": "No jobs, transfers or library changes recorded yet",
        }));
    }
    Value::Array(out)
}

/// The last few completed transfers, in timeline shape. Both directions: a
/// phone that pushed a file to the desktop is exactly the kind of thing this
/// page exists to show, and it happens without anyone touching the app.
async fn transfer_events() -> Vec<(i64, String, String, String)> {
    let Ok(pool) = pool_for("transfers").await else { return Vec::new() };
    sqlx::query_as::<_, (String, String, i64, String, String, i64)>(
        "SELECT direction, name, bytes, peer, status, at FROM transfers \
         ORDER BY at DESC LIMIT 8")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(dir, name, n, peer, status, at)| {
            let verb = match (dir.as_str(), status.as_str()) {
                (_, s) if s != "ok" => "Transfer failed",
                ("in", _) => "Received",
                _ => "Sent",
            };
            (at, "teal".to_string(), format!("{verb} — {name}"),
             format!("{} · {peer}", bytes(n.max(0))))
        })
        .collect()
}

fn today(music: i64, photos: i64, tools: i64, finances: i64) -> Value {
    json!([
        { "label": "Tracks played", "value": num(music), "accent": "pink" },
        { "label": "Photos imported", "value": num(photos), "accent": "violet" },
        { "label": "Tools jobs done", "value": num(tools), "accent": "cyan" },
        { "label": "Transactions", "value": num(finances), "accent": "lime" },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The formatters carry every number the page shows, so they are the one
    /// thing here worth pinning: an off-by-one in the thousands separator or a
    /// unit step is silently wrong rather than loudly broken.
    #[test]
    fn formatters() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1_572_864), "1.5 MB");
        // Crosses into the three-digit branch, which drops the decimal.
        assert_eq!(bytes(200 * 1024 * 1024), "200 MB");

        assert_eq!(num(0), "0");
        assert_eq!(num(999), "999");
        assert_eq!(num(1_000), "1,000");
        assert_eq!(num(21_704), "21,704");
        assert_eq!(num(-4_512), "-4,512");

        assert_eq!(dur(0), "—");
        assert_eq!(dur(59), "0m");
        assert_eq!(dur(720), "12m");
        assert_eq!(dur(15_120), "4h 12m");
    }

    /// Worst-of folding: one problem outranks any amount of busy, and busy
    /// outranks idle. This is what decides the LED colour.
    #[test]
    fn level_folding() {
        assert_eq!(worse(OK, BUSY), BUSY);
        assert_eq!(worse(BUSY, OK), BUSY);
        assert_eq!(worse(BUSY, PROBLEM), PROBLEM);
        assert_eq!(worse(PROBLEM, BUSY), PROBLEM);
        assert_eq!(worse(OK, OK), OK);
        // Unknown never wins — a section that failed to open says so on its own
        // card, and should not paint the whole app red.
        assert_eq!(worse(OK, UNKNOWN), OK);
    }

    /// The rate is derived, not reported, so it is the one number on the
    /// Transfer card that can be wrong without any query being wrong.
    #[test]
    fn wire_rate_needs_two_readings() {
        let slot = std::sync::Mutex::new((0u64, 0i64));
        // Nothing to subtract from yet.
        assert_eq!(wire_rate(&slot, 1_000, 100), 0);
        // 4 KB over 2 s.
        assert_eq!(wire_rate(&slot, 9_192, 102), 4_096);
        // Same instant twice: not a rate, not a division by zero.
        assert_eq!(wire_rate(&slot, 20_000, 102), 0);
        // The server restarted and the counter went back to zero.
        assert_eq!(wire_rate(&slot, 5, 110), 0);
        // And it picks straight back up from the new baseline.
        assert_eq!(wire_rate(&slot, 105, 111), 100);
    }

    #[test]
    fn lateness_reads_as_english() {
        assert_eq!(plural_days(1), "1 day");
        assert_eq!(plural_days(6), "6 days");
        // Due today is not late; neither is a clock skew that reads negative.
        assert_eq!(plural_days(0), "today");
        assert_eq!(plural_days(-2), "today");
    }

    #[test]
    fn queue_rows_name_the_file_not_the_path() {
        assert_eq!(file_name("/home/a/Pictures/trip.jpg"), "trip.jpg");
        assert_eq!(file_name(r"C:\Users\a\trip.jpg"), "trip.jpg");
        // A trailing separator has no last component; showing the whole thing
        // beats showing an empty cell.
        assert_eq!(file_name("/home/a/"), "/home/a/");
        assert_eq!(file_name("trip.jpg"), "trip.jpg");
    }

    /// A "today" that rolls over at UTC midnight is the wrong day for most of
    /// the planet for part of every day.
    #[test]
    fn midnight_is_local_and_is_a_midnight() {
        let m = local_midnight();
        assert!(m <= now(), "midnight has already happened");
        assert!(now() - m < 86_400, "and it was less than a day ago");
        assert_eq!((m + local_offset()).rem_euclid(86_400), 0, "on a local day boundary");
    }

    /// The timeline is three sources folded into one list, and the fold is the
    /// only place that can put them out of order. Timestamps are set far in the
    /// future so the assertion holds against a real activity log, whatever is
    /// in it.
    #[test]
    fn the_timeline_is_one_list_newest_first() {
        let ev = |at: i64, t: &str| (at, "cyan".to_string(), t.to_string(), String::new());
        let out = timeline(
            &[ev(4_000_000_100, "a job")],
            &[ev(4_000_000_300, "a send"), ev(4_000_000_050, "an old send")],
        );
        let titles: Vec<&str> = out.as_array().unwrap().iter()
            .map(|e| e["title"].as_str().unwrap()).collect();
        let at = |t: &str| titles.iter().position(|x| *x == t).unwrap_or_else(|| panic!("{t}"));
        assert!(at("a send") < at("a job"), "newest first");
        assert!(at("a job") < at("an old send"), "and the fold does not group by source");
    }

    /// Music states its size only inside a popup group, which is what used to
    /// leave exactly one row of the library table's Size column blank.
    #[test]
    fn a_section_that_states_its_size_in_a_group_still_fills_the_size_column() {
        let c = card("music", "Music", "pink", OK, "Operational",
            vec![row("Tracks", "12", "")],
            vec![kpi("12", "Tracks"), kpi("4h 20m", "Played this week")],
            vec![grp("Library", vec![row("On disk", "1.5 MB", "")])]);
        let v = library(&[c], 12, 1_572_864, &Default::default());
        assert_eq!(v["sections"][0]["count"], "12");
        assert_eq!(v["sections"][0]["size"], "1.5 MB", "not the second KPI");
    }

    #[test]
    fn mount_states_are_distinct() {
        assert_eq!(mount_state(Some("mounted")).0, "Mounted");
        assert_eq!(mount_state(Some("error")).1, "b");
        // No mount row at all is "never mounted", not "failed".
        assert_eq!(mount_state(None).1, "i");
    }
}
