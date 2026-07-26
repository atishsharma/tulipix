//! Loans and EMIs: amortisation, and what a prepayment is worth.
//!
//! A loan is an account of `kind = 'loan'` plus one row of detail, so its balance
//! comes from the same derivation every other account uses and the Accounts tab
//! needs no separate debt model.
//!
//! The schedule is integer arithmetic throughout: interest for a month is
//! `balance × rate_bp / 120_000`, truncated, and the principal part is whatever
//! is left of the EMI. The final instalment is adjusted to exactly clear the
//! balance, which is what real lenders do and what makes the schedule sum to the
//! principal rather than to the principal ± a few paise of accumulated rounding.
//!
//! [`emi_for`] is the one place a float appears in this crate. Computing an EMI
//! needs `(1 + r)^n`, and there is no integer form of that; the result is
//! immediately rounded to minor units and never used in further arithmetic.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use sqlx::SqlitePool;

use crate::date;

/// Basis points to a monthly rate: `rate_bp / 10_000 / 12`, as a divisor.
/// 8.4% a year is `840` bp, so a month's interest is `balance × 840 / 120_000`.
const BP_MONTHS: i128 = 10_000 * 12;

#[derive(Clone, Debug)]
pub struct NewLoan {
    pub name: String,
    pub principal_minor: i64,
    /// Basis points a year. 8.4% = 840.
    pub rate_bp: i64,
    pub tenure_months: i64,
    pub started_on: String,
    /// `None` computes it from the other three.
    pub emi_minor: Option<i64>,
    pub emi_day: i64,
    pub currency: String,
}

#[derive(Clone, Debug)]
pub struct LoanRow {
    pub account_id: i64,
    pub name: String,
    pub currency: String,
    pub principal_minor: i64,
    pub rate_bp: i64,
    pub tenure_months: i64,
    pub started_on: String,
    pub emi_minor: i64,
    pub emi_day: i64,
    /// Derived from the ledger: what is still owed.
    pub balance_minor: i64,
    /// Instalments actually posted against this loan.
    pub paid_count: i64,
}

impl LoanRow {
    pub fn remaining_months(&self) -> i64 {
        (self.tenure_months - self.paid_count).max(0)
    }

