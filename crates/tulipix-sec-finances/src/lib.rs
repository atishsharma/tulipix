//! Finances-section wiring: the bridge between [`tulipix_finances`] — which is
//! async, integer-only and knows nothing about Slint — and every
//! `window.on_fin_*` callback.
//!
//! `tulipix-app` calls [`wire`] once at startup and [`section_changed`] from the
//! sidebar hook. Nothing here holds a lock across an await: the shared state is
//! taken, read or written, and dropped before any database call.
//!
//! # One refresh, not nine
//!
//! [`refresh`] gathers every tab's data on every change rather than only the
//! visible tab's. It is a local SQLite file with a handful of indexed tables, and
//! the alternative is nine partial refresh paths that each have to remember which
//! of the other eight they invalidate — which is where a stale badge or a
//! disagreeing total comes from.
//!
//! ponytail: whole-section refresh, ~20 queries. Split per-tab if a ledger with
//! six figures of rows ever makes it visible.

pub mod sheet;
pub mod view;

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Datelike;

use slint::ComponentHandle;
use tulipix_finances::{
    accounts, budgets, date, dues,
    import::{self, dates::DateFormat, detect, Preview},
    insights, loans, money, obligations, recur,
    txn::{self, TxnFilter, TxnKind, TxnSort},
};
use tulipix_sec_photos::replace_rows;
use tulipix_ui::*;

/// Rows of a statement shown in the import preview. Enough to see the mapping is
/// right; the full file still imports.
const IMPORT_PREVIEW_ROWS: usize = 40;

/// Instalments per page in the amortisation sheet.
const SCHEDULE_PAGE: usize = 10;

/// How many months of budget-versus-actual the discipline table shows.
const DISCIPLINE_MONTHS: u32 = 6;

/// Months in the Insights trend card. Six, matching the discipline table, so the two
/// halves of "how has this been going" cover the same window.
const TREND_MONTHS: u32 = 6;

/// How many categories the trend card draws. Beyond about five it stops being a
/// comparison and becomes a list.
const TREND_CATEGORIES: usize = 5;

/// Rows in the overview's recent-transactions card. Enough to recognise the last
/// day or two; the Transactions tab is where the ledger is read properly.
const OVERVIEW_TXNS: usize = 6;

/// Label for "no account filter". Matched by value, so it is a constant.
const ALL_ACCOUNTS: &str = "All accounts";
const ALL_CATEGORIES: &str = "All categories";
const EVERY_MONTH: &str = "Every month";
const ANY_SOURCE: &str = "Any source";
const EVERY_SUB_CATEGORY: &str = "Every category";
const EVERY_CURRENCY: &str = "Every currency";

/// How many months of dues history the settled card carries.
const DUES_HISTORY_DAYS: i64 = 182;

#[derive(Default)]
struct State {
    /// Ledger paging and filtering.
    page: u32,
    /// `YYYY-MM` the Budgets tab is looking at.
    budget_period: String,
    /// Which group of bills the Bills tab is showing: all|needs|auto|paid|oneoff.
    bills_filter: String,
    /// Which subscriptions the Subscriptions tab is showing: all|active|paused|cancelled.
    subs_filter: String,
    /// Which dues the Dues tab is showing: all|to-me|i-owe|closed.
    dues_filter: String,
    /// `YYYY-MM` the Calendar tab is looking at, and whether it is showing the
    /// day grid or the twelve-month summary.
    cal_period: String,
    cal_view: String,
    /// `YYYY-MM` the Bills tab is looking at. The list is dated, so without this
    /// it shows whatever window the loader happened to ask for.
    bills_period: String,
    /// Which sheet is open, and what it is editing.
    sheet_kind: String,
    sheet_id: i64,
    /// Field values as the user has typed them.
    form: HashMap<String, String>,
    /// Values to overwrite the next sheet's fields with, consumed once.
    ///
    /// Needed because building a sheet is async: anything written into `form`
    /// straight after `open_sheet` is wiped when the build finishes and seeds the
    /// form from its own defaults. This is applied inside that build instead.
    prefill: HashMap<String, String>,
    /// Detected recurring charges, indexed by position — nothing exists in the
    /// database yet, so there is no id to key on.
    proposals: Vec<detect::Proposal>,
    /// A parsed statement waiting to be confirmed, and the account it lands in.
    import: Option<(Preview, i64, String)>,
    /// The amortisation schedule the sheet is showing, and where in it we are.
    ///
    /// Held whole so stepping a page is a slice rather than a query, and pushed
    /// [`SCHEDULE_PAGE`] rows at a time: a twenty-year loan is 240 instalments,
    /// and building all of them as six cells each is what made the sheet take a
    /// visible moment to open and scroll badly once it had.
    schedule: Vec<tulipix_finances::loans::Instalment>,
    schedule_currency: String,
    schedule_page: usize,
}

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default())).lock().unwrap_or_else(|e| e.into_inner())
}

/// The section's pool. An async once-cell, not `OnceLock`: opening awaits, and
/// several callbacks routinely race here on the first paint.
async fn pool() -> anyhow::Result<sqlx::SqlitePool> {
    static POOL: tokio::sync::OnceCell<sqlx::SqlitePool> = tokio::sync::OnceCell::const_new();
    POOL.get_or_try_init(tulipix_finances::open).await.cloned()
}

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
        }
        // No reactor (a unit test, or a build that never entered the runtime): the
        // section simply does nothing rather than panicking the app.
        Err(_) => tracing::warn!("finances: no tokio runtime; not loading"),
    }
}

/// Raise the desktop notification for what a tick found, if anything and if the
/// user wants them.
///
/// The wording and the decision to stay quiet both live in `tulipix-finances`,
/// where they are tested; this only carries the result to the OS.
fn announce(t: &tulipix_finances::TickResult) {
    let Some((title, body)) = tulipix_finances::notice(t) else { return };
    tulipix_platform::notify::notify(
        &tulipix_platform::notify::Notification::new("finances-due", title, body)
            .category(tulipix_platform::notify::NotificationCategory::Generic)
            .deep_link("tulipix://finances/bills"),
    );
}

// ── entry points ────────────────────────────────────────────────────────────

