// The Finances section: everything touching money.
//
// Seven views over one database. Overview is the strip and the breakdown,
// Transactions is the ledger, Accounts holds balances and loans, Planning is
// the one view that decides rather than reports, and Bills, Subscriptions and
// Lending are three filtered readings of two tables.
//
// Two tables carry the model, and the shape is worth stating once because
// every figure below is somewhere on it:
//
//     RECURRENCE          →  OBLIGATION              →  TRANSACTION
//     (template)             (owed, not yet money)      (money moved)
//
// Subscriptions and Bills are filtered views over one table, not two tables, so
// a price change is visible in every total at once.
//
// Almost none of the arithmetic is here. `tulipix-finances` owns the money —
// `i64` minor units, never a float — the recurrence engine, the budget maths,
// the amortisation, the insight flags and the statement import. What this file
// does is decide what the screen is looking at, ask that crate, and turn minor
// units into the strings Dart draws. Two conventions it is responsible for,
// both rules of the design rather than styling choices:
//
//   - An estimate arrives with its `~` already attached and `is_estimate` set,
//     so a guess can never be presented as a measurement.
//   - A transfer carries `kind: "transfer"` so the ledger can grey it back. It
//     is shown, because hiding it makes balances unexplainable, but it must not
//     look like spending.
//
// One thing the Slint build did that this does not: arc paths. `view::slices`
// emitted SVG commands because Slint has no trigonometry. Flutter's Canvas has
// `arcTo`, so the slice carries its start and its share and Dart draws it —
// forty lines of geometry that the port simply deletes.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use flutter_rust_bridge::frb;

use crate::db::finances_pool;
use crate::fin_sheet::{self, Field};
use crate::frb_generated::StreamSink;

use tulipix_finances::{
    accounts::{self, AccountKind, AccountRow, Detail as AccountDetail},
    budgets::{self, BudgetRow, MonthDiscipline},
    date,
    dues::{self, Due},
    fx,
    import::{self, csv::RawRow, dates::DateFormat, detect, Preview},
    insights::{self, Flag, Health, HealthPart, MonthSavings, Projection, Severity, Snapshot},
    loans::{self, Instalment, LoanRow},
    money, obligations,
    obligations::{Obligation, Status as ObStatus},
    recur::{self, Price, RecurRow},
    txn::{self, CategorySpend, MonthTotal, TxnFilter, TxnKind, TxnRow, TxnSort},
};

// ------------------------------------------------------------------ colour ---
//
// Hues cross as `0xAARRGGBB`, not as a colour type. Dart builds a `Color` from
// the int; a struct carrying a platform colour would be a struct Dart could not
// construct in a test.

/// Fallback palette for the spend breakdown, for categories the design did not
/// name. Fixed order rather than a hash of the name: the same category keeps
/// the same colour between months, which is what makes two months comparable.
const SLICE_HUES: &[u32] = &[
    0xFF84cc16, 0xFFfacc15, 0xFF06b6d4, 0xFF8b5cf6, 0xFFec4899, 0xFF10b981, 0xFFf97316,
    0xFF6366f1, 0xFF14b8a6, 0xFFf43f5e,
];

/// Colours for the categories the section seeds itself. Ahead of the ordinal
/// palette, so the default set always gets the hue the design assigned it —
/// loans pink, food lime, utilities yellow — however the months order them.
/// Matched on a lowercase prefix so "Food & drink" and "Food" land together.
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

/// Colours for the account and loan badges.
const BADGE_HUES: &[u32] = &[
    0xFF3b82f6, 0xFFef4444, 0xFF10b981, 0xFF06b6d4, 0xFFf472b6, 0xFF8b5cf6, 0xFFf97316,
    0xFFfacc15, 0xFF14b8a6, 0xFF6366f1,
];

const TIMELINE_INCOME: u32 = 0xFF16a34a;
const TIMELINE_BILL: u32 = 0xFFf472b6;
const TIMELINE_SUB: u32 = 0xFFa78bfa;
const TIMELINE_OTHER: u32 = 0xFF94a3b8;

/// `#rrggbb` or `#aarrggbb`. `None` on anything else rather than substituting a
/// colour, so the palette fallback takes over.
fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok().map(|v| 0xFF00_0000 | v),
        8 => u32::from_str_radix(h, 16).ok(),
        _ => None,
    }
}

/// A category's colour: its own stored one, then the named palette, then the
/// ordinal fallback.
fn category_hue(i: usize, name: &str, stored: Option<&str>) -> u32 {
    if let Some(css) = stored
        && let Some(c) = parse_hex(css)
    {
        return c;
    }
    let lower = name.to_lowercase();
    for (key, argb) in NAMED_HUES {
        if lower.starts_with(key) {
            return *argb;
        }
    }
    SLICE_HUES[i % SLICE_HUES.len()]
}

/// Hands out badge colours, one grid at a time.
///
/// A name alone is not enough: ten colours and six accounts collide about eighty
/// per cent of the time, and two cards wearing the same colour is exactly what a
/// badge exists to prevent. So the name picks where to *start*, and a taken
/// colour steps on to the next free one.
#[frb(ignore)]
#[derive(Default)]
struct HuePicker {
    used: Vec<usize>,
}

impl HuePicker {
    fn pick(&mut self, name: &str) -> u32 {
        let n: usize = name.bytes().map(usize::from).sum();
        let start = n % BADGE_HUES.len();
        let idx = (0..BADGE_HUES.len())
            .map(|k| (start + k) % BADGE_HUES.len())
            .find(|i| !self.used.contains(i))
            // More accounts than colours. Repeating is all that is left.
            .unwrap_or(start);
        self.used.push(idx);
        BADGE_HUES[idx]
    }
}

/// A badge colour from the name alone.
///
/// Unlike the account grid this does not step around collisions: the
/// subscription table is filtered, sorted and paged, so a colour that depended
/// on which rows are on screen would change under the reader. Stable beats
/// unique; the letter carries the difference when two names share a hue.
fn stable_hue(name: &str) -> u32 {
    let n: usize = name.bytes().map(usize::from).sum();
    BADGE_HUES[n % BADGE_HUES.len()]
}

/// The first *alphanumeric* character, uppercased — so a service called
/// "₹ spare change" gets a letter rather than a symbol.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "•".to_string())
}

/// The glyph on an account's badge: its initial, or the currency symbol when it
/// holds cash. A wallet of notes is not "C for Cash", it is money.
fn badge(name: &str, kind: AccountKind, base: &str) -> String {
    if kind == AccountKind::Cash
        && let Some(sym) = money::symbol(base)
    {
        return sym.to_string();
    }
    initial(name)
}

// ------------------------------------------------------------------- state ---

/// One tile in a stat strip. `tone` is ok | warn | bad | flat.
///
/// `delta` is a change against the previous month, shown instead of `sub` when
/// there is one — empty when there is no month to compare against, because a
/// first-month "+100%" is arithmetic rather than information.
pub struct FinStat {
    pub label: String,
    pub value: String,
    pub sub: String,
    pub tone: String,
    pub delta: String,
    pub delta_up: bool,
    /// Non-empty when the card opens something.
    pub action: String,
}

/// One component of the health score, for the panel behind it.
pub struct FinHealthPart {
    pub label: String,
    /// "18 / 25", or "not measured".
    pub points: String,
    pub pct: i32,
    pub detail: String,
    pub measured: bool,
    pub tone: String,
}

pub struct FinAccountRow {
    pub id: i64,
    pub name: String,
    /// bank | cash | card | wallet | loan | virtual
    pub kind: String,
    /// One glyph — the account's initial, or the currency symbol when it holds
    /// cash — on a colour picked from its own name, so the same account keeps
    /// the same badge however the list reorders.
    pub badge: String,
    pub hue: u32,
    pub balance: String,
    pub sub: String,
    /// 0 when there is no credit limit.
    pub util_pct: i32,
    pub negative: bool,
    pub closed: bool,
    pub txn_count: i64,
    // Each of these is empty when it does not apply to this kind of account, and
    // the card leaves it out — a bank account has a reconcile and a month's
    // flow, a card has a statement, a wallet has a top-up, and none of them
    // should show the others' blanks.
    pub reconciled: String,
    pub in_month: String,
    pub out_month: String,
    pub limit_note: String,
    pub statement: String,
    pub minimum_due: String,
    pub drift: String,
    pub drift_bad: bool,
    pub topped_from: String,
    pub last_topup: String,
    /// The merged holding card only. Lent out and Borrowed are two halves of one
    /// fact, so they are one card with both figures.
    pub lent: String,
    pub borrowed: String,
}

pub struct FinTxnRow {
    pub id: i64,
    pub date: String,
    pub description: String,
    pub category: String,
    /// The colour this category has in the donut, carried per row so a tag in
    /// the ledger and a slice in the chart are visibly the same category.
    pub cat_hue: u32,
    pub account: String,
    /// Manual | CSV | OFX | Receipt | Recurrence
    pub source: String,
    pub amount: String,
    /// Empty unless one account is filtered.
    pub running: String,
    /// expense | income | transfer
    pub kind: String,
}

/// A recurrence: subscription or bill. One struct, because they are one table.
pub struct FinRecurRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Already carries its `~` when it is an estimate.
    pub amount: String,
    pub is_estimate: bool,
    pub cycle: String,
    pub next_due: String,
    /// The same date without its year, for the table column.
    pub next_short: String,
    /// Negative when overdue.
    pub days: i64,
    pub status: String,
    pub account: String,
    pub category: String,
    pub badge: String,
    pub hue: u32,
    /// What the service charges under its own name — its note, or its own-currency
    /// price. The columns beside it are base-currency.
    pub plan: String,
    /// Whole percent above the oldest recorded price. 0 when it has never risen.
    pub hike_pct: i32,
    pub yearly: String,
    /// The same commitment per month. Both are shown: a monthly figure is what
    /// someone feels and a yearly one is what they cancel on.
    pub monthly: String,
    pub hike_from: String,
    pub auto_post: bool,
    pub last_paid: String,
    pub cat_hue: u32,
    /// The amount as the service charges it, when that is not the base currency.
    pub original: String,
}

/// A materialised instance: dated money that is owed but has not moved.
pub struct FinObligationRow {
    pub id: i64,
    pub name: String,
    /// subscription | bill | one-off
    pub kind: String,
    pub due: String,
    /// Day of the month, so the calendar's agenda can pick one day's rows out of
    /// the month it already has rather than asking SQL again on every click.
    pub day: i32,
    pub days: i64,
    pub estimate: String,
    pub actual: String,
    /// Empty until both are known.
    pub variance: String,
    /// The actual came in above the estimate.
    pub over: bool,
    /// upcoming | due | overdue | paid | skipped
    pub status: String,
    pub is_estimate: bool,
    pub category: String,
    pub cat_hue: u32,
    pub account: String,
    /// Posts itself on its due date, so it is not on the same footing as a bill
    /// that needs action.
    pub auto_post: bool,
}

pub struct FinDueRow {
    pub id: i64,
    pub person: String,
    /// owed_to_me | i_owe
    pub direction: String,
    pub amount: String,
    pub original: String,
    pub settled: String,
    pub opened: String,
    pub age_days: i64,
    /// open | settled | written_off
    pub status: String,
    pub note: String,
    pub part_paid: bool,
}

pub struct FinBudgetRow {
    pub category_id: i64,
    pub name: String,
    pub budget: String,
    pub spent: String,
    pub remaining: String,
    pub used_pct: i32,
    pub projected: String,
    pub projected_pct: i32,
    pub over: bool,
    pub off_pace: bool,
    pub rollover: bool,
    pub carried: String,
    /// Spending with no envelope: worth setting one on.
    pub unset: bool,
    /// "14 transactions · biggest ₹1,240 Toit". An envelope broken by one
    /// purchase is a different problem from one broken by forty.
    pub detail: String,
    /// How far over, as words, when it is over.
    pub over_by: String,
}

pub struct FinLoanRow {
    pub account_id: i64,
    pub name: String,
    pub badge: String,
    pub hue: u32,
    pub balance: String,
    pub principal: String,
    pub emi: String,
    pub rate: String,
    pub progress_pct: i32,
    pub remaining_months: i64,
    pub tenure_months: i64,
    pub next_due: String,
    /// How the next instalment divides — the number that tells someone why a
    /// loan they have paid for five years has barely moved.
    pub emi_principal: String,
    pub emi_interest: String,
    pub closes: String,
    pub interest_left: String,
    pub interest_free: bool,
}

pub struct FinInstalment {
    pub n: i64,
    pub due: String,
    pub emi: String,
    pub principal: String,
    pub interest: String,
    pub balance: String,
    /// Share of this instalment that is interest.
    pub interest_pct: i32,
}

/// One slice of a ring or a bar list.
///
/// No path: the Slint build shipped SVG commands because Slint has no
/// trigonometry. `start` and `pct` are percent-of-the-circle, and Dart's Canvas
/// draws the arc from those two numbers.
pub struct FinCatSlice {
    pub name: String,
    pub amount: String,
    pub pct: i32,
    pub start: i32,
    pub hue: u32,
}

/// One bar in the twelve-month chart. `expense_pct` is of the heaviest month's
/// spending, which is what the chart's axis is topped with.
pub struct FinMonthBar {
    pub label: String,
    /// The month and its year, for the card's heading — the bar's own label has
    /// no room for a year.
    pub long: String,
    pub expense: String,
    pub expense_pct: i32,
}

pub struct FinCalDay {
    pub day: i32,
    pub in_month: bool,
    pub is_today: bool,
    pub count: i32,
    /// Money owed on the day, empty when none is.
    pub amount: String,
    /// Money arriving on the day, empty when none does.
    pub income: String,
    pub money_out: bool,
    pub overdue: bool,
    /// Up to four category dots, so a day reads as "loan and two bills" before
    /// any figure is looked at.
    pub pips: Vec<u32>,
}

/// One month of the Year view: twelve of these instead of a day grid.
pub struct FinCalMonth {
    pub label: String,
    pub out: String,
    pub income: String,
    /// Share of the busiest month, for the bar.
    pub pct: i32,
    pub current: bool,
    pub count: i32,
}

/// A label and a figure. Used wherever the design shows a list of them — the
/// cash-flow walk, the fixed obligations no envelope covers, the position
/// summary — rather than four near-identical structs.
pub struct FinKv {
    pub label: String,
    pub value: String,
    /// flat | ok | warn | bad
    pub tone: String,
    pub strong: bool,
}

/// One envelope's row in the six-month discipline grid. `cells` is one string
/// per month, "—" for a month the envelope did not exist in.
pub struct FinEnvelopeHistory {
    pub name: String,
    pub cells: Vec<String>,
    pub over: Vec<bool>,
    pub avg_delta: String,
    pub avg_over: bool,
}

/// A claim with a number behind it, and something to do about it.
pub struct FinFlag {
    /// bad | warn | info | good
    pub severity: String,
    pub title: String,
    pub detail: String,
    /// Empty means there is nothing to press.
    pub action_label: String,
    /// Routed back to Rust verbatim, encoded `kind:id`.
    pub action: String,
}

pub struct FinDiscipline {
    pub period: String,
    pub budget: String,
    pub spent: String,
    pub within: bool,
    pub pct: i32,
}

/// One month of the savings-rate chart. `known` is false for a month with no
/// income at all — drawn as a gap rather than as a zero, because "the salary
/// landed late" and "nothing was saved" are not the same claim.
pub struct FinSavingsMonth {
    pub label: String,
    pub pct: i32,
    pub known: bool,
    pub current: bool,
    pub amount: String,
}

/// One month of the Trends drill-down, as a ring. Built by the same `slices`
/// that draws the Overview's donut, so the two cannot disagree about a month.
pub struct FinTrendPie {
    pub label: String,
    pub long: String,
    pub total: String,
    pub slices: Vec<FinCatSlice>,
}

/// One metric of the six-month chart. The chart shows one at a time and the
/// selector picks which, so all of them travel and the switch costs no query.
pub struct FinTrendSeries {
    pub name: String,
    pub months: Vec<FinSavingsMonth>,
}

/// One card on the Planning timeline: a day this month that does something.
///
/// A day, not an obligation. Three bills on the 8th are one thing that happens
/// on the 8th, and drawing them as three cards makes a busy day look like a
/// busy week.
pub struct FinTimelineEvent {
    pub day: i32,
    /// Where that day sits in `cal_days`, so clicking a card can hand the
    /// calendar the cell itself. -1 when the day is not on the grid.
    pub idx: i32,
    pub name: String,
    pub amount: String,
    pub sub: String,
    pub hue: u32,
    pub income: bool,
    /// Not an event — it is where the row leads, and the design puts it at the
    /// end of the row for exactly that reason.
    pub closing: bool,
    pub count: i32,
}

/// An envelope and the date it runs out on at the pace it is going.
pub struct FinPrediction {
    pub name: String,
    pub finish: String,
    pub note: String,
    pub hue: u32,
    /// Runs out before the month does. The whole point of the card.
    pub soon: bool,
    /// Daily spend, each a share of this envelope's own worst day.
    pub points: Vec<i32>,
}

/// One row of the habits list. `pct` is only drawn when `bar` is set — two of
/// the four are counts, and a bar under a count would be measuring nothing.
pub struct FinHabit {
    pub label: String,
    pub value: String,
    pub pct: i32,
    pub hue: u32,
    pub bar: bool,
    pub action_label: String,
    pub action: String,
}

/// One line of the exchange-rate monitor.
#[derive(Clone)]
pub struct FinRate {
    pub code: String,
    /// "US dollar" — a code alone is a puzzle for eight of the ten.
    pub name: String,
    pub rate: String,
    /// The same rate the other way round, so the pair converts in both
    /// directions without anyone dividing in their head.
    pub inverse: String,
    pub edited: String,
    pub live: bool,
    /// Watched, but nothing fetched for it yet.
    pub known: bool,
}

#[derive(Clone)]
pub struct FinPrice {
    pub amount: String,
    pub from: String,
}

/// A detected recurring charge, awaiting confirmation. Nothing is created until
/// the user presses Add.
pub struct FinProposal {
    pub idx: i32,
    pub name: String,
    pub kind: String,
    pub amount: String,
    pub cycle: String,
    pub next_due: String,
    pub occurrences: i32,
    /// high | low
    pub confidence: String,
}

#[derive(Clone)]
pub struct FinImportRow {
    pub date: String,
    pub description: String,
    pub amount: String,
    pub credit: bool,
}

/// One field of whichever sheet is open. The form is built in Rust and rendered
/// generically — the same approach Tools takes, and the reason this section
/// does not contain thirteen hand-built dialogs.
#[derive(Clone)]
pub struct FinField {
    pub key: String,
    pub label: String,
    /// text | number | money | date | dropdown | toggle | static | note
    pub kind: String,
    pub value: String,
    pub hint: String,
    pub options: Vec<String>,
    pub required: bool,
}

/// Everything the section shows, in one struct.
///
/// Every tab is filled on every dispatch, not just the visible one. There is one
/// database under all seven of them, and filling only the active tab leaves the
/// other six showing whatever they last had — which is exactly how a tab comes
/// to be opened onto stale figures.
pub struct FinancesState {
    /// overview | txns | accounts | planning | bills | subs | dues
    pub tab: String,

    // ── overview ────────────────────────────────────────────────────────────
    pub stats: Vec<FinStat>,
    pub health_parts: Vec<FinHealthPart>,
    pub health_score: String,
    pub health_band: String,
    pub slices: Vec<FinCatSlice>,
    pub spent_total: String,
    pub months: Vec<FinMonthBar>,
    pub month_label: String,
    /// Which month the breakdown is showing, as an index into `months`. -1 is
    /// the latest, which is what the section opens on.
    pub month_pick: i32,
    pub needs_you: Vec<FinObligationRow>,
    /// The overview's own short ledger — unfiltered, so the Transactions tab's
    /// filters cannot silently change what "recent" means on the dashboard.
    pub recent: Vec<FinTxnRow>,

    // ── ledger ──────────────────────────────────────────────────────────────
    pub txns: Vec<FinTxnRow>,
    pub txn_page: i32,
    pub txn_pages: i32,
    pub txn_total: i32,
    pub txn_sort: String,
    pub txn_desc: bool,
    pub txn_search: String,
    pub txn_account: String,
    pub txn_category: String,
    pub txn_kind: String,
    pub txn_period: String,
    pub txn_source: String,
    pub account_names: Vec<String>,
    pub category_names: Vec<String>,
    pub txn_periods: Vec<String>,
    pub source_names: Vec<String>,
    pub running_shown: bool,
    pub txn_spent: String,
    pub txn_income: String,

    // ── bills ───────────────────────────────────────────────────────────────
    pub bills: Vec<FinObligationRow>,
    pub bill_stats: Vec<FinStat>,
    pub bill_templates: Vec<FinRecurRow>,
    pub bills_filter: String,
    pub bills_period: String,
    pub bills_all: i32,
    pub bills_needs: i32,
    pub bills_auto: i32,
    pub bills_paid: i32,
    pub bills_oneoff: i32,

    // ── subscriptions ───────────────────────────────────────────────────────
    pub subs: Vec<FinRecurRow>,
    pub sub_stats: Vec<FinStat>,
    pub subs_yearly: String,
    pub subs_filter: String,
    pub subs_sort: String,
    pub subs_desc: bool,
    pub subs_category: String,
    pub subs_currency: String,
    pub subs_categories: Vec<String>,
    pub subs_currencies: Vec<String>,
    pub subs_all: i32,
    pub subs_active: i32,
    pub subs_paused: i32,
    pub subs_cancelled: i32,
    pub subs_removed: i32,

    // ── lending ─────────────────────────────────────────────────────────────
    pub dues: Vec<FinDueRow>,
    /// Everything settled or written off in the last six months. Its own card
    /// rather than only behind the Closed filter: the useful question is "does
    /// this get paid back", and that needs the history next to the open rows.
    pub dues_settled: Vec<FinDueRow>,
    pub due_stats: Vec<FinStat>,
    pub dues_filter: String,
    pub dues_all: i32,
    pub dues_to_me: i32,
    pub dues_i_owe: i32,
    pub dues_closed: i32,

    // ── accounts and loans ──────────────────────────────────────────────────
    pub accounts: Vec<FinAccountRow>,
    pub account_stats: Vec<FinStat>,
    pub account_filter: String,
    pub liquid_total: String,
    pub debt_total: String,
    /// Loans only. `debt_total` is every negative balance including the credit
    /// card, which is not a loan and must not head the Loans grid.
    pub loans_total: String,
    pub loans: Vec<FinLoanRow>,

    // ── planning ────────────────────────────────────────────────────────────
    pub plan_health: Vec<FinStat>,
    pub timeline: Vec<FinTimelineEvent>,
    pub attention: Vec<FinFlag>,
    pub recos: Vec<FinFlag>,
    pub predictions: Vec<FinPrediction>,
    pub habits: Vec<FinHabit>,
    pub review: Vec<FinStat>,
    pub trends: Vec<FinTrendSeries>,
    /// Which of Planning's drill-downs is open, "" for none.
    pub plan_modal: String,
    pub plan_statement: Vec<FinKv>,
    /// Income less commitments less envelopes. The slice the bar draws, so the
    /// number under it cannot be a different subtraction.
    pub plan_free: String,
    pub plan_free_pct: i32,

    // ── envelopes ───────────────────────────────────────────────────────────
    pub budgets: Vec<FinBudgetRow>,
    pub budget_period: String,
    pub budget_income: String,
    pub budget_allocated: String,
    pub budget_unallocated: String,
    pub budget_unbudgeted: String,
    pub budget_unbudgeted_sub: String,
    pub budget_allocated_pct: i32,
    pub commitments: Vec<FinCatSlice>,
    /// What leaves every month whatever the user decides — EMIs, bills, subs —
    /// itemised. Shown so the page adds up: you cannot overspend a bill, you can
    /// only fail to pay it, and hiding it makes the envelopes look like the
    /// whole month.
    pub fixed_items: Vec<FinKv>,
    pub fixed_total: String,
    pub fixed_share: String,
    pub fixed_pct: i32,
    pub envelopes_spent: String,
    pub discipline: Vec<FinDiscipline>,
    pub discipline_months: Vec<String>,
    pub envelope_history: Vec<FinEnvelopeHistory>,

