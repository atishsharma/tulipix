//! Where money sits, and what it adds up to.
//!
//! A loan is an account with a negative balance, and a credit card is the same
//! shape — which is what lets the Accounts tab total liquid money and debt from
//! one query instead of keeping a separate debt model in step with a separate
//! asset model.
//!
//! **Balances are derived, never stored.** There is no `balance_minor` column,
//! because a stored balance and a ledger are two sources of truth that drift the
//! first time a posting is edited, and the drift is silent. The cost is a `SUM`
//! per account, which is indexed and cheap. [`reconcile`] is how a derived
//! balance is brought back in line with a real statement: by posting the
//! difference as a visible adjustment, not by overwriting a number.

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

use crate::txn::{self, NewTxn, TxnKind, SIGNED};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    Bank,
    Cash,
    Card,
    Wallet,
    Loan,
    /// A holding bucket, not real money: `Lent out` and `Borrowed`. Excluded
    /// from every liquid and debt total.
    Virtual,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Bank => "bank",
            AccountKind::Cash => "cash",
            AccountKind::Card => "card",
            AccountKind::Wallet => "wallet",
            AccountKind::Loan => "loan",
            AccountKind::Virtual => "virtual",
        }
    }

    pub fn parse(s: &str) -> AccountKind {
        match s {
            "cash" => AccountKind::Cash,
            "card" => AccountKind::Card,
            "wallet" => AccountKind::Wallet,
            "loan" => AccountKind::Loan,
            "virtual" => AccountKind::Virtual,
            _ => AccountKind::Bank,
        }
    }

    /// Does this kind hold spendable money?
    pub fn is_liquid(self) -> bool {
        matches!(self, AccountKind::Bank | AccountKind::Cash | AccountKind::Wallet)
    }

    /// Does a negative balance on this kind mean debt?
    pub fn is_debt(self) -> bool {
        matches!(self, AccountKind::Card | AccountKind::Loan)
    }
}

#[derive(Clone, Debug)]
pub struct NewAccount {
    pub name: String,
    pub kind: AccountKind,
    pub currency: String,
    pub opening_minor: i64,
    pub credit_limit_minor: Option<i64>,
    pub statement_day: Option<i64>,
}

