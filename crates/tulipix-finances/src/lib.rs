//! Finances — core.
//!
//! Everything in this section that is not UI. Nothing here knows a window
//! exists, which is what lets the money arithmetic and the recurrence engine be
//! tested in CI with no renderer and no display.
//!
//! # The shape of the model
//!
//! Two tables carry it. [`txn`] is every posting, whatever produced it, and
//! [`recur`] is every repeating thing, with a `kind` telling a subscription from a
//! bill. Between them sits [`obligations`] — dated money that is owed but has not
//! moved yet:
//!
//! ```text
//! RECURRENCE          →  OBLIGATION              →  TRANSACTION
//! (template)             (owed, not yet money)      (money moved)
//! ```
//!
//! Subscriptions and Bills are filtered views over one table, not two tables. A
//! price change or a pause is therefore visible in every total at once, because
//! there is nowhere else for it to be.
//!
//! # Four rules this crate is held to
//!
//! Each has a test of its own, named after it:
//!
//! 1. **Transfers are not spending.** Wallet top-ups, card payments, EMI payments
//!    and due settlements post as transfers and are excluded from every spend
//!    total. See [`txn::SPEND`] and `txn::transfers_are_not_spending`.
//! 2. **Lending is not spending.** It posts as a transfer into a virtual holding
//!    account and returns when the due settles. See `dues::lending_is_not_spending`.
//! 3. **Estimates are never facts.** A variable bill's `~₹3,190` is the mean of the
//!    last three posted amounts, and a recurrence with no fixed amount can never
//!    auto-post. See `recur::a_variable_bill_never_auto_posts_even_when_asked_to`.
//! 4. **Money is `i64` minor units, never a float.** See [`money`]; the one
//!    exception is [`loans::emi_for`], which is documented where it is.

pub mod accounts;
pub mod budgets;
pub mod date;
pub mod demo;
pub mod dues;
pub mod fx;
pub mod import;
pub mod insights;
pub mod loans;
pub mod money;
pub mod obligations;
pub mod ocr;
pub mod recur;
pub mod schema;
pub mod txn;

use anyhow::Result;
use chrono::NaiveDate;
use sqlx::SqlitePool;

/// Settings key for how many days ahead an obligation starts being announced.
pub const LEAD_KEY: &str = "finances.alert_lead_days";

/// Settings key for the desktop-notification toggle.
pub const NOTIFY_KEY: &str = "finances.notify_enabled";

/// Default alert lead, in days.
pub const DEFAULT_LEAD_DAYS: i64 = 3;

/// Open `finances.db` next to the other section databases, apply the schema and
/// seed the defaults.
///
/// Inherits WAL mode and the pool tuning from the shared handle, so this section
/// behaves like every other one under concurrent reads.
pub async fn open() -> Result<SqlitePool> {
    let handle = tulipix_core::db::DbHandle::open("finances")?;
    let pool = handle.pool().await?;
    schema::apply_schema(&pool).await?;
    schema::seed_defaults(&pool).await?;
    // Sample data, once, and only on a database that has never held anything
    // real. Nine empty tabs are indistinguishable from nine broken ones.
    if let Err(e) = demo::seed(&pool, date::today()).await {
        tracing::warn!("finances: sample data not seeded: {e}");
    }
    Ok(pool)
}

fn advanced(key: &str) -> Option<String> {
    tulipix_core::settings::Settings::load().ok()?.advanced.get(key).cloned()
}

/// How many days ahead to start announcing an obligation.
pub fn lead_days() -> i64 {
    advanced(LEAD_KEY)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .map(|d| d.clamp(0, 60))
        .unwrap_or(DEFAULT_LEAD_DAYS)
}

/// Whether to raise a desktop notification. On unless turned off.
pub fn notify_enabled() -> bool {
    !matches!(advanced(NOTIFY_KEY).as_deref(), Some("false") | Some("0"))
}

/// What the tick found, so the caller knows whether to notify and whether to
/// redraw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickResult {
    /// Obligations created this run. Only these are worth a notification — the
    /// user has already seen the rest.
    pub created: Vec<i64>,
    /// Rows whose status moved to due or overdue.
    pub swept: u64,
    /// Names of everything now overdue, for the notification body.
    pub overdue: Vec<String>,
    /// Sample rows dropped because the user now has real data of that kind.
    pub demo_pruned: u64,
    /// A day inside the notice window where several things land at once, and
    /// what they add up to. The design's second notification trigger: a run of
    /// bills that is fine individually and painful on one morning.
    pub heavy_day: Option<HeavyDay>,
    /// Envelopes that went over their limit. The third trigger.
    pub broken_envelopes: Vec<String>,
}

/// Several obligations falling on one date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeavyDay {
    pub on: String,
    pub count: usize,
    pub total_minor: i64,
    pub currency: String,
}

impl TickResult {
    pub fn changed(&self) -> bool {
        !self.created.is_empty() || self.swept > 0 || self.demo_pruned > 0
    }
}

