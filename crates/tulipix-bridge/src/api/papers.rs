// The Papers section: receipts, bills, warranties, IDs and manuals, read on
// this computer and filed.
//
// Four tabs over one snapshot — Inbox (what needs doing, and what was just
// read), All papers, Expiring and Vault. Reading and the store are
// `crate::papers`; this file keeps the session (tab, filters, the paper open,
// whether the vault is open) and maps rows into what Dart draws. Reading runs
// in the background one paper at a time; the snapshot says how many are
// still being read, and Dart asks again until none are.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;

use crate::api::kitchen::{AccountPick, PhoneLink};
use crate::db::papers_pool;
use crate::papers::{self as p, vault};

/// "Locks again after 5 minutes, or when Tulipix locks."
const VAULT_OPEN_FOR: Duration = Duration::from_secs(5 * 60);
/// A file bigger than this is not a paper.
const MAX_BYTES: u64 = 200 * 1024 * 1024;

// ------------------------------------------------------------------- state ---

pub struct PapersState {
    /// inbox | all | expiring | vault.
    pub tab: String,
    pub notice: String,
    /// Papers still being read; Dart asks again until this is 0.
    pub reading: i64,
    /// Read and waiting to be filed: the Inbox count.
    pub unfiled: i64,
    /// Dates in the next three months: the Expiring count.
    pub soon: i64,
    pub alerts: Vec<PaperAlert>,
    pub inbox: Vec<InboxPaper>,
    /// Filed papers, for the line over the grid.
    pub total: i64,
    /// "1.2 GB".
    pub size: String,
    pub kinds: Vec<KindCount>,
    pub collections: Vec<KindCount>,
    /// A kind, or "all".
    pub kind: String,
    /// A collection's name, "@vault", or "" for any.
    pub collection: String,
    pub only_reminder: bool,
    pub only_unlinked: bool,
    /// newest | expiring | amount.
    pub sort: String,
    pub query: String,
    pub papers: Vec<PaperCard>,
    pub open: Option<PaperView>,
    pub months: Vec<MonthDots>,
    pub groups: Vec<ExpiryGroup>,
    /// Papers with a date to watch, for Expiring's header.
    pub dated: i64,
    pub vault_n: i64,
    pub vault_open: bool,
    pub has_pin: bool,
    pub pin_len: i64,
    /// The watched folder, or "".
    pub watch: String,
    /// Every collection in use, for the editor.
    pub names: Vec<String>,
    /// Finances accounts, when the open paper can be added to Finances.
    pub accounts: Vec<AccountPick>,
    /// tesseract is installed: scans and photos can be read.
    pub can_ocr: bool,
    /// The Cloud remote Papers is backed up to every day, or "".
    pub backup: String,
    /// Cloud's remotes, for the backup picker.
    pub remotes: Vec<String>,
}

pub struct PaperAlert {
    /// expiry | finance.
    pub kind: String,
    pub paper_id: i64,
    /// Days left for an expiry; papers to match for finance.
    pub n: i64,
    /// "42d", "14mo", or the count.
    pub ring: String,
    pub title: String,
    pub body: String,
    /// The Finances transaction behind it, for Open bill; 0 for none.
    pub fin_id: i64,
    /// ISO day a renewal should start by, for Plan renewal; "" when the kind
    /// does not renew.
    pub plan_on: String,
    /// A bill or contract that can be shopped around before it rolls over.
    pub compare: bool,
}

pub struct InboxPaper {
    pub id: i64,
    pub title: String,
    pub kind: String,
    /// reading | read | failed.
    pub status: String,
    pub error: String,
    pub confidence: i64,
    /// "Receipt · 14 Sep 2026 · £649.00".
    pub line: String,
    pub collection: String,
    pub vault: bool,
    /// "Warranty to 14 Sep 2028".
    pub expiry: String,
    /// "£649.00 in Finances".
    pub finance: String,
    pub thumb: String,
    /// Why the collection was suggested: "like your 6 other Currys papers".
    pub why: String,
}

pub struct KindCount {
    pub id: String,
    pub label: String,
    pub n: i64,
}

pub struct PaperCard {
    pub id: i64,
    pub title: String,
    pub kind: String,
    /// "14 Sep 2026", "Expires 31 Oct 2026", "64 pages".
    pub line: String,
    /// Empty when there is none, or the paper is sealed.
    pub thumb: String,
    pub vault: bool,
    pub reminder: bool,
    pub linked: bool,
}

pub struct PaperView {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub collection: String,
    pub vault: bool,
    /// In the vault while the vault is shut: no picture, no text, no Open.
    pub locked: bool,
    pub filed: bool,
    pub status: String,
    pub thumb: String,
    pub ext: String,
    pub fields: Vec<PaperField>,
    pub finance: Option<FinLink>,
    /// Has an amount and no Finances transaction yet.
    pub can_add: bool,
    /// Around the search words, or how it begins.
    pub snippet: String,
    /// Has a date that runs out: Snooze and Renewed apply.
    pub dated: bool,
    /// Photos taken on the paper's day and the three after, newest last.
    pub photos: Vec<i64>,
    /// "Photos from 14–17 Sep"; "" with none.
    pub photos_line: String,
}

pub struct PaperField {
    pub label: String,
    pub value: String,
    /// "reminder set", "snoozed", "passed".
    pub note: String,
}

pub struct FinLink {
    pub id: i64,
    /// "Finances · Currys".
    pub title: String,
    /// "−£649.00 · Amex · 14 Sep".
    pub sub: String,
}

pub struct MonthDots {
    /// "Oct".
    pub label: String,
    /// A kind for each date that month.
    pub kinds: Vec<String>,
}

pub struct ExpiryGroup {
    pub title: String,
    pub rows: Vec<ExpiryRow>,
}

pub struct ExpiryRow {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub days: i64,
    pub ring: String,
    /// "Expires 31 Oct 2026 · Renewal can take weeks. Start early."
    pub line: String,
    pub snoozed: bool,
}

/// A paper as the editor shows it.
pub struct PaperDraft {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub collection: String,
    /// ISO, or "".
    pub doc_date: String,
    /// "649.00", or "".
    pub amount: String,
    pub merchant: String,
    pub expires: String,
    pub expiry_label: String,
    pub serial: String,
}

// ---------------------------------------------------------------- commands ---