impl NewAccount {
    pub fn bank(name: &str, opening_minor: i64) -> Self {
        NewAccount {
            name: name.to_string(),
            kind: AccountKind::Bank,
            currency: "INR".to_string(),
            opening_minor,
            credit_limit_minor: None,
            statement_day: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AccountRow {
    pub id: i64,
    pub name: String,
    pub kind: AccountKind,
    pub currency: String,
    pub opening_minor: i64,
    pub credit_limit_minor: Option<i64>,
    pub statement_day: Option<i64>,
    pub closed: bool,
    pub sort_order: i64,
    /// Derived: opening plus every posting that touches this account.
    pub balance_minor: i64,
    pub txn_count: i64,
}

impl AccountRow {
    /// Fraction of the credit limit used, 0-100. `None` when there is no limit.
    ///
    /// A card's balance is negative when money is owed, so utilisation is the
    /// negated balance over the limit.
    pub fn utilisation_pct(&self) -> Option<i64> {
        let limit = self.credit_limit_minor.filter(|l| *l > 0)?;
        let owed = (-self.balance_minor).max(0);
        Some(((owed as i128 * 100) / limit as i128) as i64)
    }
}

/// Every account with its derived balance, ordered for display.
pub async fn list(pool: &SqlitePool, include_closed: bool) -> Result<Vec<AccountRow>> {
    // The correlated subqueries are what keep this to one round trip. `?1` is
    // the account id inside `SIGNED`, and it has to be the row's own id, so it
    // is `a.id` rather than a bound parameter.
    let signed = SIGNED.replace('?', "a.id");
    let sql = format!(
        "SELECT a.id, a.name, a.kind, a.currency, a.opening_minor, a.credit_limit_minor,
                a.statement_day, a.closed, a.sort_order,
                a.opening_minor + COALESCE((
                    SELECT SUM({signed}) FROM transactions
                     WHERE account_id = a.id OR to_account_id = a.id
                ), 0) AS balance_minor,
                COALESCE((
                    SELECT COUNT(*) FROM transactions
                     WHERE account_id = a.id OR to_account_id = a.id
                ), 0) AS txn_count
           FROM accounts a
          WHERE (? = 1 OR a.closed = 0)
          ORDER BY a.closed, a.sort_order, a.name COLLATE NOCASE"
    );
    let rows = sqlx::query_as::<
        _,
        (i64, String, String, String, i64, Option<i64>, Option<i64>, i64, i64, i64, i64),
    >(&sql)
    .bind(i64::from(include_closed))
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AccountRow {
            id: r.0,
            name: r.1,
            kind: AccountKind::parse(&r.2),
            currency: r.3,
            opening_minor: r.4,
            credit_limit_minor: r.5,
            statement_day: r.6,
            closed: r.7 != 0,
            sort_order: r.8,
            balance_minor: r.9,
            txn_count: r.10,
        })
        .collect())
}

/// One account's derived balance.
pub async fn balance(pool: &SqlitePool, id: i64) -> Result<i64> {
    let opening: i64 = sqlx::query_scalar("SELECT opening_minor FROM accounts WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .context("no such account")?;
    let delta: i64 = sqlx::query_scalar(&format!(
        "SELECT COALESCE(SUM({SIGNED}), 0) FROM transactions
          WHERE account_id = ? OR to_account_id = ?"
    ))
    .bind(id)
    .bind(id)
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(opening + delta)
}

pub async fn create(pool: &SqlitePool, a: &NewAccount) -> Result<i64> {
    if a.name.trim().is_empty() {
        bail!("an account needs a name");
    }
    let now = crate::schema::unix_now();
    let id = sqlx::query(
        "INSERT INTO accounts
           (name, kind, currency, opening_minor, credit_limit_minor, statement_day, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(a.name.trim())
    .bind(a.kind.as_str())
    .bind(a.currency.to_uppercase())
    .bind(a.opening_minor)
    .bind(a.credit_limit_minor)
    .bind(a.statement_day.map(|d| d.clamp(1, 31)))
    .bind(now)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

pub async fn rename(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("an account needs a name");
    }
    sqlx::query("UPDATE accounts SET name = ? WHERE id = ?")
        .bind(name.trim())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    opening_minor: i64,
    credit_limit_minor: Option<i64>,
    statement_day: Option<i64>,
) -> Result<()> {
    if name.trim().is_empty() {
        bail!("an account needs a name");
    }
    sqlx::query(
        "UPDATE accounts
            SET name = ?, opening_minor = ?, credit_limit_minor = ?, statement_day = ?
          WHERE id = ?",
    )
    .bind(name.trim())
    .bind(opening_minor)
    .bind(credit_limit_minor)
    .bind(statement_day.map(|d| d.clamp(1, 31)))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_closed(pool: &SqlitePool, id: i64, closed: bool) -> Result<()> {
    sqlx::query("UPDATE accounts SET closed = ? WHERE id = ?")
        .bind(i64::from(closed))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn reorder(pool: &SqlitePool, ids: &[i64]) -> Result<()> {
    for (i, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE accounts SET sort_order = ? WHERE id = ?")
            .bind(i as i64)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Delete an account, refusing if it has history.
///
/// Deleting would either orphan those postings or cascade them away, and both
/// silently change past months' totals. Closing is the operation for an account
/// that is finished with; deletion is only for one added by mistake.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    let used: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM transactions WHERE account_id = ? OR to_account_id = ?",
    )
    .bind(id)
    .bind(id)
    .fetch_one(pool)
    .await?;
    if used > 0 {
        bail!("{used} transactions use this account — close it instead of deleting it");
    }
    let is_virtual: Option<String> =
        sqlx::query_scalar("SELECT kind FROM accounts WHERE id = ?").bind(id).fetch_optional(pool).await?;
    if is_virtual.as_deref() == Some("virtual") {
        bail!("the lending holding accounts are part of how dues work and cannot be deleted");
    }
    sqlx::query("DELETE FROM accounts WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Spendable money: bank, cash and wallet balances.
    pub liquid_minor: i64,
    /// What is owed: card and loan balances, as a positive figure.
    pub debt_minor: i64,
}

/// Liquid and debt, side by side and never subtracted.
///
/// The design is explicit that net worth is not shown: it would need the flat's
/// market value, and a number the app has to ask the user to guess is a number
/// it should not print. Both of these are real, so both are returned; their
/// difference is deliberately not computed here or anywhere.
pub async fn totals(pool: &SqlitePool) -> Result<Totals> {
    let rows = list(pool, false).await?;
    let mut t = Totals::default();
    for a in rows {
        if a.kind.is_liquid() {
            t.liquid_minor += a.balance_minor;
        } else if a.kind.is_debt() {
            t.debt_minor += (-a.balance_minor).max(0);
        }
    }
    Ok(t)
}

/// Bring a derived balance in line with a real statement.
///
/// Posts the difference as an ordinary, visible transaction rather than editing
/// a balance, so the ledger still explains every rupee and the adjustment can be
/// found, questioned and deleted later. Returns the posted id, or `None` when
/// the books already agree.
pub async fn reconcile(
    pool: &SqlitePool,
    account_id: i64,
    actual_minor: i64,
    on: &str,
) -> Result<Option<i64>> {
    let derived = balance(pool, account_id).await?;
    let diff = actual_minor - derived;
    if diff == 0 {
        return Ok(None);
    }
    let currency: String = sqlx::query_scalar("SELECT currency FROM accounts WHERE id = ?")
        .bind(account_id)
        .fetch_one(pool)
        .await?;
    // Categorised as a fee when money is missing and as uncategorised income
    // when there is more than expected — the two common real causes.
    let cat_name = if diff < 0 { "Fees & Charges" } else { "Refunds" };
    let category_id: Option<i64> = sqlx::query_scalar("SELECT id FROM categories WHERE name = ?")
        .bind(cat_name)
        .fetch_optional(pool)
        .await?;

    let mut t = NewTxn::expense(account_id, diff.abs(), &currency, on, "Reconciliation adjustment");
    t.kind = if diff < 0 { TxnKind::Expense } else { TxnKind::Income };
    t.category_id = category_id;
    t.note = Some(format!("Statement said {actual_minor}; ledger said {derived}"));
    Ok(Some(txn::post(pool, &t).await?))
}

/// The id of a seeded virtual holding account, by name.
pub async fn virtual_id(pool: &SqlitePool, name: &str) -> Result<i64> {
    sqlx::query_scalar("SELECT id FROM accounts WHERE name = ? AND kind = 'virtual'")
        .bind(name)
        .fetch_optional(pool)
        .await?
        .with_context(|| format!("virtual account {name:?} is missing — seed_defaults did not run"))
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

    #[tokio::test]
    async fn a_balance_is_opening_plus_the_ledger() {
        let p = pool().await;
        let bank = create(&p, &NewAccount::bank("HDFC", 1_000_000)).await.unwrap();
        assert_eq!(balance(&p, bank).await.unwrap(), 1_000_000);

        txn::post(&p, &NewTxn::expense(bank, 40_000, "INR", "2026-07-10", "Veg")).await.unwrap();
        assert_eq!(balance(&p, bank).await.unwrap(), 960_000);

        let mut salary = NewTxn::expense(bank, 5_000_000, "INR", "2026-07-01", "Salary");
        salary.kind = TxnKind::Income;
        txn::post(&p, &salary).await.unwrap();
        assert_eq!(balance(&p, bank).await.unwrap(), 5_960_000);
    }

    #[tokio::test]
    async fn list_agrees_with_balance_for_every_account() {
        let p = pool().await;
        let a = create(&p, &NewAccount::bank("A", 500_000)).await.unwrap();
        let b = create(&p, &NewAccount::bank("B", 0)).await.unwrap();
        txn::post(&p, &NewTxn::transfer(a, b, 200_000, "INR", "2026-07-02", "Move")).await.unwrap();
        txn::post(&p, &NewTxn::expense(a, 50_000, "INR", "2026-07-03", "Veg")).await.unwrap();

        for row in list(&p, true).await.unwrap() {
            assert_eq!(
                row.balance_minor,
                balance(&p, row.id).await.unwrap(),
                "{} disagrees between the list query and the single-account query",
                row.name
            );
        }
    }

    #[tokio::test]
    async fn liquid_and_debt_are_reported_separately_and_never_subtracted() {
        let p = pool().await;
        create(&p, &NewAccount::bank("HDFC", 10_839_000)).await.unwrap();
        let card = create(
            &p,
            &NewAccount {
                kind: AccountKind::Card,
                credit_limit_minor: Some(20_000_000),
                ..NewAccount::bank("ICICI Card", 0)
            },
        )
        .await
        .unwrap();
        txn::post(&p, &NewTxn::expense(card, 64_900, "INR", "2026-07-26", "Netflix")).await.unwrap();

        let t = totals(&p).await.unwrap();
        assert_eq!(t.liquid_minor, 10_839_000);
        assert_eq!(t.debt_minor, 64_900, "a card balance owed is debt, as a positive figure");
    }

    #[tokio::test]
    async fn the_lending_buckets_are_in_neither_total() {
        let p = pool().await;
        let bank = create(&p, &NewAccount::bank("HDFC", 1_000_000)).await.unwrap();
        let lent = virtual_id(&p, schema::LENT_OUT).await.unwrap();
        // Lending ₹5,000 is a transfer, so it leaves the bank and lands nowhere
        // that counts as either an asset total or a debt total.
        txn::post(&p, &NewTxn::transfer(bank, lent, 500_000, "INR", "2026-07-20", "Lent to Ravi"))
            .await
            .unwrap();

        let t = totals(&p).await.unwrap();
        assert_eq!(t.liquid_minor, 500_000, "the cash really has left the bank");
        assert_eq!(t.debt_minor, 0);
        assert_eq!(balance(&p, lent).await.unwrap(), 500_000, "but it is still tracked");
    }

    #[tokio::test]
    async fn card_utilisation() {
        let p = pool().await;
        let card = create(
            &p,
            &NewAccount {
                kind: AccountKind::Card,
                credit_limit_minor: Some(10_000_000),
                ..NewAccount::bank("Card", 0)
            },
        )
        .await
        .unwrap();
        txn::post(&p, &NewTxn::expense(card, 2_500_000, "INR", "2026-07-01", "Shopping"))
            .await
            .unwrap();
        let row = list(&p, false).await.unwrap().into_iter().find(|a| a.id == card).unwrap();
        assert_eq!(row.utilisation_pct(), Some(25));

        let bank = create(&p, &NewAccount::bank("No limit", 0)).await.unwrap();
        let row = list(&p, false).await.unwrap().into_iter().find(|a| a.id == bank).unwrap();
        assert_eq!(row.utilisation_pct(), None);
    }

    #[tokio::test]
    async fn reconciling_posts_a_visible_adjustment_rather_than_editing_a_balance() {
        let p = pool().await;
        let bank = create(&p, &NewAccount::bank("HDFC", 1_000_000)).await.unwrap();
        // The statement says ₹9,850; the ledger says ₹10,000. ₹150 of charges.
        let id = reconcile(&p, bank, 985_000, "2026-07-31").await.unwrap();
        assert!(id.is_some());
        assert_eq!(balance(&p, bank).await.unwrap(), 985_000);

        let row: (String, i64, String) =
            sqlx::query_as("SELECT kind, base_minor, description FROM transactions WHERE id = ?")
                .bind(id.unwrap())
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(row.0, "expense");
        assert_eq!(row.1, 15_000);
        assert_eq!(row.2, "Reconciliation adjustment");

        // Reconciling again when the books agree must post nothing.
        assert!(reconcile(&p, bank, 985_000, "2026-07-31").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn reconciling_upward_posts_income() {
        let p = pool().await;
        let bank = create(&p, &NewAccount::bank("HDFC", 1_000_000)).await.unwrap();
        let id = reconcile(&p, bank, 1_020_000, "2026-07-31").await.unwrap().unwrap();
        let kind: String = sqlx::query_scalar("SELECT kind FROM transactions WHERE id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(kind, "income");
        assert_eq!(balance(&p, bank).await.unwrap(), 1_020_000);
    }

    #[tokio::test]
    async fn an_account_with_history_cannot_be_deleted() {
        let p = pool().await;
        let bank = create(&p, &NewAccount::bank("HDFC", 0)).await.unwrap();
        txn::post(&p, &NewTxn::expense(bank, 100, "INR", "2026-07-01", "x")).await.unwrap();
        assert!(delete(&p, bank).await.is_err(), "deleting would rewrite past totals");

        let empty = create(&p, &NewAccount::bank("Typo", 0)).await.unwrap();
        delete(&p, empty).await.unwrap();
    }

    #[tokio::test]
    async fn the_virtual_accounts_cannot_be_deleted() {
        let p = pool().await;
        let lent = virtual_id(&p, schema::LENT_OUT).await.unwrap();
        assert!(delete(&p, lent).await.is_err());
    }

    #[tokio::test]
    async fn closed_accounts_are_hidden_but_not_gone() {
        let p = pool().await;
        let old = create(&p, &NewAccount::bank("Old bank", 0)).await.unwrap();
        set_closed(&p, old, true).await.unwrap();
        assert!(!list(&p, false).await.unwrap().iter().any(|a| a.id == old));
        assert!(list(&p, true).await.unwrap().iter().any(|a| a.id == old));
    }

    #[tokio::test]
    async fn a_nameless_account_is_refused() {
        let p = pool().await;
        assert!(create(&p, &NewAccount::bank("   ", 0)).await.is_err());
    }
}
