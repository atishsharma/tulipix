//! Claims with numbers behind them.
//!
//! Every flag here states a figure the user can check and, where there is one,
//! names the thing to act on. Nothing says "you might want to review your
//! spending" — a flag that cannot be verified or acted on is decoration.
//!
//! One of them, [`unused_subscription_flags`], reads playback data from the Videos
//! section. That is the only place Finances couples to another part of the app, and
//! it is the strongest argument for this living inside Tulipix rather than being a
//! separate tool: no standalone finance app can tell you that you are paying for a
//! streaming service you have not opened in three months.

use anyhow::Result;
// Datelike for `with_day`: the previous month is reached by stepping back from
// the first of this one, which is the one date operation `date` has no helper for.
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;

use crate::date;
use crate::money;
use crate::{accounts, budgets, dues, obligations, recur, txn};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something is wrong now.
    Bad,
    /// Something will be wrong shortly.
    Warn,
    /// Worth knowing.
    Info,
    /// Worth knowing, and it is good news.
    Good,
}

/// What the flag's button should do, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Open the Bills tab at this obligation.
    Obligation(i64),
    /// Open the Subscriptions tab at this recurrence.
    Recurrence(i64),
    /// Open the Budgets tab at this category.
    Category(i64),
    /// Open the Accounts tab at this account.
    Account(i64),
    /// Open the Dues tab at this due.
    Due(i64),
    /// Open the FX rate editor.
    Rates,
}

#[derive(Clone, Debug)]
pub struct Flag {
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub action: Action,
}

/// How long without playback before a video subscription is called unused.
pub const UNUSED_DAYS: i64 = 60;

/// Card utilisation above this is worth saying out loud.
const HIGH_UTILISATION_PCT: i64 = 70;

/// A due older than this is probably not coming back on its own.
const STALE_DUE_DAYS: i64 = 180;