pub enum PapersCmd {
    Refresh,
    SetTab { tab: String },
    /// Search inside every paper; "" clears.
    Search { text: String },
    /// Chosen or dropped: copied in and read in the background.
    AddFiles { paths: Vec<String> },
    File { id: i64 },
    FileAll,
    Save {
        id: i64,
        title: String,
        kind: String,
        collection: String,
        doc_date: String,
        amount: String,
        merchant: String,
        expires: String,
        expiry_label: String,
        serial: String,
    },
    Delete { id: i64 },
    /// Read a paper that failed again — after installing tesseract, say.
    ReadAgain { id: i64 },
    /// The paper beside the grid; 0 closes it.
    Select { id: i64 },
    SetKind { kind: String },
    SetCollection { collection: String },
    SetOnly { reminder: bool, unlinked: bool },
    SetSort { sort: String },
    SetVault { id: i64, sealed: bool },
    Unlock { pin: String },
    LockVault,
    /// No alerts or reminders for a week.
    Snooze { id: i64 },
    /// The next date, or "" when it does not come round again.
    Renewed { id: i64, next: String },
    /// Link every filed paper that matches a Finances payment.
    MatchAll,
    Unlink { id: i64 },
    AddToFinances { id: i64, account_id: i64 },
    OpenFile { id: i64 },
    SaveCopy { id: i64, to: String },
    /// "" stops watching.
    WatchFolder { path: String },
    /// A calendar event on the day the renewal has to start.
    PlanRenewal { id: i64 },
    /// A web search for the same cover elsewhere.
    Compare { id: i64 },
    /// A Cloud remote to copy Papers to every day; "" stops.
    SetBackup { remote: String },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    query: String,
    kind: String,
    collection: String,
    only_reminder: bool,
    only_unlinked: bool,
    sort: String,
    open: i64,
    notice: String,
    unlocked_until: Option<Instant>,
    watched_at: Option<Instant>,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "inbox".into(),
            query: String::new(),
            kind: "all".into(),
            collection: String::new(),
            only_reminder: false,
            only_unlinked: false,
            sort: "newest".into(),
            open: 0,
            notice: String::new(),
            unlocked_until: None,
            watched_at: None,
        })
    })
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn say(notice: impl Into<String>) {
    lock().notice = notice.into();
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// Open without a PIN when none is set; otherwise for five minutes after the
/// PIN, and never while the app is locked.
fn vault_open() -> bool {
    use tulipix_core::account::{self, AppMode};
    if !account::has_pin() {
        return true;
    }
    if matches!(account::current(), AppMode::Locked) {
        return false;
    }
    lock().unlocked_until.is_some_and(|t| Instant::now() < t)
}

/// Where a vault paper is unsealed to be looked at. Emptied whenever the vault
/// is shut.
fn opened_dir() -> Result<PathBuf> {
    let d = tulipix_core::paths::cache_dir().ok_or_else(|| anyhow!("no cache folder"))?.join("papers-open");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn shut_opened() {
    if let Some(d) = tulipix_core::paths::cache_dir().map(|c| c.join("papers-open")) {
        std::fs::remove_dir_all(d).ok();
    }
}

fn can_ocr() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(tulipix_finances::ocr::available)
}

// ---------------------------------------------------------------- exported ---

pub async fn papers_dispatch(cmd: PapersCmd) -> Result<PapersState> {
    let pool = papers_pool().await?;
    apply(pool, cmd).await?;
    if let Err(e) = scan_watched(pool, false).await {
        tracing::info!(error = %e, "papers: the watched folder could not be read");
    }
    remind(pool).await;
    snapshot(pool).await
}

pub async fn papers_draft(id: i64) -> Result<PaperDraft> {
    let pool = papers_pool().await?;
    let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
    let currency = if r.currency.is_empty() { tulipix_finances::fx::base_currency() } else { r.currency.clone() };
    Ok(PaperDraft {
        amount: r.amount_minor.map(|a| plain_amount(a, &currency)).unwrap_or_default(),
        id,
        title: r.title,
        kind: r.kind,
        collection: r.collection,
        doc_date: r.doc_date,
        merchant: r.merchant,
        expires: r.expires,
        expiry_label: r.expiry_label,
        serial: r.serial,
    })
}

/// Scan with phone: the page's address and its QR code. A new code each time;
/// the old one stops working.
pub async fn papers_phone() -> Result<PhoneLink> {
    let url = p::phone::publish().await?;
    Ok(PhoneLink { qr: crate::api::transfer::render_qr(&url), url })
}

/// A plain copy of the paper for another section to send — Share hands it to
/// Transfer. A vault paper is unsealed into the folder emptied on lock, so the
/// vault must be open.
pub async fn papers_plain_path(id: i64) -> Result<String> {
    Ok(plain_copy(papers_pool().await?, id).await?.to_string_lossy().into_owned())
}

/// What Papers does with the section closed, on the shell's half-minute tick:
/// read what landed in the watched folder, send reminders that came due, and
/// refresh the copy of papers.db the Cloud backup carries. Nothing at all
/// until Papers has been opened once and made its database.
pub(crate) async fn background_tick() {
    if !tulipix_core::paths::db_path("papers").is_some_and(|f| f.exists()) {
        return;
    }
    let Ok(pool) = papers_pool().await else { return };
    if let Err(e) = scan_watched(pool, false).await {
        tracing::info!(error = %e, "papers: the watched folder could not be read");
    }
    remind(pool).await;
    static LAST_COPY: Mutex<Option<Instant>> = Mutex::new(None);
    let due = {
        let mut last = LAST_COPY.lock().unwrap_or_else(|e| e.into_inner());
        let due = last.is_none_or(|t| t.elapsed() > Duration::from_secs(6 * 3600));
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    // Only worth copying when a Cloud job carries it off the machine.
    if due
        && let Ok(root) = p::dir("")
        && !crate::api::cloud::backup_job(&root.to_string_lossy(), None).await.unwrap_or_default().is_empty()
    {
        snapshot_db(pool).await.ok();
    }
}

/// papers.db, consistent, beside the files the backup job copies. The vault's
/// text is sealed in it already.
async fn snapshot_db(pool: &SqlitePool) -> Result<()> {
    let to = p::dir("backup")?.join("papers.db");
    tokio::fs::remove_file(&to).await.ok();
    sqlx::query("VACUUM INTO ?").bind(to.to_string_lossy().into_owned()).execute(pool).await?;
    Ok(())
}

/// The day a renewal should start: early enough for the kind's usual wait.
/// None for kinds that do not renew. Never before today.
fn plan_on(kind: &str, end: NaiveDate) -> Option<NaiveDate> {
    let lead = match kind {
        // "Renewal takes up to 10 weeks."
        "id" => 70,
        "contract" | "bill" => 30,
        _ => return None,
    };
    Some((end - chrono::TimeDelta::days(lead)).max(today()))
}

/// A file sent from the phone page.
pub(crate) async fn take_upload(name: &str, bytes: Vec<u8>) -> Result<()> {
    let name = Path::new(name).file_name().and_then(|n| n.to_str()).unwrap_or("scan.jpg").to_string();
    let ext = Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_ascii_lowercase();
    if !p::readable(&ext) {
        bail!("only PDFs and pictures can be read");
    }
    let pool = papers_pool().await?;
    if add_bytes(pool, &name, &ext, bytes, "phone").await? {
        say(format!("{name} came in from the phone"));
    }
    Ok(())
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: PapersCmd) -> Result<()> {
    match cmd {
        PapersCmd::Refresh => {}
        PapersCmd::SetTab { tab } => lock().tab = tab,
        PapersCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            if !s.query.is_empty() && s.tab != "vault" {
                s.tab = "all".into();
            }
        }
        PapersCmd::AddFiles { paths } => {
            let (added, dup, skipped) = add_files(pool, &paths, "choose").await?;
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(format!("Reading {}", plural(added, "paper", "papers")));
            }
            if dup > 0 {
                parts.push(format!("{} already here", plural(dup, "was", "were")));
            }
            if skipped > 0 {
                parts.push(format!("{} not a PDF or a picture", plural(skipped, "was", "were")));
            }
            say(parts.join(" · "));
            lock().tab = "inbox".into();
        }
        PapersCmd::File { id } => {
            sqlx::query("UPDATE papers SET filed = 1 WHERE id = ? AND status = 'read'").bind(id).execute(pool).await?;
        }
        PapersCmd::FileAll => {
            let n = sqlx::query("UPDATE papers SET filed = 1 WHERE filed = 0 AND status = 'read'")
                .execute(pool)
                .await?
                .rows_affected();
            say(format!("Filed {}", plural(n as i64, "paper", "papers")));
        }
        PapersCmd::Save { id, title, kind, collection, doc_date, amount, merchant, expires, expiry_label, serial } => {
            save(pool, id, Edited { title, kind, collection, doc_date, amount, merchant, expires, expiry_label, serial }).await?
        }
        PapersCmd::Delete { id } => delete(pool, id).await?,
        PapersCmd::ReadAgain { id } => {
            sqlx::query("UPDATE papers SET status = 'reading', error = '' WHERE id = ?").bind(id).execute(pool).await?;
            spawn_read(pool, id);
        }
        PapersCmd::Select { id } => lock().open = id,
        PapersCmd::SetKind { kind } => lock().kind = kind,
        PapersCmd::SetCollection { collection } => lock().collection = collection,
        PapersCmd::SetOnly { reminder, unlinked } => {
            let mut s = lock();
            s.only_reminder = reminder;
            s.only_unlinked = unlinked;
        }
        PapersCmd::SetSort { sort } => lock().sort = sort,
        PapersCmd::SetVault { id, sealed } => {
            if !sealed && !vault_open() {
                bail!("unlock the vault first");
            }
            set_vault(pool, id, sealed).await?;
            say(if sealed { "Moved to the vault: sealed, and out of search" } else { "Taken out of the vault" });
        }
        PapersCmd::Unlock { pin } => {
            if !crate::api::lock::lock_verify_pin(pin) {
                bail!("That PIN is not right");
            }
            lock().unlocked_until = Some(Instant::now() + VAULT_OPEN_FOR);
        }
        PapersCmd::LockVault => {
            lock().unlocked_until = None;
            shut_opened();
        }
        PapersCmd::Snooze { id } => {
            let until = today() + chrono::TimeDelta::days(7);
            sqlx::query("UPDATE papers SET snoozed_until = ? WHERE id = ?").bind(until.to_string()).bind(id).execute(pool).await?;
            say("Snoozed for a week");
        }
        PapersCmd::Renewed { id, next } => {
            let next = if next.trim().is_empty() {
                String::new()
            } else {
                p::iso(&next).ok_or_else(|| anyhow!("the new date should look like 2027-10-31"))?.to_string()
            };
            sqlx::query("UPDATE papers SET expires = ?, snoozed_until = '' WHERE id = ?").bind(&next).bind(id).execute(pool).await?;
            say(match p::iso(&next) {
                Some(d) => format!("Next date {}: reminders 30 and 7 days before", p::long_day(d)),
                None => "Done: no more reminders for it".into(),
            });
        }
        PapersCmd::MatchAll => {
            let n = match_all(pool).await?;
            say(match n {
                0 => "Nothing new to match in Finances".to_string(),
                n => format!("Linked {} to Finances", plural(n, "paper", "papers")),
            });
        }
        PapersCmd::Unlink { id } => {
            sqlx::query("UPDATE papers SET fin_txn = NULL WHERE id = ?").bind(id).execute(pool).await?;
        }
        PapersCmd::AddToFinances { id, account_id } => add_to_finances(pool, id, account_id).await?,
        PapersCmd::OpenFile { id } => {
            let path = plain_copy(pool, id).await?;
            crate::api::transfer::open_url(&path.to_string_lossy());
        }
        PapersCmd::SaveCopy { id, to } => {
            let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
            let bytes = unsealed(&r).await?;
            tokio::fs::write(&to, bytes).await?;
            say(format!("Saved a copy of {}", r.title));
        }
        PapersCmd::WatchFolder { path } => watch(pool, path.trim()).await?,
        PapersCmd::PlanRenewal { id } => {
            let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
            let end = r.end().ok_or_else(|| anyhow!("this paper has no date to renew by"))?;
            let start = plan_on(&r.kind, end).ok_or_else(|| anyhow!("this kind of paper does not renew"))?;
            crate::ics::open(
                &format!("renew-{id}"),
                &[crate::ics::Event {
                    uid: format!("paper-{id}-{}", r.expires),
                    day: start,
                    title: format!("Start renewing: {}", r.title),
                    note: format!("{} {}. {}", r.label(), p::long_day(end), p::advice(&r.kind)).trim().to_string(),
                }],
            )?;
            say(format!("Sent to your calendar for {}", p::long_day(start)));
        }
        PapersCmd::Compare { id } => {
            let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
            let what = if r.merchant.is_empty() { r.title.clone() } else { format!("{} {}", r.merchant, p::kind_label(&r.kind)) };
            let url = reqwest::Url::parse_with_params("https://duckduckgo.com/", &[("q", format!("compare {what} renewal"))])?;
            crate::api::transfer::open_url(url.as_str());
        }
        PapersCmd::SetBackup { remote } => {
            let remote = remote.trim();
            let root = p::dir("")?;
            let dst = if remote.is_empty() { String::new() } else { format!("{remote}:Tulipix/Papers") };
            if !remote.is_empty() {
                snapshot_db(pool).await?;
            }
            crate::api::cloud::backup_job(&root.to_string_lossy(), Some(&dst)).await?;
            say(if remote.is_empty() {
                "Papers is no longer backed up".to_string()
            } else {
                format!("Backed up to {remote} every day; vault papers go up still sealed")
            });
        }
    }
    Ok(())
}