pub fn wire(window: &MainWindow) {
    {
        let today = date::today();
        let mut st = state();
        st.budget_period = date::ym(today);
        st.cal_period = date::ym(today);
        st.bills_period = date::ym(today);
        st.cal_view = "month".into();
        st.bills_filter = "all".into();
        st.subs_filter = "all".into();
        st.dues_filter = "all".into();
    }

    // The badge has to be right before the section is ever opened, or a bill due
    // today is invisible until the user happens to look.
    let w = window.as_weak();
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        match tulipix_finances::tick(&pool).await {
            // Only the startup tick notifies. Re-entering the section is the user
            // already looking at the section, and a banner over the thing it is
            // telling you about is noise.
            Ok(t) => announce(&t),
            Err(e) => tracing::warn!("finances: tick failed: {e}"),
        }
        let _ = w.upgrade_in_event_loop(|w| refresh(&w));
    });

    let w = window.as_weak();
    window.on_fin_enter(move || {
        let Some(w) = w.upgrade() else { return };
        // Section open runs the recurrence engine again: the app may have been
        // left running across a date boundary.
        let weak = w.as_weak();
        spawn(async move {
            let Ok(pool) = pool().await else { return };
            let _ = tulipix_finances::tick(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| refresh(&w));
            // Then the rates, if they are a day old. After the refresh above, not
            // before it: the section has to be on screen while the network is
            // waited on, not after.
            refresh_rates(&weak, false).await;
        });
    });

    let w = window.as_weak();
    window.on_fin_refresh(move || {
        if let Some(w) = w.upgrade() {
            refresh(&w);
        }
    });

    let w = window.as_weak();
    window.on_fin_set_tab(move |tab| {
        let Some(w) = w.upgrade() else { return };
        w.set_fin_tab(tab);
        refresh(&w);
    });

    // ── ledger ──────────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_txn_filter(move || {
        let Some(w) = w.upgrade() else { return };
        // Any filter change returns to page one; staying on page 7 of a result set
        // that now has two pages shows an empty table.
        state().page = 0;
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_txn_sort_by(move |column| {
        let Some(w) = w.upgrade() else { return };
        // Clicking the sorted column flips the direction; a different column
        // starts descending, which is what a reader of a money table wants.
        if w.get_fin_txn_sort() == column {
            w.set_fin_txn_desc(!w.get_fin_txn_desc());
        } else {
            w.set_fin_txn_sort(column);
            w.set_fin_txn_desc(true);
        }
        state().page = 0;
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_txn_goto(move |page| {
        let Some(w) = w.upgrade() else { return };
        state().page = page.max(0) as u32;
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_txn_open(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "txn", id as i64);
        }
    });

    let w = window.as_weak();
    window.on_fin_txn_delete(move |id| act(&w, move |pool| async move {
        txn::delete(&pool, id as i64).await
    }));

    // ── obligations ─────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_pay_obligation(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "pay", id as i64);
        }
    });

    let w = window.as_weak();
    window.on_fin_skip_obligation(move |id| act(&w, move |pool| async move {
        obligations::skip(&pool, id as i64).await
    }));

    let w = window.as_weak();
    window.on_fin_unpay_obligation(move |id| act(&w, move |pool| async move {
        obligations::unpay(&pool, id as i64).await
    }));

    // ── recurrences ─────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_recur_status(move |id, status| {
        let status = status.to_string();
        act(&w, move |pool| async move {
            recur::set_status(&pool, id as i64, recur::Status::parse(&status)).await
        })
    });

    let w = window.as_weak();
    window.on_fin_recur_delete(move |id| act(&w, move |pool| async move {
        recur::delete(&pool, id as i64).await
    }));

    // ── dues ────────────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_due_settle(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "settle", id as i64);
        }
    });

    let w = window.as_weak();
    window.on_fin_due_write_off(move |id| act(&w, move |pool| async move {
        dues::write_off(&pool, id as i64, date::today()).await.map(|_| ())
    }));

    let w = window.as_weak();
    window.on_fin_due_delete(move |id| act(&w, move |pool| async move {
        dues::delete(&pool, id as i64).await
    }));

    // ── accounts and loans ──────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_account_reconcile(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "reconcile", id as i64);
        }
    });

    // A recount is a reconcile — the same form, the same posting. The word is
    // different because the physical act is: nobody "reconciles" a wallet, they
    // count what is in it.
    let w = window.as_weak();
    window.on_fin_account_recount(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "reconcile", id as i64);
        }
    });

    // Paying a card is a transfer, never an expense. Booking it as spending is
    // the second of the two mistakes that make a home-grown tracker's numbers
    // wrong — the money was already counted when it was spent on the card.
    let w = window.as_weak();
    window.on_fin_account_pay_card(move |id| {
        let Some(w) = w.upgrade() else { return };
        open_sheet(&w, "txn", 0);
        let mut st = state();
        st.prefill.insert("kind".into(), "Transfer".into());
        st.prefill.insert("to_account".into(), format!("#{id}"));
        st.prefill.insert("description".into(), "Card payment".into());
    });

    let w = window.as_weak();
    window.on_fin_account_close(move |id, closed| act(&w, move |pool| async move {
        accounts::set_closed(&pool, id as i64, closed).await
    }));

    let w = window.as_weak();
    window.on_fin_account_delete(move |id| act(&w, move |pool| async move {
        accounts::delete(&pool, id as i64).await
    }));

    let w = window.as_weak();
    window.on_fin_loan_schedule(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "schedule", id as i64);
        }
    });

    // Paging the schedule never goes back to the database: the whole thing is in
    // hand, and a page is a slice of it.
    let w = window.as_weak();
    window.on_fin_schedule_step(move |delta| {
        let Some(w) = w.upgrade() else { return };
        let (rows, page, pages) = {
            let mut st = state();
            let pages = st.schedule.len().div_ceil(SCHEDULE_PAGE).max(1);
            let next = (st.schedule_page as i64 + delta as i64).clamp(0, pages as i64 - 1) as usize;
            st.schedule_page = next;
            let from = next * SCHEDULE_PAGE;
            let to = (from + SCHEDULE_PAGE).min(st.schedule.len());
            (view::schedule(&st.schedule[from..to], &st.schedule_currency), next, pages)
        };
        w.set_fin_schedule(view::model(rows));
        w.set_fin_schedule_page(page as i32 + 1);
        w.set_fin_schedule_pages(pages as i32);
    });

    let w = window.as_weak();
    window.on_fin_loan_prepay(move |id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, "prepay", id as i64);
        }
    });

    // ── budgets ─────────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_budget_remove(move |category_id| {
        let period = state().budget_period.clone();
        act(&w, move |pool| async move {
            budgets::remove(&pool, category_id as i64, &period).await
        })
    });

    let w = window.as_weak();
    window.on_fin_budget_copy_forward(move || {
        let period = state().budget_period.clone();
        act(&w, move |pool| async move {
            let previous = previous_period(&period);
            budgets::copy_forward(&pool, &previous, &period).await.map(|_| ())
        })
    });

    let w = window.as_weak();
    window.on_fin_budget_step(move |delta| {
        let Some(w) = w.upgrade() else { return };
        {
            let mut st = state();
            st.budget_period = step_period(&st.budget_period, delta);
        }
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_cal_step(move |delta| {
        let Some(w) = w.upgrade() else { return };
        {
            let mut st = state();
            // In the year view a step is a year, not a month. Stepping one month
            // through a twelve-month summary would redraw the same twelve.
            let by = if st.cal_view == "year" { delta * 12 } else { delta };
            st.cal_period = step_period(&st.cal_period, by);
        }
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_set_cal_view(move |v| {
        let Some(w) = w.upgrade() else { return };
        state().cal_view = v.to_string();
        w.set_fin_cal_view(v);
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_bills_step(move |delta| {
        let Some(w) = w.upgrade() else { return };
        {
            let mut st = state();
            st.bills_period = step_period(&st.bills_period, delta);
        }
        refresh(&w);
    });

    // ── sheets ──────────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_open_sheet(move |kind, id| {
        if let Some(w) = w.upgrade() {
            open_sheet(&w, &kind.to_string(), id as i64);
        }
    });

    let w = window.as_weak();
    window.on_fin_close_sheet(move || {
        if let Some(w) = w.upgrade() {
            close_sheet(&w);
        }
    });

    let w = window.as_weak();
    window.on_fin_set_field(move |key, value| {
        let kind = {
            let mut st = state();
            st.form.insert(key.to_string(), value.to_string());
            st.sheet_kind.clone()
        };
        // Two fields do something the moment they change rather than on submit,
        // because the answer is what tells the user whether to change them again.
        let Some(w) = w.upgrade() else { return };
        match (kind.as_str(), key.as_str()) {
            ("import", "preset") if value.as_str() != NO_PRESET => apply_preset(&w, value.as_str()),
            // Correcting a column re-reads the file straight away. The preview
            // under it is the whole reason to correct one, and a mapping that
            // only took effect on Import would be confirmed blind.
            ("import", k) if k.starts_with("map_") => remap(&w),
            ("txn", "category") => envelope_note(&w, value.as_str()),
            _ => {}
        }
    });

    let w = window.as_weak();
    window.on_fin_submit_sheet(move || {
        if let Some(w) = w.upgrade() {
            submit(&w);
        }
    });

    let w = window.as_weak();
    window.on_fin_flag_action(move |action| {
        let Some(w) = w.upgrade() else { return };
        // Encoded as `kind:id` by `view::flags` — a string rather than two
        // properties, because two properties can disagree.
        let raw = action.to_string();
        let (kind, id) = raw.split_once(':').unwrap_or(("", "0"));
        let id: i64 = id.parse().unwrap_or(0);
        match kind {
            "obligation" => {
                w.set_fin_tab("bills".into());
                refresh(&w);
            }
            "recurrence" => {
                w.set_fin_tab("subs".into());
                refresh(&w);
            }
            "category" => {
                w.set_fin_tab("budgets".into());
                refresh(&w);
            }
            "account" => {
                w.set_fin_tab("accounts".into());
                refresh(&w);
            }
            "due" => {
                w.set_fin_tab("dues".into());
                refresh(&w);
            }
            "rates" => open_sheet(&w, "rates", 0),
            _ => {}
        }
        let _ = id;
    });

    // ── import ──────────────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_fin_import_pick(move || {
        let Some(w) = w.upgrade() else { return };
        pick_statement(&w);
    });

    let w = window.as_weak();
    window.on_fin_demo_remove(move || {
        let Some(w) = w.upgrade() else { return };
        // Set before the work starts, cleared by the refresh that follows it.
        // Both operations take long enough to look like nothing happened, and the
        // popup that started them has already closed itself.
        w.set_fin_demo_busy("Clearing the section…".into());
        let weak = w.as_weak();
        spawn(async move {
            let Ok(pool) = pool().await else { return };
            // `wipe`, not `remove_all`: the button says "remove all of it", and the
            // marker-scoped version can only reach rows the seed itself wrote —
            // anything grown from them since stayed, and held a sample account
            // open with it.
            if let Err(e) = tulipix_finances::demo::wipe(&pool).await {
                tracing::warn!("finances: could not clear the section: {e}");
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_fin_demo_busy("".into());
                refresh(&w);
            });
        });
    });

    let w = window.as_weak();
    window.on_fin_demo_add(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_fin_demo_busy("Adding sample data…".into());
        let weak = w.as_weak();
        spawn(async move {
            let Ok(pool) = pool().await else { return };
            // `reseed`, not `seed`: seeding declines on a database that has
            // anything real in it, and this is the user asking rather than the app
            // deciding. It marks only the rows it adds, so their own data is not
            // tagged as a sample and cannot be pruned away later.
            if let Err(e) = tulipix_finances::demo::reseed(&pool, tulipix_finances::date::today()).await
            {
                tracing::warn!("finances: could not add the sample data: {e}");
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_fin_demo_busy("".into());
                refresh(&w);
            });
        });
    });

    let w = window.as_weak();
    window.on_fin_set_bills_filter(move |f| {
        let Some(w) = w.upgrade() else { return };
        state().bills_filter = f.to_string();
        w.set_fin_bills_filter(f);
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_set_subs_filter(move |f| {
        let Some(w) = w.upgrade() else { return };
        state().subs_filter = f.to_string();
        w.set_fin_subs_filter(f);
        refresh(&w);
    });

    // The two dropdowns write their own properties before firing, so this only
    // has to redraw — the filter values are read back out in `refresh`.
    let w = window.as_weak();
    window.on_fin_subs_filter_changed(move || {
        if let Some(w) = w.upgrade() {
            refresh(&w);
        }
    });

    let w = window.as_weak();
    window.on_fin_set_dues_filter(move |f| {
        let Some(w) = w.upgrade() else { return };
        state().dues_filter = f.to_string();
        w.set_fin_dues_filter(f);
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_fin_find_missing(move || {
        let Some(w) = w.upgrade() else { return };
        // The account being reconciled is the sheet's subject. Filter the ledger to
        // it, drop every other filter, and close the sheet: the point is to look at
        // the rows, not to keep a dialog open over them.
        let id = state().sheet_id;
        let weak = w.as_weak();
        spawn(async move {
            let Ok(pool) = pool().await else { return };
            // The account's name, because the ledger's filter is a label rather than
            // an id — same dropdown a person would have used by hand.
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM accounts WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .ok()
                    .flatten();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_fin_sheet("".into());
                if let Some(name) = name {
                    w.set_fin_txn_account(name.into());
                }
                w.set_fin_txn_category(ALL_CATEGORIES.into());
                w.set_fin_txn_kind("All".into());
                w.set_fin_txn_search("".into());
                state().page = 0;
                w.set_fin_tab("txns".into());
                refresh(&w);
            });
        });
    });

    let w = window.as_weak();
    window.on_fin_rates_refresh(move || {
        let Some(w) = w.upgrade() else { return };
        let weak = w.as_weak();
        w.set_fin_rates_busy(true);
        spawn(async move {
            refresh_rates(&weak, true).await;
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_fin_rates_busy(false);
                refresh(&w);
                // The rate list is built when the sheet opens, not by `refresh`, so
                // without this the button fetches new rates and shows the old ones.
                if w.get_fin_sheet() == "rates" {
                    open_sheet(&w, "rates", 0);
                }
            });
        });
    });

    let w = window.as_weak();
    window.on_fin_scan_receipt(move || {
        let Some(w) = w.upgrade() else { return };
        scan_receipt(&w);
    });

    let w = window.as_weak();
    window.on_fin_submit_again(move || {
        let Some(w) = w.upgrade() else { return };
        submit_with(&w, true);
    });

    let w = window.as_weak();
    window.on_fin_accept_proposal(move |idx| {
        let Some(w) = w.upgrade() else { return };
        let Some(p) = state().proposals.get(idx.max(0) as usize).cloned() else { return };
        let weak = w.as_weak();
        spawn(async move {
            let Ok(pool) = pool().await else { return };
            if let Err(e) = detect::accept(&pool, &p).await {
                tracing::warn!("finances: accepting a proposal failed: {e}");
            }
            // Re-run detection so the accepted one leaves the list.
            let fresh = detect::propose(&pool).await.unwrap_or_default();
            let rows = view::proposals(&fresh);
            state().proposals = fresh;
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Some(extra) = replace_rows(&w.get_fin_proposals(), rows) {
                    w.set_fin_proposals(view::model(extra));
                }
                refresh(&w);
            });
        });
    });
}

