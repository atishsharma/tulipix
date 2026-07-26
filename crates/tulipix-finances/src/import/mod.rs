//! Getting a bank statement into the ledger.
//!
//! The pipeline is deliberately two-stage: **parse, show, then post.** A file is
//! read into [`csv::RawRow`]s and previewed with its column mapping and date
//! format on screen; nothing reaches the database until the user says so. Getting
//! debit and credit backwards inverts every row in a file and the totals still
//! look plausible, so a silent one-shot import is the wrong shape here.
//!
//! Re-importing an overlapping statement adds nothing. That is not implemented in
//! this module — it is the `ux_txn_dedup` unique index doing it at the storage
//! layer, which means it holds for manual entry and OCR too, not just for the path
//! someone remembered to write a check on.

pub mod csv;
pub mod dates;
pub mod detect;
pub mod ofx;

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;

use crate::txn::{self, NewTxn, TxnKind};
use csv::{ColumnMap, RawRow};
use dates::DateFormat;

/// What a file looks like before any of it is posted.
#[derive(Clone, Debug)]
pub struct Preview {
    pub rows: Vec<RawRow>,
    /// Lines that were not transactions: headers, balance summaries, blanks.
    pub unreadable: usize,
    pub column_map: ColumnMap,
    /// `None` when the file's dates are genuinely ambiguous and the user has to
    /// choose. Never guessed.
    pub date_format: Option<DateFormat>,
    pub currency: String,
    pub kind: FileKind,
}

impl Preview {
    pub fn debits(&self) -> usize {
        self.rows.iter().filter(|r| !r.is_credit).count()
    }

    pub fn credits(&self) -> usize {
        self.rows.iter().filter(|r| r.is_credit).count()
    }

