//! Where money becomes text.
//!
//! `tulipix-finances` deals only in `i64` minor units. Every function here turns
//! those into the pre-formatted strings the `.slint` structs carry, so the UI
//! never does arithmetic on money and a rounding bug cannot start in a layout
//! file.
//!
//! Two conventions this module is responsible for, both of which are rules of the
//! design rather than styling choices:
//!
//! - An estimate arrives with its `~` already attached, and `is-estimate` set, so
//!   a guess can never be presented as a measurement.
//! - A transfer carries `kind: "transfer"` so the ledger can grey it back. It is
//!   shown, because hiding it would make balances unexplainable, but it must not
//!   look like spending.

use chrono::{Datelike, NaiveDate};
use slint::{ModelRc, SharedString, VecModel};
use tulipix_finances::{
    accounts::AccountRow,
    budgets::{self, BudgetRow, MonthDiscipline},
    date,
    dues::{self, Due},
    fx::Rate,
    import::{csv::RawRow, detect::Proposal},
    insights::{Flag, Severity, Snapshot},
    loans::{Instalment, LoanRow},
    money,
    obligations::{Obligation, Status as ObStatus},
    recur::{self, Price, RecurRow},
    txn::{CategorySpend, MonthTotal, TxnKind, TxnRow},
};
use tulipix_ui::*;

/// Fallback palette for the spend breakdown, for categories the design did not
/// name.
///
/// Fixed order rather than a hash of the category name: the same category keeps
/// the same colour between months, which is what makes two months comparable at a
/// glance.
const SLICE_HUES: &[u32] = &[
    0xFF84cc16, 0xFFfacc15, 0xFF06b6d4, 0xFF8b5cf6, 0xFFec4899, 0xFF10b981, 0xFFf97316,
    0xFF6366f1, 0xFF14b8a6, 0xFFf43f5e,
];

/// Colours for the categories the section seeds itself.
///
/// Ahead of the ordinal palette, so the default set of categories always gets the
/// hue the design assigned it — loans pink, food lime, utilities yellow — however
/// the months happen to order them. Matched on a lowercase prefix rather than
/// equality so "Food & drink" and "Food" land on the same colour, and anything
/// unrecognised (a category the user made) falls through to the ordinal palette.
const NAMED_HUES: &[(&str, u32)] = &[
    ("loan", 0xFFf472b6),
    ("emi", 0xFFf472b6),
    ("food", 0xFF84cc16),
    ("grocer", 0xFF84cc16),
    ("eating out", 0xFFa3e635),
    ("home", 0xFFfacc15),
    ("rent", 0xFFfacc15),
    ("utilit", 0xFFfacc15),
    ("shop", 0xFF22d3ee),
    ("health", 0xFFa78bfa),
    ("subscription", 0xFFfb923c),
    ("entertainment", 0xFFfb923c),
    ("transport", 0xFF38bdf8),
    ("fuel", 0xFF38bdf8),
    ("income", 0xFF16a34a),
    ("salary", 0xFF16a34a),
];

/// A category's colour: its own stored one, then the named palette, then the
/// ordinal fallback.
fn category_hue(i: usize, name: &str, stored: Option<&str>) -> slint::Color {
    if let Some(css) = stored
        && let Some(c) = parse_hex(css)
    {
        return c;
    }
    let lower = name.to_lowercase();
    for (key, argb) in NAMED_HUES {
        if lower.starts_with(key) {
            return slint::Color::from_argb_encoded(*argb);
        }
    }
    slint::Color::from_argb_encoded(SLICE_HUES[i % SLICE_HUES.len()])
}

/// `#rrggbb` or `#aarrggbb`. Returns `None` on anything else rather than
/// substituting a colour, so the palette fallback takes over.
fn parse_hex(s: &str) -> Option<slint::Color> {
    let h = s.trim().strip_prefix('#')?;
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            Some(slint::Color::from_argb_encoded(0xFF00_0000 | v))
        }
        8 => Some(slint::Color::from_argb_encoded(u32::from_str_radix(h, 16).ok()?)),
        _ => None,
    }
}

pub fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}

fn s(v: impl Into<String>) -> SharedString {
    v.into().into()
}

/// Empty string for `None`, so the UI can test `== ""` rather than carry an
/// Option it has no way to express.
fn opt(v: Option<String>) -> SharedString {
    v.unwrap_or_default().into()
}

// ── overview ────────────────────────────────────────────────────────────────

