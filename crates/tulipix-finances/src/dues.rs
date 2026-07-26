//! Money between you and named people.
//!
//! People are free text. There is no contact record, no profile and no member
//! table, because the design is explicit that this section will not chase anyone:
//! no messaging, no share links, no reminders to send. The app has no way to
//! contact a person and will not pretend otherwise. It remembers; that is all.
//!
//! **The honesty rule this module enforces:** lending is not spending. Handing
//! ₹5,000 to a friend moves cash out today, but it is not expenditure — the money
//! is still owed to you. So a due posts as a **transfer** into the virtual
//! `Lent out` holding account, never as an expense, and comes back when it
//! settles. Without this, every loan to a friend would show up as a month with
//! unusually high spending. See [`create`] and [`settle`].

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use sqlx::SqlitePool;

use crate::accounts;
use crate::date;
use crate::schema::{BORROWED, LENT_OUT};
use crate::txn::{self, NewTxn};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// They have your money.
    OwedToMe,
    /// You have theirs.
    IOwe,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::OwedToMe => "owed_to_me",
            Direction::IOwe => "i_owe",
        }
    }

    pub fn parse(s: &str) -> Direction {
        if s == "i_owe" { Direction::IOwe } else { Direction::OwedToMe }
    }

    /// The holding account this direction parks money in.
    fn holding(self) -> &'static str {
        match self {
            Direction::OwedToMe => LENT_OUT,
            Direction::IOwe => BORROWED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    Settled,
    /// Given up on. Only here does a due become a real expense.
    WrittenOff,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Settled => "settled",
            Status::WrittenOff => "written_off",
        }
    }

    pub fn parse(s: &str) -> Status {
        match s {
            "settled" => Status::Settled,
            "written_off" => Status::WrittenOff,
            _ => Status::Open,
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewDue {
    pub person: String,
    pub direction: Direction,
    pub amount_minor: i64,
    pub currency: String,
    pub opened_on: String,
    pub note: Option<String>,
    /// Which real account the cash left from, or arrived in. `None` records the
    /// due without moving any money — for a debt that predates the app.
    pub account_id: Option<i64>,
}

impl NewDue {
    pub fn lent(person: &str, amount_minor: i64, on: &str, account_id: i64) -> Self {
        NewDue {
            person: person.to_string(),
            direction: Direction::OwedToMe,
            amount_minor,
            currency: "INR".to_string(),
            opened_on: on.to_string(),
            note: None,
            account_id: Some(account_id),
        }
    }

    pub fn borrowed(person: &str, amount_minor: i64, on: &str, account_id: i64) -> Self {
        NewDue { direction: Direction::IOwe, ..NewDue::lent(person, amount_minor, on, account_id) }
    }
}

#[derive(Clone, Debug)]
pub struct Due {
    pub id: i64,
    pub person: String,
    pub direction: Direction,
    /// What is still outstanding. A part-settlement reduces this.
    pub amount_minor: i64,
    pub currency: String,
    pub opened_on: String,
    pub note: Option<String>,
    pub status: Status,
    pub settled_on: Option<String>,
    /// Sum already settled, across however many part-payments.
    pub settled_minor: i64,
    /// Days since it was opened.
    pub age_days: i64,
}

impl Due {
    /// The amount originally agreed.
    pub fn original_minor(&self) -> i64 {
        self.amount_minor + self.settled_minor
    }
}

pub async fn list(pool: &SqlitePool, status: Option<Status>) -> Result<Vec<Due>> {
    let today = date::today();
    let rows = sqlx::query_as::<
        _,
        (i64, String, String, i64, String, String, Option<String>, String, Option<String>),
    >(
        "SELECT d.id, d.person, d.direction, d.amount_minor, d.currency, d.opened_on, d.note,
                d.status, d.settled_on
           FROM dues d
          WHERE (? IS NULL OR d.status = ?)
          ORDER BY d.status, d.opened_on DESC, d.person COLLATE NOCASE",
    )
    .bind(status.map(|s| s.as_str()))
    .bind(status.map(|s| s.as_str()))
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let settled_minor = settled_total(pool, r.0).await?;
        let age_days = date::parse(&r.5).map(|d| date::days_between(d, today)).unwrap_or(0);
        out.push(Due {
            id: r.0,
            person: r.1,
            direction: Direction::parse(&r.2),
            amount_minor: r.3,
            currency: r.4,
            opened_on: r.5,
            note: r.6,
            status: Status::parse(&r.7),
            settled_on: r.8,
            settled_minor,
            age_days,
        });
    }
    Ok(out)
}

/// Marks a posting as a repayment rather than the opening of a due. Both are
/// transfers against the same holding account, so the note is what tells them
/// apart when totting up how much has come back.
const SETTLEMENT_NOTE: &str = "settlement";

/// How much of a due has been paid back so far.
///
/// Derived from the settlement postings rather than stored, so deleting a
/// settlement is reflected instead of leaving a due that claims to be half-paid
/// with nothing behind it.
async fn settled_total(pool: &SqlitePool, due_id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_minor), 0) FROM transactions WHERE due_id = ? AND note = ?",
    )
    .bind(due_id)
    .bind(SETTLEMENT_NOTE)
    .fetch_one(pool)
    .await?)
}

