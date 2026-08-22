//! The Finances forms.
//!
//! Every editing dialog in the section is a list of fields built here and
//! rendered generically on the Dart side — the same approach Tools takes, and
//! the reason there are not thirteen hand-built dialogs in a widget tree.
//!
//! A port of `tulipix-sec-finances/src/sheet.rs`, which had no Slint in it: it
//! only ever spoke `String` and `i64`. It could not be linked because it is a
//! module of a crate that depends on slint elsewhere, so it is re-implemented
//! rather than re-designed — the validation rules and the wording of every
//! error are the originals, because both are product decisions with tests
//! behind them in the crate this cannot reach.
//!
//! Validation lives on the submit path, and every message it produces is meant
//! to be read by the user rather than logged: a form that refuses silently is
//! worse than one that accepts nonsense.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use sqlx::SqlitePool;
use tulipix_finances::{
    accounts::{self, AccountKind, NewAccount},
    budgets, date,
    date::Cycle,
    dues::{self, Direction, NewDue},
    insights,
    loans::{self, NewLoan},
    money, obligations,
    recur::{self, NewRecurrence, RecurKind},
    txn::{self, NewTxn, TxnKind},
};

#[derive(Clone, Debug, Default)]
pub struct Field {
    pub key: String,
    pub label: String,
    /// text | number | money | date | dropdown | toggle | static | note
    pub kind: String,
    pub value: String,
    pub hint: String,
    pub options: Vec<String>,
    pub required: bool,
}

impl Field {
    pub fn new(key: &str, label: &str, kind: &str) -> Self {
        Field { key: key.into(), label: label.into(), kind: kind.into(), ..Default::default() }
    }

    pub fn v(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.into();
        self
    }

    fn opts(mut self, options: Vec<String>) -> Self {
        self.options = options;
        self
    }

    pub fn req(mut self) -> Self {
        self.required = true;
        self
    }
}

fn text(key: &str, label: &str) -> Field {
    Field::new(key, label, "text")
}

fn money_field(key: &str, label: &str) -> Field {
    Field::new(key, label, "money")
}

fn date_field(key: &str, label: &str) -> Field {
    Field::new(key, label, "date").v(date::iso(date::today()))
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    // No `kind` or `id` here: `open_sheet` already wrote both into the session
    // before calling `build`, and a second copy is a second thing that can be
    // wrong about which row is being edited.
    pub title: String,
    pub hint: String,
    pub fields: Vec<Field>,
    pub primary: String,
}

/// `name — id` pairs for a dropdown, plus the label list.
///
/// The label carries the id so the submit path can resolve a selection without
/// a second query and without matching on a name the user may have duplicated.
fn labelled(pairs: &[(i64, String)]) -> Vec<String> {
    pairs.iter().map(|(id, name)| format!("{name}  #{id}")).collect()
}

pub fn id_from_label(label: &str) -> Option<i64> {
    label.rsplit_once("  #")?.1.trim().parse().ok()
}

async fn account_options(pool: &SqlitePool, include_virtual: bool) -> Result<Vec<String>> {
    let rows = accounts::list(pool, false).await?;
    let pairs: Vec<(i64, String)> = rows
        .into_iter()
        .filter(|a| include_virtual || a.kind != AccountKind::Virtual)
        .map(|a| (a.id, a.name))
        .collect();
    Ok(labelled(&pairs))
}

async fn category_options(pool: &SqlitePool, kind: &str) -> Result<Vec<String>> {
    let rows = sqlx::query_as::<_, (i64, String, Option<String>)>(
        "SELECT c.id, c.name, p.name FROM categories c
           LEFT JOIN categories p ON p.id = c.parent_id
          WHERE c.kind = ? ORDER BY COALESCE(p.name, c.name), c.parent_id IS NOT NULL, c.name",
    )
    .bind(kind)
    .fetch_all(pool)
    .await?;
    let pairs: Vec<(i64, String)> = rows
        .into_iter()
        .map(|(id, name, parent)| match parent {
            Some(p) => (id, format!("{p} › {name}")),
            None => (id, name),
        })
        .collect();
    let mut out = vec![String::from("None")];
    out.extend(labelled(&pairs));
    Ok(out)
}

const CYCLES: [&str; 5] = ["Monthly", "Weekly", "Quarterly", "Yearly", "Irregular"];

