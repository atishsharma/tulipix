//! Finding the repeating charges already in the ledger.
//!
//! The user should not have to tell the app that Netflix is a subscription — the
//! evidence is sitting in twelve months of imported rows. This groups postings by
//! their normalised merchant, looks for a consistent period, and proposes a
//! recurrence.
//!
//! The routing rule is the one from the design, and it is what keeps Bills and
//! Subscriptions apart without the user classifying anything:
//!
//! > **identical amount → subscription; amount varying but date stable → bill.**
//!
//! Nothing here creates anything. Every return value is a proposal the user
//! confirms, because a wrong guess that silently created an auto-posting
//! subscription would start moving money on its own.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::date::{self, Cycle};
use crate::recur::RecurKind;

/// Fewest occurrences that can establish a pattern.
///
/// Two of anything is a coincidence. Three with even gaps is a subscription.
pub const MIN_OCCURRENCES: usize = 3;

/// How far a gap may stray from the cycle's nominal length and still count.
///
/// Four days absorbs weekends and month-length variation without letting a
/// quarterly charge read as monthly.
pub const GAP_TOLERANCE_DAYS: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// The amount barely moves, or the dates barely move. Safe to accept as-is.
    High,
    /// A pattern, but a loose one. Worth showing, worth checking.
    Low,
}

#[derive(Clone, Debug)]
pub struct Proposal {
    /// Normalised merchant, which is also what the ledger groups on.
    pub merchant: String,
    /// A readable name for the proposed recurrence: the merchant in title case,
    /// so `NETFLIX 6033` becomes `Netflix` rather than carrying a statement
    /// reference number into the UI forever.
    pub label: String,
    pub kind: RecurKind,
    /// `Some` for a subscription, `None` for a bill — which is exactly the
    /// distinction `recurrences.amount_minor` encodes.
    pub amount_minor: Option<i64>,
    /// Mean of the observed amounts, always present. For a bill this becomes the
    /// opening estimate.
    pub typical_minor: i64,
    pub currency: String,
    pub cycle: Cycle,
    pub anchor_day: i64,
    pub next_due_on: String,
    pub occurrences: usize,
    pub confidence: Confidence,
    pub account_id: i64,
    pub category_id: Option<i64>,
}

/// `NETFLIX COM` becomes `Netflix Com`.
///
/// The merchant is stored upper-cased for matching; shouting it back at the user
/// in every row of the Subscriptions tab is not the same requirement.
fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c.flat_map(|x| x.to_lowercase())).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Which cycle a mean gap in days looks like.
fn cycle_for(mean_gap: i64) -> Option<Cycle> {
    match mean_gap {
        6..=8 => Some(Cycle::Weekly),
        13..=16 => None, // fortnightly: real, but not a cycle the schema has
        27..=32 => Some(Cycle::Monthly),
        86..=95 => Some(Cycle::Quarterly),
        358..=372 => Some(Cycle::Yearly),
        _ => None,
    }
}

/// Nominal length of a cycle in days, for the tolerance check.
fn nominal_days(c: Cycle) -> i64 {
    match c {
        Cycle::Weekly => 7,
        Cycle::Monthly => 30,
        Cycle::Quarterly => 91,
        Cycle::Yearly => 365,
        Cycle::Irregular => 0,
    }
}

/// One merchant's postings, as the detector sees them.
struct Group {
    merchant: String,
    label: String,
    currency: String,
    account_id: i64,
    category_id: Option<i64>,
    /// Oldest first.
    dates: Vec<String>,
    amounts: Vec<i64>,
}

impl Group {
    fn gaps(&self) -> Vec<i64> {
        self.dates
            .windows(2)
            .filter_map(|w| {
                let a = date::parse(&w[0]).ok()?;
                let b = date::parse(&w[1]).ok()?;
                Some(date::days_between(a, b))
            })
            .collect()
    }
}

