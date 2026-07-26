//! Envelopes: what a category is allowed to cost this month.
//!
//! Budgets are set per `(category, YYYY-MM)`. That is more rows than a single
//! recurring figure would need, but it is what makes last month's budget stay
//! what it actually was — a single editable number would silently rewrite the
//! history the six-month discipline table is drawn from.
//!
//! Spending here is expense-only, so a credit-card payment or a wallet top-up
//! cannot eat an envelope. That is honesty rule 1 again, and it holds because
//! every figure comes through [`crate::txn::spent_between`]-shaped SQL rather
//! than a fresh `SUM` that forgot the predicate.

use anyhow::{bail, Result};
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;

use crate::date;

#[derive(Clone, Debug)]
pub struct BudgetRow {
    pub id: Option<i64>,
    pub category_id: i64,
    pub category_name: String,
    pub color: Option<String>,
    pub amount_minor: i64,
    /// Carried in from last month's surplus, when `rollover` is on.
    pub rollover_minor: i64,
    pub rollover: bool,
    pub spent_minor: i64,
}

impl BudgetRow {
    /// The envelope including anything carried in.
    pub fn allowance_minor(&self) -> i64 {
        self.amount_minor + self.rollover_minor
    }

    pub fn remaining_minor(&self) -> i64 {
        self.allowance_minor() - self.spent_minor
    }

    /// Fraction of the envelope used, 0 upward. Can exceed 100.
    pub fn used_pct(&self) -> i64 {
        let allowance = self.allowance_minor();
        if allowance <= 0 {
            // No envelope but money spent is 100% over, not a division by zero.
            return if self.spent_minor > 0 { 100 } else { 0 };
        }
        ((self.spent_minor as i128 * 100) / allowance as i128) as i64
    }

    pub fn over_budget(&self) -> bool {
        self.spent_minor > self.allowance_minor()
    }

    /// Where spending will land by month end if it carries on at this rate.
    ///
    /// `elapsed`/`total` are days. Straight-line, which is wrong for a month with
    /// one big rent payment on the 1st and right for the discretionary categories
    /// budgets are actually set on — and the design only puts envelopes on those.
    pub fn projected_minor(&self, elapsed: i64, total: i64) -> i64 {
        if elapsed <= 0 || total <= 0 {
            return self.spent_minor;
        }
        let elapsed = elapsed.min(total);
        ((self.spent_minor as i128 * total as i128) / elapsed as i128) as i64
    }

    /// Is spending ahead of an even pace through the month?
    pub fn off_pace(&self, elapsed: i64, total: i64) -> bool {
        self.projected_minor(elapsed, total) > self.allowance_minor()
    }
}

/// Days elapsed and days total for `period`, as at `today`.
///
/// A past month is fully elapsed and a future one not at all, so a projection
/// asked for either does not divide the month it is not in.
pub fn month_progress(period: &str, today: NaiveDate) -> (i64, i64) {
    let Ok((first, _)) = date::month_bounds(period) else { return (0, 0) };
    let Ok(first) = date::parse(&first) else { return (0, 0) };
    let total = date::days_in_month(first.year(), first.month()) as i64;
    let elapsed = if date::ym(today) == period {
        today.day() as i64
    } else if today > first {
        total
    } else {
        0
    };
    (elapsed, total)
}