/// Copy files in and start reading them. Returns (added, already here, not
/// readable).
async fn add_files(pool: &'static SqlitePool, paths: &[String], source: &str) -> Result<(i64, i64, i64)> {
    let (mut added, mut dup, mut skipped) = (0, 0, 0);
    for path in paths {
        let src = Path::new(path);
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let size = tokio::fs::metadata(src).await.map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0);
        if !p::readable(&ext) || size == 0 || size > MAX_BYTES {
            skipped += 1;
            continue;
        }
        let bytes = tokio::fs::read(src).await?;
        let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if add_bytes(pool, &name, &ext, bytes, source).await? {
            added += 1;
        } else {
            dup += 1;
        }
    }
    Ok((added, dup, skipped))
}

/// One paper in: a row, a copy under `<data>/papers/files`, and a reader
/// started. False when the same bytes are already here.
async fn add_bytes(pool: &'static SqlitePool, name: &str, ext: &str, bytes: Vec<u8>, source: &str) -> Result<bool> {
    let sha = p::sha(&bytes);
    let there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM papers WHERE sha = ?").bind(&sha).fetch_one(pool).await?;
    if there > 0 {
        return Ok(false);
    }
    let stem = Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name).to_string();
    let id = sqlx::query("INSERT INTO papers (title, name, ext, bytes, sha, source, created) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&stem)
        .bind(name)
        .bind(ext)
        .bind(bytes.len() as i64)
        .bind(&sha)
        .bind(source)
        .bind(p::now())
        .execute(pool)
        .await?
        .last_insert_rowid();
    let file = p::dir("files")?.join(format!("{id}.{ext}"));
    tokio::fs::write(&file, &bytes).await?;
    sqlx::query("UPDATE papers SET file = ? WHERE id = ?").bind(file.to_string_lossy().to_string()).bind(id).execute(pool).await?;
    spawn_read(pool, id);
    Ok(true)
}

/// One reader at a time: tesseract takes every core it can get.
static READER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn spawn_read(pool: &'static SqlitePool, id: i64) {
    tokio::spawn(async move {
        let _one = READER.lock().await;
        if let Err(e) = read_one(pool, id).await {
            tracing::info!(error = %e, id, "papers: could not read a paper");
            sqlx::query("UPDATE papers SET status = 'failed', error = ? WHERE id = ?")
                .bind(e.to_string())
                .bind(id)
                .execute(pool)
                .await
                .ok();
        }
    });
}

