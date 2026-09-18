//! Status: the dashboard the loopback HTML page draws, as a Flutter section.
//!
//! [`tulipix_status::collect`] produces one JSON `Value` per refresh. The
//! browser page renders that; so does `page_status.slint`; so does this. All
//! three report identical numbers by construction -- there is no second set of
//! queries here, and nothing in this file may invent a figure the snapshot does
//! not carry.
//!
//! Two things the Slint glue had to do are gone. Accents arrive as their
//! *names* and are resolved to colours in Dart, because a theme that can change
//! at runtime should not have its palette baked in Rust; and the seven-day
//! sparkline crosses as its points rather than as two SVG paths, because a
//! `CustomPainter` can accumulate a path and Slint cannot.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

/// One `key: value` line, on a card face or inside a popup group. `cls` is the
/// value's tint, carried verbatim from the JSON: "g" good, "o" attention,
/// "rd" bad, "mut" muted, "" plain.
pub struct StRow {
    pub k: String,
    pub v: String,
    pub cls: String,
}

/// One of the three big figures across the top of a card's popup.
pub struct StKpi {
    pub v: String,
    pub k: String,
}

/// One cell of a popup table. `tag` turns it into a pill; `cls` tints it.
pub struct StCell {
    pub text: String,
    pub cls: String,
    pub tag: bool,
}

pub struct StTableRow {
    pub cells: Vec<StCell>,
}

/// A block inside a popup. Either a list of rows *or* a table, never both, and
/// `cols` being non-empty is what says which.
pub struct StGroup {
    pub h: String,
    pub rows: Vec<StRow>,
    pub cols: Vec<String>,
    pub table: Vec<StTableRow>,
}

/// One section's card. `nav` is whether the section exists in the app: AI and
/// System are pages of this dashboard, not places to go.
pub struct StCard {
    pub key: String,
    pub name: String,
    pub accent: String,
    pub level: String,
    pub status: String,
    pub why: String,
    pub more: String,
    pub nav: bool,
    pub rows: Vec<StRow>,
    pub kpis: Vec<StKpi>,
    pub groups: Vec<StGroup>,
}

/// One tile of the strip under the header. Every one carries its own
/// explanation, because a number nobody can explain is a number nobody trusts.
pub struct StMetric {
    pub label: String,
    pub value: String,
    pub note: String,
    pub accent: String,
    pub help_is: String,
    pub help_for: String,
    pub rows: Vec<StRow>,
}

pub struct StEvent {
    pub at: String,
    pub accent: String,
    pub title: String,
    pub desc: String,
}

pub struct StToday {
    pub label: String,
    pub value: String,
    pub accent: String,
}

/// A watched root, and which sections found something under it.
pub struct StRoot {
    pub path: String,
    pub state: String,
    pub bad: bool,
    pub sections: Vec<String>,
}

/// One row of the library popup's "What is indexed" table.
pub struct StLibSection {
    pub name: String,
    pub accent: String,
    pub count: String,
    pub size: String,
    pub status: String,
    pub level: String,
}

/// Live indexing progress for one section. `waiting` is a scan that has not
/// finished counting its files yet, which draws as an indeterminate bar rather
/// than as one sitting at zero while the disk is clearly busy.
pub struct StScan {
    pub section: String,
    pub done: String,
    pub total: String,
    pub failed: String,
    pub pct: i32,
    pub active: bool,
    pub waiting: bool,
}

/// Everything the page draws, from one snapshot.
pub struct StatusState {
    pub level: String,
    pub headline: String,
    pub note: String,
    pub updated: String,
    pub sections_online: String,
    pub jobs_running: String,
    pub jobs_failed: String,
    pub library_bytes: String,
    pub library_items: String,
    pub library_week: String,
    /// Seven days of library growth, oldest first. Dart paints it.
    pub spark: Vec<f64>,
    pub metrics: Vec<StMetric>,
    pub cards: Vec<StCard>,
    pub timeline: Vec<StEvent>,
    pub today: Vec<StToday>,
    pub lib_items: String,
    pub lib_bytes: String,
    pub lib_databases: String,
    pub roots: Vec<StRoot>,
    pub lib_sections: Vec<StLibSection>,
    pub scan_running: bool,
    pub scan: Vec<StScan>,
    /// Where the System and Tools cards sit in `cards`. The three header
    /// figures open the card that produced them, and `collect()` is free to
    /// change the order, so the two are resolved by key here.
    pub system_idx: i32,
    pub tools_idx: i32,
    /// What the last action did, cleared by the next refresh.
    pub notice: String,
    pub error: String,
}

pub enum StatusCmd {
    Refresh,
    /// Re-add every watched folder to every section and kick their scans.
    Rescan,
    AddFolder { path: String },
    /// Erase every index, setting and cached thumbnail. The app does not
    /// restart itself the way the Slint build does -- see `reset_app`.
    ResetApp,
}