pub fn stats(snap: &Snapshot, base: &str) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    // Month-on-month change in spending, as a signed figure and a direction. Up is
    // the bad direction here — this labels spending, not growth.
    let (delta, delta_up) = match snap.spent_last_month_minor {
        Some(prev) => {
            let d = snap.spent_this_month_minor - prev;
            if d == 0 {
                (String::new(), false)
            } else {
                (format!("{} vs last month", f(d.abs())), d > 0)
            }
        }
        None => (String::new(), false),
    };
    vec![
        FinStat {
            label: s("LIQUID"),
            value: s(f(snap.liquid_minor)),
            sub: s("Bank, cash and wallets"),
            tone: s(if snap.liquid_minor < 0 { "bad" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("SPENT THIS MONTH"),
            value: s(f(snap.spent_this_month_minor)),
            sub: s("Transfers and lending excluded"),
            tone: s("flat"),
            delta: s(delta),
            delta_up,
        },
        FinStat {
            label: s("SAVED THIS MONTH"),
            value: s(f(snap.saved_this_month_minor())),
            // The rate as well as the figure: ₹23,637 saved means something
            // different on ₹40,000 of income than on ₹1,38,000.
            sub: s(match saved_pct(snap) {
                Some(p) => format!("{p} of {} in", f(snap.income_this_month_minor)),
                None => format!("{} in", f(snap.income_this_month_minor)),
            }),
            tone: s(if snap.saved_this_month_minor() >= 0 { "ok" } else { "bad" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("DEBT"),
            value: s(f(snap.debt_minor)),
            sub: s("Cards and loans"),
            tone: s(if snap.debt_minor > 0 { "warn" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
    ]
}

/// Share of this month's income that survived, to one decimal place.
///
/// Integer arithmetic in tenths of a percent — the whole crate's rule is that
/// money never touches a float, and a percentage *of* money is close enough to
/// money to keep the rule. `None` when there is no income to be a share of.
fn saved_pct(snap: &Snapshot) -> Option<String> {
    if snap.income_this_month_minor <= 0 {
        return None;
    }
    let tenths =
        snap.saved_this_month_minor() as i128 * 1000 / snap.income_this_month_minor as i128;
    Some(format!("{}.{}%", tenths / 10, (tenths % 10).abs()))
}

/// Category slices, each carrying its own ring segment.
///
/// The cumulative angle is tracked here rather than in the layout because Slint
/// has no trigonometry, and because a segment whose position depended on the
/// layout's width — which depends on its children — is the layout cycle this
/// file has hit before.
pub fn slices(rows: &[CategorySpend], base: &str) -> Vec<FinCatSlice> {
    let total: i64 = rows.iter().map(|r| r.base_minor).sum();
    if total <= 0 {
        return Vec::new();
    }
    let mut acc = 0i32;
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let pct = ((r.base_minor as i128 * 100) / total as i128) as i32;
            let out = FinCatSlice {
                name: s(&r.name),
                amount: s(money::format_minor(r.base_minor, base)),
                pct,
                path: s(arc_path(acc, pct)),
                hue: category_hue(i, &r.name, r.color.as_deref()),
            };
            acc += pct;
            out
        })
        .collect()
}

/// Geometry of the donut, in the 100×100 viewbox `SpendBreakdown` draws into.
///
/// The ring is a *stroked arc*, not a filled wedge: one `A` command with a thick
/// stroke gives a donut segment, where a wedge would need an inner arc, two
/// radial lines and a fill rule to get the hole. Half the commands, same picture.
const R: f64 = 38.0;
const MID: f64 = 50.0;

/// One ring segment as SVG path commands, starting at `start_pct` around the
/// circle and covering `sweep_pct`.
///
/// This is the only place in the section that touches floating point, and it is
/// geometry rather than money — nothing computed here is ever stored, summed or
/// shown as a figure.
fn arc_path(start_pct: i32, sweep_pct: i32) -> String {
    if sweep_pct <= 0 {
        // A zero-width arc renders as nothing anyway; returning empty commands
        // says so explicitly instead of emitting a degenerate path.
        return String::new();
    }
    // A single category holding everything would give start == end, which SVG
    // draws as nothing at all rather than as a full circle. Two half arcs.
    if sweep_pct >= 100 {
        let (tx, ty) = point(0);
        let (bx, by) = point(50);
        return format!(
            "M {tx:.2} {ty:.2} A {R} {R} 0 0 1 {bx:.2} {by:.2} A {R} {R} 0 0 1 {tx:.2} {ty:.2}"
        );
    }
    let (x0, y0) = point(start_pct);
    let (x1, y1) = point(start_pct + sweep_pct);
    // The large-arc flag has to be set past the halfway mark or SVG takes the
    // short way round and the segment comes out inverted.
    let large = if sweep_pct > 50 { 1 } else { 0 };
    format!("M {x0:.2} {y0:.2} A {R} {R} 0 {large} 1 {x1:.2} {y1:.2}")
}

/// Point on the ring at `pct` of the way round, starting from twelve o'clock and
/// going clockwise — which in a y-down coordinate system is the positive
/// direction, hence sweep-flag 1 above.
fn point(pct: i32) -> (f64, f64) {
    let theta = (pct as f64 / 100.0) * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
    (MID + R * theta.cos(), MID + R * theta.sin())
}

/// Twelve-month bars, scaled to the tallest month so the shape of the year reads.
pub fn months(rows: &[MonthTotal], base: &str) -> Vec<FinMonthBar> {
    let peak = rows
        .iter()
        .flat_map(|m| [m.income_minor, m.expense_minor])
        .max()
        .unwrap_or(0)
        .max(1);
    rows.iter()
        .map(|m| FinMonthBar {
            // "2026-07" → "Jul". The year is implicit in a twelve-month window
            // and would not fit under a 9px bar anyway.
            label: s(month_short(&m.period)),
            income: s(money::format_minor(m.income_minor, base)),
            expense: s(money::format_minor(m.expense_minor, base)),
            income_pct: ((m.income_minor as i128 * 100) / peak as i128) as i32,
            expense_pct: ((m.expense_minor as i128 * 100) / peak as i128) as i32,
        })
        .collect()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn month_short(period: &str) -> String {
    period
        .split_once('-')
        .and_then(|(_, m)| m.parse::<usize>().ok())
        .and_then(|m| MONTHS.get(m.wrapping_sub(1)).copied())
        .unwrap_or(period)
        .to_string()
}

/// "July 2026" from a `YYYY-MM` period.
pub fn month_long(period: &str) -> String {
    match period.split_once('-') {
        Some((y, m)) => match m.parse::<usize>().ok().and_then(|m| MONTHS.get(m.wrapping_sub(1))) {
            Some(name) => format!("{name} {y}"),
            None => period.to_string(),
        },
        None => period.to_string(),
    }
}

// ── ledger ──────────────────────────────────────────────────────────────────

/// Human name for `transactions.source`, so the provenance column means something
/// to a reader rather than echoing a database enum.
fn source_label(raw: &str) -> &'static str {
    match raw {
        "csv" => "CSV",
        "ofx" => "OFX",
        "ocr" => "Receipt",
        "recurrence" => "Recurrence",
        _ => "Manual",
    }
}

pub fn txns(rows: &[TxnRow], base: &str, show_running: bool) -> Vec<FinTxnRow> {
    rows.iter()
        .map(|t| FinTxnRow {
            id: t.id as i32,
            date: s(&t.occurred_on),
            // A transfer names both ends, because "Top-up" on its own does not
            // explain why one balance fell and another rose.
            description: s(match (&t.kind, &t.to_account_name) {
                (TxnKind::Transfer, Some(to)) => format!("{} → {}", t.description, to),
                _ => t.description.clone(),
            }),
            category: opt(t.category_name.clone()),
            // The same colour the donut gives this category, so a row in the
            // ledger and a slice in the chart are recognisably the same thing.
            cat_hue: category_hue(0, t.category_name.as_deref().unwrap_or(""), None),
            account: s(&t.account_name),
            source: s(source_label(&t.source)),
            amount: s(format!(
                "{}{}",
                match t.kind {
                    TxnKind::Income => "+",
                    TxnKind::Expense => "−",
                    TxnKind::Transfer => "",
                },
                // The original currency, not the base figure: the row should
                // match what the statement said.
                money::format_minor(t.amount_minor, &t.currency)
            )),
            running: s(if show_running {
                t.running_minor.map(|m| money::format_minor(m, base)).unwrap_or_default()
            } else {
                String::new()
            }),
            kind: s(t.kind.as_str()),
        })
        .collect()
}

// ── recurrences and obligations ─────────────────────────────────────────────

pub fn obligations(rows: &[Obligation]) -> Vec<FinObligationRow> {
    rows.iter()
        .map(|o| FinObligationRow {
            id: o.id as i32,
            name: s(&o.name),
            kind: s(o.kind.clone().unwrap_or_else(|| "one-off".into())),
            due: s(&o.due_on),
            days: o.days_until as i32,
            // The estimate keeps its tilde. A figure that is a guess must never
            // be readable as a measurement.
            estimate: s(o
                .estimate_minor
                .map(|m| money::format_estimate(m, &o.currency))
                .unwrap_or_default()),
            actual: s(o.actual_minor.map(|m| money::format_minor(m, &o.currency)).unwrap_or_default()),
            variance: s(o
                .variance_minor()
                .filter(|v| *v != 0)
                .map(|v| money::format_minor(v.abs(), &o.currency))
                .unwrap_or_default()),
            over: o.variance_minor().unwrap_or(0) > 0,
            status: s(o.status.as_str()),
            is_estimate: o.is_estimate(),
            category: opt(o.category_name.clone()),
            cat_hue: category_hue(0, o.category_name.as_deref().unwrap_or(""), None),
            account: opt(o.account_name.clone()),
            auto_post: o.auto_post,
        })
        .collect()
}

/// How many bills fall in each filter group.
///
/// Of the whole window, never of the filtered list: a chip showing the count of what
/// it would leave after filtering reads as zero for every group you are not in.
#[derive(Clone, Copy, Debug, Default)]
pub struct BillCounts {
    pub all: i32,
    pub needs: i32,
    pub auto: i32,
    pub paid: i32,
    pub oneoff: i32,
}

pub fn bill_counts(rows: &[Obligation]) -> BillCounts {
    let mut c = BillCounts { all: rows.len() as i32, ..Default::default() };
    for o in rows {
        if matches!(o.status, ObStatus::Paid) {
            c.paid += 1;
        } else if o.auto_post {
            c.auto += 1;
        } else if !matches!(o.status, ObStatus::Skipped) {
            // What is left is what someone has to do something about.
            c.needs += 1;
        }
        if o.recurrence_id.is_none() {
            c.oneoff += 1;
        }
    }
    c
}

/// The subset of bills a filter chip selects.
///
/// `oneoff` overlaps the others on purpose — a one-off bill is also either paid or
/// needing action — because "which of these did I enter by hand" is a different
/// question from "what do I have to do", and answering it should not require the
/// others to be mutually exclusive with it.
pub fn filter_bills<'a>(rows: &'a [Obligation], filter: &str) -> Vec<Obligation> {
    rows.iter()
        .filter(|o| match filter {
            "needs" => {
                !o.auto_post && !matches!(o.status, ObStatus::Paid | ObStatus::Skipped)
            }
            "auto" => o.auto_post && !matches!(o.status, ObStatus::Paid),
            "paid" => matches!(o.status, ObStatus::Paid),
            "oneoff" => o.recurrence_id.is_none(),
            // Unknown filter included: an empty list would look like a broken tab,
            // and the chip row can only ever send one of the five.
            _ => true,
        })
        .cloned()
        .collect::<Vec<_>>()
}

/// Six months of spending per category, biggest first.
///
/// One call per month rather than one grouped query: `spend_by_category` is the
/// function the donut already uses, and six calls to a tested aggregate beat one new
/// query that could disagree with it.
///
/// Bars are shares of each category's *own* worst month. Shares of the whole ledger
/// would draw every category except the largest as a flat line, which is exactly the
/// trend the card exists to show.
pub fn trend(months: &[(String, Vec<CategorySpend>)], base: &str, limit: usize) -> Vec<FinCatTrend> {
    // Total per category across the window, to rank and to keep only the top few.
    let mut totals: Vec<(String, i64)> = Vec::new();
    for (_, rows) in months {
        for r in rows {
            match totals.iter_mut().find(|(n, _)| *n == r.name) {
                Some((_, t)) => *t += r.base_minor,
                None => totals.push((r.name.clone(), r.base_minor)),
            }
        }
    }
    totals.sort_by(|a, b| b.1.cmp(&a.1));
    totals.truncate(limit);

    totals
        .into_iter()
        .enumerate()
        .map(|(i, (name, total))| {
            let series: Vec<i64> = months
                .iter()
                .map(|(_, rows)| {
                    rows.iter().find(|r| r.name == name).map(|r| r.base_minor).unwrap_or(0)
                })
                .collect();
            let peak = series.iter().copied().max().unwrap_or(0).max(1);
            let last = series.last().copied().unwrap_or(0);
            // Against the mean of the earlier months, not against last month alone:
            // one quiet month would otherwise report every category as rising.
            let earlier = &series[..series.len().saturating_sub(1)];
            let mean: i64 = if earlier.is_empty() {
                0
            } else {
                earlier.iter().sum::<i64>() / earlier.len() as i64
            };
            let diff = last - mean;
            // A tenth of the mean is noise; below that the card says nothing.
            let worth_saying = mean > 0 && diff.abs() * 10 > mean;

            FinCatTrend {
                name: s(&name),
                hue: category_hue(i, &name, None),
                total: s(money::format_minor(total, base)),
                delta: s(if worth_saying {
                    format!("{} vs usual", money::format_minor(diff.abs(), base))
                } else {
                    String::new()
                }),
                up: diff > 0,
                months: model(
                    months
                        .iter()
                        .zip(series.iter())
                        .enumerate()
                        .map(|(mi, ((label, _), v))| FinTrendMonth {
                            label: s(month_short(label)),
                            pct: ((*v as i128 * 100) / peak as i128) as i32,
                            current: mi + 1 == months.len(),
                        })
                        .collect(),
                ),
            }
        })
        .collect()
}

/// The four figures above the Insights list.
pub fn insight_stats(
    snap: &Snapshot,
    flags: &[Flag],
    subs_yearly_minor: i64,
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let bad = flags.iter().filter(|x| x.severity == Severity::Bad).count();
    let warn = flags.iter().filter(|x| x.severity == Severity::Warn).count();

    // What this month would come to if it carried on: not a forecast of the future,
    // an arithmetic statement about the present rate, which is why it is labelled
    // "if nothing changes" rather than "projected".
    let saved = snap.saved_this_month_minor();

    vec![
        FinStat {
            label: s("WORTH ACTING ON"),
            value: s(format!("{}", bad + warn)),
            sub: s(if bad > 0 {
                format!("{bad} urgent, {warn} to watch")
            } else if warn > 0 {
                format!("{warn} to watch, nothing urgent")
            } else {
                "nothing needs you".to_string()
            }),
            tone: s(if bad > 0 { "bad" } else if warn > 0 { "warn" } else { "ok" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("SAVINGS RATE"),
            value: s(match saved_pct(snap) {
                Some(p) => p,
                None => "—".to_string(),
            }),
            sub: s(if snap.income_this_month_minor > 0 {
                format!("{} of {}", f(saved), f(snap.income_this_month_minor))
            } else {
                "no income recorded this month".to_string()
            }),
            tone: s(if snap.income_this_month_minor == 0 {
                "flat"
            } else if saved < 0 {
                "bad"
            } else {
                "ok"
            }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("IF NOTHING CHANGES"),
            value: s(f(saved * 12)),
            sub: s("a year at this month's rate"),
            tone: s(if saved < 0 { "bad" } else { "ok" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("SUBSCRIPTIONS"),
            value: s(f(subs_yearly_minor)),
            sub: s(format!("{} a month, committed", f(subs_yearly_minor / 12))),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
    ]
}

/// The Calendar tab's two cards: whether the month is covered, and its worst day.
///
/// Returns `(warning, heaviest_amount, heaviest_label, heaviest_detail)`. The warning
/// is empty when nothing is short — the card then says so itself, rather than this
/// inventing a reassuring sentence.
pub fn calendar_cards(
    low: Option<&tulipix_finances::insights::LowPoint>,
    items: &[Obligation],
    liquid_minor: i64,
    base: &str,
) -> (String, String, String, String) {
    let f = |m: i64| money::format_minor(m, base);

    let warning = match low {
        Some(l) if l.balance_minor < 0 => format!(
            "{} leaves before {} and the balance projects to {}. Something has to move.",
            f(items.iter().filter_map(|o| o.shown_minor()).sum::<i64>()),
            l.on,
            f(l.balance_minor)
        ),
        Some(l) => {
            let out: i64 = items.iter().filter_map(|o| o.shown_minor()).sum();
            // Tight rather than short: worth saying, but it is not a warning.
            if l.balance_minor * 4 < liquid_minor && out > 0 {
                format!(
                    "{} leaves this month, and the balance dips to {} around {}.",
                    f(out),
                    f(l.balance_minor),
                    l.on
                )
            } else {
                String::new()
            }
        }
        None => String::new(),
    };

    // The single day with the most leaving it. A month can be affordable in total and
    // still fail on the 5th.
    let mut by_day: Vec<(&str, i64, usize)> = Vec::new();
    for o in items {
        let Some(m) = o.shown_minor() else { continue };
        match by_day.iter_mut().find(|(d, _, _)| *d == o.due_on.as_str()) {
            Some((_, total, n)) => {
                *total += m;
                *n += 1;
            }
            None => by_day.push((&o.due_on, m, 1)),
        }
    }
    let heaviest = by_day.into_iter().max_by_key(|(_, t, _)| *t);
    match heaviest {
        Some((day, total, n)) => (
            warning,
            f(total),
            day.to_string(),
            format!("{n} thing{} due that day", if n == 1 { "" } else { "s" }),
        ),
        None => (warning, String::new(), String::new(), "Nothing dated this month.".to_string()),
    }
}

/// Where the month's income is committed before anything discretionary.
///
/// Loans, rent-shaped bills and subscriptions, then what is left over. Percentages
/// are of income, so they only mean anything when there is income — with none, this
/// returns nothing rather than dividing by zero or showing shares of a total it
/// invented.
pub fn commitments(
    income_minor: i64,
    loan_emi_minor: i64,
    bills_minor: i64,
    subs_monthly_minor: i64,
    base: &str,
) -> Vec<FinCatSlice> {
    if income_minor <= 0 {
        return Vec::new();
    }
    let committed = loan_emi_minor + bills_minor + subs_monthly_minor;
    let left = (income_minor - committed).max(0);
    [
        ("Loans & EMI", loan_emi_minor, 0xFFf472b6u32),
        ("Bills", bills_minor, 0xFFfacc15),
        ("Subscriptions", subs_monthly_minor, 0xFFfb923c),
        ("Left to decide", left, 0xFF84cc16),
    ]
    .into_iter()
    .filter(|(_, m, _)| *m > 0)
    .map(|(name, m, argb)| FinCatSlice {
        name: s(name),
        amount: s(money::format_minor(m, base)),
        pct: ((m as i128 * 100) / income_minor as i128) as i32,
        // No ring here: this is a bar list, and an arc path for it would be geometry
        // nothing draws.
        path: s(""),
        hue: slint::Color::from_argb_encoded(argb),
    })
    .collect()
}

/// How many dues are in each group.
#[derive(Clone, Copy, Debug, Default)]
pub struct DueCounts {
    pub all: i32,
    pub to_me: i32,
    pub i_owe: i32,
    pub closed: i32,
}

pub fn due_counts(rows: &[Due]) -> DueCounts {
    let mut c = DueCounts { all: rows.len() as i32, ..Default::default() };
    for d in rows {
        if !matches!(d.status, dues::Status::Open) {
            c.closed += 1;
        } else if d.direction == dues::Direction::OwedToMe {
            c.to_me += 1;
        } else {
            c.i_owe += 1;
        }
    }
    c
}

pub fn filter_dues(rows: &[Due], filter: &str) -> Vec<Due> {
    rows.iter()
        .filter(|d| {
            let open = matches!(d.status, dues::Status::Open);
            match filter {
                "to-me" => open && d.direction == dues::Direction::OwedToMe,
                "i-owe" => open && d.direction == dues::Direction::IOwe,
                "closed" => !open,
                _ => true,
            }
        })
        .cloned()
        .collect()
}

/// The four figures above the Dues table.
///
/// The net position is shown *and* labelled as a net, because the two directions are
/// never subtracted anywhere else in this section — but "am I up or down on money
/// between friends" is the question the tab exists to answer, and refusing to
/// answer it while showing both halves would be pedantry rather than honesty.
pub fn due_stats(
    totals: &dues::Totals,
    settled: &dues::Totals,
    rows: &[Due],
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let net = totals.owed_to_me_minor - totals.i_owe_minor;
    let open: Vec<&Due> = rows.iter().filter(|d| matches!(d.status, dues::Status::Open)).collect();
    let oldest = open.iter().map(|d| d.age_days).max().unwrap_or(0);
    let stale = open.iter().filter(|d| d.age_days >= 180).count();

    vec![
        FinStat {
            label: s("OWED TO YOU"),
            value: s(f(totals.owed_to_me_minor)),
            sub: s(format!(
                "{} open",
                open.iter().filter(|d| d.direction == dues::Direction::OwedToMe).count()
            )),
            tone: s(if totals.owed_to_me_minor > 0 { "ok" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("YOU OWE"),
            value: s(f(totals.i_owe_minor)),
            sub: s(format!(
                "{} open",
                open.iter().filter(|d| d.direction == dues::Direction::IOwe).count()
            )),
            tone: s(if totals.i_owe_minor > 0 { "warn" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("NET POSITION"),
            value: s(format!("{}{}", if net < 0 { "−" } else { "" }, f(net.abs()))),
            sub: s(if net == 0 {
                "square with everyone".to_string()
            } else if net > 0 {
                "in your favour".to_string()
            } else {
                "against you".to_string()
            }),
            tone: s(if net < 0 { "bad" } else if net > 0 { "ok" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("CAME BACK, 6 MONTHS"),
            value: s(f(settled.owed_to_me_minor)),
            sub: s(if stale > 0 {
                format!("{stale} open {} months+", oldest / 30)
            } else if oldest > 0 {
                format!("oldest is {oldest} days old")
            } else {
                "nothing outstanding".to_string()
            }),
            tone: s(if stale > 0 { "warn" } else { "flat" }),
            delta: s(""),
            delta_up: false,
        },
    ]
}

/// The four figures above the Bills table.
///
/// Derived from the rows the tab is already showing rather than from four more
/// queries: they have to agree with the list underneath them, and the surest way to
/// make two numbers agree is for them to be the same number.
///
/// `income_minor` is this month's income, for the "fixed floor" share. Zero when
/// there is none, in which case the share is left off rather than shown as 0%.
pub fn bill_stats(
    rows: &[Obligation],
    subs_yearly_minor: i64,
    income_minor: i64,
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let open = |o: &&Obligation| !matches!(o.status, ObStatus::Paid | ObStatus::Skipped);

    let overdue: Vec<&Obligation> =
        rows.iter().filter(|o| matches!(o.status, ObStatus::Overdue)).collect();
    let overdue_total: i64 = overdue.iter().filter_map(|o| o.shown_minor()).sum();
    let latest = overdue.iter().map(|o| -o.days_until).max().unwrap_or(0);

    let due: Vec<&Obligation> = rows.iter().filter(open).collect();
    let due_total: i64 = due.iter().filter_map(|o| o.shown_minor()).sum();
    // An estimate needs confirming; a fixed amount does not.
    let needs_confirming = due.iter().filter(|o| o.is_estimate()).count();

    let auto: Vec<&Obligation> = due.iter().copied().filter(|o| o.auto_post).collect();
    let auto_total: i64 = auto.iter().filter_map(|o| o.shown_minor()).sum();

    // What leaves every month whatever the user does: the bills on the calendar plus
    // a twelfth of the subscriptions. The one figure on this tab that is about the
    // shape of someone's finances rather than about this month's list.
    let floor = due_total + subs_yearly_minor / 12;
    let share = if income_minor > 0 {
        format!(" · {}% of income", (floor as i128 * 100 / income_minor as i128).min(999))
    } else {
        String::new()
    };

    vec![
        FinStat {
            label: s("OVERDUE"),
            value: s(f(overdue_total)),
            sub: s(if overdue.is_empty() {
                "Nothing late".to_string()
            } else {
                format!(
                    "{} bill{} · {latest} day{} late",
                    overdue.len(),
                    if overdue.len() == 1 { "" } else { "s" },
                    if latest == 1 { "" } else { "s" }
                )
            }),
            tone: s(if overdue.is_empty() { "flat" } else { "bad" }),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("DUE THIS MONTH"),
            value: s(f(due_total)),
            sub: s(format!(
                "{} bill{} · {needs_confirming} need{} confirming",
                due.len(),
                if due.len() == 1 { "" } else { "s" },
                if needs_confirming == 1 { "s" } else { "" }
            )),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("AUTO-DEBITED"),
            value: s(f(auto_total)),
            sub: s(format!(
                "{} bill{} · no action needed",
                auto.len(),
                if auto.len() == 1 { "" } else { "s" }
            )),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("FIXED MONTHLY FLOOR"),
            value: s(f(floor)),
            sub: s(format!("bills + subscriptions{share}")),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
    ]
}

pub fn recurrences(rows: &[RecurRow], base: &str) -> Vec<FinRecurRow> {
    let today = date::today();
    rows.iter()
        .map(|r| FinRecurRow {
            id: r.id as i32,
            name: s(&r.name),
            kind: s(r.kind.as_str()),
            amount: s(match (r.amount_minor, r.estimate_minor) {
                (Some(a), _) => money::format_minor(a, &r.currency),
                (None, Some(e)) => money::format_estimate(e, &r.currency),
                // No fixed amount and no history to average: say so rather than
                // printing a zero that reads as free.
                (None, None) => "amount varies".to_string(),
            }),
            is_estimate: r.is_estimate(),
            cycle: s(cycle_label(r.cycle)),
            next_due: opt(r.next_due_on.clone()),
            days: r
                .next_due_on
                .as_deref()
                .and_then(|d| date::parse(d).ok())
                .map(|d| date::days_between(today, d) as i32)
                .unwrap_or(0),
            status: s(r.status.as_str()),
            account: opt(r.account_name.clone()),
            category: opt(r.category_name.clone()),
            yearly: s(money::format_minor(r.yearly_minor, base)),
            // A twelfth of the year, in the base currency: the point of the column
            // is comparing a yearly plan against a monthly one, and two currencies
            // in one column would not compare.
            monthly: s(money::format_minor(r.yearly_minor / 12, base)),
            hike_from: s(r
                .hike_from_minor
                .map(|m| money::format_minor(m, &r.currency))
                .unwrap_or_default()),
            auto_post: r.auto_post,
            last_paid: opt(r.last_paid_on.clone()),
            cat_hue: category_hue(0, r.category_name.as_deref().unwrap_or(""), None),
        })
        .collect()
}

/// How many subscriptions are in each status.
#[derive(Clone, Copy, Debug, Default)]
pub struct SubCounts {
    pub all: i32,
    pub active: i32,
    pub paused: i32,
    pub cancelled: i32,
}

pub fn sub_counts(rows: &[RecurRow]) -> SubCounts {
    let mut c = SubCounts { all: rows.len() as i32, ..Default::default() };
    for r in rows {
        match r.status {
            recur::Status::Active => c.active += 1,
            recur::Status::Paused => c.paused += 1,
            recur::Status::Cancelled => c.cancelled += 1,
        }
    }
    c
}

pub fn filter_subs(rows: &[RecurRow], filter: &str) -> Vec<RecurRow> {
    rows.iter()
        .filter(|r| match filter {
            "active" => matches!(r.status, recur::Status::Active),
            "paused" => matches!(r.status, recur::Status::Paused),
            "cancelled" => matches!(r.status, recur::Status::Cancelled),
            _ => true,
        })
        .cloned()
        .collect()
}

/// The four figures above the Subscriptions table.
///
/// The last two are the argument for the tab existing: what the whole set costs a
/// year, and what the ones nobody is using cost. `unused_minor` comes from the
/// Videos-playback flag and is zero when there is no playback data at all — the
/// card then says so rather than claiming nothing is unused.
pub fn sub_stats(rows: &[RecurRow], base: &str, unused: &[String], unused_minor: i64) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let active: Vec<&RecurRow> =
        rows.iter().filter(|r| matches!(r.status, recur::Status::Active)).collect();
    let yearly: i64 = active.iter().map(|r| r.yearly_minor).sum();
    let paused: i64 = rows
        .iter()
        .filter(|r| matches!(r.status, recur::Status::Paused))
        .map(|r| r.yearly_minor)
        .sum();
    // The next thing to be charged, so the tab opens on a date rather than only on
    // totals.
    let today = date::today();
    let next = active
        .iter()
        .filter_map(|r| r.next_due_on.as_deref())
        .filter_map(|d| date::parse(d).ok())
        .filter(|d| *d >= today)
        .min();

    vec![
        FinStat {
            label: s("A YEAR, ACTIVE"),
            value: s(f(yearly)),
            sub: s(format!("{} subscription{}", active.len(), if active.len() == 1 { "" } else { "s" })),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("A MONTH, ACTIVE"),
            value: s(f(yearly / 12)),
            sub: s("the recurring floor"),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("NEXT CHARGE"),
            value: s(match next {
                Some(d) => date::iso(d),
                None => "—".to_string(),
            }),
            sub: s(match next {
                Some(d) => {
                    let n = date::days_between(today, d);
                    if n == 0 { "today".to_string() } else { format!("in {n} day{}", if n == 1 { "" } else { "s" }) }
                }
                None => "nothing scheduled".to_string(),
            }),
            tone: s("flat"),
            delta: s(""),
            delta_up: false,
        },
        FinStat {
            label: s("PAID, NOT USED"),
            value: s(if unused.is_empty() { "—".to_string() } else { f(unused_minor) }),
            sub: s(if unused.is_empty() {
                if paused > 0 {
                    format!("{} a year is paused", f(paused))
                } else {
                    "nothing flagged".to_string()
                }
            } else {
                unused.join(", ")
            }),
            tone: s(if unused.is_empty() { "flat" } else { "warn" }),
            delta: s(""),
            delta_up: false,
        },
    ]
}

fn cycle_label(c: date::Cycle) -> &'static str {
    match c {
        date::Cycle::Weekly => "Weekly",
        date::Cycle::Monthly => "Monthly",
        date::Cycle::Quarterly => "Quarterly",
        date::Cycle::Yearly => "Yearly",
        date::Cycle::Irregular => "Irregular",
    }
}

pub fn prices(rows: &[Price]) -> Vec<FinPrice> {
    rows.iter()
        .map(|p| FinPrice {
            amount: s(money::format_minor(p.amount_minor, &p.currency)),
            from: s(&p.effective_from),
        })
        .collect()
}

// ── dues ────────────────────────────────────────────────────────────────────

pub fn dues(rows: &[Due]) -> Vec<FinDueRow> {
    rows.iter()
        .map(|d| FinDueRow {
            id: d.id as i32,
            person: s(&d.person),
            direction: s(d.direction.as_str()),
            amount: s(money::format_minor(d.amount_minor, &d.currency)),
            original: s(money::format_minor(d.original_minor(), &d.currency)),
            settled: s(money::format_minor(d.settled_minor, &d.currency)),
            opened: s(&d.opened_on),
            age_days: d.age_days as i32,
            status: s(d.status.as_str()),
            note: opt(d.note.clone()),
            part_paid: d.settled_minor > 0 && d.status == dues::Status::Open,
        })
        .collect()
}

// ── accounts and loans ──────────────────────────────────────────────────────

pub fn accounts(rows: &[AccountRow], base: &str) -> Vec<FinAccountRow> {
    rows.iter()
        .map(|a| FinAccountRow {
            id: a.id as i32,
            name: s(&a.name),
            kind: s(a.kind.as_str()),
            // Foreign-currency accounts still report in base: balances are summed
            // from `base_minor`, so labelling one with its own symbol would claim
            // a precision the derivation does not have.
            balance: s(money::format_minor(a.balance_minor, base)),
            sub: s(match a.utilisation_pct() {
                Some(pct) => format!("{} · {pct}% of limit", kind_label(a.kind.as_str())),
                None if a.currency != base => format!("{} · {}", kind_label(a.kind.as_str()), a.currency),
                None => kind_label(a.kind.as_str()).to_string(),
            }),
            util_pct: a.utilisation_pct().unwrap_or(0) as i32,
            negative: a.balance_minor < 0,
            closed: a.closed,
            txn_count: a.txn_count as i32,
        })
        .collect()
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "cash" => "Cash",
        "card" => "Credit card",
        "wallet" => "Wallet",
        "loan" => "Loan",
        "virtual" => "Holding account",
        _ => "Bank",
    }
}

pub fn loans(rows: &[LoanRow]) -> Vec<FinLoanRow> {
    rows.iter()
        .map(|l| FinLoanRow {
            account_id: l.account_id as i32,
            name: s(&l.name),
            balance: s(money::format_minor(l.balance_minor, &l.currency)),
            principal: s(money::format_minor(l.principal_minor, &l.currency)),
            emi: s(money::format_minor(l.emi_minor, &l.currency)),
            // Basis points back to a percentage: 840 → "8.4%".
            rate: s(format!("{}.{}%", l.rate_bp / 100, (l.rate_bp % 100) / 10)),
            progress_pct: l.progress_pct() as i32,
            remaining_months: l.remaining_months() as i32,
            tenure_months: l.tenure_months as i32,
            next_due: s(&l.started_on),
        })
        .collect()
}

pub fn schedule(rows: &[Instalment], currency: &str) -> Vec<FinInstalment> {
    rows.iter()
        .map(|i| FinInstalment {
            n: i.n as i32,
            due: s(&i.due_on),
            emi: s(money::format_minor(i.emi_minor, currency)),
            principal: s(money::format_minor(i.principal_minor, currency)),
            interest: s(money::format_minor(i.interest_minor, currency)),
            balance: s(money::format_minor(i.balance_minor, currency)),
            interest_pct: if i.emi_minor > 0 {
                ((i.interest_minor as i128 * 100) / i.emi_minor as i128) as i32
            } else {
                0
            },
        })
        .collect()
}

// ── budgets ─────────────────────────────────────────────────────────────────

pub fn budgets(rows: &[BudgetRow], base: &str, elapsed: i64, total: i64) -> Vec<FinBudgetRow> {
    rows.iter()
        .map(|b| FinBudgetRow {
            category_id: b.category_id as i32,
            name: s(&b.category_name),
            budget: s(money::format_minor(b.amount_minor, base)),
            spent: s(money::format_minor(b.spent_minor, base)),
            remaining: s(money::format_minor(b.remaining_minor().abs(), base)),
            used_pct: b.used_pct() as i32,
            projected: s(money::format_minor(b.projected_minor(elapsed, total), base)),
            projected_pct: if b.allowance_minor() > 0 {
                ((b.projected_minor(elapsed, total) as i128 * 100) / b.allowance_minor() as i128) as i32
            } else {
                0
            },
            over: b.over_budget(),
            off_pace: b.off_pace(elapsed, total),
            rollover: b.rollover,
            carried: s(if b.rollover_minor > 0 {
                money::format_minor(b.rollover_minor, base)
            } else {
                String::new()
            }),
            unset: b.id.is_none(),
        })
        .collect()
}

pub fn discipline(rows: &[MonthDiscipline], base: &str) -> Vec<FinDiscipline> {
    rows.iter()
        .map(|m| FinDiscipline {
            period: s(month_long(&m.period)),
            budget: s(money::format_minor(m.budget_minor, base)),
            spent: s(money::format_minor(m.spent_minor, base)),
            within: m.within(),
            pct: if m.budget_minor > 0 {
                ((m.spent_minor as i128 * 100) / m.budget_minor as i128) as i32
            } else {
                0
            },
        })
        .collect()
}

// ── calendar ────────────────────────────────────────────────────────────────

/// A six-by-seven month grid, Monday first, with each day's dated obligations
/// folded in.
///
/// Always 42 cells: a month starting on a Sunday would otherwise reflow the page
/// to five rows and back again as the user steps through the year.
pub fn calendar(
    period: &str,
    today: NaiveDate,
    items: &[Obligation],
    income: &[(String, i64)],
    base: &str,
) -> Vec<FinCalDay> {
    let Ok((first_iso, _)) = date::month_bounds(period) else { return Vec::new() };
    let Ok(first) = date::parse(&first_iso) else { return Vec::new() };
    let days_in = date::days_in_month(first.year(), first.month()) as i64;
    // Monday = 0.
    let lead = first.weekday().num_days_from_monday() as i64;

    (0..42)
        .map(|cell| {
            let day = cell as i64 - lead + 1;
            let in_month = day >= 1 && day <= days_in;
            let iso = if in_month {
                date::iso(first.with_day(day as u32).unwrap_or(first))
            } else {
                String::new()
            };
            let on_day: Vec<&Obligation> = items.iter().filter(|o| o.due_on == iso).collect();
            let sum: i64 = on_day.iter().filter_map(|o| o.shown_minor()).sum();
            // Income is a separate figure rather than netted against what is
            // owed. A day with ₹80,000 in and ₹78,000 out is not a quiet day,
            // and a single net number would say it was.
            let earned: i64 = income.iter().filter(|(d, _)| *d == iso).map(|(_, v)| *v).sum();
            let credits = income.iter().filter(|(d, _)| *d == iso).count();
            FinCalDay {
                day: if in_month { day as i32 } else { 0 },
                in_month,
                today: in_month && iso == date::iso(today),
                count: (on_day.len() + credits) as i32,
                amount: s(if sum > 0 { money::format_minor(sum, base) } else { String::new() }),
                income: s(if earned > 0 { money::format_minor(earned, base) } else { String::new() }),
                money_out: sum > 0,
                overdue: on_day.iter().any(|o| o.status.as_str() == "overdue"),
            }
        })
        .collect()
}

// ── insights ────────────────────────────────────────────────────────────────

pub fn flags(rows: &[Flag]) -> Vec<FinFlag> {
    use tulipix_finances::insights::Action;
    rows.iter()
        .map(|f| {
            // The action is encoded as `kind:id`, which the callback splits. A
            // string rather than a pair of properties because Slint structs carry
            // no enums and two fields could disagree.
            let (label, action) = match &f.action {
                Action::None => ("", String::new()),
                Action::Obligation(i) => ("Open bill", format!("obligation:{i}")),
                Action::Recurrence(i) => ("Open", format!("recurrence:{i}")),
                Action::Category(i) => ("Open budget", format!("category:{i}")),
                Action::Account(i) => ("Open account", format!("account:{i}")),
                Action::Due(i) => ("Open due", format!("due:{i}")),
                Action::Rates => ("Set a rate", "rates:0".to_string()),
            };
            FinFlag {
                severity: s(match f.severity {
                    Severity::Bad => "bad",
                    Severity::Warn => "warn",
                    Severity::Info => "info",
                    Severity::Good => "good",
                }),
                title: s(&f.title),
                detail: s(&f.detail),
                action_label: s(label),
                action: s(action),
            }
        })
        .collect()
}

// ── rates, import, detection ────────────────────────────────────────────────

pub fn rates(rows: &[Rate], base: &str) -> Vec<FinRate> {
    rows.iter()
        .map(|r| FinRate {
            code: s(&r.code),
            // Six decimal places back to something readable: 83_600_000 → "83.60".
            rate: s(format!(
                "1 {} = {}.{:02} {}",
                r.code,
                r.rate_micro / 1_000_000,
                (r.rate_micro % 1_000_000) / 10_000,
                base
            )),
            // Where it came from and when, because a converted total is only as
            // trustworthy as the age of the rate behind it.
            edited: s(match chrono::DateTime::<chrono::Utc>::from_timestamp(r.edited_at, 0) {
                Some(dt) => format!(
                    "{} {}",
                    if r.live { "fetched" } else { "typed" },
                    dt.with_timezone(&chrono::Local).date_naive()
                ),
                None => String::new(),
            }),
            live: r.live,
        })
        .collect()
}

/// Which column of the file became which field.
///
/// Shown in the import sheet because the mapping is *guessed*. Getting debit and
/// credit the wrong way round inverts every row in a statement and leaves all the
/// totals looking plausible, so the guess has to be visible before anything posts.
pub fn import_map(map: &tulipix_finances::import::csv::ColumnMap) -> Vec<FinField> {
    let row = |label: &str, value: Option<&str>| FinField {
        key: s(label),
        label: s(label),
        kind: s("static"),
        value: s(value.unwrap_or("")),
        hint: s(""),
        options: model(Vec::new()),
        required: false,
    };
    let mut out = vec![
        row("Date", Some(&map.date)),
        row("Description", Some(&map.description)),
    ];
    // Either one signed column or a debit/credit pair — never both, and which one it
    // is tells the user something about their bank's export.
    match (&map.amount, &map.debit, &map.credit) {
        (Some(a), _, _) => out.push(row("Amount (signed)", Some(a))),
        (None, d, c) => {
            out.push(row("Money out", d.as_deref()));
            out.push(row("Money in", c.as_deref()));
        }
    }
    out
}

pub fn import_rows(rows: &[RawRow], currency: &str, limit: usize) -> Vec<FinImportRow> {
    rows.iter()
        .take(limit)
        .map(|r| FinImportRow {
            date: s(&r.occurred_on),
            description: s(&r.description),
            amount: s(format!(
                "{}{}",
                if r.is_credit { "+" } else { "−" },
                money::format_minor(r.amount_minor, currency)
            )),
            credit: r.is_credit,
        })
        .collect()
}

pub fn proposals(rows: &[Proposal]) -> Vec<FinProposal> {
    use tulipix_finances::import::detect::Confidence;
    rows.iter()
        .enumerate()
        .map(|(i, p)| FinProposal {
            // The index into the cached proposal list, not a database id — nothing
            // has been created yet.
            idx: i as i32,
            name: s(&p.label),
            kind: s(p.kind.as_str()),
            amount: s(match p.amount_minor {
                Some(a) => money::format_minor(a, &p.currency),
                None => money::format_estimate(p.typical_minor, &p.currency),
            }),
            cycle: s(cycle_label(p.cycle)),
            next_due: s(&p.next_due_on),
            occurrences: p.occurrences as i32,
            confidence: s(match p.confidence {
                Confidence::High => "high",
                Confidence::Low => "low",
            }),
        })
        .collect()
}

/// Income-allocation figures for the Budgets header.
pub fn allocation(a: &budgets::Allocation, base: &str) -> (String, String, String, i32) {
    let pct = if a.income_minor > 0 {
        ((a.budgeted_minor as i128 * 100) / a.income_minor as i128) as i32
    } else {
        0
    };
    (
        money::format_minor(a.income_minor, base),
        money::format_minor(a.budgeted_minor, base),
        money::format_minor(a.unallocated_minor(), base),
        pct,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_labels() {
        assert_eq!(month_short("2026-07"), "Jul");
        assert_eq!(month_long("2026-07"), "July 2026");
        // Garbage in stays garbage rather than becoming a wrong month.
        assert_eq!(month_short("nonsense"), "nonsense");
        assert_eq!(month_long("2026-13"), "2026-13");
    }

    #[test]
    fn stored_category_colours_win_over_the_palette() {
        assert!(parse_hex("#84cc16").is_some());
        assert!(parse_hex("#ff84cc16").is_some());
        assert!(parse_hex("lime").is_none());
        assert!(parse_hex("#abc").is_none());
    }

    #[test]
    fn the_named_categories_keep_their_designed_colour_whatever_their_position() {
        // Position 7 in the fallback palette is indigo; Food is lime wherever it
        // lands, which is what makes two months' donuts comparable.
        let food = category_hue(7, "Food", None);
        assert_eq!((food.red(), food.green(), food.blue()), (0x84, 0xcc, 0x16));
        // Prefix, not equality: a user who renamed it still gets the lime.
        assert_eq!(category_hue(0, "Food & drink", None), food);
        // Unnamed falls through to the ordinal palette, and keeps its slot.
        assert_eq!(category_hue(2, "Aquarium supplies", None), category_hue(2, "Whatever", None));
        // A stored colour still wins over both.
        let stored = category_hue(0, "Food", Some("#ff0000"));
        assert_eq!((stored.red(), stored.green(), stored.blue()), (0xff, 0, 0));
    }

    #[test]
    fn the_savings_rate_is_a_share_of_income_and_never_invented() {
        let snap = |income: i64, spent: i64| Snapshot {
            liquid_minor: 0,
            debt_minor: 0,
            spent_this_month_minor: spent,
            spent_last_month_minor: None,
            income_this_month_minor: income,
            owed_to_me_minor: 0,
            i_owe_minor: 0,
            subscriptions_yearly_minor: 0,
            open_obligations: 0,
        };
        // The mockup's figure: ₹23,637 saved out of ₹1,38,000 in.
        assert_eq!(saved_pct(&snap(13_800_000, 11_436_300)).as_deref(), Some("17.1%"));
        // No income is no rate, not 0% and not a division by zero.
        assert_eq!(saved_pct(&snap(0, 5_000)), None);
        // Overspent: the sign belongs on the whole figure, not on the decimal.
        assert_eq!(saved_pct(&snap(10_000, 15_000)).as_deref(), Some("-50.0%"));
    }

    #[test]
    fn the_spend_delta_is_absolute_with_a_direction_and_silent_on_a_first_month() {
        let snap = |this: i64, last: Option<i64>| Snapshot {
            liquid_minor: 0,
            debt_minor: 0,
            spent_this_month_minor: this,
            spent_last_month_minor: last,
            income_this_month_minor: 0,
            owed_to_me_minor: 0,
            i_owe_minor: 0,
            subscriptions_yearly_minor: 0,
            open_obligations: 0,
        };
        let spent = |s: &Snapshot| stats(s, "INR")[1].clone();

        // Nothing to compare against — no pill at all, rather than "+100%".
        assert_eq!(spent(&snap(500_000, None)).delta, "");
        // Unchanged is not news either.
        assert_eq!(spent(&snap(500_000, Some(500_000))).delta, "");

        let up = spent(&snap(521_000, Some(500_000)));
        assert!(up.delta_up, "spending more has to read as the bad direction");
        assert!(up.delta.contains("210"), "got {}", up.delta);
        // The figure is unsigned: the arrow and the colour carry the direction, so
        // a minus sign next to a down arrow would say it twice.
        assert!(!up.delta.contains('-'), "got {}", up.delta);

        let down = spent(&snap(400_000, Some(500_000)));
        assert!(!down.delta_up);
        assert!(!down.delta.contains('-'), "got {}", down.delta);
    }

    #[test]
    fn slice_offsets_are_cumulative_and_bounded() {
        let rows = vec![
            CategorySpend { category_id: Some(1), name: "A".into(), color: None, base_minor: 500 },
            CategorySpend { category_id: Some(2), name: "B".into(), color: None, base_minor: 300 },
            CategorySpend { category_id: Some(3), name: "C".into(), color: None, base_minor: 200 },
        ];
        let out = slices(&rows, "INR");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].pct, 50);
        assert_eq!(out[1].pct, 30);
        assert_eq!(out[2].pct, 20);
        // The first starts at twelve o'clock; the second starts where it ended,
        // half a turn on, which is six o'clock.
        assert!(out[0].path.starts_with("M 50.00 12.00"), "got {}", out[0].path);
        assert!(out[1].path.starts_with("M 50.00 88.00"), "got {}", out[1].path);
    }

    #[test]
    fn the_large_arc_flag_is_set_only_past_the_halfway_mark() {
        // Without this SVG takes the short way round and a 70% slice renders as
        // the 30% one.
        assert!(arc_path(0, 70).contains(" 0 1 1 "), "got {}", arc_path(0, 70));
        assert!(arc_path(0, 40).contains(" 0 0 1 "), "got {}", arc_path(0, 40));
    }

    #[test]
    fn one_category_holding_everything_draws_a_full_ring() {
        // start == end, which SVG draws as nothing at all, so this has to be two
        // half arcs instead.
        let p = arc_path(0, 100);
        assert_eq!(p.matches(" A ").count(), 2, "got {p}");
    }

    #[test]
    fn a_rounded_away_slice_draws_nothing() {
        // A category at 0.4% of the month rounds to 0. An empty path is right:
        // a degenerate arc is a rendering question mark.
        assert_eq!(arc_path(0, 0), "");
    }

    #[test]
    fn nothing_spent_produces_no_slices_rather_than_a_division_by_zero() {
        assert!(slices(&[], "INR").is_empty());
        let zero =
            vec![CategorySpend { category_id: None, name: "A".into(), color: None, base_minor: 0 }];
        assert!(slices(&zero, "INR").is_empty());
    }

    #[test]
    fn month_bars_scale_to_the_tallest_month() {
        let rows = vec![
            MonthTotal { period: "2026-06".into(), income_minor: 1_000, expense_minor: 500 },
            MonthTotal { period: "2026-07".into(), income_minor: 500, expense_minor: 250 },
        ];
        let out = months(&rows, "INR");
        assert_eq!(out[0].income_pct, 100);
        assert_eq!(out[0].expense_pct, 50);
        assert_eq!(out[1].income_pct, 50);
        assert_eq!(out[1].label, "Jul");
    }

    #[test]
    fn an_empty_year_does_not_divide_by_zero() {
        assert!(months(&[], "INR").is_empty());
    }

    #[test]
    fn the_calendar_is_always_six_rows_of_seven() {
        let grid = calendar("2026-11", date::parse("2026-11-15").unwrap(), &[], &[], "INR");
        assert_eq!(grid.len(), 42);
        // November 2026 starts on a Sunday, the worst case for a Monday-first
        // grid: six leading blanks.
        assert_eq!(grid.iter().filter(|d| d.in_month).count(), 30);
        assert_eq!(grid[0].in_month, false);
        assert!(grid.iter().any(|d| d.today && d.day == 15));
    }

    #[test]
    fn february_in_a_leap_year_fits() {
        let grid = calendar("2028-02", date::parse("2028-02-29").unwrap(), &[], &[], "INR");
        assert_eq!(grid.len(), 42);
        assert_eq!(grid.iter().filter(|d| d.in_month).count(), 29);
    }

    #[test]
    fn a_bad_period_produces_an_empty_grid_rather_than_a_panic() {
        assert!(calendar("nope", date::today(), &[], &[], "INR").is_empty());
    }

    #[test]
    fn basis_points_read_back_as_a_percentage() {
        let l = LoanRow {
            account_id: 1,
            name: "Home".into(),
            currency: "INR".into(),
            principal_minor: 350_000_000,
            rate_bp: 840,
            tenure_months: 180,
            started_on: "2026-01-05".into(),
            emi_minor: 3_427_000,
            emi_day: 5,
            balance_minor: 350_000_000,
            paid_count: 0,
        };
        assert_eq!(loans(&[l])[0].rate, "8.4%");
    }

    #[test]
    fn source_labels_are_readable_not_database_enums() {
        assert_eq!(source_label("csv"), "CSV");
        assert_eq!(source_label("recurrence"), "Recurrence");
        assert_eq!(source_label("manual"), "Manual");
        assert_eq!(source_label("something new"), "Manual");
    }
}