async fn read_one(pool: &'static SqlitePool, id: i64) -> Result<()> {
    let Some((file, ext, name)): Option<(String, String, String)> =
        sqlx::query_as("SELECT file, ext, name FROM papers WHERE id = ?").bind(id).fetch_optional(pool).await?
    else {
        return Ok(());
    };
    let path = PathBuf::from(&file);
    let r = tokio::task::spawn_blocking(move || p::read(&path, &ext)).await??;
    let currency = tulipix_finances::fx::base_currency();
    let guess = tulipix_finances::ocr::parse(&r.text, &currency);
    let kind = p::classify(&r.text, r.pages);
    let doc = guess.occurred_on.as_deref().and_then(p::iso);
    let (expires, label) = p::expiry(&r.text, doc).map(|(d, l)| (d.to_string(), l.to_string())).unwrap_or_default();
    // An ID's first line is the issuing state, a manual's the maker's slogan;
    // neither is a shop, and the numbers on them are not money.
    let money_kind = matches!(kind, "receipt" | "bill" | "warranty" | "medical" | "other");
    let merchant = guess.merchant.filter(|_| money_kind).map(|m| p::proper(&m)).unwrap_or_default();
    let amount = guess.amount_minor.filter(|_| money_kind);
    // What the same shop's papers were filed as beats the word table: a bill
    // the table reads as a receipt, corrected once, stays corrected.
    let learned: Option<String> = if merchant.is_empty() {
        None
    } else {
        sqlx::query_scalar("SELECT kind FROM papers WHERE filed = 1 AND merchant = ? ORDER BY id DESC LIMIT 1")
            .bind(&merchant)
            .fetch_optional(pool)
            .await?
    };
    let kind = learned.as_deref().unwrap_or(kind);
    let title = p::title(kind, &merchant, &r.text, &name, doc);
    let collection = suggest(pool, kind, &merchant).await?;
    let fin = match (amount, doc) {
        (Some(a), Some(d)) => find_match(pool, a, d).await,
        _ => None,
    };
    sqlx::query(
        "UPDATE papers SET status = 'read', error = '', title = ?, kind = ?, collection = ?, body = ?, confidence = ?, \
         pages = ?, thumb = ?, doc_date = ?, amount_minor = ?, currency = ?, merchant = ?, serial = ?, expires = ?, \
         expiry_label = ?, fin_txn = COALESCE(fin_txn, ?) WHERE id = ?",
    )
    .bind(&title)
    .bind(kind)
    .bind(&collection)
    .bind(&r.text)
    .bind(r.confidence)
    .bind(r.pages)
    .bind(&r.thumb)
    .bind(doc.map(|d| d.to_string()).unwrap_or_default())
    .bind(amount)
    .bind(&currency)
    .bind(&merchant)
    .bind(p::serial(&r.text))
    .bind(&expires)
    .bind(&label)
    .bind(fin)
    .bind(id)
    .execute(pool)
    .await?;
    if p::private(kind) {
        set_vault(pool, id, true).await?;
    } else {
        index(pool, id).await?;
    }
    Ok(())
}