    // ── calendar ────────────────────────────────────────────────────────────
    pub cal_days: Vec<FinCalDay>,
    pub cal_label: String,
    /// The day the agenda opens on. Rust picks it, because "today, unless you
    /// have stepped away from this month" is a claim about a date and not a
    /// layout.
    pub cal_focus: i32,
    pub cal_agenda: Vec<FinObligationRow>,
    pub cal_view: String,
    pub cal_months: Vec<FinCalMonth>,
    pub cal_flow: Vec<FinKv>,
    pub cal_heaviest_items: Vec<FinObligationRow>,
    /// The cash-flow sentence, empty when the month is covered. Written here
    /// because it is a claim about someone's money, not a label.
    pub cal_warning: String,
    pub cal_heaviest: String,
    pub cal_heaviest_label: String,
    pub cal_heaviest_sub: String,
    pub cal_low_point: String,

    // ── insights ────────────────────────────────────────────────────────────
    pub flags: Vec<FinFlag>,
    pub insight_stats: Vec<FinStat>,
    pub trend_pies: Vec<FinTrendPie>,
    pub savings: Vec<FinSavingsMonth>,
    pub savings_summary: Vec<FinKv>,
    pub projection: Vec<FinKv>,
    pub projection_note: String,

    // ── the sheet ───────────────────────────────────────────────────────────
    /// One modal at a time, named by a string for the same reason `tab` is a
    /// string: two booleans can be true at once and a name cannot.
    pub sheet: String,
    pub sheet_title: String,
    pub sheet_hint: String,
    pub sheet_primary: String,
    pub sheet_error: String,
    /// An answer rather than a complaint: what an envelope has left, what OCR
    /// read, what a prepayment would save.
    pub sheet_note: String,
    pub sheet_busy: bool,
    pub form: Vec<FinField>,
    pub prices: Vec<FinPrice>,
    pub schedule: Vec<FinInstalment>,
    pub schedule_page: i32,
    pub schedule_pages: i32,
    pub import_rows: Vec<FinImportRow>,
    pub import_summary: String,
    pub import_needs_format: bool,
    pub import_map: Vec<FinField>,
    pub proposals: Vec<FinProposal>,
    pub rates: Vec<FinRate>,
    pub rates_busy: bool,

    // ── the section's own condition ─────────────────────────────────────────
    /// Nothing has been entered yet — no account, no transaction. The first-run
    /// door, shown in place of an Overview full of zeroes.
    pub empty: bool,
    /// Sample data is still on screen. Never hidden: invented money that does
    /// not announce itself is the one thing this section must not do.
    pub demo: bool,
    /// Non-empty while the sample data is being written or cleared, and what to
    /// call it. A string rather than a bool: the two operations take about the
    /// same time and look identical, so the chip has to say which is running.
    pub demo_busy: String,
    /// How many things are inside the notice window, and whether any are late.
    pub badge: i32,
    pub badge_overdue: bool,
}

/// Something the section noticed on a tick, worth one banner and no more.
///
/// The wording and the decision to stay quiet both live in `tulipix-finances`,
/// where they are tested. This only carries the result across.
pub enum FinancesEvent {
    Notice { title: String, body: String },
    Failed { message: String },
}

// ---------------------------------------------------------------- commands ---

/// Everything the section can be asked to do.
///
/// Filters and periods are commands rather than arguments to a query because
/// they are *state*: the ledger's month filter has to survive paying a bill on
/// another tab, and passing it in on every call would mean seven callers who
/// each have to remember it.
pub enum FinancesCmd {
    Refresh,
    SetTab { tab: String },

    // Overview
    PickMonth { index: i32 },

    // Ledger
    TxnSearch { text: String },
    TxnAccount { name: String },
    TxnCategory { name: String },
    TxnKindFilter { kind: String },
    TxnPeriod { period: String },
    TxnSource { source: String },
    TxnSortBy { column: String },
    TxnGoto { page: i32 },
    TxnOpen { id: i64 },
    TxnDelete { id: i64 },

    // Obligations
    PayObligation { id: i64 },
    SkipObligation { id: i64 },
    UnpayObligation { id: i64 },
    SetBillsFilter { filter: String },
    BillsStep { delta: i32 },

    // Recurrences
    RecurStatus { id: i64, status: String },
    RecurDelete { id: i64 },
    SetSubsFilter { filter: String },
    SubsSortBy { column: String },
    SubsCategory { name: String },
    SubsCurrency { code: String },

    // Lending
    DueSettle { id: i64 },
    DueWriteOff { id: i64 },
    DueDelete { id: i64 },
    SetDuesFilter { filter: String },

    // Accounts and loans
    SetAccountFilter { filter: String },
    AccountReconcile { id: i64 },
    AccountRecount { id: i64 },
    AccountPayCard { id: i64 },
    AccountClose { id: i64, closed: bool },
    AccountDelete { id: i64 },
    LoanSchedule { id: i64 },
    LoanPrepay { id: i64 },
    ScheduleStep { delta: i32 },

    // Envelopes and the calendar
    BudgetRemove { category_id: i64 },
    BudgetCopyForward,
    BudgetStep { delta: i32 },
    CalStep { delta: i32 },
    SetCalView { view: String },
    PlanStep { delta: i32 },
    /// An offset from the month today is in, rather than a period string: "next
    /// month" has to keep meaning next month after midnight on the 31st.
    PlanGoto { offset: i32 },
    SetPlanModal { which: String },

    // Sheets
    OpenSheet { kind: String, id: i64 },
    CloseSheet,
    SetField { key: String, value: String },
    SubmitSheet,
    /// Saves and reopens a blank form of the same kind, for entering several
    /// things in a row.
    SubmitAgain,
    FlagAction { action: String },

    // Import, receipts, detection, rates
    //
    // Both carry the path Dart's chooser returned. The bridge used to open the
    // dialog itself, through `rfd`; the extension lists the chooser filters on
    // are still this side's — `import::EXTENSIONS` for statements — and are
    // exported below rather than transcribed into Dart.
    ImportPick { path: String },
    ScanReceipt { path: String },
    AcceptProposal { idx: i32 },
    RatesRefresh,
    /// Opens the ledger filtered to the account being reconciled.
    FindMissing,

    // Sample data
    DemoAdd,
    DemoRemove,
}

// ----------------------------------------------------------------- session ---

const ALL_ACCOUNTS: &str = "All accounts";
const ALL_CATEGORIES: &str = "All categories";
const EVERY_MONTH: &str = "Every month";
const ANY_SOURCE: &str = "Any source";
const EVERY_SUB_CATEGORY: &str = "Every category";
const EVERY_CURRENCY: &str = "Every currency";
const NO_PRESET: &str = "None";
/// Label for "do not use a column from the file".
const NO_COLUMN: &str = "— none —";

/// Rows of the ledger a page holds, and of a preview and an amortisation.
const OVERVIEW_TXNS: usize = 8;
const IMPORT_PREVIEW_ROWS: usize = 40;
const SCHEDULE_PAGE: usize = 10;
const DISCIPLINE_MONTHS: i64 = 6;
const TREND_MONTHS: u32 = 6;
/// How many months of dues history the settled card carries.
const DUES_HISTORY_DAYS: i64 = 182;

/// What the section is looking at. Everything here is a view decision, not data:
/// the data is re-read on every dispatch.
#[frb(ignore)]
#[derive(Default)]
struct Session {
    tab: String,
    page: u32,
    /// `YYYY-MM` each of the three independently-stepped views is pointed at.
    budget_period: String,
    cal_period: String,
    cal_view: String,
    bills_period: String,

    txn_search: String,
    txn_account: String,
    txn_category: String,
    txn_kind: String,
    txn_period: String,
    txn_source: String,
    txn_sort: String,
    txn_desc: bool,

    bills_filter: String,
    subs_filter: String,
    subs_sort: String,
    subs_desc: bool,
    subs_category: String,
    subs_currency: String,
    dues_filter: String,
    account_filter: String,
    plan_modal: String,

    /// Which sheet is open, and what it is editing.
    sheet_kind: String,
    sheet_id: i64,
    sheet_title: String,
    sheet_hint: String,
    sheet_primary: String,
    sheet_error: String,
    sheet_note: String,
    sheet_busy: bool,
    /// Field values as the user has typed them.
    form: HashMap<String, String>,
    /// Values to overwrite the next sheet's fields with, consumed once.
    ///
    /// Needed because building a sheet is async: anything written into `form`
    /// straight after opening one is wiped when the build finishes and seeds the
    /// form from its own defaults. This is applied inside that build instead.
    prefill: HashMap<String, String>,
    fields: Vec<Field>,

    /// Extra content some sheets carry alongside the form.
    prices: Vec<FinPrice>,
    proposals: Vec<detect::Proposal>,
    rates: Vec<FinRate>,
    rates_busy: bool,
    /// A parsed statement waiting to be confirmed, the account it lands in, and
    /// the text it came from — kept so a mapping correction can re-read the file
    /// rather than ask for it again.
    import: Option<(Preview, i64, String)>,
    import_rows: Vec<FinImportRow>,
    import_map: Vec<FinField>,
    import_summary: String,
    import_needs_format: bool,
    /// The amortisation the sheet is showing, and where in it we are. Held whole
    /// so stepping a page is a slice rather than a query: a twenty-year loan is
    /// 240 instalments, and building all of them at once is what made the sheet
    /// take a visible moment to open.
    schedule: Vec<Instalment>,
    schedule_currency: String,
    schedule_page: usize,

    /// Which of the twelve month bars the Overview's breakdown is showing.
    /// `None` is the latest, which is what the section opens on.
    month_pick: Option<usize>,
    /// Spending by category for each of those months, so clicking a bar is a
    /// lookup rather than a query — the ring has to repaint under the cursor.
    month_cats: Vec<Vec<CategorySpend>>,
    month_base: String,

    demo_busy: String,
    /// Set once, so the calendar and envelope periods start on this month.
    seeded: bool,
}

fn session() -> MutexGuard<'static, Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Fill in what a fresh session has not decided yet. Called on the way into
/// every dispatch, so a command that arrives before the first refresh still
/// finds a month to work in.
fn seed(s: &mut Session) {
    if s.seeded {
        return;
    }
    let period = date::ym(date::today());
    s.tab = "overview".into();
    s.budget_period = period.clone();
    s.cal_period = period.clone();
    s.bills_period = period;
    s.cal_view = "month".into();
    s.txn_account = ALL_ACCOUNTS.into();
    s.txn_category = ALL_CATEGORIES.into();
    s.txn_kind = "All".into();
    s.txn_period = EVERY_MONTH.into();
    s.txn_source = ANY_SOURCE.into();
    s.txn_sort = "date".into();
    s.txn_desc = true;
    s.bills_filter = "all".into();
    s.subs_filter = "all".into();
    s.subs_sort = "name".into();
    s.subs_category = EVERY_SUB_CATEGORY.into();
    s.subs_currency = EVERY_CURRENCY.into();
    s.dues_filter = "all".into();
    s.seeded = true;
}

/// `YYYY-MM` stepped by whole months, across year boundaries in both
/// directions. Nonsense in comes back unchanged rather than panicking.
fn step_period(period: &str, delta: i32) -> String {
    let Some((y, m)) = period.split_once('-') else { return period.to_string() };
    let (Ok(y), Ok(m)) = (y.parse::<i32>(), m.parse::<i32>()) else {
        return period.to_string();
    };
    let total = y * 12 + (m - 1) + delta;
    format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1)
}

fn previous_period(period: &str) -> String {
    step_period(period, -1)
}

/// The id behind a dropdown label, or `None` for the "all" sentinel.
fn pick_id(pairs: &[(String, i64)], label: &str) -> Option<i64> {
    pairs.iter().find(|(n, _)| n == label).map(|(_, id)| *id)
}

// ------------------------------------------------------------------- views ---

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "2026-07" → "Jul". The year is implicit in a twelve-month window and would
/// not fit under a 9px bar anyway.
fn month_short(period: &str) -> String {
    period
        .split_once('-')
        .and_then(|(_, m)| m.parse::<usize>().ok())
        .and_then(|m| MONTHS.get(m.wrapping_sub(1)).copied())
        .unwrap_or(period)
        .to_string()
}

/// "July 2026" from a `YYYY-MM` period.
fn month_long(period: &str) -> String {
    const FULL: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    match period.split_once('-') {
        Some((y, m)) => match m.parse::<usize>().ok().and_then(|m| FULL.get(m.wrapping_sub(1))) {
            Some(name) => format!("{name} {y}"),
            None => period.to_string(),
        },
        None => period.to_string(),
    }
}

/// `2026-07-21` → `21 Jul`. The input unchanged if it is not a date, so a
/// malformed value shows itself rather than becoming an empty cell.
fn day_month(iso: &str) -> String {
    match date::parse(iso) {
        Ok(d) => format!("{} {}", d.day(), MONTHS[d.month0() as usize]),
        Err(_) => iso.to_string(),
    }
}

/// `2041-03-05` → `Mar 2041`.
fn month_year(iso: &str) -> String {
    match date::parse(iso) {
        Ok(d) => format!("{} {}", MONTHS[d.month0() as usize], d.year()),
        Err(_) => iso.to_string(),
    }
}

/// Human name for `transactions.source`, so the provenance column means
/// something to a reader rather than echoing a database enum.
fn source_label(raw: &str) -> &'static str {
    match raw {
        "csv" => "CSV",
        "ofx" => "OFX",
        "ocr" => "Receipt",
        "recurrence" => "Recurrence",
        _ => "Manual",
    }
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

fn stat(label: &str, value: String, sub: String, tone: &str) -> FinStat {
    FinStat {
        label: label.into(),
        value,
        sub,
        tone: tone.into(),
        delta: String::new(),
        delta_up: false,
        action: String::new(),
    }
}

/// Share of this month's income that survived, to one decimal place.
///
/// Integer arithmetic in tenths of a percent — the crate's rule is that money
/// never touches a float, and a percentage *of* money is close enough to money
/// to keep the rule. `None` when there is no income to be a share of.
fn saved_pct(snap: &Snapshot) -> Option<String> {
    if snap.income_this_month_minor <= 0 {
        return None;
    }
    let tenths =
        snap.saved_this_month_minor() as i128 * 1000 / snap.income_this_month_minor as i128;
    Some(format!("{}.{}%", tenths / 10, (tenths % 10).abs()))
}

fn overview_stats(snap: &Snapshot, health: &Health, base: &str) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    // Month-on-month change in spending, as a signed figure and a direction. Up
    // is the bad direction here — this labels spending, not growth.
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
        stat(
            "LIQUID",
            f(snap.liquid_minor),
            "Bank, cash and wallets".into(),
            if snap.liquid_minor < 0 { "bad" } else { "flat" },
        ),
        FinStat {
            delta,
            delta_up,
            ..stat(
                "SPENT THIS MONTH",
                f(snap.spent_this_month_minor),
                "Transfers and lending excluded".into(),
                "flat",
            )
        },
        stat(
            "SAVED THIS MONTH",
            f(snap.saved_this_month_minor()),
            // The rate as well as the figure: ₹23,637 saved means something
            // different on ₹40,000 of income than on ₹1,38,000.
            match saved_pct(snap) {
                Some(p) => format!("{p} of {} in", f(snap.income_this_month_minor)),
                None => format!("{} in", f(snap.income_this_month_minor)),
            },
            if snap.saved_this_month_minor() >= 0 { "ok" } else { "bad" },
        ),
        stat(
            "DEBT",
            f(snap.debt_minor),
            "Cards and loans".into(),
            if snap.debt_minor > 0 { "warn" } else { "flat" },
        ),
        // The one card on the strip that is not a figure off the ledger but a
        // reading of six of them, so it is the one card that opens: the total
        // alone would be a verdict with nothing behind it.
        FinStat {
            action: "health".into(),
            ..stat(
                "FINANCIAL HEALTH SCORE",
                match health.score {
                    Some(n) => format!("{n}"),
                    None => "—".to_string(),
                },
                match health.score {
                    Some(_) => format!("{} · out of 100", health.band()),
                    None => health.band().to_string(),
                },
                match health.score {
                    Some(n) if n >= 80 => "ok",
                    Some(n) if n >= 60 => "flat",
                    Some(n) if n >= 40 => "warn",
                    Some(_) => "bad",
                    None => "flat",
                },
            )
        },
    ]
}

fn health_parts(parts: &[HealthPart]) -> Vec<FinHealthPart> {
    parts
        .iter()
        .map(|p| FinHealthPart {
            label: p.label.clone(),
            points: if p.measured {
                format!("{} / {}", p.earned, p.possible)
            } else {
                "not measured".to_string()
            },
            pct: p.pct() as i32,
            detail: p.detail.clone(),
            measured: p.measured,
            tone: if !p.measured {
                "flat"
            } else if p.pct() >= 75 {
                "ok"
            } else if p.pct() >= 40 {
                "warn"
            } else {
                "bad"
            }
            .into(),
        })
        .collect()
}

/// The most categories the ring draws before folding the tail into one bucket.
/// A legibility limit — twenty arcs is a barcode — and a layout one: the card is
/// a fixed height so stepping a month does not make it jump.
const RING_CATEGORIES: usize = 6;

/// Category slices, each carrying where it starts and how much of the circle it
/// covers.
fn slices(rows: &[CategorySpend], base: &str) -> Vec<FinCatSlice> {
    let total: i64 = rows.iter().map(|r| r.base_minor).sum();
    if total <= 0 {
        return Vec::new();
    }
    // The tail, summed rather than dropped. `rows` arrives biggest first, so
    // this is the small change at the end of the list — but it is still money,
    // and a ring whose slices do not add up to its own centre figure is the kind
    // of quiet lie this section is not allowed to tell.
    let folded: Vec<CategorySpend> = if rows.len() > RING_CATEGORIES + 1 {
        let mut kept = rows[..RING_CATEGORIES].to_vec();
        kept.push(CategorySpend {
            category_id: None,
            name: format!("Other ({})", rows.len() - RING_CATEGORIES),
            color: None,
            base_minor: rows[RING_CATEGORIES..].iter().map(|r| r.base_minor).sum(),
        });
        kept
    } else {
        rows.to_vec()
    };
    // Shares that add up to 100. Truncating each one independently leaves the
    // ring short by up to a point per slice, which draws as a wedge of empty
    // track at the top of the donut that belongs to nothing. The shortfall is
    // handed back by largest remainder — the slices that lost the most to
    // rounding get it first — so the ring closes without any slice being off by
    // more than one point.
    let scaled: Vec<i128> = folded.iter().map(|r| r.base_minor as i128 * 100).collect();
    let mut pcts: Vec<i32> = scaled.iter().map(|v| (v / total as i128) as i32).collect();
    let mut short = 100 - pcts.iter().sum::<i32>();
    let mut order: Vec<usize> = (0..pcts.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(scaled[i] % total as i128));
    for &i in &order {
        if short <= 0 {
            break;
        }
        pcts[i] += 1;
        short -= 1;
    }

    let mut acc = 0i32;
    folded
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let pct = pcts[i];
            let out = FinCatSlice {
                name: r.name.clone(),
                amount: money::format_minor(r.base_minor, base),
                pct,
                start: acc,
                hue: category_hue(i, &r.name, r.color.as_deref()),
            };
            acc += pct;
            out
        })
        .collect()
}

/// Twelve-month bars, scaled to the heaviest month so the shape of the year
/// reads. Spending alone: with only spending drawn, scaling to income made
/// every bar a fraction of its height for no reason the chart still showed.
fn month_bars(rows: &[MonthTotal], base: &str) -> Vec<FinMonthBar> {
    let peak = rows.iter().map(|m| m.expense_minor).max().unwrap_or(0).max(1);
    rows.iter()
        .map(|m| FinMonthBar {
            label: month_short(&m.period),
            long: month_long(&m.period),
            expense: money::format_minor(m.expense_minor, base),
            expense_pct: ((m.expense_minor as i128 * 100) / peak as i128) as i32,
        })
        .collect()
}

fn txn_rows(rows: &[TxnRow], base: &str, show_running: bool) -> Vec<FinTxnRow> {
    rows.iter()
        .map(|t| FinTxnRow {
            id: t.id,
            date: t.occurred_on.clone(),
            // A transfer names both ends, because "Top-up" on its own does not
            // explain why one balance fell and another rose.
            description: match (&t.kind, &t.to_account_name) {
                (TxnKind::Transfer, Some(to)) => format!("{} → {}", t.description, to),
                _ => t.description.clone(),
            },
            category: t.category_name.clone().unwrap_or_default(),
            cat_hue: category_hue(0, t.category_name.as_deref().unwrap_or(""), None),
            account: t.account_name.clone(),
            source: source_label(&t.source).into(),
            amount: format!(
                "{}{}",
                match t.kind {
                    TxnKind::Income => "+",
                    TxnKind::Expense => "−",
                    TxnKind::Transfer => "",
                },
                // The original currency, not the base figure: the row should
                // match what the statement said.
                money::format_minor(t.amount_minor, &t.currency)
            ),
            running: if show_running {
                t.running_minor.map(|m| money::format_minor(m, base)).unwrap_or_default()
            } else {
                String::new()
            },
            kind: t.kind.as_str().into(),
        })
        .collect()
}

fn obligation_rows(rows: &[Obligation]) -> Vec<FinObligationRow> {
    rows.iter()
        .map(|o| FinObligationRow {
            id: o.id,
            name: o.name.clone(),
            kind: o.kind.clone().unwrap_or_else(|| "one-off".into()),
            due: o.due_on.clone(),
            // Zero when the date is unparseable, which matches no cell.
            day: o.due_on.rsplit('-').next().and_then(|d| d.parse::<i32>().ok()).unwrap_or(0),
            days: o.days_until,
            // The estimate keeps its tilde. A figure that is a guess must never
            // be readable as a measurement.
            estimate: o
                .estimate_minor
                .map(|m| money::format_estimate(m, &o.currency))
                .unwrap_or_default(),
            actual: o.actual_minor.map(|m| money::format_minor(m, &o.currency)).unwrap_or_default(),
            variance: o
                .variance_minor()
                .filter(|v| *v != 0)
                .map(|v| money::format_minor(v.abs(), &o.currency))
                .unwrap_or_default(),
            over: o.variance_minor().unwrap_or(0) > 0,
            status: o.status.as_str().into(),
            is_estimate: o.is_estimate(),
            category: o.category_name.clone().unwrap_or_default(),
            cat_hue: category_hue(0, o.category_name.as_deref().unwrap_or(""), None),
            account: o.account_name.clone().unwrap_or_default(),
            auto_post: o.auto_post,
        })
        .collect()
}

/// How many bills fall in each filter group. Of the whole window, never of the
/// filtered list: a chip showing the count of what it would leave after
/// filtering reads as zero for every group you are not in.
fn bill_counts(rows: &[Obligation]) -> (i32, i32, i32, i32, i32) {
    let (mut needs, mut auto, mut paid, mut oneoff) = (0, 0, 0, 0);
    for o in rows {
        if matches!(o.status, ObStatus::Paid) {
            paid += 1;
        } else if o.auto_post {
            auto += 1;
        } else if !matches!(o.status, ObStatus::Skipped) {
            // What is left is what someone has to do something about.
            needs += 1;
        }
        if o.recurrence_id.is_none() {
            oneoff += 1;
        }
    }
    (rows.len() as i32, needs, auto, paid, oneoff)
}

/// The subset a filter chip selects.
///
/// `oneoff` overlaps the others on purpose — a one-off bill is also either paid
/// or needing action — because "which of these did I enter by hand" is a
/// different question from "what do I have to do".
fn filter_bills(rows: &[Obligation], filter: &str) -> Vec<Obligation> {
    rows.iter()
        .filter(|o| match filter {
            "needs" => !o.auto_post && !matches!(o.status, ObStatus::Paid | ObStatus::Skipped),
            "auto" => o.auto_post && !matches!(o.status, ObStatus::Paid),
            "paid" => matches!(o.status, ObStatus::Paid),
            "oneoff" => o.recurrence_id.is_none(),
            // Unknown filter included: an empty list would look like a broken
            // tab, and the chip row can only ever send one of the five.
            _ => true,
        })
        .cloned()
        .collect()
}

