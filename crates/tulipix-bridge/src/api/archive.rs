// The Archive section: a catalogue of the folders and drives you add — search
// that reaches inside zips, duplicates by checksum, and files that changed on
// their own.
//
// Three tabs — Library (with a zip open to look inside), Duplicates and
// Health. A drive that is not plugged in stays in the catalogue and is still
// searched; only a scan of a present folder forgets what went. Scans, hashing
// and checks run one at a time in the background.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{Local, TimeZone};
use sqlx::SqlitePool;

use crate::archive::{self as a, KINDS};
use crate::db::archive_pool;

// ------------------------------------------------------------------- state ---

pub struct ArchiveState {
    /// library | dupes | health.
    pub tab: String,
    pub notice: String,
    pub roots: Vec<ArchiveRoot>,
    pub kinds: Vec<ArchiveKind>,
    /// "" is every kind.
    pub kind: String,
    pub query: String,
    /// Search results, or the newest files when there is no search.
    pub files: Vec<ArchiveFile>,
    pub total: i64,
    pub bytes: String,
    /// A zip opened to look inside.
    pub inside: Option<ArchiveInside>,
    pub dupes: Vec<ArchiveDupes>,
    /// "23.4 GB could be freed".
    pub dupe_bytes: String,
    /// Files the same size as another, not yet checksummed.
    pub dupe_unhashed: i64,
    pub problems: Vec<ArchiveProblem>,
    pub job: Option<ArchiveJob>,
}

pub struct ArchiveRoot {
    pub id: i64,
    pub label: String,
    pub path: String,
    pub online: bool,
    pub files: i64,
    pub bytes: String,
    /// "Scanned 2 days ago", "Never scanned".
    pub scanned: String,
    /// Files with a checksum.
    pub hashed: i64,
    /// "Checked yesterday", "Never checked".
    pub checked: String,
}

pub struct ArchiveKind {
    pub id: String,
    pub label: String,
    pub n: i64,
}

pub struct ArchiveFile {
    pub id: i64,
    pub name: String,
    /// The folder, from the root: "finance/2019".
    pub dir: String,
    pub root: String,
    pub online: bool,
    pub kind: String,
    pub size: String,
    /// "Mar 2019".
    pub date: String,
    /// The member a search found inside this zip, or "".
    pub inner: String,
    pub state: String,
    pub is_zip: bool,
}

pub struct ArchiveInside {
    pub file_id: i64,
    pub name: String,
    pub path: String,
    pub entries: Vec<ArchiveEntry>,
    pub count: i64,
    pub size: String,
    pub online: bool,
}

pub struct ArchiveEntry {
    pub name: String,
    pub size: String,
    pub is_dir: bool,
}

pub struct ArchiveDupes {
    pub name: String,
    pub size: String,
    /// All copies but one.
    pub wasted: String,
    pub files: Vec<ArchiveCopy>,
}

pub struct ArchiveCopy {
    pub id: i64,
    pub path: String,
    pub root: String,
    pub online: bool,
    pub date: String,
    pub newest: bool,
}

pub struct ArchiveProblem {
    pub id: i64,
    pub name: String,
    pub path: String,
    /// changed | missing | broken.
    pub state: String,
    pub why: String,
}

pub struct ArchiveJob {
    pub label: String,
    pub done: i64,
    pub total: i64,
}

// ---------------------------------------------------------------- commands ---

pub enum ArchiveCmd {
    Refresh,
    SetTab { tab: String },
    SetKind { kind: String },
    Search { text: String },
    AddRoot { path: String },
    RemoveRoot { id: i64 },
    /// 0 scans every folder that is there.
    Scan { id: i64 },
    /// Checksum every file in a folder that has none, so later checks can
    /// tell a file that changed on its own.
    Hash { id: i64 },
    /// Recompute checksums and test zips; report what changed.
    Check { id: i64 },
    /// Checksum files that share a size with another, which finds duplicates.
    FindDupes,
    /// Into the system trash, and out of the catalogue.
    Trash { id: i64 },
    OpenFile { id: i64 },
    Reveal { id: i64 },
    LookInside { id: i64 },
    CloseInside,
    /// One member of the open zip, taken out and opened.
    OpenEntry { name: String },
    /// A problem understood: forget it.
    Dismiss { id: i64 },
}

