//! Obligations: dated money that is owed but has not moved yet.
//!
//! The middle state between a template and a posting. An obligation is what the
//! "needs you" list on the Overview reads, what the sidebar badge counts, and
//! what the calendar plots. It carries an `estimate_minor` and an
//! `actual_minor` side by side, and the gap between them is what Insights
//! reports — an estimate that quietly became the recorded amount would destroy
//! that signal along with the user's trust in it.
//!
//! One-off bills live here with `recurrence_id NULL`. There is no separate
//! "one-time bill" table because a one-off bill is a bill that happens once.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use sqlx::SqlitePool;

use crate::date;
use crate::txn::{self, NewTxn, TxnKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Dated, not yet due.
    Upcoming,
    /// Due today.
    Due,
    /// Past its date and still unpaid.
    Overdue,
    Paid,
    /// Deliberately not paying this one. Keeps the row so the calendar and the
    /// history still explain the gap.
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Upcoming => "upcoming",
            Status::Due => "due",
            Status::Overdue => "overdue",
            Status::Paid => "paid",
            Status::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "due" => Status::Due,
            "overdue" => Status::Overdue,
            "paid" => Status::Paid,
            "skipped" => Status::Skipped,
            _ => Status::Upcoming,
        }
    }

    /// Does this still want the user to do something?
    pub fn is_open(self) -> bool {
        matches!(self, Status::Upcoming | Status::Due | Status::Overdue)
    }
}

#[derive(Clone, Debug)]
pub struct Obligation {
    pub id: i64,
    pub recurrence_id: Option<i64>,
    pub name: String,
    pub due_on: String,
    pub estimate_minor: Option<i64>,
    pub actual_minor: Option<i64>,
    pub status: Status,
    pub transaction_id: Option<i64>,
    /// Which kind of recurrence produced it, so the UI can label the row Bill or
    /// Subscription. `None` for a one-off.
    pub kind: Option<String>,
    pub currency: String,
    pub account_id: Option<i64>,
    /// Negative when overdue.
    pub days_until: i64,
    /// The category of the recurrence behind it, for the Bills table. `None` for a
    /// one-off or an uncategorised template.
    pub category_name: Option<String>,
    /// Which account it is paid from, by name.
    pub account_name: Option<String>,
    /// Posts itself on its due date, so it needs no action from anyone.
    pub auto_post: bool,
}

impl Obligation {
    /// What to show as the amount: the actual once paid, the estimate before.
    pub fn shown_minor(&self) -> Option<i64> {
        self.actual_minor.or(self.estimate_minor)
    }

    /// Is the shown figure a guess? Drives the `~` prefix in the UI.
    pub fn is_estimate(&self) -> bool {
        self.actual_minor.is_none()
    }

    /// Actual minus estimate, once both are known. The number Insights reports.
    pub fn variance_minor(&self) -> Option<i64> {
        Some(self.actual_minor? - self.estimate_minor?)
    }
}

fn row_to_obligation(r: Row, today: NaiveDate) -> Obligation {
    let due_on = r.3;
    let days_until = date::parse(&due_on).map(|d| date::days_between(today, d)).unwrap_or(0);
    Obligation {
        id: r.0,
        recurrence_id: r.1,
        name: r.2,
        due_on,
        estimate_minor: r.4,
        actual_minor: r.5,
        status: Status::parse(&r.6),
        transaction_id: r.7,
        kind: r.8,
        currency: r.9.unwrap_or_else(|| "INR".into()),
        account_id: r.10,
        days_until,
        category_name: r.11,
        account_name: r.12,
        auto_post: r.13.unwrap_or(0) != 0,
    }
}

/// The row shape every obligation query returns, and what `row_to_obligation`
/// takes. Named so the two cannot drift apart silently.
type Row = (
    i64,
    Option<i64>,
    String,
    String,
    Option<i64>,
    Option<i64>,
    String,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

const SELECT: &str = "SELECT o.id, o.recurrence_id, o.name, o.due_on, o.estimate_minor,
                             o.actual_minor, o.status, o.transaction_id, r.kind, r.currency,
                             r.account_id, c.name, a.name, r.auto_post
                        FROM obligations o
                        LEFT JOIN recurrences r ON r.id = o.recurrence_id
                        LEFT JOIN categories  c ON c.id = r.category_id
                        LEFT JOIN accounts    a ON a.id = r.account_id";

/// Everything still open, plus everything paid, inside a date window.
///
/// The window is inclusive on both ends and dated on `due_on`, which is the date
/// the user thinks in — not `created_at`.
pub async fn between(pool: &SqlitePool, from: &str, to: &str, today: NaiveDate) -> Result<Vec<Obligation>> {
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{SELECT} WHERE o.due_on BETWEEN ? AND ? ORDER BY o.due_on, o.name COLLATE NOCASE"
    ))
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| row_to_obligation(r, today)).collect())
}

