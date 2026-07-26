//! The ledger. Every posting, whatever produced it.
//!
//! One table holds manual entries, imported rows, OCR'd receipts, auto-posted
//! subscriptions and confirmed bills, distinguished by `source` so provenance is
//! visible in the UI rather than inferred. There is no second table for
//! "pending" or "scheduled" money — that is what `obligations` is, and the
//! difference between the two is the difference between owing and having paid.
//!
//! **The honesty rule this module enforces:** every total that claims to be
//! spending filters on `kind = 'expense'`. Transfers — wallet top-ups, card
//! payments, due settlements, lending — are movements between things the user
//! already owns and are not expenditure. They stay visible in the ledger, and
//! they never reach a spend figure. See [`SPEND`].

use anyhow::{bail, Result};
use sqlx::{Row, SqlitePool};

use crate::money;

/// 25 rows a page, per the design. Small enough that the running-balance window
/// query stays cheap and large enough that a month fits in two pages.
pub const PAGE: u32 = 25;

/// The predicate every spend total uses. Named so a review can grep for the
/// places that should have used it and did not.
pub const SPEND: &str = "kind = 'expense'";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxnKind {
    Expense,
    Income,
    Transfer,
}

impl TxnKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TxnKind::Expense => "expense",
            TxnKind::Income => "income",
            TxnKind::Transfer => "transfer",
        }
    }

    pub fn parse(s: &str) -> TxnKind {
        match s {
            "income" => TxnKind::Income,
            "transfer" => TxnKind::Transfer,
            _ => TxnKind::Expense,
        }
    }
}

/// Which column the ledger is ordered by.
///
/// A closed enum, not a string, because `ORDER BY` cannot take a bound
/// parameter — the column name has to be interpolated into the SQL, and the only
/// safe way to interpolate is to have nothing user-supplied to interpolate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxnSort {
    Date,
    Description,
    Category,
    Account,
    Source,
    Amount,
}