fn cycle_from(label: &str) -> Cycle {
    match label {
        "Weekly" => Cycle::Weekly,
        "Quarterly" => Cycle::Quarterly,
        "Yearly" => Cycle::Yearly,
        "Irregular" => Cycle::Irregular,
        _ => Cycle::Monthly,
    }
}

fn cycle_label(c: Cycle) -> String {
    match c {
        Cycle::Weekly => "Weekly",
        Cycle::Quarterly => "Quarterly",
        Cycle::Yearly => "Yearly",
        Cycle::Irregular => "Irregular",
        Cycle::Monthly => "Monthly",
    }
    .to_string()
}

/// Where the "Unused" pill on the subscriptions table comes from, in the one
/// place someone would go looking for it.
///
/// It is the only figure in this section that is not entered and not derived
/// from money, so a row can wear a red pill nobody can account for. The note
/// states the rule and then this row's own standing against it, because a rule
/// without the current reading is still not an explanation.
async fn unused_rule_note(pool: &SqlitePool, id: i64) -> String {
    let mut out = format!(
        "\"Unused\" is not something you set here. A subscription earns it when both \
         are true: its category sits under Entertainment, and the Videos section has \
         recorded no playback for {} days or more. Cancel nothing on it blindly — it \
         reads what Videos watched, not what you watched.",
        insights::UNUSED_DAYS
    );

    let counted = if id == 0 {
        None
    } else {
        insights::video_subscription_ids(pool).await.ok().map(|ids| ids.contains(&id))
    };
    match counted {
        Some(true) => out.push_str("\n\nThis one is under Entertainment, so the rule reaches it."),
        Some(false) => out.push_str(
            "\n\nThis one is not under Entertainment (or is not active), so the rule never \
             reaches it and it will not be flagged.",
        ),
        None => {}
    }

    match insights::last_video_playback().await {
        Some(last) => {
            let idle = date::days_between(last, date::today());
            out.push_str(&format!(
                " Videos last recorded playback on {} — {idle} day{} ago.",
                date::iso(last),
                if idle == 1 { "" } else { "s" }
            ));
        }
        None => out.push_str(
            " Videos has recorded no playback at all, so nothing is being flagged right now.",
        ),
    }
    out
}

// ── building ────────────────────────────────────────────────────────────────