// ----------------------------------------------------------------- session ---

struct Session {
    tab: String,
    kind: String,
    query: String,
    inside: i64,
    notice: String,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session { tab: "library".into(), kind: String::new(), query: String::new(), inside: 0, notice: String::new() })
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

// ---------------------------------------------------------------- exported ---

pub async fn archive_dispatch(cmd: ArchiveCmd) -> Result<ArchiveState> {
    let pool = archive_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// Everything in a catalogued zip into `dir`; answers how many files came out.
pub async fn archive_extract_all(id: i64, dir: String) -> Result<i64> {
    let pool = archive_pool().await?;
    let path: String = sqlx::query_scalar("SELECT path FROM files WHERE id = ?").bind(id).fetch_one(pool).await?;
    let (n, _) = tokio::task::spawn_blocking(move || tulipix_tools::archive::extract(&path, &dir, |_, _| true)).await??;
    Ok(n as i64)
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: ArchiveCmd) -> Result<()> {
    match cmd {
        ArchiveCmd::Refresh => {}
        ArchiveCmd::SetTab { tab } => {
            let mut s = lock();
            s.tab = tab;
            s.inside = 0;
        }
        ArchiveCmd::SetKind { kind } => lock().kind = kind,
        ArchiveCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            s.tab = "library".into();
            s.inside = 0;
        }
        ArchiveCmd::AddRoot { path } => {
            let p = Path::new(path.trim());
            if !p.is_dir() {
                bail!("{} is not a folder", path.trim());
            }
            let label = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.trim().to_string());
            sqlx::query("INSERT OR IGNORE INTO roots (path, label, added) VALUES (?, ?, ?)")
                .bind(path.trim())
                .bind(label)
                .bind(a::now())
                .execute(pool)
                .await?;
            let id: i64 = sqlx::query_scalar("SELECT id FROM roots WHERE path = ?").bind(path.trim()).fetch_one(pool).await?;
            queue(pool, Work::Scan(id));
        }
        ArchiveCmd::RemoveRoot { id } => {
            forget_files(pool, "root_id = ?", id).await?;
            sqlx::query("DELETE FROM roots WHERE id = ?").bind(id).execute(pool).await?;
        }
        ArchiveCmd::Scan { id } => queue(pool, Work::Scan(id)),
        ArchiveCmd::Hash { id } => queue(pool, Work::Hash(id)),
        ArchiveCmd::Check { id } => queue(pool, Work::Check(id)),
        ArchiveCmd::FindDupes => queue(pool, Work::Dupes),
        ArchiveCmd::Trash { id } => {
            let path = file_path(pool, id).await?;
            tulipix_platform::fm::move_to_trash(Path::new(&path))?;
            forget_files(pool, "id = ?", id).await?;
            say(format!("{} is in the trash", Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path)));
        }
        ArchiveCmd::OpenFile { id } => crate::api::transfer::open_url(&present(pool, id).await?),
        ArchiveCmd::Reveal { id } => {
            let p = present(pool, id).await?;
            let dir = Path::new(&p).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or(p);
            crate::api::transfer::open_url(&dir);
        }
        ArchiveCmd::LookInside { id } => lock().inside = id,
        ArchiveCmd::CloseInside => lock().inside = 0,
        ArchiveCmd::OpenEntry { name } => {
            let id = lock().inside;
            let path = present(pool, id).await?;
            let out = tulipix_core::paths::cache_dir().ok_or_else(|| anyhow!("no cache folder"))?.join("archive-open");
            let file = tokio::task::spawn_blocking(move || take_out(&path, &name, &out)).await??;
            crate::api::transfer::open_url(&file.to_string_lossy());
        }
        ArchiveCmd::Dismiss { id } => {
            sqlx::query("UPDATE files SET state = '' WHERE id = ?").bind(id).execute(pool).await?;
            sqlx::query("DELETE FROM files WHERE id = ? AND state = '' AND NOT EXISTS (SELECT 1 FROM roots r WHERE r.id = files.root_id)")
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

async fn file_path(pool: &SqlitePool, id: i64) -> Result<String> {
    Ok(sqlx::query_scalar("SELECT path FROM files WHERE id = ?").bind(id).fetch_one(pool).await?)
}

/// A catalogued file's path, when it is there to open.
async fn present(pool: &SqlitePool, id: i64) -> Result<String> {
    let p = file_path(pool, id).await?;
    if !Path::new(&p).exists() {
        bail!("that file is on a drive that is not plugged in, or it has gone");
    }
    Ok(p)
}

/// One member of a zip, written to `out` and returned. Only a member whose
/// name stays inside `out` (zip's `enclosed_name`).
fn take_out(zip_path: &str, name: &str, out: &Path) -> Result<PathBuf> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut m = z.by_name(name).with_context(|| format!("{name} is not in it"))?;
    let rel = m.enclosed_name().ok_or_else(|| anyhow!("{name} would land outside the folder"))?;
    let file = out.join(rel);
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut bytes = Vec::with_capacity(m.size() as usize);
    m.read_to_end(&mut bytes)?;
    std::fs::write(&file, bytes)?;
    Ok(file)
}