impl TxnSort {
    /// The `ORDER BY` fragment. Every arm is a literal; nothing flows in here
    /// from outside the crate.
    fn column(self) -> &'static str {
        match self {
            TxnSort::Date => "t.occurred_on",
            TxnSort::Description => "t.description COLLATE NOCASE",
            TxnSort::Category => "c.name COLLATE NOCASE",
            TxnSort::Account => "a.name COLLATE NOCASE",
            TxnSort::Source => "t.source",
            TxnSort::Amount => "t.base_minor",
        }
    }

    pub fn parse(s: &str) -> TxnSort {
        match s {
            "description" => TxnSort::Description,
            "category" => TxnSort::Category,
            "account" => TxnSort::Account,
            "source" => TxnSort::Source,
            "amount" => TxnSort::Amount,
            _ => TxnSort::Date,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TxnFilter {
    pub account_id: Option<i64>,
    pub category_id: Option<i64>,
    pub kind: Option<TxnKind>,
    /// Inclusive ISO date bounds.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Substring match on description and merchant.
    pub search: Option<String>,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewTxn {
    pub account_id: i64,
    pub to_account_id: Option<i64>,
    pub kind: TxnKind,
    pub amount_minor: i64,
    pub currency: String,
    pub rate_micro: i64,
    pub occurred_on: String,
    pub description: String,
    pub category_id: Option<i64>,
    pub recurrence_id: Option<i64>,
    pub obligation_id: Option<i64>,
    pub due_id: Option<i64>,
    pub source: String,
    pub note: Option<String>,
}

impl NewTxn {
    /// A plain expense in the base currency — the shape almost every manual
    /// entry and every posted bill has.
    pub fn expense(account_id: i64, amount_minor: i64, currency: &str, on: &str, description: &str) -> Self {
        NewTxn {
            account_id,
            to_account_id: None,
            kind: TxnKind::Expense,
            amount_minor,
            currency: currency.to_string(),
            rate_micro: money::RATE_ONE,
            occurred_on: on.to_string(),
            description: description.to_string(),
            category_id: None,
            recurrence_id: None,
            obligation_id: None,
            due_id: None,
            source: "manual".to_string(),
            note: None,
        }
    }

    /// A movement between two accounts the user already owns. Not spending.
    pub fn transfer(from: i64, to: i64, amount_minor: i64, currency: &str, on: &str, description: &str) -> Self {
        NewTxn {
            to_account_id: Some(to),
            kind: TxnKind::Transfer,
            ..NewTxn::expense(from, amount_minor, currency, on, description)
        }
    }
}

#[derive(Clone, Debug)]
pub struct TxnRow {
    pub id: i64,
    pub account_id: i64,
    pub account_name: String,
    pub to_account_id: Option<i64>,
    pub to_account_name: Option<String>,
    pub kind: TxnKind,
    pub amount_minor: i64,
    pub currency: String,
    pub base_minor: i64,
    pub occurred_on: String,
    pub description: String,
    pub category_id: Option<i64>,
    pub category_name: Option<String>,
    pub source: String,
    pub note: Option<String>,
    /// Balance after this row. Only populated when a single account is filtered —
    /// a running balance across several accounts at once is a number with no
    /// meaning, so it is absent rather than wrong.
    pub running_minor: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct TxnPage {
    pub rows: Vec<TxnRow>,
    pub total: i64,
    pub page: u32,
    pub pages: u32,
    /// Sum of `base_minor` over every row matching the filter, expense only.
    pub spent_minor: i64,
    pub income_minor: i64,
}

/// Merchant name, flattened for dedup and recurring-charge detection.
///
/// Statement exports append a reference number, a terminal id or a date to the
/// same merchant every month — `NETFLIX 4471`, `NETFLIX 5120` — so matching on
/// the raw description finds nothing repeating. Upper-cased, punctuation to
/// spaces, digit-only words dropped, collapsed.
pub fn normalise_merchant(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { ' ' })
        .collect();
    let words: Vec<&str> = cleaned
        .split_whitespace()
        // A word that is only digits is a reference, not a name. Kept when it is
        // the *only* word, because then it is all the identity there is.
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let joined = if words.is_empty() {
        cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        words.join(" ")
    };
    joined.chars().take(48).collect::<String>().trim_end().to_string()
}

/// Post a transaction, computing `base_minor` from the rate in force now.
///
/// Returns the new id, or the existing one when the dedup index rejects the row —
/// an importer re-running over an overlapping statement must be a no-op, not an
/// error the user has to read.
pub async fn post(pool: &SqlitePool, t: &NewTxn) -> Result<i64> {
    Ok(post_reporting(pool, t).await?.0)
}

/// As [`post`], and says whether the row was new.
///
/// The importer needs that distinction to report "142 added, 38 already there",
/// which is the only way a user can tell a working re-import from a broken one.
pub async fn post_reporting(pool: &SqlitePool, t: &NewTxn) -> Result<(i64, bool)> {
    if t.amount_minor < 0 {
        bail!("amount_minor must be positive; the kind carries the direction");
    }
    if t.kind == TxnKind::Transfer {
        match t.to_account_id {
            None => bail!("a transfer needs a destination account"),
            Some(to) if to == t.account_id => bail!("a transfer to the same account moves nothing"),
            _ => {}
        }
    }
    let base_minor = money::convert(t.amount_minor, t.rate_micro);
    let merchant = normalise_merchant(&t.description);

    let res = sqlx::query(
        "INSERT OR IGNORE INTO transactions
           (account_id, to_account_id, kind, amount_minor, currency, rate_micro, base_minor,
            occurred_on, description, merchant_norm, category_id, recurrence_id, obligation_id,
            due_id, source, note, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(t.account_id)
    .bind(t.to_account_id)
    .bind(t.kind.as_str())
    .bind(t.amount_minor)
    .bind(&t.currency)
    .bind(t.rate_micro)
    .bind(base_minor)
    .bind(&t.occurred_on)
    .bind(&t.description)
    .bind(&merchant)
    .bind(t.category_id)
    .bind(t.recurrence_id)
    .bind(t.obligation_id)
    .bind(t.due_id)
    .bind(&t.source)
    .bind(&t.note)
    .bind(crate::schema::unix_now())
    .execute(pool)
    .await?;

    if res.rows_affected() == 1 {
        return Ok((res.last_insert_rowid(), true));
    }
    // Rejected by ux_txn_dedup. Hand back the row that is already there.
    let existing: i64 = sqlx::query_scalar(
        "SELECT id FROM transactions
         WHERE account_id = ? AND occurred_on = ? AND amount_minor = ? AND merchant_norm = ?",
    )
    .bind(t.account_id)
    .bind(&t.occurred_on)
    .bind(t.amount_minor)
    .bind(&merchant)
    .fetch_one(pool)
    .await?;
    Ok((existing, false))
}

/// Change the parts of a row a user is allowed to change.
///
/// Not `account_id`: moving a posting between accounts silently rewrites two
/// balances, and deleting and re-adding is the honest way to do it.
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    amount_minor: i64,
    occurred_on: &str,
    description: &str,
    category_id: Option<i64>,
    note: Option<&str>,
) -> Result<()> {
    if amount_minor < 0 {
        bail!("amount_minor must be positive");
    }
    let rate: i64 = sqlx::query_scalar("SELECT rate_micro FROM transactions WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    sqlx::query(
        "UPDATE transactions
            SET amount_minor = ?, base_minor = ?, occurred_on = ?, description = ?,
                merchant_norm = ?, category_id = ?, note = ?
          WHERE id = ?",
    )
    .bind(amount_minor)
    .bind(money::convert(amount_minor, rate))
    .bind(occurred_on)
    .bind(description)
    .bind(normalise_merchant(description))
    .bind(category_id)
    .bind(note)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    // Detach the obligation first, or a paid bill keeps pointing at a row that
    // no longer exists and reads as paid forever.
    sqlx::query("UPDATE obligations SET status = 'upcoming', actual_minor = NULL, transaction_id = NULL WHERE transaction_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM transactions WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Build the shared `WHERE` clause and bind list for a filter.
///
/// Returns the SQL fragment; the caller binds in the same order this pushes.
fn where_clause(f: &TxnFilter) -> String {
    let mut parts: Vec<&str> = vec!["1 = 1"];
    if f.account_id.is_some() {
        // A transfer touches two accounts, and filtering to one of them must
        // show the leg that account was actually part of.
        parts.push("(t.account_id = ? OR t.to_account_id = ?)");
    }
    if f.category_id.is_some() {
        parts.push("t.category_id = ?");
    }
    if f.kind.is_some() {
        parts.push("t.kind = ?");
    }
    if f.from.is_some() {
        parts.push("t.occurred_on >= ?");
    }
    if f.to.is_some() {
        parts.push("t.occurred_on <= ?");
    }
    if f.search.is_some() {
        parts.push("(t.description LIKE ? OR t.merchant_norm LIKE ?)");
    }
    if f.source.is_some() {
        parts.push("t.source = ?");
    }
    parts.join(" AND ")
}

macro_rules! bind_filter {
    ($q:expr, $f:expr) => {{
        let mut q = $q;
        if let Some(a) = $f.account_id {
            q = q.bind(a).bind(a);
        }
        if let Some(c) = $f.category_id {
            q = q.bind(c);
        }
        if let Some(k) = $f.kind {
            q = q.bind(k.as_str());
        }
        if let Some(d) = $f.from.as_deref() {
            q = q.bind(d.to_string());
        }
        if let Some(d) = $f.to.as_deref() {
            q = q.bind(d.to_string());
        }
        if let Some(s) = $f.search.as_deref() {
            let like = format!("%{}%", s.trim());
            q = q.bind(like.clone()).bind(like);
        }
        if let Some(s) = $f.source.as_deref() {
            q = q.bind(s.to_string());
        }
        q
    }};
}

/// One page of the ledger, sorted in SQL.
pub async fn page(
    pool: &SqlitePool,
    filter: &TxnFilter,
    sort: TxnSort,
    desc: bool,
    page: u32,
) -> Result<TxnPage> {
    let cond = where_clause(filter);

    // These have to be `let` bindings, not `&format!(…)` inline: the macro binds
    // parameters onto the query, which borrows the SQL, so a temporary would be
    // dropped while the query still holds it.
    let count_sql = format!(
        "SELECT COUNT(*) FROM transactions t
           LEFT JOIN categories c ON c.id = t.category_id
           LEFT JOIN accounts   a ON a.id = t.account_id
         WHERE {cond}"
    );
    let total: i64 = bind_filter!(sqlx::query_scalar(&count_sql), filter)
        .fetch_one(pool)
        .await?;

    let sums_sql = format!(
        "SELECT COALESCE(SUM(CASE WHEN t.kind = 'expense' THEN t.base_minor ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN t.kind = 'income'  THEN t.base_minor ELSE 0 END), 0)
           FROM transactions t
           LEFT JOIN categories c ON c.id = t.category_id
           LEFT JOIN accounts   a ON a.id = t.account_id
         WHERE {cond}"
    );
    let (spent_minor, income_minor) = bind_filter!(sqlx::query_as::<_, (i64, i64)>(&sums_sql), filter)
        .fetch_one(pool)
        .await?;

    let pages = if total == 0 { 1 } else { ((total as u32).saturating_sub(1) / PAGE) + 1 };
    let page = page.min(pages.saturating_sub(1));
    let dir = if desc { "DESC" } else { "ASC" };
    let col = sort.column();

    // `id` as the tiebreaker keeps paging stable: two rows on the same date
    // would otherwise be free to swap between page 1 and page 2.
    let sql = format!(
        "SELECT t.id, t.account_id, a.name, t.to_account_id, d.name, t.kind, t.amount_minor,
                t.currency, t.base_minor, t.occurred_on, t.description, t.category_id, c.name,
                t.source, t.note
           FROM transactions t
           LEFT JOIN accounts   a ON a.id = t.account_id
           LEFT JOIN accounts   d ON d.id = t.to_account_id
           LEFT JOIN categories c ON c.id = t.category_id
         WHERE {cond}
         ORDER BY {col} {dir}, t.id {dir}
         LIMIT ? OFFSET ?"
    );

    let rows = bind_filter!(sqlx::query(&sql), filter)
        .bind(PAGE as i64)
        .bind((page * PAGE) as i64)
        .fetch_all(pool)
        .await?;

    let mut out: Vec<TxnRow> = rows
        .into_iter()
        .map(|r| TxnRow {
            id: r.get(0),
            account_id: r.get(1),
            account_name: r.get::<Option<String>, _>(2).unwrap_or_default(),
            to_account_id: r.get(3),
            to_account_name: r.get(4),
            kind: TxnKind::parse(&r.get::<String, _>(5)),
            amount_minor: r.get(6),
            currency: r.get(7),
            base_minor: r.get(8),
            occurred_on: r.get(9),
            description: r.get(10),
            category_id: r.get(11),
            category_name: r.get(12),
            source: r.get(13),
            note: r.get(14),
            running_minor: None,
        })
        .collect();

    // Running balance is only meaningful for one account, and only when sorted
    // by date — under any other sort the column would count rows in an order
    // that does not exist in time.
    if let Some(acct) = filter.account_id
        && sort == TxnSort::Date
    {
        attach_running(pool, acct, &mut out).await?;
    }

    Ok(TxnPage { rows: out, total, page, pages, spent_minor, income_minor })
}

/// Fill in `running_minor` for rows on one account.
///
/// One query, not one per row. The balance up to and including the *earliest*
/// row on the page is fetched once, then the page is walked forward applying
/// each row's own signed delta — which is already in hand, so the other 24 round
/// trips a naive version does buy nothing.
async fn attach_running(pool: &SqlitePool, account_id: i64, rows: &mut [TxnRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let opening: i64 = sqlx::query_scalar("SELECT opening_minor FROM accounts WHERE id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);

    // The page may be newest-first or oldest-first; the walk has to be
    // chronological either way, so work through indices in date order.
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        (rows[a].occurred_on.as_str(), rows[a].id).cmp(&(rows[b].occurred_on.as_str(), rows[b].id))
    });

    let first = &rows[order[0]];
    let before: i64 = sqlx::query_scalar(&format!(
        "SELECT COALESCE(SUM({SIGNED}), 0) FROM transactions
          WHERE (account_id = ? OR to_account_id = ?)
            AND (occurred_on < ? OR (occurred_on = ? AND id < ?))"
    ))
    .bind(account_id)
    .bind(account_id)
    .bind(account_id)
    .bind(&first.occurred_on)
    .bind(&first.occurred_on)
    .bind(first.id)
    .fetch_one(pool)
    .await?;

    let mut running = opening + before;
    for i in order {
        running += signed_delta(&rows[i], account_id);
        rows[i].running_minor = Some(running);
    }
    Ok(())
}

/// What this row does to `account_id`'s balance.
///
/// Money leaves on an expense and on the outbound leg of a transfer; it arrives
/// on income and on the inbound leg. The Rust mirror of [`SIGNED`] — the two must
/// agree, and [`the_two_signed_forms_agree`] is what holds them together.
fn signed_delta(row: &TxnRow, account_id: i64) -> i64 {
    match row.kind {
        TxnKind::Income => row.base_minor,
        TxnKind::Expense => -row.base_minor,
        TxnKind::Transfer => {
            if row.to_account_id == Some(account_id) {
                row.base_minor
            } else {
                -row.base_minor
            }
        }
    }
}

/// Signed contribution of a row to one account's balance, in SQL. The single `?`
/// takes the account id.
///
/// A transfer between two tracked accounts nets to zero across the pair, which
/// is exactly why transfers cannot be spending.
pub const SIGNED: &str = "CASE
        WHEN kind = 'income'                         THEN  base_minor
        WHEN kind = 'expense'                        THEN -base_minor
        WHEN kind = 'transfer' AND to_account_id = ? THEN  base_minor
        ELSE -base_minor
     END";

/// Income posted on each day of a month, keyed by ISO date.
///
/// The calendar shows every dated thing, and a salary credit is the one entry
/// that makes a cash-flow low point survivable. Obligations are money going out;
/// this is the other direction, which is why the calendar cell has to carry a
/// direction at all.
pub async fn income_by_day(pool: &SqlitePool, from: &str, to: &str) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT occurred_on, SUM(base_minor) FROM transactions
          WHERE kind = 'income' AND occurred_on BETWEEN ? AND ?
          GROUP BY occurred_on",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?)
}

#[derive(Clone, Debug)]
pub struct CategorySpend {
    pub category_id: Option<i64>,
    pub name: String,
    pub color: Option<String>,
    pub base_minor: i64,
}

/// Spending by category over a date range, biggest first.
///
/// Expense only. A donut that included transfers would show "Transfer" as the
/// largest slice in any month the user paid a credit card bill.
pub async fn spend_by_category(pool: &SqlitePool, from: &str, to: &str) -> Result<Vec<CategorySpend>> {
    let rows = sqlx::query_as::<_, (Option<i64>, Option<String>, Option<String>, i64)>(
        "SELECT t.category_id, c.name, c.color, SUM(t.base_minor)
           FROM transactions t LEFT JOIN categories c ON c.id = t.category_id
          WHERE t.kind = 'expense' AND t.occurred_on BETWEEN ? AND ?
          GROUP BY t.category_id
          ORDER BY SUM(t.base_minor) DESC",
    )
    .bind(from)
    .bind(to)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(category_id, name, color, base_minor)| CategorySpend {
            category_id,
            name: name.unwrap_or_else(|| "Uncategorised".into()),
            color,
            base_minor,
        })
        .collect())
}

#[derive(Clone, Debug)]
pub struct MonthTotal {
    pub period: String,
    pub income_minor: i64,
    pub expense_minor: i64,
}

/// Income and spending per month, oldest first, for the twelve-month bars.
pub async fn monthly_totals(pool: &SqlitePool, months: u32) -> Result<Vec<MonthTotal>> {
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT substr(occurred_on, 1, 7) AS period,
                COALESCE(SUM(CASE WHEN kind = 'income'  THEN base_minor ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN kind = 'expense' THEN base_minor ELSE 0 END), 0)
           FROM transactions
          GROUP BY period
          ORDER BY period DESC
          LIMIT ?",
    )
    .bind(months as i64)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<MonthTotal> = rows
        .into_iter()
        .map(|(period, income_minor, expense_minor)| MonthTotal { period, income_minor, expense_minor })
        .collect();
    out.reverse();
    Ok(out)
}

/// Total spending in a date range. Expense only.
pub async fn spent_between(pool: &SqlitePool, from: &str, to: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(base_minor), 0) FROM transactions
          WHERE kind = 'expense' AND occurred_on BETWEEN ? AND ?",
    )
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await?)
}