    /// Fraction of the principal repaid, 0-100.
    pub fn progress_pct(&self) -> i64 {
        if self.principal_minor <= 0 {
            return 100;
        }
        let repaid = (self.principal_minor - self.balance_minor).clamp(0, self.principal_minor);
        ((repaid as i128 * 100) / self.principal_minor as i128) as i64
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instalment {
    /// 1-based instalment number.
    pub n: i64,
    pub due_on: String,
    /// What is actually paid this month. Equal to the loan's EMI except on the
    /// final instalment, which is trimmed to clear the balance exactly.
    pub emi_minor: i64,
    pub interest_minor: i64,
    pub principal_minor: i64,
    /// Outstanding after this instalment.
    pub balance_minor: i64,
}

/// The standard EMI formula.
///
/// `P × r × (1+r)^n / ((1+r)^n − 1)`, where `r` is the monthly rate. The only
/// float in the crate: `(1+r)^n` has no integer form. Rounded to minor units on
/// the way out, so nothing downstream sees a fraction.
///
/// A zero rate is an interest-free loan and divides the principal evenly, which
/// the formula above cannot express (it is 0/0).
pub fn emi_for(principal_minor: i64, rate_bp: i64, tenure_months: i64) -> i64 {
    if tenure_months <= 0 || principal_minor <= 0 {
        return 0;
    }
    if rate_bp <= 0 {
        // Rounded up, so the final instalment trims down rather than leaving a
        // stub month behind.
        return (principal_minor + tenure_months - 1) / tenure_months;
    }
    let r = rate_bp as f64 / 10_000.0 / 12.0;
    let n = tenure_months as f64;
    let growth = (1.0 + r).powf(n);
    let emi = principal_minor as f64 * r * growth / (growth - 1.0);
    emi.round() as i64
}

/// One month's interest on `balance_minor`, truncated to minor units.
fn interest_for(balance_minor: i64, rate_bp: i64) -> i64 {
    if balance_minor <= 0 || rate_bp <= 0 {
        return 0;
    }
    ((balance_minor as i128 * rate_bp as i128) / BP_MONTHS) as i64
}

/// The full amortisation schedule, principal and interest split per instalment.
///
/// Runs until the balance is cleared rather than for exactly `tenure_months`: a
/// rounded EMI pays a loan off a month early or late by a few paise, and stopping
/// at the nominal tenure would leave a schedule that does not close.
pub fn schedule(
    principal_minor: i64,
    rate_bp: i64,
    tenure_months: i64,
    emi_minor: i64,
    started_on: NaiveDate,
    emi_day: i64,
) -> Vec<Instalment> {
    let mut out = Vec::new();
    if principal_minor <= 0 || emi_minor <= 0 {
        return out;
    }
    let anchor = Some(emi_day.clamp(1, 31) as u32);
    let mut balance = principal_minor;
    let mut due = started_on;
    // Generous ceiling: a 30-year loan is 360 instalments, and this only trips on
    // an EMI too small to ever cover the interest.
    let cap = tenure_months.max(1) * 2 + 12;

    for n in 1..=cap {
        let interest = interest_for(balance, rate_bp);
        if emi_minor <= interest && balance > 0 {
            // The instalment does not even cover the month's interest, so the
            // balance would grow forever. Stop and let the caller see a schedule
            // that visibly fails rather than looping.
            break;
        }
        let mut principal = emi_minor - interest;
        let mut paid = emi_minor;
        if principal >= balance {
            // Final instalment: pay off exactly what is left, plus its interest.
            principal = balance;
            paid = balance + interest;
        }
        balance -= principal;
        due = date::advance(due, date::Cycle::Monthly, anchor);
        out.push(Instalment {
            n,
            due_on: date::iso(due),
            emi_minor: paid,
            interest_minor: interest,
            principal_minor: principal,
            balance_minor: balance,
        });
        if balance == 0 {
            break;
        }
    }
    out
}

/// Total interest over a schedule.
pub fn total_interest(rows: &[Instalment]) -> i64 {
    rows.iter().map(|i| i.interest_minor).sum()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payoff {
    pub months_saved: i64,
    pub interest_saved_minor: i64,
    /// Instalments in the shortened schedule.
    pub new_months: i64,
    pub new_interest_minor: i64,
}

/// What paying `extra_minor` off the principal after `after_n` instalments is
/// worth, keeping the EMI the same and shortening the term.
///
/// The alternative — keeping the term and reducing the EMI — saves far less
/// interest and is not what someone asking this question wants. Returns `None`
/// when the prepayment is larger than what is left to owe, because then there is
/// no schedule left to model.
pub fn prepay(
    principal_minor: i64,
    rate_bp: i64,
    tenure_months: i64,
    emi_minor: i64,
    started_on: NaiveDate,
    emi_day: i64,
    extra_minor: i64,
    after_n: i64,
) -> Option<Payoff> {
    if extra_minor <= 0 {
        return None;
    }
    let base = schedule(principal_minor, rate_bp, tenure_months, emi_minor, started_on, emi_day);
    if base.is_empty() {
        return None;
    }
    let idx = (after_n.max(0) as usize).min(base.len());
    let interest_so_far: i64 = base[..idx].iter().map(|i| i.interest_minor).sum();
    let balance_then = if idx == 0 { principal_minor } else { base[idx - 1].balance_minor };
    if extra_minor >= balance_then {
        return None;
    }

    let resume = if idx == 0 {
        started_on
    } else {
        date::parse(&base[idx - 1].due_on).ok()?
    };
    let rest = schedule(
        balance_then - extra_minor,
        rate_bp,
        tenure_months - idx as i64,
        emi_minor,
        resume,
        emi_day,
    );

    let new_months = idx as i64 + rest.len() as i64;
    let new_interest = interest_so_far + total_interest(&rest);
    Some(Payoff {
        months_saved: base.len() as i64 - new_months,
        interest_saved_minor: total_interest(&base) - new_interest,
        new_months,
        new_interest_minor: new_interest,
    })
}

// ── storage ─────────────────────────────────────────────────────────────────

/// Create the loan account and its detail row together.
pub async fn create(pool: &SqlitePool, l: &NewLoan) -> Result<i64> {
    if l.name.trim().is_empty() {
        bail!("a loan needs a name");
    }
    if l.principal_minor <= 0 {
        bail!("a loan needs a positive principal");
    }
    if l.tenure_months <= 0 {
        bail!("a loan needs a tenure");
    }
    date::parse(&l.started_on).context("a loan needs a valid start date")?;

    let emi = match l.emi_minor {
        Some(e) if e > 0 => e,
        _ => emi_for(l.principal_minor, l.rate_bp, l.tenure_months),
    };
    if emi <= 0 {
        bail!("could not work out an EMI from those numbers");
    }

    let account_id = crate::accounts::create(
        pool,
        &crate::accounts::NewAccount {
            name: l.name.trim().to_string(),
            kind: crate::accounts::AccountKind::Loan,
            currency: l.currency.to_uppercase(),
            // The balance owed is the principal, as a negative balance.
            opening_minor: -l.principal_minor,
            credit_limit_minor: None,
            statement_day: None,
        },
    )
    .await?;

    sqlx::query(
        "INSERT INTO loans
           (account_id, principal_minor, rate_bp, tenure_months, started_on, emi_minor, emi_day)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(account_id)
    .bind(l.principal_minor)
    .bind(l.rate_bp.max(0))
    .bind(l.tenure_months)
    .bind(&l.started_on)
    .bind(emi)
    .bind(l.emi_day.clamp(1, 31))
    .execute(pool)
    .await?;
    Ok(account_id)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<LoanRow>> {
    let rows = sqlx::query_as::<_, (i64, String, String, i64, i64, i64, String, i64, i64)>(
        "SELECT l.account_id, a.name, a.currency, l.principal_minor, l.rate_bp, l.tenure_months,
                l.started_on, l.emi_minor, l.emi_day
           FROM loans l JOIN accounts a ON a.id = l.account_id
          ORDER BY a.closed, a.name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        // The account balance is negative while money is owed; the loan card
        // shows what is outstanding, so flip it.
        let balance_minor = (-crate::accounts::balance(pool, r.0).await?).max(0);
        let paid_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM transactions WHERE to_account_id = ? AND kind = 'transfer'",
        )
        .bind(r.0)
        .fetch_one(pool)
        .await?;
        out.push(LoanRow {
            account_id: r.0,
            name: r.1,
            currency: r.2,
            principal_minor: r.3,
            rate_bp: r.4,
            tenure_months: r.5,
            started_on: r.6,
            emi_minor: r.7,
            emi_day: r.8,
            balance_minor,
            paid_count,
        });
    }
    Ok(out)
}

pub async fn get(pool: &SqlitePool, account_id: i64) -> Result<Option<LoanRow>> {
    Ok(list(pool).await?.into_iter().find(|l| l.account_id == account_id))
}

/// The schedule for a stored loan.
pub async fn schedule_for(pool: &SqlitePool, account_id: i64) -> Result<Vec<Instalment>> {
    let l = get(pool, account_id).await?.context("no such loan")?;
    let start = date::parse(&l.started_on)?;
    Ok(schedule(l.principal_minor, l.rate_bp, l.tenure_months, l.emi_minor, start, l.emi_day))
}

/// Post an EMI payment: a transfer from a real account into the loan.
///
/// A transfer, not an expense — repaying a loan moves money from one thing the
/// user owns to another thing they owe, and counting it as spending would double
/// count the original purchase. The interest portion is genuinely a cost, but
/// splitting one payment across two kinds would make the ledger stop reconciling
/// with the bank statement; the split is shown in the schedule instead.
pub async fn pay_emi(
    pool: &SqlitePool,
    account_id: i64,
    from_account_id: i64,
    amount_minor: i64,
    on: NaiveDate,
) -> Result<i64> {
    let l = get(pool, account_id).await?.context("no such loan")?;
    if amount_minor <= 0 {
        bail!("an EMI payment needs a positive amount");
    }
    let mut t = crate::txn::NewTxn::transfer(
        from_account_id,
        account_id,
        amount_minor,
        &l.currency,
        &date::iso(on),
        &format!("EMI — {}", l.name),
    );
    t.rate_micro = crate::fx::rate_for(pool, &l.currency, &crate::fx::base_currency()).await?;
    crate::txn::post(pool, &t).await
}

pub async fn delete(pool: &SqlitePool, account_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM loans WHERE account_id = ?").bind(account_id).execute(pool).await?;
    crate::accounts::delete(pool, account_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn home_loan() -> NewLoan {
        NewLoan {
            name: "HDFC home loan".into(),
            principal_minor: 350_000_000, // ₹35,00,000
            rate_bp: 840,                 // 8.4%
            tenure_months: 180,
            started_on: "2026-01-05".into(),
            emi_minor: None,
            emi_day: 5,
            currency: "INR".into(),
        }
    }

    #[test]
    fn the_emi_formula_lands_where_a_bank_would_put_it() {
        // ₹35,00,000 at 8.4% over 15 years is about ₹34,270 a month.
        let emi = emi_for(350_000_000, 840, 180);
        assert!(
            (3_400_000..3_460_000).contains(&emi),
            "expected roughly ₹34,300 a month, got {emi}"
        );
    }

    #[test]
    fn an_interest_free_loan_divides_evenly() {
        assert_eq!(emi_for(120_000, 0, 12), 10_000);
        // And when it does not divide evenly, the EMI rounds up so the final
        // instalment trims rather than leaving a stub month.
        assert_eq!(emi_for(100_007, 0, 12), 8_334);
        let rows = schedule(100_007, 0, 12, 8_334, d("2026-01-05"), 5);
        assert_eq!(rows.len(), 12);
        assert_eq!(rows.last().unwrap().balance_minor, 0);
    }

    #[test]
    fn every_instalment_splits_exactly_into_principal_and_interest() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let rows = schedule(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5);
        assert!(!rows.is_empty());
        for i in &rows {
            assert_eq!(
                i.principal_minor + i.interest_minor,
                i.emi_minor,
                "instalment {} does not split cleanly",
                i.n
            );
        }
    }

    #[test]
    fn the_schedule_closes_on_exactly_the_principal() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let rows = schedule(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5);
        let repaid: i64 = rows.iter().map(|i| i.principal_minor).sum();
        assert_eq!(repaid, l.principal_minor, "the principal must be repaid to the paisa");
        assert_eq!(rows.last().unwrap().balance_minor, 0);
    }

    #[test]
    fn the_final_instalment_is_trimmed_not_rounded() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let rows = schedule(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5);
        let last = rows.last().unwrap();
        assert!(last.emi_minor <= emi, "the last payment is at most a full EMI, never more");
        assert_eq!(last.principal_minor, last.emi_minor - last.interest_minor);
    }

    #[test]
    fn interest_falls_and_principal_rises_over_the_term() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let rows = schedule(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5);
        assert!(rows[0].interest_minor > rows[0].principal_minor, "early on it is mostly interest");
        let last_full = &rows[rows.len() - 2];
        assert!(last_full.principal_minor > last_full.interest_minor, "and at the end mostly principal");
    }

    #[test]
    fn instalment_dates_follow_the_emi_day_through_february() {
        let rows = schedule(120_000_000, 840, 6, emi_for(120_000_000, 840, 6), d("2026-01-31"), 31);
        let dates: Vec<&str> = rows.iter().map(|i| i.due_on.as_str()).collect();
        assert_eq!(dates[0], "2026-02-28");
        assert_eq!(dates[1], "2026-03-31", "the anchor comes back");
    }

    #[test]
    fn an_emi_too_small_to_cover_the_interest_stops_rather_than_looping() {
        // ₹1 a month against ₹35 lakh at 8.4% never pays anything off.
        let rows = schedule(350_000_000, 840, 180, 100, d("2026-01-05"), 5);
        assert!(rows.is_empty(), "a schedule that cannot close must not be invented");
    }

    #[test]
    fn a_prepayment_shortens_the_term_and_saves_interest() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let base = schedule(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5);

        // ₹5,00,000 off after two years.
        let p = prepay(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5, 50_000_000, 24)
            .unwrap();
        assert!(p.months_saved > 0, "paying down principal must shorten the term");
        assert!(p.interest_saved_minor > 0);
        assert_eq!(p.new_months, base.len() as i64 - p.months_saved);
        assert_eq!(p.new_interest_minor, total_interest(&base) - p.interest_saved_minor);
    }

