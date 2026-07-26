//! OFX / QFX statements.
//!
//! OFX is SGML, not XML: closing tags are optional, so a real parser is the wrong
//! tool and every XML library refuses the files outright. What is needed is a tag
//! scanner over `<STMTTRN>` blocks, which is about eighty lines and no dependency.
//!
//! OFX is unambiguous in the two places CSV is not: `DTPOSTED` is always
//! `YYYYMMDD`, and `TRNAMT` is always signed. So there is no format to sniff and
//! no debit/credit column pair to get backwards — which is why this is the better
//! import path when a bank offers both.

use anyhow::{bail, Result};

use super::csv::RawRow;
use super::dates;
use crate::money;

/// Read the value of `tag` from one `STMTTRN` block.
///
/// The value runs to the next `<`, whether that is a closing tag or the next
/// opening one — which is exactly the SGML-optional-close case.
fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find('<').unwrap_or(rest.len());
    let v = rest[..end].trim();
    if v.is_empty() { None } else { Some(v) }
}

/// Every transaction in an OFX document.
///
/// The currency comes from the file's `CURDEF` when it has one; the caller's
/// account currency is the fallback.
pub fn parse(text: &str, fallback_currency: &str) -> Result<(Vec<RawRow>, usize, String)> {
    if !text.contains("<STMTTRN>") && !text.contains("<OFX>") {
        bail!("this does not look like an OFX or QFX statement");
    }
    let currency = tag(text, "CURDEF").unwrap_or(fallback_currency).to_uppercase();

    let mut rows = Vec::new();
    let mut skipped = 0usize;

    for block in text.split("<STMTTRN>").skip(1) {
        // Stop at the end of this transaction so a missing close tag cannot read
        // the next transaction's fields.
        let block = block.split("</STMTTRN>").next().unwrap_or(block);

        let Some(raw_date) = tag(block, "DTPOSTED") else {
            skipped += 1;
            continue;
        };
        let Some(raw_amount) = tag(block, "TRNAMT") else {
            skipped += 1;
            continue;
        };
        let Ok(on) = dates::parse_ofx(raw_date) else {
            skipped += 1;
            continue;
        };
        let Ok(signed) = money::parse_amount(raw_amount, &currency) else {
            skipped += 1;
            continue;
        };
        if signed == 0 {
            skipped += 1;
            continue;
        }

        // NAME is the merchant; MEMO is free text that is sometimes the only
        // thing filled in. CHECKNUM last, so a cheque at least says which one.
        let description = tag(block, "NAME")
            .or_else(|| tag(block, "MEMO"))
            .or_else(|| tag(block, "CHECKNUM"))
            .unwrap_or("(no description)")
            .to_string();

        rows.push(RawRow {
            occurred_on: crate::date::iso(on),
            description,
            amount_minor: signed.abs(),
            is_credit: signed > 0,
        });
    }
    Ok((rows, skipped, currency))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic QFX fragment: no closing tags on the leaf elements, which is
    /// legal OFX and what breaks every XML parser.
    const QFX: &str = "\
OFXHEADER:100
DATA:OFXSGML
<OFX>
<BANKMSGSRSV1><STMTTRNRS><STMTRS>
<CURDEF>INR
<BANKTRANLIST>
<STMTTRN>
<TRNTYPE>DEBIT
<DTPOSTED>20260726120000[-5:EST]
<TRNAMT>-649.00
<FITID>202607260001
<NAME>NETFLIX 4471
</STMTTRN>
<STMTTRN>
<TRNTYPE>CREDIT
<DTPOSTED>20260701
<TRNAMT>50000.00
<NAME>SALARY JULY
</STMTTRN>
<STMTTRN>
<TRNTYPE>DEBIT
<DTPOSTED>20260705
<TRNAMT>-340.00
<MEMO>UPI/DR/9928/SWIGGY
</STMTTRN>
</BANKTRANLIST>
</STMTRS></STMTTRNRS></BANKMSGSRSV1>
</OFX>
";

    #[test]
    fn leaf_tags_without_closing_tags_still_parse() {
        let (rows, skipped, currency) = parse(QFX, "USD").unwrap();
        assert_eq!(currency, "INR", "the file's own CURDEF wins over the fallback");
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn the_sign_carries_the_direction_with_nothing_to_guess() {
        let (rows, _, _) = parse(QFX, "INR").unwrap();
        let netflix = rows.iter().find(|r| r.description.contains("NETFLIX")).unwrap();
        assert_eq!(netflix.amount_minor, 64_900);
        assert!(!netflix.is_credit);
        let salary = rows.iter().find(|r| r.description.contains("SALARY")).unwrap();
        assert!(salary.is_credit);
        assert_eq!(salary.amount_minor, 5_000_000);
    }

    #[test]
    fn dates_need_no_sniffing() {
        let (rows, _, _) = parse(QFX, "INR").unwrap();
        assert_eq!(rows[0].occurred_on, "2026-07-26");
        assert_eq!(rows[1].occurred_on, "2026-07-01");
    }

    #[test]
    fn memo_stands_in_when_there_is_no_name() {
        let (rows, _, _) = parse(QFX, "INR").unwrap();
        assert_eq!(rows[2].description, "UPI/DR/9928/SWIGGY");
    }

    #[test]
    fn a_transaction_missing_its_amount_is_skipped_and_counted() {
        let broken = "<OFX><STMTTRN><DTPOSTED>20260726<NAME>NO AMOUNT</STMTTRN>\
                      <STMTTRN><DTPOSTED>20260727<TRNAMT>-100.00<NAME>FINE</STMTTRN></OFX>";
        let (rows, skipped, _) = parse(broken, "INR").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn a_missing_close_tag_does_not_bleed_into_the_next_transaction() {
        // The first block never closes; its fields must not be filled from the
        // second one.
        let sloppy = "<OFX><STMTTRN><DTPOSTED>20260726<TRNAMT>-100.00<NAME>FIRST\
                      <STMTTRN><DTPOSTED>20260727<TRNAMT>-200.00<NAME>SECOND</STMTTRN></OFX>";
        let (rows, _, _) = parse(sloppy, "INR").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].description, "FIRST");
        assert_eq!(rows[0].amount_minor, 10_000);
        assert_eq!(rows[1].description, "SECOND");
    }

    #[test]
    fn a_file_that_is_not_ofx_is_refused() {
        assert!(parse("Date,Narration,Amount\n05/07/2026,x,1", "INR").is_err());
    }

    #[test]
    fn a_zero_amount_line_is_not_a_transaction() {
        let z = "<OFX><STMTTRN><DTPOSTED>20260726<TRNAMT>0.00<NAME>FEE WAIVED</STMTTRN></OFX>";
        let (rows, skipped, _) = parse(z, "INR").unwrap();
        assert!(rows.is_empty());
        assert_eq!(skipped, 1);
    }
}
