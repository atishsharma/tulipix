// The Papers section's store, and the reading it rests on.
//
// `papers.db` holds every paper Tulipix has read: what it is, the fields read
// off it, where its copy lives and whether it is in the vault. The parts worth
// testing live here as plain functions: tesseract's TSV turned into text and a
// confidence, a receipt told from a passport, the dates written on a line, and
// the date something runs out.
//
// Reading is done by what is already on the machine: `pdftotext` for PDFs with
// a text layer, Books' page renderer plus `tesseract` for scans and photos.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Datelike, NaiveDate};
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS papers (
    id            INTEGER PRIMARY KEY,
    title         TEXT    NOT NULL DEFAULT '',
    -- receipt | bill | warranty | id | contract | medical | tax | manual | other
    kind          TEXT    NOT NULL DEFAULT 'other',
    collection    TEXT    NOT NULL DEFAULT '',
    file          TEXT    NOT NULL DEFAULT '',   -- our copy; `<id>.<ext>.vault` when sealed
    name          TEXT    NOT NULL DEFAULT '',   -- the file name it came in with
    ext           TEXT    NOT NULL DEFAULT '',
    bytes         INTEGER NOT NULL DEFAULT 0,
    pages         INTEGER NOT NULL DEFAULT 0,
    sha           TEXT    NOT NULL DEFAULT '',
    thumb         TEXT    NOT NULL DEFAULT '',   -- a picture of page one, or ''
    body          TEXT    NOT NULL DEFAULT '',   -- the text read off it; '' in the vault
    body_sealed   BLOB,                          -- the same text, sealed, in the vault
    confidence    INTEGER NOT NULL DEFAULT 0,    -- 0-100
    status        TEXT    NOT NULL DEFAULT 'reading', -- reading | read | failed
    error         TEXT    NOT NULL DEFAULT '',
    filed         INTEGER NOT NULL DEFAULT 0,
    vault         INTEGER NOT NULL DEFAULT 0,
    doc_date      TEXT    NOT NULL DEFAULT '',   -- ISO
    amount_minor  INTEGER,
    currency      TEXT    NOT NULL DEFAULT '',
    merchant      TEXT    NOT NULL DEFAULT '',
    serial        TEXT    NOT NULL DEFAULT '',
    expires       TEXT    NOT NULL DEFAULT '',   -- ISO: runs out, renews or falls due
    expiry_label  TEXT    NOT NULL DEFAULT '',   -- Expires | Renews | Ends | Due | Warranty ends …
    snoozed_until TEXT    NOT NULL DEFAULT '',   -- ISO
    fin_txn       INTEGER,                       -- a Finances transaction
    source        TEXT    NOT NULL DEFAULT '',   -- choose | drop | phone | folder
    created       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS papers_sha_idx ON papers(sha);

-- Searchable text, rowid = papers.id. Vault papers are never in here.
CREATE VIRTUAL TABLE IF NOT EXISTS paper_fts USING fts5(
    title, body, extra,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- A reminder sent, so it is sent once: per paper, per mark (30 or 7 days
-- before), per date — a renewed paper earns its reminders again.
CREATE TABLE IF NOT EXISTS reminded (
    paper_id INTEGER NOT NULL,
    mark     INTEGER NOT NULL,
    expires  TEXT    NOT NULL,
    PRIMARY KEY (paper_id, mark, expires)
);

-- Files already taken from the watched folder, so a paper deleted here is not
-- read in again.
CREATE TABLE IF NOT EXISTS seen (
    path TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS prefs (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub const KINDS: [(&str, &str); 9] = [
    ("receipt", "Receipt"),
    ("bill", "Bill"),
    ("warranty", "Warranty"),
    ("id", "ID"),
    ("contract", "Contract"),
    ("medical", "Medical"),
    ("tax", "Tax"),
    ("manual", "Manual"),
    ("other", "Other"),
];

pub fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map_or("Other", |(_, l)| l)
}

/// Kinds that go into the vault the moment they are read.
pub fn private(kind: &str) -> bool {
    matches!(kind, "id" | "medical" | "tax")
}

const IMAGES: &[&str] = &["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp"];

pub fn readable(ext: &str) -> bool {
    ext == "pdf" || ext == "txt" || IMAGES.contains(&ext)
}

pub fn is_image(ext: &str) -> bool {
    IMAGES.contains(&ext)
}

/// `<data>/papers/<sub>`, made on first use.
pub fn dir(sub: &str) -> Result<PathBuf> {
    let d = tulipix_core::paths::data_dir().ok_or_else(|| anyhow!("no data folder"))?.join("papers").join(sub);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

pub fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ── reading ─────────────────────────────────────────────────────────────────

pub struct Reading {
    pub text: String,
    /// 0-100: tesseract's mean word confidence; 100 for a PDF's own text.
    pub confidence: i64,
    pub pages: i64,
    /// A picture of page one: the image itself, or Books' render of a PDF.
    pub thumb: String,
}

/// Read a paper. Blocking: it shells out, and a scan takes seconds.
pub fn read(path: &Path, ext: &str) -> Result<Reading> {
    match ext {
        "txt" => Ok(Reading { text: std::fs::read_to_string(path)?, confidence: 100, pages: 1, thumb: String::new() }),
        "pdf" => read_pdf(path),
        _ => {
            let (text, confidence) = ocr(path)?;
            Ok(Reading { text, confidence, pages: 1, thumb: thumb(path, ext) })
        }
    }
}

/// A picture of page one: a photo is its own; a PDF's is rendered by Books.
/// Blocking for a PDF.
pub fn thumb(path: &Path, ext: &str) -> String {
    match ext {
        "pdf" => tulipix_books::render::page_thumb(path, "pdf", 0)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        e if is_image(e) => path.to_string_lossy().to_string(),
        _ => String::new(),
    }
}

fn read_pdf(path: &Path) -> Result<Reading> {
    let pages = tulipix_books::render::page_count(path, "pdf").unwrap_or(0) as i64;
    let thumb = thumb(path, "pdf");
    let out = Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output()
        .context("reading PDFs needs pdftotext — install poppler (e.g. `pacman -S poppler`)")?;
    let text: String = String::from_utf8_lossy(&out.stdout).lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    if text.chars().filter(|c| c.is_alphanumeric()).count() >= 40 {
        return Ok(Reading { text, confidence: 100, pages, thumb });
    }
    // A scan saved as a PDF has no text layer: read its first pages as pictures.
    let mut all = String::new();
    let mut confs = Vec::new();
    for i in 0..pages.clamp(1, 3) as usize {
        let img = tulipix_books::render::page_image(path, "pdf", i)?;
        let (t, c) = ocr(&img)?;
        all.push_str(&t);
        all.push_str("\n\n");
        confs.push(c);
    }
    let confidence = if confs.is_empty() { 0 } else { confs.iter().sum::<i64>() / confs.len() as i64 };
    Ok(Reading { text: all.trim_end().to_string(), confidence, pages, thumb })
}

pub fn ocr(path: &Path) -> Result<(String, i64)> {
    if !tulipix_finances::ocr::available() {
        bail!("reading scans and photos needs tesseract — install it (e.g. `pacman -S tesseract tesseract-data-eng`)");
    }
    let out = Command::new("tesseract").arg(path).arg("-").arg("tsv").output()?;
    if !out.status.success() {
        bail!("tesseract could not read that picture");
    }
    Ok(from_tsv(&String::from_utf8_lossy(&out.stdout)))
}

/// tesseract's TSV as text and a confidence: words joined into their lines, a
/// blank line between paragraphs, and the mean word confidence, 0-100.
pub fn from_tsv(tsv: &str) -> (String, i64) {
    let mut text = String::new();
    let mut last: Option<[&str; 4]> = None;
    let (mut sum, mut n) = (0.0, 0);
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let word = f[11].trim();
        if word.is_empty() {
            continue;
        }
        let at = [f[1], f[2], f[3], f[4]];
        match last {
            Some(l) if l == at => text.push(' '),
            Some(l) if l[..3] == at[..3] => text.push('\n'),
            Some(_) => text.push_str("\n\n"),
            None => {}
        }
        text.push_str(word);
        last = Some(at);
        if let Ok(c) = f[10].parse::<f64>()
            && c >= 0.0
        {
            sum += c;
            n += 1;
        }
    }
    (text, if n == 0 { 0 } else { (sum / n as f64).round() as i64 })
}

// ── what it is ──────────────────────────────────────────────────────────────

/// The text as " word word word ", lowercase, punctuation gone — so a phrase
/// is found only as whole words: "irs" is not in "first".
fn words(text: &str) -> String {
    let mut w = String::from(" ");
    for part in text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()) {
        w.push_str(part);
        w.push(' ');
    }
    w
}

fn has(w: &str, phrase: &str) -> bool {
    w.contains(&format!(" {phrase} "))
}

/// The words that give each kind away. First in the list wins a tie, so the
/// rare and certain kinds come before the common ones.
///
/// ponytail: a word table. A small classifier trained on what the user files
/// would do better once there is enough of it to learn from.
const SIGNS: &[(&str, &[&str])] = &[
    ("id", &["passport", "driving licence", "driving license", "identity card", "id card", "date of birth", "nationality", "national insurance", "residence permit", "place of birth"]),
    ("medical", &["patient", "nhs", "clinic", "hospital", "prescription", "diagnosis", "blood test", "test results", "vaccination", "dental", "dentist", "gp", "pharmacy"]),
    ("tax", &["hmrc", "tax return", "self assessment", "tax year", "p60", "p45", "income tax", "irs", "w 2", "1099", "tax code", "utr"]),
    ("contract", &["agreement", "tenancy", "contract", "landlord", "tenant", "hereby", "the parties", "terms and conditions", "minimum term"]),
    ("warranty", &["warranty", "guarantee", "applecare", "extended cover", "protection plan", "certificate of cover"]),
    ("manual", &["user manual", "instructions", "installation", "user guide", "troubleshooting", "safety instructions", "operating", "maintenance", "quick start"]),
    ("bill", &["bill", "invoice", "account number", "direct debit", "amount due", "payment due", "statement", "council tax", "premium", "renewal", "tariff", "meter"]),
    ("receipt", &["receipt", "total", "vat", "change", "cash", "card", "visa", "mastercard", "amex", "qty", "subtotal", "thank you"]),
];

pub fn classify(text: &str, pages: i64) -> &'static str {
    let w = words(text);
    let mut best = ("other", 0usize);
    for (kind, signs) in SIGNS {
        let mut score = signs.iter().filter(|s| has(&w, s)).count();
        if *kind == "manual" && pages >= 12 {
            score += 2;
        }
        if score > best.1 {
            best = (*kind, score);
        }
    }
    best.0
}

/// A title worth reading on a card: the shop for what was bought, the name of
/// the document for what was issued, and the file's own name as a last resort.
pub fn title(kind: &str, merchant: &str, text: &str, file_name: &str, day: Option<NaiveDate>) -> String {
    const NAMED: &[(&str, &str)] = &[
        ("passport", "Passport"),
        ("driving licence", "Driving licence"),
        ("driving license", "Driving licence"),
        ("tenancy", "Tenancy agreement"),
        ("council tax", "Council tax"),
        ("self assessment", "Self assessment"),
        ("tax return", "Tax return"),
        ("p60", "P60"),
        ("blood test", "Blood test results"),
        ("prescription", "Prescription"),
        ("applecare", "AppleCare"),
    ];
    let w = words(text);
    if !matches!(kind, "receipt" | "bill")
        && let Some((_, name)) = NAMED.iter().find(|(p, _)| has(&w, p))
    {
        return name.to_string();
    }
    if !merchant.is_empty() && matches!(kind, "receipt" | "bill" | "warranty" | "medical" | "other") {
        return merchant.to_string();
    }
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("").trim();
    let camera = stem.is_empty()
        || stem.chars().all(|c| !c.is_alphabetic() || c.is_ascii_uppercase())
        || ["img", "dsc", "pxl", "scan", "photo", "document", "doc"].iter().any(|p| stem.to_lowercase().starts_with(p));
    if camera {
        return match day {
            Some(d) => format!("{} · {}", kind_label(kind), long_day(d)),
            None => kind_label(kind).to_string(),
        };
    }
    stem.replace(['_', '-'], " ")
}

/// "CURRYS PC WORLD" → "Currys Pc World"; a name that already has small
/// letters is left as it was written.
pub fn proper(name: &str) -> String {
    if name.chars().any(|c| c.is_lowercase()) {
        return name.to_string();
    }
    name.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect()).unwrap_or_default()
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// A few lines of the text: around the first search word found, or how it
/// begins.
pub fn snippet(text: &str, query: &str) -> String {
    let chars: Vec<char> = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect();
    let low: Vec<char> = chars.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let find = |needle: &str| -> Option<(usize, usize)> {
        let n: Vec<char> = needle.to_lowercase().chars().collect();
        if n.is_empty() || n.len() > low.len() {
            return None;
        }
        (0..=low.len() - n.len()).find(|&i| low[i..i + n.len()] == n[..]).map(|i| (i, n.len()))
    };
    let hit = find(query.trim()).or_else(|| query.split_whitespace().find_map(&find));
    let (from, to) = match hit {
        Some((i, n)) => (i.saturating_sub(90), (i + n + 150).min(chars.len())),
        None => (0, chars.len().min(240)),
    };
    let mut s: String = chars[from..to].iter().collect();
    if from > 0 {
        s.insert(0, '…');
    }
    if to < chars.len() {
        s.push('…');
    }
    s
}

/// "42d" under a month; "14mo" beyond — what the ring beside a date says.
pub fn ring(days: i64) -> String {
    if days.abs() < 100 { format!("{days}d") } else { format!("{}mo", days / 30) }
}

/// "Passport expires in 42 days", "Car insurance renews on 12 Oct",
/// "Broadband contract ended 3 days ago".
pub fn when_line(title: &str, label: &str, days: i64, date: NaiveDate) -> String {
    let past = days < 0;
    let verb = match (label, past) {
        ("Renews", false) => "renews",
        ("Renews", true) => "renewed",
        ("Ends", false) => "ends",
        ("Ends", true) => "ended",
        ("Due", false) => "is due",
        ("Due", true) => "was due",
        ("Warranty ends", false) => "warranty ends",
        ("Warranty ends", true) => "warranty ended",
        ("Service due", false) => "service is due",
        ("Service due", true) => "service was due",
        ("Renew by", false) => "needs renewing",
        ("Renew by", true) => "needed renewing",
        (_, false) => "expires",
        (_, true) => "expired",
    };
    let when = match days {
        d if d < -1 => format!("{} days ago", -d),
        -1 => "yesterday".into(),
        0 => "today".into(),
        1 => "tomorrow".into(),
        d if d <= 60 => format!("in {d} days"),
        _ => format!("on {}", date.format("%-d %b")),
    };
    format!("{title} {verb} {when}")
}

/// The shelf a kind goes on when nothing like it has been filed yet.
pub fn default_collection(kind: &str) -> &'static str {
    match kind {
        "medical" => "Health",
        "tax" => "Money",
        "id" => "Personal",
        "receipt" | "bill" | "warranty" | "manual" | "contract" => "Home",
        _ => "Other",
    }
}