/// Build the form for `kind`, seeded from the database when `id` names an
/// existing row.
pub async fn build(pool: &SqlitePool, kind: &str, id: i64) -> Result<Sheet> {
    let base = tulipix_finances::fx::base_currency();
    let mut sheet = Sheet { primary: "Save".into(), ..Default::default() };

    match kind {
        "txn" => {
            sheet.title =
                if id == 0 { "Add a transaction".into() } else { "Edit transaction".into() };
            sheet.hint = "A transfer moves money between two things you own and is excluded \
                          from every spend total."
                .into();
            let existing: Option<(
                String,
                i64,
                String,
                String,
                String,
                Option<i64>,
                Option<String>,
                i64,
            )> = if id == 0 {
                None
            } else {
                sqlx::query_as(
                    "SELECT kind, amount_minor, currency, occurred_on, description,
                            category_id, note, account_id
                       FROM transactions WHERE id = ?",
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
            };

            let accounts_opts = account_options(pool, false).await?;
            let (txn_kind, amount, currency, on, desc, cat, note_v, acct) = match &existing {
                Some((k, a, c, o, d, cat, n, acct)) => (
                    k.clone(),
                    minor_to_input(*a, c),
                    c.clone(),
                    o.clone(),
                    d.clone(),
                    *cat,
                    n.clone().unwrap_or_default(),
                    Some(*acct),
                ),
                None => (
                    "expense".into(),
                    String::new(),
                    base.clone(),
                    date::iso(date::today()),
                    String::new(),
                    None,
                    String::new(),
                    None,
                ),
            };

            sheet.fields = vec![
                Field::new("kind", "Kind", "dropdown")
                    .v(match txn_kind.as_str() {
                        "income" => "Income",
                        "transfer" => "Transfer",
                        _ => "Expense",
                    })
                    .opts(vec!["Expense".into(), "Income".into(), "Transfer".into()]),
                Field::new("account", "Account", "dropdown")
                    .v(acct
                        .and_then(|a| {
                            accounts_opts.iter().find(|o| id_from_label(o) == Some(a)).cloned()
                        })
                        .or_else(|| accounts_opts.first().cloned())
                        .unwrap_or_default())
                    .opts(accounts_opts.clone())
                    .req(),
                Field::new("to_account", "To account", "dropdown")
                    .v(String::from("None"))
                    .opts({
                        let mut v = vec![String::from("None")];
                        v.extend(accounts_opts);
                        v
                    })
                    .hint("Transfers only."),
                money_field("amount", "Amount")
                    .v(amount)
                    .hint(&format!("In {base}, e.g. 649 or 649.50"))
                    .req(),
                text("currency", "Currency").v(currency).hint("Three-letter code."),
                date_field("occurred_on", "Date").v(on).req(),
                text("description", "Description").v(desc).req(),
                Field::new("category", "Category", "dropdown").v(String::from("None")).opts({
                    let mut v = category_options(pool, "expense").await?;
                    v.extend(category_options(pool, "income").await?.into_iter().skip(1));
                    v.extend(category_options(pool, "transfer").await?.into_iter().skip(1));
                    v
                }),
                text("note", "Note").v(note_v),
            ];
            // Seed the category dropdown to the row's existing category, if it
            // has one. Two steps rather than a let-chain: the second half reads
            // through the mutable borrow the first half takes.
            if let Some(c) = cat
                && let Some(f) = sheet.fields.iter_mut().find(|f| f.key == "category")
            {
                let found = f.options.iter().find(|o| id_from_label(o) == Some(c)).cloned();
                if let Some(found) = found {
                    f.value = found;
                }
            }
            sheet.primary = if id == 0 { "Post".into() } else { "Save".into() };
        }

        "account" => {
            sheet.title = if id == 0 { "New account".into() } else { "Edit account".into() };
            sheet.hint = "Balances are worked out from the ledger, so the opening balance is \
                          only the starting point."
                .into();
            let existing: Option<(String, String, i64, Option<i64>, Option<i64>)> = if id == 0 {
                None
            } else {
                sqlx::query_as(
                    "SELECT name, kind, opening_minor, credit_limit_minor, statement_day
                       FROM accounts WHERE id = ?",
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
            };
            let (name, akind, opening, limit, stmt) = match existing {
                Some((n, k, o, l, d)) => (n, k, minor_to_input(o, &base), l, d),
                None => (String::new(), "bank".into(), String::new(), None, None),
            };
            sheet.fields = vec![
                text("name", "Name").v(name).req(),
                Field::new("kind", "Kind", "dropdown")
                    .v(match akind.as_str() {
                        "cash" => "Cash",
                        "card" => "Credit card",
                        "wallet" => "Wallet",
                        _ => "Bank",
                    })
                    .opts(vec![
                        "Bank".into(),
                        "Cash".into(),
                        "Credit card".into(),
                        "Wallet".into(),
                    ]),
                money_field("opening", "Opening balance").v(opening).hint("Leave blank for zero."),
                money_field("limit", "Credit limit")
                    .v(limit.map(|l| minor_to_input(l, &base)).unwrap_or_default())
                    .hint("Credit cards only."),
                Field::new("statement_day", "Statement day", "number")
                    .v(stmt.map(|d| d.to_string()).unwrap_or_default())
                    .hint("Day of the month, 1-31. Credit cards only."),
                text("currency", "Currency").v(base.clone()),
            ];
        }

        "sub" | "bill" => {
            let is_sub = kind == "sub";
            sheet.title = match (is_sub, id) {
                (true, 0) => "New subscription".into(),
                (true, _) => "Edit subscription".into(),
                (false, 0) => "New recurring bill".into(),
                (false, _) => "Edit recurring bill".into(),
            };
            sheet.hint = if is_sub {
                "A fixed amount on a fixed cycle. It can post itself; a bill cannot.".into()
            } else {
                "Leave the amount blank when it varies. A bill with no fixed amount is \
                 estimated from its last three payments and never posts itself."
                    .into()
            };
            let existing = if id == 0 { None } else { recur::get(pool, id).await? };
            let accounts_opts = account_options(pool, false).await?;
            let cat_opts = category_options(pool, "expense").await?;

            sheet.fields = vec![
                text("name", "Name")
                    .v(existing.as_ref().map(|r| r.name.clone()).unwrap_or_default())
                    .req(),
                money_field("amount", "Amount")
                    .v(existing
                        .as_ref()
                        .and_then(|r| r.amount_minor)
                        .map(|a| minor_to_input(a, &base))
                        .unwrap_or_default())
                    .hint(if is_sub { "The fixed charge." } else { "Leave blank when it varies." }),
                text("currency", "Currency").v(existing
                    .as_ref()
                    .map(|r| r.currency.clone())
                    .unwrap_or_else(|| base.clone())),
                Field::new("cycle", "Cycle", "dropdown")
                    .v(existing
                        .as_ref()
                        .map(|r| cycle_label(r.cycle))
                        .unwrap_or_else(|| "Monthly".into()))
                    .opts(CYCLES.iter().map(|c| c.to_string()).collect()),
                date_field("next_due_on", "Next due")
                    .v(existing
                        .as_ref()
                        .and_then(|r| r.next_due_on.clone())
                        .unwrap_or_else(|| date::iso(date::today())))
                    .req(),
                Field::new("anchor_day", "Day of the month", "number")
                    .v(existing
                        .as_ref()
                        .and_then(|r| r.anchor_day)
                        .map(|d| d.to_string())
                        .unwrap_or_default())
                    .hint("31 stays the 31st — February clamps and March goes back."),
                Field::new("account", "Account", "dropdown")
                    .v(existing
                        .as_ref()
                        .and_then(|r| r.account_id)
                        .and_then(|a| {
                            accounts_opts.iter().find(|o| id_from_label(o) == Some(a)).cloned()
                        })
                        .unwrap_or_else(|| "None".into()))
                    .opts({
                        let mut v = vec![String::from("None")];
                        v.extend(accounts_opts);
                        v
                    }),
                Field::new("category", "Category", "dropdown")
                    .v(existing
                        .as_ref()
                        .and_then(|r| r.category_id)
                        .and_then(|c| cat_opts.iter().find(|o| id_from_label(o) == Some(c)).cloned())
                        .unwrap_or_else(|| "None".into()))
                    .opts(cat_opts),
                Field::new("auto_post", "Post it for me", "toggle")
                    .v(existing
                        .as_ref()
                        .map(|r| r.auto_post.to_string())
                        .unwrap_or_else(|| "false".into()))
                    .hint("Only possible with a fixed amount and an account."),
                Field::new("reminder_days", "Warn me this many days ahead", "number")
                    // A new one starts at the `finances.alert_lead_days` setting;
                    // from then on this row's own number is what the engine uses,
                    // so rent can want a fortnight while Spotify wants nothing.
                    .v(existing
                        .as_ref()
                        .map(|r| r.reminder_days.to_string())
                        .unwrap_or_else(|| tulipix_finances::lead_days().to_string()))
                    .hint("Per item. 0 means only on the day itself."),
                text("note", "Note")
                    .v(existing.as_ref().and_then(|r| r.note.clone()).unwrap_or_default())
                    .hint("Shown under the name in the table — the plan, usually."),
            ];
            if is_sub {
                sheet
                    .fields
                    .push(Field::new("unused_rule", "Unused", "note").v(unused_rule_note(pool, id).await));
            }
        }

        "one-off" => {
            sheet.title = "One-off bill".into();
            sheet.hint = "A dated thing you owe once. No template, no repeat.".into();
            sheet.fields = vec![
                text("name", "Name").req(),
                money_field("estimate", "Expected amount").hint("Optional."),
                date_field("due_on", "Due").req(),
            ];
            sheet.primary = "Add".into();
        }

        "pay" => {
            let o: Option<(String, Option<i64>, Option<i64>)> = sqlx::query_as(
                "SELECT o.name, o.estimate_minor, r.account_id
                   FROM obligations o LEFT JOIN recurrences r ON r.id = o.recurrence_id
                  WHERE o.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            let (name, estimate, acct) = o.context("no such bill")?;
            let accounts_opts = account_options(pool, false).await?;
            sheet.title = format!("Mark {name} paid");
            sheet.hint = "The amount you actually paid. The estimate stays on record, and the \
                          gap between the two is what Insights reports."
                .into();
            sheet.fields = vec![
                money_field("actual", "Amount paid")
                    .v(estimate.map(|e| minor_to_input(e, &base)).unwrap_or_default())
                    .hint("Pre-filled with the estimate — change it to what was really charged.")
                    .req(),
                date_field("on", "Paid on").req(),
                Field::new("account", "From account", "dropdown")
                    .v(acct
                        .and_then(|a| {
                            accounts_opts.iter().find(|o| id_from_label(o) == Some(a)).cloned()
                        })
                        .or_else(|| accounts_opts.first().cloned())
                        .unwrap_or_default())
                    .opts(accounts_opts)
                    .req(),
                text("currency", "Currency").v(base.clone()),
            ];
            sheet.primary = "Mark paid".into();
        }

        "due" => {
            sheet.title = "Lend or borrow".into();
            sheet.hint = "Lending is not spending: this posts a transfer into a holding \
                          account and comes back when it settles."
                .into();
            let accounts_opts = account_options(pool, false).await?;
            sheet.fields = vec![
                text("person", "Person").hint("Free text. No contacts, no profiles.").req(),
                Field::new("direction", "Which way", "dropdown")
                    .v("I lent it to them")
                    .opts(vec!["I lent it to them".into(), "I borrowed it from them".into()]),
                money_field("amount", "Amount").req(),
                text("currency", "Currency").v(base.clone()),
                date_field("opened_on", "Since").req(),
                Field::new("account", "Account the cash moved through", "dropdown")
                    .v(accounts_opts.first().cloned().unwrap_or_else(|| "None".into()))
                    .opts({
                        let mut v = vec![String::from("None")];
                        v.extend(accounts_opts);
                        v
                    })
                    .hint(
                        "None records the debt without moving any money — for one that \
                         predates the app.",
                    ),
                text("note", "Note"),
            ];
            sheet.primary = "Record".into();
        }

        "settle" => {
            let d = dues::list(pool, None)
                .await?
                .into_iter()
                .find(|d| d.id == id)
                .context("no such due")?;
            let accounts_opts = account_options(pool, false).await?;
            sheet.title = format!("Settle with {}", d.person);
            sheet.hint = format!(
                "{} outstanding. A smaller amount part-settles and leaves the rest open.",
                money::format_minor(d.amount_minor, &d.currency)
            );
            sheet.fields = vec![
                money_field("amount", "Amount").v(minor_to_input(d.amount_minor, &d.currency)).req(),
                date_field("on", "On").req(),
                Field::new("account", "Account", "dropdown")
                    .v(accounts_opts.first().cloned().unwrap_or_default())
                    .opts(accounts_opts)
                    .req(),
            ];
            sheet.primary = "Settle".into();
        }

        "budget" => {
            sheet.title = "Envelope".into();
            sheet.hint = "A monthly limit on one category. Transfers and card payments are \
                          excluded, so paying a card cannot eat it."
                .into();
            let cat_opts: Vec<String> =
                category_options(pool, "expense").await?.into_iter().skip(1).collect();
            let seeded = if id == 0 {
                cat_opts.first().cloned().unwrap_or_default()
            } else {
                cat_opts
                    .iter()
                    .find(|o| id_from_label(o) == Some(id))
                    .cloned()
                    .unwrap_or_else(|| cat_opts.first().cloned().unwrap_or_default())
            };
            sheet.fields = vec![
                Field::new("category", "Category", "dropdown").v(seeded).opts(cat_opts).req(),
                money_field("amount", "Monthly limit").req(),
                Field::new("rollover", "Carry any surplus into next month", "toggle")
                    .v("false")
                    .hint("Only a surplus carries — an overspend never does."),
            ];
        }

        "loan" => {
            // `id` is the loan's *account* id, which is what the Loans cards
            // carry — a loan has no id of its own.
            let existing = if id == 0 { None } else { loans::get(pool, id).await? };
            sheet.title = if existing.is_some() { "Edit loan".into() } else { "New loan".into() };
            sheet.hint = if existing.is_some() {
                "Editing the terms leaves the instalments already paid exactly where they are. \
                 The amount borrowed is the account's opening balance, so correcting it moves \
                 the balance too."
                    .into()
            } else {
                "A loan is an account with a negative balance, so it totals with everything \
                 else. Leave the EMI blank to have it worked out."
                    .into()
            };
            let currency =
                existing.as_ref().map(|l| l.currency.clone()).unwrap_or_else(|| base.clone());
            sheet.fields = vec![
                text("name", "Name")
                    .v(existing.as_ref().map(|l| l.name.clone()).unwrap_or_default())
                    .req(),
                money_field("principal", "Amount borrowed")
                    .v(existing
                        .as_ref()
                        .map(|l| minor_to_input(l.principal_minor, &currency))
                        .unwrap_or_default())
                    .req(),
                // Basis points back to the percentage the field asks for: 840 → "8.4".
                Field::new("rate", "Interest rate", "number")
                    .v(existing
                        .as_ref()
                        .map(|l| format!("{}.{}", l.rate_bp / 100, (l.rate_bp % 100) / 10))
                        .unwrap_or_default())
                    .hint("A year, as a percentage. 8.4 for 8.4%.")
                    .req(),
                Field::new("tenure_months", "Tenure in months", "number")
                    .v(existing
                        .as_ref()
                        .map(|l| l.tenure_months.to_string())
                        .unwrap_or_else(|| "180".into()))
                    .req(),
                date_field("started_on", "First instalment")
                    .v(existing.as_ref().map(|l| l.started_on.clone()).unwrap_or_default())
                    .req(),
                Field::new("emi_day", "EMI day", "number")
                    .v(existing.as_ref().map(|l| l.emi_day.to_string()).unwrap_or_else(|| "5".into()))
                    .req(),
                money_field("emi", "EMI")
                    .v(existing
                        .as_ref()
                        .map(|l| minor_to_input(l.emi_minor, &currency))
                        .unwrap_or_default())
                    .hint("Leave blank to compute it from the figures above."),
                text("currency", "Currency").v(currency),
            ];
            sheet.primary = if existing.is_some() { "Save".into() } else { "Add loan".into() };
        }

        "reconcile" => {
            let bal = accounts::balance(pool, id).await?;
            let name: String = sqlx::query_scalar("SELECT name FROM accounts WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?;
            sheet.title = format!("Reconcile {name}");
            sheet.hint = format!(
                "The ledger says {}. Enter what the statement says and the difference is \
                 posted as a visible adjustment — nothing is overwritten.",
                money::format_minor(bal, &base)
            );
            sheet.fields = vec![
                money_field("actual", "Statement balance").v(minor_to_input(bal, &base)).req(),
                date_field("on", "As at").req(),
            ];
            sheet.primary = "Reconcile".into();
        }

        _ => bail!("unknown form {kind:?}"),
    }
    Ok(sheet)
}

/// Format minor units for a text input: digits and one decimal point, nothing
/// else.
///
/// Not the display formatter — grouping commas and a currency symbol round-trip
/// badly through an edit box.
pub fn minor_to_input(minor: i64, currency: &str) -> String {
    let dec = money::decimals(currency);
    if dec == 0 {
        return minor.to_string();
    }
    let scale = 10i64.pow(dec);
    let whole = minor / scale;
    let frac = (minor % scale).abs();
    if frac == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{frac:0width$}", width = dec as usize)
    }
}

// ── submitting ──────────────────────────────────────────────────────────────

pub type Form = HashMap<String, String>;

fn get(form: &Form, key: &str) -> String {
    form.get(key).cloned().unwrap_or_default().trim().to_string()
}

fn opt_id(form: &Form, key: &str) -> Option<i64> {
    let v = get(form, key);
    if v.is_empty() || v == "None" { None } else { id_from_label(&v) }
}

fn need_id(form: &Form, key: &str, what: &str) -> Result<i64> {
    opt_id(form, key).with_context(|| format!("choose {what}"))
}

fn opt_num(form: &Form, key: &str) -> Option<i64> {
    let v = get(form, key);
    if v.is_empty() { None } else { v.parse().ok() }
}

fn currency_of(form: &Form, base: &str) -> String {
    let c = get(form, "currency").to_uppercase();
    if c.len() == 3 { c } else { base.to_string() }
}

fn amount(form: &Form, key: &str, currency: &str) -> Result<i64> {
    let raw = get(form, key);
    if raw.is_empty() {
        bail!("enter an amount");
    }
    money::parse_amount(&raw, currency).with_context(|| format!("{raw:?} is not an amount"))
}

/// Apply the form. Errors here are written to be read by the user.
pub async fn submit(pool: &SqlitePool, kind: &str, id: i64, form: &Form) -> Result<()> {
    let base = tulipix_finances::fx::base_currency();

    match kind {
        "txn" => {
            let currency = currency_of(form, &base);
            let account_id = need_id(form, "account", "an account")?;
            let txn_kind = match get(form, "kind").as_str() {
                "Income" => TxnKind::Income,
                "Transfer" => TxnKind::Transfer,
                _ => TxnKind::Expense,
            };
            let amount_minor = amount(form, "amount", &currency)?;
            let on = get(form, "occurred_on");
            date::parse(&on).context("the date should look like 2026-07-26")?;
            let description = get(form, "description");
            if description.is_empty() {
                bail!("a transaction needs a description");
            }

            if id != 0 {
                // Kind and account are not editable here: changing either
                // rewrites balances in ways a small edit box should not.
                return txn::update(
                    pool,
                    id,
                    amount_minor,
                    &on,
                    &description,
                    opt_id(form, "category"),
                    Some(get(form, "note")).filter(|n| !n.is_empty()).as_deref(),
                )
                .await;
            }

            let mut t = NewTxn::expense(account_id, amount_minor, &currency, &on, &description);
            t.kind = txn_kind;
            t.to_account_id = opt_id(form, "to_account");
            t.category_id = opt_id(form, "category");
            t.note = Some(get(form, "note")).filter(|n| !n.is_empty());
            // Provenance is a column so the ledger can always say where a row
            // came from. A receipt read by OCR is not a hand-typed row, and the
            // Transactions tab shows the difference.
            if get(form, "source_hint") == "ocr" {
                t.source = "ocr".into();
            }
            t.rate_micro = tulipix_finances::fx::rate_for(pool, &currency, &base).await?;
            if txn_kind == TxnKind::Transfer && t.to_account_id.is_none() {
                bail!("a transfer needs a destination account");
            }
            txn::post(pool, &t).await?;
        }

        "account" => {
            let currency = currency_of(form, &base);
            let name = get(form, "name");
            let opening =
                if get(form, "opening").is_empty() { 0 } else { amount(form, "opening", &currency)? };
            let limit = if get(form, "limit").is_empty() {
                None
            } else {
                Some(amount(form, "limit", &currency)?)
            };
            let stmt = opt_num(form, "statement_day");
            if id == 0 {
                accounts::create(
                    pool,
                    &NewAccount {
                        name,
                        kind: match get(form, "kind").as_str() {
                            "Cash" => AccountKind::Cash,
                            "Credit card" => AccountKind::Card,
                            "Wallet" => AccountKind::Wallet,
                            _ => AccountKind::Bank,
                        },
                        currency,
                        opening_minor: opening,
                        credit_limit_minor: limit,
                        statement_day: stmt,
                    },
                )
                .await?;
            } else {
                accounts::update(pool, id, &name, opening, limit, stmt).await?;
            }
        }

        "sub" | "bill" => {
            let currency = currency_of(form, &base);
            let raw_amount = get(form, "amount");
            let amount_minor = if raw_amount.is_empty() {
                if kind == "sub" {
                    bail!("a subscription needs a fixed amount — use a bill when it varies");
                }
                None
            } else {
                Some(amount(form, "amount", &currency)?)
            };
            let next = get(form, "next_due_on");
            date::parse(&next).context("the next due date should look like 2026-07-26")?;

            let r = NewRecurrence {
                kind: if kind == "sub" { RecurKind::Subscription } else { RecurKind::Bill },
                name: get(form, "name"),
                amount_minor,
                currency,
                cycle: cycle_from(&get(form, "cycle")),
                anchor_day: opt_num(form, "anchor_day"),
                next_due_on: Some(next),
                account_id: opt_id(form, "account"),
                category_id: opt_id(form, "category"),
                auto_post: get(form, "auto_post") == "true",
                reminder_days: opt_num(form, "reminder_days").unwrap_or(3),
                note: Some(get(form, "note")).filter(|n| !n.is_empty()),
            };
            if id == 0 {
                recur::create(pool, &r).await?;
            } else {
                recur::update(pool, id, &r).await?;
            }
        }

        "one-off" => {
            let estimate = if get(form, "estimate").is_empty() {
                None
            } else {
                Some(amount(form, "estimate", &base)?)
            };
            obligations::create_one_off(pool, &get(form, "name"), &get(form, "due_on"), estimate)
                .await?;
        }

        "pay" => {
            let currency = currency_of(form, &base);
            let actual = amount(form, "actual", &currency)?;
            let on = date::parse(&get(form, "on"))
                .context("the paid-on date should look like 2026-07-26")?;
            let account_id = need_id(form, "account", "an account")?;
            obligations::mark_paid(pool, id, actual, on, account_id, &currency).await?;
        }

        "due" => {
            let currency = currency_of(form, &base);
            let d = NewDue {
                person: get(form, "person"),
                // Matched on the dropdown's own label. Renaming one of these
                // without the other silently files every borrowing as a lending.
                direction: if get(form, "direction") == "I borrowed it from them" {
                    Direction::IOwe
                } else {
                    Direction::OwedToMe
                },
                amount_minor: amount(form, "amount", &currency)?,
                currency,
                opened_on: get(form, "opened_on"),
                note: Some(get(form, "note")).filter(|n| !n.is_empty()),
                account_id: opt_id(form, "account"),
            };
            dues::create(pool, &d).await?;
        }

        "settle" => {
            let d = dues::list(pool, None)
                .await?
                .into_iter()
                .find(|d| d.id == id)
                .context("no such due")?;
            let paid = amount(form, "amount", &d.currency)?;
            let on =
                date::parse(&get(form, "on")).context("the date should look like 2026-07-26")?;
            dues::settle(pool, id, paid, on, need_id(form, "account", "an account")?).await?;
        }

        "budget" => {
            let category_id = need_id(form, "category", "a category")?;
            budgets::set(
                pool,
                category_id,
                &get(form, "period_hidden"),
                amount(form, "amount", &base)?,
                get(form, "rollover") == "true",
            )
            .await?;
        }

        "loan" => {
            let currency = currency_of(form, &base);
            // "8.4" → 840 basis points. Parsed through the money helper at two
            // decimals, which is exactly the scale basis points need.
            let rate_bp =
                money::parse_scaled(&get(form, "rate"), 2).context("the rate should look like 8.4")?;
            let l = NewLoan {
                name: get(form, "name"),
                principal_minor: amount(form, "principal", &currency)?,
                rate_bp,
                tenure_months: opt_num(form, "tenure_months").unwrap_or(0),
                started_on: get(form, "started_on"),
                emi_minor: if get(form, "emi").is_empty() {
                    None
                } else {
                    Some(amount(form, "emi", &currency)?)
                },
                emi_day: opt_num(form, "emi_day").unwrap_or(1),
                currency,
            };
            if id == 0 {
                loans::create(pool, &l).await?;
            } else {
                loans::update(pool, id, &l).await?;
            }
        }

        "reconcile" => {
            let actual = amount(form, "actual", &base)?;
            accounts::reconcile(pool, id, actual, &get(form, "on")).await?;
        }

        _ => bail!("unknown form {kind:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dropdown contract, which both halves of this file depend on: a label
    /// carries its own id, and the submit path reads it back without a query.
    #[test]
    fn a_dropdown_label_carries_its_id() {
        let opts = labelled(&[(7, "HDFC".into()), (9, "Paytm Wallet".into())]);
        assert_eq!(opts[0], "HDFC  #7");
        assert_eq!(id_from_label(&opts[0]), Some(7));
        assert_eq!(id_from_label(&opts[1]), Some(9));
        assert_eq!(id_from_label("None"), None, "the null option resolves to nothing");
    }

    /// Minor units round-trip through an edit box without gaining a symbol or a
    /// grouping comma, either of which would fail to parse back.
    #[test]
    fn money_round_trips_through_a_text_field() {
        assert_eq!(minor_to_input(64_900, "INR"), "649");
        assert_eq!(minor_to_input(64_950, "INR"), "649.50");
        assert_eq!(minor_to_input(-64_950, "INR"), "-649.50");
        // A zero-decimal currency has no fractional part to print at all.
        assert_eq!(minor_to_input(1_200, "JPY"), "1200");
    }
}
