//! `finances.db` — schema and the defaults every install starts with.
//!
//! Two tables carry the model. `transactions` is every posting, whatever
//! produced it; `recurrences` is every repeating thing, with a `kind` telling a
//! subscription from a bill. Subscriptions and Bills are filtered views over
//! one table, not two tables, so a change is visible everywhere at once because
//! there is nowhere else for it to be.
//!
//! Every monetary column is named `*_minor` and holds an `i64` count of minor
//! units. That naming is deliberate: it makes a float or a major-unit value
//! reaching the database a one-line grep to find.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SCHEMA: &str = r#"
-- Where money sits. Loans are accounts with a negative balance, which is what
-- lets the Accounts tab total liquid and debt from one place.
CREATE TABLE IF NOT EXISTS accounts (
    id                 INTEGER PRIMARY KEY,
    name               TEXT    NOT NULL,
    kind               TEXT    NOT NULL,            -- bank|cash|card|wallet|loan|virtual
    currency           TEXT    NOT NULL DEFAULT 'INR',
    opening_minor      INTEGER NOT NULL DEFAULT 0,
    credit_limit_minor INTEGER,                      -- cards only
    statement_day      INTEGER,                      -- cards only, 1-31
    closed             INTEGER NOT NULL DEFAULT 0,
    sort_order         INTEGER NOT NULL DEFAULT 0,
    created_at         INTEGER NOT NULL
);