/// Apply one command and hand back the snapshot that follows it.
pub async fn status_dispatch(cmd: StatusCmd) -> Result<StatusState> {
    let mut notice = String::new();
    let mut error = String::new();
    match cmd {
        StatusCmd::Refresh => {}
        StatusCmd::Rescan => {
            rescan().await;
            notice = "Rescan started.".into();
        }
        StatusCmd::AddFolder { path } => match add_folder(Path::new(&path)) {
            true => notice = format!("Watching {path}. Rescan to index it."),
            false => error = format!("{path} is already watched, or could not be saved."),
        },
        StatusCmd::ResetApp => match reset_app() {
            Ok(()) => {
                notice = "Everything erased. Tulipix is starting again…".into();
                relaunch();
            }
            Err(e) => error = e.to_string(),
        },
    }

    let snap = tulipix_status::collect::collect(&live()).await;
    let mut state = parse(&snap.json);
    state.level = snap.level;
    state.notice = notice;
    state.error = error;
    Ok(state)
}

/// The word someone has to type before the reset button arms. Exported so the
/// page cannot disagree with the check.
#[flutter_rust_bridge::frb(sync)]
pub fn status_reset_phrase() -> String {
    tulipix_status::RESET_PHRASE.to_string()
}

// ── the snapshot ────────────────────────────────────────────────────────────

/// What only the running app knows. The Flutter build serves transfers from
/// the same in-process service the Slint one does, so those figures are real;
/// the scan counters are not, because this build has no central scan registry
/// and an invented number is worse than an honest zero.
fn live() -> tulipix_status::Live {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let x = crate::api::transfer::status_snapshot(now);
    let (models, model_bytes) = models_on_disk();
    tulipix_status::Live {
        transfer_running: x.running,
        transfer_port: x.port,
        transfer_ip: x.address,
        transfer_devices: x.devices,
        transfer_inflight: x.inflight,
        transfer_sent: x.sent_bytes,
        transfer_recv: x.recv_bytes,
        ai_models: models,
        ai_models_bytes: model_bytes,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        renderer: "flutter".into(),
        uptime_s: tulipix_status::uptime_secs(),
        scan: Vec::new(),
    }
}

/// Model files installed under the app's models directory, and what they weigh.
fn models_on_disk() -> (i64, i64) {
    let Some(root) = tulipix_photos::ai::models::models_root() else { return (0, 0) };
    let (mut count, mut total) = (0i64, 0i64);
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".onnx") || name.ends_with(".bin") || name.ends_with(".gguf") {
                count += 1;
                total += e.metadata().map(|m| m.len() as i64).unwrap_or(0);
            }
        }
    }
    (count, total)
}

// ── JSON → structs ──────────────────────────────────────────────────────────
//
// One function per shape, named for the key it reads, in the order the page
// draws them. Every one swallows a missing key into an empty string: a status
// page that renders the word "null" over a figure is worse than one that shows
// nothing there.

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn text(v: &Value, key: &str) -> String {
    s(&v[key])
}

