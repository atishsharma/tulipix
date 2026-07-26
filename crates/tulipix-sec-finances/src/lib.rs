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

/// How many months of budget-versus-actual the discipline table shows.
const DISCIPLINE_MONTHS: u32 = 6;

/// Rows in the overview's recent-transactions card. Enough to recognise the last
/// day or two; the Transactions tab is where the ledger is read properly.
const OVERVIEW_TXNS: usize = 6;

/// Label for "no account filter". Matched by value, so it is a constant.
const ALL_ACCOUNTS: &str = "All accounts";
const ALL_CATEGORIES: &str = "All categories";

#[derive(Default)]
struct State {
    /// Ledger paging and filtering.
    page: u32,
    /// `YYYY-MM` the Budgets tab is looking at.
    budget_period: String,
    /// `YYYY-MM` the Calendar tab is looking at.
    cal_period: String,
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
            st.cal_period = step_period(&st.cal_period, delta);
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
                let rows = tulipix_finances::fx::list(&pool).await.unwrap_or_default();
                let rates = view::rates(&rows, &base);
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_fin_sheet_title("Exchange rates".into());
                    w.set_fin_sheet_hint(format!("Everything is reported in {base}.").into());
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
                let ui = view::schedule(&rows, &currency);
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
                        if let Some(v) = prefill.get(&f.key) {
                            f.value = v.clone();
                        }
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
                    match import::preview(&text, &currency, Some(&preview.column_map), Some(chosen)) {
                        Ok(fresh) => {
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
                        Ok(got) => Ok(Some(format!(
                            "{} added, {} already there.",
                            got.added, got.duplicates
                        ))),
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
                let stays_open = sheet_kind == "prepay";
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
            .add_filter("Statements", &["csv", "txt", "ofx", "qfx"])
            .pick_file()
            .await
        else {
            return;
        };
        let path = file.path().to_path_buf();
        let Ok(pool) = pool().await else { return };

        let text = match tokio::fs::read(&path).await {
            // Statement exports are frequently Windows-encoded, and a hard UTF-8
            // requirement would refuse them outright. Lossy is right here: a
            // mangled character in a merchant name costs nothing, and the amounts
            // and dates are ASCII.
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(e) => {
                tracing::warn!("finances: could not read {}: {e}", path.display());
                return;
            }
        };

        show_preview(weak, &pool, text, None, None).await;
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
    override_format: Option<DateFormat>,
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

    let map = preset.as_ref().map(|p| p.column_map.clone());
    let fmt = override_format.or(preset.as_ref().map(|p| p.date_format));

    match import::preview(&text, &account.currency, map.as_ref(), fmt) {
        Ok(p) => {
            let needs_format = p.date_format.is_none();
            let rows = view::import_rows(&p.rows, &p.currency, IMPORT_PREVIEW_ROWS);
            let span = p.span();
            let summary = if needs_format {
                String::new()
            } else {
                format!(
                    "{} rows{}. {} out, {} in. {} lines were not transactions.",
                    p.rows.len(),
                    span.map(|(a, b)| format!(", {a} to {b}")).unwrap_or_default(),
                    p.debits(),
                    p.credits(),
                    p.unreadable
                )
            };
            let account_name = account.name.clone();
            let currency = p.currency.clone();
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
    let Some((_, _, text)) = state().import.clone() else { return };
    let weak = window.as_weak();
    window.set_fin_sheet_busy(true);
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        let found = import::presets(&pool).await.unwrap_or_default().into_iter().find(|p| p.id == id);
        show_preview(weak, &pool, text, found, None).await;
    });
}

// ── refresh ─────────────────────────────────────────────────────────────────

/// Everything the section shows, in one pass.
pub fn refresh(window: &MainWindow) {
    let filter_search = window.get_fin_txn_search().to_string();
    let filter_account = window.get_fin_txn_account().to_string();
    let filter_category = window.get_fin_txn_category().to_string();
    let filter_kind = window.get_fin_txn_kind().to_string();
    let sort = TxnSort::parse(&window.get_fin_txn_sort().to_lowercase());
    let desc = window.get_fin_txn_desc();

    let weak = window.as_weak();
    spawn(async move {
        let Ok(pool) = pool().await else { return };
        let base = tulipix_finances::fx::base_currency();
        let today = date::today();

        let (page_no, budget_period, cal_period) = {
            let st = state();
            (st.page, st.budget_period.clone(), st.cal_period.clone())
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

        let filter = TxnFilter {
            account_id,
            category_id,
            kind: match filter_kind.as_str() {
                "Expense" => Some(TxnKind::Expense),
                "Income" => Some(TxnKind::Income),
                "Transfer" => Some(TxnKind::Transfer),
                _ => None,
            },
            from: None,
            to: None,
            search: Some(filter_search).filter(|s| !s.trim().is_empty()),
            source: None,
        };
        let running_shown = account_id.is_some() && sort == TxnSort::Date;
        let page = txn::page(&pool, &filter, sort, desc, page_no).await.unwrap_or_default();

        // Overview.
        let snap = insights::snapshot(&pool, today).await.unwrap_or_default();
        let period = date::ym(today);
        let (from, to) = date::month_bounds(&period).unwrap_or_default();
        let cats = txn::spend_by_category(&pool, &from, &to).await.unwrap_or_default();
        let month_rows = txn::monthly_totals(&pool, 12).await.unwrap_or_default();
        let needs = obligations::needs_you(&pool, today, 14).await.unwrap_or_default();

        // Recurrences.
        let subs = recur::list(&pool, Some(recur::RecurKind::Subscription), true)
            .await
            .unwrap_or_default();
        let bill_templates =
            recur::list(&pool, Some(recur::RecurKind::Bill), true).await.unwrap_or_default();
        let (bill_from, bill_to) = (
            date::iso(today - chrono::Duration::days(90)),
            date::iso(today + chrono::Duration::days(60)),
        );
        let bills = obligations::between(&pool, &bill_from, &bill_to, today).await.unwrap_or_default();
        let subs_yearly = recur::yearly_total(&pool, &base).await.unwrap_or(0);

        // Dues, accounts, loans.
        let due_rows = dues::list(&pool, None).await.unwrap_or_default();
        let due_totals = dues::totals(&pool).await.unwrap_or_default();
        let account_totals = accounts::totals(&pool).await.unwrap_or_default();
        let loan_rows = loans::list(&pool).await.unwrap_or_default();

        // Budgets.
        let (elapsed, total_days) = budgets::month_progress(&budget_period, today);
        let budget_rows = budgets::list(&pool, &budget_period).await.unwrap_or_default();
        let allocation = budgets::allocation(&pool, &budget_period).await.unwrap_or_default();
        let disc = budgets::discipline(&pool, DISCIPLINE_MONTHS, today).await.unwrap_or_default();

        // Calendar. Income comes from the ledger rather than from a schedule:
        // salary that has actually landed is a fact, and a projected credit would
        // be a guess sitting on the same grid as real obligations.
        let (cal_from, cal_to) = date::month_bounds(&cal_period).unwrap_or_default();
        let cal_items = obligations::between(&pool, &cal_from, &cal_to, today).await.unwrap_or_default();
        let cal_income = txn::income_by_day(&pool, &cal_from, &cal_to).await.unwrap_or_default();
        let low = insights::cash_flow_low_point(&pool, today, 30).await.unwrap_or_default();

        // The overview's own short ledger, unfiltered — the Transactions tab's
        // filters must not silently change what "recent" means on the dashboard.
        let recent = txn::page(&pool, &TxnFilter::default(), TxnSort::Date, true, 0)
            .await
            .unwrap_or_default();

        // Insights and the badge.
        let flags = insights::flags(&pool, today).await.unwrap_or_default();
        let badge = obligations::badge_count(&pool, today, tulipix_finances::lead_days())
            .await
            .unwrap_or(0);

        // Everything built off the UI thread; only the assignment happens on it.
        let ui_stats = view::stats(&snap, &base);
        let ui_slices = view::slices(&cats, &base);
        let ui_months = view::months(&month_rows, &base);
        let ui_needs = view::obligations(&needs);
        let ui_txns = view::txns(&page.rows, &base, running_shown);
        let ui_bills = view::obligations(&bills);
        let ui_bill_templates = view::recurrences(&bill_templates, &base);
        let ui_subs = view::recurrences(&subs, &base);
        let ui_dues = view::dues(&due_rows);
        let ui_accounts = view::accounts(&account_rows, &base);
        let ui_loans = view::loans(&loan_rows);
        let ui_budgets = view::budgets(&budget_rows, &base, elapsed, total_days);
        let ui_disc = view::discipline(&disc, &base);
        let ui_cal = view::calendar(&cal_period, today, &cal_items, &cal_income, &base);
        let ui_agenda = view::obligations(&cal_items);
        let ui_flags = view::flags(&flags);
        let ui_recent = view::txns(
            recent.rows.iter().take(OVERVIEW_TXNS).cloned().collect::<Vec<_>>().as_slice(),
            &base,
            false,
        );

        let mut account_names = vec![slint::SharedString::from(ALL_ACCOUNTS)];
        account_names.extend(account_pairs.iter().map(|(n, _)| slint::SharedString::from(n)));
        let mut category_names = vec![slint::SharedString::from(ALL_CATEGORIES)];
        category_names.extend(category_pairs.iter().map(|(n, _)| slint::SharedString::from(n)));

        let spent_total = money::format_minor(snap.spent_this_month_minor, &base);
        let month_label = view::month_long(&period);
        let txn_spent = money::format_minor(page.spent_minor, &base);
        let txn_income = money::format_minor(page.income_minor, &base);
        let owed = money::format_minor(due_totals.owed_to_me_minor, &base);
        let i_owe = money::format_minor(due_totals.i_owe_minor, &base);
        let liquid = money::format_minor(account_totals.liquid_minor, &base);
        let debt = money::format_minor(account_totals.debt_minor, &base);
        let subs_yearly = money::format_minor(subs_yearly, &base);
        let (alloc_income, alloc_budgeted, alloc_left, alloc_pct) =
            view::allocation(&allocation, &base);
        let budget_label = view::month_long(&budget_period);
        let cal_label = view::month_long(&cal_period);
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
            put!(get_fin_slices, set_fin_slices, ui_slices);
            put!(get_fin_months, set_fin_months, ui_months);
            put!(get_fin_needs_you, set_fin_needs_you, ui_needs);
            put!(get_fin_txns, set_fin_txns, ui_txns);
            put!(get_fin_recent, set_fin_recent, ui_recent);
            put!(get_fin_bills, set_fin_bills, ui_bills);
            put!(get_fin_bill_templates, set_fin_bill_templates, ui_bill_templates);
            put!(get_fin_subs, set_fin_subs, ui_subs);
            put!(get_fin_dues, set_fin_dues, ui_dues);
            put!(get_fin_accounts, set_fin_accounts, ui_accounts);
            put!(get_fin_loans, set_fin_loans, ui_loans);
            put!(get_fin_budgets, set_fin_budgets, ui_budgets);
            put!(get_fin_discipline, set_fin_discipline, ui_disc);
            put!(get_fin_cal_days, set_fin_cal_days, ui_cal);
            put!(get_fin_cal_agenda, set_fin_cal_agenda, ui_agenda);
            put!(get_fin_flags, set_fin_flags, ui_flags);
            put!(get_fin_account_names, set_fin_account_names, account_names);
            put!(get_fin_category_names, set_fin_category_names, category_names);

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
