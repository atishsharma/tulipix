//! Sample data, so nine empty tabs are not the first thing the section shows.
//!
//! An empty finance section is indistinguishable from a broken one: every chart
//! is blank, every total is zero, and there is no way to tell a bug from a lack
//! of data. This seeds one plausible month of everything, once, on a database
//! that has never held anything real.
//!
//! # Two rules it has to obey
//!
//! **It says so.** Everything seeded is recorded in `demo_rows`, the UI carries a
//! banner while any of it survives, and it can all be removed with one button.
//! Invented money that looks real is exactly the dishonesty the rest of this
//! section is built to avoid.
//!
//! **It gets out of the way by itself.** [`prune`] drops a group the moment the
//! user has one real row of that kind — a real account removes the sample
//! accounts, a real bill removes the sample bills. Per group rather than all at
//! once, so adding one account does not blank the six other tabs the user has not
//! got to yet.
//!
//! It is built with the same public functions the UI calls, not with hand-written
//! INSERTs. That way seeding exercises the real posting, materialisation and
//! settlement paths, and a sample month that renders is evidence those paths work.

use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};
use sqlx::SqlitePool;

use crate::{
    accounts::{self, AccountKind, NewAccount},
    budgets, date,
    date::Cycle,
    dues::{self, NewDue},
    loans::{self, NewLoan},
    obligations, recur,
    recur::NewRecurrence,
    txn::{self, NewTxn, TxnKind},
};

/// Groups, each pruned independently. The `&str` is the `demo_rows.kind`.
///
/// Order matters for deletion: a group is removed before anything it points at,
/// so `transactions` precedes `accounts` and the foreign keys hold throughout.
const GROUPS: &[&str] = &["transactions", "obligations", "recurrences", "dues", "budgets", "accounts"];

/// Marker recorded once seeding has happened, under a kind no real row uses.
///
/// Without it, a user who deletes the samples by hand gets them back on the next
/// launch, which would read as the app refusing to be emptied.
const SEEDED: &str = "seeded";

async fn mark(pool: &SqlitePool, kind: &str, id: i64) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO demo_rows (kind, row_id) VALUES (?, ?)")
        .bind(kind)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn ids(pool: &SqlitePool, kind: &str) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT row_id FROM demo_rows WHERE kind = ?")
        .bind(kind)
        .fetch_all(pool)
        .await?)
}

/// Has this database ever been seeded?
pub async fn seeded(pool: &SqlitePool) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM demo_rows WHERE kind = ?")
        .bind(SEEDED)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// Is any sample data still on screen? Drives the banner.
pub async fn present(pool: &SqlitePool) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM demo_rows WHERE kind <> ?")
        .bind(SEEDED)
        .fetch_one(pool)
        .await?;
    Ok(n > 0)
}

/// Whether this database has anything real in it.
///
/// Checked before seeding rather than trusting the marker alone, so an upgrade
/// that adds this module to an established database never touches it.
async fn is_empty(pool: &SqlitePool) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM transactions)
              + (SELECT COUNT(*) FROM recurrences)
              + (SELECT COUNT(*) FROM dues)
              + (SELECT COUNT(*) FROM budgets)
              + (SELECT COUNT(*) FROM accounts WHERE kind <> 'virtual')",
    )
    .fetch_one(pool)
    .await?;
    Ok(n == 0)
}