/// A number from the snapshot as the string the page prints. Not `to_string`
/// on the `Value`: a key that is missing would render "null" into the header.
fn numstr(v: &Value, key: &str) -> String {
    v[key].as_i64().unwrap_or(0).to_string()
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn rows(v: &Value) -> Vec<StRow> {
    arr(v, "rows")
        .iter()
        .map(|r| StRow { k: text(r, "k"), v: text(r, "v"), cls: text(r, "cls") })
        .collect()
}

/// One popup block. A group is a row list *or* a table, never both, and `cols`
/// being non-empty is what the page reads to tell them apart -- so a row group
/// leaves it empty rather than emitting a header nothing sits under.
fn group(g: &Value) -> StGroup {
    let table = &g["table"];
    if !table.is_object() {
        return StGroup {
            h: text(g, "h"),
            rows: rows(g),
            cols: Vec::new(),
            table: Vec::new(),
        };
    }
    StGroup {
        h: text(g, "h"),
        rows: Vec::new(),
        cols: arr(table, "cols").iter().map(s).collect(),
        table: arr(table, "rows")
            .iter()
            .map(|r| StTableRow {
                cells: r
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .map(cell)
                    .collect(),
            })
            .collect(),
    }
}

/// A cell is either a plain scalar or `{tag, cls}`.
fn cell(c: &Value) -> StCell {
    match c.get("tag") {
        Some(t) => StCell { text: s(t), cls: text(c, "cls"), tag: true },
        // Numbers arrive unquoted, so `as_str` alone would blank them.
        None => StCell {
            text: match c.as_str() {
                Some(t) => t.to_string(),
                None if c.is_null() => String::new(),
                None => c.to_string(),
            },
            cls: String::new(),
            tag: false,
        },
    }
}

/// Sections that exist in the app. "AI & Whisper" and "System" are pages of
/// this dashboard, not places to navigate to -- the same list the HTML page's
/// `NAV` holds, and for the same reason.
const NAV: [&str; 8] =
    ["photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances"];

fn card(c: &Value) -> StCard {
    let key = text(c, "key");
    let detail = &c["detail"];
    StCard {
        name: text(c, "name"),
        accent: text(c, "accent"),
        level: text(c, "level"),
        status: text(c, "status"),
        why: text(c, "why"),
        more: text(c, "more"),
        nav: NAV.contains(&key.as_str()),
        rows: rows(c),
        kpis: arr(detail, "kpis")
            .iter()
            .map(|k| StKpi { v: s(&k[0]), k: s(&k[1]) })
            .collect(),
        groups: arr(detail, "groups").iter().map(group).collect(),
        key,
    }
}

fn metric(m: &Value) -> StMetric {
    let help = &m["help"];
    StMetric {
        label: text(m, "label"),
        value: text(m, "value"),
        note: text(m, "note"),
        accent: text(m, "accent"),
        help_is: text(help, "is"),
        help_for: text(help, "for"),
        rows: rows(help),
    }
}

fn parse(d: &Value) -> StatusState {
    let hero = &d["hero"];
    let lib = &d["library"];
    let scan = &d["scan"];
    let cards = arr(d, "cards");
    let idx_of = |key: &str| {
        cards.iter().position(|c| c["key"] == key).map(|i| i as i32).unwrap_or(-1)
    };

    StatusState {
        // Overwritten by the caller from `Snapshot.level`, which is the
        // worst-of across every card and is not in the JSON.
        level: "unknown".into(),
        headline: text(d, "headline"),
        note: text(d, "note"),
        // The same "3:25 PM" the browser footer prints, from the same function
        // -- two clocks that agree because there is only one.
        updated: tulipix_status::collect::clock(d["updated"].as_i64().unwrap_or(0)),
        sections_online: numstr(hero, "sections_online"),
        jobs_running: numstr(hero, "jobs_running"),
        jobs_failed: numstr(hero, "jobs_failed"),
        library_bytes: text(hero, "library_bytes"),
        library_items: text(hero, "library_items"),
        library_week: numstr(hero, "library_week"),
        spark: arr(hero, "spark").iter().filter_map(Value::as_f64).collect(),
        metrics: arr(d, "metrics").iter().map(metric).collect(),
        system_idx: idx_of("system"),
        tools_idx: idx_of("tools"),
        cards: cards.iter().map(card).collect(),
        timeline: arr(d, "timeline")
            .iter()
            .map(|e| StEvent {
                at: text(e, "at"),
                accent: text(e, "accent"),
                title: text(e, "title"),
                desc: text(e, "desc"),
            })
            .collect(),
        today: arr(d, "today")
            .iter()
            .map(|t| StToday {
                label: text(t, "label"),
                value: text(t, "value"),
                accent: text(t, "accent"),
            })
            .collect(),
        lib_items: text(lib, "items"),
        lib_bytes: text(lib, "bytes"),
        lib_databases: numstr(lib, "databases"),
        roots: arr(lib, "roots")
            .iter()
            .map(|r| StRoot {
                path: text(r, "path"),
                state: text(r, "state"),
                // "b" is the page's bad-pill class; the only root that carries
                // one is a watched folder that is no longer on disk.
                bad: r["cls"] == "b",
                sections: arr(r, "sections").iter().map(s).collect(),
            })
            .collect(),
        lib_sections: arr(lib, "sections")
            .iter()
            .map(|x| StLibSection {
                name: text(x, "name"),
                accent: text(x, "accent"),
                count: text(x, "count"),
                size: text(x, "size"),
                status: text(x, "status"),
                level: text(x, "level"),
            })
            .collect(),
        scan_running: scan["running"].as_bool().unwrap_or(false),
        scan: arr(scan, "rows")
            .iter()
            .map(|r| StScan {
                section: text(r, "section"),
                done: numstr(r, "done"),
                total: numstr(r, "total"),
                failed: numstr(r, "failed"),
                pct: r["pct"].as_i64().unwrap_or(0) as i32,
                active: r["active"].as_bool().unwrap_or(false),
                // A scan that has not finished counting its files: the bar has
                // nothing honest to show, so the page says so in words.
                waiting: r["active"].as_bool().unwrap_or(false)
                    && r["total"].as_i64().unwrap_or(0) == 0,
            })
            .collect(),
        notice: String::new(),
        error: String::new(),
    }
}

// ── the three actions ───────────────────────────────────────────────────────

/// Re-index every section over every watched folder.
///
/// Sequential rather than `join!`ed: four scans walking the same disks at once
/// is slower than four in a row, and each one emits its own progress events
/// that its own section page is already listening for.
async fn rescan() {
    // Named on the Libraries tab's bar while it runs. A rescan started while
    // another job holds the slot still runs; it is just not the one named.
    let _job = crate::api::maintenance::begin("rescan-all", "Rescanning every watched folder");
    rescan_sections().await;
    crate::api::maintenance::stamp_scanned(&tulipix_common::load_watched_folders());
    crate::api::maintenance::refresh_counts().await;
}

async fn rescan_sections() {
    if let Ok(pool) = crate::db::photos_pool().await {
        crate::api::photos::scan_watched(pool).await;
    }
    if let Err(e) = crate::api::music::scan_watched().await {
        tracing::warn!(error = %e, "status: music rescan");
    }
    crate::api::videos::scan_watched().await;
    if let Ok(pool) = crate::db::books_pool().await
        && let Err(e) = crate::api::books::run_scan(pool).await
    {
        tracing::warn!(error = %e, "status: books rescan");
    }
}

fn watched_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

/// Add one folder to the shared watched list. Same file, same format as the
/// four sections that read it, so a folder added here is a folder they see.
fn add_folder(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let Some(p) = watched_path() else { return false };
    let mut existing: Vec<PathBuf> = std::fs::read_to_string(&p)
        .ok()
        .and_then(|b| serde_json::from_str::<Vec<String>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if existing.iter().any(|x| x == dir) {
        return false;
    }
    existing.push(dir.to_path_buf());
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let list: Vec<String> = existing.iter().map(|x| x.to_string_lossy().into_owned()).collect();
    match serde_json::to_string_pretty(&list) {
        Ok(body) => std::fs::write(&p, body).is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "status: serialise watched folders");
            false
        }
    }
}