-- Loan detail. One row per account of kind='loan'.
CREATE TABLE IF NOT EXISTS loans (
    account_id      INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    principal_minor INTEGER NOT NULL,
    rate_bp         INTEGER NOT NULL,               -- basis points; 8.4% = 840
    tenure_months   INTEGER NOT NULL,
    started_on      TEXT    NOT NULL,               -- ISO date
    emi_minor       INTEGER NOT NULL,
    emi_day         INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS categories (
    id        INTEGER PRIMARY KEY,
    name      TEXT    NOT NULL,
    parent_id INTEGER REFERENCES categories(id),
    kind      TEXT    NOT NULL,                     -- expense|income|transfer
    color     TEXT
);
-- Uniqueness has to go through COALESCE. A plain UNIQUE(name, parent_id) never
-- fires for top-level categories, because in SQL two NULLs are not equal, so
-- every re-seed would insert another "Groceries" with a NULL parent.
CREATE UNIQUE INDEX IF NOT EXISTS ux_cat_name
    ON categories(name, COALESCE(parent_id, 0));

-- The ledger. Every posting, whatever produced it.
CREATE TABLE IF NOT EXISTS transactions (
    id             INTEGER PRIMARY KEY,
    account_id     INTEGER NOT NULL REFERENCES accounts(id),
    to_account_id  INTEGER REFERENCES accounts(id),  -- transfers only
    kind           TEXT    NOT NULL,                 -- expense|income|transfer
    amount_minor   INTEGER NOT NULL,                 -- always positive; kind carries direction
    currency       TEXT    NOT NULL,
    rate_micro     INTEGER NOT NULL,                 -- rate used at post time
    base_minor     INTEGER NOT NULL,                 -- derived, denormalised for fast SUM
    occurred_on    TEXT    NOT NULL,                 -- ISO date
    description    TEXT    NOT NULL,
    merchant_norm  TEXT    NOT NULL,                 -- upper, punctuation-stripped; dedup + detection
    category_id    INTEGER REFERENCES categories(id),
    recurrence_id  INTEGER REFERENCES recurrences(id),
    obligation_id  INTEGER REFERENCES obligations(id),
    due_id         INTEGER REFERENCES dues(id),
    source         TEXT    NOT NULL,                 -- manual|csv|ofx|ocr|recurrence
    note           TEXT,
    created_at     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_txn_date    ON transactions(occurred_on DESC);
CREATE INDEX IF NOT EXISTS ix_txn_account ON transactions(account_id, occurred_on DESC);
CREATE INDEX IF NOT EXISTS ix_txn_cat     ON transactions(category_id, occurred_on DESC);
-- Re-importing an overlapping statement must add nothing. This index is what
-- makes that true at the storage layer rather than in importer logic.
CREATE UNIQUE INDEX IF NOT EXISTS ux_txn_dedup
    ON transactions(account_id, occurred_on, amount_minor, merchant_norm);

-- Templates. Subscriptions and bills are two kinds of one thing.
CREATE TABLE IF NOT EXISTS recurrences (
    id            INTEGER PRIMARY KEY,
    kind          TEXT    NOT NULL,                 -- subscription|bill
    name          TEXT    NOT NULL,
    amount_minor  INTEGER,                          -- fixed amount; NULL for variable bills
    currency      TEXT    NOT NULL DEFAULT 'INR',
    cycle         TEXT    NOT NULL,                 -- weekly|monthly|quarterly|yearly|irregular
    anchor_day    INTEGER,                          -- day-of-month, 1-31
    next_due_on   TEXT,
    account_id    INTEGER REFERENCES accounts(id),
    category_id   INTEGER REFERENCES categories(id),
    auto_post     INTEGER NOT NULL DEFAULT 0,
    status        TEXT    NOT NULL DEFAULT 'active',  -- active|paused|cancelled
    reminder_days INTEGER NOT NULL DEFAULT 3,
    note          TEXT,
    created_at    INTEGER NOT NULL
);

-- Price history, for the hike flag.
CREATE TABLE IF NOT EXISTS recurrence_prices (
    id             INTEGER PRIMARY KEY,
    recurrence_id  INTEGER NOT NULL REFERENCES recurrences(id) ON DELETE CASCADE,
    amount_minor   INTEGER NOT NULL,
    currency       TEXT    NOT NULL,
    effective_from TEXT    NOT NULL
);

-- Materialised instances: a dated, not-yet-money obligation. One-off bills live
-- here too, with recurrence_id NULL.
CREATE TABLE IF NOT EXISTS obligations (
    id             INTEGER PRIMARY KEY,
    recurrence_id  INTEGER REFERENCES recurrences(id) ON DELETE CASCADE,
    name           TEXT    NOT NULL,
    due_on         TEXT    NOT NULL,
    estimate_minor INTEGER,
    actual_minor   INTEGER,
    status         TEXT    NOT NULL,                -- upcoming|due|overdue|paid|skipped
    transaction_id INTEGER REFERENCES transactions(id),
    UNIQUE(recurrence_id, due_on)
);
CREATE INDEX IF NOT EXISTS ix_obl_due ON obligations(due_on, status);

-- Money between you and named people. Free text, no profiles, no contacting
-- anyone — the app remembers, that is all.
CREATE TABLE IF NOT EXISTS dues (
    id            INTEGER PRIMARY KEY,
    person        TEXT    NOT NULL,
    direction     TEXT    NOT NULL,                 -- owed_to_me|i_owe
    amount_minor  INTEGER NOT NULL,
    currency      TEXT    NOT NULL DEFAULT 'INR',
    opened_on     TEXT    NOT NULL,
    note          TEXT,
    status        TEXT    NOT NULL DEFAULT 'open',  -- open|settled|written_off
    settled_on    TEXT,
    settle_txn_id INTEGER REFERENCES transactions(id)
);

CREATE TABLE IF NOT EXISTS budgets (
    id           INTEGER PRIMARY KEY,
    category_id  INTEGER NOT NULL REFERENCES categories(id),
    period       TEXT    NOT NULL,                  -- YYYY-MM
    amount_minor INTEGER NOT NULL,
    rollover     INTEGER NOT NULL DEFAULT 0,
    UNIQUE(category_id, period)
);

CREATE TABLE IF NOT EXISTS fx_rates (
    code       TEXT PRIMARY KEY,                    -- 'USD'
    rate_micro INTEGER NOT NULL,                    -- 83.60 -> 83_600_000
    edited_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS import_presets (
    id          INTEGER PRIMARY KEY,
    bank_name   TEXT    NOT NULL UNIQUE,
    column_map  TEXT    NOT NULL,                   -- JSON
    date_format TEXT    NOT NULL,
    account_id  INTEGER REFERENCES accounts(id),
    created_at  INTEGER NOT NULL
);
"#;

/// The two holding accounts that keep lending out of spend totals.
///
/// Lending ₹5,000 moves cash today but is not expenditure, so it posts as a
/// transfer into `LENT_OUT` and comes back when the due settles. Same in
/// reverse for money borrowed. Without these, every loan to a friend would show
/// up as a month with unusually high spending.
pub const LENT_OUT: &str = "Lent out";
pub const BORROWED: &str = "Borrowed";

/// Categories every install starts with. `parent` is a name in this same list.
///
/// Kept short on purpose. A long default tree is a tree the user has to prune,
/// and an unused category is worse than a missing one because it dilutes every
/// donut and every budget list.
const SEED_CATEGORIES: &[(&str, &str, Option<&str>)] = &[
    // (name, kind, parent)
    ("Food & Dining", "expense", None),
    ("Groceries", "expense", Some("Food & Dining")),
    ("Eating out", "expense", Some("Food & Dining")),
    ("Transport", "expense", None),
    ("Fuel", "expense", Some("Transport")),
    ("Housing", "expense", None),
    ("Rent", "expense", Some("Housing")),
    ("Utilities", "expense", Some("Housing")),
    ("Health", "expense", None),
    ("Insurance", "expense", None),
    ("Shopping", "expense", None),
    ("Entertainment", "expense", None),
    ("Subscriptions", "expense", Some("Entertainment")),
    ("Education", "expense", None),
    ("Travel", "expense", None),
    ("Fees & Charges", "expense", None),
    ("Gifts & Donations", "expense", None),
    ("Uncategorised", "expense", None),
    ("Salary", "income", None),
    ("Freelance", "income", None),
    ("Interest", "income", None),
    ("Refunds", "income", None),
    // Transfers are not spending. Every total that says "spent" filters these
    // out by kind, so they need to be a kind rather than a naming convention.
    ("Transfer", "transfer", None),
    ("Card payment", "transfer", Some("Transfer")),
    ("Lending", "transfer", Some("Transfer")),
];

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Wall-clock seconds, for the `created_at` columns.
///
/// These are audit stamps — when a row was entered — and are deliberately not
/// what any date logic reads. Anything that reasons about *when money moved*
/// uses `occurred_on`, which the user controls and which a back-dated entry sets
/// correctly. See [`crate::date`].
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Insert the default categories and the two virtual accounts.
///
/// Safe to call on every open. Categories lean on `ux_cat_name`, and the
/// virtual accounts on an existence check, because `accounts.name` is not
/// unique — two real accounts may legitimately share a name.
pub async fn seed_defaults(pool: &SqlitePool) -> Result<()> {
    let created = unix_now();

    for name in [LENT_OUT, BORROWED] {
        sqlx::query(
            "INSERT INTO accounts (name, kind, currency, created_at)
             SELECT ?, 'virtual', 'INR', ?
             WHERE NOT EXISTS (SELECT 1 FROM accounts WHERE name = ? AND kind = 'virtual')",
        )
        .bind(name)
        .bind(created)
        .bind(name)
        .execute(pool)
        .await?;
    }

    // Parents first, so a child's parent lookup always resolves. The seed list
    // is ordered that way and this relies on it.
    for (name, kind, parent) in SEED_CATEGORIES {
        let parent_id: Option<i64> = match parent {
            Some(p) => {
                sqlx::query_scalar("SELECT id FROM categories WHERE name = ? AND parent_id IS NULL")
                    .bind(p)
                    .fetch_optional(pool)
                    .await?
            }
            None => None,
        };
        sqlx::query(
            "INSERT OR IGNORE INTO categories (name, parent_id, kind) VALUES (?, ?, ?)",
        )
        .bind(name)
        .bind(parent_id)
        .bind(kind)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        apply_schema(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn applying_the_schema_twice_is_a_no_op() {
        let pool = mem_pool().await;
        apply_schema(&pool).await.unwrap();
        let tables: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(tables, 11);
    }

    #[tokio::test]
    async fn seeding_twice_does_not_duplicate_anything() {
        let pool = mem_pool().await;
        seed_defaults(&pool).await.unwrap();
        let after_first: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM categories").fetch_one(&pool).await.unwrap();
        seed_defaults(&pool).await.unwrap();
        let after_second: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM categories").fetch_one(&pool).await.unwrap();

        assert_eq!(after_first, SEED_CATEGORIES.len() as i64);
        assert_eq!(after_second, after_first, "a NULL parent must still collide");

        let virtuals: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE kind = 'virtual'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(virtuals, 2);
    }

    #[tokio::test]
    async fn the_two_holding_accounts_exist_so_lending_is_not_spending() {
        let pool = mem_pool().await;
        seed_defaults(&pool).await.unwrap();
        for name in [LENT_OUT, BORROWED] {
            let found: Option<i64> =
                sqlx::query_scalar("SELECT id FROM accounts WHERE name = ? AND kind = 'virtual'")
                    .bind(name)
                    .fetch_optional(&pool)
                    .await
                    .unwrap();
            assert!(found.is_some(), "{name} must be seeded");
        }
    }

    #[tokio::test]
    async fn child_categories_are_hung_off_their_parent() {
        let pool = mem_pool().await;
        seed_defaults(&pool).await.unwrap();
        let parent: Option<String> = sqlx::query_scalar(
            "SELECT p.name FROM categories c JOIN categories p ON p.id = c.parent_id
             WHERE c.name = 'Groceries'",
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(parent.as_deref(), Some("Food & Dining"));
    }

    #[tokio::test]
    async fn transfers_are_a_category_kind_not_a_naming_convention() {
        let pool = mem_pool().await;
        seed_defaults(&pool).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM categories WHERE kind = 'transfer'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 3, "Transfer, Card payment, Lending");
    }

    #[tokio::test]
    async fn re_importing_the_same_row_is_refused_by_the_index() {
        let pool = mem_pool().await;
        seed_defaults(&pool).await.unwrap();
        let acct: i64 = sqlx::query_scalar("SELECT id FROM accounts LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let insert = |n: &'static str| {
            sqlx::query(
                "INSERT INTO transactions
                   (account_id, kind, amount_minor, currency, rate_micro, base_minor,
                    occurred_on, description, merchant_norm, source, created_at)
                 VALUES (?, 'expense', 64900, 'INR', 1000000, 64900,
                         '2026-07-26', 'Netflix', ?, 'csv', 0)",
            )
            .bind(acct)
            .bind(n)
        };
        insert("NETFLIX").execute(&pool).await.unwrap();
        assert!(
            insert("NETFLIX").execute(&pool).await.is_err(),
            "the dedup index is what makes a re-import add zero rows"
        );
        // A different merchant on the same day and amount is a different thing.
        insert("SPOTIFY").execute(&pool).await.unwrap();
    }
}
