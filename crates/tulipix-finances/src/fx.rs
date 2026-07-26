//! Exchange rates, hand-edited.
//!
//! There is no rate API and there will not be one — the section is offline by
//! decree, not by phase. The user types a rate, it is stored as millionths, and
//! every transaction records the rate that was in force when it was posted.
//!
//! That last part is the important one. A transaction's `base_minor` is computed
//! once, at post time, and never recomputed. If rates were applied at read time,
//! editing the USD rate today would silently rewrite what last March cost, and
//! two runs of the same report would disagree.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::money::RATE_ONE;

/// Settings key for the base currency. Everything is reported in it.
pub const BASE_KEY: &str = "finances.base_currency";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rate {
    pub code: String,
    pub rate_micro: i64,
    pub edited_at: i64,
}

/// The base currency, from advanced settings. INR unless the user says otherwise.
pub fn base_currency() -> String {
    tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(BASE_KEY).cloned())
        .map(|s| s.trim().to_uppercase())
        .filter(|s| s.len() == 3)
        .unwrap_or_else(|| "INR".to_string())
}

/// Change the base currency.
///
/// Only the setting moves. Stored amounts are untouched: every transaction keeps
/// the currency and the rate it was posted at, and `base_minor` is re-derived
/// from those. Rewriting history to match a new base would destroy the one
/// record of what the money actually was.
pub fn set_base_currency(code: &str) -> Result<()> {
    let code = code.trim().to_uppercase();
    if code.len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        anyhow::bail!("a currency code is three letters, e.g. INR");
    }
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert(BASE_KEY.to_string(), code);
    s.save()
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Rate>> {
    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT code, rate_micro, edited_at FROM fx_rates ORDER BY code",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(code, rate_micro, edited_at)| Rate { code, rate_micro, edited_at }).collect())
}

pub async fn set(pool: &SqlitePool, code: &str, rate_micro: i64, at: i64) -> Result<()> {
    let code = code.trim().to_uppercase();
    sqlx::query(
        "INSERT INTO fx_rates (code, rate_micro, edited_at) VALUES (?, ?, ?)
         ON CONFLICT(code) DO UPDATE SET rate_micro = excluded.rate_micro,
                                         edited_at  = excluded.edited_at",
    )
    .bind(&code)
    .bind(rate_micro)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, code: &str) -> Result<()> {
    sqlx::query("DELETE FROM fx_rates WHERE code = ?").bind(code).execute(pool).await?;
    Ok(())
}

/// The rate to use when posting an amount in `code`.
///
/// The base currency is always exactly 1, never a stored row — otherwise a
/// fat-fingered INR row would rescale every rupee in the database.
pub async fn rate_for(pool: &SqlitePool, code: &str, base: &str) -> Result<i64> {
    if code.eq_ignore_ascii_case(base) {
        return Ok(RATE_ONE);
    }
    let found: Option<i64> = sqlx::query_scalar("SELECT rate_micro FROM fx_rates WHERE code = ?")
        .bind(code.to_uppercase())
        .fetch_optional(pool)
        .await?;
    // An unknown currency posts at 1:1 rather than failing. The alternative is
    // refusing to record a transaction the user actually made, which loses data
    // to protect a report; the Insights flag for a missing rate is the right
    // place to complain.
    Ok(found.unwrap_or(RATE_ONE))
}

/// Currencies used by transactions or recurrences that have no rate row.
///
/// Feeds an Insights flag: those rows are being counted at 1:1, so every total
/// containing them is wrong, and the user is the only one who can fix it.
pub async fn missing(pool: &SqlitePool, base: &str) -> Result<Vec<String>> {
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT currency FROM (
             SELECT currency FROM transactions
             UNION SELECT currency FROM recurrences
         )
         WHERE currency <> ?
           AND currency NOT IN (SELECT code FROM fx_rates)
         ORDER BY currency",
    )
    .bind(base.to_uppercase())
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
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
    async fn the_base_currency_is_always_exactly_one() {
        let p = pool().await;
        // Even with a nonsense stored row, base must not be rescaled.
        set(&p, "INR", 5_000_000, 0).await.unwrap();
        assert_eq!(rate_for(&p, "INR", "INR").await.unwrap(), RATE_ONE);
        assert_eq!(rate_for(&p, "inr", "INR").await.unwrap(), RATE_ONE);
    }

    #[tokio::test]
    async fn rates_round_trip_and_upsert() {
        let p = pool().await;
        set(&p, "usd", 83_600_000, 10).await.unwrap();
        assert_eq!(rate_for(&p, "USD", "INR").await.unwrap(), 83_600_000);
        set(&p, "USD", 84_100_000, 20).await.unwrap();
        assert_eq!(list(&p).await.unwrap().len(), 1, "upsert, not a second row");
        assert_eq!(rate_for(&p, "USD", "INR").await.unwrap(), 84_100_000);
    }

    #[tokio::test]
    async fn an_unknown_currency_posts_at_one_rather_than_refusing() {
        let p = pool().await;
        assert_eq!(rate_for(&p, "ZWL", "INR").await.unwrap(), RATE_ONE);
    }

    #[tokio::test]
    async fn missing_rates_are_reportable() {
        let p = pool().await;
        sqlx::query(
            "INSERT INTO recurrences (kind, name, currency, cycle, created_at)
             VALUES ('subscription', 'Some SaaS', 'USD', 'monthly', 0),
                    ('subscription', 'Andere',    'EUR', 'monthly', 0),
                    ('subscription', 'Netflix',   'INR', 'monthly', 0)",
        )
        .execute(&p)
        .await
        .unwrap();
        set(&p, "USD", 83_600_000, 0).await.unwrap();
        assert_eq!(missing(&p, "INR").await.unwrap(), ["EUR"]);
    }
}