/// Called from the sidebar hook. Runs the calendar catch-up on entry.
pub fn section_changed(window: &MainWindow, section: &str) {
    if section == "finances" {
        window.invoke_fin_enter();
    }
}

// ── exchange rates ──────────────────────────────────────────────────────────

/// Fetch today's rates and store them, then redraw.
///
/// The only network call in this section, and the only reason this crate needs an
/// HTTP client. It lives here rather than in `tulipix-finances` so that crate stays
/// testable with no network at all: it builds the URL and parses the response, and
/// the twenty lines between those two are here.
///
/// `force` is the Refresh button. Without it nothing happens unless the daily
/// refresh is enabled and the stored rates are actually a day old — a section that
/// re-fetched on every visit would be making the same request twenty times an hour.
///
/// Every failure is a warning in the log and nothing on screen. A rate that could
/// not be refreshed is not an error the user can act on, the previous rate is still
/// there and still correct enough to convert with, and the sheet says how old it is.
async fn refresh_rates(weak: &slint::Weak<MainWindow>, force: bool) {
    let Ok(pool) = pool().await else { return };
    if !force && !tulipix_finances::fx::auto_enabled() {
        return;
    }
    let now = tulipix_finances::schema::unix_now();
    match tulipix_finances::fx::stale(&pool, now).await {
        Ok(true) => {}
        Ok(false) if !force => return,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("finances: could not tell whether rates are stale: {e}");
            return;
        }
    }

    let base = tulipix_finances::fx::base_currency();
    let url = tulipix_finances::fx::endpoint(&base);
    let body = match reqwest::Client::builder()
        // A rate is worth a few seconds and no more: this runs on section open, and
        // a hanging request must not leave a spinner up for a minute.
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
    let Some(body) = body else { return };

    match tulipix_finances::fx::parse_rates(&body, &base) {
        Ok(rates) => match tulipix_finances::fx::apply_live(&pool, &rates, now).await {
            Ok(0) => tracing::debug!("finances: rates fetched, nothing needed updating"),
            Ok(n) => {
                tracing::info!("finances: {n} rate(s) updated");
                let _ = weak.upgrade_in_event_loop(|w| refresh(&w));
            }
            Err(e) => tracing::warn!("finances: could not store rates: {e}"),
        },
        Err(e) => tracing::warn!("finances: unusable rates response: {e}"),
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Run a database action, then refresh.
///
/// Failures are logged rather than shown: these are the one-click row actions
/// (delete, skip, pause), where the outcome is visible in the list that reappears
/// a moment later. The sheets, where a user has typed something that can be
/// wrong, report their errors on screen instead — see [`submit`].
fn act<F, Fut>(weak: &slint::Weak<MainWindow>, f: F)
where
    F: FnOnce(sqlx::SqlitePool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    let weak = weak.clone();
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        if let Err(e) = f(pool).await {
            tracing::warn!("finances: {e}");
        }
        let _ = weak.upgrade_in_event_loop(|w| refresh(&w));
    });
}

fn step_period(period: &str, delta: i32) -> String {
    let Ok((first, _)) = date::month_bounds(period) else { return period.to_string() };
    let Ok(first) = date::parse(&first) else { return period.to_string() };
    let moved = if delta >= 0 {
        date::add_months_anchored(first, delta as u32, 1)
    } else {
        // No negative months in the helper, so step back a day at a time from the
        // 1st: one day before the 1st is the previous month.
        let mut d = first;
        for _ in 0..(-delta) {
            d = d.pred_opt().unwrap_or(d);
            d = d.with_day(1).unwrap_or(d);
        }
        d
    };
    date::ym(moved)
}

fn previous_period(period: &str) -> String {
    step_period(period, -1)
}

fn pick_id(pairs: &[(String, i64)], label: &str) -> Option<i64> {
    pairs.iter().find(|(n, _)| n == label).map(|(_, id)| *id)
}

// ── sheets ──────────────────────────────────────────────────────────────────

fn close_sheet(window: &MainWindow) {
    {
        let mut st = state();
        st.sheet_kind.clear();
        st.sheet_id = 0;
        st.form.clear();
        st.import = None;
        st.schedule.clear();
        st.schedule_page = 0;
    }
    window.set_fin_sheet("".into());
    window.set_fin_sheet_error("".into());
    window.set_fin_sheet_note("".into());
    window.set_fin_form(view::model(Vec::new()));
    window.set_fin_prices(view::model(Vec::new()));
    window.set_fin_schedule(view::model(Vec::new()));
    window.set_fin_import_rows(view::model(Vec::new()));
    window.set_fin_proposals(view::model(Vec::new()));
    window.set_fin_rates(view::model(Vec::new()));
}

/// A `FinField` carries a `ModelRc` for its dropdown options, and `Rc` is not
/// `Send`, so this conversion must happen *inside* the `upgrade_in_event_loop`
/// closure. Build the plain `sheet::Field`s off-thread and convert on the UI
/// thread; a `Vec<FinField>` cannot cross the boundary.
fn to_fields(fields: &[sheet::Field]) -> Vec<FinField> {
    fields
        .iter()
        .map(|f| FinField {
            key: f.key.clone().into(),
            label: f.label.clone().into(),
            kind: f.kind.clone().into(),
            value: f.value.clone().into(),
            hint: f.hint.clone().into(),
            options: view::model(f.options.iter().map(|o| o.clone().into()).collect()),
            required: f.required,
        })
        .collect()
}

fn open_sheet(window: &MainWindow, kind: &str, id: i64) {
    let kind = kind.to_string();
    window.set_fin_sheet_error("".into());
    window.set_fin_sheet_busy(true);
    window.set_fin_sheet(kind.clone().into());

    let weak = window.as_weak();
    let period = state().budget_period.clone();
    spawn(async move {
        let Ok(pool) = pool().await else { return };

        // The sheets that are not plain forms: read-only lists, or ones whose
        // content is cached state rather than a database row.
        match kind.as_str() {
            "rates" => {
                let base = tulipix_finances::fx::base_currency();
                let rows =
                    tulipix_finances::fx::monitor(&pool, &base).await.unwrap_or_default();
                let rates = view::rates(&rows, &base);
                let checked = tulipix_finances::fx::last_checked(&pool)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|at| chrono::DateTime::<chrono::Utc>::from_timestamp(at, 0))
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local).format("%d %b %H:%M").to_string()
                    })
                    .unwrap_or_else(|| "never".to_string());
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title("Exchange rates".into());
                    w.set_fin_sheet_hint(
                        format!(
                            "Everything is reported in {base}. Checked once a day — last checked {checked}."
                        )
                        .into(),
                    );
                    w.set_fin_sheet_primary("Add or update".into());
                    w.set_fin_rates(view::model(rates));
                    w.set_fin_form(view::model(to_fields(&[])));
                    w.set_fin_sheet_busy(false);
                });
                // The three fields the rate editor needs are seeded separately so
                // the list above is not rebuilt on every keystroke.
                let base_now = tulipix_finances::fx::base_currency();
                state().form.insert("base".into(), base_now.clone());
                let fields = vec![
                    sheet::Field {
                        key: "base".into(),
                        label: "Base currency".into(),
                        kind: "text".into(),
                        value: base_now,
                        hint: "Every total is reported in this. Changing it re-reports, and rewrites nothing.".into(),
                        options: Vec::new(),
                        required: true,
                    },
                    sheet::Field {
                        key: "code".into(),
                        label: "Currency".into(),
                        kind: "text".into(),
                        value: String::new(),
                        hint: "Three-letter code, e.g. USD.".into(),
                        options: Vec::new(),
                        required: false,
                    },
                    sheet::Field {
                        key: "rate".into(),
                        label: "One unit is worth".into(),
                        kind: "money".into(),
                        value: String::new(),
                        hint: "e.g. 83.60".into(),
                        options: Vec::new(),
                        required: false,
                    },
                ];
                let _ = weak
                    .upgrade_in_event_loop(move |w| w.set_fin_form(view::model(to_fields(&fields))));
                return;
            }
            "prices" => {
                let rows = recur::price_history(&pool, id).await.unwrap_or_default();
                let name = recur::get(&pool, id)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.name)
                    .unwrap_or_default();
                let prices = view::prices(&rows);
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title(format!("{name} — what it has cost").into());
                    w.set_fin_sheet_hint(
                        "Recorded every time the amount changed. This is the only reason a quiet price rise is visible at all.".into(),
                    );
                    // Nothing to submit: this is a record, not a form.
                    w.set_fin_sheet_primary("".into());
                    w.set_fin_prices(view::model(prices));
                    w.set_fin_sheet_busy(false);
                });
                return;
            }
            "schedule" => {
                let rows = loans::schedule_for(&pool, id).await.unwrap_or_default();
                let loan = loans::get(&pool, id).await.ok().flatten();
                let currency = loan.as_ref().map(|l| l.currency.clone()).unwrap_or_else(|| "INR".into());
                let name = loan.as_ref().map(|l| l.name.clone()).unwrap_or_default();
                let interest = loans::total_interest(&rows);
                let pages = rows.len().div_ceil(SCHEDULE_PAGE).max(1);
                let ui = view::schedule(&rows[..SCHEDULE_PAGE.min(rows.len())], &currency);
                {
                    let mut st = state();
                    st.schedule = rows;
                    st.schedule_currency = currency.clone();
                    st.schedule_page = 0;
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title(format!("{name} — amortisation").into());
                    w.set_fin_sheet_hint(
                        format!(
                            "{} of interest over the whole term. The last instalment is trimmed to clear the balance exactly.",
                            money::format_minor(interest, &currency)
                        )
                        .into(),
                    );
                    w.set_fin_sheet_primary("".into());
                    w.set_fin_schedule(view::model(ui));
                    w.set_fin_schedule_page(1);
                    w.set_fin_schedule_pages(pages as i32);
                    w.set_fin_sheet_busy(false);
                });
                return;
            }
            "prepay" => {
                let loan = loans::get(&pool, id).await.ok().flatten();
                let Some(l) = loan else { return };
                let fields = vec![
                    sheet::Field {
                        key: "extra".into(),
                        label: "Lump sum".into(),
                        kind: "money".into(),
                        value: String::new(),
                        hint: "Paid off the principal, keeping the EMI the same.".into(),
                        options: Vec::new(),
                        required: true,
                    },
                    sheet::Field {
                        key: "after_n".into(),
                        label: "After how many instalments".into(),
                        kind: "number".into(),
                        value: l.paid_count.to_string(),
                        hint: String::new(),
                        options: Vec::new(),
                        required: true,
                    },
                ];
                let name = l.name.clone();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title(format!("{name} — what if I prepay?").into());
                    w.set_fin_sheet_hint(
                        "Shortens the term rather than reducing the EMI, which is what saves interest."
                            .into(),
                    );
                    w.set_fin_sheet_primary("Work it out".into());
                    w.set_fin_form(view::model(to_fields(&fields)));
                    w.set_fin_sheet_busy(false);
                });
                return;
            }
            "detect" => {
                let found = detect::propose(&pool).await.unwrap_or_default();
                let ui = view::proposals(&found);
                let n = found.len();
                state().proposals = found;
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title("Charges that look like they repeat".into());
                    w.set_fin_sheet_hint(
                        if n == 0 {
                            "Nothing in the ledger repeats predictably yet. Import a few months of statements and this gets much better.".into()
                        } else {
                            String::new()
                        }
                        .into(),
                    );
                    w.set_fin_sheet_primary("".into());
                    w.set_fin_proposals(view::model(ui));
                    w.set_fin_sheet_busy(false);
                });
                return;
            }
            _ => {}
        }

        // Plain forms.
        match sheet::build(&pool, &kind, id).await {
            Ok(mut built) => {
                // Seed the state with the built-in values, so a field the user
                // never touches still submits what was shown to them.
                {
                    let mut st = state();
                    st.sheet_kind = kind.clone();
                    st.sheet_id = id;
                    // Anything a caller asked to pre-fill wins over the defaults,
                    // and is applied to the fields themselves so the user can see
                    // and correct it.
                    let prefill = std::mem::take(&mut st.prefill);
                    for f in built.fields.iter_mut() {
                        let Some(v) = prefill.get(&f.key) else { continue };
                        // A caller with an id but no name asks for `#7`. Resolved
                        // against the field's own options here, because a value
                        // that is not one of them shows in the box and cannot be
                        // selected back once the user opens it.
                        f.value = match v.strip_prefix('#').and_then(|i| i.parse::<i64>().ok()) {
                            Some(id) => f
                                .options
                                .iter()
                                .find(|o| sheet::id_from_label(o) == Some(id))
                                .cloned()
                                .unwrap_or_else(|| f.value.clone()),
                            None => v.clone(),
                        };
                    }
                    st.form.clear();
                    for f in &built.fields {
                        st.form.insert(f.key.clone(), f.value.clone());
                    }
                    // Values with no field of their own: the period a budget is
                    // keyed on, and the provenance of an OCR'd receipt.
                    st.form.insert("period_hidden".into(), period.clone());
                    for (k, v) in prefill.iter().filter(|(k, _)| k.ends_with("_hint")) {
                        st.form.insert(k.clone(), v.clone());
                    }
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title(built.title.into());
                    w.set_fin_sheet_hint(built.hint.into());
                    w.set_fin_sheet_primary(built.primary.into());
                    w.set_fin_form(view::model(to_fields(&built.fields)));
                    w.set_fin_sheet_busy(false);
                });
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_error(msg.into());
                    w.set_fin_sheet_busy(false);
                });
            }
        }
    });
}