/// Where a paper like this was filed before: the same shop's last paper, then
/// the kind's most used collection, then the kind's default.
async fn suggest(pool: &SqlitePool, kind: &str, merchant: &str) -> Result<String> {
    if !merchant.is_empty()
        && let Some(c) = sqlx::query_scalar::<_, String>(
            "SELECT collection FROM papers WHERE filed = 1 AND merchant = ? AND collection != '' ORDER BY id DESC LIMIT 1",
        )
        .bind(merchant)
        .fetch_optional(pool)
        .await?
    {
        return Ok(c);
    }
    let c: Option<String> = sqlx::query_scalar(
        "SELECT collection FROM papers WHERE filed = 1 AND kind = ? AND collection != '' \
         GROUP BY collection ORDER BY COUNT(*) DESC LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(pool)
    .await?;
    Ok(c.unwrap_or_else(|| p::default_collection(kind).to_string()))
}

/// Put a paper's words in the search index — or take them out, for the vault.
async fn index(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM paper_fts WHERE rowid = ?").bind(id).execute(pool).await?;
    let row: Option<(String, String, String, String, String, String, bool)> =
        sqlx::query_as("SELECT title, body, merchant, collection, kind, serial, vault FROM papers WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    if let Some((title, body, merchant, collection, kind, serial, sealed)) = row
        && !sealed
    {
        sqlx::query("INSERT INTO paper_fts (rowid, title, body, extra) VALUES (?, ?, ?, ?)")
            .bind(id)
            .bind(title)
            .bind(body)
            .bind(format!("{merchant} {collection} {} {serial}", p::kind_label(&kind)))
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Seal a paper into the vault, or bring it back out.
async fn set_vault(pool: &SqlitePool, id: i64, sealed: bool) -> Result<()> {
    let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
    if r.vault == sealed {
        return Ok(());
    }
    let from = PathBuf::from(&r.file);
    if sealed {
        let plain = tokio::fs::read(&from).await?;
        let to = format!("{}.vault", r.file);
        tokio::fs::write(&to, vault::seal(&plain)?).await?;
        let text = vault::seal(r.body.as_bytes())?;
        sqlx::query("UPDATE papers SET vault = 1, file = ?, body = '', body_sealed = ?, thumb = '' WHERE id = ?")
            .bind(&to)
            .bind(text)
            .bind(id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM paper_fts WHERE rowid = ?").bind(id).execute(pool).await?;
        tokio::fs::remove_file(&from).await.ok();
        p::forget_pages(&from);
    } else {
        let plain = vault::open(&tokio::fs::read(&from).await?)?;
        let to = PathBuf::from(r.file.trim_end_matches(".vault"));
        tokio::fs::write(&to, plain).await?;
        let text = match &r.body_sealed {
            Some(b) => String::from_utf8_lossy(&vault::open(b)?).into_owned(),
            None => String::new(),
        };
        let (t2, ext) = (to.clone(), r.ext.clone());
        let thumb = tokio::task::spawn_blocking(move || p::thumb(&t2, &ext)).await?;
        sqlx::query("UPDATE papers SET vault = 0, file = ?, body = ?, body_sealed = NULL, thumb = ? WHERE id = ?")
            .bind(to.to_string_lossy().to_string())
            .bind(text)
            .bind(thumb)
            .bind(id)
            .execute(pool)
            .await?;
        index(pool, id).await?;
        tokio::fs::remove_file(&from).await.ok();
    }
    Ok(())
}

/// The paper's bytes, unsealed if it is in the vault — which must be open.
async fn unsealed(r: &Row) -> Result<Vec<u8>> {
    let bytes = tokio::fs::read(&r.file).await?;
    if !r.vault {
        return Ok(bytes);
    }
    if !vault_open() {
        bail!("unlock the vault first");
    }
    vault::open(&bytes)
}

/// A file the system viewer can open: our copy, or a vault paper unsealed
/// into a folder that is emptied when the vault shuts.
async fn plain_copy(pool: &SqlitePool, id: i64) -> Result<PathBuf> {
    let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
    if !r.vault {
        return Ok(PathBuf::from(&r.file));
    }
    let bytes = unsealed(&r).await?;
    let safe: String = r
        .title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { '_' })
        .take(60)
        .collect();
    let to = opened_dir()?.join(format!("{}.{}", safe.trim(), r.ext));
    tokio::fs::write(&to, bytes).await?;
    Ok(to)
}

struct Edited {
    title: String,
    kind: String,
    collection: String,
    doc_date: String,
    amount: String,
    merchant: String,
    expires: String,
    expiry_label: String,
    serial: String,
}

async fn save(pool: &SqlitePool, id: i64, e: Edited) -> Result<()> {
    let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
    if e.title.trim().is_empty() {
        bail!("a paper needs a name");
    }
    if !p::KINDS.iter().any(|(k, _)| *k == e.kind) {
        bail!("that is not a kind of paper");
    }
    let day = |s: &str, what: &str| -> Result<String> {
        if s.trim().is_empty() {
            return Ok(String::new());
        }
        p::iso(s).map(|d| d.to_string()).ok_or_else(|| anyhow!("the {what} should look like 2026-09-14"))
    };
    let doc_date = day(&e.doc_date, "date")?;
    let expires = day(&e.expires, "end date")?;
    let currency = if r.currency.is_empty() { tulipix_finances::fx::base_currency() } else { r.currency.clone() };
    let amount = if e.amount.trim().is_empty() {
        None
    } else {
        Some(tulipix_finances::money::parse_amount(e.amount.trim(), &currency)?)
    };
    let label = if expires.is_empty() {
        String::new()
    } else if e.expiry_label.trim().is_empty() {
        "Expires".to_string()
    } else {
        e.expiry_label.trim().to_string()
    };
    sqlx::query(
        "UPDATE papers SET title = ?, kind = ?, collection = ?, doc_date = ?, amount_minor = ?, currency = ?, \
         merchant = ?, expires = ?, expiry_label = ?, serial = ? WHERE id = ?",
    )
    .bind(e.title.trim())
    .bind(&e.kind)
    .bind(e.collection.trim())
    .bind(&doc_date)
    .bind(amount)
    .bind(&currency)
    .bind(e.merchant.trim())
    .bind(&expires)
    .bind(&label)
    .bind(e.serial.trim())
    .bind(id)
    .execute(pool)
    .await?;
    if r.expires != expires {
        sqlx::query("UPDATE papers SET snoozed_until = '' WHERE id = ?").bind(id).execute(pool).await?;
    }
    index(pool, id).await
}

async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    let Some(r) = row(pool, id).await? else { return Ok(()) };
    sqlx::query("DELETE FROM papers WHERE id = ?").bind(id).execute(pool).await?;
    sqlx::query("DELETE FROM paper_fts WHERE rowid = ?").bind(id).execute(pool).await?;
    sqlx::query("DELETE FROM reminded WHERE paper_id = ?").bind(id).execute(pool).await?;
    if !r.file.is_empty() {
        tokio::fs::remove_file(&r.file).await.ok();
        p::forget_pages(Path::new(&r.file));
    }
    let mut s = lock();
    if s.open == id {
        s.open = 0;
    }
    s.notice = format!("Deleted {}", r.title);
    Ok(())
}

// ---------------------------------------------------------------- finances ---

/// An expense in Finances for the same amount within four days of the paper's
/// date, that no other paper has claimed.
///
/// ponytail: amounts are compared as they were read, in the base currency; a
/// paper in another currency will not find its payment.
async fn find_match(pool: &SqlitePool, amount: i64, day: NaiveDate) -> Option<i64> {
    let fp = crate::db::finances_pool().await.ok()?;
    let taken: HashSet<i64> = sqlx::query_scalar::<_, i64>("SELECT fin_txn FROM papers WHERE fin_txn IS NOT NULL")
        .fetch_all(pool)
        .await
        .ok()?
        .into_iter()
        .collect();
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM transactions WHERE kind = 'expense' AND amount_minor = ?1 \
         AND occurred_on BETWEEN date(?2, '-4 days') AND date(?2, '+4 days') \
         ORDER BY abs(julianday(occurred_on) - julianday(?2))",
    )
    .bind(amount)
    .bind(day.to_string())
    .fetch_all(fp)
    .await
    .ok()?;
    ids.into_iter().find(|id| !taken.contains(id))
}

/// Filed receipts and bills with an amount and no Finances link yet, newest
/// first: (id, title, amount, currency, date).
async fn unlinked(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String, i64, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, title, amount_minor, currency, doc_date FROM papers WHERE filed = 1 AND fin_txn IS NULL \
         AND amount_minor IS NOT NULL AND doc_date != '' AND kind IN ('receipt', 'bill') ORDER BY id DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

async fn match_all(pool: &SqlitePool) -> Result<i64> {
    let mut n = 0;
    for (id, _, amount, _, day) in unlinked(pool, 200).await? {
        let Some(d) = p::iso(&day) else { continue };
        if let Some(t) = find_match(pool, amount, d).await {
            sqlx::query("UPDATE papers SET fin_txn = ? WHERE id = ?").bind(t).bind(id).execute(pool).await?;
            n += 1;
        }
    }
    Ok(n)
}

async fn add_to_finances(pool: &SqlitePool, id: i64, account_id: i64) -> Result<()> {
    use tulipix_finances::{accounts, fx, money, txn};
    let r = row(pool, id).await?.ok_or_else(|| anyhow!("that paper is gone"))?;
    let amount = r.amount_minor.ok_or_else(|| anyhow!("this paper has no amount — Edit it to add one"))?;
    let fp = crate::db::finances_pool().await?;
    let acct = accounts::list(fp, false)
        .await?
        .into_iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| anyhow!("that account is not in Finances any more"))?;
    let day = if r.doc_date.is_empty() { today().to_string() } else { r.doc_date.clone() };
    let what = if r.merchant.is_empty() { r.title.clone() } else { r.merchant.clone() };
    let mut t = txn::NewTxn::expense(acct.id, amount, &acct.currency, &day, &what);
    t.rate_micro = fx::rate_for(fp, &acct.currency, &fx::base_currency()).await?;
    t.note = Some(format!("From Papers: {}", r.title));
    let tid = txn::post(fp, &t).await?;
    sqlx::query("UPDATE papers SET fin_txn = ? WHERE id = ?").bind(tid).bind(id).execute(pool).await?;
    say(format!("Added {} to Finances", money::format_minor(amount, &acct.currency)));
    Ok(())
}

async fn fin_link(id: i64) -> Option<FinLink> {
    let fp = crate::db::finances_pool().await.ok()?;
    let (desc, amount, currency, day, account): (String, i64, String, String, String) = sqlx::query_as(
        "SELECT t.description, t.amount_minor, t.currency, t.occurred_on, a.name \
         FROM transactions t JOIN accounts a ON a.id = t.account_id WHERE t.id = ?",
    )
    .bind(id)
    .fetch_optional(fp)
    .await
    .ok()??;
    let when = p::iso(&day).map(|d| d.format("%-d %b").to_string()).unwrap_or(day);
    Some(FinLink {
        id,
        title: format!("Finances · {desc}"),
        sub: format!("−{} · {account} · {when}", tulipix_finances::money::format_minor(amount, &currency)),
    })
}

fn money(amount: i64, currency: &str) -> String {
    let c = if currency.is_empty() { tulipix_finances::fx::base_currency() } else { currency.to_string() };
    tulipix_finances::money::format_minor(amount, &c)
}

/// "649.00": the amount as the editor's text box holds it.
fn plain_amount(amount: i64, currency: &str) -> String {
    money(amount, currency).chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-').collect()
}

// --------------------------------------------------------- folder, reminders ---

async fn watch(pool: &'static SqlitePool, path: &str) -> Result<()> {
    if path.is_empty() {
        sqlx::query("DELETE FROM prefs WHERE key = 'watch'").execute(pool).await?;
        say("Stopped watching the folder");
        return Ok(());
    }
    if !Path::new(path).is_dir() {
        bail!("that folder is not there");
    }
    sqlx::query("INSERT OR REPLACE INTO prefs (key, value) VALUES ('watch', ?)").bind(path).execute(pool).await?;
    let n = scan_watched(pool, true).await?;
    let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path);
    say(format!("Watching {name}: {} in it now, and anything added later", plural(n, "paper", "papers")));
    Ok(())
}

/// The folder Papers takes new files from, or empty. Settings › Libraries
/// lists it.
pub(crate) async fn watch_folder() -> String {
    match papers_pool().await {
        Ok(pool) => watched(pool).await.unwrap_or_default(),
        Err(_) => String::new(),
    }
}

async fn watched(pool: &SqlitePool) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT value FROM prefs WHERE key = 'watch'").fetch_optional(pool).await?.unwrap_or_default())
}