/// Look for repeating charges among expenses that are not already accounted for.
///
/// Postings already tied to a recurrence are excluded, and so are merchants that
/// match an existing recurrence's name — otherwise every tick would re-propose
/// what the user has already set up, and dismissing it would never stick.
pub async fn propose(pool: &SqlitePool) -> Result<Vec<Proposal>> {
    let rows = sqlx::query_as::<_, (String, String, String, i64, i64, String, Option<i64>)>(
        "SELECT t.merchant_norm, t.description, t.occurred_on, t.amount_minor, t.account_id,
                t.currency, t.category_id
           FROM transactions t
          WHERE t.kind = 'expense' AND t.recurrence_id IS NULL AND t.merchant_norm <> ''
            AND NOT EXISTS (
                SELECT 1 FROM recurrences r
                 WHERE UPPER(r.name) = t.merchant_norm
                    -- Prefix match only for names long enough to be specific. A
                    -- three-letter recurrence name must not swallow every
                    -- merchant that happens to start with the same letters.
                    OR (LENGTH(r.name) >= 4 AND t.merchant_norm LIKE UPPER(r.name) || '%'))
          ORDER BY t.merchant_norm, t.occurred_on",
    )
    .fetch_all(pool)
    .await?;

    // Grouped in Rust rather than SQL: the gap analysis needs every date, and
    // GROUP BY would have to hand back a concatenated string to provide them.
    let mut groups: Vec<Group> = Vec::new();
    for (merchant, _description, on, amount, account_id, currency, category_id) in rows {
        match groups.last_mut() {
            Some(g) if g.merchant == merchant => {
                g.dates.push(on);
                g.amounts.push(amount);
            }
            _ => groups.push(Group {
                label: title_case(&merchant),
                merchant,
                currency,
                account_id,
                category_id,
                dates: vec![on],
                amounts: vec![amount],
            }),
        }
    }

    Ok(groups.iter().filter_map(assess).collect())
}

/// Decide whether one merchant's history is a recurrence, and which kind.
fn assess(g: &Group) -> Option<Proposal> {
    if g.dates.len() < MIN_OCCURRENCES {
        return None;
    }
    let gaps = g.gaps();
    if gaps.is_empty() {
        return None;
    }
    let mean_gap = gaps.iter().sum::<i64>() / gaps.len() as i64;
    let cycle = cycle_for(mean_gap)?;

    // Every gap has to be close to the cycle's nominal length. A merchant charged
    // twice in one week and then not for two months averages out to something
    // plausible, and this is what rejects it.
    let nominal = nominal_days(cycle);
    if gaps.iter().any(|g| (g - nominal).abs() > GAP_TOLERANCE_DAYS + month_slack(cycle)) {
        return None;
    }

    let n = g.amounts.len() as i64;
    let mean_amount = g.amounts.iter().sum::<i64>() / n;
    let identical = g.amounts.windows(2).all(|w| w[0] == w[1]);

    // The routing rule: a fixed amount is a subscription, a moving one is a bill.
    let kind = if identical { RecurKind::Subscription } else { RecurKind::Bill };

    let spread = g.amounts.iter().max()? - g.amounts.iter().min()?;
    let amount_varies_pct =
        if mean_amount > 0 { (spread as i128 * 100 / mean_amount as i128) as i64 } else { 100 };
    let gap_spread = gaps.iter().max()? - gaps.iter().min()?;
    let confidence =
        if amount_varies_pct < 2 || gap_spread <= 2 { Confidence::High } else { Confidence::Low };

    let last = g.dates.last()?;
    let last_date = date::parse(last).ok()?;
    let anchor = chrono::Datelike::day(&last_date) as i64;

    Some(Proposal {
        merchant: g.merchant.clone(),
        label: g.label.clone(),
        kind,
        amount_minor: identical.then_some(*g.amounts.last()?),
        typical_minor: mean_amount,
        currency: g.currency.clone(),
        cycle,
        anchor_day: anchor,
        next_due_on: date::iso(date::advance(last_date, cycle, Some(anchor as u32))),
        occurrences: g.dates.len(),
        confidence,
        account_id: g.account_id,
        category_id: g.category_id,
    })
}

/// Extra tolerance for cycles whose real length varies.
///
/// Calendar months are 28-31 days, so a monthly charge's gaps legitimately span
/// three days more than the tolerance allows on its own. Weekly charges have no
/// such variation and get none.
fn month_slack(c: Cycle) -> i64 {
    match c {
        Cycle::Monthly => 2,
        Cycle::Quarterly => 3,
        Cycle::Yearly => 2,
        _ => 0,
    }
}