/// One line of advice beside a date, by kind.
pub fn advice(kind: &str) -> &'static str {
    match kind {
        "id" => "Renewal can take weeks. Start early.",
        "contract" => "Out of contract, the price usually goes up.",
        "bill" => "Compare before it rolls over.",
        "manual" => "A yearly service keeps the warranty valid.",
        "warranty" | "receipt" => "Anything wrong with it? Claim before then.",
        "medical" => "Book the next appointment.",
        _ => "",
    }
}

pub fn serial(text: &str) -> String {
    const KEYS: &[&str] = &["serial number", "serial no", "serial", "s/n", "sn:"];
    for line in text.lines() {
        let low = line.to_ascii_lowercase();
        let Some(at) = KEYS.iter().find_map(|k| low.find(k).map(|i| i + k.len())) else { continue };
        let found = line[at..]
            .split(|c: char| c.is_whitespace() || c == ':' || c == '#')
            .map(|t| t.trim_matches(|c: char| c == '.' || c == ','))
            .find(|t| {
                t.len() >= 5
                    && t.chars().any(|c| c.is_ascii_digit())
                    && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/')
            });
        if let Some(s) = found {
            return s.to_string();
        }
    }
    String::new()
}

// ── dates ───────────────────────────────────────────────────────────────────