/// New files in the watched folder, read in. At most every half minute unless
/// forced; a file is only ever taken once, so one deleted here stays deleted.
///
/// ponytail: polled on the shell's half-minute tick (`background_tick`), so a
/// scan lands within 30 s with Papers closed; a `notify` watcher if that lag
/// ever matters.
async fn scan_watched(pool: &'static SqlitePool, force: bool) -> Result<i64> {
    {
        let mut s = lock();
        if !force && s.watched_at.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            return Ok(0);
        }
        s.watched_at = Some(Instant::now());
    }
    let folder = watched(pool).await?;
    if folder.is_empty() {
        return Ok(0);
    }
    let mut fresh = Vec::new();
    let mut rd = tokio::fs::read_dir(&folder).await?;
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !p::readable(&ext) || !entry.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let s = path.to_string_lossy().to_string();
        let new = sqlx::query("INSERT OR IGNORE INTO seen (path) VALUES (?)").bind(&s).execute(pool).await?.rows_affected() == 1;
        if new {
            fresh.push(s);
        }
    }
    if fresh.is_empty() {
        return Ok(0);
    }
    let (added, _, _) = add_files(pool, &fresh, "folder").await?;
    if added > 0 && !force {
        say(format!("{} from the watched folder", plural(added, "new paper", "new papers")));
    }
    Ok(added)
}

/// A desktop notification 30 days and 7 days before anything runs out — once
/// each, per date. Filed papers only; a snoozed one waits out its week.
/// Checked on every refresh and on the shell's tick, so they arrive on the day
/// whichever section is open.
async fn remind(pool: &SqlitePool) {
    let t = today();
    let rows: Vec<(i64, String, String, String, String, String)> = match sqlx::query_as(
        "SELECT id, title, kind, expires, expiry_label, snoozed_until FROM papers WHERE filed = 1 AND expires != ''",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(_) => return,
    };
    for (id, title, kind, expires, label, snoozed) in rows {
        let Some(end) = p::iso(&expires) else { continue };
        if p::iso(&snoozed).is_some_and(|s| s > t) {
            continue;
        }
        let days = (end - t).num_days();
        let mut fresh = false;
        for mark in [30, 7] {
            if (0..=mark).contains(&days) {
                let r = sqlx::query("INSERT OR IGNORE INTO reminded (paper_id, mark, expires) VALUES (?, ?, ?)")
                    .bind(id)
                    .bind(mark)
                    .bind(&expires)
                    .execute(pool)
                    .await;
                fresh |= r.is_ok_and(|r| r.rows_affected() == 1);
            }
        }
        if fresh {
            crate::api::maintenance::notify(&p::when_line(&title, &label, days, end), p::advice(&kind));
        }
    }
}

// ---------------------------------------------------------------- snapshot ---

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    title: String,
    kind: String,
    collection: String,
    file: String,
    ext: String,
    pages: i64,
    thumb: String,
    body: String,
    body_sealed: Option<Vec<u8>>,
    status: String,
    error: String,
    confidence: i64,
    filed: bool,
    vault: bool,
    doc_date: String,
    amount_minor: Option<i64>,
    currency: String,
    merchant: String,
    serial: String,
    expires: String,
    expiry_label: String,
    snoozed_until: String,
    fin_txn: Option<i64>,
    bytes: i64,
}

/// Every column but the text — for lists, where the text is dead weight. A
/// constant, so the three queries that splice it in are safe to assert.
const LIGHT: &str = "id, title, kind, collection, file, ext, pages, thumb, '' AS body, NULL AS body_sealed, status, error, \
                     confidence, filed, vault, doc_date, amount_minor, currency, merchant, serial, expires, expiry_label, \
                     snoozed_until, fin_txn, bytes";