/// Every flag, worst first.
pub async fn flags(pool: &SqlitePool, today: NaiveDate) -> Result<Vec<Flag>> {
    let base = crate::fx::base_currency();
    let fmt = |m: i64| money::format_minor(m, &base);
    let mut out: Vec<Flag> = Vec::new();

    // ── overdue and imminent ────────────────────────────────────────────────
    for o in obligations::needs_you(pool, today, 7).await? {
        let amount = o.shown_minor().map(|m| {
            if o.is_estimate() { money::format_estimate(m, &o.currency) } else { money::format_minor(m, &o.currency) }
        });
        let amount = amount.unwrap_or_else(|| "amount unknown".into());
        if o.days_until < 0 {
            out.push(Flag {
                severity: Severity::Bad,
                title: format!("{} is {} days overdue", o.name, -o.days_until),
                detail: format!("{amount}, was due {}.", o.due_on),
                action: Action::Obligation(o.id),
            });
        } else {
            out.push(Flag {
                severity: Severity::Warn,
                title: match o.days_until {
                    0 => format!("{} is due today", o.name),
                    1 => format!("{} is due tomorrow", o.name),
                    d => format!("{} is due in {d} days", o.name),
                },
                detail: format!("{amount}, due {}.", o.due_on),
                action: Action::Obligation(o.id),
            });
        }
    }

    // ── price rises ─────────────────────────────────────────────────────────
    for r in recur::list(pool, None, false).await? {
        if let (Some(from), Some(now)) = (r.hike_from_minor, r.amount_minor) {
            let rise = now - from;
            let pct = if from > 0 { (rise as i128 * 100 / from as i128) as i64 } else { 0 };
            out.push(Flag {
                severity: Severity::Warn,
                title: format!("{} costs {pct}% more than it used to", r.name),
                detail: format!(
                    "{} → {} ({} more a year).",
                    money::format_minor(from, &r.currency),
                    money::format_minor(now, &r.currency),
                    money::format_minor(rise * r.cycle.per_year(), &r.currency)
                ),
                action: Action::Recurrence(r.id),
            });
        }
    }

    // ── budgets ─────────────────────────────────────────────────────────────
    let period = date::ym(today);
    let (elapsed, total_days) = budgets::month_progress(&period, today);
    for b in budgets::list(pool, &period).await? {
        if b.id.is_none() {
            continue; // no envelope set: nothing to be over
        }
        if b.over_budget() {
            out.push(Flag {
                severity: Severity::Warn,
                title: format!("{} is over budget", b.category_name),
                detail: format!(
                    "{} spent of {}.",
                    fmt(b.spent_minor),
                    fmt(b.allowance_minor())
                ),
                action: Action::Category(b.category_id),
            });
        } else if b.off_pace(elapsed, total_days) {
            out.push(Flag {
                severity: Severity::Info,
                title: format!("{} is running ahead of pace", b.category_name),
                detail: format!(
                    "{} by day {elapsed}; on this rate the month ends at {} against {}.",
                    fmt(b.spent_minor),
                    fmt(b.projected_minor(elapsed, total_days)),
                    fmt(b.allowance_minor())
                ),
                action: Action::Category(b.category_id),
            });
        }
    }

    // ── the cash-flow low point ─────────────────────────────────────────────
    if let Some(low) = cash_flow_low_point(pool, today, 30).await?
        && low.balance_minor < 0
    {
        out.push(Flag {
            severity: Severity::Bad,
            title: format!("Money runs out around {}", low.on),
            detail: format!(
                "Liquid balance projects to {} once the next 30 days of bills are paid.",
                fmt(low.balance_minor)
            ),
            action: Action::None,
        });
    }

    // ── cards ───────────────────────────────────────────────────────────────
    for a in accounts::list(pool, false).await? {
        if let Some(pct) = a.utilisation_pct()
            && pct >= HIGH_UTILISATION_PCT
        {
            out.push(Flag {
                severity: Severity::Warn,
                title: format!("{} is {pct}% of its limit", a.name),
                detail: format!(
                    "{} owed against a {} limit.",
                    fmt(-a.balance_minor),
                    fmt(a.credit_limit_minor.unwrap_or(0))
                ),
                action: Action::Account(a.id),
            });
        }
    }

    // ── unconverted currencies ──────────────────────────────────────────────
    let missing = crate::fx::missing(pool, &base).await?;
    if !missing.is_empty() {
        out.push(Flag {
            severity: Severity::Warn,
            title: format!("No exchange rate for {}", missing.join(", ")),
            detail: format!(
                "Those amounts are being counted at 1:1, so every {base} total containing them is wrong."
            ),
            action: Action::Rates,
        });
    }

    // ── unused subscriptions (the cross-section one) ─────────────────────────
    out.extend(unused_subscription_flags(pool, today, last_video_playback().await).await?);

    // ── stale dues ──────────────────────────────────────────────────────────
    for d in dues::list(pool, Some(dues::Status::Open)).await? {
        if d.age_days >= STALE_DUE_DAYS && d.direction == dues::Direction::OwedToMe {
            out.push(Flag {
                severity: Severity::Info,
                title: format!("{} has owed you for {} months", d.person, d.age_days / 30),
                detail: format!(
                    "{} outstanding since {}. Writing it off would move it out of your assets.",
                    money::format_minor(d.amount_minor, &d.currency),
                    d.opened_on
                ),
                action: Action::Due(d.id),
            });
        }
    }

    // ── variance between estimates and reality ──────────────────────────────
    let six_months_ago = date::iso(today - chrono::Duration::days(180));
    if let Some((name, delta)) = obligations::variance_since(pool, &six_months_ago).await?.into_iter().next() {
        out.push(Flag {
            severity: Severity::Info,
            title: format!("{name} keeps coming in {} than estimated", if delta > 0 { "higher" } else { "lower" }),
            detail: format!("Last bill was {} off the estimate.", fmt(delta.abs())),
            action: Action::None,
        });
    }

    // ── the good news ───────────────────────────────────────────────────────
    let alloc = budgets::allocation(pool, &period).await?;
    if let Some(rate) = alloc.savings_rate_pct() {
        out.push(Flag {
            severity: if rate >= 20 { Severity::Good } else { Severity::Info },
            title: format!("Saving {rate}% of income this month"),
            detail: format!("{} in, {} out.", fmt(alloc.income_minor), fmt(alloc.spent_minor)),
            action: Action::None,
        });
    }

    let yearly = recur::yearly_total(pool, &base).await?;
    if yearly > 0 {
        let share = if alloc.income_minor > 0 {
            format!(" — {}% of a year at this income", yearly * 100 / (alloc.income_minor * 12))
        } else {
            String::new()
        };
        out.push(Flag {
            severity: Severity::Info,
            title: format!("Subscriptions and bills cost {} a year", fmt(yearly)),
            detail: format!("Across every active recurrence{share}."),
            action: Action::None,
        });
    }

    out.sort_by_key(|f| f.severity);
    Ok(out)
}