/// How many obligations on one date make it worth mentioning.
const HEAVY_DAY_ITEMS: usize = 3;

/// And how far ahead to look for one.
const HEAVY_DAY_WINDOW: i64 = 14;

/// Bring the database up to date with the calendar.
///
/// Runs on section open and on the existing scan-schedule tick, so it has to be
/// safe to call twice in the same second — and it is, because
/// `UNIQUE(recurrence_id, due_on)` refuses the duplicate insert rather than this
/// code checking first and racing with itself.
pub async fn tick(pool: &SqlitePool) -> Result<TickResult> {
    let today = date::today();
    let created = recur::materialise_due(pool, today).await?;
    let swept = obligations::sweep(pool, today).await?;
    let overdue = obligations::overdue_names(pool, today).await.unwrap_or_default();
    // Sample data steps aside as the user's own arrives. Cheap enough to check
    // every tick, and a no-op once the last sample is gone.
    let demo_pruned = demo::prune(pool).await.unwrap_or(0);
    let heavy_day = heaviest_day_ahead(pool, today).await.unwrap_or_default();
    let broken_envelopes = broken_envelopes(pool, today).await.unwrap_or_default();
    Ok(TickResult { created, swept, overdue, demo_pruned, heavy_day, broken_envelopes })
}

/// The worst single day in the next two weeks, when there is one worth naming.
///
/// Only counts what is still open: a day whose bills are already paid is not
/// something to warn about, and warning about it once a month for the rest of
/// the month is how a notification becomes noise the user turns off.
async fn heaviest_day_ahead(pool: &SqlitePool, today: NaiveDate) -> Result<Option<HeavyDay>> {
    let base = fx::base_currency();
    let items = obligations::needs_you(pool, today, HEAVY_DAY_WINDOW).await?;
    let mut by_day: std::collections::BTreeMap<String, (usize, i64)> = Default::default();
    for o in items {
        if !o.status.is_open() {
            continue;
        }
        let Some(amount) = o.shown_minor() else { continue };
        let slot = by_day.entry(o.due_on.clone()).or_default();
        slot.0 += 1;
        slot.1 += amount;
    }
    Ok(by_day
        .into_iter()
        .filter(|(_, (count, _))| *count >= HEAVY_DAY_ITEMS)
        .max_by_key(|(_, (_, total))| *total)
        .map(|(on, (count, total_minor))| HeavyDay {
            on,
            count,
            total_minor,
            currency: base.clone(),
        }))
}

/// Envelopes over their limit this month.
async fn broken_envelopes(pool: &SqlitePool, today: NaiveDate) -> Result<Vec<String>> {
    Ok(budgets::list(pool, &date::ym(today))
        .await?
        .into_iter()
        .filter(|b| b.id.is_some() && b.over_budget())
        .map(|b| b.category_name)
        .collect())
}