/// Open a due, moving the cash if a real account was named.
///
/// The posting is a **transfer** to the holding account, not an expense. This is
/// the whole of honesty rule 2, and [`lending_is_not_spending`] is the test that
/// holds it.
pub async fn create(pool: &SqlitePool, d: &NewDue) -> Result<i64> {
    if d.person.trim().is_empty() {
        bail!("a due needs a person's name");
    }
    if d.amount_minor <= 0 {
        bail!("a due needs a positive amount");
    }
    date::parse(&d.opened_on).context("a due needs a valid date")?;

    let id = sqlx::query(
        "INSERT INTO dues (person, direction, amount_minor, currency, opened_on, note, status)
         VALUES (?, ?, ?, ?, ?, ?, 'open')",
    )
    .bind(d.person.trim())
    .bind(d.direction.as_str())
    .bind(d.amount_minor)
    .bind(d.currency.to_uppercase())
    .bind(&d.opened_on)
    .bind(&d.note)
    .execute(pool)
    .await?
    .last_insert_rowid();

    if let Some(account_id) = d.account_id {
        let holding = accounts::virtual_id(pool, d.direction.holding()).await?;
        let (from, to, what) = match d.direction {
            // Cash leaves the account and parks in "Lent out".
            Direction::OwedToMe => (account_id, holding, "Lent to"),
            // Cash arrives from "Borrowed" into the account.
            Direction::IOwe => (holding, account_id, "Borrowed from"),
        };
        let mut t = NewTxn::transfer(
            from,
            to,
            d.amount_minor,
            &d.currency,
            &d.opened_on,
            &format!("{what} {}", d.person.trim()),
        );
        t.due_id = Some(id);
        t.rate_micro = crate::fx::rate_for(pool, &d.currency, &crate::fx::base_currency()).await?;
        txn::post(pool, &t).await?;
    }
    Ok(id)
}