    /// Earliest and latest date in the file.
    pub fn span(&self) -> Option<(String, String)> {
        let mut dates: Vec<&str> = self.rows.iter().map(|r| r.occurred_on.as_str()).collect();
        dates.sort_unstable();
        Some((dates.first()?.to_string(), dates.last()?.to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Csv,
    Ofx,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Csv => "csv",
            FileKind::Ofx => "ofx",
        }
    }
}

/// Which kind of file this is, by looking at it rather than at its extension.
///
/// A `.qfx` renamed to `.csv` is common enough, and the extension is the least
/// reliable thing about a downloaded statement.
pub fn sniff_kind(text: &str) -> FileKind {
    let head: String = text.chars().take(2048).collect();
    if head.contains("<STMTTRN>") || head.contains("OFXHEADER") || head.contains("<OFX>") {
        FileKind::Ofx
    } else {
        FileKind::Csv
    }
}

/// Read a file into a preview, guessing the mapping and sniffing the date format.
///
/// `override_map` and `override_format` let the import sheet re-run this after the
/// user corrects something, which is the whole point of previewing.
pub fn preview(
    text: &str,
    account_currency: &str,
    override_map: Option<&ColumnMap>,
    override_format: Option<DateFormat>,
) -> Result<Preview> {
    if text.trim().is_empty() {
        bail!("that file is empty");
    }
    match sniff_kind(text) {
        FileKind::Ofx => {
            let (rows, unreadable, currency) = ofx::parse(text, account_currency)?;
            Ok(Preview {
                rows,
                unreadable,
                column_map: ColumnMap::default(),
                // OFX states its own format, so there is nothing to sniff and
                // nothing for the user to choose.
                date_format: Some(DateFormat::Iso),
                currency,
                kind: FileKind::Ofx,
            })
        }
        FileKind::Csv => {
            let headers = csv::headers(text)?;
            let map = match override_map {
                Some(m) => m.clone(),
                None => csv::guess(&headers),
            };
            let fmt = match override_format {
                Some(f) => Some(f),
                None => {
                    let samples = csv::date_samples(text, &map, 60).unwrap_or_default();
                    dates::sniff(&samples)
                }
            };
            // With no format decided there is nothing to parse yet; the sheet asks
            // and calls back. An empty row list with a `None` format is the signal.
            let (rows, unreadable) = match fmt {
                Some(f) => csv::parse(text, &map, f, account_currency)?,
                None => (Vec::new(), 0),
            };
            Ok(Preview {
                rows,
                unreadable,
                column_map: map,
                date_format: fmt,
                currency: account_currency.to_uppercase(),
                kind: FileKind::Csv,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ingested {
    pub added: usize,
    /// Rows the dedup index already had. A re-import is all of them.
    pub duplicates: usize,
}

/// Post a previewed file into one account.
///
/// Debits become expenses and credits become income. Neither becomes a transfer:
/// a statement cannot tell a top-up of your own wallet from a payment to a shop,
/// and guessing wrong would hide real spending. Re-categorising an imported row as
/// a transfer is a deliberate act in the ledger.
pub async fn ingest(pool: &SqlitePool, account_id: i64, p: &Preview) -> Result<Ingested> {
    if p.rows.is_empty() {
        bail!("there is nothing in that file to import");
    }
    let base = crate::fx::base_currency();
    let rate = crate::fx::rate_for(pool, &p.currency, &base).await?;
    let mut out = Ingested::default();

    for row in &p.rows {
        let mut t = NewTxn::expense(
            account_id,
            row.amount_minor,
            &p.currency,
            &row.occurred_on,
            &row.description,
        );
        t.kind = if row.is_credit { TxnKind::Income } else { TxnKind::Expense };
        t.rate_micro = rate;
        t.source = p.kind.as_str().to_string();
        let (_, is_new) = txn::post_reporting(pool, &t).await?;
        if is_new {
            out.added += 1;
        } else {
            out.duplicates += 1;
        }
    }
    Ok(out)
}

// ── per-bank presets ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: i64,
    pub bank_name: String,
    pub column_map: ColumnMap,
    pub date_format: DateFormat,
    pub account_id: Option<i64>,
}

pub async fn presets(pool: &SqlitePool) -> Result<Vec<Preset>> {
    let rows = sqlx::query_as::<_, (i64, String, String, String, Option<i64>)>(
        "SELECT id, bank_name, column_map, date_format, account_id FROM import_presets
          ORDER BY bank_name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, bank_name, map, fmt, account_id)| {
            Some(Preset {
                id,
                bank_name,
                column_map: ColumnMap::from_json(&map).ok()?,
                date_format: DateFormat::parse_name(&fmt)?,
                account_id,
            })
        })
        .collect())
}

/// Remember a mapping under a bank's name, so the next statement imports in one
/// click. Re-saving the same bank replaces its mapping.
pub async fn save_preset(
    pool: &SqlitePool,
    bank_name: &str,
    map: &ColumnMap,
    fmt: DateFormat,
    account_id: Option<i64>,
) -> Result<i64> {
    if bank_name.trim().is_empty() {
        bail!("a preset needs a name");
    }
    sqlx::query(
        "INSERT INTO import_presets (bank_name, column_map, date_format, account_id, created_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(bank_name) DO UPDATE SET
            column_map = excluded.column_map, date_format = excluded.date_format,
            account_id = excluded.account_id",
    )
    .bind(bank_name.trim())
    .bind(map.to_json())
    .bind(fmt.as_str())
    .bind(account_id)
    .bind(crate::schema::unix_now())
    .execute(pool)
    .await?;
    Ok(sqlx::query_scalar("SELECT id FROM import_presets WHERE bank_name = ?")
        .bind(bank_name.trim())
        .fetch_one(pool)
        .await
        .context("preset vanished after saving")?)
}

pub async fn delete_preset(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM import_presets WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::txn::{TxnFilter, TxnSort};
    use crate::{accounts, schema};

    const HDFC: &str = "\
Date,Narration,Withdrawal Amt.,Deposit Amt.,Closing Balance
05/07/2026,\"UPI/DR/9928/SWIGGY, BANGALORE\",340.00,,10499.00
26/07/2026,NETFLIX 4471,649.00,,9850.00
01/07/2026,SALARY JULY,,50000.00,10839.00
,Closing balance,,,9850.00
";

    /// Every date in the first twelve days: undecidable on purpose.
    const AMBIGUOUS: &str = "\
Date,Narration,Amount
05/07/2026,SHOP A,-100.00
09/07/2026,SHOP B,-200.00
11/07/2026,SHOP C,-300.00
";

    const QFX: &str = "\
OFXHEADER:100
<OFX><CURDEF>INR
<STMTTRN><DTPOSTED>20260726<TRNAMT>-649.00<NAME>NETFLIX 4471</STMTTRN>
<STMTTRN><DTPOSTED>20260701<TRNAMT>50000.00<NAME>SALARY JULY</STMTTRN>
</OFX>
";

    async fn pool() -> SqlitePool {
        let p = SqlitePool::connect("sqlite::memory:").await.unwrap();
        schema::apply_schema(&p).await.unwrap();
        schema::seed_defaults(&p).await.unwrap();
        p
    }

    async fn bank(p: &SqlitePool) -> i64 {
        accounts::create(p, &accounts::NewAccount::bank("HDFC", 0)).await.unwrap()
    }

    #[test]
    fn file_kind_comes_from_the_contents_not_the_extension() {
        assert_eq!(sniff_kind(HDFC), FileKind::Csv);
        assert_eq!(sniff_kind(QFX), FileKind::Ofx);
    }

    #[test]
    fn a_csv_preview_guesses_the_mapping_and_the_format() {
        let p = preview(HDFC, "INR", None, None).unwrap();
        assert_eq!(p.date_format, Some(DateFormat::DayFirst));
        assert_eq!(p.column_map.debit.as_deref(), Some("Withdrawal Amt."));
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.unreadable, 1);
        assert_eq!(p.debits(), 2);
        assert_eq!(p.credits(), 1);
        assert_eq!(p.span(), Some(("2026-07-01".into(), "2026-07-26".into())));
    }

    #[test]
    fn an_ambiguous_file_is_previewed_with_no_rows_and_no_guess() {
        let p = preview(AMBIGUOUS, "INR", None, None).unwrap();
        assert_eq!(p.date_format, None, "guessing would shift the statement silently");
        assert!(p.rows.is_empty(), "and there is nothing to show until it is settled");

        // Once the user chooses, the same file parses.
        let p = preview(AMBIGUOUS, "INR", None, Some(DateFormat::DayFirst)).unwrap();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].occurred_on, "2026-07-05");
    }

