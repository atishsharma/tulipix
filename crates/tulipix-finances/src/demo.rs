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
    seed_now(pool, today).await
}

/// Seed again because the user asked, whatever is in the database already.
///
/// Behind a button rather than automatic: the guards on [`seed`] exist so the app
/// never invents money on its own, and pressing "Add sample data" is not the app
/// deciding anything. Any surviving samples go first, so pressing it twice does not
/// leave two Netflixes.
pub async fn reseed(pool: &SqlitePool, today: NaiveDate) -> Result<bool> {
    remove_all(pool).await?;
    // The marker too, or the seed below refuses to run.
    sqlx::query("DELETE FROM demo_rows").execute(pool).await?;
    seed_now(pool, today).await
}

async fn seed_now(pool: &SqlitePool, today: NaiveDate) -> Result<bool> {
    // What is already here, so the marking sweep at the end can tell the sample
    // rows from the user's. On a fresh database these are all zero and the sweep
    // marks everything, which is the same thing.
    let before = high_water(pool).await?;
    // Written before the rows, so a failure halfway through cannot come back on
    // the next launch and seed a second copy on top of the first.
    mark(pool, SEEDED, 1).await?;

    let iso = |d: NaiveDate| date::iso(d);
    let ago = |n: i64| today - Duration::days(n);
    let ahead = |n: i64| today + Duration::days(n);

    // ── accounts ────────────────────────────────────────────────────────────
    // The opening balances have to cover a year of three loans' EMIs and a year of
    // spending and still leave plausible figures, or the sample opens overdrawn —
    // which `the_sample_month_is_internally_consistent` refuses to let ship.
    let bank = accounts::create(pool, &NewAccount::bank("HDFC Savings", 200_000_000)).await?;

    let cash = accounts::create(
        pool,
        &NewAccount { kind: AccountKind::Cash, ..NewAccount::bank("Cash", 500_000) },
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
        &NewAccount { kind: AccountKind::Wallet, ..NewAccount::bank("Paytm", 500_000) },
    )
    .await?;

    // A second bank, so the Accounts grid is a grid and the account filter on the
    // ledger has something to narrow to. Left un-reconciled on purpose: "never
    // checked" beside two accounts that have been is the contrast that makes the
    // reconcile line mean anything.
    let savings = accounts::create(pool, &NewAccount::bank("SBI Emergency fund", 45_000_000)).await?;

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

    // Two more, at different stages: a car loan over halfway through and a phone
    // EMI nearly finished. One loan shows a progress bar; three show that the bar
    // means something, and give the Loans card the shape it was designed around.
    for (name, principal, rate_bp, tenure, started, paid) in [
        // ₹8,00,000 over 5 years, and ₹96,000 over one. Minor units, like every
        // other figure in this crate.
        ("Car loan", 80_000_000i64, 950i64, 60i64, 1180i64, 39i64),
        ("Phone EMI", 9_600_000, 1400, 12, 280, 9),
    ] {
        let id = loans::create(
            pool,
            &NewLoan {
                name: name.into(),
                principal_minor: principal,
                rate_bp,
                tenure_months: tenure,
                started_on: iso(ago(started)),
                emi_minor: None,
                emi_day: 7,
                currency: "INR".into(),
            },
        )
        .await?;
        let e = loans::get(pool, id).await?.map(|l| l.emi_minor).unwrap_or(0);
        if e > 0 {
            for n in 0..paid {
                loans::pay_emi(pool, id, bank, e, ago(started - n * 30)).await?;
            }
        }
    }

    // ── categories to hang things off ───────────────────────────────────────
    // Names taken from `schema::SEED_CATEGORIES` exactly, so the sample uses the
    // user's own categories rather than inventing near-duplicates beside them.
    let food = category(pool, "Eating out").await?;
    let groceries = category(pool, "Groceries").await?;
    let transport = category(pool, "Transport").await?;
    let fuel = category(pool, "Fuel").await?;
    let bills_cat = category(pool, "Utilities").await?;
    let rent_cat = category(pool, "Rent").await?;
    let subs_cat = category(pool, "Subscriptions").await?;
    let health = category(pool, "Health").await?;
    let shopping = category(pool, "Shopping").await?;
    let salary = category(pool, "Salary").await?;

    // ── the ledger ──────────────────────────────────────────────────────────
    // Income first, so the running balance reads the way a real month does.
    let mut rows: Vec<(NewTxn, Option<i64>)> = Vec::new();

    // The salary has to land *inside* the current month, whatever day of the month
    // the app is first opened on. Dating it a fixed number of days back put it in
    // the previous month for the first 25 days of every month, which left the
    // Overview with zero income, a negative "saved this month", and no savings
    // rate — the section's headline figures all wrong on the strength of one date.
    let pay_day = today.day().min(25);
    let payday = today.with_day(pay_day).unwrap_or(today);
    let mut pay = NewTxn::expense(bank, 12_500_000, "INR", &iso(payday), "Salary — Acme Corp");
    pay.kind = TxnKind::Income;
    rows.push((pay, Some(salary)));

    // Eleven earlier months, so every bar of the twelve-month chart has something
    // in it and the six-month discipline table is full. Six left five empty
    // columns on the left of the chart, which reads as missing data rather than as
    // a quiet year.
    for k in 1..=11i64 {
        let Some(month) = month_start(today, k) else { continue };
        // Enough that no two months are the same height, and shaped so the year
        // has a trend rather than a sawtooth.
        let vary = k * 37_000;
        let mut earlier = NewTxn::expense(
            bank,
            12_500_000 - vary,
            "INR",
            &iso(month + Duration::days(1)),
            "Salary — Acme Corp",
        );
        earlier.kind = TxnKind::Income;
        rows.push((earlier, Some(salary)));
        // One posting per budgeted category, every month. An envelope with a
        // limit but no spending behind it leaves a blank cell in the six-month
        // grid, which reads as a bug rather than as a quiet month.
        for (day, amount, desc, c, acct) in [
            (5i64, 1_850_000 + vary, "Groceries and household", groceries, bank),
            (8, 3_800_000, "Rent", rent_cat, bank),
            (11, 260_000 + vary / 4, "Shopping", shopping, card),
            (13, 640_000 - vary / 2, "Eating out", food, card),
            // From the bank, not from cash: eleven months of it would drain a
            // ₹5,000 wallet several times over and the sample would open
            // overdrawn, which `the_sample_month_is_internally_consistent`
            // refuses to let ship.
            (15, 190_000, "Metro card recharge", transport, bank),
            (17, 420_000 + vary / 3, "Fuel", fuel, card),
            (21, 310_000, "Pharmacy", health, bank),
        ] {
            rows.push((
                NewTxn::expense(acct, amount, "INR", &iso(month + Duration::days(day)), desc),
                Some(c),
            ));
        }
    }

    // This month, in detail. Every category the donut can show needs a posting in
    // it, or the chart is three slices wide and says nothing about where the money
    // actually goes.
    //
    // Several merchants appear more than once on purpose. A merchant the ledger
    // has seen before is what lets an imported statement categorise itself, and a
    // sample where every description is unique would never exercise that.
    for (days, amount, desc, c, acct) in [
        (27i64, 118_000, "Blinkit", groceries, card),
        (26, 64_000, "Swiggy", food, card),
        (24, 462_000, "BigBasket — monthly groceries", groceries, bank),
        (23, 3_800_000, "Rent — August", rent_cat, bank),
        (22, 289_000, "Croma — kettle", shopping, card),
        (21, 34_000, "Auto to office", transport, cash),
        (20, 64_900, "Netflix", subs_cat, card),
        (19, 128_000, "Dinner — Toit", food, card),
        (18, 245_000, "Apollo Pharmacy", health, bank),
        (17, 349_900, "Amazon — headphones", shopping, card),
        (16, 89_900, "Petrol — Indian Oil", fuel, card),
        (15, 142_000, "Electricity — BESCOM", bills_cat, bank),
        (14, 356_000, "Myntra — shirts", shopping, card),
        (13, 84_000, "Blinkit", groceries, card),
        (12, 24_000, "Chai and samosa", food, cash),
        (11, 78_000, "Uber to airport", transport, wallet),
        (10, 52_000, "Swiggy", food, card),
        (9, 156_000, "Blinkit — top-up shop", groceries, wallet),
        (8, 199_000, "Spotify family", subs_cat, card),
        (7, 92_000, "Dr Rao — consultation", health, cash),
        (6, 41_000, "Auto to office", transport, cash),
        (5, 45_000, "Metro card recharge", transport, bank),
        (4, 268_000, "Swiggy — weekend", food, card),
        (3, 121_000, "Petrol — Shell", fuel, card),
        (2, 68_000, "Lunch — Meghana", food, cash),
        (1, 32_000, "Filter coffee", food, cash),
    ] {
        // Only what has already happened this month: a sample that posts spending
        // in the future would make every month-to-date total a lie.
        if days <= today.day() as i64 {
            rows.push((NewTxn::expense(acct, amount, "INR", &iso(ago(days)), desc), Some(c)));
        }
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

    // A wallet top-up and a standing transfer into savings. Both are transfers,
    // neither is spending, and the wallet card has nothing to say without one.
    txn::post(pool, &NewTxn::transfer(bank, wallet, 200_000, "INR", &iso(ago(10)), "Paytm top-up"))
        .await?;
    for k in 0..4i64 {
        let Some(month) = month_start(today, k) else { continue };
        txn::post(
            pool,
            &NewTxn::transfer(
                bank,
                savings,
                1_500_000,
                "INR",
                &iso(month + Duration::days(2)),
                "Monthly saving",
            ),
        )
        .await?;
    }

    // ── reconciles ──────────────────────────────────────────────────────────
    // One that agreed and one that did not. A balance nobody has ever checked
    // against a statement and one checked last week look identical without this,
    // and the difference is the whole point of the Accounts tab.
    let bank_balance = accounts::balance(pool, bank).await?;
    accounts::reconcile(pool, bank, bank_balance, &iso(ago(9))).await?;
    // Cash always comes up short. ₹120 missing is the kind of gap a real wallet
    // has and the reason a recount books rather than silently adjusts.
    let cash_balance = accounts::balance(pool, cash).await?;
    accounts::reconcile(pool, cash, cash_balance - 12_000, &iso(ago(6))).await?;

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

    // Three foreign-currency ones, in two currencies. The stored amount is what
    // the service charges; the rupee figure beside it is always derived, so a rate
    // edit can never rewrite what last March cost.
    for (name, amount, currency, days) in [
        ("Claude Pro", 2_000i64, "USD", 11i64),
        ("GitHub Copilot", 1_000, "USD", 17),
        ("Proton Unlimited", 499, "EUR", 8),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.currency = currency.into();
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        recur::create(pool, &r).await?;
    }
    // Without a rate a foreign subscription is counted at 1:1, so the sample
    // supplies them — as *fetched* rates, dated a day old. That way the first visit
    // to the section replaces them with today's real ones, which is both more
    // accurate and the only way to see that the daily refresh works. Seeding them
    // as hand-typed would have exempted them from exactly that.
    crate::fx::apply_live(
        pool,
        &[("USD".to_string(), 83_600_000), ("EUR".to_string(), 90_500_000)],
        crate::schema::unix_now() - crate::fx::REFRESH_SECS,
    )
    .await?;

    // The rest of a realistic stack. Enough of them that the yearly total is a
    // number worth looking at, which is the argument for the tab existing.
    for (name, amount, days, cycle) in [
        ("Spotify", 11_900i64, 2i64, Cycle::Monthly),
        ("iCloud+ 200 GB", 7_500, 26, Cycle::Monthly),
        ("Amazon Prime", 149_900, 14, Cycle::Yearly),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.cycle = cycle;
        recur::create(pool, &r).await?;
    }

    // A second price rise, so the hike flag is a pattern rather than one row.
    let youtube = {
        let mut r = NewRecurrence::subscription("YouTube Premium", 14_900, &iso(ahead(18)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        recur::create(pool, &r).await?
    };
    recur::record_price(pool, youtube, 12_900, "INR", &iso(ago(500))).await?;
    recur::record_price(pool, youtube, 14_900, "INR", &iso(ago(210))).await?;

    // Entertainment-categorised, so the cross-section "paid for, not watched"
    // flag has something to find once Videos has playback history. It is the one
    // thing on this page a generic finance app could never say.
    {
        let mut r = NewRecurrence::subscription("JioHotstar", 29_900, &iso(ahead(22)));
        r.account_id = Some(card);
        r.category_id = Some(category(pool, "Entertainment").await?);
        recur::create(pool, &r).await?;
    }

    let paused = {
        let mut r = NewRecurrence::subscription("Gym", 150_000, &iso(ahead(20)));
        r.account_id = Some(bank);
        r.category_id = Some(health);
        recur::create(pool, &r).await?
    };
    recur::set_status(pool, paused, recur::Status::Paused).await?;

    // And one already stopped. Kept visible rather than deleted: money that used
    // to leave every month is context for the ones that still do.
    let cancelled = {
        let mut r = NewRecurrence::subscription("Adobe Creative Cloud", 191_500, &iso(ahead(28)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        recur::create(pool, &r).await?
    };
    recur::set_status(pool, cancelled, recur::Status::Cancelled).await?;

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
        r.category_id = Some(rent_cat);
        r.cycle = Cycle::Monthly;
        r.anchor_day = Some(1);
        r.reminder_days = 7;
        recur::create(pool, &r).await?;
    }

    // The rest of a household's fixed month. A calendar with three things on it
    // does not show the shape the Calendar tab exists to show — which is that
    // most of the outgo clears in one week.
    for (name, amount, days, auto, acct) in [
        ("Internet — ACT Fibernet", 118_000i64, 15i64, true, card),
        ("Housekeeping", 400_000, 1, false, cash),
        ("Phone — Airtel postpaid", 79_900, 12, true, card),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.kind = recur::RecurKind::Bill;
        r.account_id = Some(acct);
        r.category_id = Some(bills_cat);
        r.auto_post = auto;
        recur::create(pool, &r).await?;
    }

    // An irregular one, already paid, so the Paid filter and the variance line
    // are not empty states.
    {
        let gas = obligations::create_one_off(pool, "Gas cylinder — Indane", &iso(ago(11)), Some(115_000))
            .await?;
        obligations::mark_paid(pool, gas, 115_200, ago(11), wallet, "INR").await?;
    }

    // One that is late, because an overdue row is the state most worth seeing
    // work: it drives the badge, the sweep and the notification.
    obligations::create_one_off(pool, "Water bill", &iso(ago(6)), Some(84_000)).await?;

    obligations::create_one_off(pool, "Society maintenance", &iso(ahead(7)), Some(250_000)).await?;
    // A yearly one, far enough out to sit in the Calendar's year view rather than
    // this month's grid.
    obligations::create_one_off(pool, "Property tax — BBMP", &iso(ahead(64)), Some(840_000)).await?;

    // ── dues, in both directions ────────────────────────────────────────────
    dues::create(pool, &NewDue::lent("Ravi", 500_000, &iso(ago(34)), bank)).await?;
    dues::create(pool, &NewDue::lent("Ankit", 240_000, &iso(ago(23)), bank)).await?;
    dues::create(pool, &NewDue::borrowed("Dad", 1_500_000, &iso(ago(67)), bank)).await?;
    dues::create(pool, &NewDue::borrowed("Sneha", 185_000, &iso(ago(8)), bank)).await?;
    // A part-settled one, so the remainder-still-open path is exercised.
    let part = dues::create(pool, &NewDue::lent("Meera", 300_000, &iso(ago(20)), bank)).await?;
    dues::settle(pool, part, 100_000, ago(4), bank).await?;

    // Closed ones, so the Settled card has a history to show. The useful question
    // about a person is whether this comes back, and that needs the ones that did
    // sitting beside the ones that have not.
    for (person, amount, opened, closed) in
        [("Vikram", 320_000i64, 60i64, 49i64), ("Rohit", 800_000, 140, 45)]
    {
        let id = dues::create(pool, &NewDue::lent(person, amount, &iso(ago(opened)), bank)).await?;
        dues::settle(pool, id, amount, ago(closed), bank).await?;
    }
    // And one given up on. The only direction in which a due ever becomes a real
    // expense, which is worth having on screen once.
    {
        let id = dues::create(pool, &NewDue::lent("Karan", 120_000, &iso(ago(400)), bank)).await?;
        dues::write_off(pool, id, ago(30)).await?;
    }

    // ── budgets ─────────────────────────────────────────────────────────────
    // Six envelopes over six months, which is what the discipline grid is sized
    // for: one row per envelope, one column per month, and a habit visible along
    // a row. Three envelopes over four months left half of it blank.
    //
    // Eating out is deliberately set below what the ledger actually spends, so the
    // over-budget state — its colour, its pill and its Insights flag — is on screen
    // rather than only reachable by the user overspending.
    for k in 0..6i64 {
        let Some(period) = month_start(today, k).map(date::ym) else { continue };
        for (c, amount, rollover) in [
            (groceries, 800_000, true),
            (food, 250_000, false),
            (transport, 300_000, false),
            (shopping, 600_000, false),
            (health, 300_000, false),
            (fuel, 500_000, false),
        ] {
            budgets::set(pool, c, &period, amount, rollover).await?;
        }
    }

    // Bring the calendar and the badge up to date the same way a real launch
    // does, rather than leaving the seeded templates unmaterialised.
    recur::materialise_due(pool, today).await?;
    obligations::sweep(pool, today).await?;

    mark_new_rows(pool, &before).await?;
    Ok(true)
}

/// The tables a group is drawn from, and the SQL that finds the group's rows.
const TABLES: &[(&str, &str)] = &[
    ("transactions", "transactions"),
    ("obligations", "obligations"),
    ("recurrences", "recurrences"),
    ("dues", "dues"),
    ("budgets", "budgets"),
    ("accounts", "accounts"),
];

/// Highest row id in each table right now.
///
/// SQLite hands out `MAX(rowid) + 1`, so any row inserted after this call has an id
/// above the figure recorded here — which is what makes "everything newer than this
/// is sample data" sound even on a database that already had rows.
async fn high_water(pool: &SqlitePool) -> Result<Vec<(&'static str, i64)>> {
    let mut out = Vec::with_capacity(TABLES.len());
    for (kind, table) in TABLES {
        let sql = format!("SELECT COALESCE(MAX(id), 0) FROM {table}");
        out.push((*kind, sqlx::query_scalar::<_, i64>(&sql).fetch_one(pool).await?));
    }
    Ok(out)
}

/// Record every row added since `before` as sample data.
///
/// A sweep at the end rather than a `mark` beside each insert, because half these
/// rows are side effects: `dues::create` posts the lending transfer, `loans::create`
/// creates an account, `mark_paid` posts the payment, and `materialise_due` creates
/// obligations from the templates. Any one of them missed would leave an unmarked
/// row that reads as the user's own — and one of those is enough to make the very
/// first prune delete the entire sample.
///
/// Bounded by the high-water marks rather than marking the whole table, because
/// [`reseed`] can run on a database with the user's own rows in it, and marking one
/// of those as a sample would have the next prune delete their data.
async fn mark_new_rows(pool: &SqlitePool, before: &[(&str, i64)]) -> Result<()> {
    for (kind, floor) in before {
        let sql = match *kind {
            "transactions" => "SELECT id FROM transactions WHERE id > ?",
            "obligations" => "SELECT id FROM obligations WHERE id > ?",
            "recurrences" => "SELECT id FROM recurrences WHERE id > ?",
            "dues" => "SELECT id FROM dues WHERE id > ?",
            "budgets" => "SELECT id FROM budgets WHERE id > ?",
            // The two holding accounts are seeded for every user and are not samples.
            "accounts" => "SELECT id FROM accounts WHERE id > ? AND kind <> 'virtual'",
            _ => continue,
        };
        for id in sqlx::query_scalar::<_, i64>(sql).bind(floor).fetch_all(pool).await? {
            mark(pool, kind, id).await?;
        }
    }
    Ok(())
}

/// The first of the month `k` months before `today`.
///
/// Through `checked_sub_months` from the 1st rather than by subtracting 30 days:
/// day arithmetic skips a month entirely across a February and would leave a gap
/// in the chart it is meant to fill.
fn month_start(today: NaiveDate, k: i64) -> Option<NaiveDate> {
    today.with_day(1)?.checked_sub_months(chrono::Months::new(k.max(0) as u32))
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
        // Open in both directions, one part-settled, two settled and one written
        // off — the Settled card needs closed rows to have anything to show.
        assert_eq!(count(&p, "dues").await, 8);
        let closed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dues WHERE status <> 'open'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(closed, 3, "two settled and one written off");
        // Six envelopes across six months, which is what the discipline grid is
        // sized for.
        assert_eq!(count(&p, "budgets").await, 36);
        // Every one of the twelve chart columns has something in it. Six left five
        // empty bars, which reads as missing data rather than as a quiet year.
        let months = txn::monthly_totals(&p, 12).await.unwrap();
        assert_eq!(months.iter().filter(|m| m.income_minor > 0).count(), 12);
        assert_eq!(count(&p, "loans").await, 3, "home, car and phone");

        // The current month has income in it. Dating the salary a fixed number of
        // days back put it in the *previous* month for the first 25 days of every
        // month, which left the Overview with no income, a negative "saved this
        // month" and no savings rate.
        let (from, to) = date::month_bounds(&date::ym(today())).unwrap();
        assert!(txn::income_between(&p, &from, &to).await.unwrap() > 0, "the salary is inside the current month");

        // And enough categories this month for the donut to be a donut.
        let cats = txn::spend_by_category(&p, &from, &to).await.unwrap();
        assert!(cats.len() >= 6, "only {} categories this month", cats.len());
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
    async fn reseeding_never_marks_the_users_own_rows_as_samples() {
        // The dangerous direction of the Add button: seeding on top of real data
        // must not tag that data as a sample, or the next prune deletes it.
        let p = pool().await;
        let mine_acct = accounts::create(&p, &NewAccount::bank("My bank", 1_000_000)).await.unwrap();
        let mine = txn::post(
            &p,
            &NewTxn::expense(mine_acct, 12_300, "INR", "2026-07-25", "My own spend"),
        )
        .await
        .unwrap();

        // Auto-seeding declines, because the database is not empty.
        assert!(!seed(&p, today()).await.unwrap());
        // The button does not.
        assert!(reseed(&p, today()).await.unwrap());
        assert!(present(&p).await.unwrap());

        // Neither of the user's rows is marked.
        let marked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM demo_rows WHERE (kind = 'transactions' AND row_id = ?)
                                               OR (kind = 'accounts' AND row_id = ?)",
        )
        .bind(mine)
        .bind(mine_acct)
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(marked, 0);

        // So removing every sample leaves them standing.
        remove_all(&p).await.unwrap();
        assert_eq!(count(&p, "transactions").await, 1, "the user's posting survives");
        assert_eq!(count(&p, "accounts").await, 3, "their account plus the two virtual ones");
    }

    #[tokio::test]
    async fn reseeding_twice_does_not_leave_two_of_everything() {
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        let n = count(&p, "recurrences").await;
        reseed(&p, today()).await.unwrap();
        assert_eq!(count(&p, "recurrences").await, n, "the old samples went first");
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

    /// The states the new Accounts, Subscriptions and Budgets cards were built
    /// around. Each is invisible without data of exactly one shape, so a sample
    /// that happens to omit one silently ships an untested card.
    #[tokio::test]
    async fn the_sample_reaches_every_state_the_tabs_can_render() {
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        let rows = accounts::list(&p, false).await.unwrap();

        // One account checked against a statement, one never — the contrast is
        // what makes the reconcile line mean anything.
        assert!(rows.iter().any(|a| a.reconciled_on.is_some()), "something has been reconciled");
        assert!(
            rows.iter().any(|a| a.reconciled_on.is_none() && a.kind == AccountKind::Bank),
            "and something has not"
        );

        let (from, to) = date::month_bounds(&date::ym(today())).unwrap();
        let details = accounts::details(&p, &from, &to, today()).await.unwrap();
        // Cash came up short at its last count, which is what a real wallet does.
        assert!(
            details.iter().any(|d| d.drift_minor.is_some_and(|m| m != 0)),
            "the cash recount found a gap"
        );
        // The wallet says what tops it up.
        assert!(details.iter().any(|d| d.topped_from.is_some()), "a wallet has a top-up behind it");
        // The card has a statement date and a minimum to plan around.
        assert!(details.iter().any(|d| d.statement_on.is_some() && d.minimum_due_minor.is_some()));

        // Every subscription status, and more than one currency.
        let subs = recur::list(&p, Some(recur::RecurKind::Subscription), true).await.unwrap();
        for want in [recur::Status::Active, recur::Status::Paused, recur::Status::Cancelled] {
            assert!(subs.iter().any(|r| r.status == want), "no {want:?} subscription");
        }
        let mut currencies: Vec<&str> = subs.iter().map(|r| r.currency.as_str()).collect();
        currencies.sort_unstable();
        currencies.dedup();
        assert!(currencies.len() >= 3, "only {currencies:?} — the FX path needs more than one");
        assert!(
            subs.iter().filter(|r| r.hike_from_minor.is_some()).count() >= 2,
            "a single price rise reads as a one-off rather than as a pattern"
        );

        // The envelope grid: six months, every one of them with spending in every
        // budgeted category, or the row has a hole in it.
        let (periods, history) = budgets::envelope_history(&p, 6, today()).await.unwrap();
        assert_eq!(periods.len(), 6);
        assert!(history.len() >= 6, "only {} envelopes", history.len());
        for h in &history {
            // The current month is still running, so only the closed ones must be
            // complete.
            assert!(
                h.spent.iter().take(5).all(|c| c.is_some_and(|m| m > 0)),
                "{} has an empty month behind it",
                h.name
            );
        }

        // And something is actually over budget, so that state is on screen
        // rather than only reachable by the user overspending.
        let now = budgets::list(&p, &date::ym(today())).await.unwrap();
        assert!(
            now.iter().any(|b| b.id.is_some() && b.over_budget()),
            "no envelope is over, so the over-budget colour never shows"
        );
    }
}