/// Settle all or part of a due.
///
/// A partial payment reduces the outstanding amount and leaves the due open; only
/// a payment covering the remainder closes it. The posting is again a transfer,
/// in the opposite direction to the one [`create`] made, so the round trip nets
/// to zero across the ledger and never reaches a spend total.
///
/// Returns the posted transaction id.
pub async fn settle(
    pool: &SqlitePool,
    id: i64,
    amount_minor: i64,
    on: NaiveDate,
    account_id: i64,
) -> Result<i64> {
    if amount_minor <= 0 {
        bail!("a settlement needs a positive amount");
    }
    let (person, direction, outstanding, currency, status): (String, String, i64, String, String) =
        sqlx::query_as("SELECT person, direction, amount_minor, currency, status FROM dues WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .context("no such due")?;
    if status != "open" {
        bail!("this due is already {status}");
    }
    if amount_minor > outstanding {
        bail!("that is more than the {outstanding} still outstanding");
    }
    let direction = Direction::parse(&direction);
    let holding = accounts::virtual_id(pool, direction.holding()).await?;

    let (from, to, what) = match direction {
        // They pay you back: money comes out of the holding bucket into the bank.
        Direction::OwedToMe => (holding, account_id, "Repaid by"),
        // You pay them back: money leaves the bank and clears the bucket.
        Direction::IOwe => (account_id, holding, "Repaid to"),
    };
    let mut t = NewTxn::transfer(
        from,
        to,
        amount_minor,
        &currency,
        &date::iso(on),
        &format!("{what} {person}"),
    );
    t.due_id = Some(id);
    t.rate_micro = crate::fx::rate_for(pool, &currency, &crate::fx::base_currency()).await?;
    // Tagged so `settled_total` can tell a settlement from the opening posting.
    t.note = Some(SETTLEMENT_NOTE.to_string());
    let txn_id = txn::post(pool, &t).await?;

    let left = outstanding - amount_minor;
    if left == 0 {
        sqlx::query(
            "UPDATE dues SET amount_minor = 0, status = 'settled', settled_on = ?, settle_txn_id = ?
              WHERE id = ?",
        )
        .bind(date::iso(on))
        .bind(txn_id)
        .bind(id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("UPDATE dues SET amount_minor = ? WHERE id = ?")
            .bind(left)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(txn_id)
}

/// Give up on a due.
///
/// This is the one place a due becomes real spending: money lent and never coming
/// back has, at that point, actually been spent. Clears the holding bucket with a
/// matching expense so `Lent out` does not keep claiming money that is gone.
pub async fn write_off(pool: &SqlitePool, id: i64, on: NaiveDate) -> Result<i64> {
    let (person, direction, outstanding, currency, status): (String, String, i64, String, String) =
        sqlx::query_as("SELECT person, direction, amount_minor, currency, status FROM dues WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .context("no such due")?;
    if status != "open" {
        bail!("this due is already {status}");
    }
    let direction = Direction::parse(&direction);
    let holding = accounts::virtual_id(pool, direction.holding()).await?;

    // Written off in your favour is income; written off against you is expense.
    let kind = match direction {
        Direction::OwedToMe => txn::TxnKind::Expense,
        Direction::IOwe => txn::TxnKind::Income,
    };
    let mut t = NewTxn::expense(
        holding,
        outstanding,
        &currency,
        &date::iso(on),
        &format!("Written off — {person}"),
    );
    t.kind = kind;
    t.due_id = Some(id);
    t.rate_micro = crate::fx::rate_for(pool, &currency, &crate::fx::base_currency()).await?;
    let txn_id = txn::post(pool, &t).await?;

    sqlx::query(
        "UPDATE dues SET amount_minor = 0, status = 'written_off', settled_on = ?, settle_txn_id = ?
          WHERE id = ?",
    )
    .bind(date::iso(on))
    .bind(txn_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(txn_id)
}

pub async fn update_note(pool: &SqlitePool, id: i64, note: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE dues SET note = ? WHERE id = ?").bind(note).bind(id).execute(pool).await?;
    Ok(())
}

/// Remove a due and every posting it made.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    let txns: Vec<i64> = sqlx::query_scalar("SELECT id FROM transactions WHERE due_id = ?")
        .bind(id)
        .fetch_all(pool)
        .await?;
    for t in txns {
        txn::delete(pool, t).await?;
    }
    sqlx::query("DELETE FROM dues WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub owed_to_me_minor: i64,
    pub i_owe_minor: i64,
}

/// Outstanding in both directions. Open dues only.
pub async fn totals(pool: &SqlitePool) -> Result<Totals> {
    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT direction, COALESCE(SUM(amount_minor), 0) FROM dues
          WHERE status = 'open' GROUP BY direction",
    )
    .fetch_all(pool)
    .await?;
    let mut t = Totals::default();
    for (dir, sum) in rows {
        match Direction::parse(&dir) {
            Direction::OwedToMe => t.owed_to_me_minor = sum,
            Direction::IOwe => t.i_owe_minor = sum,
        }
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;
    use crate::txn::{TxnFilter, TxnSort};

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
    async fn lending_is_not_spending() {
        // Honesty rule 2, with a test of its own as the spec requires.
        let p = pool().await;
        let acct = bank(&p).await;
        create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();

        assert_eq!(
            txn::spent_between(&p, "2026-07-01", "2026-07-31").await.unwrap(),
            0,
            "₹5,000 lent is not ₹5,000 spent"
        );
        // The cash really has left the bank, though.
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 9_500_000);
        // And it is sitting where it can be found.
        let lent = accounts::virtual_id(&p, LENT_OUT).await.unwrap();
        assert_eq!(accounts::balance(&p, lent).await.unwrap(), 500_000);

        // The posting is a transfer, not an expense.
        let kind: String = sqlx::query_scalar("SELECT kind FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(kind, "transfer");
    }

    #[tokio::test]
    async fn settling_posts_a_transfer_not_an_income() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        settle(&p, id, 500_000, d("2026-08-05"), acct).await.unwrap();

        let kinds: Vec<String> =
            sqlx::query_scalar("SELECT kind FROM transactions ORDER BY id").fetch_all(&p).await.unwrap();
        assert_eq!(kinds, ["transfer", "transfer"], "money coming back is not new income");

        // The round trip nets to zero: the bank is whole and the bucket is empty.
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000);
        let lent = accounts::virtual_id(&p, LENT_OUT).await.unwrap();
        assert_eq!(accounts::balance(&p, lent).await.unwrap(), 0);

        assert_eq!(txn::income_between(&p, "2026-01-01", "2026-12-31").await.unwrap(), 0);
        assert_eq!(txn::spent_between(&p, "2026-01-01", "2026-12-31").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_part_settlement_leaves_the_remainder_open() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();

        settle(&p, id, 200_000, d("2026-08-05"), acct).await.unwrap();
        let due = list(&p, None).await.unwrap().into_iter().find(|x| x.id == id).unwrap();
        assert_eq!(due.status, Status::Open);
        assert_eq!(due.amount_minor, 300_000, "₹3,000 still outstanding");
        assert_eq!(due.settled_minor, 200_000);
        assert_eq!(due.original_minor(), 500_000);

        settle(&p, id, 300_000, d("2026-09-05"), acct).await.unwrap();
        let due = list(&p, None).await.unwrap().into_iter().find(|x| x.id == id).unwrap();
        assert_eq!(due.status, Status::Settled);
        assert_eq!(due.amount_minor, 0);
        assert_eq!(due.settled_on.as_deref(), Some("2026-09-05"));
    }

    #[tokio::test]
    async fn over_settling_is_refused() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        assert!(settle(&p, id, 600_000, d("2026-08-05"), acct).await.is_err());
        assert!(settle(&p, id, 0, d("2026-08-05"), acct).await.is_err());
    }

    #[tokio::test]
    async fn settling_a_closed_due_is_refused() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 100_000, "2026-07-20", acct)).await.unwrap();
        settle(&p, id, 100_000, d("2026-08-05"), acct).await.unwrap();
        assert!(settle(&p, id, 100_000, d("2026-09-05"), acct).await.is_err());
    }

    #[tokio::test]
    async fn borrowing_runs_the_other_way() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::borrowed("Amma", 2_000_000, "2026-07-01", acct)).await.unwrap();
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 12_000_000, "the cash arrived");
        let borrowed = accounts::virtual_id(&p, BORROWED).await.unwrap();
        assert_eq!(accounts::balance(&p, borrowed).await.unwrap(), -2_000_000);
        // And it is not income.
        assert_eq!(txn::income_between(&p, "2026-01-01", "2026-12-31").await.unwrap(), 0);

        settle(&p, id, 2_000_000, d("2026-09-01"), acct).await.unwrap();
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000);
        assert_eq!(accounts::balance(&p, borrowed).await.unwrap(), 0);
        assert_eq!(txn::spent_between(&p, "2026-01-01", "2026-12-31").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn writing_off_is_the_one_place_lending_becomes_spending() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 500_000, "2024-01-20", acct)).await.unwrap();
        write_off(&p, id, d("2026-07-26")).await.unwrap();

        assert_eq!(
            txn::spent_between(&p, "2026-01-01", "2026-12-31").await.unwrap(),
            500_000,
            "money that is never coming back has, at that point, been spent"
        );
        let lent = accounts::virtual_id(&p, LENT_OUT).await.unwrap();
        assert_eq!(accounts::balance(&p, lent).await.unwrap(), 0, "the bucket must not keep claiming it");

        let due = list(&p, None).await.unwrap().into_iter().find(|x| x.id == id).unwrap();
        assert_eq!(due.status, Status::WrittenOff);
    }

    #[tokio::test]
    async fn a_debt_forgiven_in_your_favour_is_income() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::borrowed("Amma", 200_000, "2024-01-01", acct)).await.unwrap();
        write_off(&p, id, d("2026-07-26")).await.unwrap();
        assert_eq!(txn::income_between(&p, "2026-01-01", "2026-12-31").await.unwrap(), 200_000);
        let borrowed = accounts::virtual_id(&p, BORROWED).await.unwrap();
        assert_eq!(accounts::balance(&p, borrowed).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_due_with_no_account_records_without_moving_money() {
        let p = pool().await;
        let mut nd = NewDue::lent("Old debt", 100_000, "2020-01-01", 0);
        nd.account_id = None;
        create(&p, &nd).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 0, "a debt that predates the app has no posting to make");
        assert_eq!(totals(&p).await.unwrap().owed_to_me_minor, 100_000);
    }

    #[tokio::test]
    async fn totals_report_both_directions_separately() {
        let p = pool().await;
        let acct = bank(&p).await;
        create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        create(&p, &NewDue::lent("Sneha", 250_000, "2026-07-21", acct)).await.unwrap();
        let owed = create(&p, &NewDue::borrowed("Amma", 2_000_000, "2026-07-01", acct)).await.unwrap();

        let t = totals(&p).await.unwrap();
        assert_eq!(t.owed_to_me_minor, 750_000);
        assert_eq!(t.i_owe_minor, 2_000_000);

        settle(&p, owed, 2_000_000, d("2026-08-01"), acct).await.unwrap();
        assert_eq!(totals(&p).await.unwrap().i_owe_minor, 0, "settled dues leave the total");
    }

    #[tokio::test]
    async fn a_nameless_or_zero_due_is_refused() {
        let p = pool().await;
        let acct = bank(&p).await;
        assert!(create(&p, &NewDue::lent("  ", 100, "2026-07-01", acct)).await.is_err());
        assert!(create(&p, &NewDue::lent("Ravi", 0, "2026-07-01", acct)).await.is_err());
        assert!(create(&p, &NewDue::lent("Ravi", 100, "nope", acct)).await.is_err());
    }

    #[tokio::test]
    async fn deleting_a_due_removes_its_postings_too() {
        let p = pool().await;
        let acct = bank(&p).await;
        let id = create(&p, &NewDue::lent("Ravi", 500_000, "2026-07-20", acct)).await.unwrap();
        settle(&p, id, 200_000, d("2026-08-05"), acct).await.unwrap();
        delete(&p, id).await.unwrap();

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 0);
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 10_000_000);
        let all = txn::page(&p, &TxnFilter::default(), TxnSort::Date, true, 0).await.unwrap();
        assert_eq!(all.total, 0);
    }
}
