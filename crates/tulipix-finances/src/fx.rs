//! Exchange rates: fetched daily, overridable by hand.
//!
//! A rate is stored as millionths, and every transaction records the rate that was
//! in force when it was posted.
//!
//! That last part is the important one, and it does not change now that rates
//! arrive from the network. A transaction's `base_minor` is computed once, at post
//! time, and never recomputed. If rates were applied at read time, today's refresh
//! would silently rewrite what last March cost, and two runs of the same report
//! would disagree.
//!
//! # Two things the daily refresh is not allowed to do
//!
//! **Overwrite a rate the user typed.** A hand-edited rate is a decision — often a
//! deliberately different one, like the rate a card actually charged. [`apply_live`]
//! skips every row whose `source` is `manual`.
//!
//! **Reach the network from this crate.** Everything here is pure or SQL: the URL
//! is built by [`endpoint`], the response is turned into rates by [`parse_rates`],
//! and the HTTP between them happens in the glue crate. That keeps this crate
//! testable with no display, no network and no fixtures beyond a JSON string, which
//! is the whole reason for the core/glue split.

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::money::RATE_ONE;

/// Settings key for the base currency. Everything is reported in it.
pub const BASE_KEY: &str = "finances.base_currency";

/// Settings key for the daily refresh. On unless turned off.
pub const AUTO_KEY: &str = "finances.fx_auto";

/// How old a fetched rate gets before it is refetched. Once a day: published
/// rates move on a daily cycle, and a desktop app polling faster than its source
/// updates is just noise on someone's connection.
pub const REFRESH_SECS: i64 = 86_400;

/// Keyless, no attribution header, base-relative. `{}` is the base currency.
const ENDPOINT: &str = "https://open.er-api.com/v6/latest/{}";

/// The currencies the app keeps a rate for whether or not anything is priced in
/// them yet — the ten most traded, plus the app's own default base.
///
/// Before this, a rate existed only once money had already been posted in that
/// currency, which made the rates panel a form for typing numbers into rather
/// than something you could read. Ten rows fetched daily is one request either
/// way, and it means a foreign amount converts correctly the *first* time it is
/// entered instead of silently posting at 1:1 until someone notices.
///
/// Every rate is stored against the base, so any pair converts through it:
/// `A → B` is `micro(A) / micro(B)`. That is why ten rows are enough for ninety
/// pairs, and why nothing here needs a cross-rate table.
pub const TOP_CURRENCIES: &[(&str, &str)] = &[
    ("USD", "US dollar"),
    ("EUR", "Euro"),
    ("JPY", "Japanese yen"),
    ("GBP", "Pound sterling"),
    ("CNY", "Chinese yuan"),
    ("AUD", "Australian dollar"),
    ("CAD", "Canadian dollar"),
    ("CHF", "Swiss franc"),
    ("INR", "Indian rupee"),
    ("SGD", "Singapore dollar"),
];

/// The English name of a currency, for the rates panel. The code itself when it
/// is not one of the ten — a rate the user added by hand is still legible.
pub fn currency_name(code: &str) -> String {
    let code = code.trim().to_uppercase();
    TOP_CURRENCIES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| name.to_string())
        .unwrap_or(code)
}

/// Where to fetch rates relative to `base`.
pub fn endpoint(base: &str) -> String {
    ENDPOINT.replace("{}", &base.trim().to_uppercase())
}

