//! Status-section wiring: the snapshot the browser dashboard draws, as Slint
//! models.
//!
//! [`tulipix_status::collect`] produces one JSON `Value` per refresh. The
//! loopback server hands that straight to the HTML page; this crate hands the
//! *same* `Value` to `page_status.slint`. Both renderings therefore report
//! identical numbers by construction — there is no second set of queries here,
//! and nothing in this file may invent a figure the snapshot does not carry.
//!
//! Everything is built on the UI thread. Each `StCard` owns a `ModelRc` of its
//! rows, and a `ModelRc` is an `Rc`, so the structs are `!Send` and cannot be
//! posted across into the event loop already assembled.

use slint::{Color, Model, ModelRc, SharedString, VecModel};
use tulipix_ui::*;

/// The dashboard palette, resolved from the accent *names* the snapshot uses.
///
/// The HTML page reads these as CSS custom properties; here they are literals,
/// which is one of the real costs of the native rendering — two files now hold
/// the same ten hues, and only the stylesheet's copy is themable at runtime.
fn accent(name: &str) -> Color {
    let rgb: u32 = match name {
        "violet" => 0x8b5cf6,
        "indigo" => 0x6366f1,
        "pink" => 0xec4899,
        "emerald" => 0x10b981,
        "teal" => 0x14b8a6,
        "cyan" => 0x06b6d4,
        "orange" => 0xf97316,
        "lime" => 0x84cc16,
        "brand" => 0x7c3aed,
        // Anything unrecognised is slate, exactly as the page's `accent()` does.
        _ => 0x94a3b8,
    };
    Color::from_argb_encoded(0xFF00_0000 | rgb)
}

fn s(v: &serde_json::Value) -> SharedString {
    v.as_str().unwrap_or("").into()
}

fn text(v: &serde_json::Value, key: &str) -> SharedString {
    s(&v[key])
}

/// A number from the snapshot as the string the page prints. Not `to_string`
/// on the `Value`: a key that is missing would render the word "null" into the
/// middle of the header.
fn numstr(v: &serde_json::Value, key: &str) -> SharedString {
    v[key].as_i64().unwrap_or(0).to_string().into()
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}

/// Update a list the window already holds, in place where possible.
///
/// Handing Slint a *new* `ModelRc` on every tick tears down and rebuilds every
/// repeated row. This page refreshes every two seconds while it is on screen,
/// so a row was routinely destroyed a moment after its own button was pressed —
/// and a binding in that row evaluating afterwards is slint#6426, "accessing
/// deleted parent". `set_vec` keeps the model instance the repeater is attached
/// to and swaps only the data. The first call has no `VecModel` to update, so
/// it installs one and every call after that reuses it.
fn put<T: Clone + 'static>(
    current: ModelRc<T>,
    rows: Vec<T>,
    install: impl FnOnce(ModelRc<T>),
) {
    match current.as_any().downcast_ref::<VecModel<T>>() {
        Some(vm) => vm.set_vec(rows),
        None => install(model(rows)),
    }
}

