//! Recurrences: every repeating thing, subscription and bill alike.
//!
//! The design's central observation is that a subscription, a bill and an expense
//! are one thing at three points in its life:
//!
//! ```text
//! RECURRENCE          →  OBLIGATION              →  TRANSACTION
//! (template)             (owed, not yet money)      (money moved)
//! Netflix ₹649/mo        due 31 Jul, auto           31 Jul, −₹649
//! Electricity            due 12 Aug, ~₹3,190 est    12 Aug, −₹3,284
//! ```
//!
//! So this is one table with a `kind`, not two tables. Subscriptions and Bills
//! are filtered views over it, which is why a price change or a pause is visible
//! in every total at once — there is nowhere else for it to be.
//!
//! **The honesty rule this module enforces:** an estimate is never a fact. A
//! variable bill (`amount_minor IS NULL`) gets `estimate_minor` from
//! [`estimate_for`] and is never auto-posted, whatever `auto_post` says. See
//! [`materialise_due`].

use anyhow::{bail, Result};
use chrono::NaiveDate;
use sqlx::SqlitePool;

use crate::date::{self, Cycle};
use crate::money;

/// A subscription is fixed and posts itself; a bill varies and needs confirming.
/// They share a table and differ on those two axes, which is also exactly what
/// the importer's detector uses to tell them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecurKind {
    Subscription,
    Bill,
}