/// Erase every index, setting and cached thumbnail.
///
/// One difference from the Slint build, and it is visible: that one relaunches
/// itself with `--factory-reset` to re-wipe whatever a locked handle left
/// behind. A cdylib has no process of its own to relaunch, so this wipes what
/// it can and the page asks for a restart. The pools already open in this
/// process keep their unlinked files alive until then, which on Linux is
/// harmless and on Windows leaves the databases to the next start.
/// Start a fresh copy and leave, as the Slint build does after a reset: this
/// process holds open the databases that were just deleted under it, and only
/// a new one sees the empty library cleanly. A beat first, so the page can say
/// what is happening.
fn relaunch() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(1200));
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
        std::process::exit(0);
    });
}

fn reset_app() -> Result<()> {
    for dir in [
        tulipix_core::paths::data_dir(),
        tulipix_core::paths::config_dir(),
        tulipix_core::paths::cache_dir(),
    ]
    .into_iter()
    .flatten()
    {
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page tells a table group from a row group by `cols` being non-empty,
    /// so a row group that emitted columns would draw a header over nothing.
    #[test]
    fn a_row_group_carries_no_columns_and_a_table_group_carries_no_rows() {
        let g = group(&serde_json::json!({
            "h": "Library", "rows": [{ "k": "a", "v": "1", "cls": "" }],
        }));
        assert_eq!(g.rows.len(), 1);
        assert!(g.cols.is_empty());

        let t = group(&serde_json::json!({
            "h": "Queue",
            "table": { "cols": ["File", "State"], "rows": [["a.bin", { "tag": "Running", "cls": "w" }]] },
        }));
        assert!(t.rows.is_empty());
        assert_eq!(t.cols.len(), 2);
        assert!(!t.table[0].cells[0].tag, "a plain cell is not a pill");
        assert!(t.table[0].cells[1].tag, "and a {{tag}} cell is");
    }

    /// Mirrors put numbers in table cells unquoted, and `as_str` alone blanks
    /// them -- which is how a queue depth of 12 became an empty column.
    #[test]
    fn a_numeric_cell_keeps_its_digits() {
        assert_eq!(cell(&serde_json::json!(12)).text, "12");
        assert_eq!(cell(&serde_json::json!(null)).text, "");
    }

    /// AI and System are pages of this dashboard, not places to go.
    #[test]
    fn only_real_sections_offer_a_jump_button() {
        let mk = |key: &str| card(&serde_json::json!({ "key": key, "detail": {} }));
        assert!(mk("music").nav);
        assert!(!mk("ai").nav);
        assert!(!mk("system").nav);
    }

    /// The three header figures open a card by index, and `collect()` is free
    /// to reorder -- so an absent card must read as -1, not as card zero.
    #[test]
    fn the_header_figures_resolve_their_card_by_key() {
        let d = serde_json::json!({ "cards": [{ "key": "music" }, { "key": "system" }] });
        let st = parse(&d);
        assert_eq!(st.system_idx, 1);
        assert_eq!(st.tools_idx, -1);
    }
}