fn submit(window: &MainWindow) {
    submit_with(window, false);
}

/// Submit the open sheet. `again` reopens a fresh one of the same kind instead of
/// closing, which is what makes entering a morning's worth of cash spends
/// bearable.
fn submit_with(window: &MainWindow, again: bool) {
    let (kind, id, form, import) = {
        let st = state();
        // Cloned, not taken: an import whose date order is still unsettled has to
        // survive the round trip so the file can be re-read rather than re-picked.
        (st.sheet_kind.clone(), st.sheet_id, st.form.clone(), st.import.clone())
    };
    let sheet_kind = window.get_fin_sheet().to_string();
    window.set_fin_sheet_busy(true);
    window.set_fin_sheet_error("".into());

    let weak = window.as_weak();
    spawn(async move {
        let Ok(pool) = pool().await else { return };

        // Sheets whose submit is not `sheet::submit`.
        let result: anyhow::Result<Option<String>> = match sheet_kind.as_str() {
            "rates" => {
                // The sheet does two jobs: pick the base currency, and add or
                // update one rate. Either alone is a valid submit, so a blank
                // code with a changed base is not an error.
                let base_now = tulipix_finances::fx::base_currency();
                let want_base = form.get("base").cloned().unwrap_or_default().trim().to_uppercase();
                let base_changed = !want_base.is_empty() && want_base != base_now;

                let code = form.get("code").cloned().unwrap_or_default().trim().to_uppercase();
                let raw = form.get("rate").cloned().unwrap_or_default();
                let want_rate = !code.is_empty() || !raw.trim().is_empty();

                let mut outcome: anyhow::Result<Option<String>> = Ok(None);
                if base_changed {
                    outcome = tulipix_finances::fx::set_base_currency(&want_base).map(|_| None);
                }
                if outcome.is_ok() && want_rate {
                    outcome = if code.len() != 3 {
                        Err(anyhow::anyhow!("a currency code is three letters, e.g. USD"))
                    } else {
                        match money::parse_rate(&raw) {
                            Ok(micro) if micro > 0 => tulipix_finances::fx::set(
                                &pool,
                                &code,
                                micro,
                                tulipix_finances::schema::unix_now(),
                            )
                            .await
                            .map(|_| None),
                            _ => Err(anyhow::anyhow!("{raw:?} is not a rate — try 83.60")),
                        }
                    };
                }
                if outcome.is_ok() && !base_changed && !want_rate {
                    outcome = Err(anyhow::anyhow!("nothing to save — set a base currency or a rate"));
                }
                outcome
            }
            "prepay" => {
                let loan = loans::get(&pool, id).await.ok().flatten();
                match loan {
                    None => Err(anyhow::anyhow!("that loan is gone")),
                    Some(l) => {
                        let extra = form
                            .get("extra")
                            .and_then(|v| money::parse_amount(v, &l.currency).ok())
                            .unwrap_or(0);
                        let after: i64 =
                            form.get("after_n").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
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
                                "{} off after {after} instalments finishes the loan {} months early and saves {} in interest.",
                                money::format_minor(extra, &l.currency),
                                p.months_saved,
                                money::format_minor(p.interest_saved_minor, &l.currency)
                            ))),
                            None => Err(anyhow::anyhow!(
                                "that is either nothing or more than the balance — there would be no schedule left to model"
                            )),
                        }
                    }
                }
            }
            "import" => match import {
                None => Err(anyhow::anyhow!("that file is no longer loaded")),
                // The date order was ambiguous and the user has now chosen: re-read
                // the file with it rather than importing rows that were never
                // parsed.
                Some((preview, account_id, text)) if preview.date_format.is_none() => {
                    let chosen = form
                        .get("date_format")
                        .and_then(|v| DateFormat::parse_name(v))
                        .unwrap_or(DateFormat::DayFirst);
                    let currency = preview.currency.clone();
                    // The origin kind survives the re-run: this is the same file,
                    // read again with the date order the user just chose.
                    let was = preview.kind;
                    match import::preview(&text, &currency, Some(&preview.column_map), Some(chosen)) {
                        Ok(mut fresh) => {
                            if was.converted() {
                                fresh.kind = was;
                            }
                            let rows = view::import_rows(&fresh.rows, &fresh.currency, IMPORT_PREVIEW_ROWS);
                            let n = fresh.rows.len();
                            let span = fresh.span();
                            state().import = Some((fresh, account_id, text));
                            let _ = weak.upgrade_in_event_loop(move |w| {
                                w.set_fin_import_needs_format(false);
                                w.set_fin_sheet_primary("Import".into());
                                w.set_fin_import_summary(
                                    format!(
                                        "{n} rows{}, read as {}.",
                                        span.map(|(a, b)| format!(", {a} to {b}")).unwrap_or_default(),
                                        chosen.as_str()
                                    )
                                    .into(),
                                );
                                if let Some(extra) = replace_rows(&w.get_fin_import_rows(), rows) {
                                    w.set_fin_import_rows(view::model(extra));
                                }
                                w.set_fin_sheet_busy(false);
                            });
                            return;
                        }
                        Err(e) => Err(e),
                    }
                }
                Some((preview, account_id, _)) => {
                    // Saved before the ingest, so a mapping that took three tries
                    // to get right is not lost if the posting itself fails.
                    if let Some(name) = form.get("remember_as").map(|s| s.trim()).filter(|s| !s.is_empty())
                        && let Some(fmt) = preview.date_format
                    {
                        if let Err(e) = import::save_preset(
                            &pool,
                            name,
                            &preview.column_map,
                            fmt,
                            Some(account_id),
                        )
                        .await
                        {
                            tracing::warn!("finances: could not save import preset: {e}");
                        }
                    }
                    match import::ingest(&pool, account_id, &preview).await {
                        // What the rows became, not just how many. Every line is
                        // something the user would otherwise have to go and check:
                        // which rows still need a category, which bills the file
                        // closed off, and whether the account now agrees with the
                        // statement it came from.
                        Ok(got) => {
                            let base = tulipix_finances::fx::base_currency();
                            let mut parts = vec![format!(
                                "{} added, {} already there.",
                                got.added, got.duplicates
                            )];
                            if got.categorised > 0 {
                                parts.push(format!(
                                    "{} categorised from what you filed the same merchants under before.",
                                    got.categorised
                                ));
                            }
                            if got.uncategorised > 0 {
                                parts.push(format!(
                                    "{} still need a category.",
                                    got.uncategorised
                                ));
                            }
                            if got.matched_bills > 0 {
                                parts.push(format!(
                                    "{} matched an open bill and closed it.",
                                    got.matched_bills
                                ));
                            }
                            match got.balance_gap_minor {
                                Some(0) => parts.push(
                                    "Closing balance matches the ledger.".to_string(),
                                ),
                                Some(gap) => parts.push(format!(
                                    "Closing balance is {} off the ledger — something is missing or duplicated.",
                                    money::format_minor(gap.abs(), &base)
                                )),
                                None => {}
                            }
                            Ok(Some(parts.join(" ")))
                        }
                        Err(e) => Err(e),
                    }
                }
            },
            _ => sheet::submit(&pool, &kind, id, &form).await.map(|_| None),
        };

        match result {
            // A result to show: the sheet stays open with the answer in it. That
            // is the whole point of the prepayment and import sheets.
            Ok(Some(message)) => {
                // An import's report is the point of having run it — closing the
                // sheet on it would throw away the one moment the user could act
                // on "8 still need a category".
                let stays_open = sheet_kind == "prepay" || sheet_kind == "import";
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_busy(false);
                    if stays_open {
                        w.set_fin_sheet_hint(message.into());
                    } else if again {
                        reopen(&w, &sheet_kind);
                    } else {
                        close_sheet(&w);
                    }
                    refresh(&w);
                });
            }
            Ok(None) => {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_busy(false);
                    if again {
                        reopen(&w, &sheet_kind);
                    } else {
                        close_sheet(&w);
                    }
                    refresh(&w);
                });
            }
            // Shown on screen, not logged: the user typed this and is the only one
            // who can fix it.
            Err(e) => {
                let msg = e.to_string();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_busy(false);
                    w.set_fin_sheet_error(msg.into());
                });
            }
        }
    });
}