/// Video subscriptions being paid for while Videos sits unopened.
///
/// `last_playback` comes from [`last_video_playback`], which reads
/// `watch_progress` out of `videos.db`. It is a parameter rather than a read
/// inside here so this stays a pure function of the two databases' contents: as
/// an ambient read it answered differently on a machine that happens to have a
/// `videos.db`, which is exactly what a test cannot control.
///
/// A subscription counts as video-shaped when it sits under the Entertainment
/// category tree, which the user controls. There is deliberately no hardcoded list
/// of streaming brands: it would be wrong within a year and it would silently miss
/// whatever the user actually subscribes to.
pub async fn unused_subscription_flags(
    pool: &SqlitePool,
    today: NaiveDate,
    last_playback: Option<NaiveDate>,
) -> Result<Vec<Flag>> {
    let Some(last) = last_playback else { return Ok(Vec::new()) };
    let idle = date::days_between(last, today);
    if idle < UNUSED_DAYS {
        return Ok(Vec::new());
    }

    let ids: Vec<i64> = sqlx::query_scalar(
        "WITH RECURSIVE tree(id) AS (
             SELECT id FROM categories WHERE name = 'Entertainment'
             UNION ALL
             SELECT c.id FROM categories c JOIN tree t ON c.parent_id = t.id
         )
         SELECT r.id FROM recurrences r
          WHERE r.status = 'active' AND r.category_id IN (SELECT id FROM tree)",
    )
    .fetch_all(pool)
    .await?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let all = recur::list(pool, None, false).await?;
    Ok(all
        .into_iter()
        .filter(|r| ids.contains(&r.id))
        .filter_map(|r| {
            let shown = r.shown_minor()?;
            Some(Flag {
                severity: Severity::Warn,
                title: format!("{} — paid, not watched", r.name),
                detail: format!(
                    "No playback logged in Videos for {idle} days. {} {}, {} a year.",
                    money::format_minor(shown, &r.currency),
                    match r.cycle {
                        date::Cycle::Weekly => "a week",
                        date::Cycle::Monthly => "a month",
                        date::Cycle::Quarterly => "a quarter",
                        date::Cycle::Yearly => "a year",
                        date::Cycle::Irregular => "each time",
                    },
                    money::format_minor(r.yearly_minor, &r.currency)
                ),
                action: Action::Recurrence(r.id),
            })
        })
        .collect())
}

/// The most recent playback recorded in the Videos section.
///
/// Opens `videos.db` read-only through the shared handle. Returns `None` on any
/// problem at all — this is an optional signal, not a dependency.
async fn last_video_playback() -> Option<NaiveDate> {
    let handle = tulipix_core::db::DbHandle::open("videos").ok()?;
    if !handle.path.exists() {
        return None;
    }
    let pool = handle.pool().await.ok()?;
    let latest: Option<i64> = sqlx::query_scalar("SELECT MAX(updated) FROM watch_progress")
        .fetch_optional(&pool)
        .await
        .ok()
        .flatten();
    pool.close().await;
    let secs = latest?;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|dt| dt.with_timezone(&chrono::Local).date_naive())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LowPoint {
    pub on: String,
    pub balance_minor: i64,
}