fn recur_rows(rows: &[RecurRow], base: &str) -> Vec<FinRecurRow> {
    let today = date::today();
    rows.iter()
        .map(|r| FinRecurRow {
            id: r.id,
            name: r.name.clone(),
            kind: r.kind.as_str().into(),
            amount: match (r.amount_minor, r.estimate_minor) {
                (Some(a), _) => money::format_minor(a, &r.currency),
                (None, Some(e)) => money::format_estimate(e, &r.currency),
                // No fixed amount and no history to average: say so rather than
                // printing a zero that reads as free.
                (None, None) => "amount varies".to_string(),
            },
            is_estimate: r.is_estimate(),
            cycle: cycle_label(r.cycle).into(),
            next_due: r.next_due_on.clone().unwrap_or_default(),
            days: r
                .next_due_on
                .as_deref()
                .and_then(|d| date::parse(d).ok())
                .map(|d| date::days_between(today, d))
                .unwrap_or(0),
            next_short: r.next_due_on.as_deref().map(day_month).unwrap_or_default(),
            status: r.status.as_str().into(),
            account: r.account_name.clone().unwrap_or_default(),
            category: r.category_name.clone().unwrap_or_default(),
            badge: initial(&r.name),
            hue: stable_hue(&r.name),
            // What the service itself charges, under its own name. The stored
            // amount and currency are the truth; the base-currency columns
            // beside it are the derived figures.
            plan: match (r.note.as_deref(), r.shown_minor()) {
                (Some(n), _) if !n.trim().is_empty() => format!("{} · {}", n.trim(), r.currency),
                (_, Some(a)) => format!("{} · {}", money::format_minor(a, &r.currency), r.currency),
                (_, None) => "amount varies".to_string(),
            },
            // How far the price has risen since the oldest on record, as a whole
            // percent. 0 when it never has, which keeps the pill off every row
            // with nothing to report.
            hike_pct: match (r.hike_from_minor, r.shown_minor()) {
                (Some(from), Some(now)) if from > 0 && now > from => {
                    (((now - from) * 100 + from / 2) / from) as i32
                }
                _ => 0,
            },
            yearly: money::format_minor(r.yearly_minor, base),
            // A twelfth of the year, in the base currency: the point of the
            // column is comparing a yearly plan against a monthly one, and two
            // currencies in one column would not compare.
            monthly: money::format_minor(r.yearly_minor / 12, base),
            hike_from: r
                .hike_from_minor
                .map(|m| money::format_minor(m, &r.currency))
                .unwrap_or_default(),
            auto_post: r.auto_post,
            last_paid: r.last_paid_on.clone().unwrap_or_default(),
            cat_hue: category_hue(0, r.category_name.as_deref().unwrap_or(""), None),
            // Only when it differs from base. Printing "₹649 (₹649)" beside
            // every domestic row to make one foreign row consistent is noise.
            original: match r.amount_minor {
                Some(a) if r.currency != base => money::format_minor(a, &r.currency),
                _ => String::new(),
            },
        })
        .collect()
}