/// Seed one month of plausible finances. Returns whether anything was written.
///
/// Dated relative to `today` so the sample always looks current: a fixed month
/// would show a section full of two-year-old bills.
pub async fn seed(pool: &SqlitePool, today: NaiveDate) -> Result<bool> {
    if seeded(pool).await? || !is_empty(pool).await? {
        return Ok(false);
    }
    // Written before the rows, so a failure halfway through cannot come back on
    // the next launch and seed a second copy on top of the first.
    mark(pool, SEEDED, 1).await?;

    let iso = |d: NaiveDate| date::iso(d);
    let ago = |n: i64| today - Duration::days(n);
    let ahead = |n: i64| today + Duration::days(n);

    // ── accounts ────────────────────────────────────────────────────────────
    // The opening balance has to cover a year of the home-loan EMIs below and
    // still leave a plausible figure, or the sample opens on an overdrawn account.
    let bank = accounts::create(pool, &NewAccount::bank("HDFC Savings", 65_000_000)).await?;

    let cash = accounts::create(
        pool,
        &NewAccount { kind: AccountKind::Cash, ..NewAccount::bank("Cash", 250_000) },
    )
    .await?;

    // A card with a limit, so utilisation has something to show.
    let card = accounts::create(
        pool,
        &NewAccount {
            kind: AccountKind::Card,
            credit_limit_minor: Some(20_000_000),
            statement_day: Some(18),
            ..NewAccount::bank("ICICI Card", 0)
        },
    )
    .await?;

    let wallet = accounts::create(
        pool,
        &NewAccount { kind: AccountKind::Wallet, ..NewAccount::bank("Paytm", 300_000) },
    )
    .await?;

    // ── a loan, which is an account with a negative balance ─────────────────
    let loan = loans::create(
        pool,
        &NewLoan {
            name: "Home loan".into(),
            principal_minor: 350_000_000,
            rate_bp: 840,
            tenure_months: 240,
            started_on: iso(ago(400)),
            emi_minor: None,
            emi_day: 5,
            currency: "INR".into(),
        },
    )
    .await?;
    // Thirteen months of EMIs actually paid, so the progress bar, the
    // principal-versus-interest split and "227 of 240 left" all have something
    // true behind them. A loan started a year ago with nothing paid would be the
    // sort of detail that makes sample data obviously fake.
    let emi = loans::get(pool, loan).await?.map(|l| l.emi_minor).unwrap_or(0);
    if emi > 0 {
        for n in 0..13 {
            loans::pay_emi(pool, loan, bank, emi, ago(400 - n * 30)).await?;
        }
    }

    // ── categories to hang things off ───────────────────────────────────────
    // Names taken from `schema::SEED_CATEGORIES` exactly, so the sample uses the
    // user's own categories rather than inventing near-duplicates beside them.
    let food = category(pool, "Eating out").await?;
    let groceries = category(pool, "Groceries").await?;
    let transport = category(pool, "Transport").await?;
    let bills_cat = category(pool, "Utilities").await?;
    let subs_cat = category(pool, "Subscriptions").await?;
    let salary = category(pool, "Salary").await?;

    // ── the ledger ──────────────────────────────────────────────────────────
    // Income first, so the running balance reads the way a real month does.
    let mut rows: Vec<(NewTxn, Option<i64>)> = Vec::new();

    let mut pay = NewTxn::expense(bank, 12_500_000, "INR", &iso(ago(26)), "SALARY");
    pay.kind = TxnKind::Income;
    rows.push((pay, Some(salary)));

    // Six earlier months, thinner but real, so the twelve-month chart has a shape
    // and the six-month discipline table has rows. One month of data draws a
    // single bar, which tells the user nothing about either.
    for k in 1..=6i64 {
        let Some(month) = month_start(today, k) else { continue };
        let vary = k * 37_000; // enough that no two months are the same height
        let mut earlier = NewTxn::expense(
            bank,
            12_500_000 - vary,
            "INR",
            &iso(month + Duration::days(1)),
            "SALARY",
        );
        earlier.kind = TxnKind::Income;
        rows.push((earlier, Some(salary)));
        rows.push((
            NewTxn::expense(
                bank,
                1_850_000 + vary,
                "INR",
                &iso(month + Duration::days(6)),
                "Groceries and household",
            ),
            Some(groceries),
        ));
        rows.push((
            NewTxn::expense(
                card,
                640_000 - vary / 2,
                "INR",
                &iso(month + Duration::days(14)),
                "Eating out",
            ),
            Some(food),
        ));
    }

    for (days, amount, desc, c, acct) in [
        (24i64, 462_000, "BigBasket — monthly groceries", groceries, bank),
        (21, 34_000, "Auto to office", transport, cash),
        (19, 128_000, "Dinner — Toit", food, card),
        (16, 89_900, "Petrol", transport, card),
        (12, 24_000, "Chai and samosa", food, cash),
        (9, 156_000, "Blinkit — top-up shop", groceries, wallet),
        (5, 45_000, "Metro card recharge", transport, bank),
        (2, 68_000, "Lunch — Meghana", food, cash),
    ] {
        rows.push((NewTxn::expense(acct, amount, "INR", &iso(ago(days)), desc), Some(c)));
    }

    for (t, c) in rows {
        let mut t = t;
        t.category_id = c;
        t.source = "manual".into();
        txn::post(pool, &t).await?;
    }

    // A card payment, which is the transfer that most often gets miscounted as
    // spending. Seeded on purpose so the greyed-back row is visible.
    txn::post(
        pool,
        &NewTxn::transfer(bank, card, 217_900, "INR", &iso(ago(3)), "ICICI card payment"),
    )
    .await?;

    // ── subscriptions ───────────────────────────────────────────────────────
    // One that auto-posts, one in a foreign currency, one paused, and one whose
    // price went up — the four states the tab has to render.
    let netflix = {
        let mut r = NewRecurrence::subscription("Netflix", 64_900, &iso(ahead(5)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.auto_post = true;
        r.anchor_day = Some(ahead(5).day() as i64);
        recur::create(pool, &r).await?
    };
    // The hike this app exists to make visible: ₹499 → ₹649 three months ago.
    recur::record_price(pool, netflix, 49_900, "INR", &iso(ago(400))).await?;
    recur::record_price(pool, netflix, 64_900, "INR", &iso(ago(92))).await?;

    {
        let mut r = NewRecurrence::subscription("Some SaaS", 999, &iso(ahead(11)));
        r.currency = "USD".into();
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        recur::create(pool, &r).await?;
    }
    // Without a rate the foreign subscription cannot be shown in the base
    // currency at all, so the sample supplies one. Hand-edited, like every rate
    // in this section.
    crate::fx::set(pool, "USD", 83_600_000, crate::schema::unix_now()).await?;

    let paused = {
        let mut r = NewRecurrence::subscription("Gym", 150_000, &iso(ahead(20)));
        r.account_id = Some(bank);
        recur::create(pool, &r).await?
    };
    recur::set_status(pool, paused, recur::Status::Paused).await?;

    // ── bills ───────────────────────────────────────────────────────────────
    // A variable one with history, so the estimate is a real mean of three and
    // not a placeholder.
    let power = {
        let mut r = NewRecurrence::variable_bill("Electricity — BESCOM", &iso(ahead(9)));
        r.account_id = Some(bank);
        r.category_id = Some(bills_cat);
        r.reminder_days = 5;
        recur::create(pool, &r).await?
    };
    for (days, amount) in [(100i64, 300_000), (70, 320_000), (40, 328_400)] {
        let obl = obligations::create_one_off(pool, "Electricity — BESCOM", &iso(ago(days)), None).await?;
        sqlx::query("UPDATE obligations SET recurrence_id = ? WHERE id = ?")
            .bind(power)
            .bind(obl)
            .execute(pool)
            .await?;
        obligations::mark_paid(pool, obl, amount, ago(days), bank, "INR").await?;
    }

    {
        let mut r = NewRecurrence::subscription("Rent", 3_800_000, &iso(ahead(4)));
        r.account_id = Some(bank);
        r.cycle = Cycle::Monthly;
        r.anchor_day = Some(1);
        r.reminder_days = 7;
        recur::create(pool, &r).await?;
    }

    // One that is late, because an overdue row is the state most worth seeing
    // work: it drives the badge, the sweep and the notification.
    obligations::create_one_off(pool, "Water bill", &iso(ago(6)), Some(84_000)).await?;

    obligations::create_one_off(pool, "Society maintenance", &iso(ahead(7)), Some(250_000)).await?;

    // ── dues, in both directions ────────────────────────────────────────────
    dues::create(pool, &NewDue::lent("Ravi", 500_000, &iso(ago(34)), bank)).await?;
    dues::create(pool, &NewDue::borrowed("Dad", 1_500_000, &iso(ago(67)), bank)).await?;
    // A part-settled one, so the remainder-still-open path is exercised.
    let part = dues::create(pool, &NewDue::lent("Meera", 300_000, &iso(ago(20)), bank)).await?;
    dues::settle(pool, part, 100_000, ago(4), bank).await?;

    // ── budgets ─────────────────────────────────────────────────────────────
    // One comfortably inside, one over. Both states have their own colour and
    // neither is visible without data.
    // Set for the last four months, not only this one: the discipline table shows
    // six months and only counts a category that had an envelope that month, so
    // one period's worth of envelopes leaves it empty.
    for k in 0..4i64 {
        let Some(period) = month_start(today, k).map(date::ym) else { continue };
        for (c, amount, rollover) in
            [(groceries, 800_000, true), (food, 250_000, false), (transport, 300_000, false)]
        {
            budgets::set(pool, c, &period, amount, rollover).await?;
        }
    }

    // Bring the calendar and the badge up to date the same way a real launch
    // does, rather than leaving the seeded templates unmaterialised.
    recur::materialise_due(pool, today).await?;
    obligations::sweep(pool, today).await?;

    mark_everything(pool).await?;
    Ok(true)
}

/// The first of the month `k` months before `today`.
///
/// Through `checked_sub_months` from the 1st rather than by subtracting 30 days:
/// day arithmetic skips a month entirely across a February and would leave a gap
/// in the chart it is meant to fill.
fn month_start(today: NaiveDate, k: i64) -> Option<NaiveDate> {
    today.with_day(1)?.checked_sub_months(chrono::Months::new(k.max(0) as u32))
}

/// Record every row now in the database as sample data.
///
/// A sweep at the end rather than a `mark` beside each insert, because half these
/// rows are side effects: `dues::create` posts the lending transfer, `loans::create`
/// creates an account, `mark_paid` posts the payment, and `materialise_due` creates
/// obligations from the templates. Any one of them missed would leave an unmarked
/// row that reads as the user's own — and one of those is enough to make the very
/// first prune delete the entire sample.
///
/// Sound because [`seed`] only ever runs on a database with nothing real in it, so
/// at this moment everything present is sample data by definition.
async fn mark_everything(pool: &SqlitePool) -> Result<()> {
    for (kind, sql) in [
        ("transactions", "SELECT id FROM transactions"),
        ("obligations", "SELECT id FROM obligations"),
        ("recurrences", "SELECT id FROM recurrences"),
        ("dues", "SELECT id FROM dues"),
        ("budgets", "SELECT id FROM budgets"),
        // The two holding accounts are seeded for every user and are not samples.
        ("accounts", "SELECT id FROM accounts WHERE kind <> 'virtual'"),
    ] {
        for id in sqlx::query_scalar::<_, i64>(sql).fetch_all(pool).await? {
            mark(pool, kind, id).await?;
        }
    }
    Ok(())
}

/// A category id by name, creating it if the seed list does not have it.
async fn category(pool: &SqlitePool, name: &str) -> Result<i64> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT id FROM categories WHERE name = ? LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await?
    {
        return Ok(id);
    }
    Ok(sqlx::query("INSERT INTO categories (name, kind) VALUES (?, 'expense')")
        .bind(name)
        .execute(pool)
        .await?
        .last_insert_rowid())
}

/// Count of rows in `kind` that are *not* sample data.
async fn real_rows(pool: &SqlitePool, kind: &str) -> Result<i64> {
    // Written out per group rather than by interpolating a table name, so no
    // string from anywhere near the UI can reach the query.
    let sql = match kind {
        "transactions" => {
            "SELECT COUNT(*) FROM transactions t
              WHERE NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'transactions' AND d.row_id = t.id)"
        }
        "obligations" => {
            "SELECT COUNT(*) FROM obligations o
              WHERE NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'obligations' AND d.row_id = o.id)"
        }
        "recurrences" => {
            "SELECT COUNT(*) FROM recurrences r
              WHERE NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'recurrences' AND d.row_id = r.id)"
        }
        "dues" => {
            "SELECT COUNT(*) FROM dues x
              WHERE NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'dues' AND d.row_id = x.id)"
        }
        "budgets" => {
            "SELECT COUNT(*) FROM budgets b
              WHERE NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'budgets' AND d.row_id = b.id)"
        }
        // Virtual accounts are seeded for every user and are not samples.
        "accounts" => {
            "SELECT COUNT(*) FROM accounts a
              WHERE a.kind <> 'virtual'
                AND NOT EXISTS (SELECT 1 FROM demo_rows d
                                 WHERE d.kind = 'accounts' AND d.row_id = a.id)"
        }
        _ => return Ok(0),
    };
    Ok(sqlx::query_scalar(sql).fetch_one(pool).await?)
}