/// Walk the next `days` of known obligations against today's liquid balance and
/// report the worst day.
///
/// Only counts what is actually scheduled. Expected income is not projected —
/// guessing that a salary will arrive is exactly the kind of invented number the
/// design refuses, and a low point that assumes money not yet promised is worse
/// than no low point at all.
pub async fn cash_flow_low_point(
    pool: &SqlitePool,
    today: NaiveDate,
    days: i64,
) -> Result<Option<LowPoint>> {
    let liquid = accounts::totals(pool).await?.liquid_minor;
    let horizon = date::iso(today + chrono::Duration::days(days.max(0)));
    let upcoming = obligations::between(pool, &date::iso(today), &horizon, today).await?;

    let mut running = liquid;
    let mut worst: Option<LowPoint> = None;
    for o in upcoming {
        if !o.status.is_open() {
            continue;
        }
        let Some(amount) = o.shown_minor() else { continue };
        running -= amount;
        let here = LowPoint { on: o.due_on.clone(), balance_minor: running };
        match &worst {
            Some(w) if w.balance_minor <= here.balance_minor => {}
            _ => worst = Some(here),
        }
    }
    Ok(worst)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub liquid_minor: i64,
    pub debt_minor: i64,
    pub spent_this_month_minor: i64,
    /// The same figure for the previous calendar month, so the strip can say
    /// whether this month is worse. `None` when there is no previous month on
    /// record — a first month has nothing to be up or down against, and "+100%"
    /// is arithmetic rather than information.
    pub spent_last_month_minor: Option<i64>,
    pub income_this_month_minor: i64,
    pub owed_to_me_minor: i64,
    pub i_owe_minor: i64,
    pub subscriptions_yearly_minor: i64,
    pub open_obligations: i64,
}

impl Snapshot {
    /// Income minus spending this month. Not net worth — see the design's note on
    /// why that number is not printed anywhere.
    pub fn saved_this_month_minor(&self) -> i64 {
        self.income_this_month_minor - self.spent_this_month_minor
    }
}