fn sub_categories(rows: &[RecurRow]) -> Vec<String> {
    let mut out: Vec<String> = rows.iter().filter_map(|r| r.category_name.clone()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn sub_currencies(rows: &[RecurRow]) -> Vec<String> {
    let mut out: Vec<String> = rows.iter().map(|r| r.currency.clone()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// `(all, active, paused, cancelled, removed)`. "All" is the tab you work on,
/// and a row you pressed the X on is not on it.
fn sub_counts(rows: &[RecurRow]) -> (i32, i32, i32, i32, i32) {
    let (mut active, mut paused, mut cancelled, mut removed) = (0, 0, 0, 0);
    for r in rows {
        match r.status {
            recur::Status::Active => active += 1,
            recur::Status::Paused => paused += 1,
            recur::Status::Cancelled => cancelled += 1,
            recur::Status::Removed => removed += 1,
        }
    }
    (active + paused + cancelled, active, paused, cancelled, removed)
}

fn filter_subs(
    rows: &[RecurRow],
    filter: &str,
    category: Option<&str>,
    currency: Option<&str>,
) -> Vec<RecurRow> {
    rows.iter()
        .filter(|r| match filter {
            "active" => matches!(r.status, recur::Status::Active),
            "paused" => matches!(r.status, recur::Status::Paused),
            "cancelled" => matches!(r.status, recur::Status::Cancelled),
            "removed" => matches!(r.status, recur::Status::Removed),
            // Removed is a holding pen, not a status you happen to be in: it is
            // absent from every other view, including "All".
            _ => !matches!(r.status, recur::Status::Removed),
        })
        .filter(|r| match category {
            Some(c) => r.category_name.as_deref() == Some(c),
            None => true,
        })
        .filter(|r| match currency {
            Some(c) => r.currency == c,
            None => true,
        })
        .cloned()
        .collect()
}

/// Sorts the rows, not the formatted cells: "₹1,299" and "₹999" sort the wrong
/// way round as text, and the next-charge column is a date whose displayed form
/// is "12 Aug". A missing next charge sorts last in both directions — "no date"
/// is not earlier than every date, it is absent.
fn sort_subs(rows: &mut [RecurRow], key: &str, desc: bool) {
    use std::cmp::Ordering;
    let ci = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
    rows.sort_by(|a, b| {
        let o = match key {
            "category" => ci(
                a.category_name.as_deref().unwrap_or(""),
                b.category_name.as_deref().unwrap_or(""),
            ),
            "cycle" => a.cycle.per_year().cmp(&b.cycle.per_year()),
            "monthly" | "yearly" => a.yearly_minor.cmp(&b.yearly_minor),
            "next" => match (a.next_due_on.as_deref(), b.next_due_on.as_deref()) {
                (Some(x), Some(y)) => x.cmp(y),
                // Absent sorts last whichever way the column points, so the
                // reversal below has to be undone for these two.
                (None, Some(_)) => {
                    if desc {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                (Some(_), None) => {
                    if desc {
                        Ordering::Greater
                    } else {
                        Ordering::Less
                    }
                }
                (None, None) => Ordering::Equal,
            },
            "account" => ci(
                a.account_name.as_deref().unwrap_or(""),
                b.account_name.as_deref().unwrap_or(""),
            ),
            "status" => a.status.as_str().cmp(b.status.as_str()),
            _ => ci(&a.name, &b.name),
        };
        let o = if desc { o.reverse() } else { o };
        // Name breaks every tie, so a re-sort never shuffles equal rows about.
        if o == Ordering::Equal { ci(&a.name, &b.name) } else { o }
    });
}

/// The four figures above the Subscriptions table. The last two are the argument
/// for the tab existing: what the whole set costs a year, and what the ones
/// nobody is using cost.
fn sub_stats(rows: &[RecurRow], base: &str, unused: &[String], unused_minor: i64) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let active: Vec<&RecurRow> =
        rows.iter().filter(|r| matches!(r.status, recur::Status::Active)).collect();
    let yearly: i64 = active.iter().map(|r| r.yearly_minor).sum();
    let paused: i64 = rows
        .iter()
        .filter(|r| matches!(r.status, recur::Status::Paused))
        .map(|r| r.yearly_minor)
        .sum();
    let today = date::today();
    let next = active
        .iter()
        .filter_map(|r| r.next_due_on.as_deref())
        .filter_map(|d| date::parse(d).ok())
        .filter(|d| *d >= today)
        .min();

    vec![
        stat(
            "A YEAR, ACTIVE",
            f(yearly),
            format!("{} subscription{}", active.len(), if active.len() == 1 { "" } else { "s" }),
            "flat",
        ),
        stat("A MONTH, ACTIVE", f(yearly / 12), "the recurring floor".into(), "flat"),
        stat(
            "NEXT CHARGE",
            match next {
                Some(d) => date::iso(d),
                None => "—".to_string(),
            },
            match next {
                Some(d) => {
                    let n = date::days_between(today, d);
                    if n == 0 {
                        "today".to_string()
                    } else {
                        format!("in {n} day{}", if n == 1 { "" } else { "s" })
                    }
                }
                None => "nothing scheduled".to_string(),
            },
            "flat",
        ),
        stat(
            "PAID, NOT USED",
            if unused.is_empty() { "—".to_string() } else { f(unused_minor) },
            // A count, not the names: joined, four flagged services ran to a
            // line several times longer than any other card's.
            if unused.is_empty() {
                if paused > 0 {
                    format!("{} a year is paused", f(paused))
                } else {
                    "nothing flagged".to_string()
                }
            } else {
                format!("{} subscription{}", unused.len(), if unused.len() == 1 { "" } else { "s" })
            },
            if unused.is_empty() { "flat" } else { "warn" },
        ),
    ]
}

/// The four figures above the Bills table.
///
/// Derived from the rows the tab is already showing rather than from four more
/// queries: they have to agree with the list underneath them, and the surest way
/// to make two numbers agree is for them to be the same number.
fn bill_stats(
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

    // What leaves every month whatever the user does: the bills on the calendar
    // plus a twelfth of the subscriptions. The one figure on this tab about the
    // shape of someone's finances rather than about this month's list.
    let floor = due_total + subs_yearly_minor / 12;
    let share = if income_minor > 0 {
        format!(" · {}% of income", (floor as i128 * 100 / income_minor as i128).min(999))
    } else {
        String::new()
    };

    vec![
        stat(
            "OVERDUE",
            f(overdue_total),
            if overdue.is_empty() {
                "Nothing late".to_string()
            } else {
                format!(
                    "{} bill{} · {latest} day{} late",
                    overdue.len(),
                    if overdue.len() == 1 { "" } else { "s" },
                    if latest == 1 { "" } else { "s" }
                )
            },
            if overdue.is_empty() { "flat" } else { "bad" },
        ),
        stat(
            "DUE THIS MONTH",
            f(due_total),
            format!(
                "{} bill{} · {needs_confirming} need{} confirming",
                due.len(),
                if due.len() == 1 { "" } else { "s" },
                if needs_confirming == 1 { "s" } else { "" }
            ),
            "flat",
        ),
        stat(
            "AUTO-DEBITED",
            f(auto_total),
            format!(
                "{} bill{} · no action needed",
                auto.len(),
                if auto.len() == 1 { "" } else { "s" }
            ),
            "flat",
        ),
        stat("FIXED MONTHLY FLOOR", f(floor), format!("bills + subscriptions{share}"), "flat"),
    ]
}

fn due_rows(rows: &[Due]) -> Vec<FinDueRow> {
    rows.iter()
        .map(|d| FinDueRow {
            id: d.id,
            person: d.person.clone(),
            direction: d.direction.as_str().into(),
            amount: money::format_minor(d.amount_minor, &d.currency),
            original: money::format_minor(d.original_minor(), &d.currency),
            settled: money::format_minor(d.settled_minor, &d.currency),
            opened: d.opened_on.clone(),
            age_days: d.age_days,
            status: d.status.as_str().into(),
            note: d.note.clone().unwrap_or_default(),
            part_paid: d.settled_minor > 0 && d.status == dues::Status::Open,
        })
        .collect()
}

/// `(all, to_me, i_owe, closed)`.
fn due_counts(rows: &[Due]) -> (i32, i32, i32, i32) {
    let (mut to_me, mut i_owe, mut closed) = (0, 0, 0);
    for d in rows {
        if !matches!(d.status, dues::Status::Open) {
            closed += 1;
        } else if d.direction == dues::Direction::OwedToMe {
            to_me += 1;
        } else {
            i_owe += 1;
        }
    }
    (rows.len() as i32, to_me, i_owe, closed)
}

fn filter_dues(rows: &[Due], filter: &str) -> Vec<Due> {
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

/// The four figures above the Lending table.
///
/// The net position is shown *and* labelled as a net, because the two directions
/// are never subtracted anywhere else in this section — but "am I up or down on
/// money between friends" is the question the tab exists to answer, and refusing
/// to answer it while showing both halves would be pedantry rather than honesty.
fn due_stats(
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
        stat(
            "LENT OUT",
            f(totals.owed_to_me_minor),
            format!(
                "{} open",
                open.iter().filter(|d| d.direction == dues::Direction::OwedToMe).count()
            ),
            if totals.owed_to_me_minor > 0 { "ok" } else { "flat" },
        ),
        stat(
            "BORROWED",
            f(totals.i_owe_minor),
            format!(
                "{} open",
                open.iter().filter(|d| d.direction == dues::Direction::IOwe).count()
            ),
            if totals.i_owe_minor > 0 { "warn" } else { "flat" },
        ),
        stat(
            "NET POSITION",
            format!("{}{}", if net < 0 { "−" } else { "" }, f(net.abs())),
            if net == 0 {
                "square with everyone".to_string()
            } else if net > 0 {
                "in your favour".to_string()
            } else {
                "against you".to_string()
            },
            if net < 0 {
                "bad"
            } else if net > 0 {
                "ok"
            } else {
                "flat"
            },
        ),
        stat(
            "CAME BACK, 6 MONTHS",
            f(settled.owed_to_me_minor),
            if stale > 0 {
                format!("{stale} open {} months+", oldest / 30)
            } else if oldest > 0 {
                format!("oldest is {oldest} days old")
            } else {
                "nothing outstanding".to_string()
            },
            if stale > 0 { "warn" } else { "flat" },
        ),
    ]
}

/// The Accounts grid.
///
/// Two kinds never appear as a plain card. A loan has its own block underneath,
/// with the amortisation and the prepayment calculator its terms are actually
/// edited from. And the two virtual holding accounts are two halves of one fact,
/// so they arrive as one card carrying both figures.
fn account_rows(rows: &[AccountRow], details: &[AccountDetail], base: &str) -> Vec<FinAccountRow> {
    let f = |m: i64| money::format_minor(m, base);
    let mut hues = HuePicker::default();
    let blank = || FinAccountRow {
        id: 0,
        name: String::new(),
        kind: String::new(),
        badge: String::new(),
        hue: 0,
        balance: String::new(),
        sub: String::new(),
        util_pct: 0,
        negative: false,
        closed: false,
        txn_count: 0,
        reconciled: String::new(),
        in_month: String::new(),
        out_month: String::new(),
        limit_note: String::new(),
        statement: String::new(),
        minimum_due: String::new(),
        drift: String::new(),
        drift_bad: false,
        topped_from: String::new(),
        last_topup: String::new(),
        lent: String::new(),
        borrowed: String::new(),
    };
    let mut out: Vec<FinAccountRow> = rows
        .iter()
        .filter(|a| !matches!(a.kind, AccountKind::Loan | AccountKind::Virtual))
        .map(|a| {
            let d = details.iter().find(|d| d.account_id == a.id);
            let drift = d.and_then(|d| d.drift_minor).filter(|m| *m != 0);
            FinAccountRow {
                id: a.id,
                name: a.name.clone(),
                kind: a.kind.as_str().into(),
                badge: badge(&a.name, a.kind, base),
                hue: hues.pick(&a.name),
                // Foreign-currency accounts still report in base: balances are
                // summed from `base_minor`, so labelling one with its own symbol
                // would claim a precision the derivation does not have.
                balance: money::format_minor(a.balance_minor, base),
                sub: match a.utilisation_pct() {
                    Some(pct) => format!("{} · {pct}% of limit", kind_label(a.kind.as_str())),
                    None if a.currency != base => {
                        format!("{} · {}", kind_label(a.kind.as_str()), a.currency)
                    }
                    None => kind_label(a.kind.as_str()).to_string(),
                },
                util_pct: a.utilisation_pct().unwrap_or(0) as i32,
                negative: a.balance_minor < 0,
                closed: a.closed,
                txn_count: a.txn_count,
                // "Never checked" rather than a blank: an unverified balance is a
                // fact about the account, and the blank reads as "fine".
                reconciled: match (&a.reconciled_on, a.kind.as_str()) {
                    (_, "virtual") | (_, "loan") => String::new(),
                    (Some(on), _) => format!("Checked {}", day_month(on)),
                    (None, _) => "Never checked".to_string(),
                },
                in_month: d
                    .filter(|d| d.in_month_minor > 0)
                    .map(|d| f(d.in_month_minor))
                    .unwrap_or_default(),
                out_month: d
                    .filter(|d| d.out_month_minor > 0)
                    .map(|d| f(d.out_month_minor))
                    .unwrap_or_default(),
                limit_note: match (a.credit_limit_minor, a.utilisation_pct()) {
                    (Some(limit), Some(pct)) => format!("{pct}% of a {} limit", f(limit)),
                    _ => String::new(),
                },
                statement: d
                    .and_then(|d| d.statement_on.as_deref())
                    .map(day_month)
                    .unwrap_or_default(),
                minimum_due: d.and_then(|d| d.minimum_due_minor).map(f).unwrap_or_default(),
                drift: drift.map(|m| f(m.abs())).unwrap_or_default(),
                // Money missing is the case worth colouring. Finding more cash
                // than the ledger expected is a nice surprise, not a problem.
                drift_bad: drift.is_some_and(|m| m < 0),
                topped_from: d.and_then(|d| d.topped_from.clone()).unwrap_or_default(),
                last_topup: match d {
                    Some(d) => match (d.last_topup_minor, &d.last_topup_on) {
                        (Some(m), Some(on)) => format!("{} on {}", f(m), day_month(on)),
                        _ => String::new(),
                    },
                    None => String::new(),
                },
                ..blank()
            }
        })
        .collect();

    // Lent out and Borrowed, as one card. Neither is an account anything can be
    // done to on its own — they are the two directions of the same outstanding
    // balance. Kept at kind `virtual` so every guard that already refuses to
    // edit, delete or reconcile a holding account keeps refusing.
    let held = |name: &str| rows.iter().find(|a| a.kind == AccountKind::Virtual && a.name == name);
    if let (Some(lent), Some(borrowed)) =
        (held(tulipix_finances::schema::LENT_OUT), held(tulipix_finances::schema::BORROWED))
    {
        let net = lent.balance_minor + borrowed.balance_minor;
        out.push(FinAccountRow {
            name: "Lending".into(),
            kind: "virtual".into(),
            balance: f(net),
            sub: "Holding accounts · net".into(),
            badge: "L".into(),
            hue: hues.pick("Lending"),
            negative: net < 0,
            lent: f(lent.balance_minor),
            // Stored as a negative balance, shown as what is owed.
            borrowed: f(borrowed.balance_minor.abs()),
            ..blank()
        });
    }
    out
}

/// Bank, card, loans and lending as five cards, in the order the design states
/// them. Always five, never folded: a strip that drops a card on a month with no
/// card debt is a strip that changes width, and the tab reads as a different
/// tab. Zero is a fact worth printing here.
///
/// Deliberately stops at liquid and debt. There is no net-worth card here or
/// anywhere: it would need the market value of things the app can only ask the
/// user to guess at, and a guess printed beside four real figures reads as a
/// fifth real figure.
fn account_stats(rows: &[AccountRow], dues_net_minor: i64, base: &str) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let sum = |pred: fn(&AccountRow) -> bool| -> i64 {
        rows.iter().filter(|a| !a.closed).filter(|a| pred(a)).map(|a| a.balance_minor).sum()
    };
    let open = |pred: fn(&AccountRow) -> bool| -> usize {
        rows.iter().filter(|a| !a.closed).filter(|a| pred(a)).count()
    };
    let cash = sum(|a| a.kind.is_liquid());
    let cards = sum(|a| a.kind.as_str() == "card");
    let loan_total = sum(|a| a.kind.as_str() == "loan");
    let n = |c: usize, one: &str, many: &str| {
        if c == 1 { format!("1 {one}") } else { format!("{c} {many}") }
    };
    // Each card carries an `acct:` action, so the strip is also the tab's
    // filter — the figure and the rows it was added up from are one control.

    vec![
        FinStat {
            action: "acct:all".into(),
            ..stat(
                "LIQUID",
                f(cash + cards),
                "everything on this tab".into(),
                if cash + cards > 0 { "ok" } else { "flat" },
            )
        },
        FinStat {
            action: "acct:cash".into(),
            ..stat(
                "BANK, CASH & WALLETS",
                f(cash),
                n(open(|a| a.kind.is_liquid()), "account", "accounts"),
                "flat",
            )
        },
        FinStat {
            action: "acct:cards".into(),
            ..stat(
                "CARD OUTSTANDING",
                f(cards),
                n(open(|a| a.kind.as_str() == "card"), "card", "cards"),
                if cards != 0 { "bad" } else { "flat" },
            )
        },
        FinStat {
            action: "acct:loans".into(),
            ..stat(
                "LOANS OUTSTANDING",
                f(loan_total),
                n(open(|a| a.kind.as_str() == "loan"), "loan", "loans"),
                if loan_total != 0 { "bad" } else { "flat" },
            )
        },
        FinStat {
            action: "acct:lending".into(),
            ..stat(
                "LENDING, NET",
                f(dues_net_minor),
                if dues_net_minor > 0 {
                    "owed to you"
                } else if dues_net_minor < 0 {
                    "you owe"
                } else {
                    "nothing outstanding"
                }
                .into(),
                if dues_net_minor < 0 {
                    "bad"
                } else if dues_net_minor > 0 {
                    "ok"
                } else {
                    "flat"
                },
            )
        },
    ]
}

fn loan_rows(rows: &[LoanRow], today: NaiveDate) -> Vec<FinLoanRow> {
    // Its own picker, not the accounts grid's: the two are read separately, and
    // sharing one would exhaust the palette twice as fast.
    let mut hues = HuePicker::default();
    rows.iter()
        .map(|l| {
            // The remaining schedule, from what is outstanding now. Its first row
            // is the split the card shows and its last is when the loan actually
            // closes — both move as it is paid down, so neither can be read off
            // the original terms.
            let rest = loans::schedule(
                l.balance_minor.max(0),
                l.rate_bp,
                l.remaining_months(),
                l.emi_minor,
                today,
                l.emi_day,
            );
            let next = rest.first();
            let interest_left: i64 = rest.iter().map(|i| i.interest_minor).sum();
            FinLoanRow {
                account_id: l.account_id,
                name: l.name.clone(),
                badge: badge(&l.name, AccountKind::Loan, &l.currency),
                hue: hues.pick(&l.name),
                balance: money::format_minor(l.balance_minor, &l.currency),
                principal: money::format_minor(l.principal_minor, &l.currency),
                emi: money::format_minor(l.emi_minor, &l.currency),
                // Basis points back to a percentage: 840 → "8.4%".
                rate: format!("{}.{}%", l.rate_bp / 100, (l.rate_bp % 100) / 10),
                progress_pct: l.progress_pct() as i32,
                remaining_months: l.remaining_months(),
                tenure_months: l.tenure_months,
                next_due: next.map(|i| i.due_on.clone()).unwrap_or_else(|| l.started_on.clone()),
                emi_principal: next
                    .map(|i| money::format_minor(i.principal_minor, &l.currency))
                    .unwrap_or_default(),
                emi_interest: next
                    .map(|i| money::format_minor(i.interest_minor, &l.currency))
                    .unwrap_or_default(),
                closes: rest.last().map(|i| month_year(&i.due_on)).unwrap_or_default(),
                interest_left: money::format_minor(interest_left, &l.currency),
                // A no-cost EMI has nothing to warn about, and colouring a zero
                // red would make the cheapest loan on the page look worst.
                interest_free: interest_left == 0,
            }
        })
        .collect()
}

fn schedule_rows(rows: &[Instalment], currency: &str) -> Vec<FinInstalment> {
    rows.iter()
        .map(|i| FinInstalment {
            n: i.n,
            due: i.due_on.clone(),
            emi: money::format_minor(i.emi_minor, currency),
            principal: money::format_minor(i.principal_minor, currency),
            interest: money::format_minor(i.interest_minor, currency),
            balance: money::format_minor(i.balance_minor, currency),
            interest_pct: if i.emi_minor > 0 {
                ((i.interest_minor as i128 * 100) / i.emi_minor as i128) as i32
            } else {
                0
            },
        })
        .collect()
}

// ── envelopes ───────────────────────────────────────────────────────────────

fn budget_rows(rows: &[BudgetRow], base: &str, elapsed: i64, total: i64) -> Vec<FinBudgetRow> {
    rows.iter()
        .map(|b| FinBudgetRow {
            category_id: b.category_id,
            name: b.category_name.clone(),
            budget: money::format_minor(b.amount_minor, base),
            spent: money::format_minor(b.spent_minor, base),
            remaining: money::format_minor(b.remaining_minor().abs(), base),
            used_pct: b.used_pct() as i32,
            projected: money::format_minor(b.projected_minor(elapsed, total), base),
            projected_pct: if b.allowance_minor() > 0 {
                ((b.projected_minor(elapsed, total) as i128 * 100) / b.allowance_minor() as i128)
                    as i32
            } else {
                0
            },
            over: b.over_budget(),
            off_pace: b.off_pace(elapsed, total),
            rollover: b.rollover,
            carried: if b.rollover_minor > 0 {
                money::format_minor(b.rollover_minor, base)
            } else {
                String::new()
            },
            unset: b.id.is_none(),
            detail: envelope_detail(b, base),
            over_by: if b.over_budget() {
                format!(
                    "over by {}",
                    money::format_minor(b.spent_minor - b.allowance_minor(), base)
                )
            } else {
                String::new()
            },
        })
        .collect()
}

/// "14 transactions · biggest ₹1,240 Toit".
///
/// The count is what separates an envelope broken by one purchase from one
/// broken by a habit, and only one of those is worth changing the envelope over.
fn envelope_detail(b: &BudgetRow, base: &str) -> String {
    if b.txn_count == 0 {
        return String::new();
    }
    let plural = if b.txn_count == 1 { "transaction" } else { "transactions" };
    let mut out = format!("{} {plural}", b.txn_count);
    // Only when one posting is a real share of the envelope. On forty even
    // spends the largest is not the story, and naming it implies it is.
    if b.txn_count > 1 && b.biggest_minor * 3 >= b.spent_minor {
        out.push_str(&format!(" · biggest {}", money::format_minor(b.biggest_minor, base)));
        if !b.biggest_name.is_empty() {
            out.push(' ');
            out.push_str(b.biggest_name.trim());
        }
    }
    out
}

/// Everything that leaves the month whatever the user decides, itemised.
///
/// Returns the rows, their total, and the subscription share of that total on
/// its own — the month statement splits bills from subscriptions, and deriving
/// the split a second time is how two screens end up disagreeing by one EMI.
fn fixed_items(bills: &[Obligation], subs: &[RecurRow], base: &str) -> (Vec<FinKv>, i64, i64) {
    let mut rows: Vec<(String, i64)> = bills
        .iter()
        .filter(|o| !matches!(o.status, ObStatus::Skipped))
        .filter_map(|o| o.shown_minor().map(|m| (o.name.clone(), m)))
        .collect();

    // Subscriptions as one line rather than eleven: individually they are noise
    // next to an EMI, and together they are one of the larger numbers here.
    let subs_monthly: i64 = subs
        .iter()
        .filter(|r| r.status == recur::Status::Active)
        .map(|r| r.yearly_minor / 12)
        .sum();
    if subs_monthly > 0 {
        rows.push((format!("Subscriptions · {}", subs.len()), subs_monthly));
    }
    rows.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
    let total: i64 = rows.iter().map(|(_, m)| m).sum();

    (
        rows.into_iter()
            .map(|(label, m)| FinKv {
                label,
                value: money::format_minor(m, base),
                tone: "flat".into(),
                strong: false,
            })
            .collect(),
        total,
        subs_monthly,
    )
}

/// The month as money in, money committed, money planned, money left.
///
/// The other statement in this section walks the *bank balance* — what is there
/// now, what is still to leave it, where it ends — which answers a different
/// question: a balance carries every month before it, so a good month can close
/// low and a bad one high. This one starts at what came in and takes off what
/// has a claim on it. Nothing here is a forecast.
fn month_statement(
    income_minor: i64,
    bills_minor: i64,
    subs_minor: i64,
    envelopes_minor: i64,
    base: &str,
) -> Vec<FinKv> {
    let f = |m: i64| money::format_minor(m, base);
    let left = income_minor - bills_minor - subs_minor - envelopes_minor;
    let line = |label: &str, value: String, tone: &str, strong: bool| FinKv {
        label: label.into(),
        value,
        tone: tone.into(),
        strong,
    };
    let mut rows = vec![line(
        "Money in this month",
        f(income_minor),
        if income_minor > 0 { "ok" } else { "flat" },
        true,
    )];
    // A line for nothing is a line that has to be read to be dismissed. Each of
    // the three appears only when it is a claim on the month.
    if bills_minor > 0 {
        rows.push(line("Bills, loans and EMIs", f(-bills_minor), "flat", false));
    }
    if subs_minor > 0 {
        rows.push(line("Subscriptions", f(-subs_minor), "flat", false));
    }
    if envelopes_minor > 0 {
        rows.push(line("Set aside in envelopes", f(-envelopes_minor), "flat", false));
    }
    rows.push(line(
        if left < 0 { "Over-committed by" } else { "Free to use" },
        f(left.abs()),
        if left < 0 { "bad" } else { "ok" },
        true,
    ));
    rows
}

/// The envelope-by-month grid.
///
/// A month the envelope did not exist in is an em dash, never a zero: "no
/// envelope" and "spent nothing" are different claims and the grid must not
/// merge them into a run of good months.
fn envelope_history(rows: &[budgets::EnvelopeHistory], base: &str) -> Vec<FinEnvelopeHistory> {
    rows.iter()
        .map(|h| FinEnvelopeHistory {
            name: h.name.clone(),
            cells: h
                .spent
                .iter()
                .map(|c| match c {
                    Some(m) => money::format_minor(*m, base),
                    None => "—".to_string(),
                })
                .collect(),
            // Over is decided against the average, not against each month's own
            // envelope: the row is a habit, and one bad month inside six is not
            // one.
            over: h
                .spent
                .iter()
                .map(|c| c.is_some_and(|m| m > 0 && h.avg_delta_minor > 0))
                .collect(),
            avg_delta: format!(
                "{}{}",
                if h.avg_delta_minor > 0 { "+" } else { "−" },
                money::format_minor(h.avg_delta_minor.abs(), base)
            ),
            avg_over: h.avg_delta_minor > 0,
        })
        .collect()
}

fn period_labels(periods: &[String]) -> Vec<String> {
    periods
        .iter()
        .map(|p| {
            p.split('-')
                .nth(1)
                .and_then(|m| m.parse::<usize>().ok())
                .filter(|m| (1..=12).contains(m))
                .map(|m| MONTHS[m - 1].to_string())
                .unwrap_or_else(|| p.clone())
        })
        .collect()
}

fn discipline_rows(rows: &[MonthDiscipline], base: &str) -> Vec<FinDiscipline> {
    rows.iter()
        .map(|m| FinDiscipline {
            period: month_long(&m.period),
            budget: money::format_minor(m.budget_minor, base),
            spent: money::format_minor(m.spent_minor, base),
            within: m.within(),
            pct: if m.budget_minor > 0 {
                ((m.spent_minor as i128 * 100) / m.budget_minor as i128) as i32
            } else {
                0
            },
        })
        .collect()
}

/// Income-allocation figures for the envelope header.
fn allocation(a: &budgets::Allocation, base: &str) -> (String, String, String, i32) {
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

/// Where the month's income is committed before anything discretionary.
///
/// Percentages are of income, so they only mean anything when there is income —
/// with none this returns nothing rather than showing shares of a total it
/// invented.
fn commitments(
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
        name: name.into(),
        amount: money::format_minor(m, base),
        pct: ((m as i128 * 100) / income_minor as i128) as i32,
        // A bar list rather than a ring: nothing draws an arc from these, so
        // there is no angle for `start` to be the start of.
        start: 0,
        hue: argb,
    })
    .collect()
}

// ── calendar ────────────────────────────────────────────────────────────────

/// A six-by-seven month grid, Monday first, with each day's dated obligations
/// folded in.
///
/// Always 42 cells, whether or not the last seven are in the month: the grid is
/// addressed by index, and a variable-length model would make every cell's row
/// depend on how long February is.
fn calendar(
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

            // A dot per thing due, coloured by its category, biggest first. Four
            // slots: the point is "a loan and two bills", which is legible at a
            // glance in a small cell, and a fifth dot is not.
            let mut pips: Vec<u32> = {
                let mut with_amount: Vec<&&Obligation> = on_day.iter().collect();
                with_amount.sort_by_key(|o| std::cmp::Reverse(o.shown_minor().unwrap_or(0)));
                with_amount
                    .iter()
                    .enumerate()
                    .map(|(i, o)| category_hue(i, o.category_name.as_deref().unwrap_or(""), None))
                    .collect()
            };
            if earned > 0 {
                // Income leads: it is the one dot whose colour means something
                // different from all the others.
                pips.insert(0, category_hue(0, "income", None));
            }
            pips.truncate(4);

            FinCalDay {
                day: if in_month { day as i32 } else { 0 },
                in_month,
                is_today: in_month && iso == date::iso(today),
                count: (on_day.len() + credits) as i32,
                amount: if sum > 0 { money::format_minor(sum, base) } else { String::new() },
                income: if earned > 0 {
                    money::format_minor(earned, base)
                } else {
                    String::new()
                },
                money_out: sum > 0,
                overdue: on_day.iter().any(|o| o.status.as_str() == "overdue"),
                pips,
            }
        })
        .collect()
}

/// Which cell the agenda opens on: today when the month on screen is the one
/// today falls in, otherwise that month's first day — stepping to March and
/// being shown the 4th of it because today happens to be the 4th of August
/// would be a coincidence dressed as a selection.
fn calendar_focus(days: &[FinCalDay]) -> i32 {
    days.iter()
        .position(|d| d.is_today)
        .or_else(|| days.iter().position(|d| d.in_month))
        .map(|i| i as i32)
        .unwrap_or(-1)
}

/// Twelve months at a glance. Bars are shares of the busiest month rather than
/// of income: the question this view answers is which months are heavy, and
/// scaling to income would flatten every month of a year where income is steady.
fn calendar_year(
    months: &[(String, Vec<Obligation>, i64)],
    current: &str,
    base: &str,
) -> Vec<FinCalMonth> {
    let totals: Vec<i64> = months
        .iter()
        .map(|(_, items, _)| items.iter().filter_map(|o| o.shown_minor()).sum())
        .collect();
    let peak = totals.iter().copied().max().unwrap_or(0).max(1);

    months
        .iter()
        .zip(totals)
        .map(|((period, items, income), out)| FinCalMonth {
            label: month_long(period),
            out: if out > 0 { money::format_minor(out, base) } else { String::new() },
            income: if *income > 0 { money::format_minor(*income, base) } else { String::new() },
            pct: ((out as i128 * 100) / peak as i128) as i32,
            current: period == current,
            count: items.len() as i32,
        })
        .collect()
}

/// The cash-flow walk.
///
/// Every row is something that has actually been recorded. Expected income is
/// deliberately not projected forward: a salary that has not landed is not
/// money, and a closing balance built on one would be a forecast wearing a
/// ledger's clothes.
fn cash_flow(
    opening_minor: i64,
    items: &[Obligation],
    income_minor: i64,
    low: Option<&insights::LowPoint>,
    base: &str,
) -> Vec<FinKv> {
    let f = |m: i64| money::format_minor(m, base);
    let out: i64 =
        items.iter().filter(|o| o.status.is_open()).filter_map(|o| o.shown_minor()).sum();
    let line = |label: String, value: String, tone: &str, strong: bool| FinKv {
        label,
        value,
        tone: tone.into(),
        strong,
    };

    let mut rows = vec![line("In the bank now".into(), f(opening_minor), "flat", false)];
    if out > 0 {
        rows.push(line("Still to leave this month".into(), f(-out), "bad", false));
    }
    if let Some(l) = low {
        rows.push(line(
            format!("Low point · {}", day_month(&l.on)),
            f(l.balance_minor),
            if l.balance_minor < 0 { "bad" } else { "warn" },
            false,
        ));
    }
    if income_minor > 0 {
        rows.push(line("In this month".into(), f(income_minor), "ok", false));
    }
    rows.push(line(
        "Where it closes".into(),
        f(opening_minor - out),
        if opening_minor - out < 0 { "bad" } else { "flat" },
        true,
    ));
    rows
}

/// Whether the month is covered, and its worst day.
///
/// Returns `(warning, heaviest_amount, heaviest_label, heaviest_detail)`. The
/// warning is empty when nothing is short — the card then says so itself, rather
/// than this inventing a reassuring sentence.
fn calendar_cards(
    low: Option<&insights::LowPoint>,
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

    // The single day with the most leaving it. A month can be affordable in
    // total and still fail on the 5th.
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
    match by_day.into_iter().max_by_key(|(_, t, _)| *t) {
        Some((day, total, n)) => (
            warning,
            f(total),
            day.to_string(),
            format!("{n} thing{} due that day", if n == 1 { "" } else { "s" }),
        ),
        None => (warning, String::new(), String::new(), "Nothing dated this month.".to_string()),
    }
}

// ── insights ────────────────────────────────────────────────────────────────

/// One flag. Its own function because three lists are built from the same rows —
/// Insights shows all of them, Planning splits them into what is going wrong and
/// what could be done better — and three copies of this mapping would be three
/// chances for the same flag to offer a different button in a different place.
fn flag(f: &Flag) -> FinFlag {
    use insights::Action;
    // The action is encoded as `kind:id`, which the dispatcher splits. A string
    // rather than a pair of fields because two fields could disagree.
    let (label, action) = match &f.action {
        Action::None => ("", String::new()),
        Action::Obligation(i) => ("Open bill", format!("obligation:{i}")),
        Action::Recurrence(i) => ("Open", format!("recurrence:{i}")),
        Action::Category(i) => ("Open budget", format!("category:{i}")),
        Action::Account(i) => ("Open account", format!("account:{i}")),
        Action::Due(i) => ("Open it in Lending", format!("due:{i}")),
        Action::Rates => ("Set a rate", "rates:0".to_string()),
    };
    FinFlag {
        severity: match f.severity {
            Severity::Bad => "bad",
            Severity::Warn => "warn",
            Severity::Info => "info",
            Severity::Good => "good",
        }
        .into(),
        title: f.title.clone(),
        detail: f.detail.clone(),
        action_label: label.into(),
        action,
    }
}

/// The four figures above the Insights list.
fn insight_stats(
    snap: &Snapshot,
    flags: &[Flag],
    subs_yearly_minor: i64,
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    let bad = flags.iter().filter(|x| x.severity == Severity::Bad).count();
    let warn = flags.iter().filter(|x| x.severity == Severity::Warn).count();
    let saved = snap.saved_this_month_minor();

    vec![
        stat(
            "WORTH ACTING ON",
            format!("{}", bad + warn),
            if bad > 0 {
                format!("{bad} urgent, {warn} to watch")
            } else if warn > 0 {
                format!("{warn} to watch, nothing urgent")
            } else {
                "nothing needs you".to_string()
            },
            if bad > 0 {
                "bad"
            } else if warn > 0 {
                "warn"
            } else {
                "ok"
            },
        ),
        stat(
            "SAVINGS RATE",
            saved_pct(snap).unwrap_or_else(|| "—".to_string()),
            if snap.income_this_month_minor > 0 {
                format!("{} of {}", f(saved), f(snap.income_this_month_minor))
            } else {
                "no income recorded this month".to_string()
            },
            if snap.income_this_month_minor == 0 {
                "flat"
            } else if saved < 0 {
                "bad"
            } else {
                "ok"
            },
        ),
        // Not a forecast of the future: an arithmetic statement about the present
        // rate, which is why it is labelled "if nothing changes".
        stat(
            "IF NOTHING CHANGES",
            f(saved * 12),
            "a year at this month's rate".into(),
            if saved < 0 { "bad" } else { "ok" },
        ),
        stat(
            "SUBSCRIPTIONS",
            f(subs_yearly_minor),
            format!("{} a month, committed", f(subs_yearly_minor / 12)),
            "flat",
        ),
    ]
}

/// The savings-rate bars, oldest left. A month with no income is drawn as a gap,
/// not as a zero: the salary landing on the 1st instead of the 31st is a
/// calendar accident, and a bar at the floor would report it as the worst month
/// on record.
fn savings_months(rows: &[MonthSavings], base: &str) -> Vec<FinSavingsMonth> {
    let last = rows.last().map(|m| m.period.clone()).unwrap_or_default();
    rows.iter()
        .map(|m| FinSavingsMonth {
            label: month_short(&m.period),
            // Negative rates clamp to zero for the bar's height only; the figure
            // beside it still says what actually happened.
            pct: m.rate_pct.unwrap_or(0).clamp(0, 100) as i32,
            known: m.rate_pct.is_some(),
            current: m.period == last,
            amount: match m.rate_pct {
                Some(p) => format!("{p}% · {}", money::format_minor(m.saved_minor, base)),
                None => "no income".to_string(),
            },
        })
        .collect()
}

/// This month, the average, the best and the worst — under the bars.
fn savings_summary(rows: &[MonthSavings], base: &str) -> Vec<FinKv> {
    let known: Vec<&MonthSavings> = rows.iter().filter(|m| m.rate_pct.is_some()).collect();
    if known.is_empty() {
        return Vec::new();
    }
    let kv = |label: String, m: &MonthSavings, tone: &str| FinKv {
        label,
        value: format!(
            "{}% · {}",
            m.rate_pct.unwrap_or(0),
            money::format_minor(m.saved_minor, base)
        ),
        tone: tone.into(),
        strong: false,
    };
    let mean = known.iter().filter_map(|m| m.rate_pct).sum::<i64>() / known.len() as i64;
    let best = known.iter().max_by_key(|m| m.rate_pct).copied();
    let worst = known.iter().min_by_key(|m| m.rate_pct).copied();

    let mut out = Vec::new();
    if let Some(now) = known.last().copied() {
        out.push(kv(month_long(&now.period), now, "flat"));
    }
    out.push(FinKv {
        label: format!("{}-month average", known.len()),
        value: format!("{mean}%"),
        tone: "flat".into(),
        strong: false,
    });
    // Only when there is more than one month; "best and worst" over a single
    // month is the same month twice.
    if known.len() > 1 {
        if let Some(b) = best {
            out.push(kv(format!("Best · {}", month_long(&b.period)), b, "ok"));
        }
        if let Some(w) = worst {
            out.push(kv(format!("Worst · {}", month_long(&w.period)), w, "warn"));
        }
    }
    out
}

/// "If nothing changes" — twelve months out, from what has actually happened.
fn projection(p: &Projection, base: &str) -> (Vec<FinKv>, String) {
    if p.months_of_history == 0 {
        return (Vec::new(), String::new());
    }
    let f = |m: i64| money::format_minor(m, base);
    let line = |label: &str, value: String, tone: &str, strong: bool| FinKv {
        label: label.into(),
        value,
        tone: tone.into(),
        strong,
    };
    let mut rows = vec![
        line("Spend, next 12 months", f(p.spend_12m_minor), "flat", false),
        line(
            "Saved, next 12 months",
            f(p.saved_12m_minor),
            if p.saved_12m_minor < 0 { "bad" } else { "ok" },
            false,
        ),
    ];
    if p.debt_cleared_12m_minor > 0 {
        rows.push(line(
            "Debt cleared, next 12 months",
            f(p.debt_cleared_12m_minor),
            "flat",
            false,
        ));
    }
    if p.actionable_monthly_minor > 0 {
        rows.push(line(
            "Acting on every flag above",
            format!("{} a month", f(p.actionable_monthly_minor)),
            "ok",
            true,
        ));
    }

    // The honesty line. Two months of history cannot forecast a year, and the
    // figures above are still worth showing — with that said out loud rather
    // than left for the reader to work out.
    let note = match (p.months_of_history, p.rate_now_pct, p.rate_after_pct) {
        (n, _, _) if n < 3 => format!(
            "Built from {n} month{} of history, which is not enough to forecast a year. \
             Treat it as arithmetic, not a plan.",
            if n == 1 { "" } else { "s" }
        ),
        (_, Some(now), Some(after)) if after > now => format!(
            "Acting on the flags above is {} a year, and takes the savings rate from \
             {now}% to {after}%.",
            f(p.actionable_monthly_minor * 12)
        ),
        _ => String::new(),
    };
    (rows, note)
}

/// The same six months as one donut each. `slices` already folds the tail into
/// "Other" and closes the ring to a hundred, so the six agree with each other
/// and with the Overview's ring, which is built by the same function.
fn trend_pies(months: &[(String, Vec<CategorySpend>)], base: &str) -> Vec<FinTrendPie> {
    months
        .iter()
        .map(|(period, rows)| {
            let total: i64 = rows.iter().map(|r| r.base_minor).sum();
            FinTrendPie {
                label: month_short(period),
                long: month_long(period),
                total: money::format_minor(total, base),
                slices: slices(rows, base),
            }
        })
        .collect()
}

/// The six-month chart, one series per metric. All four travel and the selector
/// picks between them: they come off one query that has already run, and a round
/// trip to redraw six bars would be slower than the click that asked for it.
fn trend_series(rows: &[MonthSavings], base: &str) -> Vec<FinTrendSeries> {
    let last = rows.last().map(|m| m.period.clone()).unwrap_or_default();
    // Every money series is a share of its own largest month. Sharing one scale
    // across income and spending would flatten spending in any month that is not
    // a disaster, which is most of them.
    let series = |name: &str, pick: fn(&MonthSavings) -> Option<i64>, is_money: bool| {
        let peak = rows.iter().filter_map(pick).map(|v| v.abs()).max().unwrap_or(0).max(1);
        let months: Vec<FinSavingsMonth> = rows
            .iter()
            .map(|m| {
                let v = pick(m);
                FinSavingsMonth {
                    label: month_short(&m.period),
                    pct: match v {
                        Some(v) if is_money => ((v.abs() as i128 * 100) / peak as i128) as i32,
                        Some(v) => v.clamp(0, 100) as i32,
                        None => 0,
                    },
                    known: v.is_some(),
                    current: m.period == last,
                    amount: match v {
                        Some(v) if is_money => money::format_minor(v, base),
                        Some(v) => format!("{v}%"),
                        None => String::new(),
                    },
                }
            })
            .collect();
        FinTrendSeries { name: name.into(), months }
    };

    vec![
        series("Savings rate", |m| m.rate_pct, false),
        series("Saved", |m| Some(m.saved_minor), true),
        series("Spending", |m| Some(m.spent_minor), true),
        series("Income", |m| Some(m.income_minor), true),
    ]
}

// ── planning ────────────────────────────────────────────────────────────────
//
// Planning answers one question the other tabs do not: what should be done next.
// Every figure below is a decision, a prediction or an action — never another
// way of stating a total that is already on Overview.

/// The four cards Planning opens with. Cash, then where the month closes, then
/// what is being kept, then the verdict on all three. The status card reads the
/// other cards, so it comes after them.
fn plan_health(
    liquid_minor: i64,
    closing_minor: i64,
    warning: &str,
    savings_pct: Option<i64>,
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    // "Tight" is not a warning — nothing is going to bounce. It is the month
    // taking most of what is in the bank, which is worth seeing before it
    // becomes the warning.
    let tight = warning.is_empty() && closing_minor >= 0 && closing_minor * 5 < liquid_minor;
    let (status, status_sub, status_tone) = if !warning.is_empty() {
        ("At risk", warning.to_string(), "bad")
    } else if tight {
        (
            "Tight",
            "What is dated this month takes most of what is in the bank.".to_string(),
            "warn",
        )
    } else {
        ("Covered", "Everything dated this month is covered.".to_string(), "ok")
    };

    vec![
        stat("CASH AVAILABLE", f(liquid_minor), "In bank now".into(), "flat"),
        stat(
            "PROJECTED CLOSING",
            f(closing_minor),
            "If nothing changes".into(),
            if closing_minor < 0 { "bad" } else { "flat" },
        ),
        stat(
            "SAVINGS RATE",
            match savings_pct {
                Some(p) => format!("{p}%"),
                None => "—".to_string(),
            },
            match savings_pct {
                Some(_) => "of what came in last month",
                None => "No income recorded to measure against",
            }
            .into(),
            match savings_pct {
                Some(p) if p >= 20 => "ok",
                Some(p) if p < 0 => "bad",
                Some(_) => "warn",
                None => "flat",
            },
        ),
        // Last, not second. The three before it are the figures the status is a
        // reading of, and a verdict is easier to trust after the numbers it was
        // taken from than before them.
        stat("MONTH STATUS", status.into(), status_sub, status_tone),
    ]
}

/// When the month's money moves, one card per day that something happens on.
fn timeline(
    days: &[FinCalDay],
    items: &[Obligation],
    income: &[(String, i64)],
    closing_minor: i64,
    base: &str,
) -> Vec<FinTimelineEvent> {
    // Keyed by ISO date, so the row comes out in date order without a sort and a
    // salary landing on a bill's due date becomes one card rather than two.
    #[derive(Default)]
    struct Day {
        owed: i64,
        earned: i64,
        names: Vec<String>,
        kinds: Vec<String>,
    }
    let mut by_day: BTreeMap<String, Day> = BTreeMap::new();
    for o in items.iter().filter(|o| !matches!(o.status, ObStatus::Skipped)) {
        let Some(m) = o.shown_minor() else { continue };
        let e = by_day.entry(o.due_on.clone()).or_default();
        e.owed += m;
        e.names.push(o.name.clone());
        e.kinds.push(o.kind.clone().unwrap_or_default());
    }
    for (on, m) in income.iter().filter(|(_, m)| *m > 0) {
        by_day.entry(on.clone()).or_default().earned += *m;
    }

    let day_of =
        |iso: &str| iso.rsplit('-').next().and_then(|d| d.parse::<i32>().ok()).unwrap_or(0);
    let idx_of = |day: i32| {
        days.iter().position(|d| d.in_month && d.day == day).map(|i| i as i32).unwrap_or(-1)
    };

    let mut rows: Vec<FinTimelineEvent> = by_day
        .iter()
        .map(|(iso, d)| {
            let day = day_of(iso);
            // Income only when nothing is also owed that day. A day with both is
            // still a day money leaves, and colouring it green would say the
            // opposite of what it is.
            let is_income = d.owed == 0 && d.earned > 0;
            let subs = d.kinds.iter().filter(|k| *k == "subscription").count();
            FinTimelineEvent {
                day,
                idx: idx_of(day),
                name: if is_income {
                    "Money in".to_string()
                } else if d.names.len() == 1 {
                    d.names[0].clone()
                } else {
                    format!("{} due", d.names.len())
                },
                amount: if is_income {
                    money::format_minor(d.earned, base)
                } else {
                    money::format_minor(-d.owed, base)
                },
                sub: if d.earned > 0 && d.owed > 0 {
                    format!("and {} in", money::format_minor(d.earned, base))
                } else if d.names.len() > 1 {
                    d.names.join(", ")
                } else {
                    String::new()
                },
                hue: if is_income {
                    TIMELINE_INCOME
                } else if subs * 2 > d.kinds.len() {
                    TIMELINE_SUB
                } else if d.kinds.iter().any(|k| k == "bill") {
                    TIMELINE_BILL
                } else {
                    TIMELINE_OTHER
                },
                income: is_income,
                closing: false,
                count: (d.names.len() + usize::from(d.earned > 0)) as i32,
            }
        })
        .collect();

    // The month has to end somewhere, and the row is the only place on the page
    // that reads left to right in time.
    let last = days.iter().filter(|d| d.in_month).map(|d| d.day).max().unwrap_or(0);
    if last > 0 {
        rows.push(FinTimelineEvent {
            day: last,
            idx: idx_of(last),
            name: "Closing balance".into(),
            amount: money::format_minor(closing_minor, base),
            sub: "if nothing else moves".into(),
            hue: if closing_minor < 0 { 0xFFef4444 } else { TIMELINE_INCOME },
            income: closing_minor >= 0,
            closing: true,
            count: 0,
        });
    }
    rows
}

/// What is going wrong, from both places that know.
///
/// One list because they are one question — what bites this week — and two lists
/// side by side would leave the reader to merge them by eye. Auto-posting
/// obligations are left out: they need no one's attention, which is the entire
/// meaning of the flag.
fn attention(needs: &[Obligation], flags: &[Flag], base: &str) -> Vec<FinFlag> {
    let mut rows: Vec<FinFlag> = needs
        .iter()
        .filter(|o| !o.auto_post)
        .filter(|o| o.status.is_open())
        .filter(|o| o.days_until <= 7)
        .map(|o| {
            let overdue = o.status.as_str() == "overdue";
            let amount = o.shown_minor().map(|m| money::format_minor(m, base)).unwrap_or_default();
            FinFlag {
                severity: if overdue { "bad" } else { "warn" }.into(),
                title: if overdue {
                    format!("{} overdue", o.name)
                } else if o.days_until == 0 {
                    format!("{} due today", o.name)
                } else if o.days_until == 1 {
                    format!("{} due tomorrow", o.name)
                } else {
                    format!("{} due in {} days", o.name, o.days_until)
                },
                detail: if amount.is_empty() {
                    format!("Due on {}", day_month(&o.due_on))
                } else if overdue {
                    format!("{amount} was due on {}", day_month(&o.due_on))
                } else {
                    format!("{amount} on {}", day_month(&o.due_on))
                },
                action_label: "Open bill".into(),
                action: format!("obligation:{}", o.id),
            }
        })
        .collect();
    rows.extend(
        flags.iter().filter(|f| matches!(f.severity, Severity::Bad | Severity::Warn)).map(flag),
    );
    rows
}

/// The other half of the flags: the ones that are not going wrong, only worth
/// doing something about.
fn recommendations(rows: &[Flag]) -> Vec<FinFlag> {
    rows.iter().filter(|f| matches!(f.severity, Severity::Info | Severity::Good)).map(flag).collect()
}

/// When each envelope runs out, at the pace it is going.
///
/// Only envelopes that have been spent from and have a limit: a prediction off
/// zero days of spending is a division, and one for a category with no envelope
/// has nothing to run out of.
fn predictions(
    rows: &[BudgetRow],
    daily: &[(i64, String, i64)],
    first: NaiveDate,
    elapsed: i64,
    days_in: i64,
    base: &str,
) -> Vec<FinPrediction> {
    if elapsed <= 0 || days_in <= 0 {
        return Vec::new();
    }
    let mut out: Vec<(i64, FinPrediction)> = rows
        .iter()
        .filter(|b| b.id.is_some() && b.spent_minor > 0 && b.allowance_minor() > 0)
        .enumerate()
        .map(|(i, b)| {
            // Whole days throughout: the answer is a date, and carrying a
            // fraction of a rupee a day through it changes nothing about which
            // day it lands on.
            let rate = (b.spent_minor / elapsed).max(1);
            let left = b.remaining_minor().max(0);
            let finish_day = elapsed + left / rate;
            let soon = finish_day <= days_in;
            let on = first + chrono::Duration::days(finish_day - 1);

            // Daily spend for this envelope's own category, laid on the month's
            // axis and scaled to its own worst day.
            let mut points: Vec<i64> = vec![0; days_in as usize];
            for (_, iso, minor) in daily.iter().filter(|(id, _, _)| *id == b.category_id) {
                let day = iso.rsplit('-').next().and_then(|d| d.parse::<usize>().ok());
                if let Some(d) = day.filter(|d| *d >= 1 && *d <= days_in as usize) {
                    points[d - 1] += *minor;
                }
            }
            let peak = points.iter().copied().max().unwrap_or(0).max(1);
            let points: Vec<i32> =
                points.iter().map(|p| ((*p as i128 * 100) / peak as i128) as i32).collect();

            (
                finish_day,
                FinPrediction {
                    name: b.category_name.clone(),
                    finish: if b.over_budget() {
                        "Already spent".to_string()
                    } else {
                        format!("{} {}", day_month(&date::iso(on)), on.year())
                    },
                    note: if b.over_budget() {
                        format!(
                            "{} over the envelope",
                            money::format_minor(b.spent_minor - b.allowance_minor(), base)
                        )
                    } else if soon {
                        format!(
                            "{} left at {} a day",
                            money::format_minor(left, base),
                            money::format_minor(rate, base)
                        )
                    } else {
                        format!("{} left — lasts the month", money::format_minor(left, base))
                    },
                    hue: category_hue(i, &b.category_name, b.color.as_deref()),
                    soon: soon || b.over_budget(),
                    points,
                },
            )
        })
        .collect();
    // Soonest first: the card that matters is the envelope about to run out, and
    // it must not be the fourth one along because its category sorts late.
    out.sort_by_key(|(day, _)| *day);
    out.into_iter().map(|(_, p)| p).collect()
}

/// The four habits.
///
/// Every one is a count of something the app already recorded. Nothing here is
/// scored, ranked or graded: a number the user can check against their own
/// ledger is worth more than a mark out of ten they cannot.
fn habits(
    items: &[Obligation],
    disc: &[MonthDiscipline],
    subs: &[RecurRow],
    unbudgeted_minor: i64,
    unbudgeted_n: usize,
    base: &str,
) -> Vec<FinHabit> {
    let paid = items.iter().filter(|o| o.status.as_str() == "paid").count();
    let missed = items.iter().filter(|o| o.status.as_str() == "overdue").count();
    let settled = paid + missed;
    let paid_pct = if settled > 0 { (paid * 100 / settled) as i32 } else { 0 };

    let within = disc.iter().filter(|m| m.within()).count();
    let measured = disc.iter().filter(|m| m.budget_minor > 0).count();
    let within_pct = if measured > 0 { (within * 100 / measured) as i32 } else { 0 };

    let risen = subs.iter().filter(|r| r.hike_from_minor.is_some()).count();

    vec![
        FinHabit {
            label: "Bills paid on time".into(),
            value: if settled == 0 { "—".to_string() } else { format!("{paid_pct}%") },
            pct: paid_pct,
            hue: if paid_pct >= 90 { 0xFF16a34a } else { 0xFFf59e0b },
            bar: settled > 0,
            action_label: String::new(),
            action: String::new(),
        },
        FinHabit {
            label: "Stayed in budget".into(),
            value: if measured == 0 { "—".to_string() } else { format!("{within_pct}%") },
            pct: within_pct,
            hue: if within_pct >= 70 { 0xFF16a34a } else { 0xFFf59e0b },
            bar: measured > 0,
            action_label: String::new(),
            action: String::new(),
        },
        FinHabit {
            label: "Subscriptions that have risen".into(),
            value: risen.to_string(),
            pct: 0,
            hue: TIMELINE_SUB,
            bar: false,
            // A habit's action names one of Planning's own drill-downs, not a
            // tab: the count is the summary and the list behind it is the
            // detail, and sending the reader to another tab to see four rows
            // would cost them the page they were reading.
            action_label: if risen > 0 { "View details" } else { "" }.into(),
            action: if risen > 0 { "subs" } else { "" }.into(),
        },
        FinHabit {
            label: "Spending no envelope watches".into(),
            value: if unbudgeted_minor > 0 {
                money::format_minor(unbudgeted_minor, base)
            } else {
                "None".to_string()
            },
            pct: 0,
            hue: if unbudgeted_minor > 0 { 0xFFf472b6 } else { 0xFF16a34a },
            bar: false,
            action_label: if unbudgeted_n > 0 { "Set envelopes" } else { "" }.into(),
            action: if unbudgeted_n > 0 { "budgets" } else { "" }.into(),
        },
    ]
}

/// The month read backwards: which envelope held, which did not, and the two
/// days the money moved most.
fn monthly_review(
    rows: &[BudgetRow],
    daily: &[(i64, String, i64)],
    income: &[(String, i64)],
    base: &str,
) -> Vec<FinStat> {
    let f = |m: i64| money::format_minor(m, base);
    // Only envelopes that were actually set. A category with no limit cannot be
    // the best-kept one — nothing was kept.
    let set: Vec<&BudgetRow> =
        rows.iter().filter(|b| b.id.is_some() && b.allowance_minor() > 0).collect();
    let best = set.iter().filter(|b| !b.over_budget()).min_by_key(|b| b.used_pct());
    let worst = set.iter().max_by_key(|b| b.used_pct());

    // Days, summed across every category. `daily` is expense only, so this is
    // spending rather than movement.
    let mut by_day: BTreeMap<&str, i64> = BTreeMap::new();
    for (_, on, minor) in daily {
        *by_day.entry(on.as_str()).or_default() += *minor;
    }
    let heaviest = by_day.into_iter().max_by_key(|(_, m)| *m);
    let richest = income.iter().filter(|(_, m)| *m > 0).max_by_key(|(_, m)| *m);

    vec![
        stat(
            "BEST CATEGORY",
            best.map(|b| b.category_name.clone()).unwrap_or_else(|| "—".into()),
            match best {
                Some(b) => format!("{}% of its envelope spent", b.used_pct()),
                None => "No envelope came in under this month.".into(),
            },
            if best.is_some() { "ok" } else { "flat" },
        ),
        stat(
            "WORST CATEGORY",
            worst.map(|b| b.category_name.clone()).unwrap_or_else(|| "—".into()),
            match worst {
                Some(b) if b.over_budget() => {
                    format!("over by {}", f(b.spent_minor - b.allowance_minor()))
                }
                Some(b) => format!("{}% of its envelope spent", b.used_pct()),
                None => "No envelope is set this month.".into(),
            },
            match worst {
                Some(b) if b.over_budget() => "bad",
                Some(_) => "warn",
                None => "flat",
            },
        ),
        stat(
            "HEAVIEST SPEND DAY",
            heaviest.map(|(on, _)| day_month(on)).unwrap_or_else(|| "—".into()),
            match heaviest {
                Some((_, m)) => f(m),
                None => "Nothing spent this month.".into(),
            },
            "flat",
        ),
        stat(
            "BIGGEST INCOME DAY",
            richest.map(|(on, _)| day_month(on)).unwrap_or_else(|| "—".into()),
            match richest {
                Some((_, m)) => f(*m),
                None => "Nothing came in this month.".into(),
            },
            "flat",
        ),
    ]
}

// ── rates, import, detection ────────────────────────────────────────────────

/// Millionths as a decimal, to `dp` places. Integer arithmetic throughout: this
/// is the number every converted total is multiplied by, and a float here would
/// round differently from the one the posting used.
fn micro_str(micro: i64, dp: u32) -> String {
    let scale = 10i64.pow(dp);
    let scaled = (micro as i128 * scale as i128 + 500_000) / 1_000_000;
    let whole = scaled as i64 / scale;
    let frac = (scaled as i64 % scale).abs();
    format!("{whole}.{frac:0width$}", width = dp as usize)
}

fn rate_rows(rows: &[fx::Monitored], base: &str) -> Vec<FinRate> {
    rows.iter()
        .map(|r| FinRate {
            code: r.code.clone(),
            name: r.name.clone(),
            rate: match r.rate_micro {
                // Four places: two is not enough for a currency worth less than
                // a hundredth of the base, and the yen is exactly that.
                Some(m) => format!("1 {} = {} {}", r.code, micro_str(m, 4), base),
                None => "not fetched yet".to_string(),
            },
            // Every rate is stored against the base, so the reverse is the
            // reciprocal — and with all of them held, any pair converts through
            // it.
            inverse: match r.rate_micro {
                Some(m) if m > 0 => {
                    let back = (1_000_000i128 * 1_000_000i128 / m as i128) as i64;
                    format!("1 {base} = {} {}", micro_str(back, 6), r.code)
                }
                _ => String::new(),
            },
            // Where it came from and when, because a converted total is only as
            // trustworthy as the age of the rate behind it.
            edited: match (
                r.rate_micro,
                chrono::DateTime::<chrono::Utc>::from_timestamp(r.edited_at, 0),
            ) {
                (Some(_), Some(dt)) => format!(
                    "{} {}",
                    if r.live { "fetched" } else { "typed" },
                    dt.with_timezone(&chrono::Local).date_naive()
                ),
                _ => String::new(),
            },
            live: r.live,
            known: r.rate_micro.is_some(),
        })
        .collect()
}

/// Which column of the file became which field.
///
/// Shown in the import sheet because the mapping is *guessed*. Getting debit and
/// credit the wrong way round inverts every row in a statement and leaves all
/// the totals looking plausible, so the guess has to be visible before anything
/// posts. Given the file's own headers each row is a dropdown the user can
/// correct; without them (an OFX, which states its own structure) they fall back
/// to read-only text.
fn import_map(map: &import::csv::ColumnMap, headers: &[String]) -> Vec<FinField> {
    let options: Vec<String> =
        std::iter::once(NO_COLUMN.to_string()).chain(headers.iter().cloned()).collect();
    let editable = !headers.is_empty();
    let row = |key: &str, label: &str, value: Option<&str>| FinField {
        key: key.into(),
        label: label.into(),
        kind: if editable { "dropdown" } else { "static" }.into(),
        value: value.filter(|v| !v.is_empty()).unwrap_or(NO_COLUMN).into(),
        hint: String::new(),
        options: options.clone(),
        required: false,
    };
    let mut out = vec![
        row("map_date", "Date", Some(&map.date)),
        row("map_description", "Description", Some(&map.description)),
    ];
    // Either one signed column or a debit/credit pair — never both, and which it
    // is tells the user something about their bank's export.
    match (&map.amount, &map.debit, &map.credit) {
        (Some(a), _, _) => out.push(row("map_amount", "Amount (signed)", Some(a))),
        (None, d, c) => {
            out.push(row("map_debit", "Money out", d.as_deref()));
            out.push(row("map_credit", "Money in", c.as_deref()));
        }
    }
    // Worth mapping even though it is not a transaction field: it lets the
    // import reconcile the account in the same pass, so a missing entry surfaces
    // on import day rather than at month end.
    out.push(row("map_balance", "Running balance", map.balance.as_deref()));
    out
}

/// Rebuild a column map from what the mapping dropdowns now say.
///
/// Keeps the shape the file was read as: a statement with a debit/credit pair
/// stays a pair. A key the form has never carried leaves its column alone; a key
/// explicitly set to [`NO_COLUMN`] clears it. Those are different intentions and
/// collapsing them would drop the whole mapping the second time this runs.
fn column_map_from(
    original: &import::csv::ColumnMap,
    get: impl Fn(&str) -> Option<String>,
) -> import::csv::ColumnMap {
    // `None` = untouched, `Some(None)` = cleared, `Some(Some(v))` = chosen.
    let pick = |key: &str| -> Option<Option<String>> {
        get(key).map(|v| (!v.is_empty() && v != NO_COLUMN).then_some(v))
    };
    let mut m = original.clone();
    if let Some(Some(v)) = pick("map_date") {
        m.date = v;
    }
    if let Some(Some(v)) = pick("map_description") {
        m.description = v;
    }
    if original.amount.is_some() {
        if let Some(v) = pick("map_amount") {
            m.amount = v;
        }
    } else {
        if let Some(v) = pick("map_debit") {
            m.debit = v;
        }
        if let Some(v) = pick("map_credit") {
            m.credit = v;
        }
    }
    if let Some(v) = pick("map_balance") {
        m.balance = v;
    }
    m
}

fn import_rows(rows: &[RawRow], currency: &str, limit: usize) -> Vec<FinImportRow> {
    rows.iter()
        .take(limit)
        .map(|r| FinImportRow {
            date: r.occurred_on.clone(),
            description: r.description.clone(),
            amount: format!(
                "{}{}",
                if r.is_credit { "+" } else { "−" },
                money::format_minor(r.amount_minor, currency)
            ),
            credit: r.is_credit,
        })
        .collect()
}

fn proposal_rows(rows: &[detect::Proposal]) -> Vec<FinProposal> {
    use detect::Confidence;
    rows.iter()
        .enumerate()
        .map(|(i, p)| FinProposal {
            // The index into the cached list, not a database id — nothing has
            // been created yet.
            idx: i as i32,
            name: p.label.clone(),
            kind: p.kind.as_str().into(),
            amount: match p.amount_minor {
                Some(a) => money::format_minor(a, &p.currency),
                None => money::format_estimate(p.typical_minor, &p.currency),
            },
            cycle: cycle_label(p.cycle).into(),
            next_due: p.next_due_on.clone(),
            occurrences: p.occurrences as i32,
            confidence: match p.confidence {
                Confidence::High => "high",
                Confidence::Low => "low",
            }
            .into(),
        })
        .collect()
}

fn to_fields(fields: &[Field]) -> Vec<FinField> {
    fields
        .iter()
        .map(|f| FinField {
            key: f.key.clone(),
            label: f.label.clone(),
            kind: f.kind.clone(),
            value: f.value.clone(),
            hint: f.hint.clone(),
            options: f.options.clone(),
            required: f.required,
        })
        .collect()
}

fn price_rows(rows: &[Price]) -> Vec<FinPrice> {
    rows.iter()
        .map(|p| FinPrice {
            amount: money::format_minor(p.amount_minor, &p.currency),
            from: p.effective_from.clone(),
        })
        .collect()
}

// ------------------------------------------------------------------ sheets ---

/// Build a form and put it on screen.
///
/// Five kinds are not plain forms: read-only lists, or ones whose content is
/// cached state rather than a database row. They are handled first and return
/// early; everything else goes through `fin_sheet::build`.
async fn open_sheet(kind: &str, id: i64) -> Result<()> {
    let pool = finances_pool().await?;
    {
        let mut s = session();
        s.sheet_kind = kind.to_string();
        s.sheet_id = id;
        s.sheet_error.clear();
        s.sheet_note.clear();
        s.sheet_busy = true;
    }

    match kind {
        "rates" => {
            let base = fx::base_currency();
            let rows = fx::monitor(pool, &base).await.unwrap_or_default();
            let rates = rate_rows(&rows, &base);
            let checked = fx::last_checked(pool)
                .await
                .ok()
                .flatten()
                .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0))
                .map(|dt| dt.with_timezone(&chrono::Local).format("%d %b %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            let fields = vec![
                Field::new("base", "Base currency", "text").v(base.clone()).hint(
                    "Every total is reported in this. Changing it re-reports, and rewrites \
                     nothing.",
                ).req(),
                Field::new("code", "Currency", "text").hint("Three-letter code, e.g. USD."),
                Field::new("rate", "One unit is worth", "money").hint("e.g. 83.60"),
            ];
            let mut s = session();
            s.sheet_title = "Exchange rates".into();
            s.sheet_hint = format!(
                "Everything is reported in {base}. Checked once a day — last checked {checked}."
            );
            s.sheet_primary = "Add or update".into();
            s.rates = rates;
            s.form.clear();
            for f in &fields {
                s.form.insert(f.key.clone(), f.value.clone());
            }
            s.fields = fields;
            s.sheet_busy = false;
            return Ok(());
        }
        "prices" => {
            let rows = recur::price_history(pool, id).await.unwrap_or_default();
            let name =
                recur::get(pool, id).await.ok().flatten().map(|r| r.name).unwrap_or_default();
            let mut s = session();
            s.sheet_title = format!("{name} — what it has cost");
            s.sheet_hint = "Recorded every time the amount changed. This is the only reason a \
                            quiet price rise is visible at all."
                .into();
            // Nothing to submit: this is a record, not a form.
            s.sheet_primary.clear();
            s.prices = price_rows(&rows);
            s.fields.clear();
            s.sheet_busy = false;
            return Ok(());
        }
        "schedule" => {
            let rows = loans::schedule_for(pool, id).await.unwrap_or_default();
            let loan = loans::get(pool, id).await.ok().flatten();
            let currency = loan.as_ref().map(|l| l.currency.clone()).unwrap_or_else(|| "INR".into());
            let name = loan.as_ref().map(|l| l.name.clone()).unwrap_or_default();
            let interest = loans::total_interest(&rows);
            let mut s = session();
            s.sheet_title = format!("{name} — amortisation");
            s.sheet_hint = format!(
                "{} of interest over the whole term. The last instalment is trimmed to clear \
                 the balance exactly.",
                money::format_minor(interest, &currency)
            );
            s.sheet_primary.clear();
            s.schedule = rows;
            s.schedule_currency = currency;
            s.schedule_page = 0;
            s.fields.clear();
            s.sheet_busy = false;
            return Ok(());
        }
        "prepay" => {
            let Some(l) = loans::get(pool, id).await.ok().flatten() else {
                let mut s = session();
                s.sheet_error = "that loan is gone".into();
                s.sheet_busy = false;
                return Ok(());
            };
            let fields = vec![
                Field::new("extra", "Lump sum", "money")
                    .hint("Paid off the principal, keeping the EMI the same.")
                    .req(),
                Field::new("after_n", "After how many instalments", "number")
                    .v(l.paid_count.to_string())
                    .req(),
            ];
            let mut s = session();
            s.sheet_title = format!("{} — what if I prepay?", l.name);
            s.sheet_hint = "Shortens the term rather than reducing the EMI, which is what \
                            saves interest."
                .into();
            s.sheet_primary = "Work it out".into();
            s.form.clear();
            for f in &fields {
                s.form.insert(f.key.clone(), f.value.clone());
            }
            s.fields = fields;
            s.sheet_busy = false;
            return Ok(());
        }
        "detect" => {
            let found = detect::propose(pool).await.unwrap_or_default();
            let n = found.len();
            let mut s = session();
            s.sheet_title = "Charges that look like they repeat".into();
            s.sheet_hint = if n == 0 {
                "Nothing in the ledger repeats predictably yet. Import a few months of \
                 statements and this gets much better."
                    .into()
            } else {
                String::new()
            };
            s.sheet_primary.clear();
            s.proposals = found;
            s.fields.clear();
            s.sheet_busy = false;
            return Ok(());
        }
        _ => {}
    }

    let period = session().budget_period.clone();
    match fin_sheet::build(pool, kind, id).await {
        Ok(mut built) => {
            let mut s = session();
            // Anything a caller asked to pre-fill wins over the defaults, and is
            // applied to the fields themselves so the user can see and correct
            // it.
            let prefill = std::mem::take(&mut s.prefill);
            for f in built.fields.iter_mut() {
                let Some(v) = prefill.get(&f.key) else { continue };
                // A caller with an id but no name asks for `#7`. Resolved against
                // the field's own options here, because a value that is not one
                // of them shows in the box and cannot be selected back once the
                // user opens it.
                f.value = match v.strip_prefix('#').and_then(|i| i.parse::<i64>().ok()) {
                    Some(id) => f
                        .options
                        .iter()
                        .find(|o| fin_sheet::id_from_label(o) == Some(id))
                        .cloned()
                        .unwrap_or_else(|| f.value.clone()),
                    None => v.clone(),
                };
            }
            s.form.clear();
            for f in &built.fields {
                s.form.insert(f.key.clone(), f.value.clone());
            }
            // Values with no field of their own: the period a budget is keyed on,
            // and the provenance of an OCR'd receipt.
            s.form.insert("period_hidden".into(), period);
            for (k, v) in prefill.iter().filter(|(k, _)| k.ends_with("_hint")) {
                s.form.insert(k.clone(), v.clone());
            }
            s.sheet_title = built.title;
            s.sheet_hint = built.hint;
            s.sheet_primary = built.primary;
            s.fields = built.fields;
            s.sheet_busy = false;
        }
        Err(e) => {
            let mut s = session();
            s.sheet_error = e.to_string();
            s.sheet_busy = false;
        }
    }
    Ok(())
}