async fn forget_files(pool: &SqlitePool, filter: &str, key: i64) -> Result<()> {
    let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT id FROM files WHERE {filter}"))).bind(key).fetch_all(pool).await?;
    let mut tx = pool.begin().await?;
    for id in ids {
        sqlx::query("DELETE FROM entries WHERE file_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM names_fts WHERE file_id = ?").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM files WHERE id = ?").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

// ------------------------------------------------------------------ worker ---

#[derive(Clone, Copy, PartialEq)]
enum Work {
    Scan(i64),
    Hash(i64),
    Check(i64),
    Dupes,
}

fn jobs() -> &'static Mutex<(Vec<Work>, Option<(String, i64, i64)>)> {
    static J: OnceLock<Mutex<(Vec<Work>, Option<(String, i64, i64)>)>> = OnceLock::new();
    J.get_or_init(|| Mutex::new((Vec::new(), None)))
}

fn jobs_lock() -> MutexGuard<'static, (Vec<Work>, Option<(String, i64, i64)>)> {
    jobs().lock().unwrap_or_else(|e| e.into_inner())
}

fn progress(label: &str, done: i64, total: i64) {
    jobs_lock().1 = Some((label.to_string(), done, total));
}

static RUNNING: AtomicBool = AtomicBool::new(false);

fn queue(pool: &'static SqlitePool, w: Work) {
    {
        let mut j = jobs_lock();
        if !j.0.contains(&w) {
            j.0.push(w);
        }
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let next = {
                let mut j = jobs_lock();
                if j.0.is_empty() { None } else { Some(j.0.remove(0)) }
            };
            let Some(w) = next else { break };
            let r = match w {
                Work::Scan(id) => scan(pool, id).await,
                Work::Hash(id) => hash(pool, id).await,
                Work::Check(id) => check(pool, id).await,
                Work::Dupes => dupes(pool).await,
            };
            if let Err(e) = r {
                say(format!("Stopped: {e}"));
            }
            jobs_lock().1 = None;
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

async fn roots_for(pool: &SqlitePool, id: i64) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as("SELECT id, path FROM roots WHERE (?1 = 0 OR id = ?1) ORDER BY id").bind(id).fetch_all(pool).await?)
}