// ── receipts ────────────────────────────────────────────────────────────────

/// Photo of a receipt → the add sheet, with what could be read filled in.
///
/// The sheet opens either way. A receipt that read badly still saves the user the
/// account, date and category, and the fields it did fill are corrections away
/// from right — which is the only safe shape for this: an OCR'd total is a guess,
/// and a guess must pass under human eyes before it becomes a ledger row.
fn scan_receipt(window: &MainWindow) {
    if !tulipix_finances::ocr::available() {
        // The message has to go somewhere the user is looking, and the note only
        // renders inside a sheet — so open the one they were heading for anyway.
        open_sheet(window, "txn", 0);
        window.set_fin_sheet_note(
            "Reading receipts needs `tesseract` on PATH. Install tesseract and its English data to use the camera route; until then this is the form to type into."
                .into(),
        );
        return;
    }

    let weak = window.as_weak();
    spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Choose a photo of the receipt")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        let base = tulipix_finances::fx::base_currency();

        // OCR is a subprocess doing real work on a multi-megapixel photo, so it
        // goes to the blocking pool rather than stalling a reactor thread.
        let read = tokio::task::spawn_blocking(move || tulipix_finances::ocr::read(&path, &base))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);

        match read {
            Ok(receipt) => {
                let currency = tulipix_finances::fx::base_currency();
                let amount = receipt
                    .amount_minor
                    .map(|m| sheet::minor_to_input(m, &currency))
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

                let _ = weak.upgrade_in_event_loop(move |w| {
                    // The ordinary add sheet, pre-filled. Building it is async, so
                    // the values go through `prefill` rather than being written
                    // after the call and overwritten by the build.
                    {
                        let mut st = state();
                        st.prefill.clear();
                        if !amount.is_empty() {
                            st.prefill.insert("amount".into(), amount);
                        }
                        if !merchant.is_empty() {
                            st.prefill.insert("description".into(), merchant);
                        }
                        st.prefill.insert("occurred_on".into(), on);
                        st.prefill.insert("source_hint".into(), "ocr".into());
                    }
                    open_sheet(&w, "txn", 0);
                    w.set_fin_sheet_note(note.into());
                });
            }
            Err(e) => {
                let msg = e.to_string();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    // Still the add sheet: a failed read is a reason to type it in,
                    // not a dead end.
                    open_sheet(&w, "txn", 0);
                    w.set_fin_sheet_error(msg.into());
                });
            }
        }
    });
}

// ── import ──────────────────────────────────────────────────────────────────

fn pick_statement(window: &MainWindow) {
    let weak = window.as_weak();
    spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .set_title("Choose a bank statement")
            .add_filter("Statements", import::EXTENSIONS)
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        let Ok(pool) = pool().await else { return };

        // Read as bytes, not as text: a PDF or a spreadsheet is binary, and even a
        // CSV is often Windows-encoded. `read_file` decides what the file is by its
        // contents and converts PDFs and spreadsheets to CSV, so everything below
        // this line works on one shape.
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("finances: could not read {}: {e}", path.display());
                return;
            }
        };
        let (text, kind, skipped) = match import::read_file(&bytes) {
            Ok(t) => t,
            Err(e) => {
                // The one import failure worth showing rather than logging: the user
                // chose this file, and "it is a scan, not a text PDF" is something
                // only they can do anything about.
                let msg = format!("{e}");
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet("import".into());
                    w.set_fin_sheet_title("That file could not be read".into());
                    w.set_fin_sheet_hint(msg.into());
                    w.set_fin_sheet_primary("".into());
                });
                return;
            }
        };

        show_preview(weak, &pool, text, None, None, None, Some((kind, skipped))).await;
    });
}

/// Label for "do not use a saved mapping".
const NO_PRESET: &str = "None";

/// Preview a statement and put the import sheet on screen.
///
/// Shared by the file picker and by choosing a saved preset, which is the same
/// operation with a mapping supplied — the alternative was two copies of the
/// summary wording drifting apart.
///
/// `preset` names the account and mapping to use; without one the statement lands
/// in the first real account and the mapping is sniffed.
async fn show_preview(
    weak: slint::Weak<MainWindow>,
    pool: &sqlx::SqlitePool,
    text: String,
    preset: Option<import::Preset>,
    // A mapping the user has corrected by hand, which outranks the preset's.
    override_map: Option<import::csv::ColumnMap>,
    override_format: Option<DateFormat>,
    // What the file was before it became this text, for a PDF or a spreadsheet, and
    // how many of its lines the converter could not use. `None` for a file that was
    // already CSV or OFX. Carried rather than re-sniffed because the text in hand is
    // CSV either way, and the preview has to say which it really was.
    origin: Option<(import::FileKind, usize)>,
) {
    let all = accounts::list(pool, false).await.unwrap_or_default();
    let account = preset
        .as_ref()
        .and_then(|p| p.account_id)
        .and_then(|id| all.iter().find(|a| a.id == id).cloned())
        .or_else(|| all.iter().find(|a| a.kind != accounts::AccountKind::Virtual).cloned());
    let Some(account) = account else {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_fin_sheet("import".into());
            w.set_fin_sheet_title("Nowhere to put it".into());
            w.set_fin_sheet_hint(
                "Add an account first — an imported statement has to belong to one.".into(),
            );
            w.set_fin_sheet_primary("".into());
        });
        return;
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
            let rows = view::import_rows(&p.rows, &p.currency, IMPORT_PREVIEW_ROWS);
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
                    // A converted file says so. Column positions in a PDF are worked
                    // out from where the text happened to land, so this is the one
                    // case where checking the rows before importing really matters.
                    if p.kind.converted() {
                        format!(" Read from a {} — check the columns.", p.kind.as_str())
                    } else {
                        String::new()
                    }
                )
            };
            let account_name = account.name.clone();
            let currency = p.currency.clone();
            // The map itself crosses to the UI thread; the `FinField`s it becomes
            // are built on the other side, because a FinField owns a ModelRc and a
            // ModelRc is an Rc.
            let column_map = p.column_map.clone();
            // The file's own column names, so the mapping rows can be dropdowns
            // the user corrects rather than a guess they can only read. An OFX
            // states its own structure and has none, which is the signal to show
            // the mapping read-only.
            let file_headers =
                import::csv::headers(&text).unwrap_or_default();
            // With no format settled there is nothing to import yet, so the
            // button re-reads the file with the chosen order instead.
            let primary = if needs_format { "Read it again" } else { "Import" };

            let mut fields = Vec::new();
            if needs_format {
                fields.push(sheet::Field {
                    key: "date_format".into(),
                    label: "Date order".into(),
                    kind: "dropdown".into(),
                    value: DateFormat::DayFirst.as_str().into(),
                    hint: String::new(),
                    options: vec![
                        DateFormat::DayFirst.as_str().into(),
                        DateFormat::MonthFirst.as_str().into(),
                    ],
                    required: true,
                });
            }
            // Only offered once something is saved: a dropdown whose only entry
            // is "None" is a control that does nothing.
            if !saved.is_empty() {
                fields.push(sheet::Field {
                    key: "preset".into(),
                    label: "Saved bank".into(),
                    kind: "dropdown".into(),
                    value: chosen_label,
                    hint: "Re-reads the file with that bank's column mapping.".into(),
                    options: preset_labels,
                    required: false,
                });
            }
            fields.push(sheet::Field {
                key: "remember_as".into(),
                label: "Remember this bank as".into(),
                kind: "text".into(),
                value: preset.as_ref().map(|p| p.bank_name.clone()).unwrap_or_default(),
                hint: "Optional. Saves this mapping so the next statement imports in one click."
                    .into(),
                required: false,
                options: Vec::new(),
            });

            {
                let mut st = state();
                st.sheet_kind = "import".into();
                st.sheet_id = account.id;
                st.form.clear();
                // The form map has to start out agreeing with what is on screen,
                // or a field the user never touches submits as empty.
                for f in &fields {
                    st.form.insert(f.key.clone(), f.value.clone());
                }
                // The mapping rows live in their own model rather than in `form`,
                // so seed them here too — otherwise correcting one column would
                // read every other one as untouched and re-derive it from the
                // guess the user is in the middle of correcting.
                for f in view::import_map(&column_map, &file_headers) {
                    st.form.insert(f.key.to_string(), f.value.to_string());
                }
                st.import = Some((p, account.id, text));
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_fin_sheet("import".into());
                w.set_fin_sheet_title("Import a statement".into());
                w.set_fin_sheet_hint(
                    format!("Into {account_name}, read as {currency}. Nothing is posted until you confirm, and re-importing an overlapping month adds nothing.")
                        .into(),
                );
                w.set_fin_sheet_primary(primary.into());
                w.set_fin_import_needs_format(needs_format);
                w.set_fin_import_summary(summary.into());
                w.set_fin_import_rows(view::model(rows));
                w.set_fin_import_map(view::model(view::import_map(&column_map, &file_headers)));
                w.set_fin_form(view::model(to_fields(&fields)));
                w.set_fin_sheet_busy(false);
            });
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_fin_sheet("import".into());
                w.set_fin_sheet_title("That file could not be read".into());
                w.set_fin_sheet_hint(msg.into());
                w.set_fin_sheet_primary("".into());
            });
        }
    }
}