async fn row(pool: &SqlitePool, id: i64) -> Result<Option<Row>> {
    Ok(sqlx::query_as::<_, Row>(
        "SELECT id, title, kind, collection, file, ext, pages, thumb, body, body_sealed, status, error, confidence, \
         filed, vault, doc_date, amount_minor, currency, merchant, serial, expires, expiry_label, snoozed_until, \
         fin_txn, bytes FROM papers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

impl Row {
    fn day(&self) -> Option<NaiveDate> {
        p::iso(&self.doc_date)
    }
    fn end(&self) -> Option<NaiveDate> {
        p::iso(&self.expires)
    }
    fn label(&self) -> &str {
        if self.expiry_label.is_empty() { "Expires" } else { &self.expiry_label }
    }
    fn snoozed(&self) -> bool {
        p::iso(&self.snoozed_until).is_some_and(|s| s > today())
    }
    fn amount(&self) -> Option<String> {
        self.amount_minor.map(|a| money(a, &self.currency))
    }
    fn unlinked(&self) -> bool {
        self.amount_minor.is_some() && self.fin_txn.is_none() && matches!(self.kind.as_str(), "receipt" | "bill")
    }
    /// "Warranty to 14 Sep 2028", "Expires 31 Oct 2026".
    fn end_line(&self) -> String {
        match self.end() {
            Some(d) if self.label() == "Warranty ends" => format!("Warranty to {}", p::long_day(d)),
            Some(d) => format!("{} {}", self.label(), p::long_day(d)),
            None => String::new(),
        }
    }
}

fn size(bytes: i64) -> String {
    let b = bytes as f64;
    match bytes {
        n if n >= 1 << 30 => format!("{:.1} GB", b / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.0} MB", b / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.0} KB", b / 1024.0),
        n => format!("{n} bytes"),
    }
}

/// FTS words from what was typed: each word a prefix, all of them required.
pub(crate) fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Papers whose words match: the index, plus — while it is open — the vault,
/// read in memory.
async fn search(pool: &SqlitePool, q: &str, unlocked: bool) -> Result<HashSet<i64>> {
    let fq = fts_query(q);
    let mut hits: HashSet<i64> = if fq.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar::<_, i64>("SELECT rowid FROM paper_fts WHERE paper_fts MATCH ?")
            .bind(fq)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect()
    };
    if unlocked {
        let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
        let sealed: Vec<(i64, String, Option<Vec<u8>>)> =
            sqlx::query_as("SELECT id, title, body_sealed FROM papers WHERE vault = 1").fetch_all(pool).await?;
        for (id, title, blob) in sealed {
            let body = blob.and_then(|b| vault::open(&b).ok()).map(|b| String::from_utf8_lossy(&b).to_lowercase()).unwrap_or_default();
            let hay = format!("{} {body}", title.to_lowercase());
            if !words.is_empty() && words.iter().all(|w| hay.contains(w.as_str())) {
                hits.insert(id);
            }
        }
    }
    Ok(hits)
}

async fn snapshot(pool: &'static SqlitePool) -> Result<PapersState> {
    let (tab, query, kind, collection, only_reminder, only_unlinked, sort, open, notice) = {
        let mut s = lock();
        (
            s.tab.clone(),
            s.query.clone(),
            s.kind.clone(),
            s.collection.clone(),
            s.only_reminder,
            s.only_unlinked,
            s.sort.clone(),
            s.open,
            std::mem::take(&mut s.notice),
        )
    };
    let unlocked = vault_open();
    if !unlocked {
        shut_opened();
    }
    let t = today();
    let reading: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM papers WHERE status = 'reading'").fetch_one(pool).await?;
    let unfiled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM papers WHERE filed = 0 AND status != 'reading'")
        .fetch_one(pool)
        .await?;
    let vault_n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM papers WHERE vault = 1 AND filed = 1").fetch_one(pool).await?;

    // Every paper with a date to watch, soonest first.
    let dated: Vec<(Row, i64)> = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(&*format!(
        "SELECT {LIGHT} FROM papers WHERE expires != '' AND status = 'read' ORDER BY expires"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter_map(|r| {
        let days = (r.end()? - t).num_days();
        Some((r, days))
    })
    .collect();
    let soon = dated.iter().filter(|(_, d)| (-30..=92).contains(d)).count() as i64;

    let mut alerts = Vec::new();
    let mut inbox = Vec::new();
    if tab == "inbox" {
        for (r, days) in dated.iter().filter(|(r, d)| (-30..=92).contains(d) && !r.snoozed()).take(2) {
            let mut body = p::advice(&r.kind).to_string();
            if r.vault {
                body.push_str(if body.is_empty() { "It is in the vault." } else { " It is in the vault." });
            }
            alerts.push(PaperAlert {
                kind: "expiry".into(),
                paper_id: r.id,
                n: *days,
                ring: p::ring(*days),
                title: p::when_line(&r.title, r.label(), *days, r.end().unwrap_or(t)),
                body,
                fin_id: r.fin_txn.unwrap_or(0),
                plan_on: r.end().and_then(|e| plan_on(&r.kind, e)).map(|d| d.to_string()).unwrap_or_default(),
                compare: matches!(r.kind.as_str(), "bill" | "contract"),
            });
        }
        let mut matched: Vec<String> = Vec::new();
        for (_, title, amount, currency, day) in unlinked(pool, 20).await? {
            if let Some(d) = p::iso(&day)
                && find_match(pool, amount, d).await.is_some()
            {
                matched.push(format!("{title} {}", money(amount, &currency)));
            }
        }
        if !matched.is_empty() {
            let n = matched.len() as i64;
            let names = match matched.len() {
                1 => matched[0].clone(),
                2 => format!("{} and {}", matched[0], matched[1]),
                _ => format!("{}, {} and {} more", matched[0], matched[1], n - 2),
            };
            alerts.push(PaperAlert {
                kind: "finance".into(),
                paper_id: 0,
                n,
                ring: n.to_string(),
                title: if n == 1 { "1 receipt isn’t in Finances".into() } else { format!("{n} receipts aren’t in Finances") },
                body: format!("{names} {} card payments.", if n == 1 { "matches" } else { "match" }),
                fin_id: 0,
                plan_on: String::new(),
                compare: false,
            });
        }
        let rows = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(&*format!("SELECT {LIGHT} FROM papers WHERE filed = 0 ORDER BY id DESC")))
            .fetch_all(pool)
            .await?;
        let mut whys = Vec::with_capacity(rows.len());
        for r in &rows {
            whys.push(why(pool, &r.merchant, &r.kind, &r.collection).await);
        }
        inbox = rows
            .into_iter()
            .zip(whys)
            .map(|(r, why)| {
                let line = match r.status.as_str() {
                    "reading" => "Reading…".to_string(),
                    "failed" => r.error.clone(),
                    _ => {
                        let mut bits = vec![p::kind_label(&r.kind).to_string()];
                        if r.kind == "manual" && r.pages > 1 {
                            bits.push(format!("{} pages", r.pages));
                        } else if let Some(d) = r.day() {
                            bits.push(p::long_day(d));
                        }
                        if let Some(m) = r.amount() {
                            bits.push(m);
                        }
                        bits.join(" · ")
                    }
                };
                InboxPaper {
                    finance: match (r.fin_txn, r.amount()) {
                        (Some(_), Some(m)) => format!("{m} in Finances"),
                        _ => String::new(),
                    },
                    expiry: r.end_line(),
                    thumb: if r.vault { String::new() } else { r.thumb.clone() },
                    id: r.id,
                    title: r.title,
                    kind: r.kind,
                    status: r.status,
                    error: r.error,
                    confidence: r.confidence,
                    line,
                    collection: r.collection,
                    vault: r.vault,
                    why,
                }
            })
            .collect();
    }

    // All papers and the vault: filed papers, the rail's counts, the grid.
    let filed: Vec<Row> = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(&*format!("SELECT {LIGHT} FROM papers WHERE filed = 1")))
        .fetch_all(pool)
        .await?;
    let total = filed.len() as i64;
    let size_of = size(filed.iter().map(|r| r.bytes).sum());
    let mut kinds = vec![KindCount { id: "all".into(), label: "All".into(), n: total }];
    for (k, label) in p::KINDS {
        let n = filed.iter().filter(|r| r.kind == k).count() as i64;
        // Every kind, as the rail and the editor list them; Other only once used.
        if n > 0 || k != "other" {
            kinds.push(KindCount { id: k.into(), label: label.into(), n });
        }
    }
    let mut collections: Vec<KindCount> = Vec::new();
    for r in filed.iter().filter(|r| !r.collection.is_empty()) {
        match collections.iter_mut().find(|c| c.id == r.collection) {
            Some(c) => c.n += 1,
            None => collections.push(KindCount { id: r.collection.clone(), label: r.collection.clone(), n: 1 }),
        }
    }
    collections.sort_by(|a, b| b.n.cmp(&a.n).then_with(|| a.label.cmp(&b.label)));
    if vault_n > 0 {
        collections.push(KindCount { id: "@vault".into(), label: "Vault".into(), n: vault_n });
    }

    let mut papers = Vec::new();
    if tab == "all" || tab == "vault" {
        let hits = if query.is_empty() { None } else { Some(search(pool, &query, unlocked).await?) };
        let mut keep: Vec<&Row> = filed
            .iter()
            .filter(|r| {
                if tab == "vault" {
                    return r.vault && unlocked && hits.as_ref().is_none_or(|h| h.contains(&r.id));
                }
                (kind == "all" || r.kind == kind)
                    && match collection.as_str() {
                        "" => true,
                        "@vault" => r.vault,
                        c => r.collection == c,
                    }
                    && (!only_reminder || r.end().is_some_and(|e| e >= t))
                    && (!only_unlinked || r.unlinked())
                    && hits.as_ref().is_none_or(|h| h.contains(&r.id))
            })
            .collect();
        match sort.as_str() {
            "expiring" => keep.sort_by_key(|r| (r.end().is_none(), r.end(), std::cmp::Reverse(r.id))),
            "amount" => keep.sort_by_key(|r| (std::cmp::Reverse(r.amount_minor.unwrap_or(-1)), std::cmp::Reverse(r.id))),
            _ => keep.sort_by_key(|r| std::cmp::Reverse((r.day().unwrap_or(NaiveDate::MIN), r.id))),
        }
        papers = keep
            .into_iter()
            .map(|r| PaperCard {
                line: if r.end().is_some() && r.kind != "receipt" {
                    r.end_line()
                } else if r.kind == "manual" && r.pages > 1 {
                    format!("{} pages", r.pages)
                } else {
                    r.day().map(p::long_day).unwrap_or_default()
                },
                thumb: if r.vault { String::new() } else { r.thumb.clone() },
                reminder: r.end().is_some_and(|e| e >= t),
                linked: r.fin_txn.is_some(),
                id: r.id,
                title: r.title.clone(),
                kind: r.kind.clone(),
                vault: r.vault,
            })
            .collect();
    }

    let open_view = if open != 0 { paper_view(pool, open, unlocked, &query).await? } else { None };
    if open != 0 && open_view.is_none() {
        lock().open = 0;
    }

    // Expiring: twelve months of dots and the dates in groups.
    let mut months = Vec::new();
    let mut groups = Vec::new();
    if tab == "expiring" {
        let mut start = NaiveDate::from_ymd_opt(t.year(), t.month(), 1).unwrap_or(t);
        for _ in 0..12 {
            let next = start.checked_add_months(chrono::Months::new(1)).unwrap_or(start);
            months.push(MonthDots {
                label: start.format("%b").to_string(),
                kinds: dated
                    .iter()
                    .filter(|(r, _)| r.end().is_some_and(|e| e >= start && e < next))
                    .map(|(r, _)| r.kind.clone())
                    .collect(),
            });
            start = next;
        }
        let spans: [(&str, i64, i64); 4] = [
            ("Passed", i64::MIN, -1),
            ("This month", 0, 31),
            ("Next three months", 32, 92),
            ("Later", 93, i64::MAX),
        ];
        for (title, lo, hi) in spans {
            let rows: Vec<ExpiryRow> = dated
                .iter()
                .filter(|(_, d)| (lo..=hi).contains(d))
                .map(|(r, d)| {
                    let advice = p::advice(&r.kind);
                    ExpiryRow {
                        id: r.id,
                        title: r.title.clone(),
                        kind: r.kind.clone(),
                        days: *d,
                        ring: p::ring(*d),
                        line: if advice.is_empty() { r.end_line() } else { format!("{} · {advice}", r.end_line()) },
                        snoozed: r.snoozed(),
                    }
                })
                .collect();
            if !rows.is_empty() {
                groups.push(ExpiryGroup { title: title.into(), rows });
            }
        }
    }

    let names: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT collection FROM papers WHERE collection != '' ORDER BY collection COLLATE NOCASE")
            .fetch_all(pool)
            .await?;
    let accounts = if open_view.as_ref().is_some_and(|v| v.can_add) {
        match crate::db::finances_pool().await {
            Ok(fp) => tulipix_finances::accounts::list(fp, false)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| AccountPick { id: a.id, name: a.name, currency: a.currency })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let lc = crate::api::lock::lock_config();

    Ok(PapersState {
        tab,
        notice,
        reading,
        unfiled,
        soon,
        alerts,
        inbox,
        total,
        size: size_of,
        kinds,
        collections,
        kind,
        collection,
        only_reminder,
        only_unlinked,
        sort,
        query,
        papers,
        open: open_view,
        months,
        groups,
        dated: dated.len() as i64,
        vault_n,
        vault_open: unlocked,
        has_pin: lc.has_pin,
        pin_len: lc.pin_len as i64,
        watch: watched(pool).await?,
        names,
        accounts,
        can_ocr: can_ocr(),
        backup: crate::api::cloud::backup_job(&p::dir("")?.to_string_lossy(), None).await.unwrap_or_default(),
        remotes: crate::api::cloud::remote_names().await,
    })
}

/// The reason under a suggestion, from the papers already filed there.
async fn why(pool: &SqlitePool, merchant: &str, kind: &str, collection: &str) -> String {
    if collection.is_empty() {
        return String::new();
    }
    let count = |sql: &'static str, key: &str| {
        sqlx::query_scalar::<_, i64>(sql).bind(key.to_string()).bind(collection.to_string()).fetch_one(pool)
    };
    if !merchant.is_empty() {
        let n = count("SELECT COUNT(*) FROM papers WHERE filed = 1 AND merchant = ? AND collection = ?", merchant)
            .await
            .unwrap_or(0);
        if n > 0 {
            return format!("like your {} from {merchant}", plural(n, "other paper", "other papers"));
        }
    }
    let n = count("SELECT COUNT(*) FROM papers WHERE filed = 1 AND kind = ? AND collection = ?", kind).await.unwrap_or(0);
    if n > 0 {
        format!("where {} like it {} filed", n, if n == 1 { "is" } else { "are" })
    } else {
        String::new()
    }
}

async fn paper_view(pool: &SqlitePool, id: i64, unlocked: bool, query: &str) -> Result<Option<PaperView>> {
    let Some(r) = row(pool, id).await? else { return Ok(None) };
    let locked = r.vault && !unlocked;
    let t = today();
    let mut fields = Vec::new();
    let field = |label: &str, value: String, note: &str| PaperField { label: label.into(), value, note: note.into() };
    if !r.merchant.is_empty() {
        fields.push(field(if r.kind == "receipt" { "Shop" } else { "From" }, r.merchant.clone(), ""));
    }
    if let Some(d) = r.day() {
        fields.push(field("Date", p::long_day(d), ""));
    }
    if let Some(m) = r.amount() {
        fields.push(field(if r.kind == "receipt" { "Total" } else { "Amount" }, m, ""));
    }
    if let Some(e) = r.end() {
        let note = if e < t {
            "passed"
        } else if r.snoozed() {
            "snoozed"
        } else {
            "reminder set"
        };
        fields.push(field(r.label(), p::long_day(e), note));
    }
    if !r.serial.is_empty() {
        fields.push(field("Serial", r.serial.clone(), ""));
    }
    if r.pages > 1 {
        fields.push(field("Pages", r.pages.to_string(), ""));
    }
    if r.confidence > 0 && r.confidence < 100 {
        fields.push(field("Read", format!("{}% sure", r.confidence), ""));
    }
    let text = if locked {
        String::new()
    } else if r.vault {
        r.body_sealed
            .as_ref()
            .and_then(|b| vault::open(b).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    } else {
        r.body.clone()
    };
    let finance = match r.fin_txn {
        Some(tid) => fin_link(tid).await,
        None => None,
    };
    // Photos from the day it was bought and the three after: the thing as it
    // arrived, the damage for a claim. Not for a sealed paper.
    let (mut photos, mut photos_line) = (Vec::new(), String::new());
    if let Some(d) = r.day().filter(|_| !locked && matches!(r.kind.as_str(), "receipt" | "warranty" | "medical" | "other")) {
        let mut last = None;
        for i in 0..4 {
            let day = d + chrono::TimeDelta::days(i);
            let got = crate::journal::day_photos(day).await;
            if !got.is_empty() {
                last = Some(day);
            }
            photos.extend(got.into_iter().map(|p| p.0));
        }
        // ponytail: every photo of those days, capped; a link table would keep
        // only the ones the user says belong.
        photos.truncate(8);
        if let Some(end) = last {
            photos_line = if end == d {
                format!("Photos from {}", d.format("%-d %b"))
            } else {
                format!("Photos from {}–{}", d.format("%-d"), end.format("%-d %b"))
            };
        }
    }
    Ok(Some(PaperView {
        photos,
        photos_line,
        can_add: r.amount_minor.is_some() && finance.is_none(),
        snippet: p::snippet(&text, query),
        thumb: if r.vault { String::new() } else { r.thumb.clone() },
        dated: r.end().is_some(),
        finance,
        fields,
        locked,
        id: r.id,
        title: r.title,
        kind: r.kind,
        collection: r.collection,
        vault: r.vault,
        filed: r.filed,
        status: r.status,
        ext: r.ext,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_words_become_prefixes() {
        assert_eq!(fts_query("washing mach"), "\"washing\"* \"mach\"*");
        assert_eq!(fts_query("  \"quoted\" (x) "), "\"quoted\"* \"x\"*");
        assert_eq!(fts_query("—"), "");
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(size(1_288_490_189), "1.2 GB");
        assert_eq!(size(5 * 1024 * 1024), "5 MB");
        assert_eq!(size(2048), "2 KB");
        assert_eq!(size(12), "12 bytes");
    }
}