fn close_sheet() {
    let mut s = session();
    s.sheet_kind.clear();
    s.sheet_id = 0;
    s.sheet_title.clear();
    s.sheet_hint.clear();
    s.sheet_primary.clear();
    s.sheet_error.clear();
    s.sheet_note.clear();
    s.sheet_busy = false;
    s.form.clear();
    s.fields.clear();
    s.prices.clear();
    s.schedule.clear();
    s.schedule_page = 0;
    s.import = None;
    s.import_rows.clear();
    s.import_map.clear();
    s.import_summary.clear();
    s.import_needs_format = false;
    s.proposals.clear();
    s.rates.clear();
}

/// Submit the open sheet. `again` reopens a fresh one of the same kind instead
/// of closing, which is what makes entering a morning's worth of cash spends
/// bearable.
async fn submit_sheet(again: bool) -> Result<()> {
    let pool = finances_pool().await?;
    let (kind, id, form, import_state) = {
        let mut s = session();
        s.sheet_busy = true;
        s.sheet_error.clear();
        // Cloned, not taken: an import whose date order is still unsettled has to
        // survive the round trip so the file can be re-read rather than
        // re-picked.
        (s.sheet_kind.clone(), s.sheet_id, s.form.clone(), s.import.clone())
    };

    // Sheets whose submit is not `fin_sheet::submit`.
    let result: Result<Option<String>> = match kind.as_str() {
        "rates" => submit_rates(pool, &form).await,
        "prepay" => submit_prepay(pool, id, &form).await,
        "import" => match import_state {
            None => Err(anyhow::anyhow!("that file is no longer loaded")),
            Some(state) => return submit_import(pool, state, &form, again).await,
        },
        _ => fin_sheet::submit(pool, &kind, id, &form).await.map(|_| None),
    };

    match result {
        // A result to show: the sheet stays open with the answer in it. That is
        // the whole point of the prepayment sheet.
        Ok(Some(message)) => {
            let stays_open = kind == "prepay";
            // The guard is confined to its own block rather than dropped by
            // hand on one branch: a `MutexGuard` that is merely *in scope*
            // across an `.await` makes the whole future non-`Send`, whether or
            // not the path taken released it first. frb's dispatcher requires
            // `Send`, and the error lands in generated code with no line of its
            // own — so the rule is kept by shape, not by care.
            {
                let mut s = session();
                s.sheet_busy = false;
                if stays_open {
                    s.sheet_hint = message;
                }
            }
            if !stays_open {
                if again {
                    reopen(&kind).await?;
                } else {
                    close_sheet();
                }
            }
        }
        Ok(None) => {
            session().sheet_busy = false;
            if again {
                reopen(&kind).await?;
            } else {
                close_sheet();
            }
        }
        // Shown on screen, not logged: the user typed this and is the only one
        // who can fix it.
        Err(e) => {
            let mut s = session();
            s.sheet_busy = false;
            s.sheet_error = e.to_string();
        }
    }
    Ok(())
}