/// The Overview strip, in one call.
pub async fn snapshot(pool: &SqlitePool, today: NaiveDate) -> Result<Snapshot> {
    let period = date::ym(today);
    let (from, to) = date::month_bounds(&period)?;
    let acct = accounts::totals(pool).await?;
    let due = dues::totals(pool).await?;

    // Previous month, and only if something was actually posted in it: an empty
    // month would make this month's spending look like a spike when the truth is
    // that the ledger only started here.
    let prev = date::month_bounds(&date::ym(
        today.with_day(1).unwrap_or(today).pred_opt().unwrap_or(today),
    ))?;
    let prev_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE occurred_on BETWEEN ? AND ?")
            .bind(&prev.0)
            .bind(&prev.1)
            .fetch_one(pool)
            .await?;
    let spent_last_month_minor = if prev_rows > 0 {
        Some(txn::spent_between(pool, &prev.0, &prev.1).await?)
    } else {
        None
    };

    Ok(Snapshot {
        liquid_minor: acct.liquid_minor,
        debt_minor: acct.debt_minor,
        spent_this_month_minor: txn::spent_between(pool, &from, &to).await?,
        spent_last_month_minor,
        income_this_month_minor: txn::income_between(pool, &from, &to).await?,
        owed_to_me_minor: due.owed_to_me_minor,
        i_owe_minor: due.i_owe_minor,
        subscriptions_yearly_minor: recur::yearly_total(pool, &crate::fx::base_currency()).await?,
        open_obligations: obligations::badge_count(pool, today, 14).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::txn::NewTxn;
    use crate::{accounts, schema};

    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&p).await.unwrap();
        schema::seed_defaults(&p).await.unwrap();
        p
    }

    fn d(s: &str) -> NaiveDate {
        date::parse(s).unwrap()
    }

    async fn bank(p: &SqlitePool, opening: i64) -> i64 {
        accounts::create(p, &accounts::NewAccount::bank("HDFC", opening)).await.unwrap()
    }

    async fn cat(p: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM categories WHERE name = ?")
            .bind(name)
            .fetch_one(p)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_empty_database_produces_no_alarming_claims() {
        let p = pool().await;
        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(
            f.iter().all(|x| x.severity == Severity::Info || x.severity == Severity::Good),
            "nothing is wrong yet, so nothing should say it is: {f:#?}"
        );
    }

    #[tokio::test]
    async fn an_overdue_bill_is_the_worst_thing_on_the_list() {
        let p = pool().await;
        bank(&p, 10_000_000).await;
        obligations::create_one_off(&p, "Electricity", "2026-06-12", Some(328_400)).await.unwrap();
        obligations::sweep(&p, d("2026-07-26")).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(f[0].severity, Severity::Bad, "worst first");
        assert!(f[0].title.contains("Electricity"));
        assert!(f[0].title.contains("44 days overdue"));
        assert_eq!(f[0].action, Action::Obligation(1));
    }

    #[tokio::test]
    async fn a_price_rise_is_reported_with_both_figures_and_the_yearly_cost() {
        let p = pool().await;
        let id = recur::create(&p, &recur::NewRecurrence::subscription("JioHotstar", 29_900, "2026-01-15"))
            .await
            .unwrap();
        recur::update(&p, id, &recur::NewRecurrence::subscription("JioHotstar", 49_900, "2026-08-15"))
            .await
            .unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let hike = f.iter().find(|x| x.title.contains("JioHotstar")).unwrap();
        assert!(hike.title.contains("66% more"), "got {:?}", hike.title);
        assert!(hike.detail.contains("₹299"));
        assert!(hike.detail.contains("₹499"));
        assert!(hike.detail.contains("₹2,400 more a year"), "got {:?}", hike.detail);
        assert_eq!(hike.action, Action::Recurrence(id));
    }

    #[tokio::test]
    async fn going_over_budget_is_reported_and_running_ahead_is_only_noted() {
        let p = pool().await;
        let acct = bank(&p, 100_000_000).await;
        let groceries = cat(&p, "Groceries").await;
        let travel = cat(&p, "Travel").await;
        budgets::set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        budgets::set(&p, travel, "2026-07", 1_000_000, false).await.unwrap();

        let mut over = NewTxn::expense(acct, 900_000, "INR", "2026-07-10", "Veg");
        over.category_id = Some(groceries);
        txn::post(&p, &over).await.unwrap();
        // Ahead of pace but not yet over: ₹9,000 of a ₹10,000 envelope by the 26th
        // would be fine, so use a figure that projects past it.
        let mut fast = NewTxn::expense(acct, 900_000, "INR", "2026-07-05", "Flights");
        fast.category_id = Some(travel);
        txn::post(&p, &fast).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let g = f.iter().find(|x| x.title.contains("Groceries")).unwrap();
        assert_eq!(g.severity, Severity::Warn);
        assert!(g.title.contains("over budget"));
        assert_eq!(g.action, Action::Category(groceries));

        let t = f.iter().find(|x| x.title.contains("Travel"));
        if let Some(t) = t {
            assert_eq!(t.severity, Severity::Info, "ahead of pace is not yet a problem");
        }
    }

    #[tokio::test]
    async fn a_category_with_no_envelope_is_never_over_budget() {
        let p = pool().await;
        let acct = bank(&p, 100_000_000).await;
        let shopping = cat(&p, "Shopping").await;
        let mut t = NewTxn::expense(acct, 5_000_000, "INR", "2026-07-10", "Spree");
        t.category_id = Some(shopping);
        txn::post(&p, &t).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(!f.iter().any(|x| x.title.contains("Shopping") && x.title.contains("over budget")));
    }

    #[tokio::test]
    async fn the_cash_flow_low_point_only_counts_what_is_actually_scheduled() {
        let p = pool().await;
        bank(&p, 500_000).await; // ₹5,000 liquid
        obligations::create_one_off(&p, "Rent", "2026-08-05", Some(3_800_000)).await.unwrap();
        obligations::create_one_off(&p, "Power", "2026-08-12", Some(328_400)).await.unwrap();

        let low = cash_flow_low_point(&p, d("2026-07-26"), 30).await.unwrap().unwrap();
        assert_eq!(low.on, "2026-08-12", "the worst day is after both bills");
        assert_eq!(low.balance_minor, 500_000 - 3_800_000 - 328_400);

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let ran_out = f.iter().find(|x| x.title.contains("Money runs out")).unwrap();
        assert_eq!(ran_out.severity, Severity::Bad);
    }

    #[tokio::test]
    async fn a_healthy_balance_produces_no_low_point_warning() {
        let p = pool().await;
        bank(&p, 100_000_000).await;
        obligations::create_one_off(&p, "Rent", "2026-08-05", Some(3_800_000)).await.unwrap();
        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(!f.iter().any(|x| x.title.contains("Money runs out")));
    }

    #[tokio::test]
    async fn a_paid_bill_does_not_drag_the_projection_down_twice() {
        let p = pool().await;
        let acct = bank(&p, 500_000).await;
        let obl = obligations::create_one_off(&p, "Rent", "2026-08-05", Some(400_000)).await.unwrap();
        obligations::mark_paid(&p, obl, 400_000, d("2026-08-05"), acct, "INR").await.unwrap();
        // The money has already left the account, so the balance reflects it and
        // the obligation must not be subtracted again.
        let low = cash_flow_low_point(&p, d("2026-07-26"), 30).await.unwrap();
        assert!(low.is_none());
    }

    #[tokio::test]
    async fn a_card_near_its_limit_is_flagged_with_both_figures() {
        let p = pool().await;
        let card = accounts::create(
            &p,
            &accounts::NewAccount {
                kind: accounts::AccountKind::Card,
                credit_limit_minor: Some(10_000_000),
                ..accounts::NewAccount::bank("ICICI Card", 0)
            },
        )
        .await
        .unwrap();
        txn::post(&p, &NewTxn::expense(card, 8_000_000, "INR", "2026-07-01", "Shopping"))
            .await
            .unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let c = f.iter().find(|x| x.title.contains("ICICI Card")).unwrap();
        assert!(c.title.contains("80%"));
        assert!(c.detail.contains("₹80,000"));
        assert!(c.detail.contains("₹1,00,000"));
        assert_eq!(c.action, Action::Account(card));
    }

    #[tokio::test]
    async fn a_currency_with_no_rate_is_called_out_because_every_total_is_wrong() {
        let p = pool().await;
        let mut usd = recur::NewRecurrence::subscription("Some SaaS", 999, "2026-08-15");
        usd.currency = "USD".into();
        recur::create(&p, &usd).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let r = f.iter().find(|x| x.title.contains("exchange rate")).unwrap();
        assert!(r.title.contains("USD"));
        assert_eq!(r.action, Action::Rates);

        crate::fx::set(&p, "USD", 83_600_000, 0).await.unwrap();
        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(!f.iter().any(|x| x.title.contains("exchange rate")));
    }

    #[tokio::test]
    async fn an_old_loan_to_a_friend_is_noted_but_not_alarming() {
        let p = pool().await;
        let acct = bank(&p, 10_000_000).await;
        dues::create(&p, &dues::NewDue::lent("Ravi", 500_000, "2025-01-10", acct)).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let s = f.iter().find(|x| x.title.contains("Ravi")).unwrap();
        assert_eq!(s.severity, Severity::Info, "the design forbids chasing anyone");
        assert!(s.detail.contains("₹5,000"));
    }

    #[tokio::test]
    async fn a_recent_loan_is_not_stale() {
        let p = pool().await;
        let acct = bank(&p, 10_000_000).await;
        dues::create(&p, &dues::NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(!f.iter().any(|x| x.title.contains("Ravi")));
    }

    #[tokio::test]
    async fn the_savings_rate_appears_when_there_is_income() {
        let p = pool().await;
        let acct = bank(&p, 0).await;
        let mut salary = NewTxn::expense(acct, 10_000_000, "INR", "2026-07-01", "Salary");
        salary.kind = txn::TxnKind::Income;
        txn::post(&p, &salary).await.unwrap();
        txn::post(&p, &NewTxn::expense(acct, 2_000_000, "INR", "2026-07-05", "Veg")).await.unwrap();

        let f = flags(&p, d("2026-07-26")).await.unwrap();
        let s = f.iter().find(|x| x.title.contains("Saving")).unwrap();
        assert!(s.title.contains("80%"));
        assert_eq!(s.severity, Severity::Good);
    }

    #[tokio::test]
    async fn without_income_no_savings_rate_is_invented() {
        let p = pool().await;
        let acct = bank(&p, 10_000_000).await;
        txn::post(&p, &NewTxn::expense(acct, 2_000_000, "INR", "2026-07-05", "Veg")).await.unwrap();
        let f = flags(&p, d("2026-07-26")).await.unwrap();
        assert!(!f.iter().any(|x| x.title.contains("Saving")));
    }

    #[tokio::test]
    async fn the_unused_subscription_flag_stays_silent_with_no_playback_data() {
        // With no videos.db — or one with an empty watch_progress — this must say
        // nothing rather than accusing the user of not watching what they pay for.
        let p = pool().await;
        let mut r = recur::NewRecurrence::subscription("JioHotstar", 29_900, "2026-08-15");
        r.category_id = Some(cat(&p, "Subscriptions").await);
        recur::create(&p, &r).await.unwrap();
        let today = d("2026-07-26");
        assert!(unused_subscription_flags(&p, today, None).await.unwrap().is_empty());
        // Watched yesterday: also silent, and this is the case that would fire
        // wrongly if the idle comparison were the wrong way round.
        assert!(
            unused_subscription_flags(&p, today, Some(d("2026-07-25"))).await.unwrap().is_empty()
        );
        // And the whole flag set still builds.
        assert!(!flags(&p, today).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_subscription_unwatched_for_two_months_is_flagged() {
        let p = pool().await;
        let mut r = recur::NewRecurrence::subscription("JioHotstar", 29_900, "2026-08-15");
        r.category_id = Some(cat(&p, "Subscriptions").await);
        recur::create(&p, &r).await.unwrap();

        let today = d("2026-07-26");
        let stale = today - chrono::Duration::days(UNUSED_DAYS + 30);
        let f = unused_subscription_flags(&p, today, Some(stale)).await.unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].title, "JioHotstar — paid, not watched");
        assert_eq!(f[0].severity, Severity::Warn);
        // The yearly figure is the argument, so it has to be in the body.
        assert!(f[0].detail.contains(&money::format_minor(29_900 * 12, "INR")));
    }

    #[tokio::test]
    async fn last_months_spending_is_only_reported_when_last_month_exists() {
        let p = pool().await;
        let acct = bank(&p, 50_000_000).await;
        txn::post(&p, &NewTxn::expense(acct, 2_000_000, "INR", "2026-07-05", "Veg")).await.unwrap();

        // Only July on record: there is no June to be up or down against, and a
        // month compared against an empty one reads as a spike that never happened.
        let july = snapshot(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(july.spent_last_month_minor, None);

        txn::post(&p, &NewTxn::expense(acct, 1_500_000, "INR", "2026-06-11", "Veg")).await.unwrap();
        let again = snapshot(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(again.spent_last_month_minor, Some(1_500_000));
        assert_eq!(again.spent_this_month_minor, 2_000_000, "July is unchanged by the June row");

        // January looks back across the year boundary, not to month zero.
        txn::post(&p, &NewTxn::expense(acct, 900_000, "INR", "2025-12-30", "Veg")).await.unwrap();
        let jan = snapshot(&p, d("2026-01-15")).await.unwrap();
        assert_eq!(jan.spent_last_month_minor, Some(900_000));
    }

    #[tokio::test]
    async fn the_snapshot_gathers_the_overview_strip() {
        let p = pool().await;
        let acct = bank(&p, 10_839_000).await;
        let card = accounts::create(
            &p,
            &accounts::NewAccount {
                kind: accounts::AccountKind::Card,
                ..accounts::NewAccount::bank("Card", 0)
            },
        )
        .await
        .unwrap();
        txn::post(&p, &NewTxn::expense(card, 64_900, "INR", "2026-07-26", "Netflix")).await.unwrap();
        let mut salary = NewTxn::expense(acct, 10_000_000, "INR", "2026-07-01", "Salary");
        salary.kind = txn::TxnKind::Income;
        txn::post(&p, &salary).await.unwrap();
        dues::create(&p, &dues::NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        recur::create(&p, &recur::NewRecurrence::subscription("Netflix", 64_900, "2026-08-26"))
            .await
            .unwrap();

        let s = snapshot(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(s.debt_minor, 64_900);
        assert_eq!(s.income_this_month_minor, 10_000_000);
        assert_eq!(s.spent_this_month_minor, 64_900);
        assert_eq!(s.owed_to_me_minor, 500_000);
        assert_eq!(s.subscriptions_yearly_minor, 64_900 * 12);
        assert_eq!(s.saved_this_month_minor(), 10_000_000 - 64_900);
        // Lending left the bank, so liquid is down by it.
        assert_eq!(s.liquid_minor, 10_839_000 + 10_000_000 - 500_000);
    }
}