/// Whether the daily refresh is wanted. On unless explicitly turned off.
pub fn auto_enabled() -> bool {
    !matches!(
        tulipix_core::settings::Settings::load()
            .ok()
            .and_then(|s| s.advanced.get(AUTO_KEY).cloned())
            .as_deref(),
        Some("false") | Some("0")
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rate {
    pub code: String,
    pub rate_micro: i64,
    pub edited_at: i64,
    /// `false` when the user typed this rate, which means the daily refresh will
    /// leave it alone.
    pub live: bool,
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
    let rows = sqlx::query_as::<_, (String, i64, i64, String)>(
        "SELECT code, rate_micro, edited_at, source FROM fx_rates ORDER BY code",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(code, rate_micro, edited_at, source)| Rate {
            code,
            rate_micro,
            edited_at,
            live: source == "live",
        })
        .collect())
}

/// One line of the rates monitor: a currency the app watches, and the rate it
/// has for it — if it has one yet.
#[derive(Clone, Debug)]
pub struct Monitored {
    pub code: String,
    pub name: String,
    /// `None` before the first successful fetch. Shown as such rather than as a
    /// 1:1 placeholder, which would read as a real rate.
    pub rate_micro: Option<i64>,
    pub edited_at: i64,
    pub live: bool,
}

/// Every currency the app watches, in trading order, with whatever rate it holds.
///
/// The list is [`needed_codes`], so it is the ten majors plus anything the user
/// actually uses — which means the panel is a monitor to read rather than a form
/// to type into, and a currency shows up before the first transaction in it does.
pub async fn monitor(pool: &SqlitePool, base: &str) -> Result<Vec<Monitored>> {
    let held = list(pool).await?;
    let wanted = needed_codes(pool, base).await?;
    // The majors first and in their own order — a monitor sorted alphabetically
    // buries the dollar under the dirham. Everything else follows, sorted.
    let mut order: Vec<String> = TOP_CURRENCIES
        .iter()
        .map(|(c, _)| c.to_string())
        .filter(|c| wanted.contains(c))
        .collect();
    let mut rest: Vec<String> = wanted.into_iter().filter(|c| !order.contains(c)).collect();
    rest.sort();
    order.extend(rest);

    Ok(order
        .into_iter()
        .map(|code| {
            let found = held.iter().find(|r| r.code.eq_ignore_ascii_case(&code));
            Monitored {
                name: currency_name(&code),
                rate_micro: found.map(|r| r.rate_micro),
                edited_at: found.map(|r| r.edited_at).unwrap_or(0),
                live: found.map(|r| r.live).unwrap_or(true),
                code,
            }
        })
        .collect())
}

/// When the last fetched rate was written, for the "checked" line. `None` when
/// nothing has ever been fetched.
pub async fn last_checked(pool: &SqlitePool) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT MAX(edited_at) FROM fx_rates WHERE source = 'live'")
        .fetch_optional(pool)
        .await?
        .flatten())
}

/// Store a hand-typed rate. Marked `manual`, so the daily refresh leaves it alone.
pub async fn set(pool: &SqlitePool, code: &str, rate_micro: i64, at: i64) -> Result<()> {
    upsert(pool, code, rate_micro, at, "manual").await
}

async fn upsert(
    pool: &SqlitePool,
    code: &str,
    rate_micro: i64,
    at: i64,
    source: &str,
) -> Result<()> {
    let code = code.trim().to_uppercase();
    sqlx::query(
        "INSERT INTO fx_rates (code, rate_micro, edited_at, source) VALUES (?, ?, ?, ?)
         ON CONFLICT(code) DO UPDATE SET rate_micro = excluded.rate_micro,
                                         edited_at  = excluded.edited_at,
                                         source     = excluded.source",
    )
    .bind(&code)
    .bind(rate_micro)
    .bind(at)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(())
}

/// Turn a rates response into `(code, rate_micro)` pairs, where `rate_micro` is
/// **how much base one unit of that currency is worth** — the direction the rest of
/// this crate multiplies by.
///
/// The response is the other way round: it quotes how many foreign units one unit
/// of base buys. So every figure is inverted here, and inverted with integer
/// arithmetic: the quoted number is read to twelve decimal places and the reciprocal
/// taken in `i128`, because this is a number every converted total is multiplied by
/// and a float would put a different rounding in each of them.
pub fn parse_rates(json: &str, base: &str) -> Result<Vec<(String, i64)>> {
    let v: serde_json::Value = serde_json::from_str(json).context("rates response is not JSON")?;
    let rates = v
        .get("rates")
        .and_then(|r| r.as_object())
        .context("rates response has no `rates` object")?;
    let base = base.trim().to_uppercase();

    let mut out = Vec::with_capacity(rates.len());
    for (code, quoted) in rates {
        let code = code.trim().to_uppercase();
        // A currency code, or nothing: the response is remote input.
        if code.len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) || code == base {
            continue;
        }
        let serde_json::Value::Number(num) = quoted else { continue };
        // Through the number's own text and the crate's decimal parser rather than
        // as an f64 — same path a hand-typed rate takes.
        let Ok(pico) = crate::money::parse_scaled(&num.to_string(), 12) else { continue };
        if pico <= 0 {
            continue;
        }
        // 1 / quoted, in millionths: (1e6 * 1e12) / pico.
        let micro = (RATE_ONE as i128) * 1_000_000_000_000i128 / (pico as i128);
        if micro <= 0 || micro > i64::MAX as i128 {
            continue;
        }
        out.push((code, micro as i64));
    }
    if out.is_empty() {
        anyhow::bail!("no usable rates in the response");
    }
    out.sort();
    Ok(out)
}