/// Walk a folder: new and changed files in, their zips listed; files gone
/// from a folder that is there, out. A folder that is not there is skipped
/// and keeps its catalogue.
async fn scan(pool: &SqlitePool, id: i64) -> Result<()> {
    for (root_id, root) in roots_for(pool, id).await? {
        let dir = PathBuf::from(&root);
        if !dir.is_dir() {
            continue;
        }
        progress(&format!("Reading {root}"), 0, 0);
        let found = tokio::task::spawn_blocking(move || a::walk(&dir)).await?;
        let known: HashMap<String, (i64, i64, i64)> = sqlx::query_as::<_, (String, i64, i64, i64)>(
            "SELECT path, id, size, mtime FROM files WHERE root_id = ?",
        )
        .bind(root_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(p, id, size, mtime)| (p, (id, size, mtime)))
        .collect();
        let total = found.len() as i64;
        let mut seen = std::collections::HashSet::new();
        let mut tx = pool.begin().await?;
        for (i, (path, size, mtime)) in found.iter().enumerate() {
            let p = path.to_string_lossy().to_string();
            seen.insert(p.clone());
            if i % 500 == 0 {
                progress(&format!("Reading {root}"), i as i64, total);
            }
            match known.get(&p) {
                Some(&(_, s, m)) if s == *size && m == *mtime => continue,
                Some(&(fid, _, _)) => {
                    // Changed on purpose (a new date): its old checksum no
                    // longer says anything.
                    sqlx::query("UPDATE files SET size = ?, mtime = ?, sha = '', sha_at = 0, state = '' WHERE id = ?")
                        .bind(*size)
                        .bind(*mtime)
                        .bind(fid)
                        .execute(&mut *tx)
                        .await?;
                    list_zip(&mut tx, fid, path).await?;
                }
                None => {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let fid = sqlx::query("INSERT INTO files (root_id, path, name, kind, size, mtime) VALUES (?, ?, ?, ?, ?, ?)")
                        .bind(root_id)
                        .bind(&p)
                        .bind(&name)
                        .bind(a::kind_of(&name))
                        .bind(*size)
                        .bind(*mtime)
                        .execute(&mut *tx)
                        .await?
                        .last_insert_rowid();
                    sqlx::query("INSERT INTO names_fts (name, file_id, inner) VALUES (?, ?, '')")
                        .bind(&name)
                        .bind(fid)
                        .execute(&mut *tx)
                        .await?;
                    list_zip(&mut tx, fid, path).await?;
                }
            }
        }
        tx.commit().await?;
        let gone: Vec<i64> = known.iter().filter(|(p, _)| !seen.contains(*p)).map(|(_, v)| v.0).collect();
        for fid in gone {
            // Gone with a checksum is worth saying; gone without, just gone.
            let had: String = sqlx::query_scalar("SELECT sha FROM files WHERE id = ?").bind(fid).fetch_one(pool).await.unwrap_or_default();
            if had.is_empty() {
                forget_files(pool, "id = ?", fid).await?;
            } else {
                sqlx::query("UPDATE files SET state = 'missing' WHERE id = ?").bind(fid).execute(pool).await?;
            }
        }
        sqlx::query("UPDATE roots SET scanned = ? WHERE id = ?").bind(a::now()).bind(root_id).execute(pool).await?;
    }
    Ok(())
}