/// Blank sheet of the same kind, for "Save & add another". Id 0 means "new",
/// which is what makes it a fresh form rather than an edit of what was just
/// saved.
fn reopen(window: &MainWindow, kind: &str) {
    window.set_fin_sheet_note("".into());
    open_sheet(window, kind, 0);
}

/// What picking a category means for that category's envelope this month.
///
/// A note beside the form rather than a rebuilt field list: rebuilding the model
/// mid-typing would take the caret out of whatever the user was writing.
fn envelope_note(window: &MainWindow, category_label: &str) {
    let Some(cat) = sheet::id_from_label(category_label) else {
        window.set_fin_sheet_note("".into());
        return;
    };
    let period = state().budget_period.clone();
    let weak = window.as_weak();
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        let base = tulipix_finances::fx::base_currency();
        let row = budgets::list(&pool, &period)
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.category_id == cat);
        let note = match row {
            // No envelope on this category is not a problem to report — most
            // categories will never have one.
            None => String::new(),
            // `allowance_minor` rather than `amount_minor`: a rolled-over surplus
            // is part of what is actually available to spend.
            Some(b) if b.id.is_some() => format!(
                "{} envelope: {} of {} used this month.",
                b.category_name,
                money::format_minor(b.spent_minor, &base),
                money::format_minor(b.allowance_minor(), &base)
            ),
            // Spending exists in this category but no envelope has been set on it.
            Some(_) => String::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_fin_sheet_note(note.into()));
    });
}

/// Re-read the statement already in hand with a saved bank's mapping.
fn apply_preset(window: &MainWindow, label: &str) {
    let Some(id) = sheet::id_from_label(label) else { return };
    let Some((cached, _, text)) = state().import.clone() else { return };
    // The file is still the file it was; only the mapping changed.
    let origin = cached.kind.converted().then_some((cached.kind, 0));
    let weak = window.as_weak();
    window.set_fin_sheet_busy(true);
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        let found = import::presets(&pool).await.unwrap_or_default().into_iter().find(|p| p.id == id);
        show_preview(weak, &pool, text, found, None, None, origin).await;
    });
}

/// Re-read the loaded file with the mapping the user has just corrected.
///
/// The preview is the point of the mapping rows: getting debit and credit the
/// wrong way round inverts a whole statement and every total still looks
/// plausible, so a correction has to show its effect before anything posts.
fn remap(window: &MainWindow) {
    let Some((cached, _, text)) = state().import.clone() else { return };
    let corrected = {
        let st = state();
        view::column_map_from(&cached.column_map, |k| st.form.get(k).cloned())
    };
    if corrected == cached.column_map {
        return;
    }
    // The file has not changed, only how it is being read — so the date order
    // already settled on stays settled, and a converted file stays converted.
    let fmt = cached.date_format;
    let origin = cached.kind.converted().then_some((cached.kind, 0));
    let weak = window.as_weak();
    window.set_fin_sheet_busy(true);
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        show_preview(weak, &pool, text, None, Some(corrected), fmt, origin).await;
    });
}

// ── refresh ─────────────────────────────────────────────────────────────────