/// Every envelope for a period, plus every category that has spending but no
/// envelope — those are the ones worth setting one on, and hiding them would
/// hide the reason the budget page exists.
pub async fn list(pool: &SqlitePool, period: &str) -> Result<Vec<BudgetRow>> {
    let (from, to) = date::month_bounds(period)?;

    let rows = sqlx::query_as::<_, (Option<i64>, i64, String, Option<String>, i64, i64, i64)>(
        "SELECT b.id, c.id, c.name, c.color,
                COALESCE(b.amount_minor, 0), COALESCE(b.rollover, 0),
                COALESCE((SELECT SUM(t.base_minor) FROM transactions t
                           WHERE t.category_id = c.id AND t.kind = 'expense'
                             AND t.occurred_on BETWEEN ? AND ?), 0)
           FROM categories c
           LEFT JOIN budgets b ON b.category_id = c.id AND b.period = ?
          WHERE c.kind = 'expense'
            AND (b.id IS NOT NULL OR EXISTS (
                  SELECT 1 FROM transactions t
                   WHERE t.category_id = c.id AND t.kind = 'expense'
                     AND t.occurred_on BETWEEN ? AND ?))
          ORDER BY b.id IS NULL, c.name COLLATE NOCASE",
    )
    .bind(&from)
    .bind(&to)
    .bind(period)
    .bind(&from)
    .bind(&to)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let rollover = r.5 != 0;
        let rollover_minor = if rollover { carried_in(pool, r.1, period).await? } else { 0 };
        out.push(BudgetRow {
            id: r.0,
            category_id: r.1,
            category_name: r.2,
            color: r.3,
            amount_minor: r.4,
            rollover_minor,
            rollover,
            spent_minor: r.6,
        });
    }
    Ok(out)
}

/// Last month's unspent remainder, when rollover is on.
///
/// Only ever positive: carrying an overspend forward turns one bad month into a
/// permanently broken envelope, which is punishment rather than budgeting.
async fn carried_in(pool: &SqlitePool, category_id: i64, period: &str) -> Result<i64> {
    let Ok((first, _)) = date::month_bounds(period) else { return Ok(0) };
    let Ok(first) = date::parse(&first) else { return Ok(0) };
    let prev = first.pred_opt().unwrap_or(first);
    let prev_period = date::ym(prev);
    let (pfrom, pto) = date::month_bounds(&prev_period)?;

    let budgeted: Option<i64> =
        sqlx::query_scalar("SELECT amount_minor FROM budgets WHERE category_id = ? AND period = ?")
            .bind(category_id)
            .bind(&prev_period)
            .fetch_optional(pool)
            .await?;
    let Some(budgeted) = budgeted else { return Ok(0) };

    let spent: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(base_minor), 0) FROM transactions
          WHERE category_id = ? AND kind = 'expense' AND occurred_on BETWEEN ? AND ?",
    )
    .bind(category_id)
    .bind(&pfrom)
    .bind(&pto)
    .fetch_one(pool)
    .await?;

    Ok((budgeted - spent).max(0))
}

pub async fn set(
    pool: &SqlitePool,
    category_id: i64,
    period: &str,
    amount_minor: i64,
    rollover: bool,
) -> Result<()> {
    if amount_minor < 0 {
        bail!("a budget cannot be negative");
    }
    date::month_bounds(period)?;
    sqlx::query(
        "INSERT INTO budgets (category_id, period, amount_minor, rollover) VALUES (?, ?, ?, ?)
         ON CONFLICT(category_id, period) DO UPDATE SET
            amount_minor = excluded.amount_minor, rollover = excluded.rollover",
    )
    .bind(category_id)
    .bind(period)
    .bind(amount_minor)
    .bind(i64::from(rollover))
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, category_id: i64, period: &str) -> Result<()> {
    sqlx::query("DELETE FROM budgets WHERE category_id = ? AND period = ?")
        .bind(category_id)
        .bind(period)
        .execute(pool)
        .await?;
    Ok(())
}