/// Store fetched rates, leaving hand-typed ones alone.
///
/// Returns how many rows were written. Only currencies the user actually has —
/// [`needed_codes`] — so a response listing 160 currencies does not put 160 rows in
/// a database that uses two.
pub async fn apply_live(
    pool: &SqlitePool,
    rates: &[(String, i64)],
    at: i64,
) -> Result<u64> {
    let wanted = needed_codes(pool, &base_currency()).await?;
    let manual: Vec<String> =
        sqlx::query_scalar("SELECT code FROM fx_rates WHERE source = 'manual'")
            .fetch_all(pool)
            .await?;
    let mut n = 0;
    for (code, micro) in rates {
        if !wanted.contains(code) || manual.iter().any(|m| m == code) {
            continue;
        }
        upsert(pool, code, *micro, at, "live").await?;
        n += 1;
    }
    Ok(n)
}

/// The currencies worth having a rate for: the ten in [`TOP_CURRENCIES`], plus
/// everything already in `fx_rates`, plus every currency money has actually been
/// posted in — minus the base, which is always exactly 1 and is never a row.
pub async fn needed_codes(pool: &SqlitePool, base: &str) -> Result<Vec<String>> {
    let base = base.trim().to_uppercase();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT code FROM fx_rates
         UNION
         SELECT DISTINCT currency FROM transactions
         UNION
         SELECT DISTINCT currency FROM recurrences",
    )
    .fetch_all(pool)
    .await?;
    let mut out: Vec<String> = TOP_CURRENCIES.iter().map(|(c, _)| c.to_string()).collect();
    out.extend(rows.into_iter().map(|c| c.to_uppercase()));
    out.retain(|c| *c != base);
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Whether the fetched rates are older than a day.
///
/// True when there is nothing fetched yet but something to fetch *for*: a database
/// with no foreign currency in it has nothing to refresh, and asking the network on
/// its behalf every launch would be a request made for no reason.
pub async fn stale(pool: &SqlitePool, now: i64) -> Result<bool> {
    if needed_codes(pool, &base_currency()).await?.is_empty() {
        return Ok(false);
    }
    let newest: Option<i64> =
        sqlx::query_scalar("SELECT MAX(edited_at) FROM fx_rates WHERE source = 'live'")
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(match newest {
        Some(at) => now - at >= REFRESH_SECS,
        None => true,
    })
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

    /// A trimmed open.er-api.com response, with the base quoted as 1 and a
    /// currency whose rate needs eight decimal places to be worth anything.
    const RESPONSE: &str = r#"{
        "result": "success",
        "base_code": "INR",
        "time_last_update_unix": 1785000000,
        "rates": { "INR": 1, "USD": 0.011962, "EUR": 0.011045, "JPY": 1.8123,
                   "KWD": 0.00366, "junk": 1.0, "TOOLONG": 2.0, "BAD": "x" }
    }"#;

    #[test]
    fn a_rates_response_is_inverted_into_what_a_unit_is_worth() {
        let out = parse_rates(RESPONSE, "INR").unwrap();
        let get = |c: &str| out.iter().find(|(k, _)| k == c).map(|(_, v)| *v);

        // The response says 1 INR buys 0.011962 USD, so one USD is worth
        // 1/0.011962 = ₹83.60. Getting this backwards would value a $10
        // subscription at 12 paise.
        assert_eq!(get("USD"), Some(83_598_060));
        assert_eq!(get("JPY"), Some(551_785));
        // A currency worth ~₹273: fine at twelve decimal places, meaningless at
        // the six a hand-typed rate uses.
        assert_eq!(get("KWD"), Some(273_224_043));

        // The base itself is never stored as a rate — `rate_for` returns 1 for it
        // by definition, and a row saying otherwise is a trap.
        assert_eq!(get("INR"), None);
        // Remote input: anything that is not a three-letter code, or not a number,
        // is dropped rather than trusted.
        assert_eq!(get("JUNK"), None);
        assert_eq!(get("TOOLONG"), None);
        assert_eq!(get("BAD"), None);
    }

    #[test]
    fn a_response_that_is_not_a_rates_response_is_an_error_not_an_empty_list() {
        // Silently applying nothing would leave stale rates looking fresh.
        assert!(parse_rates("not json", "INR").is_err());
        assert!(parse_rates(r#"{"result":"error"}"#, "INR").is_err());
        assert!(parse_rates(r#"{"rates":{}}"#, "INR").is_err());
        // Only the base came back, so there is nothing to store.
        assert!(parse_rates(r#"{"rates":{"INR":1}}"#, "INR").is_err());
    }

    #[tokio::test]
    async fn a_hand_typed_rate_survives_the_daily_refresh() {
        // The rule this whole `source` column exists for: someone who types the
        // rate their card actually charged must not have it quietly replaced.
        let p = pool().await;
        sqlx::query(
            "INSERT INTO recurrences (kind, name, currency, cycle, created_at)
             VALUES ('subscription', 'Some SaaS', 'USD', 'monthly', 0),
                    ('subscription', 'Andere',    'EUR', 'monthly', 0)",
        )
        .execute(&p)
        .await
        .unwrap();
        set(&p, "USD", 90_000_000, 5).await.unwrap();

        let fetched = parse_rates(RESPONSE, "INR").unwrap();
        let written = apply_live(&p, &fetched, 1_000).await.unwrap();

        assert_eq!(rate_for(&p, "USD", "INR").await.unwrap(), 90_000_000, "theirs stands");
        assert_eq!(rate_for(&p, "EUR", "INR").await.unwrap(), 90_538_705, "this one is fetched");
        assert!(written >= 1, "EUR at least");
        // The hand-typed one is not among what was written, whatever else was.
        let held = list(&p).await.unwrap();
        assert!(held.iter().any(|r| r.code == "USD" && !r.live), "USD is still theirs");
        // A major is kept whether or not anything is priced in it yet — that is
        // what makes the rates panel a monitor rather than a form.
        assert!(held.iter().any(|r| r.code == "JPY" && r.live));
        // A currency that is neither a major nor in use is still not stored: the
        // response lists well over a hundred and this database uses three.
        assert!(held.iter().all(|r| r.code != "KWD"));
    }

    #[tokio::test]
    async fn rates_go_stale_after_a_day_and_never_before() {
        let p = pool().await;
        // The ten majors are always wanted now, so an untouched database has
        // something to fetch for from the start — which is the point: a rate has
        // to be there *before* the first foreign amount is entered, not after.
        assert!(stale(&p, 100_000).await.unwrap(), "nothing fetched yet");

        sqlx::query(
            "INSERT INTO recurrences (kind, name, currency, cycle, created_at)
             VALUES ('subscription', 'Some SaaS', 'USD', 'monthly', 0)",
        )
        .execute(&p)
        .await
        .unwrap();
        assert!(stale(&p, 100_000).await.unwrap(), "nothing fetched yet");

        apply_live(&p, &[("USD".into(), 83_600_000)], 100_000).await.unwrap();
        assert!(!stale(&p, 100_000 + REFRESH_SECS - 1).await.unwrap());
        assert!(stale(&p, 100_000 + REFRESH_SECS).await.unwrap());

        // A hand-typed rate is not a fetch: it must not hold the refresh off,
        // because the currencies the user did *not* type still need one.
        let p2 = pool().await;
        sqlx::query(
            "INSERT INTO recurrences (kind, name, currency, cycle, created_at)
             VALUES ('subscription', 'Some SaaS', 'USD', 'monthly', 0)",
        )
        .execute(&p2)
        .await
        .unwrap();
        set(&p2, "USD", 83_600_000, 100_000).await.unwrap();
        assert!(stale(&p2, 100_001).await.unwrap());
    }

    #[test]
    fn the_endpoint_is_built_from_the_base() {
        assert_eq!(endpoint("inr"), "https://open.er-api.com/v6/latest/INR");
        assert!(auto_enabled(), "on unless turned off");
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