/// Total income in a date range.
pub async fn income_between(pool: &SqlitePool, from: &str, to: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COALESCE(SUM(base_minor), 0) FROM transactions
          WHERE kind = 'income' AND occurred_on BETWEEN ? AND ?",
    )
    .bind(from)
    .bind(to)
    .fetch_one(pool)
    .await?)
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

    async fn account(p: &SqlitePool, name: &str) -> i64 {
        sqlx::query("INSERT INTO accounts (name, kind, created_at) VALUES (?, 'bank', 0)")
            .bind(name)
            .execute(p)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    #[test]
    fn merchant_names_survive_the_reference_number() {
        assert_eq!(normalise_merchant("NETFLIX 4471"), "NETFLIX");
        assert_eq!(normalise_merchant("netflix.com/bill 5120"), "NETFLIX COM BILL");
        assert_eq!(normalise_merchant("UPI/DR/9928/Swiggy"), "UPI DR SWIGGY");
        assert_eq!(normalise_merchant("  Big  Bazaar  "), "BIG BAZAAR");
        // All-digits is all the identity there is, so it is kept.
        assert_eq!(normalise_merchant("998812"), "998812");
    }

    #[tokio::test]
    async fn transfers_are_not_spending() {
        // Honesty rule 1, with a test of its own as the spec requires.
        let p = pool().await;
        let bank = account(&p, "HDFC").await;
        let wallet = account(&p, "Paytm").await;

        post(&p, &NewTxn::expense(bank, 40_000, "INR", "2026-07-10", "Groceries")).await.unwrap();
        post(&p, &NewTxn::transfer(bank, wallet, 500_000, "INR", "2026-07-11", "Top-up"))
            .await
            .unwrap();

        assert_eq!(
            spent_between(&p, "2026-07-01", "2026-07-31").await.unwrap(),
            40_000,
            "a ₹5,000 wallet top-up is not ₹5,000 of spending"
        );
        // But it is still in the ledger, visibly.
        let all = page(&p, &TxnFilter::default(), TxnSort::Date, true, 0).await.unwrap();
        assert_eq!(all.total, 2);
    }

    #[tokio::test]
    async fn a_transfer_nets_to_zero_across_the_pair() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let b = account(&p, "B").await;
        post(&p, &NewTxn::transfer(a, b, 100_000, "INR", "2026-07-11", "Move")).await.unwrap();

        let out = sql_signed_sum(&p, a).await;
        let inn = sql_signed_sum(&p, b).await;
        assert_eq!(out, -100_000);
        assert_eq!(inn, 100_000);
        assert_eq!(out + inn, 0, "money moved between two owned accounts is not spending");
    }

    /// `SIGNED` summed over one account, which is what `accounts::balance` does.
    async fn sql_signed_sum(p: &SqlitePool, account_id: i64) -> i64 {
        sqlx::query_scalar(&format!(
            "SELECT COALESCE(SUM({SIGNED}), 0) FROM transactions
              WHERE account_id = ? OR to_account_id = ?"
        ))
        .bind(account_id)
        .bind(account_id)
        .bind(account_id)
        .fetch_one(p)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_two_signed_forms_agree() {
        // `SIGNED` (SQL) computes balances; `signed_delta` (Rust) computes the
        // running-balance column. They are the same rule written twice, so a
        // change to one that misses the other has to fail here.
        let p = pool().await;
        let a = account(&p, "A").await;
        let b = account(&p, "B").await;

        post(&p, &NewTxn::expense(a, 40_000, "INR", "2026-07-01", "Spend")).await.unwrap();
        let mut inc = NewTxn::expense(a, 500_000, "INR", "2026-07-02", "Salary");
        inc.kind = TxnKind::Income;
        post(&p, &inc).await.unwrap();
        post(&p, &NewTxn::transfer(a, b, 70_000, "INR", "2026-07-03", "Out")).await.unwrap();
        post(&p, &NewTxn::transfer(b, a, 20_000, "INR", "2026-07-04", "Back")).await.unwrap();

        for acct in [a, b] {
            let all = page(
                &p,
                &TxnFilter { account_id: Some(acct), ..Default::default() },
                TxnSort::Date,
                false,
                0,
            )
            .await
            .unwrap();
            let in_rust: i64 = all.rows.iter().map(|r| signed_delta(r, acct)).sum();
            assert_eq!(in_rust, sql_signed_sum(&p, acct).await, "account {acct}");
            // And the last running figure must be that same sum, since opening
            // is zero here.
            assert_eq!(all.rows.last().unwrap().running_minor, Some(in_rust));
        }
    }

    #[tokio::test]
    async fn income_by_day_groups_credits_and_ignores_everything_else() {
        // The calendar's other direction. Expenses and transfers must not appear:
        // a day showing "+₹70,000" because money moved between two of your own
        // accounts would be a lie about payday.
        let p = pool().await;
        let a = account(&p, "A").await;
        let b = account(&p, "B").await;

        let mut one = NewTxn::expense(a, 500_000, "INR", "2026-07-02", "Salary");
        one.kind = TxnKind::Income;
        post(&p, &one).await.unwrap();
        let mut two = NewTxn::expense(a, 120_000, "INR", "2026-07-02", "Freelance");
        two.kind = TxnKind::Income;
        post(&p, &two).await.unwrap();
        post(&p, &NewTxn::expense(a, 40_000, "INR", "2026-07-02", "Groceries")).await.unwrap();
        post(&p, &NewTxn::transfer(a, b, 70_000, "INR", "2026-07-02", "Top-up")).await.unwrap();
        // Outside the window.
        let mut later = NewTxn::expense(a, 900_000, "INR", "2026-08-01", "Next month");
        later.kind = TxnKind::Income;
        post(&p, &later).await.unwrap();

        let days = income_by_day(&p, "2026-07-01", "2026-07-31").await.unwrap();
        assert_eq!(days, [("2026-07-02".to_string(), 620_000)]);
    }

    #[tokio::test]
    async fn a_foreign_amount_is_converted_once_at_post_time() {
        let p = pool().await;
        let card = account(&p, "ICICI Card").await;
        let mut t = NewTxn::expense(card, 999, "USD", "2026-07-12", "Some SaaS");
        t.rate_micro = 83_600_000;
        let id = post(&p, &t).await.unwrap();

        let (amount, base, rate): (i64, i64, i64) =
            sqlx::query_as("SELECT amount_minor, base_minor, rate_micro FROM transactions WHERE id = ?")
                .bind(id)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(amount, 999, "the original amount is what the statement says");
        assert_eq!(rate, 83_600_000);
        assert_eq!(base, money::convert(999, 83_600_000));
    }

    #[tokio::test]
    async fn re_posting_an_identical_row_returns_the_first_id() {
        let p = pool().await;
        let bank = account(&p, "HDFC").await;
        let t = NewTxn::expense(bank, 64_900, "INR", "2026-07-26", "Netflix 4471");
        let first = post(&p, &t).await.unwrap();
        // Different reference number, same normalised merchant: the same charge.
        let again = post(&p, &NewTxn::expense(bank, 64_900, "INR", "2026-07-26", "Netflix 5120"))
            .await
            .unwrap();
        assert_eq!(first, again, "a re-import must add zero rows");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn negative_amounts_and_broken_transfers_are_refused() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let mut bad = NewTxn::expense(a, -100, "INR", "2026-07-01", "x");
        assert!(post(&p, &bad).await.is_err());

        bad = NewTxn::expense(a, 100, "INR", "2026-07-01", "x");
        bad.kind = TxnKind::Transfer;
        assert!(post(&p, &bad).await.is_err(), "a transfer needs a destination");

        bad.to_account_id = Some(a);
        assert!(post(&p, &bad).await.is_err(), "to itself moves nothing");
    }

    #[tokio::test]
    async fn paging_is_stable_and_reports_its_totals() {
        let p = pool().await;
        let bank = account(&p, "HDFC").await;
        for i in 1..=30 {
            post(
                &p,
                &NewTxn::expense(bank, i * 100, "INR", &format!("2026-07-{i:02}"), &format!("Buy {i}")),
            )
            .await
            .unwrap();
        }
        let first = page(&p, &TxnFilter::default(), TxnSort::Date, true, 0).await.unwrap();
        assert_eq!(first.total, 30);
        assert_eq!(first.pages, 2);
        assert_eq!(first.rows.len(), 25);
        assert_eq!(first.rows[0].description, "Buy 30", "newest first");

        let second = page(&p, &TxnFilter::default(), TxnSort::Date, true, 1).await.unwrap();
        assert_eq!(second.rows.len(), 5);
        assert_eq!(second.rows[0].description, "Buy 5");

        // Off the end clamps to the last page rather than returning nothing.
        let far = page(&p, &TxnFilter::default(), TxnSort::Date, true, 99).await.unwrap();
        assert_eq!(far.page, 1);
    }

    #[tokio::test]
    async fn sorting_by_amount_uses_the_base_figure() {
        let p = pool().await;
        let bank = account(&p, "HDFC").await;
        post(&p, &NewTxn::expense(bank, 100, "INR", "2026-07-01", "Small")).await.unwrap();
        post(&p, &NewTxn::expense(bank, 900_000, "INR", "2026-07-02", "Large")).await.unwrap();
        let by_amount = page(&p, &TxnFilter::default(), TxnSort::Amount, true, 0).await.unwrap();
        assert_eq!(by_amount.rows[0].description, "Large");
    }

    #[tokio::test]
    async fn a_running_balance_appears_only_for_one_account_sorted_by_date() {
        let p = pool().await;
        sqlx::query("UPDATE accounts SET opening_minor = 1000000 WHERE id = ?")
            .bind(account(&p, "HDFC").await)
            .execute(&p)
            .await
            .unwrap();
        let bank: i64 =
            sqlx::query_scalar("SELECT id FROM accounts WHERE name = 'HDFC'").fetch_one(&p).await.unwrap();

        post(&p, &NewTxn::expense(bank, 40_000, "INR", "2026-07-10", "A")).await.unwrap();
        post(&p, &NewTxn::expense(bank, 10_000, "INR", "2026-07-11", "B")).await.unwrap();

        let one = page(
            &p,
            &TxnFilter { account_id: Some(bank), ..Default::default() },
            TxnSort::Date,
            false,
            0,
        )
        .await
        .unwrap();
        assert_eq!(one.rows[0].running_minor, Some(1_000_000 - 40_000));
        assert_eq!(one.rows[1].running_minor, Some(1_000_000 - 50_000));

        let all = page(&p, &TxnFilter::default(), TxnSort::Date, false, 0).await.unwrap();
        assert!(all.rows[0].running_minor.is_none(), "meaningless across accounts");

        let by_desc = page(
            &p,
            &TxnFilter { account_id: Some(bank), ..Default::default() },
            TxnSort::Description,
            false,
            0,
        )
        .await
        .unwrap();
        assert!(by_desc.rows[0].running_minor.is_none(), "meaningless out of date order");
    }

    #[tokio::test]
    async fn filtering_by_account_shows_both_legs_of_a_transfer() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let b = account(&p, "B").await;
        post(&p, &NewTxn::transfer(a, b, 100_000, "INR", "2026-07-11", "Move")).await.unwrap();
        for acct in [a, b] {
            let f = TxnFilter { account_id: Some(acct), ..Default::default() };
            let got = page(&p, &f, TxnSort::Date, true, 0).await.unwrap();
            assert_eq!(got.total, 1, "the destination account is part of the transfer too");
        }
    }

    #[tokio::test]
    async fn the_category_donut_excludes_transfers() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let b = account(&p, "B").await;
        let food: i64 = sqlx::query_scalar("SELECT id FROM categories WHERE name = 'Groceries'")
            .fetch_one(&p)
            .await
            .unwrap();
        let xfer: i64 = sqlx::query_scalar("SELECT id FROM categories WHERE name = 'Transfer'")
            .fetch_one(&p)
            .await
            .unwrap();

        let mut e = NewTxn::expense(a, 40_000, "INR", "2026-07-10", "Veg");
        e.category_id = Some(food);
        post(&p, &e).await.unwrap();

        let mut t = NewTxn::transfer(a, b, 900_000, "INR", "2026-07-11", "Card payment");
        t.category_id = Some(xfer);
        post(&p, &t).await.unwrap();

        let slices = spend_by_category(&p, "2026-07-01", "2026-07-31").await.unwrap();
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].name, "Groceries");
    }

    #[tokio::test]
    async fn monthly_totals_are_oldest_first_and_split_by_direction() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let mut income = NewTxn::expense(a, 5_000_000, "INR", "2026-06-01", "Salary");
        income.kind = TxnKind::Income;
        post(&p, &income).await.unwrap();
        post(&p, &NewTxn::expense(a, 40_000, "INR", "2026-07-10", "Veg")).await.unwrap();

        let months = monthly_totals(&p, 12).await.unwrap();
        assert_eq!(months.len(), 2);
        assert_eq!(months[0].period, "2026-06");
        assert_eq!(months[0].income_minor, 5_000_000);
        assert_eq!(months[1].period, "2026-07");
        assert_eq!(months[1].expense_minor, 40_000);
    }

    #[tokio::test]
    async fn deleting_a_posting_frees_the_obligation_it_paid() {
        let p = pool().await;
        let a = account(&p, "A").await;
        let id = post(&p, &NewTxn::expense(a, 40_000, "INR", "2026-07-10", "Power bill")).await.unwrap();
        sqlx::query(
            "INSERT INTO obligations (name, due_on, status, transaction_id, actual_minor)
             VALUES ('Power', '2026-07-10', 'paid', ?, 40000)",
        )
        .bind(id)
        .execute(&p)
        .await
        .unwrap();

        delete(&p, id).await.unwrap();
        let (status, txn): (String, Option<i64>) =
            sqlx::query_as("SELECT status, transaction_id FROM obligations")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(status, "upcoming", "the bill is unpaid again, not paid-with-no-payment");
        assert!(txn.is_none());
    }
}