/// Drop the sample rows of every group the user has now got real data in.
///
/// Cheap enough to call on every refresh: six indexed counts, and it does nothing
/// at all once the last sample is gone.
pub async fn prune(pool: &SqlitePool) -> Result<u64> {
    if !present(pool).await? {
        return Ok(0);
    }
    let mut removed = 0;
    for kind in GROUPS {
        if real_rows(pool, kind).await? > 0 {
            removed += drop_group(pool, kind).await?;
        }
    }
    Ok(removed)
}

/// Remove every sample row, in dependency order.
pub async fn remove_all(pool: &SqlitePool) -> Result<u64> {
    let mut removed = 0;
    for kind in GROUPS {
        removed += drop_group(pool, kind).await?;
    }
    Ok(removed)
}

/// Delete one group's sample rows through the same functions the UI deletes with,
/// so their cleanup — unlinking postings, keeping real money — applies here too.
async fn drop_group(pool: &SqlitePool, kind: &str) -> Result<u64> {
    let rows = ids(pool, kind).await?;
    let mut n = 0;
    for id in &rows {
        let outcome = match kind {
            "transactions" => txn::delete(pool, *id).await,
            "obligations" => obligations::delete(pool, *id).await,
            "recurrences" => recur::delete(pool, *id).await,
            "dues" => dues::delete(pool, *id).await,
            "budgets" => delete_budget(pool, *id).await,
            "accounts" => delete_account(pool, *id).await,
            _ => Ok(()),
        };
        match outcome {
            Ok(()) => n += 1,
            // A sample row the user has since built on — a real transaction in a
            // sample account, say. Leaving it and its marker in place is right:
            // the next prune tries again, and nothing of the user's is destroyed
            // to tidy up something the app invented.
            Err(e) => {
                tracing::debug!("finances: sample {kind} {id} kept: {e}");
                continue;
            }
        }
        sqlx::query("DELETE FROM demo_rows WHERE kind = ? AND row_id = ?")
            .bind(kind)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(n)
}

/// Nothing references a budget row, so it goes directly rather than through
/// `budgets::remove`, which is keyed on the period and could not reach a sample
/// envelope once the month has turned over.
async fn delete_budget(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM budgets WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// A loan goes through `loans::delete`, which removes the loan row *and* the
/// account behind it; anything else goes straight to `accounts::delete`.
///
/// One or the other, never both: calling `accounts::delete` after `loans::delete`
/// fails on an account that is already gone, and a failure here leaves the marker
/// in place, so the banner would never clear.
async fn delete_account(pool: &SqlitePool, id: i64) -> Result<()> {
    let is_loan: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM loans WHERE account_id = ?)")
            .bind(id)
            .fetch_one(pool)
            .await?;
    if is_loan { loans::delete(pool, id).await } else { accounts::delete(pool, id).await }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema;

    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&p).await.unwrap();
        schema::seed_defaults(&p).await.unwrap();
        p
    }

    fn today() -> NaiveDate {
        date::parse("2026-07-26").unwrap()
    }

    /// The sample bank account, which is the one everything else hangs off.
    async fn sample_account(p: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT id FROM accounts WHERE name = 'HDFC Savings'")
            .fetch_one(p)
            .await
            .unwrap()
    }

    async fn count(p: &SqlitePool, table: &str) -> i64 {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        sqlx::query_scalar(&sql).fetch_one(p).await.unwrap()
    }

    #[tokio::test]
    async fn seeding_fills_every_tab() {
        let p = pool().await;
        assert!(seed(&p, today()).await.unwrap());
        assert!(present(&p).await.unwrap());

        assert!(count(&p, "accounts").await > 2, "beyond the two virtual ones");
        assert!(count(&p, "transactions").await > 5);
        assert!(count(&p, "recurrences").await >= 5);
        assert!(count(&p, "obligations").await > 3);
        assert_eq!(count(&p, "dues").await, 3);
        // Three envelopes across four months.
        assert_eq!(count(&p, "budgets").await, 12);
        // Six earlier months plus this one, so the twelve-month chart has a shape.
        let months = txn::monthly_totals(&p, 12).await.unwrap();
        assert!(months.iter().filter(|m| m.income_minor > 0).count() >= 6);
        assert_eq!(count(&p, "loans").await, 1);
    }

    #[tokio::test]
    async fn seeding_happens_once_and_never_on_a_database_with_real_data() {
        let p = pool().await;
        assert!(seed(&p, today()).await.unwrap());
        let n = count(&p, "transactions").await;
        assert!(!seed(&p, today()).await.unwrap(), "the marker stops a second copy");
        assert_eq!(count(&p, "transactions").await, n);

        // And a database that already has something real is never touched.
        let fresh = pool().await;
        let acct = accounts::create(&fresh, &NewAccount::bank("Mine", 0)).await.unwrap();
        txn::post(&fresh, &NewTxn::expense(acct, 100, "INR", "2026-07-01", "Real")).await.unwrap();
        assert!(!seed(&fresh, today()).await.unwrap());
        assert!(!present(&fresh).await.unwrap());
    }

    #[tokio::test]
    async fn a_real_due_removes_only_the_sample_dues() {
        // The whole point of pruning per group: adding one thing must not blank
        // the tabs the user has not got to yet.
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        let subs_before = count(&p, "recurrences").await;

        let bank = sample_account(&p).await;
        dues::create(&p, &NewDue::lent("Someone real", 1_000, "2026-07-20", bank)).await.unwrap();
        prune(&p).await.unwrap();

        assert_eq!(count(&p, "dues").await, 1, "only the user's own is left");
        assert_eq!(count(&p, "recurrences").await, subs_before, "and nothing else moved");
        assert!(present(&p).await.unwrap(), "so the banner stays up");
    }

    #[tokio::test]
    async fn a_real_transaction_in_a_sample_account_is_never_destroyed() {
        // The dangerous case. The account is sample data, the posting is not, and
        // tidying up must not take the user's row with it.
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        let bank = sample_account(&p).await;
        let mine = txn::post(
            &p,
            &NewTxn::expense(bank, 12_300, "INR", "2026-07-25", "My own spend"),
        )
        .await
        .unwrap();

        prune(&p).await.unwrap();

        let survived: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE id = ?")
            .bind(mine)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(survived, 1, "the user's posting is untouchable");
        let account_left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE id = ?")
            .bind(bank)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(account_left, 1, "and the account it needs is still there");
    }

    #[tokio::test]
    async fn removing_everything_leaves_a_clean_database() {
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        remove_all(&p).await.unwrap();

        assert!(!present(&p).await.unwrap());
        assert_eq!(count(&p, "transactions").await, 0);
        assert_eq!(count(&p, "recurrences").await, 0);
        assert_eq!(count(&p, "obligations").await, 0);
        assert_eq!(count(&p, "dues").await, 0);
        assert_eq!(count(&p, "budgets").await, 0);
        assert_eq!(count(&p, "loans").await, 0);
        // The two virtual holding accounts are not samples and stay.
        assert_eq!(count(&p, "accounts").await, 2);

        // And it does not come back.
        assert!(!seed(&p, today()).await.unwrap());
    }

    #[tokio::test]
    async fn the_sample_month_is_internally_consistent() {
        // If seeding produced nonsense the tabs would render nonsense, which is
        // worse than rendering nothing.
        let p = pool().await;
        seed(&p, today()).await.unwrap();

        let spent = txn::spent_between(&p, "2000-01-01", "2100-01-01").await.unwrap();
        let income = txn::income_between(&p, "2000-01-01", "2100-01-01").await.unwrap();
        assert!(spent > 0 && income > 0);

        // Every account the sample shows has to be one a real person could have.
        for a in accounts::list(&p, false).await.unwrap() {
            if matches!(a.kind, AccountKind::Bank | AccountKind::Cash | AccountKind::Wallet) {
                assert!(a.balance_minor >= 0, "{} is overdrawn: {}", a.name, a.balance_minor);
            }
        }

        // The card payment and the part-settlement are transfers, so neither may
        // appear in spending.
        let transfers: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE kind = 'transfer'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert!(transfers >= 2);

        let totals = accounts::totals(&p).await.unwrap();
        assert!(totals.liquid_minor > 0, "the sample is solvent");
        assert!(totals.debt_minor > 0, "and has the loan to show");

        // The variable bill has three paid actuals, so it can produce an estimate.
        let power: i64 = sqlx::query_scalar(
            "SELECT id FROM recurrences WHERE amount_minor IS NULL LIMIT 1",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert!(recur::estimate_for(&p, power).await.unwrap().is_some());
    }
}