/// Copy every envelope from `from_period` into `to_period`, skipping ones already
/// set — so the user does not re-enter the same twelve numbers every month.
pub async fn copy_forward(pool: &SqlitePool, from_period: &str, to_period: &str) -> Result<u64> {
    Ok(sqlx::query(
        "INSERT OR IGNORE INTO budgets (category_id, period, amount_minor, rollover)
         SELECT category_id, ?, amount_minor, rollover FROM budgets WHERE period = ?",
    )
    .bind(to_period)
    .bind(from_period)
    .execute(pool)
    .await?
    .rows_affected())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonthDiscipline {
    pub period: String,
    pub budget_minor: i64,
    pub spent_minor: i64,
}

impl MonthDiscipline {
    pub fn within(&self) -> bool {
        self.budget_minor > 0 && self.spent_minor <= self.budget_minor
    }
}

/// Budget versus actual for the last `months` periods, oldest first.
///
/// Only counts spending in categories that had an envelope that month — the
/// comparison is meaningless otherwise, because unbudgeted categories would make
/// every month look like an overspend.
pub async fn discipline(pool: &SqlitePool, months: u32, today: NaiveDate) -> Result<Vec<MonthDiscipline>> {
    let mut out = Vec::new();
    let mut cursor = today;
    for _ in 0..months {
        let period = date::ym(cursor);
        let (from, to) = date::month_bounds(&period)?;
        let budget: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(amount_minor), 0) FROM budgets WHERE period = ?")
                .bind(&period)
                .fetch_one(pool)
                .await?;
        let spent: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(t.base_minor), 0) FROM transactions t
              WHERE t.kind = 'expense' AND t.occurred_on BETWEEN ? AND ?
                AND t.category_id IN (SELECT category_id FROM budgets WHERE period = ?)",
        )
        .bind(&from)
        .bind(&to)
        .bind(&period)
        .fetch_one(pool)
        .await?;
        out.push(MonthDiscipline { period, budget_minor: budget, spent_minor: spent });

        let first = date::parse(&from)?;
        cursor = first.pred_opt().unwrap_or(first);
    }
    out.reverse();
    Ok(out)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allocation {
    pub income_minor: i64,
    pub budgeted_minor: i64,
    pub spent_minor: i64,
}

impl Allocation {
    /// Income not accounted for by any envelope. Negative means over-allocated.
    pub fn unallocated_minor(&self) -> i64 {
        self.income_minor - self.budgeted_minor
    }

    /// Income minus what actually went out, as a percentage of income.
    ///
    /// `None` with no income, because a savings rate out of nothing is a division
    /// by zero dressed up as a statistic.
    pub fn savings_rate_pct(&self) -> Option<i64> {
        if self.income_minor <= 0 {
            return None;
        }
        Some((((self.income_minor - self.spent_minor) as i128 * 100) / self.income_minor as i128) as i64)
    }
}