    #[test]
    fn an_ofx_preview_needs_no_choices_at_all() {
        let p = preview(QFX, "USD", None, None).unwrap();
        assert_eq!(p.kind, FileKind::Ofx);
        assert_eq!(p.currency, "INR", "the file said so");
        assert_eq!(p.rows.len(), 2);
        assert!(p.date_format.is_some());
    }

    #[test]
    fn an_empty_file_is_refused() {
        assert!(preview("   \n", "INR", None, None).is_err());
    }

    #[test]
    fn a_corrected_mapping_is_honoured() {
        // Deliberately read the balance column as the amount, as a user fixing a
        // bad guess might.
        let map = ColumnMap {
            date: "Date".into(),
            description: "Narration".into(),
            amount: Some("Closing Balance".into()),
            ..Default::default()
        };
        let p = preview(HDFC, "INR", Some(&map), Some(DateFormat::DayFirst)).unwrap();
        assert_eq!(p.rows[0].amount_minor, 1_049_900);
    }

    #[tokio::test]
    async fn debits_become_expenses_and_credits_become_income() {
        let p = pool().await;
        let acct = bank(&p).await;
        let file = preview(HDFC, "INR", None, None).unwrap();
        let got = ingest(&p, acct, &file).await.unwrap();
        assert_eq!(got.added, 3);
        assert_eq!(got.duplicates, 0);

        assert_eq!(txn::spent_between(&p, "2026-07-01", "2026-07-31").await.unwrap(), 68_900);
        assert_eq!(txn::income_between(&p, "2026-07-01", "2026-07-31").await.unwrap(), 5_000_000);
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 5_000_000 - 68_900);
    }

    #[tokio::test]
    async fn re_importing_an_overlapping_statement_adds_zero_rows() {
        let p = pool().await;
        let acct = bank(&p).await;
        let file = preview(HDFC, "INR", None, None).unwrap();
        ingest(&p, acct, &file).await.unwrap();

        let again = ingest(&p, acct, &file).await.unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.duplicates, 3, "and the user is told, rather than left guessing");

        let all = txn::page(&p, &TxnFilter::default(), TxnSort::Date, true, 0).await.unwrap();
        assert_eq!(all.total, 3);
        assert_eq!(accounts::balance(&p, acct).await.unwrap(), 5_000_000 - 68_900);
    }

    #[tokio::test]
    async fn an_overlapping_month_imports_only_the_new_rows() {
        let p = pool().await;
        let acct = bank(&p).await;
        ingest(&p, acct, &preview(HDFC, "INR", None, None).unwrap()).await.unwrap();

        let next = "\
Date,Narration,Withdrawal Amt.,Deposit Amt.,Closing Balance
26/07/2026,NETFLIX 4471,649.00,,9850.00
02/08/2026,BESCOM POWER,3284.00,,6566.00
";
        let got = ingest(&p, acct, &preview(next, "INR", None, None).unwrap()).await.unwrap();
        assert_eq!(got.added, 1, "only the August row is new");
        assert_eq!(got.duplicates, 1);
    }

    #[tokio::test]
    async fn imported_rows_carry_their_provenance() {
        let p = pool().await;
        let acct = bank(&p).await;
        ingest(&p, acct, &preview(HDFC, "INR", None, None).unwrap()).await.unwrap();
        ingest(&p, acct, &preview(QFX, "INR", None, None).unwrap()).await.unwrap();

        let sources: Vec<String> =
            sqlx::query_scalar("SELECT DISTINCT source FROM transactions ORDER BY source")
                .fetch_all(&p)
                .await
                .unwrap();
        assert_eq!(sources, ["csv", "ofx"], "the Source column has to mean something");
    }

    #[tokio::test]
    async fn an_ofx_import_deduplicates_against_the_csv_of_the_same_statement() {
        // The dedup key is (account, date, amount, normalised merchant), so the
        // same charge from two file formats is still one charge.
        let p = pool().await;
        let acct = bank(&p).await;
        ingest(&p, acct, &preview(HDFC, "INR", None, None).unwrap()).await.unwrap();
        let got = ingest(&p, acct, &preview(QFX, "INR", None, None).unwrap()).await.unwrap();
        assert_eq!(got.added, 0);
        assert_eq!(got.duplicates, 2);
    }

    #[tokio::test]
    async fn a_foreign_currency_file_is_converted_at_import() {
        let p = pool().await;
        let acct = bank(&p).await;
        crate::fx::set(&p, "USD", 83_600_000, 0).await.unwrap();
        let us = "Transaction Date,Description,Amount\n07/26/2026,NETFLIX.COM,-9.99\n";
        let file = preview(us, "USD", None, Some(DateFormat::MonthFirst)).unwrap();
        ingest(&p, acct, &file).await.unwrap();

        let (amount, base): (i64, i64) =
            sqlx::query_as("SELECT amount_minor, base_minor FROM transactions").fetch_one(&p).await.unwrap();
        assert_eq!(amount, 999);
        assert_eq!(base, crate::money::convert(999, 83_600_000));
    }

    #[tokio::test]
    async fn ingesting_nothing_is_an_error_rather_than_a_silent_success() {
        let p = pool().await;
        let acct = bank(&p).await;
        let empty = preview(AMBIGUOUS, "INR", None, None).unwrap();
        assert!(ingest(&p, acct, &empty).await.is_err());
    }

    #[tokio::test]
    async fn presets_round_trip_and_upsert_by_bank_name() {
        let p = pool().await;
        let acct = bank(&p).await;
        let file = preview(HDFC, "INR", None, None).unwrap();
        let id = save_preset(&p, "HDFC", &file.column_map, DateFormat::DayFirst, Some(acct)).await.unwrap();

        let stored = presets(&p).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].bank_name, "HDFC");
        assert_eq!(stored[0].column_map, file.column_map);
        assert_eq!(stored[0].date_format, DateFormat::DayFirst);
        assert_eq!(stored[0].account_id, Some(acct));

        // Saving the same bank again replaces rather than duplicating.
        let again = save_preset(&p, "HDFC", &ColumnMap::default(), DateFormat::Iso, None).await.unwrap();
        assert_eq!(again, id);
        let stored = presets(&p).await.unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].date_format, DateFormat::Iso);

        delete_preset(&p, id).await.unwrap();
        assert!(presets(&p).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_nameless_preset_is_refused() {
        let p = pool().await;
        assert!(save_preset(&p, "  ", &ColumnMap::default(), DateFormat::Iso, None).await.is_err());
    }

    #[tokio::test]
    async fn importing_then_detecting_finds_the_subscription() {
        // The two halves of the intake chain, joined up.
        let p = pool().await;
        let acct = bank(&p).await;
        let three_months = "\
Date,Narration,Withdrawal Amt.,Deposit Amt.
26/05/2026,NETFLIX 4471,649.00,
26/06/2026,NETFLIX 5120,649.00,
26/07/2026,NETFLIX 6033,649.00,
";
        ingest(&p, acct, &preview(three_months, "INR", None, None).unwrap()).await.unwrap();
        let props = detect::propose(&p).await.unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].label, "Netflix");
        assert_eq!(props[0].amount_minor, Some(64_900));
    }
}