/// What needs the user in the next `days`, plus anything already overdue.
///
/// Overdue items are included regardless of how far back they go: a bill missed
/// six weeks ago is more urgent than one due tomorrow, and dropping it off the
/// list because it fell outside the window is how it stays unpaid.
pub async fn needs_you(pool: &SqlitePool, today: NaiveDate, days: i64) -> Result<Vec<Obligation>> {
    let horizon = date::iso(today + chrono::Duration::days(days.max(0)));
    // One bound, not two: every overdue date is already below `today`, which is
    // at or below the horizon, so `<= horizon` catches the whole backlog.
    let rows = sqlx::query_as::<_, Row>(&format!(
        "{SELECT}
          WHERE o.status IN ('upcoming', 'due', 'overdue') AND o.due_on <= ?
          ORDER BY o.due_on, o.name COLLATE NOCASE"
    ))
    .bind(&horizon)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| row_to_obligation(r, today)).collect())
}

/// How many open items are due within the alert lead. Feeds the sidebar badge.
pub async fn badge_count(pool: &SqlitePool, today: NaiveDate, lead_days: i64) -> Result<i64> {
    let horizon = date::iso(today + chrono::Duration::days(lead_days.max(0)));
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM obligations
          WHERE status IN ('upcoming', 'due', 'overdue') AND due_on <= ?",
    )
    .bind(horizon)
    .fetch_one(pool)
    .await?)
}

/// Names of everything past its due date and still unpaid, soonest first.
///
/// Capped at five: a notification body listing thirty bills is a wall of text
/// nobody reads, and the section itself is the place to see all of them.
pub async fn overdue_names(pool: &SqlitePool, today: NaiveDate) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT name FROM obligations
          WHERE status IN ('upcoming', 'due', 'overdue') AND due_on < ?
          ORDER BY due_on LIMIT 5",
    )
    .bind(date::iso(today))
    .fetch_all(pool)
    .await?)
}

/// Move `upcoming` rows to `due` or `overdue` as the calendar catches up.
///
/// Returns how many changed, so a caller can skip a UI refresh when nothing did.
/// Deliberately does not touch `paid` or `skipped`: those are decisions, and the
/// passage of time does not undo a decision.
pub async fn sweep(pool: &SqlitePool, today: NaiveDate) -> Result<u64> {
    let today = date::iso(today);
    let overdue = sqlx::query(
        "UPDATE obligations SET status = 'overdue'
          WHERE status IN ('upcoming', 'due') AND due_on < ?",
    )
    .bind(&today)
    .execute(pool)
    .await?
    .rows_affected();
    let due = sqlx::query(
        "UPDATE obligations SET status = 'due' WHERE status = 'upcoming' AND due_on = ?",
    )
    .bind(&today)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(overdue + due)
}

/// A bill that happens once, with no template behind it.
pub async fn create_one_off(
    pool: &SqlitePool,
    name: &str,
    due_on: &str,
    estimate_minor: Option<i64>,
) -> Result<i64> {
    if name.trim().is_empty() {
        bail!("a bill needs a name");
    }
    date::parse(due_on).context("a bill needs a valid due date")?;
    Ok(sqlx::query(
        "INSERT INTO obligations (recurrence_id, name, due_on, estimate_minor, status)
         VALUES (NULL, ?, ?, ?, 'upcoming')",
    )
    .bind(name.trim())
    .bind(due_on)
    .bind(estimate_minor)
    .execute(pool)
    .await?
    .last_insert_rowid())
}