    #[test]
    fn a_prepayment_covering_the_whole_balance_has_no_schedule_to_model() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        assert!(
            prepay(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5, 400_000_000, 24)
                .is_none()
        );
        assert!(
            prepay(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5, 0, 24).is_none()
        );
    }

    #[test]
    fn prepaying_earlier_saves_more_than_prepaying_later() {
        let l = home_loan();
        let emi = emi_for(l.principal_minor, l.rate_bp, l.tenure_months);
        let early =
            prepay(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5, 20_000_000, 12)
                .unwrap();
        let late =
            prepay(l.principal_minor, l.rate_bp, l.tenure_months, emi, d("2026-01-05"), 5, 20_000_000, 120)
                .unwrap();
        assert!(
            early.interest_saved_minor > late.interest_saved_minor,
            "the whole point of prepaying early"
        );
    }

    #[tokio::test]
    async fn a_stored_loan_starts_owing_its_principal() {
        let p = pool().await;
        let id = create(&p, &home_loan()).await.unwrap();
        assert_eq!(accounts::balance(&p, id).await.unwrap(), -350_000_000);

        let row = get(&p, id).await.unwrap().unwrap();
        assert_eq!(row.balance_minor, 350_000_000, "shown as what is owed, positive");
        assert_eq!(row.progress_pct(), 0);
        assert_eq!(row.remaining_months(), 180);
        assert!(row.emi_minor > 0, "the EMI was worked out for us");
    }

    #[tokio::test]
    async fn a_loan_counts_as_debt_and_never_as_liquid_money() {
        let p = pool().await;
        accounts::create(&p, &accounts::NewAccount::bank("HDFC", 10_839_000)).await.unwrap();
        create(&p, &home_loan()).await.unwrap();

        let t = accounts::totals(&p).await.unwrap();
        assert_eq!(t.liquid_minor, 10_839_000);
        assert_eq!(t.debt_minor, 350_000_000);
    }

    #[tokio::test]
    async fn paying_an_emi_reduces_the_debt_and_is_not_spending() {
        let p = pool().await;
        let bank = accounts::create(&p, &accounts::NewAccount::bank("HDFC", 100_000_000)).await.unwrap();
        let loan = create(&p, &home_loan()).await.unwrap();
        let emi = get(&p, loan).await.unwrap().unwrap().emi_minor;

        pay_emi(&p, loan, bank, emi, d("2026-02-05")).await.unwrap();
        assert_eq!(accounts::balance(&p, bank).await.unwrap(), 100_000_000 - emi);
        assert_eq!(accounts::balance(&p, loan).await.unwrap(), -350_000_000 + emi);
        assert_eq!(
            crate::txn::spent_between(&p, "2026-01-01", "2026-12-31").await.unwrap(),
            0,
            "repaying a loan is not a purchase"
        );

        let row = get(&p, loan).await.unwrap().unwrap();
        assert_eq!(row.paid_count, 1);
        assert_eq!(row.remaining_months(), 179);
    }

    #[tokio::test]
    async fn the_stored_schedule_matches_the_computed_one() {
        let p = pool().await;
        let id = create(&p, &home_loan()).await.unwrap();
        let stored = schedule_for(&p, id).await.unwrap();
        let row = get(&p, id).await.unwrap().unwrap();
        let computed =
            schedule(row.principal_minor, row.rate_bp, row.tenure_months, row.emi_minor, d("2026-01-05"), 5);
        assert_eq!(stored, computed);
    }

    #[tokio::test]
    async fn nonsense_loans_are_refused() {
        let p = pool().await;
        assert!(create(&p, &NewLoan { name: "  ".into(), ..home_loan() }).await.is_err());
        assert!(create(&p, &NewLoan { principal_minor: 0, ..home_loan() }).await.is_err());
        assert!(create(&p, &NewLoan { tenure_months: 0, ..home_loan() }).await.is_err());
        assert!(create(&p, &NewLoan { started_on: "nope".into(), ..home_loan() }).await.is_err());
    }

    #[tokio::test]
    async fn an_explicit_emi_overrides_the_computed_one() {
        let p = pool().await;
        let id = create(&p, &NewLoan { emi_minor: Some(3_820_000), ..home_loan() }).await.unwrap();
        assert_eq!(get(&p, id).await.unwrap().unwrap().emi_minor, 3_820_000);
    }
}