const MONTHS: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december",
];

fn month(tok: &str) -> Option<u32> {
    let t = tok.to_lowercase();
    if t.len() < 3 || !t.chars().all(|c| c.is_alphabetic()) {
        return None;
    }
    MONTHS.iter().position(|m| m.starts_with(&t)).map(|i| i as u32 + 1)
}

fn year(tok: &str) -> Option<i32> {
    let n: i32 = tok.parse().ok()?;
    match tok.len() {
        4 if (1900..2200).contains(&n) => Some(n),
        2 => Some(2000 + n),
        _ => None,
    }
}

fn day_of(tok: &str) -> Option<u32> {
    let t = tok.trim_end_matches(|c: char| c.is_alphabetic());
    let n: u32 = t.parse().ok()?;
    (t.len() <= 2 && (1..=31).contains(&n)).then_some(n)
}

fn last_day(y: i32, m: u32) -> Option<NaiveDate> {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)?.pred_opt()
}

/// Every date written in a line, in order: 14/09/2026, 2026-09-14, 14.09.26,
/// 14 Sep 2026, 14th September 2026, Sep 14, 2026. Day first when a date could
/// be read either way, as on the papers this was written for. `loose` also
/// takes a month and a year alone — "09/28", "Nov 2027" — as the month's last
/// day; only for lines that say something expires, where that is what it means.
pub fn dates_in(line: &str, loose: bool) -> Vec<NaiveDate> {
    let toks: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        let parts: Vec<&str> = t.split(['/', '-', '.']).collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let d = if parts[0].len() == 4 {
                NaiveDate::from_ymd_opt(parts[0].parse().unwrap_or(0), parts[1].parse().unwrap_or(0), parts[2].parse().unwrap_or(0))
            } else {
                year(parts[2]).and_then(|y| NaiveDate::from_ymd_opt(y, parts[1].parse().unwrap_or(0), parts[0].parse().unwrap_or(0)))
            };
            if let Some(d) = d {
                out.push(d);
            }
            i += 1;
            continue;
        }
        if loose && parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let m: u32 = parts[0].parse().unwrap_or(0);
            if (1..=12).contains(&m)
                && let Some(d) = year(parts[1]).and_then(|y| last_day(y, m))
            {
                out.push(d);
            }
            i += 1;
            continue;
        }
        let at = |k: usize| toks.get(i + k).copied().unwrap_or("");
        // 14 Sep 2026
        if let (Some(d), Some(m), Some(y)) = (day_of(t), month(at(1)), year(at(2)))
            && let Some(date) = NaiveDate::from_ymd_opt(y, m, d)
        {
            out.push(date);
            i += 3;
            continue;
        }
        // Sep 14 2026
        if let (Some(m), Some(d), Some(y)) = (month(t), day_of(at(1)), year(at(2)))
            && let Some(date) = NaiveDate::from_ymd_opt(y, m, d)
        {
            out.push(date);
            i += 3;
            continue;
        }
        // Nov 2027
        if loose
            && let (Some(m), Some(y)) = (month(t), year(at(1)).filter(|_| at(1).len() == 4))
            && let Some(date) = last_day(y, m)
        {
            out.push(date);
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

/// The words that say a line carries the date something runs out, and how the
/// app says it. Longer phrases first: "date of expiry" before "expiry".
const ENDS: &[(&str, &str)] = &[
    ("date of expiry", "Expires"),
    ("expiry date", "Expires"),
    ("expiry", "Expires"),
    ("expires", "Expires"),
    ("expiration", "Expires"),
    ("exp", "Expires"),
    ("valid until", "Expires"),
    ("valid to", "Expires"),
    ("valid thru", "Expires"),
    ("renewal date", "Renews"),
    ("renews", "Renews"),
    ("renew by", "Renew by"),
    ("contract end", "Ends"),
    ("end date", "Ends"),
    ("ends", "Ends"),
    ("next service", "Service due"),
    ("service due", "Service due"),
    ("payment due", "Due"),
    ("due date", "Due"),
    ("due by", "Due"),
];

/// When it runs out, and how to say so. A line that names an end date wins;
/// failing that, a guarantee counted in years runs from the paper's own date.
pub fn expiry(text: &str, doc: Option<NaiveDate>) -> Option<(NaiveDate, &'static str)> {
    for line in text.lines() {
        let w = words(line);
        if let Some((_, label)) = ENDS.iter().find(|(k, _)| has(&w, k))
            && let Some(d) = dates_in(line, true).into_iter().max()
        {
            return Some((d, *label));
        }
    }
    let doc = doc?;
    let w = words(text);
    let toks: Vec<&str> = w.split_whitespace().collect();
    for i in 0..toks.len() {
        let Ok(n) = toks[i].parse::<i32>() else { continue };
        if !(1..=10).contains(&n) || !matches!(toks.get(i + 1), Some(&("year" | "years" | "yr" | "yrs"))) {
            continue;
        }
        if toks[i + 2..(i + 5).min(toks.len())].iter().any(|t| matches!(*t, "warranty" | "guarantee")) {
            let end = NaiveDate::from_ymd_opt(doc.year() + n, doc.month(), doc.day())
                .or_else(|| last_day(doc.year() + n, doc.month()))?;
            return Some((end, "Warranty ends"));
        }
    }
    None
}

pub fn long_day(d: NaiveDate) -> String {
    d.format("%-d %b %Y").to_string()
}

pub fn iso(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok()
}

// ── the vault ───────────────────────────────────────────────────────────────

/// Sealing for vault papers: ChaCha20-Poly1305, a fresh nonce in front of
/// every sealed file.
///
/// The key is the app's database key from the system keychain (`db_encrypt`),
/// hashed with a label so the same bytes are never used by two ciphers. That
/// key is made once and never replaced, so a vault paper stays readable for as
/// long as the keychain entry does.
pub mod vault {
    use anyhow::{Result, anyhow, bail};
    use chacha20poly1305::aead::{Aead, KeyInit};
    use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

    /// Asked of the keychain once per run: it is a round trip, and the key
    /// never changes.
    fn key() -> Result<[u8; 32]> {
        use sha2::{Digest, Sha256};
        static KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
        if let Some(k) = KEY.get() {
            return Ok(*k);
        }
        let root = tulipix_core::sec::db_encrypt::key_hex()?;
        let mut h = Sha256::new();
        h.update(b"tulipix papers vault\0");
        h.update(root.as_bytes());
        let mut k = [0u8; 32];
        k.copy_from_slice(&h.finalize());
        Ok(*KEY.get_or_init(|| k))
    }

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>> {
        seal_with(&key()?, plain)
    }

    pub fn open(sealed: &[u8]) -> Result<Vec<u8>> {
        open_with(&key()?, sealed)
    }

    pub fn seal_with(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|e| anyhow!("no random source: {e}"))?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
        let body = cipher.encrypt(Nonce::from_slice(&nonce), plain).map_err(|_| anyhow!("could not seal the paper"))?;
        let mut out = nonce.to_vec();
        out.extend(body);
        Ok(out)
    }

    pub fn open_with(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < 12 + 16 {
            bail!("that sealed paper is cut short");
        }
        let (nonce, body) = sealed.split_at(12);
        ChaCha20Poly1305::new(Key::from_slice(key))
            .decrypt(Nonce::from_slice(nonce), body)
            .map_err(|_| anyhow!("the vault key does not open this paper"))
    }
}