fn arr<'a>(v: &'a serde_json::Value, key: &str) -> &'a [serde_json::Value] {
    v[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn rows(v: &serde_json::Value) -> Vec<StRow> {
    arr(v, "rows")
        .iter()
        .map(|r| StRow { k: text(r, "k"), v: text(r, "v"), cls: text(r, "cls") })
        .collect()
}

/// One popup block. A group is a row list *or* a table, never both, and `cols`
/// being non-empty is what the page reads to tell them apart — so a row group
/// leaves it empty rather than emitting a header nothing sits under.
fn group(g: &serde_json::Value) -> StGroup {
    let table = &g["table"];
    if table.is_object() {
        let cols: Vec<SharedString> = arr(table, "cols").iter().map(s).collect();
        let body: Vec<StTableRow> = arr(table, "rows")
            .iter()
            .map(|r| StTableRow {
                cells: model(
                    r.as_array()
                        .map(Vec::as_slice)
                        .unwrap_or(&[])
                        .iter()
                        .map(|c| {
                            // A cell is either a plain scalar or `{tag, cls}`.
                            match c.get("tag") {
                                Some(t) => StCell {
                                    text: s(t),
                                    cls: text(c, "cls"),
                                    tag: true,
                                },
                                // A scalar cell. Numbers arrive unquoted, so
                                // `as_str` alone would blank them.
                                None => StCell {
                                    text: match c.as_str() {
                                        Some(t) => t.into(),
                                        None if c.is_null() => SharedString::new(),
                                        None => c.to_string().into(),
                                    },
                                    cls: SharedString::new(),
                                    tag: false,
                                },
                            }
                        })
                        .collect(),
                ),
            })
            .collect();
        StGroup {
            h: text(g, "h"),
            rows: model(Vec::new()),
            cols: model(cols),
            table: model(body),
        }
    } else {
        StGroup {
            h: text(g, "h"),
            rows: model(rows(g)),
            cols: model(Vec::new()),
            table: model(Vec::new()),
        }
    }
}

/// Sections that exist in the app. "AI & Whisper" and "System" are pages of
/// this dashboard, not places to navigate to — the same list the HTML page's
/// `NAV` holds, and for the same reason.
const NAV: [&str; 8] =
    ["photos", "videos", "music", "books", "cloud", "tools", "transfer", "finances"];

fn card(c: &serde_json::Value) -> StCard {
    let key = text(c, "key");
    let detail = &c["detail"];
    StCard {
        name: text(c, "name"),
        accent: accent(c["accent"].as_str().unwrap_or("")),
        level: text(c, "level"),
        status: text(c, "status"),
        why: text(c, "why"),
        more: text(c, "more"),
        nav: NAV.contains(&key.as_str()),
        rows: model(rows(c)),
        kpis: model(
            arr(detail, "kpis")
                .iter()
                .map(|k| StKpi { v: s(&k[0]), k: s(&k[1]) })
                .collect(),
        ),
        groups: model(arr(detail, "groups").iter().map(group).collect()),
        key,
    }
}

fn metric(m: &serde_json::Value) -> StMetric {
    let help = &m["help"];
    StMetric {
        label: text(m, "label"),
        value: text(m, "value"),
        note: text(m, "note"),
        accent: accent(m["accent"].as_str().unwrap_or("")),
        help_is: text(help, "is"),
        help_for: text(help, "for"),
        rows: model(rows(help)),
    }
}

/// The seven-day sparkline as two SVG paths over a 100×100 viewbox: the line,
/// and the area closed to the floor under it.
///
/// The browser draws this from a point list with an inline `<svg>`; Slint has
/// no way to accumulate a path over a model, so the geometry is done here. Same
/// arithmetic, different side of the boundary — the sharpest small example of
/// what the two renderings actually cost.
fn spark(points: &[serde_json::Value]) -> (SharedString, SharedString) {
    let pts: Vec<f64> = points.iter().filter_map(serde_json::Value::as_f64).collect();
    if pts.len() < 2 {
        return (SharedString::new(), SharedString::new());
    }
    let lo = pts.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = pts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // A flat week is a straight line, not a division by zero.
    let span = if hi > lo { hi - lo } else { 1.0 };
    let n = (pts.len() - 1) as f64;

    let mut line = String::new();
    for (i, v) in pts.iter().enumerate() {
        let x = i as f64 / n * 100.0;
        let y = 92.0 - (v - lo) / span * 84.0;
        line.push_str(&format!("{} {x:.1} {y:.1} ", if i == 0 { "M" } else { "L" }));
    }
    let line = line.trim_end().to_string();
    let area = format!("{line} L 100 100 L 0 100 Z");
    (line.into(), area.into())
}

/// Push one snapshot into the window. Cheap enough to run on every refresh
/// tick, but the caller only does so while the section is on screen — rebuilding
/// ten nested models for a page nobody is looking at is exactly the kind of work
/// the HTML page's "slow tick when unwatched" rule exists to avoid.
pub fn apply(w: &MainWindow, d: &serde_json::Value) {
    let hero = &d["hero"];
    w.set_st_headline(text(d, "headline"));
    w.set_st_note(text(d, "note"));
    // The same "3:25 PM" the browser footer prints, from the same function —
    // two clocks that agree because there is only one.
    w.set_st_updated(
        tulipix_status::collect::clock(d["updated"].as_i64().unwrap_or(0)).into(),
    );
    w.set_st_sections_online(numstr(hero, "sections_online"));
    w.set_st_jobs_running(numstr(hero, "jobs_running"));
    w.set_st_jobs_failed(numstr(hero, "jobs_failed"));
    w.set_st_library_bytes(text(hero, "library_bytes"));
    w.set_st_library_items(text(hero, "library_items"));
    w.set_st_library_week(numstr(hero, "library_week"));

    let (line, area) = spark(arr(hero, "spark"));
    w.set_st_spark_line(line);
    w.set_st_spark_area(area);

    put(w.get_st_metrics(), arr(d, "metrics").iter().map(metric).collect(), |m| {
        w.set_st_metrics(m)
    });

    let cards = arr(d, "cards");
    // The header's three figures open the card behind them, and the page
    // reaches a card by index — so the two it needs are resolved here rather
    // than by position, which `collect()` is free to change.
    let idx_of = |key: &str| {
        cards.iter().position(|c| c["key"] == key).map(|i| i as i32).unwrap_or(-1)
    };
    w.set_st_system_idx(idx_of("system"));
    w.set_st_tools_idx(idx_of("tools"));
    put(w.get_st_cards(), cards.iter().map(card).collect(), |m| w.set_st_cards(m));

    put(
        w.get_st_timeline(),
        arr(d, "timeline")
            .iter()
            .map(|e| StEvent {
                at: text(e, "at"),
                accent: accent(e["accent"].as_str().unwrap_or("")),
                title: text(e, "title"),
                desc: text(e, "desc"),
            })
            .collect(),
        |m| w.set_st_timeline(m),
    );
    put(
        w.get_st_today(),
        arr(d, "today")
            .iter()
            .map(|t| StToday {
                label: text(t, "label"),
                value: text(t, "value"),
                accent: accent(t["accent"].as_str().unwrap_or("")),
            })
            .collect(),
        |m| w.set_st_today(m),
    );

    let lib = &d["library"];
    w.set_st_lib_items(text(lib, "items"));
    w.set_st_lib_bytes(text(lib, "bytes"));
    w.set_st_lib_databases(numstr(lib, "databases"));
    put(
        w.get_st_roots(),
        arr(lib, "roots")
            .iter()
            .map(|r| StRoot {
                path: text(r, "path"),
                state: text(r, "state"),
                // "b" is the page's bad-pill class; the only root that carries
                // one is a watched folder that is no longer on disk.
                bad: r["cls"] == "b",
                sections: model(arr(r, "sections").iter().map(s).collect()),
            })
            .collect(),
        |m| w.set_st_roots(m),
    );
    put(
        w.get_st_lib_sections(),
        arr(lib, "sections")
            .iter()
            .map(|x| StLibSection {
                name: text(x, "name"),
                accent: accent(x["accent"].as_str().unwrap_or("")),
                count: text(x, "count"),
                size: text(x, "size"),
                status: text(x, "status"),
                level: text(x, "level"),
            })
            .collect(),
        |m| w.set_st_lib_sections(m),
    );

    let scan = &d["scan"];
    w.set_st_scan_running(scan["running"].as_bool().unwrap_or(false));
    put(
        w.get_st_scan(),
        arr(scan, "rows")
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
        |m| w.set_st_scan(m),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_accent_falls_back_to_slate_rather_than_black() {
        assert_eq!(accent("nope"), accent("slate"));
        assert_ne!(accent("violet"), accent("slate"));
    }

    /// A flat week must not divide by zero, and one reading is not a line.
    #[test]
    fn a_sparkline_needs_two_points_and_survives_a_flat_week() {
        let n = |v: f64| serde_json::json!(v);
        assert_eq!(spark(&[]).0, "");
        assert_eq!(spark(&[n(5.0)]).0, "");
        let (line, area) = spark(&[n(5.0), n(5.0), n(5.0)]);
        assert!(line.starts_with("M 0.0"), "{line}");
        assert!(line.ends_with("100.0 92.0"), "flat sits on one row: {line}");
        assert!(area.ends_with("L 100 100 L 0 100 Z"), "and closes on the floor");
    }

    /// The page tells a table group from a row group by `cols` being non-empty,
    /// so a row group that emitted columns would draw a header over nothing.
    #[test]
    fn a_row_group_carries_no_columns_and_a_table_group_carries_no_rows() {
        let rowg = serde_json::json!({ "h": "Library", "rows": [{ "k": "a", "v": "1", "cls": "" }] });
        let g = group(&rowg);
        assert_eq!(g.rows.row_count(), 1);
        assert_eq!(g.cols.row_count(), 0);

        let tableg = serde_json::json!({
            "h": "Queue",
            "table": { "cols": ["File", "State"], "rows": [["a.bin", { "tag": "Running", "cls": "w" }]] },
        });
        let t = group(&tableg);
        assert_eq!(t.rows.row_count(), 0);
        assert_eq!(t.cols.row_count(), 2);
        let cells = t.table.row_data(0).unwrap().cells;
        assert!(!cells.row_data(0).unwrap().tag, "a plain cell is not a pill");
        assert!(cells.row_data(1).unwrap().tag, "and a {{tag}} cell is");
    }

    /// AI and System are pages of this dashboard, not places to go.
    #[test]
    fn only_real_sections_offer_a_jump_button() {
        let mk = |key: &str| card(&serde_json::json!({ "key": key, "detail": {} }));
        assert!(mk("music").nav);
        assert!(!mk("ai").nav);
        assert!(!mk("system").nav);
    }
}