impl RecurKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RecurKind::Subscription => "subscription",
            RecurKind::Bill => "bill",
        }
    }

    pub fn parse(s: &str) -> RecurKind {
        if s == "bill" { RecurKind::Bill } else { RecurKind::Subscription }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Active,
    Paused,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Paused => "paused",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "paused" => Status::Paused,
            "cancelled" => Status::Cancelled,
            _ => Status::Active,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewRecurrence {
    pub kind: RecurKind,
    pub name: String,
    /// `None` means the amount varies — a variable bill. That single `NULL` is
    /// what decides whether this thing can ever auto-post.
    pub amount_minor: Option<i64>,
    pub currency: String,
    pub cycle: Cycle,
    pub anchor_day: Option<i64>,
    pub next_due_on: Option<String>,
    pub account_id: Option<i64>,
    pub category_id: Option<i64>,
    pub auto_post: bool,
    pub reminder_days: i64,
    pub note: Option<String>,
}

impl NewRecurrence {
    pub fn subscription(name: &str, amount_minor: i64, next_due_on: &str) -> Self {
        let day = date::parse(next_due_on).map(|d| chrono::Datelike::day(&d) as i64).ok();
        NewRecurrence {
            kind: RecurKind::Subscription,
            name: name.to_string(),
            amount_minor: Some(amount_minor),
            currency: "INR".to_string(),
            cycle: Cycle::Monthly,
            anchor_day: day,
            next_due_on: Some(next_due_on.to_string()),
            account_id: None,
            category_id: None,
            auto_post: true,
            reminder_days: 3,
            note: None,
        }
    }

    /// A bill whose amount is not known in advance.
    pub fn variable_bill(name: &str, next_due_on: &str) -> Self {
        NewRecurrence {
            kind: RecurKind::Bill,
            amount_minor: None,
            auto_post: false,
            ..NewRecurrence::subscription(name, 0, next_due_on)
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecurRow {
    pub id: i64,
    pub kind: RecurKind,
    pub name: String,
    pub amount_minor: Option<i64>,
    pub currency: String,
    pub cycle: Cycle,
    pub anchor_day: Option<i64>,
    pub next_due_on: Option<String>,
    pub account_id: Option<i64>,
    pub account_name: Option<String>,
    pub category_id: Option<i64>,
    pub category_name: Option<String>,
    pub auto_post: bool,
    pub status: Status,
    pub reminder_days: i64,
    pub note: Option<String>,
    /// Mean of the last three posted amounts, for a variable bill.
    pub estimate_minor: Option<i64>,
    /// The most recent posted amount, and when.
    pub last_paid_minor: Option<i64>,
    pub last_paid_on: Option<String>,
    /// Set when the current price is above the oldest recorded one.
    pub hike_from_minor: Option<i64>,
    /// Cost per year at the current price. Zero for an irregular cycle.
    pub yearly_minor: i64,
}

impl RecurRow {
    /// The figure to show: the fixed amount, or the estimate for a variable bill.
    pub fn shown_minor(&self) -> Option<i64> {
        self.amount_minor.or(self.estimate_minor)
    }

    /// Is the shown figure a guess rather than a fact? Drives the `~` prefix.
    pub fn is_estimate(&self) -> bool {
        self.amount_minor.is_none()
    }
}

/// Recurrences of one kind, with their derived figures.
pub async fn list(pool: &SqlitePool, kind: Option<RecurKind>, include_cancelled: bool) -> Result<Vec<RecurRow>> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            Option<i64>,
            String,
            String,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<i64>,
            Option<String>,
            i64,
            String,
            i64,
            Option<String>,
        ),
    >(
        "SELECT r.id, r.kind, r.name, r.amount_minor, r.currency, r.cycle, r.anchor_day,
                r.next_due_on, r.account_id, a.name, r.category_id, c.name, r.auto_post,
                r.status, r.reminder_days, r.note
           FROM recurrences r
           LEFT JOIN accounts   a ON a.id = r.account_id
           LEFT JOIN categories c ON c.id = r.category_id
          WHERE (? IS NULL OR r.kind = ?)
            AND (? = 1 OR r.status <> 'cancelled')
          ORDER BY r.next_due_on IS NULL, r.next_due_on, r.name COLLATE NOCASE",
    )
    .bind(kind.map(|k| k.as_str()))
    .bind(kind.map(|k| k.as_str()))
    .bind(i64::from(include_cancelled))
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id = r.0;
        let cycle = Cycle::parse(&r.5);
        let amount_minor = r.3;
        let estimate_minor = if amount_minor.is_none() { estimate_for(pool, id).await? } else { None };
        let (last_paid_minor, last_paid_on) = last_posted(pool, id).await?;
        let hike_from_minor = hike_from(pool, id, amount_minor).await?;
        let shown = amount_minor.or(estimate_minor).unwrap_or(0);

        out.push(RecurRow {
            id,
            kind: RecurKind::parse(&r.1),
            name: r.2,
            amount_minor,
            currency: r.4,
            cycle,
            anchor_day: r.6,
            next_due_on: r.7,
            account_id: r.8,
            account_name: r.9,
            category_id: r.10,
            category_name: r.11,
            auto_post: r.12 != 0,
            status: Status::parse(&r.13),
            reminder_days: r.14,
            note: r.15,
            estimate_minor,
            last_paid_minor,
            last_paid_on,
            hike_from_minor,
            yearly_minor: shown * cycle.per_year(),
        });
    }
    Ok(out)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<RecurRow>> {
    Ok(list(pool, None, true).await?.into_iter().find(|r| r.id == id))
}

pub async fn create(pool: &SqlitePool, r: &NewRecurrence) -> Result<i64> {
    if r.name.trim().is_empty() {
        bail!("a recurrence needs a name");
    }
    if let Some(a) = r.amount_minor
        && a < 0
    {
        bail!("amount_minor must be positive");
    }
    // A variable bill cannot auto-post whatever the caller asked for: posting an
    // estimate would write a guess into the ledger as a fact.
    let auto_post = r.auto_post && r.amount_minor.is_some();

    let id = sqlx::query(
        "INSERT INTO recurrences
           (kind, name, amount_minor, currency, cycle, anchor_day, next_due_on, account_id,
            category_id, auto_post, status, reminder_days, note, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
    )
    .bind(r.kind.as_str())
    .bind(r.name.trim())
    .bind(r.amount_minor)
    .bind(r.currency.to_uppercase())
    .bind(r.cycle.as_str())
    .bind(r.anchor_day.map(|d| d.clamp(1, 31)))
    .bind(&r.next_due_on)
    .bind(r.account_id)
    .bind(r.category_id)
    .bind(i64::from(auto_post))
    .bind(r.reminder_days.max(0))
    .bind(&r.note)
    .bind(crate::schema::unix_now())
    .execute(pool)
    .await?
    .last_insert_rowid();

    // The opening price is part of the price history, or the first hike has
    // nothing to be a hike from.
    if let Some(a) = r.amount_minor {
        let from = r.next_due_on.clone().unwrap_or_else(|| date::iso(date::today()));
        record_price(pool, id, a, &r.currency, &from).await?;
    }
    Ok(id)
}

/// Update a recurrence, recording a price change when the amount moved.
pub async fn update(pool: &SqlitePool, id: i64, r: &NewRecurrence) -> Result<()> {
    if r.name.trim().is_empty() {
        bail!("a recurrence needs a name");
    }
    let previous: Option<i64> =
        sqlx::query_scalar("SELECT amount_minor FROM recurrences WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let auto_post = r.auto_post && r.amount_minor.is_some();

    sqlx::query(
        "UPDATE recurrences
            SET kind = ?, name = ?, amount_minor = ?, currency = ?, cycle = ?, anchor_day = ?,
                next_due_on = ?, account_id = ?, category_id = ?, auto_post = ?,
                reminder_days = ?, note = ?
          WHERE id = ?",
    )
    .bind(r.kind.as_str())
    .bind(r.name.trim())
    .bind(r.amount_minor)
    .bind(r.currency.to_uppercase())
    .bind(r.cycle.as_str())
    .bind(r.anchor_day.map(|d| d.clamp(1, 31)))
    .bind(&r.next_due_on)
    .bind(r.account_id)
    .bind(r.category_id)
    .bind(i64::from(auto_post))
    .bind(r.reminder_days.max(0))
    .bind(&r.note)
    .bind(id)
    .execute(pool)
    .await?;

    if let Some(new) = r.amount_minor
        && previous != Some(new)
    {
        record_price(pool, id, new, &r.currency, &date::iso(date::today())).await?;
    }
    Ok(())
}

pub async fn set_status(pool: &SqlitePool, id: i64, status: Status) -> Result<()> {
    sqlx::query("UPDATE recurrences SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(id)
        .execute(pool)
        .await?;
    // A paused or cancelled thing must stop producing obligations, and the ones
    // it already produced but that were never paid are no longer owed.
    if status != Status::Active {
        sqlx::query(
            "DELETE FROM obligations
              WHERE recurrence_id = ? AND status IN ('upcoming', 'due', 'overdue')",
        )
        .bind(id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    // Postings keep their history; they simply stop claiming a template.
    sqlx::query("UPDATE transactions SET recurrence_id = NULL WHERE recurrence_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    // And stop claiming the obligation, which is about to be cascaded away with
    // the recurrence. Without this the DELETE below fails on the foreign key from
    // transactions.obligation_id — an auto-posting subscription cannot be deleted
    // at all once it has posted once.
    sqlx::query(
        "UPDATE transactions SET obligation_id = NULL
          WHERE obligation_id IN (SELECT id FROM obligations WHERE recurrence_id = ?)",
    )
    .bind(id)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM recurrences WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

// ── price history ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Price {
    pub amount_minor: i64,
    pub currency: String,
    pub effective_from: String,
}

pub async fn record_price(
    pool: &SqlitePool,
    recurrence_id: i64,
    amount_minor: i64,
    currency: &str,
    effective_from: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO recurrence_prices (recurrence_id, amount_minor, currency, effective_from)
         VALUES (?, ?, ?, ?)",
    )
    .bind(recurrence_id)
    .bind(amount_minor)
    .bind(currency.to_uppercase())
    .bind(effective_from)
    .execute(pool)
    .await?;
    Ok(())
}

/// Price history, oldest first.
pub async fn price_history(pool: &SqlitePool, recurrence_id: i64) -> Result<Vec<Price>> {
    let rows = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT amount_minor, currency, effective_from FROM recurrence_prices
          WHERE recurrence_id = ? ORDER BY effective_from, id",
    )
    .bind(recurrence_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(amount_minor, currency, effective_from)| Price { amount_minor, currency, effective_from })
        .collect())
}

/// The oldest recorded price, when it is below the current one.
///
/// This is the subscription drift signal: streaming services raise prices
/// quietly, and the only reason the user notices is that the app remembers what
/// it used to cost.
async fn hike_from(pool: &SqlitePool, id: i64, current: Option<i64>) -> Result<Option<i64>> {
    let Some(current) = current else { return Ok(None) };
    let oldest: Option<i64> = sqlx::query_scalar(
        "SELECT amount_minor FROM recurrence_prices
          WHERE recurrence_id = ? ORDER BY effective_from, id LIMIT 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(oldest.filter(|o| *o < current))
}

// ── estimates ───────────────────────────────────────────────────────────────

/// How many past actuals the estimate averages.
pub const ESTIMATE_SAMPLE: i64 = 3;

/// Estimates round to the nearest ₹10, in minor units.
pub const ESTIMATE_STEP: i64 = 1_000;

/// Mean of the last three posted actuals, rounded to the nearest ₹10.
///
/// Rounded on purpose. A variable bill estimate of `~₹3,187.43` reads as a
/// measurement; `~₹3,190` reads as the guess it is. Returns `None` when there is
/// nothing posted yet — no history means no estimate, and inventing one would be
/// the exact failure the honesty rule exists to prevent.
pub async fn estimate_for(pool: &SqlitePool, recurrence_id: i64) -> Result<Option<i64>> {
    let amounts = sqlx::query_scalar::<_, i64>(
        "SELECT actual_minor FROM obligations
          WHERE recurrence_id = ? AND status = 'paid' AND actual_minor IS NOT NULL
          ORDER BY due_on DESC LIMIT ?",
    )
    .bind(recurrence_id)
    .bind(ESTIMATE_SAMPLE)
    .fetch_all(pool)
    .await?;

    if amounts.is_empty() {
        return Ok(None);
    }
    // Integer mean over however many are available — fewer than three is still
    // better than nothing, and the alternative is showing no figure at all for
    // the first two months of every new bill.
    let sum: i64 = amounts.iter().sum();
    let mean = sum / amounts.len() as i64;
    Ok(Some(money::round_to(mean, ESTIMATE_STEP)))
}

/// The most recent posted amount and date for a recurrence.
async fn last_posted(pool: &SqlitePool, recurrence_id: i64) -> Result<(Option<i64>, Option<String>)> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT amount_minor, occurred_on FROM transactions
          WHERE recurrence_id = ? ORDER BY occurred_on DESC, id DESC LIMIT 1",
    )
    .bind(recurrence_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((a, d)) => (Some(a), Some(d)),
        None => (None, None),
    })
}

// ── the engine ──────────────────────────────────────────────────────────────

/// Turn due recurrences into obligations, and auto-post the ones that may be.
///
/// Runs on section open and on the scan-schedule tick. Both can fire in the same
/// second, so this has to be idempotent — which it is, because
/// `UNIQUE(recurrence_id, due_on)` refuses the second insert rather than this
/// code checking first and racing.
///
/// Returns the obligations it created, so the caller can raise a notification for
/// exactly those and not for ones the user has already seen.
///
/// How far ahead to look is per recurrence, not global: `reminder_days` is a
/// column because rent wants a week's warning and a ₹149 subscription wants
/// none. The `finances.alert_lead_days` setting is the default a new recurrence
/// starts with, not an override applied here.
pub async fn materialise_due(pool: &SqlitePool, today: NaiveDate) -> Result<Vec<i64>> {
    let today_iso = date::iso(today);

    // The horizon is computed per row in SQL rather than in Rust, so a
    // recurrence with a 14-day reminder is picked up on the same pass as one
    // with none.
    let due = sqlx::query_as::<_, (i64, String, Option<i64>, String, String, Option<i64>, i64, Option<i64>, Option<i64>, i64)>(
        "SELECT id, name, amount_minor, currency, cycle, anchor_day, auto_post, account_id,
                category_id, reminder_days
           FROM recurrences
          WHERE status = 'active'
            AND next_due_on IS NOT NULL
            AND next_due_on <= date(?, '+' || MAX(reminder_days, 0) || ' days')",
    )
    .bind(&today_iso)
    .fetch_all(pool)
    .await?;

    let mut created = Vec::new();

    for (id, name, amount_minor, currency, cycle_s, anchor_day, auto_post, account_id, _category_id, reminder_days) in due {
        let cycle = Cycle::parse(&cycle_s);
        let anchor = anchor_day.map(|d| d as u32);
        let horizon = date::iso(today + chrono::Duration::days(reminder_days.clamp(0, 60)));

        // Catch up one cycle at a time. The app may not have been opened for
        // months, and each missed period is a real obligation that happened —
        // skipping straight to the next due date would lose them all.
        let mut cursor = current_due(pool, id).await?;
        let mut guard = 0;
        while let Some(due_on) = cursor {
            if date::iso(due_on) > horizon || guard > 600 {
                break;
            }
            guard += 1;

            let estimate = if amount_minor.is_none() { estimate_for(pool, id).await? } else { None };
            let inserted = sqlx::query(
                "INSERT OR IGNORE INTO obligations
                   (recurrence_id, name, due_on, estimate_minor, status)
                 VALUES (?, ?, ?, ?, 'upcoming')",
            )
            .bind(id)
            .bind(&name)
            .bind(date::iso(due_on))
            .bind(amount_minor.or(estimate))
            .execute(pool)
            .await?;

            if inserted.rows_affected() == 1 {
                let obligation_id = inserted.last_insert_rowid();
                created.push(obligation_id);

                // Auto-post only a fixed amount, only with somewhere to post it,
                // and only once it is actually due rather than merely visible on
                // the horizon.
                if auto_post == 1
                    && let Some(fixed) = amount_minor
                    && let Some(account) = account_id
                    && due_on <= today
                {
                    crate::obligations::mark_paid(
                        pool,
                        obligation_id,
                        fixed,
                        due_on,
                        account,
                        &currency,
                    )
                    .await?;
                }
            }

            let next = date::advance(due_on, cycle, anchor);
            if cycle == Cycle::Irregular || next <= due_on {
                // Nothing repeats; one obligation is all there is.
                cursor = None;
            } else {
                cursor = Some(next);
                sqlx::query("UPDATE recurrences SET next_due_on = ? WHERE id = ?")
                    .bind(date::iso(next))
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
        }
    }
    Ok(created)
}

/// The date a recurrence is currently pointed at.
async fn current_due(pool: &SqlitePool, id: i64) -> Result<Option<NaiveDate>> {
    let s: Option<String> = sqlx::query_scalar("SELECT next_due_on FROM recurrences WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .flatten();
    Ok(s.and_then(|s| date::parse(&s).ok()))
}

/// Total cost per year of every active recurrence, in base-currency minor units.
///
/// Foreign-currency rows are converted at their stored rate, so a USD
/// subscription counts at what it actually costs rather than at its face value.
pub async fn yearly_total(pool: &SqlitePool, base: &str) -> Result<i64> {
    let rows = list(pool, None, false).await?;
    let mut total = 0i64;
    for r in rows {
        if r.status != Status::Active {
            continue;
        }
        let Some(shown) = r.shown_minor() else { continue };
        let rate = crate::fx::rate_for(pool, &r.currency, base).await?;
        total += money::convert(shown, rate) * r.cycle.per_year();
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts, obligations, schema};

    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&p).await.unwrap();
        schema::seed_defaults(&p).await.unwrap();
        p
    }

    fn d(s: &str) -> NaiveDate {
        date::parse(s).unwrap()
    }

    async fn bank(p: &SqlitePool) -> i64 {
        accounts::create(p, &accounts::NewAccount::bank("HDFC", 10_000_000)).await.unwrap()
    }

    #[tokio::test]
    async fn materialising_twice_creates_one_obligation() {
        // The scan tick and section-open can both fire in the same second.
        let p = pool().await;
        let id = create(&p, &NewRecurrence::subscription("Netflix", 64_900, "2026-07-31")).await.unwrap();

        let first = materialise_due(&p, d("2026-07-31")).await.unwrap();
        let second = materialise_due(&p, d("2026-07-31")).await.unwrap();
        assert_eq!(first.len(), 1);
        assert!(second.is_empty(), "the unique index makes the second tick a no-op");

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM obligations WHERE recurrence_id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn a_variable_bill_never_auto_posts_even_when_asked_to() {
        // Honesty rule 3, with a test of its own as the spec requires.
        let p = pool().await;
        let acct = bank(&p).await;
        let mut r = NewRecurrence::variable_bill("Electricity", "2026-07-12");
        r.auto_post = true; // the caller insists
        r.account_id = Some(acct);
        let id = create(&p, &r).await.unwrap();

        let stored: i64 = sqlx::query_scalar("SELECT auto_post FROM recurrences WHERE id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(stored, 0, "an estimate must never be able to post itself");

        materialise_due(&p, d("2026-07-12")).await.unwrap();
        let posted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(posted, 0);
        let status: String = sqlx::query_scalar("SELECT status FROM obligations").fetch_one(&p).await.unwrap();
        assert_eq!(status, "upcoming", "it is owed, not paid");
    }

    #[tokio::test]
    async fn a_fixed_subscription_with_an_account_posts_itself() {
        let p = pool().await;
        let acct = bank(&p).await;
        let mut r = NewRecurrence::subscription("Netflix", 64_900, "2026-07-31");
        r.account_id = Some(acct);
        create(&p, &r).await.unwrap();

        materialise_due(&p, d("2026-07-31")).await.unwrap();
        let (status, actual): (String, Option<i64>) =
            sqlx::query_as("SELECT status, actual_minor FROM obligations").fetch_one(&p).await.unwrap();
        assert_eq!(status, "paid");
        assert_eq!(actual, Some(64_900));
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000 - 64_900);
    }

    #[tokio::test]
    async fn a_subscription_visible_on_the_horizon_is_not_posted_yet() {
        let p = pool().await;
        let acct = bank(&p).await;
        let mut r = NewRecurrence::subscription("Netflix", 64_900, "2026-07-31");
        r.account_id = Some(acct);
        r.reminder_days = 7;
        create(&p, &r).await.unwrap();

        // Five days ahead of time, with a seven-day lead.
        materialise_due(&p, d("2026-07-26")).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM obligations WHERE due_on = '2026-07-31'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(status, "upcoming", "due on the 31st means due on the 31st");
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000);
    }

    #[tokio::test]
    async fn the_reminder_lead_is_per_recurrence() {
        // The whole reason reminder_days is a column: rent wants a fortnight's
        // warning, a ₹149 subscription wants none, and one global number cannot
        // serve both.
        let p = pool().await;
        let mut early = NewRecurrence::subscription("Rent", 3_800_000, "2026-08-05");
        early.reminder_days = 14;
        let early_id = create(&p, &early).await.unwrap();

        let mut late = NewRecurrence::subscription("Spotify", 14_900, "2026-08-05");
        late.reminder_days = 0;
        let late_id = create(&p, &late).await.unwrap();

        materialise_due(&p, d("2026-07-26")).await.unwrap();

        let seen: Vec<i64> = sqlx::query_scalar(
            "SELECT recurrence_id FROM obligations WHERE recurrence_id IN (?, ?)",
        )
        .bind(early_id)
        .bind(late_id)
        .fetch_all(&p)
        .await
        .unwrap();
        assert_eq!(seen, [early_id], "a fortnight's lead reaches it; no lead does not");
    }

    #[tokio::test]
    async fn catching_up_after_months_away_creates_every_missed_period() {
        let p = pool().await;
        let id = create(&p, &NewRecurrence::subscription("Rent", 3_800_000, "2026-04-05")).await.unwrap();
        // Opened again in late July: April, May, June and July were all owed.
        let created = materialise_due(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(created.len(), 4);

        let dates: Vec<String> = sqlx::query_scalar(
            "SELECT due_on FROM obligations WHERE recurrence_id = ? ORDER BY due_on",
        )
        .bind(id)
        .fetch_all(&p)
        .await
        .unwrap();
        assert_eq!(dates, ["2026-04-05", "2026-05-05", "2026-06-05", "2026-07-05"]);

        // And it is now pointed at August, not still at April.
        let next: Option<String> = sqlx::query_scalar("SELECT next_due_on FROM recurrences WHERE id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(next.as_deref(), Some("2026-08-05"));
    }

    #[tokio::test]
    async fn a_month_end_anchor_survives_february() {
        let p = pool().await;
        let mut r = NewRecurrence::subscription("Rent", 3_800_000, "2026-01-31");
        r.anchor_day = Some(31);
        let id = create(&p, &r).await.unwrap();
        materialise_due(&p, d("2026-04-01")).await.unwrap();

        let dates: Vec<String> =
            sqlx::query_scalar("SELECT due_on FROM obligations WHERE recurrence_id = ? ORDER BY due_on")
                .bind(id)
                .fetch_all(&p)
                .await
                .unwrap();
        assert_eq!(
            dates,
            ["2026-01-31", "2026-02-28", "2026-03-31"],
            "clamped in February, back to the 31st in March"
        );
    }

    #[tokio::test]
    async fn an_irregular_recurrence_produces_one_obligation_and_stops() {
        let p = pool().await;
        let mut r = NewRecurrence::variable_bill("Car service", "2026-07-20");
        r.cycle = Cycle::Irregular;
        let id = create(&p, &r).await.unwrap();
        materialise_due(&p, d("2026-12-31")).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM obligations WHERE recurrence_id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(n, 1, "no period means no second instance to invent");
    }

    #[tokio::test]
    async fn the_estimate_is_the_mean_of_three_rounded_to_ten_rupees() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewRecurrence::variable_bill("Electricity", "2026-04-12")).await.unwrap();

        assert_eq!(estimate_for(&p, id).await.unwrap(), None, "no history, no guess");

        // Four paid months; only the last three count.
        for (due, amount) in
            [("2026-04-12", 900_000i64), ("2026-05-12", 300_000), ("2026-06-12", 320_000), ("2026-07-12", 328_400)]
        {
            pay(&p, id, acct, due, amount).await;
        }

        // (300000 + 320000 + 328400) / 3 = 316133 -> nearest ₹10 = 316000.
        // Note the ₹9,000 April outlier is excluded, which is the point of
        // averaging three rather than everything.
        assert_eq!(estimate_for(&p, id).await.unwrap(), Some(316_000));
    }

    /// Materialise one obligation for `recurrence` on `due` and pay it.
    async fn pay(p: &SqlitePool, recurrence: i64, account: i64, due: &str, amount: i64) {
        let obl = sqlx::query(
            "INSERT INTO obligations (recurrence_id, name, due_on, status)
             VALUES (?, 'test', ?, 'upcoming')",
        )
        .bind(recurrence)
        .bind(due)
        .execute(p)
        .await
        .unwrap()
        .last_insert_rowid();
        obligations::mark_paid(p, obl, amount, d(due), account, "INR").await.unwrap();
    }

    #[tokio::test]
    async fn the_estimate_works_with_fewer_than_three_actuals() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewRecurrence::variable_bill("Water", "2026-07-01")).await.unwrap();
        pay(&p, id, acct, "2026-07-01", 123_400).await;
        assert_eq!(estimate_for(&p, id).await.unwrap(), Some(123_000));

        pay(&p, id, acct, "2026-08-01", 100_600).await;
        // (123400 + 100600) / 2 = 112000, already a round ₹10.
        assert_eq!(estimate_for(&p, id).await.unwrap(), Some(112_000));
    }

    #[tokio::test]
    async fn an_estimate_is_marked_as_one_and_a_fixed_amount_is_not() {
        let p = pool().await;
        create(&p, &NewRecurrence::subscription("Netflix", 64_900, "2026-07-31")).await.unwrap();
        create(&p, &NewRecurrence::variable_bill("Electricity", "2026-07-12")).await.unwrap();

        let rows = list(&p, None, false).await.unwrap();
        let netflix = rows.iter().find(|r| r.name == "Netflix").unwrap();
        let power = rows.iter().find(|r| r.name == "Electricity").unwrap();
        assert!(!netflix.is_estimate());
        assert!(power.is_estimate());
    }

    #[tokio::test]
    async fn a_price_rise_is_remembered_and_flagged() {
        let p = pool().await;
        let id = create(&p, &NewRecurrence::subscription("JioHotstar", 29_900, "2026-01-15")).await.unwrap();
        let mut r = NewRecurrence::subscription("JioHotstar", 49_900, "2026-07-15");
        r.currency = "INR".into();
        update(&p, id, &r).await.unwrap();

        let history = price_history(&p, id).await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].amount_minor, 29_900);
        assert_eq!(history[1].amount_minor, 49_900);

        let row = get(&p, id).await.unwrap().unwrap();
        assert_eq!(row.hike_from_minor, Some(29_900), "the user only notices because we remembered");
    }

    #[tokio::test]
    async fn updating_without_changing_the_price_records_nothing_new() {
        let p = pool().await;
        let id = create(&p, &NewRecurrence::subscription("Netflix", 64_900, "2026-07-31")).await.unwrap();
        let mut r = NewRecurrence::subscription("Netflix (family)", 64_900, "2026-07-31");
        r.note = Some("renamed".into());
        update(&p, id, &r).await.unwrap();
        assert_eq!(price_history(&p, id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn pausing_withdraws_the_obligations_that_were_never_paid() {
        let p = pool().await;
        let acct = bank(&p).await;
        let mut r = NewRecurrence::subscription("Netflix", 64_900, "2026-06-30");
        r.account_id = Some(acct);
        let id = create(&p, &r).await.unwrap();
        materialise_due(&p, d("2026-07-31")).await.unwrap();

        let paid_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM obligations WHERE status = 'paid'")
                .fetch_one(&p)
                .await
                .unwrap();
        set_status(&p, id, Status::Paused).await.unwrap();

        let unpaid: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM obligations WHERE status IN ('upcoming','due','overdue')",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(unpaid, 0, "a paused subscription is not owed");
        let paid_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM obligations WHERE status = 'paid'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(paid_after, paid_before, "what was actually paid stays paid");

        // And a later tick must not resurrect it.
        assert!(materialise_due(&p, d("2026-09-01")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_a_recurrence_keeps_the_money_it_moved() {
        let p = pool().await;
        let acct = bank(&p).await;
        let mut r = NewRecurrence::subscription("Netflix", 64_900, "2026-07-01");
        r.account_id = Some(acct);
        let id = create(&p, &r).await.unwrap();
        materialise_due(&p, d("2026-07-01")).await.unwrap();

        delete(&p, id).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 1, "the ₹649 really did leave the account");
        // Both links have to be dropped, not just the template one: the
        // obligation is cascaded away with the recurrence, so a transaction still
        // pointing at it makes the delete fail on a foreign key.
        let (recurrence, obligation): (Option<i64>, Option<i64>) =
            sqlx::query_as("SELECT recurrence_id, obligation_id FROM transactions")
                .fetch_one(&p)
                .await
                .unwrap();
        assert!(recurrence.is_none());
        assert!(obligation.is_none());
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000 - 64_900);
    }

    #[tokio::test]
    async fn the_yearly_total_converts_foreign_currencies() {
        let p = pool().await;
        crate::fx::set(&p, "USD", 83_600_000, 0).await.unwrap();
        create(&p, &NewRecurrence::subscription("Netflix", 64_900, "2026-07-31")).await.unwrap();
        let mut usd = NewRecurrence::subscription("Some SaaS", 999, "2026-07-15");
        usd.currency = "USD".into();
        create(&p, &usd).await.unwrap();

        let total = yearly_total(&p, "INR").await.unwrap();
        let expected = 64_900 * 12 + money::convert(999, 83_600_000) * 12;
        assert_eq!(total, expected);
    }

    #[tokio::test]
    async fn a_cancelled_recurrence_leaves_the_yearly_total() {
        let p = pool().await;
        let id = create(&p, &NewRecurrence::subscription("Netflix", 64_900, "2026-07-31")).await.unwrap();
        assert_eq!(yearly_total(&p, "INR").await.unwrap(), 64_900 * 12);
        set_status(&p, id, Status::Cancelled).await.unwrap();
        assert_eq!(yearly_total(&p, "INR").await.unwrap(), 0);
    }
}