/// Blank sheet of the same kind, for "Save & add another". Id 0 means "new",
/// which is what makes it a fresh form rather than an edit of what was just
/// saved.
async fn reopen(kind: &str) -> Result<()> {
    close_sheet();
    open_sheet(kind, 0).await
}

/// The rates sheet does two jobs: pick the base currency, and add or update one
/// rate. Either alone is a valid submit, so a blank code with a changed base is
/// not an error.
async fn submit_rates(pool: &sqlx::SqlitePool, form: &HashMap<String, String>) -> Result<Option<String>> {
    let base_now = fx::base_currency();
    let want_base = form.get("base").cloned().unwrap_or_default().trim().to_uppercase();
    let base_changed = !want_base.is_empty() && want_base != base_now;

    let code = form.get("code").cloned().unwrap_or_default().trim().to_uppercase();
    let raw = form.get("rate").cloned().unwrap_or_default();
    let want_rate = !code.is_empty() || !raw.trim().is_empty();

    let mut outcome: Result<Option<String>> = Ok(None);
    if base_changed {
        outcome = fx::set_base_currency(&want_base).map(|_| None);
    }
    if outcome.is_ok() && want_rate {
        outcome = if code.len() != 3 {
            Err(anyhow::anyhow!("a currency code is three letters, e.g. USD"))
        } else {
            match money::parse_rate(&raw) {
                Ok(micro) if micro > 0 => {
                    fx::set(pool, &code, micro, tulipix_finances::schema::unix_now())
                        .await
                        .map(|_| None)
                }
                _ => Err(anyhow::anyhow!("{raw:?} is not a rate — try 83.60")),
            }
        };
    }
    if outcome.is_ok() && !base_changed && !want_rate {
        outcome = Err(anyhow::anyhow!("nothing to save — set a base currency or a rate"));
    }
    outcome
}

async fn submit_prepay(
    pool: &sqlx::SqlitePool,
    id: i64,
    form: &HashMap<String, String>,
) -> Result<Option<String>> {
    let Some(l) = loans::get(pool, id).await.ok().flatten() else {
        return Err(anyhow::anyhow!("that loan is gone"));
    };
    let extra = form
        .get("extra")
        .and_then(|v| money::parse_amount(v, &l.currency).ok())
        .unwrap_or(0);
    let after: i64 = form.get("after_n").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let start = date::parse(&l.started_on).unwrap_or_else(|_| date::today());
    match loans::prepay(
        l.principal_minor,
        l.rate_bp,
        l.tenure_months,
        l.emi_minor,
        start,
        l.emi_day,
        extra,
        after,
    ) {
        Some(p) => Ok(Some(format!(
            "{} off after {after} instalments finishes the loan {} months early and saves {} \
             in interest.",
            money::format_minor(extra, &l.currency),
            p.months_saved,
            money::format_minor(p.interest_saved_minor, &l.currency)
        ))),
        None => Err(anyhow::anyhow!(
            "that is either nothing or more than the balance — there would be no schedule \
             left to model"
        )),
    }
}

// ── import ──────────────────────────────────────────────────────────────────

/// Submit the import sheet.
///
/// Two different actions wear one button. With the date order still unsettled it
/// re-reads the file with the order the user just chose; with it settled it
/// posts. The first is not an import, so it returns without closing anything.
async fn submit_import(
    pool: &sqlx::SqlitePool,
    (preview, account_id, text): (Preview, i64, String),
    form: &HashMap<String, String>,
    again: bool,
) -> Result<()> {
    let _ = again;
    if preview.date_format.is_none() {
        let chosen = form
            .get("date_format")
            .and_then(|v| DateFormat::parse_name(v))
            .unwrap_or(DateFormat::DayFirst);
        let currency = preview.currency.clone();
        // The origin kind survives the re-run: this is the same file, read again
        // with the date order the user just chose.
        let was = preview.kind;
        match import::preview(&text, &currency, Some(&preview.column_map), Some(chosen)) {
            Ok(mut fresh) => {
                if was.converted() {
                    fresh.kind = was;
                }
                let rows = import_rows(&fresh.rows, &fresh.currency, IMPORT_PREVIEW_ROWS);
                let n = fresh.rows.len();
                let span = fresh.span();
                let mut s = session();
                s.import_needs_format = false;
                s.sheet_primary = "Import".into();
                s.import_summary = format!(
                    "{n} rows{}, read as {}.",
                    span.map(|(a, b)| format!(", {a} to {b}")).unwrap_or_default(),
                    chosen.as_str()
                );
                s.import_rows = rows;
                s.import = Some((fresh, account_id, text));
                s.sheet_busy = false;
            }
            Err(e) => {
                let mut s = session();
                s.sheet_error = e.to_string();
                s.sheet_busy = false;
            }
        }
        return Ok(());
    }

    // Saved before the ingest, so a mapping that took three tries to get right is
    // not lost if the posting itself fails.
    if let Some(name) = form.get("remember_as").map(|s| s.trim()).filter(|s| !s.is_empty())
        && let Some(fmt) = preview.date_format
        && let Err(e) =
            import::save_preset(pool, name, &preview.column_map, fmt, Some(account_id)).await
    {
        tracing::warn!("finances: could not save import preset: {e}");
    }

    match import::ingest(pool, account_id, &preview).await {
        // What the rows became, not just how many. Every line is something the
        // user would otherwise have to go and check: which rows still need a
        // category, which bills the file closed off, and whether the account now
        // agrees with the statement it came from.
        Ok(got) => {
            let base = fx::base_currency();
            let mut parts =
                vec![format!("{} added, {} already there.", got.added, got.duplicates)];
            if got.categorised > 0 {
                parts.push(format!(
                    "{} categorised from what you filed the same merchants under before.",
                    got.categorised
                ));
            }
            if got.uncategorised > 0 {
                parts.push(format!("{} still need a category.", got.uncategorised));
            }
            if got.matched_bills > 0 {
                parts.push(format!("{} matched an open bill and closed it.", got.matched_bills));
            }
            match got.balance_gap_minor {
                Some(0) => parts.push("Closing balance matches the ledger.".to_string()),
                Some(gap) => parts.push(format!(
                    "Closing balance is {} off the ledger — something is missing or duplicated.",
                    money::format_minor(gap.abs(), &base)
                )),
                None => {}
            }
            // The report is the point of having run it — closing the sheet on it
            // would throw away the one moment the user could act on "8 still need
            // a category".
            let mut s = session();
            s.sheet_hint = parts.join(" ");
            s.sheet_primary.clear();
            s.sheet_busy = false;
        }
        Err(e) => {
            let mut s = session();
            s.sheet_error = e.to_string();
            s.sheet_busy = false;
        }
    }
    Ok(())
}

/// Preview a statement and put the import sheet on screen.
///
/// Shared by the file picker, by choosing a saved preset and by correcting a
/// column, which are the same operation with a mapping supplied — the
/// alternative was three copies of the summary wording drifting apart.
async fn show_preview(
    pool: &sqlx::SqlitePool,
    text: String,
    preset: Option<import::Preset>,
    // A mapping the user has corrected by hand, which outranks the preset's.
    override_map: Option<import::csv::ColumnMap>,
    override_format: Option<DateFormat>,
    // What the file was before it became this text, for a PDF or a spreadsheet,
    // and how many of its lines the converter could not use.
    origin: Option<(import::FileKind, usize)>,
) -> Result<()> {
    let all = accounts::list(pool, false).await.unwrap_or_default();
    let account = preset
        .as_ref()
        .and_then(|p| p.account_id)
        .and_then(|id| all.iter().find(|a| a.id == id).cloned())
        .or_else(|| all.iter().find(|a| a.kind != AccountKind::Virtual).cloned());
    let Some(account) = account else {
        let mut s = session();
        s.sheet_kind = "import".into();
        s.sheet_title = "Nowhere to put it".into();
        s.sheet_hint =
            "Add an account first — an imported statement has to belong to one.".into();
        s.sheet_primary.clear();
        s.sheet_busy = false;
        return Ok(());
    };

    let saved = import::presets(pool).await.unwrap_or_default();
    let preset_labels: Vec<String> = std::iter::once(NO_PRESET.to_string())
        .chain(saved.iter().map(|p| format!("{}  #{}", p.bank_name, p.id)))
        .collect();
    let chosen_label = preset
        .as_ref()
        .and_then(|c| preset_labels.iter().find(|l| l.ends_with(&format!("  #{}", c.id))).cloned())
        .unwrap_or_else(|| NO_PRESET.to_string());

    let map = override_map.or_else(|| preset.as_ref().map(|p| p.column_map.clone()));
    let fmt = override_format.or(preset.as_ref().map(|p| p.date_format));

    match import::preview(&text, &account.currency, map.as_ref(), fmt) {
        Ok(mut p) => {
            if let Some((kind, skipped)) = origin {
                p.kind = kind;
                p.unreadable += skipped;
            }
            let needs_format = p.date_format.is_none();
            let rows = import_rows(&p.rows, &p.currency, IMPORT_PREVIEW_ROWS);
            let span = p.span();
            let summary = if needs_format {
                String::new()
            } else {
                format!(
                    "{} rows{}. {} out, {} in. {} lines were not transactions.{}",
                    p.rows.len(),
                    span.map(|(a, b)| format!(", {a} to {b}")).unwrap_or_default(),
                    p.debits(),
                    p.credits(),
                    p.unreadable,
                    // A converted file says so. Column positions in a PDF are
                    // worked out from where the text happened to land, so this is
                    // the one case where checking the rows really matters.
                    if p.kind.converted() {
                        format!(" Read from a {} — check the columns.", p.kind.as_str())
                    } else {
                        String::new()
                    }
                )
            };
            let column_map = p.column_map.clone();
            // The file's own column names, so the mapping rows can be dropdowns
            // the user corrects rather than a guess they can only read. An OFX
            // states its own structure and has none, which is the signal to show
            // the mapping read-only.
            let file_headers = import::csv::headers(&text).unwrap_or_default();
            // With no format settled there is nothing to import yet, so the
            // button re-reads the file with the chosen order instead.
            let primary = if needs_format { "Read it again" } else { "Import" };

            let mut fields = Vec::new();
            if needs_format {
                fields.push(
                    Field::new("date_format", "Date order", "dropdown")
                        .v(DateFormat::DayFirst.as_str())
                        .req(),
                );
                if let Some(f) = fields.last_mut() {
                    f.options = vec![
                        DateFormat::DayFirst.as_str().into(),
                        DateFormat::MonthFirst.as_str().into(),
                    ];
                }
            }
            // Only offered once something is saved: a dropdown whose only entry
            // is "None" is a control that does nothing.
            if !saved.is_empty() {
                let mut f = Field::new("preset", "Saved bank", "dropdown")
                    .v(chosen_label)
                    .hint("Re-reads the file with that bank's column mapping.");
                f.options = preset_labels;
                fields.push(f);
            }
            fields.push(
                Field::new("remember_as", "Remember this bank as", "text")
                    .v(preset.as_ref().map(|p| p.bank_name.clone()).unwrap_or_default())
                    .hint(
                        "Optional. Saves this mapping so the next statement imports in one \
                         click.",
                    ),
            );

            let mapping = import_map(&column_map, &file_headers);
            let mut s = session();
            s.sheet_kind = "import".into();
            s.sheet_id = account.id;
            s.sheet_title = "Import a statement".into();
            s.sheet_hint = format!(
                "Into {}, read as {}. Nothing is posted until you confirm, and re-importing \
                 an overlapping month adds nothing.",
                account.name, p.currency
            );
            s.sheet_primary = primary.into();
            s.sheet_error.clear();
            s.form.clear();
            // The form map has to start out agreeing with what is on screen, or a
            // field the user never touches submits as empty.
            for f in &fields {
                s.form.insert(f.key.clone(), f.value.clone());
            }
            // The mapping rows live in their own list rather than in `fields`, so
            // seed them here too — otherwise correcting one column would read
            // every other one as untouched and re-derive it from the guess the
            // user is in the middle of correcting.
            for f in &mapping {
                s.form.insert(f.key.clone(), f.value.clone());
            }
            s.fields = fields;
            s.import_map = mapping;
            s.import_rows = rows;
            s.import_summary = summary;
            s.import_needs_format = needs_format;
            s.import = Some((p, account.id, text));
            s.sheet_busy = false;
        }
        Err(e) => {
            let mut s = session();
            s.sheet_kind = "import".into();
            s.sheet_title = "That file could not be read".into();
            s.sheet_hint = e.to_string();
            s.sheet_primary.clear();
            s.sheet_busy = false;
        }
    }
    Ok(())
}

async fn pick_statement(chosen: String) -> Result<()> {
    if chosen.is_empty() {
        return Ok(());
    }
    let path = std::path::PathBuf::from(chosen);
    let pool = finances_pool().await?;

    // Read as bytes, not as text: a PDF or a spreadsheet is binary, and even a
    // CSV is often Windows-encoded. `read_file` decides what the file is by its
    // contents and converts PDFs and spreadsheets to CSV, so everything below
    // this line works on one shape.
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("finances: could not read {}: {e}", path.display());
            return Ok(());
        }
    };
    let (text, kind, skipped) = match import::read_file(&bytes) {
        Ok(t) => t,
        Err(e) => {
            // The one import failure worth showing rather than logging: the user
            // chose this file, and "it is a scan, not a text PDF" is something
            // only they can do anything about.
            let mut s = session();
            s.sheet_kind = "import".into();
            s.sheet_title = "That file could not be read".into();
            s.sheet_hint = e.to_string();
            s.sheet_primary.clear();
            s.sheet_busy = false;
            return Ok(());
        }
    };
    show_preview(pool, text, None, None, None, Some((kind, skipped))).await
}

/// Re-read the statement already in hand with a saved bank's mapping.
async fn apply_preset(label: &str) -> Result<()> {
    let Some(id) = fin_sheet::id_from_label(label) else { return Ok(()) };
    let Some((cached, _, text)) = session().import.clone() else { return Ok(()) };
    // The file is still the file it was; only the mapping changed.
    let origin = cached.kind.converted().then_some((cached.kind, 0));
    let pool = finances_pool().await?;
    let found = import::presets(pool).await.unwrap_or_default().into_iter().find(|p| p.id == id);
    show_preview(pool, text, found, None, None, origin).await
}

/// Re-read the loaded file with the mapping the user has just corrected.
///
/// The preview is the point of the mapping rows: getting debit and credit the
/// wrong way round inverts a whole statement and every total still looks
/// plausible, so a correction has to show its effect before anything posts.
async fn remap() -> Result<()> {
    let (cached, text, corrected) = {
        let s = session();
        let Some((cached, _, text)) = s.import.clone() else { return Ok(()) };
        let corrected = column_map_from(&cached.column_map, |k| s.form.get(k).cloned());
        (cached, text, corrected)
    };
    if corrected == cached.column_map {
        return Ok(());
    }
    // The file has not changed, only how it is being read — so the date order
    // already settled on stays settled, and a converted file stays converted.
    let fmt = cached.date_format;
    let origin = cached.kind.converted().then_some((cached.kind, 0));
    let pool = finances_pool().await?;
    show_preview(pool, text, None, Some(corrected), fmt, origin).await
}

/// What picking a category means for that category's envelope this month.
///
/// A note beside the form rather than a rebuilt field list: rebuilding the list
/// mid-typing would take the caret out of whatever the user was writing.
async fn envelope_note(category_label: &str) -> Result<()> {
    let Some(cat) = fin_sheet::id_from_label(category_label) else {
        session().sheet_note.clear();
        return Ok(());
    };
    let period = session().budget_period.clone();
    let pool = finances_pool().await?;
    let base = fx::base_currency();
    let row = budgets::list(pool, &period)
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.category_id == cat);
    session().sheet_note = match row {
        // No envelope on this category is not a problem to report — most
        // categories will never have one.
        None => String::new(),
        // `allowance_minor` rather than `amount_minor`: a rolled-over surplus is
        // part of what is actually available to spend.
        Some(b) if b.id.is_some() => format!(
            "{} envelope: {} of {} used this month.",
            b.category_name,
            money::format_minor(b.spent_minor, &base),
            money::format_minor(b.allowance_minor(), &base)
        ),
        // Spending exists in this category but no envelope has been set on it.
        Some(_) => String::new(),
    };
    Ok(())
}