/// A zip's members into the catalogue, replacing what was there.
async fn list_zip(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, fid: i64, path: &Path) -> Result<()> {
    let low = path.to_string_lossy().to_lowercase();
    if !(low.ends_with(".zip") || low.ends_with(".cbz")) {
        return Ok(());
    }
    sqlx::query("DELETE FROM entries WHERE file_id = ?").bind(fid).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM names_fts WHERE file_id = ? AND inner != ''").bind(fid).execute(&mut **tx).await?;
    let p = path.to_string_lossy().to_string();
    let Ok(list) = tokio::task::spawn_blocking(move || tulipix_tools::archive::list(&p)).await? else { return Ok(()) };
    for e in list.into_iter().filter(|e| !e.is_dir).take(5000) {
        sqlx::query("INSERT INTO entries (file_id, name, size) VALUES (?, ?, ?)")
            .bind(fid)
            .bind(&e.name)
            .bind(e.bytes as i64)
            .execute(&mut **tx)
            .await?;
        let leaf = e.name.rsplit('/').next().unwrap_or(&e.name).to_string();
        sqlx::query("INSERT INTO names_fts (name, file_id, inner) VALUES (?, ?, ?)")
            .bind(leaf)
            .bind(fid)
            .bind(&e.name)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn take_sha(path: &str) -> Option<String> {
    let p = PathBuf::from(path);
    tokio::task::spawn_blocking(move || a::sha256(&p).ok()).await.ok().flatten()
}

/// Checksum what has none, in a folder that is there.
async fn hash(pool: &SqlitePool, id: i64) -> Result<()> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.path, f.mtime FROM files f WHERE f.root_id = ? AND f.sha = '' ORDER BY f.size",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let total = rows.len() as i64;
    for (i, (fid, path, mtime)) in rows.into_iter().enumerate() {
        progress("Taking checksums", i as i64, total);
        if let Some(sha) = take_sha(&path).await {
            sqlx::query("UPDATE files SET sha = ?, sha_at = ? WHERE id = ?").bind(sha).bind(mtime).bind(fid).execute(pool).await?;
        }
    }
    say(format!("Checksums taken for {} files — Check finds any that change on their own", total));
    Ok(())
}

/// Every checksummed file read again. Same date and size but different bytes
/// is a file that changed on its own; a zip that does not read through is
/// broken; a file that is not there, while its folder is, is missing.
async fn check(pool: &SqlitePool, id: i64) -> Result<()> {
    for (root_id, root) in roots_for(pool, id).await? {
        if !Path::new(&root).is_dir() {
            continue;
        }
        let rows: Vec<(i64, String, String, i64, i64)> =
            sqlx::query_as("SELECT id, path, sha, sha_at, size FROM files WHERE root_id = ? AND (sha != '' OR kind = 'archive')")
                .bind(root_id)
                .fetch_all(pool)
                .await?;
        let total = rows.len() as i64;
        let mut bad = 0;
        for (i, (fid, path, sha, sha_at, size)) in rows.into_iter().enumerate() {
            progress(&format!("Checking {root}"), i as i64, total);
            let p = PathBuf::from(&path);
            let Ok(meta) = std::fs::metadata(&p) else {
                sqlx::query("UPDATE files SET state = 'missing' WHERE id = ?").bind(fid).execute(pool).await?;
                bad += 1;
                continue;
            };
            let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            let mut state = "";
            if !sha.is_empty() && mtime == sha_at && meta.len() as i64 == size {
                if take_sha(&path).await.is_some_and(|now| now != sha) {
                    state = "changed";
                }
            }
            let low = path.to_lowercase();
            if state.is_empty() && (low.ends_with(".zip") || low.ends_with(".cbz")) {
                let q = p.clone();
                if tokio::task::spawn_blocking(move || a::test_zip(&q)).await?.is_some() {
                    state = "broken";
                }
            }
            if !state.is_empty() {
                bad += 1;
            }
            sqlx::query("UPDATE files SET state = ? WHERE id = ? AND state != 'missing'").bind(state).bind(fid).execute(pool).await?;
        }
        sqlx::query("UPDATE roots SET checked = ? WHERE id = ?").bind(a::now()).bind(root_id).execute(pool).await?;
        say(if bad == 0 { format!("{root}: all {total} files as they were") } else { format!("{root}: {bad} problems — see Health") });
    }
    Ok(())
}

/// Checksum every file of 1 MB or more that shares its size with another.
async fn dupes(pool: &SqlitePool) -> Result<()> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.path, f.mtime FROM files f JOIN roots r ON r.id = f.root_id \
         WHERE f.sha = '' AND f.size >= 1048576 \
           AND f.size IN (SELECT size FROM files GROUP BY size HAVING COUNT(*) > 1) ORDER BY f.size DESC",
    )
    .fetch_all(pool)
    .await?;
    let total = rows.len() as i64;
    for (i, (fid, path, mtime)) in rows.into_iter().enumerate() {
        progress("Comparing files of the same size", i as i64, total);
        if !Path::new(&path).exists() {
            continue;
        }
        if let Some(sha) = take_sha(&path).await {
            sqlx::query("UPDATE files SET sha = ?, sha_at = ? WHERE id = ?").bind(sha).bind(mtime).bind(fid).execute(pool).await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

fn local(ts: i64) -> chrono::DateTime<Local> {
    Local.timestamp_opt(ts, 0).single().unwrap_or_else(Local::now)
}

fn ago(ts: i64, what: &str) -> String {
    if ts == 0 {
        return format!("Never {what}");
    }
    let days = (Local::now().date_naive() - local(ts).date_naive()).num_days();
    match days {
        0 => format!("{} today", cap(what)),
        1 => format!("{} yesterday", cap(what)),
        n if n < 30 => format!("{} {n} days ago", cap(what)),
        _ => format!("{} {}", cap(what), local(ts).format("%-d %b %Y")),
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

type FileQ = (i64, String, String, String, i64, i64, String, String, String, String);

fn file_row(r: FileQ, inner: String) -> ArchiveFile {
    let (id, name, path, kind, size, mtime, state, root, root_path, _) = r;
    let dir = Path::new(&path)
        .parent()
        .and_then(|d| d.strip_prefix(&root_path).ok())
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    let low = name.to_lowercase();
    ArchiveFile {
        id,
        online: Path::new(&root_path).is_dir(),
        is_zip: low.ends_with(".zip") || low.ends_with(".cbz"),
        name,
        dir,
        root,
        kind,
        size: a::size_label(size),
        date: local(mtime).format("%b %Y").to_string(),
        inner,
        state,
    }
}

const FILE_COLS: &str = "SELECT f.id, f.name, f.path, f.kind, f.size, f.mtime, f.state, r.label, r.path, '' \
                         FROM files f JOIN roots r ON r.id = f.root_id";

async fn snapshot(pool: &SqlitePool) -> Result<ArchiveState> {
    let (tab, kind, query, inside, notice) = {
        let mut s = lock();
        (s.tab.clone(), s.kind.clone(), s.query.clone(), s.inside, std::mem::take(&mut s.notice))
    };

    let roots: Vec<ArchiveRoot> = sqlx::query_as::<_, (i64, String, String, i64, i64, i64, i64, i64)>(
        "SELECT r.id, r.label, r.path, r.scanned, r.checked, \
                (SELECT COUNT(*) FROM files f WHERE f.root_id = r.id), \
                (SELECT COALESCE(SUM(size), 0) FROM files f WHERE f.root_id = r.id), \
                (SELECT COUNT(*) FROM files f WHERE f.root_id = r.id AND f.sha != '') \
         FROM roots r ORDER BY r.id",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, label, path, scanned, checked, files, bytes, hashed)| ArchiveRoot {
        id,
        online: Path::new(&path).is_dir(),
        label,
        path,
        files,
        bytes: a::size_label(bytes),
        scanned: ago(scanned, "scanned"),
        hashed,
        checked: ago(checked, "checked"),
    })
    .collect();

    let counts: HashMap<String, i64> =
        sqlx::query_as::<_, (String, i64)>("SELECT kind, COUNT(*) FROM files GROUP BY kind").fetch_all(pool).await?.into_iter().collect();
    let kinds: Vec<ArchiveKind> = KINDS
        .iter()
        .map(|(id, label)| ArchiveKind { id: id.to_string(), label: label.to_string(), n: counts.get(*id).copied().unwrap_or(0) })
        .filter(|k| k.n > 0)
        .collect();
    let (total, bytes): (i64, i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size), 0) FROM files").fetch_one(pool).await?;

    let files: Vec<ArchiveFile> = if query.is_empty() {
        sqlx::query_as::<_, FileQ>(sqlx::AssertSqlSafe(format!("{FILE_COLS} WHERE (?1 = '' OR f.kind = ?1) ORDER BY f.mtime DESC LIMIT 200")))
            .bind(&kind)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| file_row(r, String::new()))
            .collect()
    } else {
        let fq = crate::api::papers::fts_query(&query);
        let hits: Vec<(i64, String)> = if fq.is_empty() {
            Vec::new()
        } else {
            sqlx::query_as("SELECT file_id, inner FROM names_fts WHERE names_fts MATCH ? ORDER BY rank LIMIT 300")
                .bind(&fq)
                .fetch_all(pool)
                .await?
        };
        let mut out = Vec::new();
        let mut shown = std::collections::HashSet::new();
        for (fid, inner) in hits {
            if !shown.insert((fid, inner.clone())) {
                continue;
            }
            let row: Option<FileQ> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{FILE_COLS} WHERE f.id = ? AND (?2 = '' OR f.kind = ?2)")))
                .bind(fid)
                .bind(&kind)
                .fetch_optional(pool)
                .await?;
            if let Some(r) = row {
                out.push(file_row(r, inner));
            }
        }
        out
    };

    let inside = if inside > 0 {
        let row: Option<(String, String, i64)> =
            sqlx::query_as("SELECT name, path, size FROM files WHERE id = ?").bind(inside).fetch_optional(pool).await?;
        match row {
            None => None,
            Some((name, path, size)) => {
                let online = Path::new(&path).exists();
                // Live when the drive is here; the catalogue's copy when not.
                let entries: Vec<ArchiveEntry> = if online {
                    let p = path.clone();
                    tokio::task::spawn_blocking(move || tulipix_tools::archive::list(&p))
                        .await?
                        .unwrap_or_default()
                        .into_iter()
                        .take(3000)
                        .map(|e| ArchiveEntry { name: e.name, size: a::size_label(e.bytes as i64), is_dir: e.is_dir })
                        .collect()
                } else {
                    sqlx::query_as::<_, (String, i64)>("SELECT name, size FROM entries WHERE file_id = ? ORDER BY name LIMIT 3000")
                        .bind(inside)
                        .fetch_all(pool)
                        .await?
                        .into_iter()
                        .map(|(name, size)| ArchiveEntry { name, size: a::size_label(size), is_dir: false })
                        .collect()
                };
                Some(ArchiveInside {
                    file_id: inside,
                    count: entries.iter().filter(|e| !e.is_dir).count() as i64,
                    entries,
                    name,
                    path,
                    size: a::size_label(size),
                    online,
                })
            }
        }
    } else {
        None
    };

    // Duplicates: files sharing a checksum.
    let rows: Vec<(String, i64, String, i64, String, i64, String, String)> = sqlx::query_as(
        "SELECT f.sha, f.id, f.path, f.size, f.name, f.mtime, r.label, r.path FROM files f JOIN roots r ON r.id = f.root_id \
         WHERE f.sha != '' AND f.sha IN (SELECT sha FROM files WHERE sha != '' GROUP BY sha HAVING COUNT(*) > 1) \
         ORDER BY f.size DESC, f.sha, f.mtime DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut groups: Vec<ArchiveDupes> = Vec::new();
    let mut wasted_total = 0i64;
    let mut last_sha = String::new();
    for (sha, id, path, size, name, mtime, root, root_path) in rows {
        if sha != last_sha {
            last_sha = sha.clone();
            groups.push(ArchiveDupes { name: name.clone(), size: a::size_label(size), wasted: String::new(), files: Vec::new() });
        }
        let g = groups.last_mut().expect("pushed above");
        let newest = g.files.is_empty();
        if !newest {
            wasted_total += size;
        }
        g.files.push(ArchiveCopy {
            id,
            path,
            root,
            online: Path::new(&root_path).is_dir(),
            date: local(mtime).format("%-d %b %Y").to_string(),
            newest,
        });
        g.wasted = a::size_label(size * (g.files.len() as i64 - 1));
    }
    let dupe_unhashed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM files WHERE sha = '' AND size >= 1048576 \
           AND size IN (SELECT size FROM files GROUP BY size HAVING COUNT(*) > 1)",
    )
    .fetch_one(pool)
    .await?;

    let problems: Vec<ArchiveProblem> = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT id, name, path, state FROM files WHERE state != '' ORDER BY state, name",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, name, path, state)| ArchiveProblem {
        why: match state.as_str() {
            "changed" => "Its bytes changed but its date and size did not — often a failing disk. Restore it from another copy.",
            "broken" => "A member of this zip does not read back.",
            _ => "It had a checksum and is not there any more.",
        }
        .to_string(),
        id,
        name,
        path,
        state,
    })
    .collect();

    let job = jobs_lock().1.clone().map(|(label, done, total)| ArchiveJob { label, done, total });
    Ok(ArchiveState {
        tab,
        notice,
        roots,
        kinds,
        kind,
        query,
        files,
        total,
        bytes: a::size_label(bytes),
        inside,
        dupes: groups,
        dupe_bytes: a::size_label(wasted_total),
        dupe_unhashed,
        problems,
        job,
    })
}