/// Record that an obligation was paid, posting exactly one transaction.
///
/// This is the moment an obligation becomes money. `actual_minor` is what was
/// really paid, which for a variable bill is not the estimate — and the estimate
/// is left in place rather than overwritten, because the difference between them
/// is a number Insights needs.
///
/// Paying an already-paid obligation is refused rather than posting a second
/// transaction; a double click on Mark paid should not cost the user twice.
pub async fn mark_paid(
    pool: &SqlitePool,
    id: i64,
    actual_minor: i64,
    on: NaiveDate,
    account_id: i64,
    currency: &str,
) -> Result<i64> {
    if actual_minor < 0 {
        bail!("a paid amount cannot be negative");
    }
    let existing: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT status, transaction_id FROM obligations WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let (status, already) = existing.context("no such obligation")?;
    if status == "paid" {
        if let Some(txn_id) = already {
            return Ok(txn_id);
        }
        bail!("this bill is marked paid but has no posting behind it");
    }

    let (name, recurrence_id): (String, Option<i64>) =
        sqlx::query_as("SELECT name, recurrence_id FROM obligations WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let category_id: Option<i64> = match recurrence_id {
        Some(r) => sqlx::query_scalar("SELECT category_id FROM recurrences WHERE id = ?")
            .bind(r)
            .fetch_optional(pool)
            .await?
            .flatten(),
        None => None,
    };
    let rate = crate::fx::rate_for(pool, currency, &crate::fx::base_currency()).await?;

    let mut t = NewTxn::expense(account_id, actual_minor, currency, &date::iso(on), &name);
    t.kind = TxnKind::Expense;
    t.rate_micro = rate;
    t.category_id = category_id;
    t.recurrence_id = recurrence_id;
    t.obligation_id = Some(id);
    t.source = "recurrence".to_string();
    let txn_id = txn::post(pool, &t).await?;

    sqlx::query(
        "UPDATE obligations SET status = 'paid', actual_minor = ?, transaction_id = ? WHERE id = ?",
    )
    .bind(actual_minor)
    .bind(txn_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(txn_id)
}

/// Close an obligation against a transaction that already exists.
///
/// The importer's counterpart to [`mark_paid`]: the money has demonstrably moved
/// — it is on the statement — so posting a second transaction for it would
/// double-count the payment. This only records that the two are the same event,
/// and inherits the recurrence's category onto the imported row, which is the
/// one thing a bank narration can never supply.
///
/// Refuses an obligation that is already paid, so a re-import cannot re-close
/// something and overwrite what was actually recorded the first time.
pub async fn attach(
    pool: &SqlitePool,
    id: i64,
    transaction_id: i64,
    actual_minor: i64,
    on: &str,
) -> Result<()> {
    let recurrence_id: Option<i64> = sqlx::query_scalar(
        "SELECT recurrence_id FROM obligations WHERE id = ? AND status <> 'paid'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .flatten();

    let affected = sqlx::query(
        "UPDATE obligations SET status = 'paid', actual_minor = ?, transaction_id = ?
          WHERE id = ? AND status <> 'paid'",
    )
    .bind(actual_minor)
    .bind(transaction_id)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if affected == 0 {
        bail!("that obligation is already closed");
    }

    let category_id: Option<i64> = match recurrence_id {
        Some(r) => sqlx::query_scalar("SELECT category_id FROM recurrences WHERE id = ?")
            .bind(r)
            .fetch_optional(pool)
            .await?
            .flatten(),
        None => None,
    };
    sqlx::query(
        "UPDATE transactions
            SET obligation_id = ?, recurrence_id = ?, occurred_on = ?,
                category_id = COALESCE(category_id, ?)
          WHERE id = ?",
    )
    .bind(id)
    .bind(recurrence_id)
    .bind(on)
    .bind(category_id)
    .bind(transaction_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Decide not to pay this instance. Posts nothing.
pub async fn skip(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE obligations SET status = 'skipped' WHERE id = ? AND status <> 'paid'")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Undo a mark-paid: deletes the posting and reopens the obligation.
pub async fn unpay(pool: &SqlitePool, id: i64) -> Result<()> {
    let txn_id: Option<i64> = sqlx::query_scalar("SELECT transaction_id FROM obligations WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .flatten();
    // `txn::delete` already reopens the obligation it was attached to, so this
    // only has to handle the case where the posting is already gone.
    match txn_id {
        Some(t) => txn::delete(pool, t).await?,
        None => {
            sqlx::query(
                "UPDATE obligations SET status = 'upcoming', actual_minor = NULL WHERE id = ?",
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE transactions SET obligation_id = NULL WHERE obligation_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM obligations WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Estimate-versus-actual across paid bills, for the Insights variance figure.
///
/// Only rows that had both, so a fixed subscription (where the two are equal by
/// construction) does not dilute the signal.
pub async fn variance_since(pool: &SqlitePool, from: &str) -> Result<Vec<(String, i64)>> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT o.name, o.actual_minor - o.estimate_minor
           FROM obligations o JOIN recurrences r ON r.id = o.recurrence_id
          WHERE o.status = 'paid' AND o.actual_minor IS NOT NULL AND o.estimate_minor IS NOT NULL
            AND r.amount_minor IS NULL AND o.due_on >= ?
          ORDER BY ABS(o.actual_minor - o.estimate_minor) DESC",
    )
    .bind(from)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts, recur, schema};

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
    async fn overdue_names_lists_only_the_unpaid_and_caps_the_list() {
        let p = pool().await;
        let acct = bank(&p).await;

        for (name, day) in [("Water", "01"), ("Rent", "02"), ("Gas", "03")] {
            create_one_off(&p, name, &format!("2026-07-{day}"), Some(100_000)).await.unwrap();
        }
        let paid = create_one_off(&p, "Society", "2026-07-04", Some(100_000)).await.unwrap();
        mark_paid(&p, paid, 100_000, d("2026-07-04"), acct, "INR").await.unwrap();
        // Not yet due, so not late.
        create_one_off(&p, "Insurance", "2026-08-20", Some(100_000)).await.unwrap();

        let names = overdue_names(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(names, ["Water", "Rent", "Gas"], "soonest first, paid and future excluded");

        for i in 0..6 {
            create_one_off(&p, &format!("Extra {i}"), "2026-07-05", Some(1)).await.unwrap();
        }
        assert_eq!(overdue_names(&p, d("2026-07-26")).await.unwrap().len(), 5, "capped");
    }

    #[tokio::test]
    async fn mark_paid_posts_exactly_one_transaction() {
        let p = pool().await;
        let acct = bank(&p).await;
        let obl = create_one_off(&p, "Society dues", "2026-07-15", Some(250_000)).await.unwrap();

        let txn_id = mark_paid(&p, obl, 250_000, d("2026-07-15"), acct, "INR").await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 1);

        // A second click must not cost the user twice.
        let again = mark_paid(&p, obl, 250_000, d("2026-07-15"), acct, "INR").await.unwrap();
        assert_eq!(again, txn_id);
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 1, "still one");
    }

    #[tokio::test]
    async fn paying_keeps_the_estimate_so_the_variance_survives() {
        // Honesty rule 3: the posted amount is the truth, and the gap between it
        // and the estimate is what Insights reports. Overwriting the estimate
        // would destroy that.
        let p = pool().await;
        let acct = bank(&p).await;
        let id = recur::create(&p, &recur::NewRecurrence::variable_bill("Electricity", "2026-07-12"))
            .await
            .unwrap();
        let obl = sqlx::query(
            "INSERT INTO obligations (recurrence_id, name, due_on, estimate_minor, status)
             VALUES (?, 'Electricity', '2026-07-12', 319000, 'upcoming')",
        )
        .bind(id)
        .execute(&p)
        .await
        .unwrap()
        .last_insert_rowid();

        mark_paid(&p, obl, 328_400, d("2026-07-12"), acct, "INR").await.unwrap();

        let got = between(&p, "2026-07-01", "2026-07-31", d("2026-07-26")).await.unwrap();
        let row = got.iter().find(|o| o.id == obl).unwrap();
        assert_eq!(row.estimate_minor, Some(319_000), "the guess is still on record");
        assert_eq!(row.actual_minor, Some(328_400));
        assert_eq!(row.variance_minor(), Some(9_400));
        assert!(!row.is_estimate(), "once paid, the shown figure is a fact");

        let variance = variance_since(&p, "2026-01-01").await.unwrap();
        assert_eq!(variance, [("Electricity".to_string(), 9_400)]);
    }

    #[tokio::test]
    async fn the_overdue_sweep_lands_on_the_right_side_of_the_boundary() {
        let p = pool().await;
        create_one_off(&p, "Yesterday", "2026-07-25", None).await.unwrap();
        create_one_off(&p, "Today", "2026-07-26", None).await.unwrap();
        create_one_off(&p, "Tomorrow", "2026-07-27", None).await.unwrap();

        let changed = sweep(&p, d("2026-07-26")).await.unwrap();
        assert_eq!(changed, 2, "yesterday became overdue, today became due");

        let statuses: Vec<(String, String)> =
            sqlx::query_as("SELECT name, status FROM obligations ORDER BY due_on")
                .fetch_all(&p)
                .await
                .unwrap();
        assert_eq!(
            statuses,
            [
                ("Yesterday".to_string(), "overdue".to_string()),
                ("Today".to_string(), "due".to_string()),
                ("Tomorrow".to_string(), "upcoming".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn the_sweep_does_not_undo_a_decision() {
        let p = pool().await;
        let acct = bank(&p).await;
        let paid = create_one_off(&p, "Paid", "2026-07-01", Some(1000)).await.unwrap();
        let skipped = create_one_off(&p, "Skipped", "2026-07-02", None).await.unwrap();
        mark_paid(&p, paid, 1000, d("2026-07-01"), acct, "INR").await.unwrap();
        skip(&p, skipped).await.unwrap();

        assert_eq!(sweep(&p, d("2026-07-26")).await.unwrap(), 0);
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM obligations ORDER BY due_on").fetch_all(&p).await.unwrap();
        assert_eq!(statuses, ["paid", "skipped"]);
    }

    #[tokio::test]
    async fn overdue_items_stay_on_the_needs_you_list_however_old() {
        let p = pool().await;
        create_one_off(&p, "Six weeks ago", "2026-06-14", None).await.unwrap();
        create_one_off(&p, "Tomorrow", "2026-07-27", None).await.unwrap();
        create_one_off(&p, "Next month", "2026-08-30", None).await.unwrap();
        sweep(&p, d("2026-07-26")).await.unwrap();

        let list = needs_you(&p, d("2026-07-26"), 14).await.unwrap();
        let names: Vec<&str> = list.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["Six weeks ago", "Tomorrow"]);
        assert_eq!(list[0].days_until, -42, "and it says how late it is");
    }

    #[tokio::test]
    async fn the_badge_counts_only_what_is_inside_the_lead() {
        let p = pool().await;
        create_one_off(&p, "Soon", "2026-07-28", None).await.unwrap();
        create_one_off(&p, "Later", "2026-08-20", None).await.unwrap();
        assert_eq!(badge_count(&p, d("2026-07-26"), 3).await.unwrap(), 1);
        assert_eq!(badge_count(&p, d("2026-07-26"), 30).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn skipping_posts_nothing() {
        let p = pool().await;
        let obl = create_one_off(&p, "Gym", "2026-07-15", Some(200_000)).await.unwrap();
        skip(&p, obl).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn unpaying_removes_the_posting_and_reopens_the_bill() {
        let p = pool().await;
        let acct = bank(&p).await;
        let obl = create_one_off(&p, "Power", "2026-07-12", Some(300_000)).await.unwrap();
        mark_paid(&p, obl, 328_400, d("2026-07-12"), acct, "INR").await.unwrap();
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000 - 328_400);

        unpay(&p, obl).await.unwrap();
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000);
        let (status, actual): (String, Option<i64>) =
            sqlx::query_as("SELECT status, actual_minor FROM obligations WHERE id = ?")
                .bind(obl)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(status, "upcoming");
        assert!(actual.is_none());
    }

    #[tokio::test]
    async fn a_foreign_bill_is_converted_at_the_rate_in_force() {
        let p = pool().await;
        let acct = bank(&p).await;
        crate::fx::set(&p, "USD", 83_600_000, 0).await.unwrap();
        let obl = create_one_off(&p, "Some SaaS", "2026-07-15", Some(999)).await.unwrap();
        let txn_id = mark_paid(&p, obl, 999, d("2026-07-15"), acct, "USD").await.unwrap();

        let (amount, base): (i64, i64) =
            sqlx::query_as("SELECT amount_minor, base_minor FROM transactions WHERE id = ?")
                .bind(txn_id)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(amount, 999);
        assert_eq!(base, crate::money::convert(999, 83_600_000));
    }

    #[tokio::test]
    async fn a_negative_payment_is_refused() {
        let p = pool().await;
        let acct = bank(&p).await;
        let obl = create_one_off(&p, "Power", "2026-07-12", None).await.unwrap();
        assert!(mark_paid(&p, obl, -1, d("2026-07-12"), acct, "INR").await.is_err());
    }

    #[tokio::test]
    async fn a_one_off_needs_a_name_and_a_real_date() {
        let p = pool().await;
        assert!(create_one_off(&p, "  ", "2026-07-12", None).await.is_err());
        assert!(create_one_off(&p, "Power", "not-a-date", None).await.is_err());
        assert!(create_one_off(&p, "Power", "2026-02-30", None).await.is_err());
    }
}