/// Photo of a receipt → the add sheet, with what could be read filled in.
///
/// The sheet opens either way. A receipt that read badly still saves the user
/// the account, date and category, and the fields it did fill are corrections
/// away from right — which is the only safe shape for this: an OCR'd total is a
/// guess, and a guess must pass under human eyes before it becomes a ledger row.
async fn scan_receipt(chosen: String) -> Result<()> {
    if !tulipix_finances::ocr::available() {
        // The message has to go somewhere the user is looking, and the note only
        // renders inside a sheet — so open the one they were heading for anyway.
        open_sheet("txn", 0).await?;
        session().sheet_note =
            "Reading receipts needs `tesseract` on PATH. Install tesseract and its English \
             data to use the camera route; until then this is the form to type into."
                .into();
        return Ok(());
    }

    if chosen.is_empty() {
        return Ok(());
    }
    let path = std::path::PathBuf::from(chosen);
    let base = fx::base_currency();

    // OCR is a subprocess doing real work on a multi-megapixel photo, so it goes
    // to the blocking pool rather than stalling a reactor thread.
    let read = tokio::task::spawn_blocking(move || tulipix_finances::ocr::read(&path, &base))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|r| r);

    match read {
        Ok(receipt) => {
            let currency = fx::base_currency();
            let amount = receipt
                .amount_minor
                .map(|m| fin_sheet::minor_to_input(m, &currency))
                .unwrap_or_default();
            let merchant = receipt.merchant.clone().unwrap_or_default();
            let on = receipt.occurred_on.clone().unwrap_or_else(|| date::iso(date::today()));
            let found = [
                receipt.merchant.is_some().then_some("merchant"),
                receipt.amount_minor.is_some().then_some("amount"),
                receipt.occurred_on.is_some().then_some("date"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let note = if found.is_empty() {
                "Nothing readable came off that photo. The raw text is in the log.".to_string()
            } else {
                format!("Read from the photo: {}. Check them before posting.", found.join(", "))
            };
            tracing::debug!("finances: receipt text\n{}", receipt.text);

            {
                // The ordinary add sheet, pre-filled. Building it is async, so the
                // values go through `prefill` rather than being written after the
                // call and overwritten by the build.
                let mut s = session();
                s.prefill.clear();
                if !amount.is_empty() {
                    s.prefill.insert("amount".into(), amount);
                }
                if !merchant.is_empty() {
                    s.prefill.insert("description".into(), merchant);
                }
                s.prefill.insert("occurred_on".into(), on);
                s.prefill.insert("source_hint".into(), "ocr".into());
            }
            open_sheet("txn", 0).await?;
            session().sheet_note = note;
        }
        Err(e) => {
            // Still the add sheet: a failed read is a reason to type it in, not a
            // dead end.
            open_sheet("txn", 0).await?;
            session().sheet_error = e.to_string();
        }
    }
    Ok(())
}

/// Fetch today's rates and store them.
///
/// The only network call in this section, and the only reason the bridge's HTTP
/// client is named here. `force` is the Refresh button: without it nothing
/// happens unless the daily refresh is enabled and the stored rates are actually
/// a day old — a section that re-fetched on every visit would be making the same
/// request twenty times an hour.
///
/// Every failure is a warning in the log and nothing on screen. A rate that
/// could not be refreshed is not an error the user can act on, the previous rate
/// is still there and still correct enough to convert with, and the sheet says
/// how old it is.
async fn refresh_rates(force: bool) -> Result<()> {
    let pool = finances_pool().await?;
    if !force && !fx::auto_enabled() {
        return Ok(());
    }
    let now = tulipix_finances::schema::unix_now();
    match fx::stale(pool, now).await {
        Ok(true) => {}
        Ok(false) if !force => return Ok(()),
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("finances: could not tell whether rates are stale: {e}");
            return Ok(());
        }
    }

    let base = fx::base_currency();
    let url = fx::endpoint(&base);
    let body = match reqwest::Client::builder()
        // A rate is worth a few seconds and no more: this runs on section open,
        // and a hanging request must not leave a spinner up for a minute.
        .timeout(std::time::Duration::from_secs(8))
        .build()
    {
        Ok(c) => match c.get(&url).send().await {
            Ok(r) => match r.error_for_status() {
                Ok(r) => r.text().await.ok(),
                Err(e) => {
                    tracing::warn!("finances: rates request refused: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("finances: rates unreachable: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("finances: no HTTP client: {e}");
            None
        }
    };
    let Some(body) = body else { return Ok(()) };

    match fx::parse_rates(&body, &base) {
        Ok(rates) => match fx::apply_live(pool, &rates, now).await {
            Ok(0) => tracing::debug!("finances: rates fetched, nothing needed updating"),
            Ok(n) => tracing::info!("finances: {n} rate(s) updated"),
            Err(e) => tracing::warn!("finances: could not store rates: {e}"),
        },
        Err(e) => tracing::warn!("finances: unusable rates response: {e}"),
    }
    Ok(())
}

// ------------------------------------------------------------------ events ---

fn events() -> &'static OnceLock<StreamSink<FinancesEvent>> {
    static E: OnceLock<StreamSink<FinancesEvent>> = OnceLock::new();
    &E
}

fn emit(e: FinancesEvent) {
    if let Some(sink) = events().get() {
        let _ = sink.add(e);
    }
}

/// Run the recurrence engine, then top up the exchange rates if they are stale.
///
/// Both on the way into a refresh rather than on a timer: the app may have been
/// left running across a date boundary, and a bill that became due at midnight
/// has to be on the page the moment the section is opened rather than at the
/// next restart.
///
/// Only the first tick of the process raises a notification. Re-entering the
/// section is the user already looking at the section, and a banner over the
/// thing it is telling you about is noise.
async fn tick(pool: &sqlx::SqlitePool) {
    static FIRST: OnceLock<()> = OnceLock::new();
    let announce = FIRST.set(()).is_ok();
    match tulipix_finances::tick(pool).await {
        Ok(t) => {
            if announce && let Some((title, body)) = tulipix_finances::notice(&t) {
                emit(FinancesEvent::Notice { title, body });
            }
        }
        Err(e) => tracing::warn!("finances: tick failed: {e}"),
    }
    // After the tick, never before it: the rates call goes over the network, and
    // a refresh must not wait on it to post a bill that is already due.
    if let Err(e) = refresh_rates(false).await {
        tracing::warn!("finances: rates pass failed: {e}");
    }
}

// --------------------------------------------------------------- the verbs ---

/// Everything a command changes, before the snapshot is taken.
///
/// Nothing here holds the session lock across an `.await`: a `MutexGuard` that
/// crosses one makes the whole future `!Send`, and frb's dispatcher requires
/// `Send`. The failure lands in generated code with no line number of its own,
/// so the rule is kept by construction — take the lock, read or write, drop it.
async fn apply(cmd: FinancesCmd) -> Result<()> {
    seed(&mut session());
    match cmd {
        FinancesCmd::Refresh => {
            let pool = finances_pool().await?;
            tick(pool).await;
        }
        FinancesCmd::SetTab { tab } => session().tab = tab,

        // ── overview ────────────────────────────────────────────────────────
        // Clicking one of the twelve bars retargets the ring above it. No query:
        // the twelve breakdowns were read on the last pass, so this is a lookup.
        FinancesCmd::PickMonth { index } => {
            let mut s = session();
            let n = s.month_cats.len();
            s.month_pick = usize::try_from(index).ok().filter(|i| *i < n);
        }

        // ── ledger ──────────────────────────────────────────────────────────
        // Any filter change returns to page one; staying on page 7 of a result
        // set that now has two pages shows an empty table.
        FinancesCmd::TxnSearch { text } => {
            let mut s = session();
            s.txn_search = text;
            s.page = 0;
        }
        FinancesCmd::TxnAccount { name } => {
            let mut s = session();
            s.txn_account = name;
            s.page = 0;
        }
        FinancesCmd::TxnCategory { name } => {
            let mut s = session();
            s.txn_category = name;
            s.page = 0;
        }
        FinancesCmd::TxnKindFilter { kind } => {
            let mut s = session();
            s.txn_kind = kind;
            s.page = 0;
        }
        FinancesCmd::TxnPeriod { period } => {
            let mut s = session();
            s.txn_period = period;
            s.page = 0;
        }
        FinancesCmd::TxnSource { source } => {
            let mut s = session();
            s.txn_source = source;
            s.page = 0;
        }
        // Clicking the sorted column flips the direction; a different column
        // starts descending, which is what a reader of a money table wants.
        FinancesCmd::TxnSortBy { column } => {
            let mut s = session();
            if s.txn_sort == column {
                s.txn_desc = !s.txn_desc;
            } else {
                s.txn_sort = column;
                s.txn_desc = true;
            }
            s.page = 0;
        }
        FinancesCmd::TxnGoto { page } => session().page = page.max(0) as u32,
        FinancesCmd::TxnOpen { id } => open_sheet("txn", id).await?,
        FinancesCmd::TxnDelete { id } => {
            let pool = finances_pool().await?;
            txn::delete(pool, id).await?;
        }

        // ── obligations ─────────────────────────────────────────────────────
        FinancesCmd::PayObligation { id } => open_sheet("pay", id).await?,
        FinancesCmd::SkipObligation { id } => {
            let pool = finances_pool().await?;
            obligations::skip(pool, id).await?;
        }
        FinancesCmd::UnpayObligation { id } => {
            let pool = finances_pool().await?;
            obligations::unpay(pool, id).await?;
        }
        FinancesCmd::SetBillsFilter { filter } => session().bills_filter = filter,
        FinancesCmd::BillsStep { delta } => {
            let mut s = session();
            s.bills_period = step_period(&s.bills_period, delta);
        }

        // ── recurrences ─────────────────────────────────────────────────────
        FinancesCmd::RecurStatus { id, status } => {
            let pool = finances_pool().await?;
            recur::set_status(pool, id, recur::Status::parse(&status)).await?;
        }
        FinancesCmd::RecurDelete { id } => {
            let pool = finances_pool().await?;
            recur::delete(pool, id).await?;
        }
        FinancesCmd::SetSubsFilter { filter } => session().subs_filter = filter,
        // Same rule as the ledger's headers, with one difference: a new column
        // starts ascending unless it is money — a table of names reads A-first,
        // where a table of money reads biggest-first.
        FinancesCmd::SubsSortBy { column } => {
            let mut s = session();
            if s.subs_sort == column {
                s.subs_desc = !s.subs_desc;
            } else {
                s.subs_desc = column == "monthly" || column == "yearly";
                s.subs_sort = column;
            }
        }
        FinancesCmd::SubsCategory { name } => session().subs_category = name,
        FinancesCmd::SubsCurrency { code } => session().subs_currency = code,

        // ── lending ─────────────────────────────────────────────────────────
        FinancesCmd::DueSettle { id } => open_sheet("settle", id).await?,
        FinancesCmd::DueWriteOff { id } => {
            let pool = finances_pool().await?;
            dues::write_off(pool, id, date::today()).await?;
        }
        FinancesCmd::DueDelete { id } => {
            let pool = finances_pool().await?;
            dues::delete(pool, id).await?;
        }
        FinancesCmd::SetDuesFilter { filter } => session().dues_filter = filter,

        // ── accounts and loans ──────────────────────────────────────────────
        FinancesCmd::SetAccountFilter { filter } => session().account_filter = filter,
        FinancesCmd::AccountReconcile { id } => open_sheet("reconcile", id).await?,
        // A recount is a reconcile — the same form, the same posting. The word
        // is different because the physical act is: nobody "reconciles" a
        // wallet, they count what is in it.
        FinancesCmd::AccountRecount { id } => open_sheet("reconcile", id).await?,
        // Paying a card is a transfer, never an expense. Booking it as spending
        // is the second of the two mistakes that make a home-grown tracker's
        // numbers wrong — the money was already counted when it was spent.
        FinancesCmd::AccountPayCard { id } => {
            {
                let mut s = session();
                s.prefill.insert("kind".into(), "Transfer".into());
                s.prefill.insert("to_account".into(), format!("#{id}"));
                s.prefill.insert("description".into(), "Card payment".into());
            }
            open_sheet("txn", 0).await?;
        }
        FinancesCmd::AccountClose { id, closed } => {
            let pool = finances_pool().await?;
            accounts::set_closed(pool, id, closed).await?;
        }
        FinancesCmd::AccountDelete { id } => {
            let pool = finances_pool().await?;
            accounts::delete(pool, id).await?;
        }
        FinancesCmd::LoanSchedule { id } => open_sheet("schedule", id).await?,
        FinancesCmd::LoanPrepay { id } => open_sheet("prepay", id).await?,
        // Paging the schedule never goes back to the database: the whole thing
        // is in hand, and a page is a slice of it.
        FinancesCmd::ScheduleStep { delta } => {
            let mut s = session();
            let pages = s.schedule.len().div_ceil(SCHEDULE_PAGE).max(1);
            s.schedule_page =
                (s.schedule_page as i64 + delta as i64).clamp(0, pages as i64 - 1) as usize;
        }

        // ── envelopes and the calendar ──────────────────────────────────────
        FinancesCmd::BudgetRemove { category_id } => {
            let period = session().budget_period.clone();
            let pool = finances_pool().await?;
            budgets::remove(pool, category_id, &period).await?;
        }
        FinancesCmd::BudgetCopyForward => {
            let period = session().budget_period.clone();
            let pool = finances_pool().await?;
            budgets::copy_forward(pool, &previous_period(&period), &period).await?;
        }
        FinancesCmd::BudgetStep { delta } => {
            let mut s = session();
            s.budget_period = step_period(&s.budget_period, delta);
        }
        // In the year view a step is a year, not a month. Stepping one month
        // through a twelve-month summary would redraw the same twelve.
        FinancesCmd::CalStep { delta } => {
            let mut s = session();
            let by = if s.cal_view == "year" { delta * 12 } else { delta };
            s.cal_period = step_period(&s.cal_period, by);
        }
        FinancesCmd::SetCalView { view } => session().cal_view = view,
        // Planning has one month selector over everything on it, so it steps
        // both periods together — the envelopes and the due dates are always the
        // same month, which was the point of merging Budgets and Calendar.
        FinancesCmd::PlanStep { delta } => {
            let mut s = session();
            let by = if s.cal_view == "year" { delta * 12 } else { delta };
            let next = step_period(&s.cal_period, by);
            s.budget_period = next.clone();
            s.cal_period = next;
        }
        FinancesCmd::PlanGoto { offset } => {
            let next = step_period(&date::ym(date::today()), offset);
            let mut s = session();
            s.budget_period = next.clone();
            s.cal_period = next;
        }
        FinancesCmd::SetPlanModal { which } => session().plan_modal = which,

        // ── sheets ──────────────────────────────────────────────────────────
        FinancesCmd::OpenSheet { kind, id } => open_sheet(&kind, id).await?,
        FinancesCmd::CloseSheet => close_sheet(),
        FinancesCmd::SetField { key, value } => {
            let kind = {
                let mut s = session();
                s.form.insert(key.clone(), value.clone());
                s.sheet_kind.clone()
            };
            // Three fields do something the moment they change rather than on
            // submit, because the answer is what tells the user whether to
            // change them again.
            match (kind.as_str(), key.as_str()) {
                ("import", "preset") if value != NO_PRESET => apply_preset(&value).await?,
                // Correcting a column re-reads the file straight away. The
                // preview under it is the whole reason to correct one, and a
                // mapping that only took effect on Import would be confirmed
                // blind.
                ("import", k) if k.starts_with("map_") => remap().await?,
                ("txn", "category") => envelope_note(&value).await?,
                _ => {}
            }
        }
        FinancesCmd::SubmitSheet => submit_sheet(false).await?,
        FinancesCmd::SubmitAgain => submit_sheet(true).await?,
        // Encoded as `kind:id` by `flag` — a string rather than two fields,
        // because two fields can disagree.
        FinancesCmd::FlagAction { action } => {
            let kind = action.split_once(':').map(|(k, _)| k).unwrap_or("");
            match kind {
                "obligation" => session().tab = "bills".into(),
                "recurrence" => session().tab = "subs".into(),
                // There is no Budgets tab any more, and nothing routes to Plan:
                // the full envelope list is the drill-down behind Planning's
                // Envelopes block, so the flag opens that rather than a tab.
                "category" => {
                    let mut s = session();
                    s.tab = "planning".into();
                    s.plan_modal = "budgets".into();
                }
                "account" => session().tab = "accounts".into(),
                "due" => session().tab = "dues".into(),
                "rates" => open_sheet("rates", 0).await?,
                _ => {}
            }
        }

        // ── import, receipts, detection, rates ──────────────────────────────
        FinancesCmd::ImportPick { path } => pick_statement(path).await?,
        FinancesCmd::ScanReceipt { path } => scan_receipt(path).await?,
        FinancesCmd::AcceptProposal { idx } => {
            let Some(p) = session().proposals.get(idx.max(0) as usize).cloned() else {
                return Ok(());
            };
            let pool = finances_pool().await?;
            if let Err(e) = detect::accept(pool, &p).await {
                tracing::warn!("finances: accepting a proposal failed: {e}");
            }
            // Re-run detection so the accepted one leaves the list.
            let fresh = detect::propose(pool).await.unwrap_or_default();
            session().proposals = fresh;
        }
        FinancesCmd::RatesRefresh => {
            session().rates_busy = true;
            let outcome = refresh_rates(true).await;
            // Bound to a local, never read inside the `if` below: a guard taken
            // in an `if` condition lives to the end of the whole `if`, which
            // would hold the lock across the `.await` in `open_sheet` — a
            // deadlock, and a future that is no longer `Send`.
            let showing_rates = {
                let mut s = session();
                s.rates_busy = false;
                s.sheet_kind == "rates"
            };
            outcome?;
            // The rate list is built when the sheet opens, not by the snapshot,
            // so without this the button fetches new rates and shows the old.
            if showing_rates {
                open_sheet("rates", 0).await?;
            }
        }
        // The account being reconciled is the sheet's subject. Filter the ledger
        // to it, drop every other filter, and close the sheet: the point is to
        // look at the rows, not to keep a dialog open over them.
        FinancesCmd::FindMissing => {
            let id = session().sheet_id;
            let pool = finances_pool().await?;
            // The account's name, because the ledger's filter is a label rather
            // than an id — the same dropdown a person would have used by hand.
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM accounts WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
            close_sheet();
            let mut s = session();
            s.txn_account = name.unwrap_or_else(|| ALL_ACCOUNTS.to_string());
            s.txn_category = ALL_CATEGORIES.into();
            s.txn_kind = "All".into();
            s.txn_search = String::new();
            s.page = 0;
            s.tab = "txns".into();
        }

        // ── sample data ─────────────────────────────────────────────────────
        FinancesCmd::DemoAdd => {
            session().demo_busy = "Adding sample data…".into();
            let pool = finances_pool().await?;
            // `reseed`, not `seed`: seeding declines on a database that has
            // anything real in it, and this is the user asking rather than the
            // app deciding. It marks only the rows it adds, so their own data is
            // not tagged as a sample and cannot be pruned away later.
            if let Err(e) = tulipix_finances::demo::reseed(pool, date::today()).await {
                tracing::warn!("finances: could not add the sample data: {e}");
                emit(FinancesEvent::Failed { message: e.to_string() });
            }
            session().demo_busy.clear();
        }
        FinancesCmd::DemoRemove => {
            session().demo_busy = "Clearing the section…".into();
            let pool = finances_pool().await?;
            // `wipe`, not `remove_all`: the button says "remove all of it", and
            // the marker-scoped version can only reach rows the seed itself
            // wrote — anything grown from them since stayed, and held a sample
            // account open with it.
            if let Err(e) = tulipix_finances::demo::wipe(pool).await {
                tracing::warn!("finances: could not clear the section: {e}");
                emit(FinancesEvent::Failed { message: e.to_string() });
            }
            session().demo_busy.clear();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

/// One pass for the whole section.
///
/// Every tab is loaded on every dispatch, not just the visible one: there is one
/// database under all of them, and loading only the active tab leaves the others
/// showing whatever they last had — which is exactly how a tab comes to be
/// opened onto stale figures. Sequentially that was some thirty round trips end
/// to end; the pool holds eight connections, so awaiting them in groups is the
/// same work in a fraction of the time.
///
/// ponytail: whole-section refresh, ~35 queries. Split per-tab if a ledger with
/// six figures of rows ever makes it visible.
async fn snapshot() -> Result<FinancesState> {
    let pool = finances_pool().await?;
    let base = fx::base_currency();
    let today = date::today();

    // Everything the pass needs out of the session, read and dropped before a
    // single query goes out. The guard must not survive into the awaits below.
    let (
        tab,
        page_no,
        budget_period,
        cal_period,
        cal_view,
        bills_period,
        bills_filter,
        subs_filter,
        subs_sort,
        subs_desc,
        subs_category,
        subs_currency,
        dues_filter,
        account_filter,
        plan_modal,
        filter_search,
        filter_account,
        filter_category,
        filter_kind,
        filter_period,
        filter_source,
        txn_sort_key,
        txn_desc,
    ) = {
        let mut s = session();
        seed(&mut s);
        (
            s.tab.clone(),
            s.page,
            s.budget_period.clone(),
            s.cal_period.clone(),
            s.cal_view.clone(),
            s.bills_period.clone(),
            s.bills_filter.clone(),
            s.subs_filter.clone(),
            s.subs_sort.clone(),
            s.subs_desc,
            s.subs_category.clone(),
            s.subs_currency.clone(),
            s.dues_filter.clone(),
            s.account_filter.clone(),
            s.plan_modal.clone(),
            s.txn_search.clone(),
            s.txn_account.clone(),
            s.txn_category.clone(),
            s.txn_kind.clone(),
            s.txn_period.clone(),
            s.txn_source.clone(),
            s.txn_sort.clone(),
            s.txn_desc,
        )
    };
    let sort = TxnSort::parse(&txn_sort_key.to_lowercase());

    // Filter dropdowns, and the maps that let a chosen label become an id.
    let all_accounts = accounts::list(pool, false).await.unwrap_or_default();
    let account_pairs: Vec<(String, i64)> =
        all_accounts.iter().map(|a| (a.name.clone(), a.id)).collect();
    let category_pairs = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, name FROM categories ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let account_id = pick_id(&account_pairs, &filter_account);
    let category_id = pick_id(
        &category_pairs.iter().map(|(id, n)| (n.clone(), *id)).collect::<Vec<_>>(),
        &filter_category,
    );

    // The month filter narrows in SQL, not after paging: filtering a page that
    // was already cut to 25 rows would show four of them and call it page one of
    // fifty.
    let (period_from, period_to) = match filter_period.as_str() {
        EVERY_MONTH => (None, None),
        p => match date::month_bounds(p) {
            Ok((f, t)) => (Some(f), Some(t)),
            Err(_) => (None, None),
        },
    };
    let filter = TxnFilter {
        account_id,
        category_id,
        kind: match filter_kind.as_str() {
            "Expense" => Some(TxnKind::Expense),
            "Income" => Some(TxnKind::Income),
            "Transfer" => Some(TxnKind::Transfer),
            _ => None,
        },
        from: period_from,
        to: period_to,
        search: Some(filter_search.clone()).filter(|s| !s.trim().is_empty()),
        source: Some(filter_source.clone()).filter(|s| s != ANY_SOURCE),
    };
    // A running balance only means something down one account's own column, and
    // only when the rows are in date order. Anywhere else it would be a running
    // total of unrelated rows.
    let running_shown = account_id.is_some() && sort == TxnSort::Date;

    // Every window this pass needs, worked out before a single query goes out.
    // Pure date arithmetic, so none of the reads below depends on another one
    // and they can all be in flight at once.
    let period = date::ym(today);
    let (from, to) = date::month_bounds(&period).unwrap_or_default();
    // The month the Bills tab is pointed at, widened backwards so a bill that
    // was due last month and is still unpaid stays in front of the user rather
    // than falling off the end of a calendar boundary.
    let (bill_from, bill_to) = match date::month_bounds(&bills_period) {
        Ok((f, t)) => (
            date::parse(&f)
                .map(|d| date::iso(d - chrono::Duration::days(31)))
                .unwrap_or(f),
            t,
        ),
        Err(_) => (
            date::iso(today - chrono::Duration::days(90)),
            date::iso(today + chrono::Duration::days(60)),
        ),
    };
    let (cal_from, cal_to) = date::month_bounds(&cal_period).unwrap_or_default();
    // The envelopes' own month. Its own query rather than the Bills tab's
    // window: the two step independently, and a "not budgeted" list for a
    // different month than the envelopes above it is worse than no list.
    let (budget_from, budget_to) = date::month_bounds(&budget_period).unwrap_or_default();

    let (txn_periods, txn_sources, page, snap, cats, month_rows, needs) = tokio::join!(
        txn::periods(pool),
        txn::sources(pool),
        txn::page(pool, &filter, sort, txn_desc, page_no),
        insights::snapshot(pool, today),
        txn::spend_by_category(pool, &from, &to),
        txn::monthly_totals(pool, 12),
        obligations::needs_you(pool, today, 14),
    );
    let txn_periods = txn_periods.unwrap_or_default();
    let txn_sources = txn_sources.unwrap_or_default();
    let page = page.unwrap_or_default();
    let snap = snap.unwrap_or_default();
    let cats = cats.unwrap_or_default();
    let month_rows = month_rows.unwrap_or_default();
    let needs = needs.unwrap_or_default();

    let (
        subs,
        bill_templates,
        bills,
        subs_yearly_minor,
        all_dues,
        due_totals,
        account_totals,
        loans_rows,
        account_details,
    ) = tokio::join!(
        recur::list(pool, Some(recur::RecurKind::Subscription), true),
        recur::list(pool, Some(recur::RecurKind::Bill), true),
        obligations::between(pool, &bill_from, &bill_to, today),
        recur::yearly_total(pool, &base),
        dues::list(pool, None),
        dues::totals(pool),
        accounts::totals(pool),
        loans::list(pool),
        accounts::details(pool, &from, &to, today),
    );
    let subs = subs.unwrap_or_default();
    let bill_templates = bill_templates.unwrap_or_default();
    let bills = bills.unwrap_or_default();
    let subs_yearly_minor = subs_yearly_minor.unwrap_or(0);
    let all_dues = all_dues.unwrap_or_default();
    let due_totals = due_totals.unwrap_or_default();
    let account_totals = account_totals.unwrap_or_default();
    let loans_rows = loans_rows.unwrap_or_default();
    let account_details = account_details.unwrap_or_default();

    let (elapsed, total_days) = budgets::month_progress(&budget_period, today);
    // Calendar income comes from the ledger rather than from a schedule: salary
    // that has actually landed is a fact, and a projected credit would be a
    // guess sitting on the same grid as real obligations.
    let (budget_list, alloc, disc, disc_history, budget_fixed, cal_items, cal_income, low) = tokio::join!(
        budgets::list(pool, &budget_period),
        budgets::allocation(pool, &budget_period),
        budgets::discipline(pool, DISCIPLINE_MONTHS as u32, today),
        budgets::envelope_history(pool, DISCIPLINE_MONTHS as u32, today),
        obligations::between(pool, &budget_from, &budget_to, today),
        obligations::between(pool, &cal_from, &cal_to, today),
        txn::income_by_day(pool, &cal_from, &cal_to),
        insights::cash_flow_low_point(pool, today, 30),
    );
    let budget_list = budget_list.unwrap_or_default();
    let alloc = alloc.unwrap_or_default();
    let disc = disc.unwrap_or_default();
    let (disc_periods, disc_rows) = disc_history.unwrap_or_default();
    let budget_fixed = budget_fixed.unwrap_or_default();
    let cal_items = cal_items.unwrap_or_default();
    let cal_income = cal_income.unwrap_or_default();
    let low = low.unwrap_or_default();

    // Twelve months around the one being viewed, for the Year toggle. Only
    // loaded when that view is on: it is twelve times the work of the grid and
    // nothing on the month view reads it.
    let mut cal_year: Vec<(String, Vec<Obligation>, i64)> = Vec::new();
    if cal_view == "year"
        && let Some(jan) = date::parse(&cal_from).ok().and_then(|d| d.with_month(1))
    {
        for m in 1..=12u32 {
            let Some(first) = jan.with_month(m) else { continue };
            let p = date::ym(first);
            let Ok((f, t)) = date::month_bounds(&p) else { continue };
            let items = obligations::between(pool, &f, &t, today).await.unwrap_or_default();
            let income = txn::income_between(pool, &f, &t).await.unwrap_or(0);
            cal_year.push((p, items, income));
        }
    }

    // What actually falls on the heaviest day, so the card can list it rather
    // than only totalling it.
    let heaviest_on = {
        let mut by_day: BTreeMap<&str, i64> = BTreeMap::new();
        for o in &cal_items {
            if let Some(m) = o.shown_minor() {
                *by_day.entry(o.due_on.as_str()).or_default() += m;
            }
        }
        by_day.into_iter().max_by_key(|(_, m)| *m).map(|(d, _)| d.to_string())
    };
    let cal_heaviest_items: Vec<Obligation> = match &heaviest_on {
        Some(d) => cal_items.iter().filter(|o| &o.due_on == d).cloned().collect(),
        None => Vec::new(),
    };

    // The overview's own short ledger, unfiltered — the Transactions tab's
    // filters must not silently change what "recent" means on the dashboard.
    let recent = txn::page(pool, &TxnFilter::default(), TxnSort::Date, true, 0)
        .await
        .unwrap_or_default();

    // Six months of spending by category, for the Insights trend card. Oldest
    // first, so the bars read left to right like every other chart here.
    let mut trend_months: Vec<(String, Vec<CategorySpend>)> = Vec::new();
    for back in (0..TREND_MONTHS).rev() {
        let Some(first) = today
            .with_day(1)
            .and_then(|d| d.checked_sub_months(chrono::Months::new(back)))
        else {
            continue;
        };
        let p = date::ym(first);
        let Ok((f, t)) = date::month_bounds(&p) else { continue };
        trend_months.push((p, txn::spend_by_category(pool, &f, &t).await.unwrap_or_default()));
    }

    // The same breakdown for each of the twelve bars, because the bars are the
    // Overview's month picker and clicking one has to repaint the ring at once.
    // Twelve indexed reads on a pass that already makes thirty is cheaper than
    // one read on every click.
    let mut month_cats: Vec<Vec<CategorySpend>> = Vec::new();
    for m in &month_rows {
        month_cats.push(match date::month_bounds(&m.period) {
            Ok((f, t)) => txn::spend_by_category(pool, &f, &t).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        });
    }

    // Insights and the badge. `projection` is the one read here that has to
    // wait: it is computed *from* the flags and the savings history.
    //
    // Overdue is read separately from `badge`, which counts everything inside
    // the notice window: overdue makes the badge an alarm rather than a count.
    let (flags, savings_rows, demo, badge, overdue) = tokio::join!(
        insights::flags(pool, today),
        insights::savings_history(pool, TREND_MONTHS),
        tulipix_finances::demo::present(pool),
        obligations::badge_count(pool, today, tulipix_finances::lead_days()),
        obligations::overdue_names(pool, today),
    );
    let flags = flags.unwrap_or_default();
    let savings_rows = savings_rows.unwrap_or_default();
    let demo = demo.unwrap_or(false);
    let badge = badge.unwrap_or(0);
    let overdue = overdue.unwrap_or_default();
    let projection_data = insights::projection(pool, today, &flags, &savings_rows)
        .await
        .unwrap_or_default();

    // The health score is always about *this* month, whatever month the
    // envelopes happen to be parked on — a score that moved when you stepped a
    // tab would be measuring the interface, not the money.
    let health_budgets = if budget_period == period {
        budget_list.clone()
    } else {
        budgets::list(pool, &period).await.unwrap_or_default()
    };
    let health = insights::health(
        &snap,
        &savings_rows,
        &health_budgets,
        overdue.len() as i64,
        &base,
    );

    // Unused-subscription figures come from the same flag the Insights tab
    // shows, so the two tabs cannot disagree about what is unused.
    let last_playback = insights::last_video_playback().await;
    let unused = insights::unused_subscription_flags(pool, today, last_playback)
        .await
        .unwrap_or_default();
    let unused_names: Vec<String> = unused.iter().map(|f| f.title.clone()).collect();
    let unused_yearly: i64 = subs
        .iter()
        .filter(|r| unused.iter().any(|f| f.title.starts_with(&r.name)))
        .map(|r| r.yearly_minor)
        .sum();

    let settled_from = date::iso(today - chrono::Duration::days(DUES_HISTORY_DAYS));
    let due_settled_totals = dues::settled_since(pool, &settled_from).await.unwrap_or_default();

    // The one read no other tab asks for: the day-by-day spend the burn-down
    // lines are drawn from.
    let daily_cat = txn::daily_by_category(pool, &budget_from, &budget_to)
        .await
        .unwrap_or_default();

    // ── everything below is arithmetic on what is already in hand ───────────

    // Hand the month picker its lookup table, and clamp the selection while we
    // are here: the bar list is rebuilt on every pass and can get shorter, and
    // an index left pointing past its end would empty the ring instead of
    // falling back to the latest month.
    let month_pick = {
        let mut s = session();
        s.month_cats = month_cats.clone();
        s.month_base = base.clone();
        match s.month_pick {
            Some(i) if i < month_cats.len() => Some(i),
            _ => {
                s.month_pick = None;
                None
            }
        }
    };
    // Nothing picked yet means the newest bar, which is the one the chart
    // highlights — not `cats`. A month with no spending at all is absent from
    // `monthly_totals`, so on the first day of a month those two are different
    // months, and taking `cats` here would put this month's ring under last
    // month's heading. `cats` stays only as the no-history fallback.
    let picked_cats = month_pick
        .and_then(|i| month_cats.get(i))
        .or_else(|| month_cats.last())
        .unwrap_or(&cats);

    // The stat strip is computed from every bill in the window, and the list
    // from the filtered subset — so the four figures keep saying what the month
    // holds while the user narrows the list under them.
    let (bills_all, bills_needs, bills_auto, bills_paid, bills_oneoff) = bill_counts(&bills);
    let shown_bills = filter_bills(&bills, &bills_filter);

    let (subs_all, subs_active, subs_paused, subs_cancelled, subs_removed) = sub_counts(&subs);
    // Sorted on the rows rather than on the formatted cells: "₹1,299" and "₹999"
    // sort the wrong way round as text.
    let mut shown_subs = filter_subs(
        &subs,
        &subs_filter,
        Some(subs_category.as_str()).filter(|c| *c != EVERY_SUB_CATEGORY),
        Some(subs_currency.as_str()).filter(|c| *c != EVERY_CURRENCY),
    );
    sort_subs(&mut shown_subs, &subs_sort, subs_desc);

    let (dues_all, dues_to_me, dues_i_owe, dues_closed) = due_counts(&all_dues);
    // Closed inside the history window. `due_settled_totals` above is the totals
    // for the stat strip; the card needs the rows themselves, which are already
    // in hand — `all_dues` is every due whatever its status.
    let settled_rows: Vec<Due> = all_dues
        .iter()
        .filter(|d| !matches!(d.status, dues::Status::Open))
        .filter(|d| d.settled_on.as_deref().is_none_or(|on| on >= settled_from.as_str()))
        .cloned()
        .collect();

    // The strip is built from every account; only the grid under it narrows. A
    // card that changed the total it is part of would be a card that argues with
    // itself.
    let shown_accounts: Vec<AccountRow> = all_accounts
        .iter()
        .filter(|a| match account_filter.as_str() {
            "cash" => a.kind.is_liquid(),
            "cards" => a.kind.as_str() == "card",
            // "loans" hides the grid outright rather than emptying it, and the
            // rest — "", "all", "lending" — do not narrow it at all.
            "loans" => false,
            _ => true,
        })
        .cloned()
        .collect();

    let (ui_fixed_items, fixed_total_minor, subs_month_minor) =
        fixed_items(&budget_fixed, &subs, &base);
    // Where this month's income is committed. Bills here are the ones still to
    // pay this month, so the figure moves as they are paid — which is the point.
    let loan_emi: i64 = loans_rows.iter().map(|l| l.emi_minor).sum();
    let bills_left: i64 = bills
        .iter()
        .filter(|o| !matches!(o.status, ObStatus::Paid | ObStatus::Skipped))
        .filter_map(|o| o.shown_minor())
        .sum();
    // Spending this month with no envelope watching it.
    let unbudgeted: i64 = budget_list.iter().filter(|b| b.id.is_none()).map(|b| b.spent_minor).sum();
    let unbudgeted_n = budget_list.iter().filter(|b| b.id.is_none() && b.spent_minor > 0).count();

    let cal_days = calendar(&cal_period, today, &cal_items, &cal_income, &base);
    let (cal_warning, cal_heaviest, cal_heaviest_label, cal_heaviest_sub) =
        calendar_cards(low.as_ref(), &cal_items, account_totals.liquid_minor, &base);
    // What the month closes at: what is in the bank now, less everything dated
    // and still open. Expected income is not added — the same rule the cash-flow
    // walk states, and the closing figure has to be the same one.
    let cal_out: i64 = cal_items
        .iter()
        .filter(|o| o.status.is_open())
        .filter_map(|o| o.shown_minor())
        .sum();
    let plan_closing_minor = account_totals.liquid_minor - cal_out;
    let cal_flow = cash_flow(
        account_totals.liquid_minor,
        &cal_items,
        snap.income_this_month_minor,
        low.as_ref(),
        &base,
    );
    // In the year view the header names the year, not a month inside it.
    let cal_label = if cal_view == "year" {
        cal_period.split('-').next().unwrap_or(&cal_period).to_string()
    } else {
        month_long(&cal_period)
    };
    let cal_low_point = low
        .filter(|l| l.balance_minor < 0)
        .map(|l| format!("Lowest around {}: {}", l.on, money::format_minor(l.balance_minor, &base)))
        .unwrap_or_default();

    // Measured against the income of the month being *viewed*, not of today —
    // the tab steps, and a share of a different month's income is a ratio of two
    // unrelated numbers. The same division twice: once as a sentence for the
    // envelopes drill-down, once as a number for Planning's cash-flow bar, so
    // the bar and the caption under it cannot disagree.
    let fixed_pct = if alloc.income_minor > 0 {
        (fixed_total_minor as i128 * 100 / alloc.income_minor as i128).clamp(0, 100) as i32
    } else {
        0
    };
    let fixed_share = if alloc.income_minor > 0 {
        format!(
            "{}% of what came in that month",
            (fixed_total_minor as i128 * 100 / alloc.income_minor as i128).min(999)
        )
    } else {
        "No income recorded that month to measure it against.".to_string()
    };
    let (alloc_income, alloc_budgeted, alloc_left, alloc_pct) = allocation(&alloc, &base);
    // What no one has claimed. Floored at zero: a month whose commitments and
    // envelopes together come to more than its income has nothing free, and a
    // negative slice would be drawn as a bar running backwards.
    let plan_free_pct = (100 - fixed_pct - alloc_pct).max(0);

    // The first of the month being viewed, so a prediction that spills past the
    // month can say which day of the next one it lands on.
    let budget_first = date::parse(&budget_from).unwrap_or(today);
    let savings_pct = savings_rows.last().and_then(|m| m.rate_pct);
    let (projection_rows, projection_note) = projection(&projection_data, &base);

    // Nothing at all yet — no real account, no posting. The first-run door
    // stands in for an Overview that would otherwise be nine zeroes.
    let empty = page.total == 0
        && all_accounts.iter().all(|a| a.kind == AccountKind::Virtual)
        && filter.search.is_none();

    let mut account_names = vec![ALL_ACCOUNTS.to_string()];
    account_names.extend(account_pairs.iter().map(|(n, _)| n.clone()));
    let mut category_names = vec![ALL_CATEGORIES.to_string()];
    category_names.extend(category_pairs.iter().map(|(_, n)| n.clone()));
    // Month labels are long-form for the dropdown ("July 2026") but the filter
    // is keyed on the period, so the sentinel and the raw periods go through and
    // the box shows those. A prettier label would need a second list and a
    // lookup between them for no gain.
    let mut period_names = vec![EVERY_MONTH.to_string()];
    period_names.extend(txn_periods);
    let mut source_names = vec![ANY_SOURCE.to_string()];
    source_names.extend(txn_sources);
    let mut sub_category_names = vec![EVERY_SUB_CATEGORY.to_string()];
    sub_category_names.extend(sub_categories(&subs));
    let mut sub_currency_names = vec![EVERY_CURRENCY.to_string()];
    sub_currency_names.extend(sub_currencies(&subs));

    // The sheet, last: it is the only part of the pass that is pure session, and
    // reading it here rather than at the top means a command that opened one is
    // already reflected.
    let s = session();
    let sched_pages = s.schedule.len().div_ceil(SCHEDULE_PAGE).max(1);
    let sched_from = (s.schedule_page * SCHEDULE_PAGE).min(s.schedule.len());
    let sched_to = (sched_from + SCHEDULE_PAGE).min(s.schedule.len());
    let state = FinancesState {
        tab,

        stats: overview_stats(&snap, &health, &base),
        health_parts: health_parts(&health.parts),
        health_score: match health.score {
            Some(n) => format!("{n} / 100"),
            None => "—".to_string(),
        },
        health_band: health.band().to_string(),
        slices: slices(picked_cats, &base),
        spent_total: money::format_minor(snap.spent_this_month_minor, &base),
        months: month_bars(&month_rows, &base),
        month_label: month_long(&period),
        month_pick: month_pick.map(|i| i as i32).unwrap_or(-1),
        needs_you: obligation_rows(&needs),
        recent: txn_rows(
            recent.rows.iter().take(OVERVIEW_TXNS).cloned().collect::<Vec<_>>().as_slice(),
            &base,
            false,
        ),

        txns: txn_rows(&page.rows, &base, running_shown),
        txn_page: page.page as i32,
        txn_pages: (page.pages as i32).max(1),
        txn_total: page.total as i32,
        txn_sort: txn_sort_key,
        txn_desc,
        txn_search: filter_search,
        txn_account: filter_account,
        txn_category: filter_category,
        txn_kind: filter_kind,
        txn_period: filter_period,
        txn_source: filter_source,
        account_names,
        category_names,
        txn_periods: period_names,
        source_names,
        running_shown,
        txn_spent: money::format_minor(page.spent_minor, &base),
        txn_income: money::format_minor(page.income_minor, &base),

        bills: obligation_rows(&shown_bills),
        bill_stats: bill_stats(&bills, subs_yearly_minor, snap.income_this_month_minor, &base),
        bill_templates: recur_rows(&bill_templates, &base),
        bills_filter,
        bills_period: month_long(&bills_period),
        bills_all,
        bills_needs,
        bills_auto,
        bills_paid,
        bills_oneoff,

        subs: recur_rows(&shown_subs, &base),
        sub_stats: sub_stats(&subs, &base, &unused_names, unused_yearly),
        subs_yearly: money::format_minor(subs_yearly_minor, &base),
        subs_filter,
        subs_sort,
        subs_desc,
        subs_category,
        subs_currency,
        subs_categories: sub_category_names,
        subs_currencies: sub_currency_names,
        subs_all,
        subs_active,
        subs_paused,
        subs_cancelled,
        subs_removed,

        dues: due_rows(&filter_dues(&all_dues, &dues_filter)),
        dues_settled: due_rows(&settled_rows),
        due_stats: due_stats(&due_totals, &due_settled_totals, &all_dues, &base),
        dues_filter,
        dues_all,
        dues_to_me,
        dues_i_owe,
        dues_closed,

        accounts: account_rows(&shown_accounts, &account_details, &base),
        account_stats: account_stats(
            &all_accounts,
            due_totals.owed_to_me_minor - due_totals.i_owe_minor,
            &base,
        ),
        account_filter,
        liquid_total: money::format_minor(account_totals.liquid_minor, &base),
        debt_total: money::format_minor(account_totals.debt_minor, &base),
        // `LoanRow::balance_minor` is already flipped positive by `loans::list` —
        // it is what is outstanding, not the account's negative balance.
        loans_total: money::format_minor(
            loans_rows.iter().map(|l| l.balance_minor.max(0)).sum(),
            &base,
        ),
        loans: loan_rows(&loans_rows, today),

        plan_health: plan_health(
            account_totals.liquid_minor,
            plan_closing_minor,
            &cal_warning,
            savings_pct,
            &base,
        ),
        timeline: timeline(&cal_days, &cal_items, &cal_income, plan_closing_minor, &base),
        attention: attention(&needs, &flags, &base),
        recos: recommendations(&flags),
        predictions: predictions(&budget_list, &daily_cat, budget_first, elapsed, total_days, &base),
        habits: habits(&cal_items, &disc, &subs, unbudgeted, unbudgeted_n, &base),
        review: monthly_review(&budget_list, &daily_cat, &cal_income, &base),
        trends: trend_series(&savings_rows, &base),
        plan_modal,
        // The month on its own terms: what came in, less what has a claim on it.
        // Separate from the cash-flow walk, which measures a bank balance and
        // therefore carries every month before this one inside it.
        plan_statement: month_statement(
            alloc.income_minor,
            fixed_total_minor - subs_month_minor,
            subs_month_minor,
            alloc.budgeted_minor,
            &base,
        ),
        // What the bar's third slice is actually worth — income less
        // commitments less envelopes, the same subtraction the bar draws.
        plan_free: money::format_minor(
            alloc.income_minor - fixed_total_minor - alloc.budgeted_minor,
            &base,
        ),
        plan_free_pct,

        budgets: budget_rows(&budget_list, &base, elapsed, total_days),
        budget_period: month_long(&budget_period),
        budget_income: alloc_income,
        budget_allocated: alloc_budgeted,
        budget_unallocated: alloc_left,
        budget_unbudgeted: if unbudgeted > 0 {
            money::format_minor(unbudgeted, &base)
        } else {
            String::new()
        },
        budget_unbudgeted_sub: if unbudgeted_n == 0 {
            "Every category you spent in this month has an envelope.".to_string()
        } else {
            format!(
                "across {unbudgeted_n} categor{} with no envelope set",
                if unbudgeted_n == 1 { "y" } else { "ies" }
            )
        },
        budget_allocated_pct: alloc_pct,
        commitments: commitments(
            snap.income_this_month_minor,
            loan_emi,
            bills_left,
            subs_yearly_minor / 12,
            &base,
        ),
        fixed_items: ui_fixed_items,
        fixed_total: money::format_minor(fixed_total_minor, &base),
        fixed_share,
        fixed_pct,
        // What the envelopes have actually taken. `budget_allocated` is what
        // they were set to; a strip card showing one without the other says
        // nothing.
        envelopes_spent: money::format_minor(
            budget_list.iter().map(|b| b.spent_minor).sum(),
            &base,
        ),
        discipline: discipline_rows(&disc, &base),
        discipline_months: period_labels(&disc_periods),
        envelope_history: envelope_history(&disc_rows, &base),

        cal_focus: calendar_focus(&cal_days),
        cal_days,
        cal_label,
        cal_agenda: obligation_rows(&cal_items),
        cal_view,
        cal_months: calendar_year(&cal_year, &cal_period, &base),
        cal_flow,
        cal_heaviest_items: obligation_rows(&cal_heaviest_items),
        cal_warning,
        cal_heaviest,
        cal_heaviest_label,
        cal_heaviest_sub,
        cal_low_point,

        flags: flags.iter().map(flag).collect(),
        insight_stats: insight_stats(&snap, &flags, subs_yearly_minor, &base),
        trend_pies: trend_pies(&trend_months, &base),
        savings: savings_months(&savings_rows, &base),
        savings_summary: savings_summary(&savings_rows, &base),
        projection: projection_rows,
        projection_note,

        sheet: s.sheet_kind.clone(),
        sheet_title: s.sheet_title.clone(),
        sheet_hint: s.sheet_hint.clone(),
        sheet_primary: s.sheet_primary.clone(),
        sheet_error: s.sheet_error.clone(),
        sheet_note: s.sheet_note.clone(),
        sheet_busy: s.sheet_busy,
        form: to_fields(&s.fields),
        prices: s.prices.clone(),
        schedule: schedule_rows(&s.schedule[sched_from..sched_to], &s.schedule_currency),
        schedule_page: s.schedule_page as i32 + 1,
        schedule_pages: sched_pages as i32,
        import_rows: s.import_rows.clone(),
        import_summary: s.import_summary.clone(),
        import_needs_format: s.import_needs_format,
        import_map: s.import_map.clone(),
        proposals: proposal_rows(&s.proposals),
        rates: s.rates.clone(),
        rates_busy: s.rates_busy,

        empty,
        demo,
        demo_busy: s.demo_busy.clone(),
        badge: badge as i32,
        badge_overdue: !overdue.is_empty(),
    };
    drop(s);
    Ok(state)
}

// ---------------------------------------------------------------- exported ---

pub async fn finances_dispatch(cmd: FinancesCmd) -> Result<FinancesState> {
    apply(cmd).await?;
    snapshot().await
}

#[frb(sync)]
pub fn finances_events(sink: StreamSink<FinancesEvent>) {
    let _ = events().set(sink);
}

/// What the statement chooser should offer, straight from the importer that has
/// to read the file. Transcribing this list into Dart would mean a format the
/// pipeline gained but the dialog would not show.
#[frb(sync)]
pub fn finances_import_extensions() -> Vec<String> {
    import::EXTENSIONS.iter().map(|e| e.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_a_period_crosses_year_boundaries_both_ways() {
        assert_eq!(step_period("2026-07", 1), "2026-08");
        assert_eq!(step_period("2026-12", 1), "2027-01");
        assert_eq!(step_period("2026-01", -1), "2025-12");
        assert_eq!(step_period("2026-03", -1), "2026-02", "and lands on a short month");
        assert_eq!(step_period("2026-07", 0), "2026-07");
        assert_eq!(step_period("2026-07", 6), "2027-01");
        assert_eq!(step_period("2026-07", -7), "2025-12");
    }

    #[test]
    fn a_nonsense_period_is_returned_unchanged_rather_than_panicking() {
        assert_eq!(step_period("not a month", 1), "not a month");
        assert_eq!(previous_period(""), "");
    }

    #[test]
    fn a_dropdown_label_resolves_to_its_id_and_the_all_label_to_none() {
        let pairs = vec![("HDFC".to_string(), 7i64), ("Paytm".to_string(), 9)];
        assert_eq!(pick_id(&pairs, "HDFC"), Some(7));
        assert_eq!(pick_id(&pairs, "Paytm"), Some(9));
        assert_eq!(pick_id(&pairs, ALL_ACCOUNTS), None);
        assert_eq!(pick_id(&pairs, "Gone"), None);
    }

    /// The free slice can never be drawn backwards, however far the two claims
    /// above it overrun the month's income.
    #[test]
    fn the_free_slice_floors_at_zero() {
        let free = |fixed: i32, envelopes: i32| (100 - fixed - envelopes).max(0);
        assert_eq!(free(40, 30), 30);
        assert_eq!(free(80, 40), 0, "commitments and envelopes over income leave nothing");
        assert_eq!(free(0, 0), 100);
    }
}