/// One line for a desktop notification, or `None` when there is nothing worth
/// interrupting the user for.
///
/// Kept here rather than in the UI crate because the wording is a product
/// decision and this is the crate with the tests.
/// The three triggers, in the order the design fixes them: something is late,
/// a heavy day is coming, an envelope broke. One notification, not three — the
/// most urgent thing wins, and the rest are on the page when it opens.
pub fn notice(t: &TickResult) -> Option<(String, String)> {
    if !notify_enabled() {
        return None;
    }
    match t.overdue.len() {
        0 => {}
        1 => {
            return Some((
                format!("{} is late", t.overdue[0]),
                "Open Finances to mark it paid.".into(),
            ))
        }
        n => return Some((format!("{n} bills are late"), t.overdue.join(", "))),
    }

    if let Some(h) = &t.heavy_day {
        return Some((
            format!("{} clears on {}", money::format_minor(h.total_minor, &h.currency), h.on),
            format!("{} things land on the same day.", h.count),
        ));
    }

    match t.broken_envelopes.len() {
        0 => {}
        1 => {
            return Some((
                format!("{} is over budget", t.broken_envelopes[0]),
                "Open Budgets to see by how much.".into(),
            ))
        }
        n => {
            return Some((
                format!("{n} envelopes are over budget"),
                t.broken_envelopes.join(", "),
            ))
        }
    }

    // Nothing wrong, but the calendar grew. Worth a line, not an alarm.
    match t.created.len() {
        0 => None,
        n => Some((
            format!("{n} thing{} coming up", if n == 1 { "" } else { "s" }),
            "Finances has new obligations on the calendar.".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three triggers in the order the design fixes them, and only ever one
    /// notification. A tick that found all three at once must not fire three
    /// banners — the other two are on the page when it opens.
    #[test]
    fn the_most_urgent_trigger_is_the_one_that_notifies() {
        let heavy = HeavyDay {
            on: "2026-08-05".into(),
            count: 4,
            total_minor: 6_175_200,
            currency: "INR".into(),
        };
        let all = TickResult {
            created: vec![1, 2],
            swept: 0,
            overdue: vec!["Water".into()],
            demo_pruned: 0,
            heavy_day: Some(heavy.clone()),
            broken_envelopes: vec!["Eating out".into()],
        };
        assert!(
            notice(&all).unwrap().0.contains("Water"),
            "late beats a heavy day and a broken envelope"
        );

        let no_overdue = TickResult { overdue: Vec::new(), ..all.clone() };
        assert!(
            notice(&no_overdue).unwrap().0.contains("2026-08-05"),
            "a heavy day beats a broken envelope"
        );

        let only_envelope =
            TickResult { heavy_day: None, ..no_overdue.clone() };
        assert!(notice(&only_envelope).unwrap().0.contains("Eating out"));

        // Nothing wrong at all, but the calendar grew: a line, not an alarm.
        let quiet = TickResult { broken_envelopes: Vec::new(), ..only_envelope.clone() };
        assert!(notice(&quiet).unwrap().0.contains("2 things coming up"));

        // And a tick that found nothing says nothing.
        assert_eq!(notice(&TickResult::default()), None);
    }

    /// The real open path, against a temp file rather than the user's data dir,
    /// so WAL mode and the on-disk schema are exercised and not just
    /// `sqlite::memory:`.
    #[tokio::test]
    async fn a_fresh_database_opens_seeded_and_reopens_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("finances.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let handle = tulipix_core::db::DbHandle { section: "finances".into(), path, url };

        let pool = handle.pool().await.unwrap();
        schema::apply_schema(&pool).await.unwrap();
        schema::seed_defaults(&pool).await.unwrap();

        let mode: String =
            sqlx::query_scalar("PRAGMA journal_mode").fetch_one(&pool).await.unwrap();
        assert_eq!(mode.to_lowercase(), "wal");

        let cats: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM categories").fetch_one(&pool).await.unwrap();
        assert!(cats > 0);

        // Second open, as happens on every launch.
        schema::apply_schema(&pool).await.unwrap();
        schema::seed_defaults(&pool).await.unwrap();
        let again: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM categories").fetch_one(&pool).await.unwrap();
        assert_eq!(again, cats);
    }

    #[tokio::test]
    async fn a_double_tick_is_a_no_op_the_second_time() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&pool).await.unwrap();
        schema::seed_defaults(&pool).await.unwrap();

        // Dated in the past so it is due whenever this test happens to run.
        recur::create(&pool, &recur::NewRecurrence::subscription("Netflix", 64_900, "2020-01-31"))
            .await
            .unwrap();

        let first = tick(&pool).await.unwrap();
        assert!(first.changed());
        let second = tick(&pool).await.unwrap();
        assert!(second.created.is_empty(), "the tick can fire twice in one second");
    }

    #[test]
    fn the_alert_lead_falls_back_and_is_bounded() {
        // No settings file in a test environment, so this exercises the default.
        assert_eq!(lead_days(), DEFAULT_LEAD_DAYS);
        assert!(notify_enabled());
    }

    #[test]
    fn a_quiet_tick_says_nothing() {
        // The most important case: no banner when nothing happened. An app that
        // notifies on every startup gets its notifications turned off.
        assert_eq!(notice(&TickResult::default()), None);
    }

    #[test]
    fn one_late_bill_is_named_and_several_are_counted() {
        let one = TickResult { overdue: vec!["Water".into()], ..Default::default() };
        let (title, _) = notice(&one).unwrap();
        assert_eq!(title, "Water is late");

        let many =
            TickResult { overdue: vec!["Water".into(), "Rent".into()], ..Default::default() };
        let (title, body) = notice(&many).unwrap();
        assert_eq!(title, "2 bills are late");
        assert_eq!(body, "Water, Rent", "the body names them, the title counts them");
    }

    #[test]
    fn new_obligations_alone_are_announced_without_alarm() {
        let t = TickResult { created: vec![1, 2, 3], ..Default::default() };
        let (title, _) = notice(&t).unwrap();
        assert_eq!(title, "3 things coming up");
        // Singular, because "1 things" is how software announces it was written
        // carelessly.
        let one = TickResult { created: vec![9], ..Default::default() };
        assert_eq!(notice(&one).unwrap().0, "1 thing coming up");
    }

    #[tokio::test]
    async fn a_tick_reports_what_is_overdue_by_name() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&pool).await.unwrap();
        schema::seed_defaults(&pool).await.unwrap();
        // One-offs rather than a recurrence: a monthly template dated years back
        // catches up every missed period, which is correct behaviour and useless
        // for testing the wording.
        obligations::create_one_off(&pool, "Water", "2020-03-01", Some(100_000)).await.unwrap();

        let t = tick(&pool).await.unwrap();
        assert_eq!(t.overdue, ["Water"]);
        assert!(t.swept > 0, "it moved to overdue on this tick");
        assert_eq!(notice(&t).unwrap().0, "Water is late");
    }
}