/// Everything the section shows, in one pass.
pub fn refresh(window: &MainWindow) {
    let filter_search = window.get_fin_txn_search().to_string();
    let filter_account = window.get_fin_txn_account().to_string();
    let filter_category = window.get_fin_txn_category().to_string();
    let filter_kind = window.get_fin_txn_kind().to_string();
    let filter_period = window.get_fin_txn_period().to_string();
    let filter_source = window.get_fin_txn_source().to_string();
    let subs_category = window.get_fin_subs_category().to_string();
    let subs_currency = window.get_fin_subs_currency().to_string();
    let sort = TxnSort::parse(&window.get_fin_txn_sort().to_lowercase());
    let desc = window.get_fin_txn_desc();

    let weak = window.as_weak();
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        let base = tulipix_finances::fx::base_currency();
        let today = date::today();

        let (
            page_no,
            budget_period,
            cal_period,
            cal_view,
            bills_period,
            bills_filter,
            subs_filter,
            dues_filter,
        ) = {
            let st = state();
            (
                st.page,
                st.budget_period.clone(),
                st.cal_period.clone(),
                st.cal_view.clone(),
                st.bills_period.clone(),
                st.bills_filter.clone(),
                st.subs_filter.clone(),
                st.dues_filter.clone(),
            )
        };

        // Filter dropdowns, and the maps that let a chosen label become an id.
        let account_rows = accounts::list(&pool, false).await.unwrap_or_default();
        let account_pairs: Vec<(String, i64)> =
            account_rows.iter().map(|a| (a.name.clone(), a.id)).collect();
        let category_rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, name FROM categories ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        let category_pairs: Vec<(String, i64)> =
            category_rows.iter().map(|(id, n)| (n.clone(), *id)).collect();

        let account_id = pick_id(&account_pairs, &filter_account);
        let category_id = pick_id(&category_pairs, &filter_category);

        // The month filter narrows in SQL, not after paging: filtering a page
        // that was already cut to 25 rows would show four of them and call it
        // page one of fifty.
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
            search: Some(filter_search).filter(|s| !s.trim().is_empty()),
            source: Some(filter_source).filter(|s| s != ANY_SOURCE),
        };
        // Every window this pass needs, worked out before a single query goes
        // out. Pure date arithmetic, so none of the reads below depends on
        // another one and they can all be in flight at once.
        let running_shown = account_id.is_some() && sort == TxnSort::Date;
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
        // The Budgets tab's own month. Its own query rather than the Bills tab's
        // window: the two tabs step independently, and a "not budgeted" list for a
        // different month than the envelopes above it is worse than no list.
        let (budget_from, budget_to) = date::month_bounds(&budget_period).unwrap_or_default();

        // One pass for the whole section, run together rather than one after
        // another.
        //
        // Every tab is loaded on every refresh, not just the visible one: there is
        // one database under all nine of them, and loading only the active tab
        // leaves the other eight showing whatever they last had — which is exactly
        // how a tab comes to be opened onto stale figures. Sequentially that was
        // some thirty round trips end to end. The pool holds eight connections, so
        // awaiting them together is the same work in a fraction of the time.
        let (txn_periods, txn_sources, page, snap, cats, month_rows, needs) = tokio::join!(
            txn::periods(&pool),
            txn::sources(&pool),
            txn::page(&pool, &filter, sort, desc, page_no),
            insights::snapshot(&pool, today),
            txn::spend_by_category(&pool, &from, &to),
            txn::monthly_totals(&pool, 12),
            obligations::needs_you(&pool, today, 14),
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
            subs_yearly,
            due_rows,
            due_totals,
            account_totals,
            loan_rows,
            account_details,
        ) = tokio::join!(
            recur::list(&pool, Some(recur::RecurKind::Subscription), true),
            recur::list(&pool, Some(recur::RecurKind::Bill), true),
            obligations::between(&pool, &bill_from, &bill_to, today),
            recur::yearly_total(&pool, &base),
            dues::list(&pool, None),
            dues::totals(&pool),
            accounts::totals(&pool),
            loans::list(&pool),
            accounts::details(&pool, &from, &to, today),
        );
        let subs = subs.unwrap_or_default();
        let bill_templates = bill_templates.unwrap_or_default();
        let bills = bills.unwrap_or_default();
        let subs_yearly = subs_yearly.unwrap_or(0);
        let due_rows = due_rows.unwrap_or_default();
        let due_totals = due_totals.unwrap_or_default();
        let account_totals = account_totals.unwrap_or_default();
        let loan_rows = loan_rows.unwrap_or_default();
        let account_details = account_details.unwrap_or_default();

        let (elapsed, total_days) = budgets::month_progress(&budget_period, today);
        // Calendar income comes from the ledger rather than from a schedule:
        // salary that has actually landed is a fact, and a projected credit would
        // be a guess sitting on the same grid as real obligations.
        let (
            budget_rows,
            allocation,
            disc,
            disc_history,
            budget_fixed,
            cal_items,
            cal_income,
            low,
        ) = tokio::join!(
            budgets::list(&pool, &budget_period),
            budgets::allocation(&pool, &budget_period),
            budgets::discipline(&pool, DISCIPLINE_MONTHS, today),
            budgets::envelope_history(&pool, DISCIPLINE_MONTHS, today),
            obligations::between(&pool, &budget_from, &budget_to, today),
            obligations::between(&pool, &cal_from, &cal_to, today),
            txn::income_by_day(&pool, &cal_from, &cal_to),
            insights::cash_flow_low_point(&pool, today, 30),
        );
        let budget_rows = budget_rows.unwrap_or_default();
        let allocation = allocation.unwrap_or_default();
        let disc = disc.unwrap_or_default();
        let (disc_periods, disc_rows) = disc_history.unwrap_or_default();
        let budget_fixed = budget_fixed.unwrap_or_default();
        let cal_items = cal_items.unwrap_or_default();
        let cal_income = cal_income.unwrap_or_default();
        let low = low.unwrap_or_default();

        // Twelve months around the one being viewed, for the Year toggle. Only
        // loaded when that view is on: it is twelve times the work of the grid
        // and nothing on the month view reads it.
        let mut cal_year: Vec<(String, Vec<obligations::Obligation>, i64)> = Vec::new();
        if cal_view == "year"
            && let Some(jan) = date::parse(&cal_from).ok().and_then(|d| d.with_month(1))
        {
            for m in 1..=12u32 {
                let Some(first) = jan.with_month(m) else { continue };
                let period = date::ym(first);
                let Ok((f, t)) = date::month_bounds(&period) else { continue };
                let items =
                    obligations::between(&pool, &f, &t, today).await.unwrap_or_default();
                let income = txn::income_between(&pool, &f, &t).await.unwrap_or(0);
                cal_year.push((period, items, income));
            }
        }

        // What actually falls on the heaviest day, so the card can list it
        // rather than only totalling it.
        let heaviest_on = {
            let mut by_day: std::collections::BTreeMap<&str, i64> = Default::default();
            for o in &cal_items {
                if let Some(m) = o.shown_minor() {
                    *by_day.entry(o.due_on.as_str()).or_default() += m;
                }
            }
            by_day.into_iter().max_by_key(|(_, m)| *m).map(|(d, _)| d.to_string())
        };
        let cal_heaviest_items: Vec<obligations::Obligation> = match &heaviest_on {
            Some(d) => cal_items.iter().filter(|o| &o.due_on == d).cloned().collect(),
            None => Vec::new(),
        };

        // The overview's own short ledger, unfiltered — the Transactions tab's
        // filters must not silently change what "recent" means on the dashboard.
        let recent = txn::page(&pool, &TxnFilter::default(), TxnSort::Date, true, 0)
            .await
            .unwrap_or_default();

        // Six months of spending by category, for the Insights trend card. Oldest
        // first, so the bars read left to right like every other chart here.
        let mut trend_months: Vec<(String, Vec<tulipix_finances::txn::CategorySpend>)> = Vec::new();
        for back in (0..TREND_MONTHS).rev() {
            let Some(first) = today
                .with_day(1)
                .and_then(|d| d.checked_sub_months(chrono::Months::new(back)))
            else {
                continue;
            };
            let period = date::ym(first);
            let Ok((from, to)) = date::month_bounds(&period) else { continue };
            let rows = txn::spend_by_category(&pool, &from, &to).await.unwrap_or_default();
            trend_months.push((period, rows));
        }

        // Insights and the badge. `projection` is the one read here that has to
        // wait: it is computed *from* the flags and the savings history.
        //
        // Overdue is read separately from `badge`, which counts everything inside
        // the notice window: overdue makes the badge an alarm rather than a count.
        let (flags, savings_rows, demo, badge, overdue) = tokio::join!(
            insights::flags(&pool, today),
            insights::savings_history(&pool, TREND_MONTHS),
            tulipix_finances::demo::present(&pool),
            obligations::badge_count(&pool, today, tulipix_finances::lead_days()),
            obligations::overdue_names(&pool, today),
        );
        let flags = flags.unwrap_or_default();
        let savings_rows = savings_rows.unwrap_or_default();
        let demo = demo.unwrap_or(false);
        let badge = badge.unwrap_or(0);
        let overdue = overdue.unwrap_or_default();
        let projection = insights::projection(&pool, today, &flags, &savings_rows)
            .await
            .unwrap_or_default();
        let badge_overdue = !overdue.is_empty();
        // The health score is always about *this* month, whatever month the
        // Budgets tab happens to be parked on — a score that moved when you
        // stepped a tab would be measuring the interface, not the money.
        let health_budgets = if budget_period == period {
            budget_rows.clone()
        } else {
            budgets::list(&pool, &period).await.unwrap_or_default()
        };
        let health = insights::health(&snap, &savings_rows, &health_budgets, overdue.len() as i64, &base);
        // Nothing at all yet — no real account, no posting. The first-run door
        // stands in for an Overview that would otherwise be nine zeroes.
        let empty = page.total == 0
            && account_rows.iter().all(|a| a.kind == accounts::AccountKind::Virtual)
            && filter.search.is_none();

        // Everything built off the UI thread; only the assignment happens on it.
        let ui_stats = view::stats(&snap, &health, &base);
        let ui_health = view::health_parts(&health.parts);
        let health_score = match health.score {
            Some(n) => format!("{n} / 100"),
            None => "—".to_string(),
        };
        let health_band = health.band().to_string();
        let ui_slices = view::slices(&cats, &base);
        let ui_months = view::months(&month_rows, &base);
        let ui_needs = view::obligations(&needs);
        let ui_txns = view::txns(&page.rows, &base, running_shown);
        // The stat strip is computed from every bill in the window, and the list
        // from the filtered subset — so the four figures keep saying what the month
        // holds while the user narrows the list under them.
        let ui_bill_stats = view::bill_stats(&bills, subs_yearly, snap.income_this_month_minor, &base);
        let counts = view::bill_counts(&bills);
        let shown = view::filter_bills(&bills, &bills_filter);
        let ui_bills = view::obligations(&shown);
        // Unused-subscription figures come from the same flag the Insights tab
        // shows, so the two tabs cannot disagree about what is unused.
        let last_playback = insights::last_video_playback().await;
        let unused = insights::unused_subscription_flags(&pool, today, last_playback)
            .await
            .unwrap_or_default();
        let unused_names: Vec<String> = unused.iter().map(|f| f.title.clone()).collect();
        let idle_days = last_playback
            .map(|d| date::days_between(d, today))
            .unwrap_or(0);
        // Name → days idle, for the row pill. Keyed on the recurrence's own name
        // rather than the flag's sentence, so a rename cannot orphan the pill.
        let unused_pairs: Vec<(String, i64)> = subs
            .iter()
            .filter(|r| unused_names.iter().any(|t| t.starts_with(&r.name)))
            .map(|r| (r.name.clone(), idle_days))
            .collect();
        let ui_bill_templates = view::recurrences(&bill_templates, &base, &[]);
        let unused_yearly: i64 = subs
            .iter()
            .filter(|r| unused.iter().any(|f| f.title.starts_with(&r.name)))
            .map(|r| r.yearly_minor)
            .sum();
        let ui_sub_stats = view::sub_stats(&subs, &base, &unused_names, unused_yearly);
        let sub_counts = view::sub_counts(&subs);
        let ui_sub_categories = view::sub_categories(&subs);
        let ui_sub_currencies = view::sub_currencies(&subs);
        let ui_subs = view::recurrences(
            &view::filter_subs(
                &subs,
                &subs_filter,
                Some(subs_category.as_str()).filter(|c| *c != EVERY_SUB_CATEGORY),
                Some(subs_currency.as_str()).filter(|c| *c != EVERY_CURRENCY),
            ),
            &base,
            &unused_pairs,
        );
        let due_settled = dues::settled_since(
            &pool,
            &date::iso(today - chrono::Duration::days(DUES_HISTORY_DAYS)),
        )
        .await
        .unwrap_or_default();
        let ui_due_stats = view::due_stats(&due_totals, &due_settled, &due_rows, &base);
        let due_counts = view::due_counts(&due_rows);
        let ui_dues = view::dues(&view::filter_dues(&due_rows, &dues_filter));
        // Closed inside the history window. `due_settled` above is the totals for
        // the stat strip; the card needs the rows themselves, which are already
        // in hand — `due_rows` is every due whatever its status.
        let settled_from = date::iso(today - chrono::Duration::days(DUES_HISTORY_DAYS));
        let settled_rows: Vec<dues::Due> = due_rows
            .iter()
            .filter(|d| !matches!(d.status, dues::Status::Open))
            .filter(|d| d.settled_on.as_deref().is_none_or(|on| on >= settled_from.as_str()))
            .cloned()
            .collect();
        let ui_dues_settled = view::dues(&settled_rows);
        let ui_accounts = view::accounts(&account_rows, &account_details, &base);
        let ui_position = view::position(
            &account_rows,
            due_totals.owed_to_me_minor - due_totals.i_owe_minor,
            &base,
        );
        let ui_loans = view::loans(&loan_rows, today);
        let ui_budgets = view::budgets(&budget_rows, &base, elapsed, total_days);
        let ui_disc = view::discipline(&disc, &base);
        let ui_disc_months = view::period_labels(&disc_periods);
        // No `ui_envelope_history` here, for the same reason as `ui_trend`: each
        // row holds a nested model of its cells, a `ModelRc` is an `Rc`, and the
        // struct is therefore `!Send`. The raw rows travel; the models are built
        // on the UI thread below.
        let (ui_fixed_items, fixed_total_minor) =
            view::fixed_items(&budget_fixed, &subs, &base);
        let ui_cal = view::calendar(&cal_period, today, &cal_items, &cal_income, &base);
        let ui_cal_months = view::calendar_year(&cal_year, &cal_period, &base);
        let ui_cal_flow = view::cash_flow(
            account_totals.liquid_minor,
            &cal_items,
            snap.income_this_month_minor,
            low.as_ref(),
            &base,
        );
        let ui_cal_heaviest_items = view::obligations(&cal_heaviest_items);
        let ui_agenda = view::obligations(&cal_items);
        let ui_flags = view::flags(&flags);
        let ui_savings = view::savings(&savings_rows, &base);
        let ui_savings_summary = view::savings_summary(&savings_rows, &base);
        let (ui_projection, projection_note) = view::projection(&projection, &base);
        // No `ui_trend` here: a `FinCatTrend` holds a nested model of its months, and
        // a `ModelRc` is an `Rc`, so the struct is `!Send` and cannot cross into the
        // event loop. The raw months are plain data and travel fine; the rows are
        // built on the UI thread below.
        let ui_insight_stats = view::insight_stats(&snap, &flags, subs_yearly, &base);

        // The calendar's two cards. `cal_items` is this month; the low point looks 30
        // days ahead, which is deliberately not the same window — the question "is
        // this month covered" is answered by what is coming, not by what is dated
        // inside an arbitrary boundary.
        let (cal_warning, cal_heaviest, cal_heaviest_label, cal_heaviest_sub) =
            view::calendar_cards(low.as_ref(), &cal_items, account_totals.liquid_minor, &base);

        // Where this month's income is committed. Bills here are the ones still to
        // pay this month, so the figure moves as they are paid — which is the point.
        let loan_emi: i64 = loan_rows.iter().map(|l| l.emi_minor).sum();
        let bills_left: i64 = bills
            .iter()
            .filter(|o| !matches!(o.status, obligations::Status::Paid | obligations::Status::Skipped))
            .filter_map(|o| o.shown_minor())
            .sum();
        let ui_commitments = view::commitments(
            snap.income_this_month_minor,
            loan_emi,
            bills_left,
            subs_yearly / 12,
            &base,
        );
        // Spending this month with no envelope watching it.
        let unbudgeted: i64 = budget_rows.iter().filter(|b| b.id.is_none()).map(|b| b.spent_minor).sum();
        let unbudgeted_n = budget_rows.iter().filter(|b| b.id.is_none() && b.spent_minor > 0).count();
        let ui_recent = view::txns(
            recent.rows.iter().take(OVERVIEW_TXNS).cloned().collect::<Vec<_>>().as_slice(),
            &base,
            false,
        );

        let mut account_names = vec![slint::SharedString::from(ALL_ACCOUNTS)];
        account_names.extend(account_pairs.iter().map(|(n, _)| slint::SharedString::from(n)));
        let mut category_names = vec![slint::SharedString::from(ALL_CATEGORIES)];
        category_names.extend(category_pairs.iter().map(|(n, _)| slint::SharedString::from(n)));
        // Month labels are long-form for the dropdown ("July 2026") but the
        // filter is keyed on the period, so the sentinel and the raw periods go
        // through and the box shows those. A prettier label would need a second
        // list and a lookup between them for no gain.
        let shared = |v: &String| slint::SharedString::from(v.as_str());
        let mut period_names = vec![slint::SharedString::from(EVERY_MONTH)];
        period_names.extend(txn_periods.iter().map(shared));
        let mut source_names = vec![slint::SharedString::from(ANY_SOURCE)];
        source_names.extend(txn_sources.iter().map(shared));
        let mut sub_category_names = vec![slint::SharedString::from(EVERY_SUB_CATEGORY)];
        sub_category_names.extend(ui_sub_categories.iter().map(shared));
        let mut sub_currency_names = vec![slint::SharedString::from(EVERY_CURRENCY)];
        sub_currency_names.extend(ui_sub_currencies.iter().map(shared));

        let spent_total = money::format_minor(snap.spent_this_month_minor, &base);
        let month_label = view::month_long(&period);
        let txn_spent = money::format_minor(page.spent_minor, &base);
        let txn_income = money::format_minor(page.income_minor, &base);
        let owed = money::format_minor(due_totals.owed_to_me_minor, &base);
        let i_owe = money::format_minor(due_totals.i_owe_minor, &base);
        let liquid = money::format_minor(account_totals.liquid_minor, &base);
        let debt = money::format_minor(account_totals.debt_minor, &base);
        // Loans alone. `debt_minor` is every negative balance there is, cards
        // included, and heading the Loans grid with it would count the credit
        // card as a loan.
        // `LoanRow::balance_minor` is already flipped positive by `loans::list` —
        // it is what is outstanding, not the account's negative balance. Negating
        // it here made every term zero and the Loans heading read "0 outstanding"
        // over a grid of live loans.
        let loans_out =
            money::format_minor(loan_rows.iter().map(|l| l.balance_minor.max(0)).sum(), &base);
        let subs_yearly = money::format_minor(subs_yearly, &base);
        let (alloc_income, alloc_budgeted, alloc_left, alloc_pct) =
            view::allocation(&allocation, &base);
        let budget_label = view::month_long(&budget_period);
        let bills_label = view::month_long(&bills_period);
        // In the year view the header names the year, not a month inside it.
        let cal_label = if cal_view == "year" {
            cal_period.split('-').next().unwrap_or(&cal_period).to_string()
        } else {
            view::month_long(&cal_period)
        };
        let fixed_total = money::format_minor(fixed_total_minor, &base);
        // Measured against the income of the month being *viewed*, not of today —
        // the tab steps, and a share of a different month's income is a ratio of
        // two unrelated numbers.
        let fixed_share = if allocation.income_minor > 0 {
            format!(
                "{}% of what came in that month",
                (fixed_total_minor as i128 * 100 / allocation.income_minor as i128).min(999)
            )
        } else {
            "No income recorded that month to measure it against.".to_string()
        };
        let low_label = low
            .filter(|l| l.balance_minor < 0)
            .map(|l| format!("Lowest around {}: {}", l.on, money::format_minor(l.balance_minor, &base)))
            .unwrap_or_default();
        let (page_index, page_count, page_total) =
            (page.page as i32, page.pages as i32, page.total as i32);

        let _ = weak.upgrade_in_event_loop(move |w| {
            // `replace_rows` (VecModel::set_vec), never a ModelRc swap: every list
            // here is redrawn by buttons living inside its own rows, which is the
            // slint#6426 trash-crash case.
            macro_rules! put {
                ($get:ident, $set:ident, $rows:expr) => {
                    if let Some(extra) = replace_rows(&w.$get(), $rows) {
                        w.$set(view::model(extra));
                    }
                };
            }
            put!(get_fin_stats, set_fin_stats, ui_stats);
            put!(get_fin_health_parts, set_fin_health_parts, ui_health);
            w.set_fin_health_score(health_score.into());
            w.set_fin_health_band(health_band.into());
            put!(get_fin_slices, set_fin_slices, ui_slices);
            put!(get_fin_months, set_fin_months, ui_months);
            put!(get_fin_needs_you, set_fin_needs_you, ui_needs);
            put!(get_fin_txns, set_fin_txns, ui_txns);
            put!(get_fin_recent, set_fin_recent, ui_recent);
            put!(get_fin_bills, set_fin_bills, ui_bills);
            put!(get_fin_sub_stats, set_fin_sub_stats, ui_sub_stats);
            put!(get_fin_due_stats, set_fin_due_stats, ui_due_stats);
            w.set_fin_dues_all(due_counts.all);
            w.set_fin_dues_to_me(due_counts.to_me);
            w.set_fin_dues_i_owe(due_counts.i_owe);
            w.set_fin_dues_closed(due_counts.closed);
            w.set_fin_subs_all(sub_counts.all);
            w.set_fin_subs_active(sub_counts.active);
            w.set_fin_subs_paused(sub_counts.paused);
            w.set_fin_subs_cancelled(sub_counts.cancelled);
            put!(get_fin_bill_stats, set_fin_bill_stats, ui_bill_stats);
            w.set_fin_bills_all(counts.all);
            w.set_fin_bills_needs(counts.needs);
            w.set_fin_bills_auto(counts.auto);
            w.set_fin_bills_paid(counts.paid);
            w.set_fin_bills_oneoff(counts.oneoff);
            put!(get_fin_bill_templates, set_fin_bill_templates, ui_bill_templates);
            put!(get_fin_subs, set_fin_subs, ui_subs);
            put!(get_fin_dues, set_fin_dues, ui_dues);
            put!(get_fin_dues_settled, set_fin_dues_settled, ui_dues_settled);
            put!(get_fin_accounts, set_fin_accounts, ui_accounts);
            put!(get_fin_position, set_fin_position, ui_position);
            put!(get_fin_loans, set_fin_loans, ui_loans);
            put!(get_fin_budgets, set_fin_budgets, ui_budgets);
            put!(get_fin_fixed_items, set_fin_fixed_items, ui_fixed_items);
            put!(get_fin_discipline, set_fin_discipline, ui_disc);
            put!(get_fin_discipline_months, set_fin_discipline_months, ui_disc_months);
            put!(
                get_fin_envelope_history,
                set_fin_envelope_history,
                view::envelope_history(&disc_rows, &base)
            );
            put!(get_fin_cal_days, set_fin_cal_days, ui_cal);
            put!(get_fin_cal_months, set_fin_cal_months, ui_cal_months);
            put!(get_fin_cal_flow, set_fin_cal_flow, ui_cal_flow);
            put!(
                get_fin_cal_heaviest_items,
                set_fin_cal_heaviest_items,
                ui_cal_heaviest_items
            );
            put!(get_fin_cal_agenda, set_fin_cal_agenda, ui_agenda);
            put!(get_fin_flags, set_fin_flags, ui_flags);
            put!(get_fin_savings, set_fin_savings, ui_savings);
            put!(get_fin_savings_summary, set_fin_savings_summary, ui_savings_summary);
            put!(get_fin_projection, set_fin_projection, ui_projection);
            put!(
                get_fin_trend,
                set_fin_trend,
                view::trend(&trend_months, &base, TREND_CATEGORIES)
            );
            put!(get_fin_insight_stats, set_fin_insight_stats, ui_insight_stats);
            put!(get_fin_commitments, set_fin_commitments, ui_commitments);
            w.set_fin_cal_warning(cal_warning.into());
            w.set_fin_cal_heaviest(cal_heaviest.into());
            w.set_fin_cal_heaviest_label(cal_heaviest_label.into());
            w.set_fin_cal_heaviest_sub(cal_heaviest_sub.into());
            w.set_fin_budget_unbudgeted(
                if unbudgeted > 0 { money::format_minor(unbudgeted, &base) } else { String::new() }
                    .into(),
            );
            w.set_fin_budget_unbudgeted_sub(
                if unbudgeted_n == 0 {
                    "Every category you spent in this month has an envelope.".to_string()
                } else {
                    format!(
                        "across {unbudgeted_n} categor{} with no envelope set",
                        if unbudgeted_n == 1 { "y" } else { "ies" }
                    )
                }
                .into(),
            );
            put!(get_fin_account_names, set_fin_account_names, account_names);
            put!(get_fin_category_names, set_fin_category_names, category_names);
            put!(get_fin_txn_periods, set_fin_txn_periods, period_names);
            put!(get_fin_source_names, set_fin_source_names, source_names);
            put!(get_fin_subs_categories, set_fin_subs_categories, sub_category_names);
            put!(get_fin_subs_currencies, set_fin_subs_currencies, sub_currency_names);

            w.set_fin_bills_period(bills_label.into());
            w.set_fin_cal_view(cal_view.into());
            w.set_fin_fixed_total(fixed_total.into());
            w.set_fin_fixed_share(fixed_share.into());
            w.set_fin_projection_note(projection_note.into());
            w.set_fin_badge_overdue(badge_overdue);
            w.set_fin_empty(empty);
            w.set_fin_demo(demo);
            w.set_fin_spent_total(spent_total.into());
            w.set_fin_month_label(month_label.into());
            w.set_fin_txn_page(page_index);
            w.set_fin_txn_pages(page_count.max(1));
            w.set_fin_txn_total(page_total);
            w.set_fin_running_shown(running_shown);
            w.set_fin_txn_spent(txn_spent.into());
            w.set_fin_txn_income(txn_income.into());
            w.set_fin_subs_yearly(subs_yearly.into());
            w.set_fin_owed_to_me(owed.into());
            w.set_fin_i_owe(i_owe.into());
            w.set_fin_liquid_total(liquid.into());
            w.set_fin_debt_total(debt.into());
            w.set_fin_loans_total(loans_out.into());
            w.set_fin_budget_period(budget_label.into());
            w.set_fin_budget_income(alloc_income.into());
            w.set_fin_budget_allocated(alloc_budgeted.into());
            w.set_fin_budget_unallocated(alloc_left.into());
            w.set_fin_budget_allocated_pct(alloc_pct);
            w.set_fin_cal_label(cal_label.into());
            w.set_fin_cal_low_point(low_label.into());
            w.set_fin_badge(badge as i32);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_a_period_crosses_year_boundaries_both_ways() {
        assert_eq!(step_period("2026-07", 1), "2026-08");
        assert_eq!(step_period("2026-12", 1), "2027-01");
        assert_eq!(step_period("2026-01", -1), "2025-12");
        assert_eq!(step_period("2026-07", -1), "2026-06");
        assert_eq!(step_period("2026-03", -1), "2026-02", "and lands on a short month");
        assert_eq!(step_period("2026-07", 0), "2026-07");
    }

    #[test]
    fn stepping_several_months_at_once() {
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
}