/// Books keeps rendered pages under a hash of the file's path. A paper going
/// into the vault takes its pages with it — its thumbnail and, for a scan, the
/// page pictures tesseract read.
pub fn forget_pages(file: &Path) {
    if let Some(root) = tulipix_books::cache_dir() {
        let d = root.join("pages").join(tulipix_books::short_hash(&file.display().to_string()));
        std::fs::remove_dir_all(d).ok();
    }
}

// ── the phone ───────────────────────────────────────────────────────────────

/// Scan with phone: a page on the local network where the phone's camera or
/// files send papers straight in.
///
/// The same shape as Kitchen's list page: one listener for the process, one
/// random 128-bit path at a time, nothing else answered. Uploads are plain
/// POSTs of the file's bytes, so there is no form encoding to parse.
pub mod phone {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};

    use anyhow::{Result, anyhow, bail};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// A phone photo is a few MB; a long scanned PDF can be tens.
    const MAX: usize = 80 * 1024 * 1024;

    static PORT: AtomicU16 = AtomicU16::new(0);
    static TOKEN: Mutex<String> = Mutex::new(String::new());

    fn token() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            b.copy_from_slice(&n.to_le_bytes());
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A fresh address for the page; the one before it stops answering.
    pub async fn publish() -> Result<String> {
        let ip = tulipix_transfer::net::interfaces()
            .first()
            .map(|(_, ip)| ip.to_string())
            .ok_or_else(|| anyhow!("this computer has no network address a phone could reach"))?;
        let port = serve().await?;
        let t = token();
        if let Ok(mut g) = TOKEN.lock() {
            *g = t.clone();
        }
        Ok(format!("http://{ip}:{port}/{t}"))
    }

    async fn serve() -> Result<u16> {
        let p = PORT.load(Ordering::SeqCst);
        if p != 0 {
            return Ok(p);
        }
        let listener = TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();
        PORT.store(port, Ordering::SeqCst);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Err(e) = answer(stream).await {
                        tracing::debug!(error = %e, "papers: phone request failed");
                    }
                });
            }
            PORT.store(0, Ordering::SeqCst);
        });
        Ok(port)
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// `%E2%82%AC` → "€". What `encodeURIComponent` sends.
    pub fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%'
                && i + 2 < b.len()
                && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    async fn answer(mut s: TcpStream) -> Result<()> {
        let mut buf = Vec::with_capacity(16 * 1024);
        let mut chunk = vec![0u8; 64 * 1024];
        let head_end = loop {
            let n = s.read(&mut chunk).await?;
            if n == 0 {
                bail!("closed before the request was whole");
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i + 4;
            }
            if buf.len() > 32 * 1024 {
                bail!("request head too long");
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let mut first = head.lines().next().unwrap_or("").split_whitespace();
        let method = first.next().unwrap_or("");
        let path = first.next().unwrap_or("");
        let header = |name: &str| {
            head.lines().skip(1).find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
            })
        };
        let want = TOKEN.lock().map(|g| g.clone()).unwrap_or_default();
        let ok = !want.is_empty();

        let (status, kind, body) = if ok && method == "GET" && path == format!("/{want}") {
            ("200 OK", "text/html; charset=utf-8", PAGE.to_string())
        } else if ok && method == "POST" && path == format!("/{want}/up") {
            let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            if len == 0 || len > MAX {
                ("413 Payload Too Large", "text/plain", "too big".to_string())
            } else {
                let mut body = buf[head_end..].to_vec();
                while body.len() < len {
                    let n = s.read(&mut chunk).await?;
                    if n == 0 {
                        bail!("upload cut short");
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                body.truncate(len);
                let name = unescape(&header("x-name").unwrap_or_default());
                match crate::api::papers::take_upload(&name, body).await {
                    Ok(()) => ("200 OK", "text/plain", "ok".to_string()),
                    Err(e) => ("400 Bad Request", "text/plain", e.to_string()),
                }
            }
        } else {
            ("404 Not Found", "text/html; charset=utf-8", "<p>Not here.</p>".to_string())
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(resp.as_bytes()).await?;
        Ok(())
    }

    const PAGE: &str = r#"<!doctype html><meta name=viewport content='width=device-width,initial-scale=1'>
<title>Send papers</title><style>
body{font:17px system-ui,sans-serif;margin:0 16px 40px;color:#1a1a1a;background:#f6fbfa}
h1{font-size:22px;margin:22px 0 6px}p{color:#555;margin:0 0 18px}
label{display:block;text-align:center;padding:22px;border-radius:14px;background:#14b8a6;color:#fff;font-weight:600}
input{display:none}li{padding:10px 0;border-bottom:1px solid #dfe9e7;list-style:none}ul{padding:0}
.bad{color:#dc2626}.ok{color:#0f766e}</style>
<h1>Send papers to Tulipix</h1><p>Take a photo or pick PDFs. They are read on the computer, not here.</p>
<label>Photo or file<input id=f type=file accept="image/*,application/pdf" multiple></label><ul id=l></ul>
<script>
f.onchange=async()=>{for(const x of f.files){const li=document.createElement('li');li.textContent=x.name+' — sending…';l.prepend(li);
try{const r=await fetch(location.pathname+'/up',{method:'POST',headers:{'X-Name':encodeURIComponent(x.name)},body:x});
li.textContent=x.name+(r.ok?' — sent':' — '+await r.text());li.className=r.ok?'ok':'bad'}catch(e){li.textContent=x.name+' — failed';li.className='bad'}}f.value=''};
</script>"#;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn tsv_becomes_lines_and_a_confidence() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   1\t1\t0\t0\t0\t0\t0\t0\t10\t10\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t0\t0\t1\t1\t96\tCURRYS\n\
                   5\t1\t1\t1\t1\t2\t0\t0\t1\t1\t90\tLTD\n\
                   5\t1\t1\t1\t2\t1\t0\t0\t1\t1\t100\tTotal\n\
                   5\t1\t2\t1\t1\t1\t0\t0\t1\t1\t94\t£649.00\n";
        let (text, conf) = from_tsv(tsv);
        assert_eq!(text, "CURRYS LTD\nTotal\n\n£649.00");
        assert_eq!(conf, 95);
        assert_eq!(from_tsv("header only\n"), (String::new(), 0));
    }

    #[test]
    fn papers_are_told_apart() {
        assert_eq!(classify("PASSPORT\nNationality BRITISH CITIZEN\nDate of birth 02 MAR 1990", 1), "id");
        assert_eq!(classify("CURRYS\nWashing machine 649.00\nTOTAL 649.00\nVAT 108.17\nCard AMEX", 1), "receipt");
        assert_eq!(classify("Octopus Energy\nYour bill\nAccount number 1234\nDirect Debit", 1), "bill");
        assert_eq!(classify("ASSURED SHORTHOLD TENANCY AGREEMENT between the Landlord and the Tenant", 3), "contract");
        assert_eq!(classify("Greenstar 4000 installation and servicing instructions", 64), "manual");
        assert_eq!(classify("the first thing", 1), "other");
    }

    #[test]
    fn dates_are_found_however_they_are_written() {
        assert_eq!(dates_in("Date: 14/09/2026 12:04", false), vec![d(2026, 9, 14)]);
        assert_eq!(dates_in("2026-09-14", false), vec![d(2026, 9, 14)]);
        assert_eq!(dates_in("Issued 3rd July 2026, due 17 Jul 26", false), vec![d(2026, 7, 3), d(2026, 7, 17)]);
        assert_eq!(dates_in("Sep 14, 2026", false), vec![d(2026, 9, 14)]);
        assert_eq!(dates_in("EXP 09/28", true), vec![d(2028, 9, 30)]);
        assert_eq!(dates_in("EXP 09/28", false), vec![]);
        assert_eq!(dates_in("valid until Nov 2027", true), vec![d(2027, 11, 30)]);
        assert_eq!(dates_in("31/02/2026", false), vec![]);
    }

    #[test]
    fn expiry_comes_from_the_line_or_the_guarantee() {
        let passport = "PASSPORT\nDate of issue 31 OCT 2016\nDate of expiry 31 OCT 2026";
        assert_eq!(expiry(passport, None), Some((d(2026, 10, 31), "Expires")));
        let receipt = "CURRYS\n14/09/2026\nWashing machine\n2 year manufacturer guarantee\nTOTAL 649.00";
        assert_eq!(expiry(receipt, Some(d(2026, 9, 14))), Some((d(2028, 9, 14), "Warranty ends")));
        assert_eq!(expiry("Renewal date: 12 Oct 2026", None), Some((d(2026, 10, 12), "Renews")));
        assert_eq!(expiry("TOTAL 12.00", Some(d(2026, 1, 1))), None);
    }

    #[test]
    fn serials_and_titles() {
        assert_eq!(serial("Model WW90T684DLH\nSerial No: WW90T684DLH-24\n"), "WW90T684DLH-24");
        assert_eq!(serial("Serial: n/a"), "");
        assert_eq!(title("id", "", "PASSPORT", "IMG_2031.jpg", None), "Passport");
        assert_eq!(title("receipt", "CURRYS", "", "IMG_2031.jpg", None), "CURRYS");
        assert_eq!(title("receipt", "", "", "IMG_2031.jpg", Some(d(2026, 9, 14))), "Receipt · 14 Sep 2026");
        assert_eq!(title("manual", "", "", "boiler_manual.pdf", None), "boiler manual");
    }

    #[test]
    fn names_snippets_and_reminders_read_well() {
        assert_eq!(proper("CURRYS PC WORLD"), "Currys Pc World");
        assert_eq!(proper("Octopus Energy"), "Octopus Energy");
        let text = "Samsung   Series 6\nWW90T684DLH washing machine 9kg\n2 year manufacturer guarantee";
        assert_eq!(snippet(text, "washing"), "Samsung Series 6 WW90T684DLH washing machine 9kg 2 year manufacturer guarantee");
        assert_eq!(snippet(&"a ".repeat(200), "zzz").chars().count(), 241);
        assert_eq!(ring(42), "42d");
        assert_eq!(ring(420), "14mo");
        assert_eq!(when_line("Passport", "Expires", 42, d(2026, 10, 31)), "Passport expires in 42 days");
        assert_eq!(when_line("Car insurance", "Renews", 75, d(2026, 10, 12)), "Car insurance renews on 12 Oct");
        assert_eq!(when_line("Broadband", "Ends", -3, d(2026, 9, 1)), "Broadband ended 3 days ago");
    }

    #[test]
    fn sealed_papers_open_only_with_their_key() {
        let k = [7u8; 32];
        let sealed = vault::seal_with(&k, b"passport scan").unwrap();
        assert_ne!(&sealed[12..], b"passport scan");
        assert_eq!(vault::open_with(&k, &sealed).unwrap(), b"passport scan");
        assert!(vault::open_with(&[8u8; 32], &sealed).is_err());
    }

    #[test]
    fn uploaded_names_are_unescaped() {
        assert_eq!(phone::unescape("scan%20%E2%82%AC1.pdf"), "scan €1.pdf");
        assert_eq!(phone::unescape("100%"), "100%");
    }
}