/// Turn an accepted proposal into a real recurrence, and adopt the postings that
/// produced it.
///
/// Adoption matters: without it the same rows would be proposed again on the next
/// import, and the new recurrence would have no history to estimate from.
pub async fn accept(pool: &SqlitePool, p: &Proposal) -> Result<i64> {
    let r = crate::recur::NewRecurrence {
        kind: p.kind,
        name: p.label.clone(),
        amount_minor: p.amount_minor,
        currency: p.currency.clone(),
        cycle: p.cycle,
        anchor_day: Some(p.anchor_day),
        next_due_on: Some(p.next_due_on.clone()),
        account_id: Some(p.account_id),
        category_id: p.category_id,
        // Never on by default. The user asked to record a pattern, not to hand
        // over the authority to move money unprompted.
        auto_post: false,
        reminder_days: 3,
        note: Some(format!("Detected from {} imported charges", p.occurrences)),
    };
    let id = crate::recur::create(pool, &r).await?;
    sqlx::query("UPDATE transactions SET recurrence_id = ? WHERE merchant_norm = ? AND recurrence_id IS NULL")
        .bind(id)
        .bind(&p.merchant)
        .execute(pool)
        .await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::txn::{self, NewTxn};
    use crate::{accounts, schema};

    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&p).await.unwrap();
        schema::seed_defaults(&p).await.unwrap();
        p
    }

    async fn bank(p: &SqlitePool) -> i64 {
        accounts::create(p, &accounts::NewAccount::bank("HDFC", 100_000_000)).await.unwrap()
    }

    async fn charge(p: &SqlitePool, acct: i64, desc: &str, on: &str, amount: i64) {
        let mut t = NewTxn::expense(acct, amount, "INR", on, desc);
        t.source = "csv".into();
        txn::post(p, &t).await.unwrap();
    }

    #[tokio::test]
    async fn an_identical_monthly_amount_proposes_a_subscription() {
        let p = pool().await;
        let a = bank(&p).await;
        // Different reference numbers each month, as a real statement has.
        charge(&p, a, "NETFLIX 4471", "2026-05-26", 64_900).await;
        charge(&p, a, "NETFLIX 5120", "2026-06-26", 64_900).await;
        charge(&p, a, "NETFLIX 6033", "2026-07-26", 64_900).await;

        let props = propose(&p).await.unwrap();
        assert_eq!(props.len(), 1);
        let n = &props[0];
        assert_eq!(n.merchant, "NETFLIX");
        assert_eq!(n.kind, RecurKind::Subscription);
        assert_eq!(n.amount_minor, Some(64_900));
        assert_eq!(n.cycle, Cycle::Monthly);
        assert_eq!(n.anchor_day, 26);
        assert_eq!(n.next_due_on, "2026-08-26");
        assert_eq!(n.occurrences, 3);
        assert_eq!(n.confidence, Confidence::High);
    }

    #[tokio::test]
    async fn a_varying_amount_on_a_stable_date_proposes_a_bill() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "BESCOM POWER", "2026-05-12", 300_000).await;
        charge(&p, a, "BESCOM POWER", "2026-06-12", 320_000).await;
        charge(&p, a, "BESCOM POWER", "2026-07-12", 328_400).await;

        let props = propose(&p).await.unwrap();
        assert_eq!(props.len(), 1);
        let b = &props[0];
        assert_eq!(b.kind, RecurKind::Bill);
        assert_eq!(b.amount_minor, None, "a bill's amount is not fixed, and must not pretend to be");
        assert_eq!(b.typical_minor, (300_000 + 320_000 + 328_400) / 3);
        assert_eq!(b.confidence, Confidence::High, "the dates barely move");
    }

    #[tokio::test]
    async fn two_occurrences_are_a_coincidence_not_a_pattern() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "NETFLIX", "2026-06-26", 64_900).await;
        charge(&p, a, "NETFLIX", "2026-07-26", 64_900).await;
        assert!(propose(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn irregular_gaps_propose_nothing() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "BIG BAZAAR", "2026-07-01", 120_000).await;
        charge(&p, a, "BIG BAZAAR", "2026-07-04", 80_000).await;
        charge(&p, a, "BIG BAZAAR", "2026-07-19", 250_000).await;
        charge(&p, a, "BIG BAZAAR", "2026-07-22", 40_000).await;
        assert!(propose(&p).await.unwrap().is_empty(), "shopping is not a subscription");
    }

    #[tokio::test]
    async fn a_burst_then_a_long_gap_is_rejected_even_though_the_mean_looks_right() {
        let p = pool().await;
        let a = bank(&p).await;
        // Gaps of 2 and 58 days: mean 30, which would pass a mean-only check.
        charge(&p, a, "SOME SHOP", "2026-05-01", 50_000).await;
        charge(&p, a, "SOME SHOP", "2026-05-03", 50_000).await;
        charge(&p, a, "SOME SHOP", "2026-06-30", 50_000).await;
        assert!(propose(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn weekly_and_quarterly_and_yearly_are_recognised() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "WEEKLY THING", "2026-07-05", 10_000).await;
        charge(&p, a, "WEEKLY THING", "2026-07-12", 10_000).await;
        charge(&p, a, "WEEKLY THING", "2026-07-19", 10_000).await;
        charge(&p, a, "QUARTER THING", "2026-01-10", 90_000).await;
        charge(&p, a, "QUARTER THING", "2026-04-10", 90_000).await;
        charge(&p, a, "QUARTER THING", "2026-07-10", 90_000).await;
        charge(&p, a, "YEAR THING", "2024-03-01", 500_000).await;
        charge(&p, a, "YEAR THING", "2025-03-01", 500_000).await;
        charge(&p, a, "YEAR THING", "2026-03-01", 500_000).await;

        let props = propose(&p).await.unwrap();
        let cycle_of = |m: &str| props.iter().find(|p| p.merchant == m).map(|p| p.cycle);
        assert_eq!(cycle_of("WEEKLY THING"), Some(Cycle::Weekly));
        assert_eq!(cycle_of("QUARTER THING"), Some(Cycle::Quarterly));
        assert_eq!(cycle_of("YEAR THING"), Some(Cycle::Yearly));
    }

    #[tokio::test]
    async fn a_fortnightly_charge_is_not_forced_into_a_cycle_the_schema_lacks() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "FORTNIGHT", "2026-07-01", 10_000).await;
        charge(&p, a, "FORTNIGHT", "2026-07-15", 10_000).await;
        charge(&p, a, "FORTNIGHT", "2026-07-29", 10_000).await;
        assert!(propose(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_wobbly_amount_and_wobbly_dates_are_low_confidence() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "SOME GYM", "2026-05-03", 200_000).await;
        charge(&p, a, "SOME GYM", "2026-06-06", 260_000).await;
        charge(&p, a, "SOME GYM", "2026-07-04", 210_000).await;
        let props = propose(&p).await.unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].confidence, Confidence::Low);
    }

    #[tokio::test]
    async fn income_is_never_a_candidate() {
        let p = pool().await;
        let a = bank(&p).await;
        for on in ["2026-05-01", "2026-06-01", "2026-07-01"] {
            let mut t = NewTxn::expense(a, 5_000_000, "INR", on, "SALARY");
            t.kind = txn::TxnKind::Income;
            txn::post(&p, &t).await.unwrap();
        }
        assert!(propose(&p).await.unwrap().is_empty(), "income is not a subscription");
    }

    #[tokio::test]
    async fn a_merchant_the_user_already_set_up_is_not_re_proposed() {
        let p = pool().await;
        let a = bank(&p).await;
        crate::recur::create(&p, &crate::recur::NewRecurrence::subscription("Netflix", 64_900, "2026-08-26"))
            .await
            .unwrap();
        charge(&p, a, "NETFLIX 4471", "2026-05-26", 64_900).await;
        charge(&p, a, "NETFLIX 5120", "2026-06-26", 64_900).await;
        charge(&p, a, "NETFLIX 6033", "2026-07-26", 64_900).await;
        assert!(propose(&p).await.unwrap().is_empty(), "dismissing it has to stick");
    }

    #[tokio::test]
    async fn accepting_adopts_the_history_and_stops_re_proposing() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "BESCOM POWER", "2026-05-12", 300_000).await;
        charge(&p, a, "BESCOM POWER", "2026-06-12", 320_000).await;
        charge(&p, a, "BESCOM POWER", "2026-07-12", 328_400).await;

        let props = propose(&p).await.unwrap();
        let id = accept(&p, &props[0]).await.unwrap();

        let adopted: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM transactions WHERE recurrence_id = ?")
                .bind(id)
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(adopted, 3, "the new recurrence needs its own history to estimate from");
        assert!(propose(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_accepted_proposal_never_auto_posts() {
        let p = pool().await;
        let a = bank(&p).await;
        charge(&p, a, "NETFLIX 4471", "2026-05-26", 64_900).await;
        charge(&p, a, "NETFLIX 5120", "2026-06-26", 64_900).await;
        charge(&p, a, "NETFLIX 6033", "2026-07-26", 64_900).await;
        let props = propose(&p).await.unwrap();
        let id = accept(&p, &props[0]).await.unwrap();

        let auto: i64 = sqlx::query_scalar("SELECT auto_post FROM recurrences WHERE id = ?")
            .bind(id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(auto, 0, "a guess must not be granted authority to move money");
    }
}
