//! Sample data, so nine empty tabs are not the first thing the section shows.
//!
//! An empty finance section is indistinguishable from a broken one: every chart
//! is blank, every total is zero, and there is no way to tell a bug from a lack
//! of data. This seeds twelve plausible months of everything, once, on a
//! database that has never held anything real — twelve because every chart in
//! the section is a year wide, and a sample that filled three of its columns
//! read as missing data rather than as a quiet year.
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
    // The opening balances are what these accounts held when the ledger begins,
    // which is the month the oldest loan was taken out — thirty-two months back.
    // The bank's has to carry the instalments paid before the twelve months of
    // income below, or the sample opens overdrawn, which
    // `the_sample_month_is_internally_consistent` refuses to let ship.
    let bank = accounts::create(pool, &NewAccount::bank("HDFC Savings", 90_000_000)).await?;

    let cash = accounts::create(
        pool,
        &NewAccount { kind: AccountKind::Cash, ..NewAccount::bank("Cash", 500_000) },
    )
    .await?;

    // A card with a limit, so utilisation has something to show, and a statement
    // day, so the Accounts card can say when the bill lands.
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

    // A second bank, so the Accounts grid is a grid and the ledger's account
    // filter has something to narrow to. Left un-reconciled on purpose: "never
    // checked" beside two accounts that have been is the contrast that makes the
    // reconcile line mean anything.
    let savings = accounts::create(pool, &NewAccount::bank("SBI Emergency fund", 30_000_000)).await?;

    // ── categories ──────────────────────────────────────────────────────────
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
    let entertainment = category(pool, "Entertainment").await?;
    let travel = category(pool, "Travel").await?;
    let education = category(pool, "Education").await?;
    let gifts = category(pool, "Gifts & Donations").await?;
    let insurance = category(pool, "Insurance").await?;
    let fees = category(pool, "Fees & Charges").await?;
    let salary = category(pool, "Salary").await?;
    let freelance = category(pool, "Freelance").await?;
    let interest_cat = category(pool, "Interest").await?;

    // ── loans, which are accounts with a negative balance ───────────────────
    // Three at different stages, and each with every instalment it has actually
    // paid posted into the ledger — the progress bar, the principal-and-interest
    // split and "28 of 60 left" are all read back off those postings, so a loan
    // seeded without them would show a bar at zero on a loan years old.
    //
    // The counts are chosen so the share repaid reads plausibly against the share
    // of the term elapsed. Every rupee of an instalment reduces the balance here,
    // interest included, so a long loan's bar runs ahead of its calendar.
    let mut emi_events: Vec<(i64, i64, NaiveDate)> = Vec::new();
    for (name, principal, rate_bp, tenure, started_k, paid, emi_day, fixed_emi) in [
        ("Home loan", 350_000_000i64, 840i64, 240i64, 32i64, 32i64, 5i64, None),
        ("Car loan", 80_000_000, 950, 60, 28, 28, 7, None),
        // A no-cost EMI, so the card that says "₹0 interest" is not dead code.
        ("Phone EMI", 9_600_000, 0, 12, 9, 9, 10, Some(800_000i64)),
    ] {
        let Some(started) = month_start(today, started_k) else { continue };
        let id = loans::create(
            pool,
            &NewLoan {
                name: name.into(),
                principal_minor: principal,
                rate_bp,
                tenure_months: tenure,
                started_on: iso(started + Duration::days(emi_day - 1)),
                emi_minor: fixed_emi,
                emi_day,
                currency: "INR".into(),
            },
        )
        .await?;
        let emi = loans::get(pool, id).await?.map(|l| l.emi_minor).unwrap_or(0);
        for n in 0..paid {
            let Some(month) = month_start(today, started_k - n) else { continue };
            let on = month + Duration::days(emi_day - 1);
            if on <= today {
                emi_events.push((id, emi, on));
            }
        }
    }

    // ── twelve months of ledger ─────────────────────────────────────────────
    // One household month, repeated with variation. Twelve of them, because every
    // chart in the section is twelve columns wide and a sample that filled three
    // of them read as missing data rather than as a quiet year.
    //
    // `every` is what keeps the twelve from being twelve copies: 1 is every month,
    // 2 every other, 12 once a year. Between that and the jitter below, no two
    // months have the same shape or the same height.
    let month_lines: &[(i64, i64, &str, i64, i64, i64)] = &[
        // day, amount, description, category, account, every-n-months
        (2, 3_800_000, "Rent", rent_cat, bank, 1),
        // Groceries, across all four ways a household actually buys them.
        (3, 462_000, "BigBasket — monthly stock-up", groceries, bank, 1),
        (8, 118_000, "Blinkit", groceries, card, 1),
        (15, 96_000, "Zepto", groceries, wallet, 1),
        (22, 84_000, "Blinkit", groceries, card, 1),
        (26, 132_000, "More Supermarket", groceries, bank, 1),
        // Eating out. Repeat merchants on purpose: a description the ledger has
        // seen before is what lets an imported statement categorise itself, and a
        // sample where every line is unique would never exercise that.
        (5, 64_000, "Swiggy", food, card, 1),
        (9, 128_000, "Dinner — Toit", food, card, 2),
        (12, 24_000, "Chai and samosa", food, cash, 1),
        (17, 52_000, "Swiggy", food, card, 1),
        (20, 88_000, "Third Wave Coffee", food, card, 1),
        (24, 268_000, "Swiggy — weekend", food, card, 1),
        (27, 68_000, "Lunch — Meghana", food, cash, 1),
        // Getting about.
        (4, 41_000, "Auto to office", transport, cash, 1),
        (11, 78_000, "Uber to airport", transport, wallet, 2),
        (18, 45_000, "Metro card recharge", transport, bank, 1),
        (25, 34_000, "Auto to office", transport, cash, 1),
        (7, 89_900, "Petrol — Indian Oil", fuel, card, 1),
        (21, 121_000, "Petrol — Shell", fuel, card, 1),
        // The fixed month.
        (6, 84_000, "Water — Bisleri cans", bills_cat, cash, 1),
        (13, 118_000, "Internet — ACT Fibernet", bills_cat, card, 1),
        (14, 79_900, "Phone — Airtel postpaid", bills_cat, card, 1),
        (19, 400_000, "Housekeeping", bills_cat, cash, 1),
        // Subscriptions, as charges rather than only as templates: the donut and
        // the six-month trend read the ledger, not the Subscriptions tab.
        (5, 64_900, "Netflix", subs_cat, card, 1),
        (10, 11_900, "Spotify", subs_cat, card, 1),
        (16, 14_900, "YouTube Premium", subs_cat, card, 1),
        (23, 29_900, "JioHotstar", entertainment, card, 1),
        (26, 7_500, "iCloud+ 200 GB", subs_cat, card, 1),
        // The discretionary half.
        (9, 356_000, "Myntra", shopping, card, 2),
        (16, 268_000, "Nykaa — skincare", shopping, card, 1),
        (23, 210_000, "Decathlon", shopping, card, 3),
        (10, 245_000, "Apollo Pharmacy", health, bank, 1),
        (18, 129_000, "Cult.fit", health, card, 2),
        (27, 320_000, "Dr Rao — consultation", health, bank, 3),
        // And the ones that only turn up now and then, which is what makes a
        // twelve-month chart have a shape at all.
        (12, 1_240_000, "IndiGo — flight home", travel, card, 4),
        (20, 899_000, "Coursera Plus", education, card, 6),
        (24, 250_000, "Birthday gift", gifts, card, 3),
        (11, 1_850_000, "Term insurance premium", insurance, bank, 12),
        (28, 5_900, "Card fee — GST", fees, card, 6),
    ];

    let mut rows: Vec<(NewTxn, Option<i64>, TxnKind)> = Vec::new();
    // Each month's card charges, and the day the bill was settled. A card that is
    // only ever spent on carries a year of charges into the present and shows a
    // utilisation bar several times past its own limit; a real one is cleared each
    // month, and clearing it is a transfer, not spending.
    let mut card_bills: Vec<(NaiveDate, i64)> = Vec::new();
    let mut transfers: Vec<(i64, i64, i64, NaiveDate, &str)> = Vec::new();

    for k in (0..12i64).rev() {
        let Some(month) = month_start(today, k) else { continue };
        let day_of = |d: i64| month + Duration::days(d - 1);

        // Income first, so the running balance reads the way a real month does.
        // On the 1st, so it is inside its own month whatever day the app is first
        // opened on — a fixed number of days back put the salary in the *previous*
        // month for most of every month, which left the Overview with no income, a
        // negative "saved this month" and no savings rate.
        let pay = 18_500_000 + jitter(k, "salary", 150_000);
        rows.push((
            NewTxn::expense(bank, pay, "INR", &iso(day_of(1)), "Salary — Acme Corp"),
            Some(salary),
            TxnKind::Income,
        ));
        // A second income, four times a year. One source of money all year is a
        // salary; two is a household, and the Insights savings figures are more
        // interesting when income moves as well as spending.
        if k % 3 == 0 && day_of(14) <= today {
            rows.push((
                NewTxn::expense(
                    bank,
                    2_800_000 + jitter(k, "freelance", 900_000),
                    "INR",
                    &iso(day_of(14)),
                    "Freelance — design retainer",
                ),
                Some(freelance),
                TxnKind::Income,
            ));
        }
        // Quarterly interest on the emergency fund, which is the only income that
        // does not arrive in the current account.
        if k % 3 == 1 && day_of(28) <= today {
            rows.push((
                NewTxn::expense(savings, 62_000 + jitter(k, "interest", 9_000), "INR", &iso(day_of(28)), "Savings interest"),
                Some(interest_cat),
                TxnKind::Income,
            ));
        }

        let mut card_spend = 0i64;
        for (day, base, desc, cat, acct, every) in month_lines {
            if k % every != 0 {
                continue;
            }
            let on = day_of(*day);
            // Nothing in the future: a sample that posts spending that has not
            // happened makes every month-to-date total a lie.
            if on > today {
                continue;
            }
            let amount = (base + jitter(k, desc, base / 5)).max(1_000);
            if *acct == card {
                card_spend += amount;
            }
            rows.push((
                NewTxn::expense(*acct, amount, "INR", &iso(on), desc),
                Some(*cat),
                TxnKind::Expense,
            ));
        }

        // The month's own housekeeping: cash out of the machine, money onto the
        // wallet, money into the emergency fund. All three are transfers, none of
        // them is spending, and without them a year of cash and wallet postings
        // would drain two accounts that opened with ₹5,000 each.
        for (to, amount, day, desc) in [
            (cash, 800_000i64, 6i64, "ATM withdrawal"),
            (wallet, 200_000, 4, "Paytm top-up"),
            (savings, 1_500_000, 2, "Monthly saving"),
        ] {
            let on = day_of(day);
            if on <= today {
                transfers.push((bank, to, amount, on, desc));
            }
        }

        // Each closed month's card bill, settled at the end of it — the 28th,
        // which is past the last charge in the table, so a closed month clears
        // exactly. The current month's charges stay outstanding, which is what a
        // card is meant to look like and what keeps the utilisation bar inside
        // its own limit.
        if card_spend > 0 && k > 0 {
            card_bills.push((day_of(28), card_spend));
        }
    }

    for (t, cat, kind) in rows {
        let mut t = t;
        t.kind = kind;
        t.category_id = cat;
        t.source = "manual".into();
        txn::post(pool, &t).await?;
    }
    for (from, to, amount, on, desc) in transfers {
        txn::post(pool, &NewTxn::transfer(from, to, amount, "INR", &iso(on), desc)).await?;
    }
    for (on, amount) in card_bills {
        if on <= today {
            txn::post(
                pool,
                &NewTxn::transfer(bank, card, amount, "INR", &iso(on), "ICICI card payment"),
            )
            .await?;
        }
    }
    // The instalments, after the income that pays them.
    for (loan_id, emi, on) in emi_events {
        if emi > 0 {
            loans::pay_emi(pool, loan_id, bank, emi, on).await?;
        }
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
    // One that auto-posts, three in foreign currency, one paused, one cancelled
    // and two whose price went up — every state the tab has to render.
    let netflix = {
        let mut r = NewRecurrence::subscription("Netflix", 64_900, &iso(ahead(5)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        // The plan, which is what the table prints under the name. Every sample
        // subscription carries one: a row that says only "₹649" does not tell you
        // which of the four Netflix tiers is being paid for.
        r.note = Some("Standard".into());
        r.auto_post = true;
        r.anchor_day = Some(ahead(5).day() as i64);
        recur::create(pool, &r).await?
    };
    // The hike this app exists to make visible: ₹499 → ₹649 three months ago.
    recur::record_price(pool, netflix, 49_900, "INR", &iso(ago(400))).await?;
    recur::record_price(pool, netflix, 64_900, "INR", &iso(ago(92))).await?;

    // Foreign currency, in two currencies. The stored amount is what the service
    // charges; the rupee figure beside it is always derived, so a rate edit can
    // never rewrite what last March cost.
    for (name, amount, currency, days, plan) in [
        ("Claude Pro", 2_000i64, "USD", 11i64, "Pro"),
        ("GitHub Copilot", 1_000, "USD", 17, "Individual"),
        ("Proton Unlimited", 499, "EUR", 8, "Unlimited"),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.currency = currency.into();
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.note = Some(plan.into());
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
    for (name, amount, days, cycle, plan) in [
        ("Spotify", 11_900i64, 2i64, Cycle::Monthly, "Individual"),
        ("iCloud+ 200 GB", 7_500, 26, Cycle::Monthly, "200 GB"),
        ("Amazon Prime", 149_900, 14, Cycle::Yearly, "Annual"),
        ("Google One 2 TB", 21_000, 9, Cycle::Monthly, "2 TB"),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.cycle = cycle;
        r.note = Some(plan.into());
        recur::create(pool, &r).await?;
    }

    // A second price rise, so the hike flag is a pattern rather than one row.
    let youtube = {
        let mut r = NewRecurrence::subscription("YouTube Premium", 14_900, &iso(ahead(18)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.note = Some("Individual".into());
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
        r.category_id = Some(entertainment);
        r.note = Some("Super".into());
        recur::create(pool, &r).await?;
    }

    let paused = {
        let mut r = NewRecurrence::subscription("Gym — Cult Elite", 150_000, &iso(ahead(20)));
        r.account_id = Some(bank);
        r.category_id = Some(health);
        r.note = Some("Annual".into());
        recur::create(pool, &r).await?
    };
    recur::set_status(pool, paused, recur::Status::Paused).await?;

    // And one already stopped. Kept visible rather than deleted: money that used
    // to leave every month is context for the ones that still do.
    let cancelled = {
        let mut r = NewRecurrence::subscription("Adobe Creative Cloud", 191_500, &iso(ahead(28)));
        r.account_id = Some(card);
        r.category_id = Some(subs_cat);
        r.note = Some("All Apps".into());
        recur::create(pool, &r).await?
    };
    recur::set_status(pool, cancelled, recur::Status::Cancelled).await?;

    // ── bills ───────────────────────────────────────────────────────────────
    // A variable one with a year of history, so the estimate is a real mean and
    // the estimate-versus-actual variance line has something to measure.
    let power = {
        let mut r = NewRecurrence::variable_bill("Electricity — BESCOM", &iso(ahead(9)));
        r.account_id = Some(bank);
        r.category_id = Some(bills_cat);
        r.reminder_days = 5;
        recur::create(pool, &r).await?
    };
    for k in 1..=11i64 {
        let Some(month) = month_start(today, k) else { continue };
        let on = month + Duration::days(15);
        if on > today {
            continue;
        }
        let obl =
            obligations::create_one_off(pool, "Electricity — BESCOM", &iso(on), None).await?;
        sqlx::query("UPDATE obligations SET recurrence_id = ? WHERE id = ?")
            .bind(power)
            .bind(obl)
            .execute(pool)
            .await?;
        // Summer bills are the high ones. A flat series would make the estimate
        // exact and the variance card empty.
        let amount = 300_000 + jitter(k, "power", 90_000);
        obligations::mark_paid(pool, obl, amount, on, bank, "INR").await?;
    }

    {
        // A bill, not a subscription. Both are recurrences, but rent is the
        // household's largest fixed obligation and belongs beside the electricity
        // and the phone — not in a list of things that can be cancelled.
        let mut r = NewRecurrence::subscription("Rent", 3_800_000, &iso(ahead(4)));
        r.kind = recur::RecurKind::Bill;
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
        ("Water — Bisleri cans", 84_000, 6, false, cash),
    ] {
        let mut r = NewRecurrence::subscription(name, amount, &iso(ahead(days)));
        r.kind = recur::RecurKind::Bill;
        r.account_id = Some(acct);
        r.category_id = Some(bills_cat);
        r.auto_post = auto;
        recur::create(pool, &r).await?;
    }

    // Irregular ones, already paid, so the Paid filter and the variance line are
    // not empty states.
    for (name, due, paid, acct) in [
        ("Gas cylinder — Indane", 11i64, 115_200i64, wallet),
        ("Car service — Bosch", 38, 742_000, card),
        ("Dentist — root canal", 61, 950_000, bank),
    ] {
        let id = obligations::create_one_off(pool, name, &iso(ago(due)), Some(paid)).await?;
        obligations::mark_paid(pool, id, paid, ago(due), acct, "INR").await?;
    }

    // One that is late, because an overdue row is the state most worth seeing
    // work: it drives the badge, the sweep and the notification.
    obligations::create_one_off(pool, "Society maintenance", &iso(ago(6)), Some(250_000)).await?;

    // And a few still ahead, so the Calendar has something in front of it as well
    // as behind. The last is far enough out to sit in the year view rather than
    // this month's grid.
    for (name, days, amount) in [
        ("Broadband — annual plan", 9i64, 899_000i64),
        ("Car insurance renewal", 26, 1_420_000),
        ("Property tax — BBMP", 64, 840_000),
    ] {
        obligations::create_one_off(pool, name, &iso(ahead(days)), Some(amount)).await?;
    }

    // ── lending, in both directions ─────────────────────────────────────────
    // Every one carries a note. Without one the NOTE column is a wall of blanks
    // and the tab reads as a list of amounts against names — which is exactly
    // the thing a person cannot remember six weeks later.
    let lending = |person: &str, amount: i64, days: i64, note: &str, borrowed: bool| {
        let mut d = if borrowed {
            NewDue::borrowed(person, amount, &iso(ago(days)), bank)
        } else {
            NewDue::lent(person, amount, &iso(ago(days)), bank)
        };
        d.note = Some(note.to_string());
        d
    };

    for (person, amount, days, note, borrowed) in [
        ("Ravi", 500_000i64, 34i64, "Covered his half of the Goa trip", false),
        ("Ankit", 240_000, 23, "Concert tickets, said he'd transfer", false),
        ("Dad", 1_500_000, 67, "Towards the car down payment", true),
        ("Sneha", 185_000, 8, "She paid the whole dinner bill", true),
        ("Imran", 60_000, 3, "Cab to the airport, split later", false),
    ] {
        dues::create(pool, &lending(person, amount, days, note, borrowed)).await?;
    }

    // A part-settled one, so the remainder-still-open path is exercised.
    let part = dues::create(
        pool,
        &lending("Meera", 300_000, 20, "Deposit for the shared studio", false),
    )
    .await?;
    dues::settle(pool, part, 100_000, ago(4), bank).await?;

    // Closed ones, so the Squared-up card has a history to show. The useful
    // question about a person is whether this comes back, and that needs the ones
    // that did sitting beside the ones that have not. One of them is a borrowing
    // paid off, which is the only way that card's "You paid them back" is seen.
    for (person, amount, opened, closed, note, borrowed) in [
        ("Vikram", 320_000i64, 60i64, 49i64, "Lent for his laptop repair", false),
        ("Rohit", 800_000, 140, 45, "Bridged him to payday", false),
        ("Priya", 450_000, 95, 62, "She fronted the flight booking", true),
    ] {
        let id = dues::create(pool, &lending(person, amount, opened, note, borrowed)).await?;
        dues::settle(pool, id, amount, ago(closed), bank).await?;
    }
    // And one given up on. The only direction in which lending ever becomes a real
    // expense, which is worth having on screen once.
    {
        let id = dues::create(
            pool,
            &lending("Karan", 120_000, 400, "Never coming back — written off", false),
        )
        .await?;
        dues::write_off(pool, id, ago(30)).await?;
    }

    // ── budgets ─────────────────────────────────────────────────────────────
    // Six envelopes over twelve months, so the discipline grid is full along its
    // whole width rather than trailing off after six.
    //
    // Eating out and Shopping are deliberately set below what the ledger actually
    // spends, so the over-budget state — its colour, its pill, its Insights flag
    // and the health score's "envelopes held" component — are all on screen rather
    // than only reachable by the user overspending.
    for k in 0..12i64 {
        let Some(period) = month_start(today, k).map(date::ym) else { continue };
        for (c, amount, rollover) in [
            (groceries, 950_000, true),
            (food, 500_000, false),
            (transport, 250_000, false),
            (shopping, 400_000, false),
            (health, 400_000, false),
            (fuel, 250_000, false),
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
        out.push((*kind, sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*sql)).fetch_one(pool).await?));
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

/// A repeatable wobble, so twelve months differ from one another.
///
/// Not a random number generator and not trying to be one: it is a fixed mix of
/// the month index and the line's own description, which means the sample looks
/// varied and still comes out identical on two runs of the same day. A real RNG
/// here would make every screenshot, every test and every bug report describe a
/// different set of numbers.
///
/// Returns something in `-spread ..= spread`.
fn jitter(month: i64, name: &str, spread: i64) -> i64 {
    if spread <= 0 {
        return 0;
    }
    let seed = name.bytes().map(i64::from).sum::<i64>().wrapping_mul(2_654_435_761)
        ^ month.wrapping_mul(40_503);
    seed.rem_euclid(2 * spread + 1) - spread
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
    mark_descendants(pool).await?;
    let mut removed = 0;
    for kind in GROUPS {
        if real_rows(pool, kind).await? > 0 {
            removed += drop_group(pool, kind).await?;
        }
    }
    Ok(removed)
}

/// Empty the section: every account, posting, template, due, budget and loan,
/// sample or not.
///
/// This is what the "Remove all of it" button does, and it is deliberately
/// blunter than [`remove_all`]. The surgical version can only reach rows it
/// marked, and anything grown from them since — a bill materialised on a later
/// launch, a payment posted by hand against a sample subscription — held a
/// sample account open and left part of the section still full, which is not
/// what a button called "remove all of it" is understood to mean.
///
/// What is kept, because none of it is money: the category tree (configuration,
/// and the app reseeds nothing without it), import presets (a column mapping is
/// not a transaction) and the two virtual holding accounts that make lending
/// work. Exchange rates go, and the daily monitor puts them back.
///
/// One statement per table, in one transaction with the foreign keys deferred:
/// the row-at-a-time path had to guess a deletion order and reported whatever it
/// got wrong as a bare "FOREIGN KEY constraint failed".
pub async fn wipe(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("PRAGMA defer_foreign_keys = ON").execute(&mut *tx).await?;
    for sql in [
        "DELETE FROM transactions",
        "DELETE FROM obligations",
        "DELETE FROM recurrence_prices",
        "DELETE FROM recurrences",
        "DELETE FROM dues",
        "DELETE FROM budgets",
        "DELETE FROM loans",
        "DELETE FROM accounts WHERE kind <> 'virtual'",
        "DELETE FROM fx_rates",
        "DELETE FROM demo_rows",
    ] {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    // The seeded marker goes straight back. Without it the next launch finds an
    // empty database that has "never been seeded" and helpfully fills it with
    // sample data again — which is the section undoing a button the user pressed
    // on purpose. Adding it back is what the Add button is for.
    sqlx::query("INSERT OR IGNORE INTO demo_rows (kind, row_id) VALUES (?, 1)")
        .bind(SEEDED)
        .execute(&mut *tx)
        .await?;
    // The holding accounts survive, so their balances have to be brought back to
    // zero by hand — every due that made them non-zero has just gone.
    sqlx::query("UPDATE accounts SET opening_minor = 0 WHERE kind = 'virtual'")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Remove every sample row, in dependency order.
pub async fn remove_all(pool: &SqlitePool) -> Result<u64> {
    mark_descendants(pool).await?;
    let mut removed = 0;
    for kind in GROUPS {
        removed += drop_group(pool, kind).await?;
    }
    forget_unused_rates(pool).await?;
    Ok(removed)
}

/// Sample rows go on producing more sample rows after the seed run.
///
/// A seeded subscription keeps working: every launch materialises its next
/// obligation and, if it auto-posts, books the charge. Those rows are created
/// long after [`mark_new_rows`] swept, so nothing knew they were samples — and a
/// single one of them is enough to hold a sample account open, which left the
/// banner up and part of the section still full after "Remove all of it".
///
/// Parentage only, never "everything in a sample account": the user's own
/// posting into a sample account is theirs, and
/// `a_real_transaction_in_a_sample_account_is_never_destroyed` says so.
async fn mark_descendants(pool: &SqlitePool) -> Result<()> {
    // Obligations first, so the transactions sweep below can see the ones it
    // has just adopted.
    sqlx::query(
        "INSERT OR IGNORE INTO demo_rows (kind, row_id)
         SELECT 'obligations', o.id FROM obligations o
          WHERE EXISTS (SELECT 1 FROM demo_rows d
                         WHERE d.kind = 'recurrences' AND d.row_id = o.recurrence_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO demo_rows (kind, row_id)
         SELECT 'transactions', t.id FROM transactions t
          WHERE EXISTS (SELECT 1 FROM demo_rows d
                         WHERE d.kind = 'recurrences' AND d.row_id = t.recurrence_id)
             OR EXISTS (SELECT 1 FROM demo_rows d
                         WHERE d.kind = 'obligations' AND d.row_id = t.obligation_id)
             OR EXISTS (SELECT 1 FROM demo_rows d
                         WHERE d.kind = 'dues' AND d.row_id = t.due_id)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop exchange rates for currencies nothing is left in.
///
/// The sample seeds a USD and a EUR rate for its foreign subscriptions. Rates are
/// not owned by any group, so removing the subscriptions left the two rows behind
/// and the currency list still looked seeded. Keyed on what is actually still
/// priced in that currency rather than on a marker, so a rate the user fetched
/// for a currency they really use survives — and one dropped here is a click and
/// a daily refresh away from coming back.
async fn forget_unused_rates(pool: &SqlitePool) -> Result<()> {
    let base = crate::fx::base_currency().to_uppercase();
    let orphans: Vec<String> = sqlx::query_scalar(
        "SELECT code FROM fx_rates
          WHERE UPPER(code) <> ?
            -- A rate the user typed is a deliberate act, even for a currency
            -- they have not spent in yet. Only fetched ones are swept.
            AND source <> 'manual'
            AND NOT EXISTS (SELECT 1 FROM transactions WHERE UPPER(currency) = UPPER(fx_rates.code))
            AND NOT EXISTS (SELECT 1 FROM recurrences  WHERE UPPER(currency) = UPPER(fx_rates.code))
            AND NOT EXISTS (SELECT 1 FROM accounts     WHERE UPPER(currency) = UPPER(fx_rates.code))",
    )
    .bind(&base)
    .fetch_all(pool)
    .await?;
    for code in orphans {
        crate::fx::remove(pool, &code).await?;
    }
    Ok(())
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
        sqlx::query_scalar(sqlx::AssertSqlSafe(&*sql)).fetch_one(p).await.unwrap()
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
        // Open in both directions, one part-settled, three settled and one written
        // off — the Squared-up card needs closed rows to have anything to show,
        // and one of the settled ones is a borrowing so its "You paid them back"
        // branch is not shipped untested.
        assert_eq!(count(&p, "dues").await, 10);
        let closed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dues WHERE status <> 'open'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(closed, 4, "three settled and one written off");
        let settled_borrowing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dues WHERE status = 'settled' AND direction = 'i_owe'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(settled_borrowing, 1, "a borrowing that was paid off");
        // Six envelopes across twelve months, so the discipline grid is full
        // along its whole width rather than trailing off halfway.
        assert_eq!(count(&p, "budgets").await, 72);
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
        // Including the rates the foreign subscriptions needed. These belong to no
        // group, so nothing used to remove them and the currency list stayed
        // seeded long after everything it priced was gone.
        assert_eq!(count(&p, "fx_rates").await, 0);

        // And it does not come back.
        assert!(!seed(&p, today()).await.unwrap());
    }

    #[tokio::test]
    async fn the_reset_empties_the_section_including_what_the_user_put_there() {
        // The blunt one, behind "Remove all of it". The surgical `remove_all` is
        // required to keep the user's own rows; this is required to take them,
        // because a button with that label that leaves half a section behind is
        // the complaint it exists to answer.
        let p = pool().await;
        seed(&p, today()).await.unwrap();
        let mine = accounts::create(&p, &NewAccount::bank("My bank", 1_000_000)).await.unwrap();
        txn::post(&p, &NewTxn::expense(mine, 12_300, "INR", "2026-07-25", "Mine")).await.unwrap();

        wipe(&p).await.unwrap();

        for table in [
            "transactions",
            "recurrences",
            "recurrence_prices",
            "obligations",
            "dues",
            "budgets",
            "loans",
            "fx_rates",
        ] {
            assert_eq!(count(&p, table).await, 0, "{table} survived the reset");
        }
        // The two virtual holding accounts stay: lending is built on them, and
        // they hold no money of their own once the dues behind them are gone.
        assert_eq!(count(&p, "accounts").await, 2);
        // The category tree is configuration, not money, and seeding needs it.
        assert!(count(&p, "categories").await > 0);
        assert!(!present(&p).await.unwrap(), "the banner has to clear");
        // And it stays empty. An emptied section that refills itself on the next
        // launch has overruled the button that emptied it.
        assert!(!seed(&p, today()).await.unwrap());
        // The Add button is the way back, and it still works.
        assert!(reseed(&p, today()).await.unwrap());
    }

    #[tokio::test]
    async fn what_the_sample_goes_on_producing_is_removed_with_it() {
        // The leak this guards. A seeded subscription keeps working after the seed
        // run: the next launch materialises its obligation and auto-posts the
        // charge. Those rows are created long after the marking sweep, so nothing
        // knew they were samples — and one of them was enough to hold the sample
        // card account open, which left the banner up and the Accounts tab still
        // full after "Remove all of it".
        let p = pool().await;
        seed(&p, today()).await.unwrap();

        // A month of launches, which is what actually produces them.
        for k in 1..=30i64 {
            let on = today() + Duration::days(k);
            recur::materialise_due(&p, on).await.unwrap();
            obligations::sweep(&p, on).await.unwrap();
        }
        let after = count(&p, "obligations").await;
        assert!(after > 0, "nothing was materialised, so this proves nothing");

        remove_all(&p).await.unwrap();

        assert!(!present(&p).await.unwrap(), "the banner has to clear");
        assert_eq!(count(&p, "obligations").await, 0);
        assert_eq!(count(&p, "transactions").await, 0);
        assert_eq!(count(&p, "accounts").await, 2, "and the accounts they held open are gone");
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