/// The income-allocation bar: what came in, what is committed, what has gone.
pub async fn allocation(pool: &SqlitePool, period: &str) -> Result<Allocation> {
    let (from, to) = date::month_bounds(period)?;
    Ok(Allocation {
        income_minor: crate::txn::income_between(pool, &from, &to).await?,
        budgeted_minor: sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_minor), 0) FROM budgets WHERE period = ?",
        )
        .bind(period)
        .fetch_one(pool)
        .await?,
        spent_minor: crate::txn::spent_between(pool, &from, &to).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::txn::{self, NewTxn, TxnKind};
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

    async fn cat(p: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM categories WHERE name = ?")
            .bind(name)
            .fetch_one(p)
            .await
            .unwrap()
    }

    async fn spend(p: &SqlitePool, acct: i64, category: i64, on: &str, amount: i64) {
        let mut t = NewTxn::expense(acct, amount, "INR", on, &format!("Buy {on} {amount}"));
        t.category_id = Some(category);
        txn::post(p, &t).await.unwrap();
    }

    #[tokio::test]
    async fn spent_against_an_envelope_excludes_transfers() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 10_000_000)).await.unwrap();
        let other = accounts::create(&p, &accounts::NewAccount::bank("Paytm", 0)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        let transfer = cat(&p, "Transfer").await;
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        set(&p, transfer, "2026-07", 100_000, false).await.unwrap();

        spend(&p, acct, groceries, "2026-07-05", 300_000).await;
        // A big top-up categorised as a transfer must not eat any envelope.
        let mut t = NewTxn::transfer(acct, other, 5_000_000, "INR", "2026-07-06", "Top-up");
        t.category_id = Some(transfer);
        txn::post(&p, &t).await.unwrap();

        let rows = list(&p, "2026-07").await.unwrap();
        let g = rows.iter().find(|r| r.category_id == groceries).unwrap();
        assert_eq!(g.spent_minor, 300_000);
        // The transfer category is not an expense category, so it is not listed.
        assert!(rows.iter().all(|r| r.category_id != transfer));
    }

    #[tokio::test]
    async fn remaining_and_over_budget() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 10_000_000)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        spend(&p, acct, groceries, "2026-07-05", 300_000).await;

        let row = list(&p, "2026-07").await.unwrap().into_iter().next().unwrap();
        assert_eq!(row.remaining_minor(), 500_000);
        assert_eq!(row.used_pct(), 37);
        assert!(!row.over_budget());

        spend(&p, acct, groceries, "2026-07-20", 600_000).await;
        let row = list(&p, "2026-07").await.unwrap().into_iter().next().unwrap();
        assert_eq!(row.remaining_minor(), -100_000);
        assert!(row.over_budget());
        assert_eq!(row.used_pct(), 112);
    }

    #[test]
    fn pace_projection_is_straight_line() {
        let row = BudgetRow {
            id: Some(1),
            category_id: 1,
            category_name: "Groceries".into(),
            color: None,
            amount_minor: 800_000,
            rollover_minor: 0,
            rollover: false,
            spent_minor: 300_000,
        };
        // ₹3,000 spent by the 10th of a 31-day month projects to ₹9,300.
        assert_eq!(row.projected_minor(10, 31), 930_000);
        assert!(row.off_pace(10, 31), "that lands well past the ₹8,000 envelope");
        // The same spend by the 25th is on track.
        assert_eq!(row.projected_minor(25, 31), 372_000);
        assert!(!row.off_pace(25, 31));
        // Day zero cannot divide.
        assert_eq!(row.projected_minor(0, 31), 300_000);
    }

    #[test]
    fn an_envelope_of_zero_with_spending_reads_as_fully_over() {
        let row = BudgetRow {
            id: None,
            category_id: 1,
            category_name: "Shopping".into(),
            color: None,
            amount_minor: 0,
            rollover_minor: 0,
            rollover: false,
            spent_minor: 100_000,
        };
        assert_eq!(row.used_pct(), 100);
        assert!(row.over_budget());
    }

    #[test]
    fn month_progress_handles_past_present_and_future() {
        assert_eq!(month_progress("2026-07", d("2026-07-26")), (26, 31));
        assert_eq!(month_progress("2026-06", d("2026-07-26")), (30, 30), "a past month is done");
        assert_eq!(month_progress("2026-09", d("2026-07-26")), (0, 30), "a future month has not begun");
        assert_eq!(month_progress("2028-02", d("2028-02-29")), (29, 29));
    }

    #[tokio::test]
    async fn categories_with_spending_but_no_envelope_are_still_listed() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 10_000_000)).await.unwrap();
        let shopping = cat(&p, "Shopping").await;
        spend(&p, acct, shopping, "2026-07-08", 450_000).await;

        let rows = list(&p, "2026-07").await.unwrap();
        let s = rows.iter().find(|r| r.category_id == shopping).unwrap();
        assert!(s.id.is_none(), "no envelope set");
        assert_eq!(s.spent_minor, 450_000, "but the spending is visible, which is the point");
    }

    #[tokio::test]
    async fn a_category_with_neither_envelope_nor_spending_is_not_noise() {
        let p = pool().await;
        assert!(list(&p, "2026-07").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rollover_carries_a_surplus_but_never_an_overspend() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 100_000_000)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        set(&p, groceries, "2026-06", 800_000, true).await.unwrap();
        set(&p, groceries, "2026-07", 800_000, true).await.unwrap();

        // June came in ₹2,000 under.
        spend(&p, acct, groceries, "2026-06-10", 600_000).await;
        let july = list(&p, "2026-07").await.unwrap().into_iter().next().unwrap();
        assert_eq!(july.rollover_minor, 200_000);
        assert_eq!(july.allowance_minor(), 1_000_000);

        // Now blow through June instead.
        spend(&p, acct, groceries, "2026-06-20", 900_000).await;
        let july = list(&p, "2026-07").await.unwrap().into_iter().next().unwrap();
        assert_eq!(july.rollover_minor, 0, "one bad month must not break the envelope forever");
    }

    #[tokio::test]
    async fn rollover_off_means_no_carry() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 10_000_000)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        set(&p, groceries, "2026-06", 800_000, false).await.unwrap();
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        spend(&p, acct, groceries, "2026-06-10", 100_000).await;
        let july = list(&p, "2026-07").await.unwrap().into_iter().next().unwrap();
        assert_eq!(july.rollover_minor, 0);
    }

    #[tokio::test]
    async fn setting_an_envelope_twice_updates_it() {
        let p = pool().await;
        let groceries = cat(&p, "Groceries").await;
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        set(&p, groceries, "2026-07", 900_000, true).await.unwrap();
        let rows = list(&p, "2026-07").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount_minor, 900_000);
        assert!(rows[0].rollover);
    }

    #[tokio::test]
    async fn a_negative_envelope_or_bad_period_is_refused() {
        let p = pool().await;
        let groceries = cat(&p, "Groceries").await;
        assert!(set(&p, groceries, "2026-07", -1, false).await.is_err());
        assert!(set(&p, groceries, "July", 100, false).await.is_err());
    }

    #[tokio::test]
    async fn copying_forward_does_not_overwrite_what_is_already_set() {
        let p = pool().await;
        let groceries = cat(&p, "Groceries").await;
        let travel = cat(&p, "Travel").await;
        set(&p, groceries, "2026-06", 800_000, false).await.unwrap();
        set(&p, travel, "2026-06", 500_000, false).await.unwrap();
        set(&p, travel, "2026-07", 100_000, false).await.unwrap();

        let copied = copy_forward(&p, "2026-06", "2026-07").await.unwrap();
        assert_eq!(copied, 1, "only Groceries was missing");
        let rows = list(&p, "2026-07").await.unwrap();
        let t = rows.iter().find(|r| r.category_id == travel).unwrap();
        assert_eq!(t.amount_minor, 100_000, "July's own figure survived");
    }

    #[tokio::test]
    async fn discipline_only_counts_budgeted_categories() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 100_000_000)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        let rent = cat(&p, "Rent").await;
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();
        spend(&p, acct, groceries, "2026-07-05", 700_000).await;
        // Rent has no envelope; a ₹38,000 payment must not make July an overspend.
        spend(&p, acct, rent, "2026-07-05", 3_800_000).await;

        let months = discipline(&p, 2, d("2026-07-26")).await.unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(months[0].period, "2026-06");
        assert_eq!(months[1].period, "2026-07");
        assert_eq!(months[1].budget_minor, 800_000);
        assert_eq!(months[1].spent_minor, 700_000);
        assert!(months[1].within());
        // A month with no envelopes at all is not "within" — there was nothing to
        // be within.
        assert!(!months[0].within());
    }

    #[tokio::test]
    async fn allocation_and_savings_rate() {
        let p = pool().await;
        let acct = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 0)).await.unwrap();
        let groceries = cat(&p, "Groceries").await;
        set(&p, groceries, "2026-07", 800_000, false).await.unwrap();

        let mut salary = NewTxn::expense(acct, 10_000_000, "INR", "2026-07-01", "Salary");
        salary.kind = TxnKind::Income;
        txn::post(&p, &salary).await.unwrap();
        spend(&p, acct, groceries, "2026-07-05", 2_500_000).await;

        let a = allocation(&p, "2026-07").await.unwrap();
        assert_eq!(a.income_minor, 10_000_000);
        assert_eq!(a.budgeted_minor, 800_000);
        assert_eq!(a.spent_minor, 2_500_000);
        assert_eq!(a.unallocated_minor(), 9_200_000);
        assert_eq!(a.savings_rate_pct(), Some(75));
    }

    #[test]
    fn a_savings_rate_out_of_no_income_is_not_a_number() {
        let a = Allocation { income_minor: 0, budgeted_minor: 0, spent_minor: 500_000 };
        assert_eq!(a.savings_rate_pct(), None);
    }
}
