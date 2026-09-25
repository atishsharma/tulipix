// The Feeds section: sites, blogs and newsletters in one reader.
//
// Four tabs — Today's brief, Unread (sources · list · reader), Read later and
// Highlights — over one snapshot. The parsing, extraction, summaries and story
// grouping are `crate::feeds`; this file keeps the session (which tab, which
// source, which article is open) and maps rows into what Dart draws.
//
// The page asks on open and every half hour while it is built, which is what
// the Slint podcast refresh does for its own feeds; `background_tick` keeps
// the same half hour from the shell while the page is closed.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;

use crate::db::feeds_pool;
use crate::feeds;

// ------------------------------------------------------------------- state ---

pub struct FeedsState {
    /// today | unread | saved | highlights.
    pub tab: String,
    pub unread: i64,
    pub has_feeds: bool,
    /// Said once, then cleared: "4 new articles", "Sent to Books".
    pub notice: String,
    /// Unix seconds of the last network refresh this session; 0 before one.
    pub last_refresh: i64,

    pub brief: Brief,

    pub sources: Vec<SourceRow>,
    /// "" everything · `d:<folder>` · `f:<feed id>`.
    pub source: String,
    pub list_title: String,
    pub unread_only: bool,
    pub query: String,
    pub list: Vec<ArticleRow>,
    pub open: Option<ArticleView>,

    pub saved: Vec<ArticleRow>,
    pub saved_total: i64,
    /// all | started | new | done.
    pub saved_filter: String,

    pub highlights: Vec<HighlightRow>,
    pub highlighted_articles: i64,

    /// Folder names in use, for the Follow dialog.
    pub folders: Vec<String>,
    /// The model on the user's own server that writes summaries; "" when
    /// summaries are the extractive ones.
    pub summarizer: String,
    /// That server's address, for the dialog that sets it.
    pub summarizer_url: String,
    /// The mailbox newsletters are read from: "you@fastmail.com · Newsletters";
    /// "" when none is set.
    pub mail: String,
    /// Its server and folder, for the dialog that sets it.
    pub mail_host: String,
    pub mail_folder: String,
}

pub struct Brief {
    pub stories: Vec<Story>,
    pub minutes: i64,
    /// The brief read aloud, estimated.
    pub listen_s: i64,
    pub newsletters: Vec<ArticleRow>,
}

/// One story, told by one source or several.
pub struct Story {
    /// The newest telling, which is what opens.
    pub article_id: i64,
    pub title: String,
    pub bullets: Vec<String>,
    pub sources: Vec<String>,
    pub image: String,
}

pub struct SourceRow {
    pub key: String,
    pub label: String,
    /// 0 top level, 1 a feed inside a folder.
    pub depth: i64,
    /// 0 for Everything and for a folder row.
    pub feed_id: i64,
    pub folder: String,
    pub unread: i64,
    /// Why the last refresh of this feed failed; empty when it did not.
    pub error: String,
}

pub struct ArticleRow {
    pub id: i64,
    pub feed_id: i64,
    pub source: String,
    pub title: String,
    pub published: i64,
    pub minutes: i64,
    pub image: String,
    pub read: bool,
    pub saved: bool,
    pub progress: f64,
    pub summary: bool,
}

pub struct ArticleView {
    pub id: i64,
    pub feed_id: i64,
    pub source: String,
    pub title: String,
    pub author: String,
    pub url: String,
    pub published: i64,
    pub minutes: i64,
    pub image: String,
    pub paragraphs: Vec<String>,
    pub summary: Vec<String>,
    pub saved: bool,
    pub progress: f64,
    /// The page has been fetched for this one; no point offering it again.
    pub full: bool,
    /// Quotes already kept from it, to mark in the text.
    pub highlights: Vec<String>,
    /// The summary came from the user's model server.
    pub summary_model: bool,
}

pub struct HighlightRow {
    pub id: i64,
    pub article_id: i64,
    pub quote: String,
    pub note: String,
    pub feed_id: i64,
    pub source: String,
    pub title: String,
    pub created: i64,
}

// ---------------------------------------------------------------- commands ---

pub enum FeedsCmd {
    /// The snapshot, from the database only.
    Refresh,
    /// Every feed, over the network.
    Fetch,
    SetTab { tab: String },
    SetSource { key: String },
    SetUnreadOnly { on: bool },
    Search { text: String },
    /// Open in the reader, and mark read.
    Open { id: i64 },
    SetRead { id: i64, read: bool },
    /// Everything unread in the current source.
    MarkAllRead,
    /// Everything unread that is not in today's brief.
    MarkRestRead,
    ToggleSaved { id: i64 },
    SetProgress { id: i64, progress: f64 },
    SetSavedFilter { filter: String },
    Follow { url: String, folder: String },
    Unfollow { feed_id: i64 },
    MoveFeed { feed_id: i64, folder: String },
    AddHighlight { id: i64, quote: String, note: String },
    SetHighlightNote { id: i64, note: String },
    DeleteHighlight { id: i64 },
    SendToBooks { id: i64 },
    OpenOriginal { id: i64 },
    /// Fetch the article's page for the text the feed left out.
    FullText { id: i64 },
    /// The mailbox newsletters come from, signed in to before it is kept. An
    /// empty `host` forgets it and its password.
    SetMail { host: String, port: i64, user: String, password: String, folder: String },
    /// The model server that writes summaries, tried before it is kept.
    /// An empty `url` goes back to the extractive summaries.
    SetSummarizer { url: String, model: String },
}

// ----------------------------------------------------------------- session ---

#[derive(Clone)]
struct Session {
    tab: String,
    source: String,
    unread_only: bool,
    query: String,
    open: i64,
    saved_filter: String,
    /// Read this session. They stay in an Unread list until the source
    /// changes, so the row you just clicked does not vanish from under you.
    seen: HashSet<i64>,
    notice: String,
    last_refresh: i64,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            tab: "today".into(),
            source: String::new(),
            unread_only: true,
            query: String::new(),
            open: 0,
            saved_filter: "all".into(),
            seen: HashSet::new(),
            notice: String::new(),
            last_refresh: 0,
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

// ---------------------------------------------------------------- exported ---

pub async fn feeds_dispatch(cmd: FeedsCmd) -> Result<FeedsState> {
    let pool = feeds_pool().await?;
    apply(pool, cmd).await?;
    snapshot(pool).await
}

/// Feeds with the page closed, on the shell's tick: every feed fetched on the
/// half hour, as the page does while it is built, so the brief is fresh
/// whichever section the app opened on. Nothing until Feeds has been opened
/// and made its database.
pub(crate) async fn background_tick() {
    if !tulipix_core::paths::db_path("feeds").is_some_and(|f| f.exists()) {
        return;
    }
    {
        // Claimed before the fetch, so the next tick does not start another.
        let mut s = lock();
        if feeds::now() - s.last_refresh < 30 * 60 {
            return;
        }
        s.last_refresh = feeds::now();
    }
    let Ok(pool) = feeds_pool().await else { return };
    if let Err(e) = feeds::refresh_all(pool, &feeds::client()).await {
        tracing::info!(error = %e, "feeds: the background refresh failed");
    }
    if let Err(e) = mail_pass(pool).await {
        tracing::info!(error = %e, "feeds: the mailbox could not be read");
    }
}

/// The mailbox as saved: settings for where, the keychain for the password.
fn mail_account() -> Option<crate::mail::Account> {
    let s = crate::api::shell::load();
    let host = s.text("feeds.mail-host");
    if host.trim().is_empty() {
        return None;
    }
    Some(crate::mail::Account {
        port: s.text("feeds.mail-port").parse().unwrap_or(993),
        user: s.text("feeds.mail-user"),
        folder: match s.text("feeds.mail-folder") {
            f if f.trim().is_empty() => "Newsletters".into(),
            f => f,
        },
        password: tulipix_core::api_keys::fetch(crate::mail::KEYCHAIN).ok().flatten().unwrap_or_default(),
        host,
    })
}

/// New issues from the mailbox since the last look, a day of overlap for
/// clocks that disagree (the Message-ID keeps a letter from arriving twice).
/// Ok(0) with no mailbox set.
async fn mail_pass(pool: &SqlitePool) -> Result<usize> {
    let Some(account) = mail_account() else { return Ok(0) };
    let today = chrono::Local::now().date_naive();
    let since = crate::api::shell::load()
        .text("feeds.mail-since")
        .parse::<chrono::NaiveDate>()
        .unwrap_or(today - chrono::TimeDelta::days(14));
    let raws = crate::mail::fetch(&account, since, 200).await?;
    let mut added = 0;
    for raw in raws {
        if let Some(l) = crate::mail::letter(&raw) {
            added += feeds::store_mail(pool, &l.address, &l.name, std::slice::from_ref(&l.item)).await?;
        }
    }
    crate::api::shell::put("feeds.mail-since", &(today - chrono::TimeDelta::days(1)).to_string());
    Ok(added)
}

/// Unread articles, for the sidebar's badge.
pub(crate) async fn unread_count() -> i32 {
    if !tulipix_core::paths::db_path("feeds").is_some_and(|f| f.exists()) {
        return 0;
    }
    let Ok(pool) = feeds_pool().await else { return 0 };
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM articles WHERE read = 0").fetch_one(pool).await.unwrap_or(0) as i32
}

/// A summary from the user's model server for one article, kept in place of
/// the extractive one. Its own call: a local model takes seconds, and the
/// reader should open before it answers.
pub async fn feeds_summarise(id: i64) -> Result<FeedsState> {
    let pool = feeds_pool().await?;
    let (url, model) = summarizer();
    if url.is_empty() {
        bail!("no summary server is set");
    }
    let (title, body): (String, String) =
        sqlx::query_as("SELECT title, body FROM articles WHERE id = ?").bind(id).fetch_one(pool).await?;
    if body.split_whitespace().count() < 60 {
        bail!("too short to summarise");
    }
    let lines = feeds::model_summary(&feeds::client(), &url, &model, &title, &body).await?;
    sqlx::query("UPDATE articles SET summary = ?, summary_by = 'model' WHERE id = ?")
        .bind(lines.join("\n"))
        .bind(id)
        .execute(pool)
        .await?;
    snapshot(pool).await
}

/// (address, model) of the summary server, both "" when none is set.
fn summarizer() -> (String, String) {
    let s = crate::api::shell::load();
    let url = s.text("feeds.summary-url");
    if url.trim().is_empty() { (String::new(), String::new()) } else { (url, s.text("feeds.summary-model")) }
}

/// An article's text as the sentences read-aloud speaks, one at a time. The
/// same splitter the Books reader uses.
pub fn feeds_sentences(text: String) -> Vec<String> {
    tulipix_books::tts::sentences(&text).into_iter().map(|s| s.text).collect()
}

/// Title and text of one article, for Listen on a card that has not been
/// opened.
pub async fn feeds_article_text(id: i64) -> Result<String> {
    let (title, body): (String, String) = sqlx::query_as("SELECT title, body FROM articles WHERE id = ?")
        .bind(id)
        .fetch_one(feeds_pool().await?)
        .await?;
    Ok(format!("{title}\n\n{body}"))
}

/// Follow every feed in an OPML file (another reader's export), in its
/// folders. Six at a time; one that cannot be reached is counted, not fatal.
pub async fn feeds_import_opml(path: String) -> Result<FeedsState> {
    let pool = feeds_pool().await?;
    let xml = tokio::fs::read_to_string(&path).await.with_context(|| format!("could not read {path}"))?;
    let listed = feeds::opml(&xml);
    if listed.is_empty() {
        bail!("no feeds in that file — is it an OPML export?");
    }
    let have: HashSet<String> = sqlx::query_scalar("SELECT url FROM feeds").fetch_all(pool).await?.into_iter().collect();
    let fresh: Vec<_> = listed.into_iter().filter(|(url, _, _)| !have.contains(url)).collect();
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(6));
    let client = feeds::client();
    let mut jobs = tokio::task::JoinSet::new();
    for (url, _, folder) in fresh {
        let (gate, client) = (gate.clone(), client.clone());
        jobs.spawn(async move {
            let _slot = gate.acquire_owned().await.ok()?;
            let (at, parsed) = feeds::discover(&client, &url).await.ok()?;
            feeds::follow(pool, &at, &folder, &parsed).await.ok()
        });
    }
    let (mut added, mut failed) = (0, 0);
    while let Some(r) = jobs.join_next().await {
        match r {
            Ok(Some(_)) => added += 1,
            _ => failed += 1,
        }
    }
    say(match failed {
        0 => format!("Following {}", plural(added, "new feed", "new feeds")),
        f => format!("Following {} · {} could not be reached", plural(added, "new feed", "new feeds"), plural(f, "feed", "feeds")),
    });
    snapshot(pool).await
}

/// Every followed feed as OPML, for another reader or a backup. Returns how
/// many were written.
pub async fn feeds_export_opml(path: String) -> Result<i64> {
    let rows: Vec<(String, String, String)> = sqlx::query_as("SELECT url, title, folder FROM feeds ORDER BY folder, title")
        .fetch_all(feeds_pool().await?)
        .await?;
    tokio::fs::write(&path, feeds::to_opml(&rows)).await.with_context(|| format!("could not write {path}"))?;
    Ok(rows.len() as i64)
}

/// Every highlight as Markdown, grouped by article, written to `path`.
/// Returns how many were written.
pub async fn feeds_export_highlights(path: String) -> Result<i64> {
    let pool = feeds_pool().await?;
    let rows: Vec<(i64, String, String, String, String, String)> = sqlx::query_as(
        "SELECT a.id, a.title, a.url, f.title, h.quote, h.note
         FROM highlights h JOIN articles a ON a.id = h.article_id JOIN feeds f ON f.id = a.feed_id
         ORDER BY a.published DESC, a.id, h.id",
    )
    .fetch_all(pool)
    .await?;
    let mut md = String::from("# Highlights\n");
    let mut last = 0;
    for (id, title, url, source, quote, note) in &rows {
        if *id != last {
            last = *id;
            md.push_str(&format!("\n## {title}\n\n{source}"));
            if !url.is_empty() {
                md.push_str(&format!(" · <{url}>"));
            }
            md.push('\n');
        }
        md.push_str(&format!("\n> {}\n", quote.trim().replace('\n', "\n> ")));
        if !note.trim().is_empty() {
            md.push_str(&format!("\n{}\n", note.trim()));
        }
    }
    tokio::fs::write(&path, md).await.with_context(|| format!("could not write {path}"))?;
    Ok(rows.len() as i64)
}

// ------------------------------------------------------------------- apply ---

async fn apply(pool: &'static SqlitePool, cmd: FeedsCmd) -> Result<()> {
    match cmd {
        FeedsCmd::Refresh => {}
        FeedsCmd::Fetch => {
            let (mut added, failed) = feeds::refresh_all(pool, &feeds::client()).await?;
            let mail = mail_pass(pool).await;
            added += *mail.as_ref().unwrap_or(&0);
            let mut s = lock();
            s.last_refresh = feeds::now();
            s.notice = match (added, failed) {
                (0, 0) => String::new(),
                (a, 0) => plural(a, "new article", "new articles"),
                (0, f) => format!("{} could not be reached", plural(f, "feed", "feeds")),
                (a, f) => format!(
                    "{} · {} could not be reached",
                    plural(a, "new article", "new articles"),
                    plural(f, "feed", "feeds")
                ),
            };
            if let Err(e) = mail {
                let line = format!("Your mail: {e}");
                s.notice = if s.notice.is_empty() { line } else { format!("{} · {line}", s.notice) };
            }
        }
        FeedsCmd::SetTab { tab } => lock().tab = tab,
        FeedsCmd::SetSource { key } => {
            let mut s = lock();
            s.source = key;
            s.query.clear();
            s.seen.clear();
        }
        FeedsCmd::SetUnreadOnly { on } => {
            let mut s = lock();
            s.unread_only = on;
            s.seen.clear();
        }
        FeedsCmd::Search { text } => {
            let mut s = lock();
            s.query = text.trim().to_string();
            if !s.query.is_empty() {
                s.tab = "unread".into();
            }
        }
        FeedsCmd::Open { id } => {
            {
                let mut s = lock();
                // Opened from Today, Read later or Highlights: the reader is on
                // Unread, and a source filter from earlier could hide the row.
                if s.tab != "unread" {
                    s.tab = "unread".into();
                    s.source.clear();
                }
                s.open = id;
                s.seen.insert(id);
            }
            set_read(pool, id, true).await?;
        }
        FeedsCmd::SetRead { id, read } => {
            lock().seen.insert(id);
            set_read(pool, id, read).await?;
        }
        FeedsCmd::MarkAllRead => {
            let source = lock().source.clone();
            let (feed, folder) = source_filter(&source);
            sqlx::query(
                "UPDATE articles SET read = 1 WHERE read = 0
                 AND (?1 = 0 OR feed_id = ?1)
                 AND (?2 = '' OR feed_id IN (SELECT id FROM feeds WHERE folder = ?2))",
            )
            .bind(feed)
            .bind(folder)
            .execute(pool)
            .await?;
        }
        FeedsCmd::MarkRestRead => {
            let (_, keep) = brief(pool, &names(pool).await?).await?;
            let keep = if keep.is_empty() {
                "0".to_string()
            } else {
                keep.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            };
            // Integers only, so formatting them in is not an injection.
            let r = sqlx::query(sqlx::AssertSqlSafe(format!(
                "UPDATE articles SET read = 1 WHERE read = 0 AND id NOT IN ({keep})"
            )))
            .execute(pool)
            .await?;
            say(format!("{} marked read", plural(r.rows_affected() as usize, "article", "articles")));
        }
        FeedsCmd::ToggleSaved { id } => {
            sqlx::query("UPDATE articles SET saved = 1 - saved, saved_at = ? WHERE id = ?")
                .bind(feeds::now())
                .bind(id)
                .execute(pool)
                .await?;
            // Read later means offline in full: the page is fetched now, while
            // there is a network, not when the train goes into the tunnel.
            let (saved, full): (bool, bool) =
                sqlx::query_as("SELECT saved, full FROM articles WHERE id = ?")
                    .bind(id)
                    .fetch_one(pool)
                    .await?;
            if saved && !full {
                if let Err(e) = feeds::fetch_full(pool, &feeds::client(), id).await {
                    tracing::info!(error = %e, id, "feeds: kept the feed's text; the page did not load");
                }
            }
        }
        FeedsCmd::SetProgress { id, progress } => {
            // The furthest point reached: scrolling back up to reread a
            // paragraph does not un-finish an article.
            sqlx::query("UPDATE articles SET progress = MAX(progress, ?) WHERE id = ?")
                .bind(progress.clamp(0.0, 1.0))
                .bind(id)
                .execute(pool)
                .await?;
        }
        FeedsCmd::SetSavedFilter { filter } => lock().saved_filter = filter,
        FeedsCmd::Follow { url, folder } => {
            let (at, parsed) = feeds::discover(&feeds::client(), &url).await?;
            let id = feeds::follow(pool, &at, &folder, &parsed).await?;
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM articles WHERE feed_id = ? AND read = 0")
                .bind(id)
                .fetch_one(pool)
                .await?;
            let name = if parsed.title.is_empty() { host(&at) } else { parsed.title.clone() };
            say(format!("Following {name} · {}", plural(n as usize, "unread article", "unread articles")));
        }
        FeedsCmd::Unfollow { feed_id } => {
            // A mail sender comes back with its next issue unless it is muted.
            sqlx::query("INSERT OR IGNORE INTO muted (url) SELECT url FROM feeds WHERE id = ? AND url LIKE 'mail:%'")
                .bind(feed_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM feeds WHERE id = ?").bind(feed_id).execute(pool).await?;
            let mut s = lock();
            if s.source == format!("f:{feed_id}") {
                s.source.clear();
            }
        }
        FeedsCmd::MoveFeed { feed_id, folder } => {
            sqlx::query("UPDATE feeds SET folder = ? WHERE id = ?")
                .bind(folder.trim())
                .bind(feed_id)
                .execute(pool)
                .await?;
        }
        FeedsCmd::AddHighlight { id, quote, note } => {
            let quote = quote.trim();
            if quote.is_empty() {
                bail!("select some text to highlight first");
            }
            sqlx::query("INSERT INTO highlights (article_id, quote, note, created) VALUES (?, ?, ?, ?)")
                .bind(id)
                .bind(quote)
                .bind(note.trim())
                .bind(feeds::now())
                .execute(pool)
                .await?;
        }
        FeedsCmd::SetHighlightNote { id, note } => {
            sqlx::query("UPDATE highlights SET note = ? WHERE id = ?")
                .bind(note.trim())
                .bind(id)
                .execute(pool)
                .await?;
        }
        FeedsCmd::DeleteHighlight { id } => {
            sqlx::query("DELETE FROM highlights WHERE id = ?").bind(id).execute(pool).await?;
        }
        FeedsCmd::SendToBooks { id } => send_to_books(pool, id).await?,
        FeedsCmd::OpenOriginal { id } => {
            let url: String = sqlx::query_scalar("SELECT url FROM articles WHERE id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?;
            if !feeds::is_web(&url) {
                bail!("this article has no web address");
            }
            crate::api::transfer::open_url(&url);
        }
        FeedsCmd::SetMail { host, port, user, password, folder } => {
            let host = host.trim().to_string();
            if host.is_empty() {
                crate::api::shell::put("feeds.mail-host", "");
                tulipix_core::api_keys::delete(crate::mail::KEYCHAIN).ok();
                say("Newsletters are no longer read from your mail");
            } else {
                let folder = if folder.trim().is_empty() { "Newsletters".to_string() } else { folder.trim().to_string() };
                let password = if password.is_empty() {
                    tulipix_core::api_keys::fetch(crate::mail::KEYCHAIN).ok().flatten().unwrap_or_default()
                } else {
                    password
                };
                let account = crate::mail::Account {
                    host: host.clone(),
                    port: u16::try_from(port).ok().filter(|p| *p > 0).unwrap_or(993),
                    user: user.trim().to_string(),
                    password,
                    folder: folder.clone(),
                };
                // Sign in and open the folder before anything is kept.
                crate::mail::fetch(&account, chrono::Local::now().date_naive(), 0).await?;
                tulipix_core::api_keys::store(crate::mail::KEYCHAIN, &account.password)?;
                let shell = [
                    ("feeds.mail-host", host.as_str()),
                    ("feeds.mail-user", account.user.as_str()),
                    ("feeds.mail-folder", folder.as_str()),
                    ("feeds.mail-since", ""),
                ];
                for (k, v) in shell {
                    crate::api::shell::put(k, v);
                }
                crate::api::shell::put("feeds.mail-port", &account.port.to_string());
                let n = mail_pass(pool).await?;
                say(format!("Reading {folder} · {}", plural(n, "newsletter", "newsletters")));
            }
        }
        FeedsCmd::SetSummarizer { url, model } => {
            let (url, model) = (url.trim().to_string(), model.trim().to_string());
            if url.is_empty() {
                crate::api::shell::put("feeds.summary-url", "");
                say("Summaries are picked from the article again");
            } else {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    bail!("the address should start with http:// — e.g. http://localhost:11434");
                }
                if model.is_empty() {
                    bail!("name the model — e.g. llama3.2");
                }
                // One small request first, so a wrong address or model says
                // so here rather than on every article.
                let sample = "The city council voted on Tuesday to reopen the harbour railway after a two year \
                    restoration. Trains will run at weekends from May. Volunteers rebuilt the track and two \
                    steam engines. The council says the line will bring visitors to the waterfront. Tickets \
                    go on sale next month, with discounts for residents and children under twelve.";
                feeds::model_summary(&feeds::client(), &url, &model, "Harbour railway to reopen", sample).await?;
                crate::api::shell::put("feeds.summary-url", &url);
                crate::api::shell::put("feeds.summary-model", &model);
                say(format!("Summaries now come from {model}"));
            }
        }
        FeedsCmd::FullText { id } => {
            if let Err(e) = feeds::fetch_full(pool, &feeds::client(), id).await {
                say(format!("The page did not load, so this is what the feed sent. {e}"));
            }
        }
    }
    Ok(())
}

async fn set_read(pool: &SqlitePool, id: i64, read: bool) -> Result<()> {
    sqlx::query("UPDATE articles SET read = ? WHERE id = ?")
        .bind(read)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The article as FictionBook into `<data>/articles`, which Books is told to
/// watch — the same hand-off Genesis makes with its download folder.
async fn send_to_books(pool: &SqlitePool, id: i64) -> Result<()> {
    let (title, author, body, source): (String, String, String, String) = sqlx::query_as(
        "SELECT a.title, a.author, a.body, f.title FROM articles a JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    if body.trim().is_empty() {
        bail!("there is no text to send yet — open the article to fetch it first");
    }
    let dir = tulipix_core::paths::data_dir().context("no data folder")?.join("articles");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.fb2", feeds::file_stem(&title)));
    let by = if author.is_empty() { source } else { author };
    std::fs::write(&path, feeds::fb2(&title, &by, &body))?;
    let books = crate::db::books_pool().await?;
    tulipix_books::scan::add_folder(books, &dir.display().to_string()).await?;
    tokio::spawn(async move {
        if let Err(e) = tulipix_books::scan::scan_all(books).await {
            tracing::warn!(error = %e, "feeds: books rescan after send");
        }
    });
    say("Sent to Books");
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

/// Columns of an [`ArticleRow`], in `row()`'s order.
const ROW_COLS: &str = "a.id, a.feed_id, a.title, a.published, length(a.body), a.image, \
                        a.read, a.saved, a.progress, a.summary != ''";

type Row = (i64, i64, String, i64, i64, String, bool, bool, f64, bool);

fn row(r: Row, names: &HashMap<i64, String>) -> ArticleRow {
    ArticleRow {
        id: r.0,
        feed_id: r.1,
        source: names.get(&r.1).cloned().unwrap_or_default(),
        title: r.2,
        published: r.3,
        minutes: minutes(r.4),
        image: r.5,
        read: r.6,
        saved: r.7,
        progress: r.8,
        summary: r.9,
    }
}

/// About 230 words a minute, at the five-and-a-bit characters a word English
/// averages.
fn minutes(chars: i64) -> i64 {
    (chars / 1150).max(1)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

fn host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
        .unwrap_or_else(|| url.to_string())
}

fn source_filter(key: &str) -> (i64, String) {
    match key.split_once(':') {
        Some(("f", id)) => (id.parse().unwrap_or(0), String::new()),
        Some(("d", folder)) => (0, folder.to_string()),
        _ => (0, String::new()),
    }
}

/// Feed id → the name it goes by: its title, or its host when it has none.
async fn names(pool: &SqlitePool) -> Result<HashMap<i64, String>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as("SELECT id, title, url FROM feeds").fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, url)| (id, if title.is_empty() { host(&url) } else { title }))
        .collect())
}

async fn snapshot(pool: &SqlitePool) -> Result<FeedsState> {
    let s = {
        let mut g = lock();
        let s = g.clone();
        g.notice.clear();
        s
    };
    let names = names(pool).await?;

    // ---- sources ----
    let feeds: Vec<(i64, String, String, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.url, f.folder, f.error, COALESCE(SUM(a.read = 0), 0)
         FROM feeds f LEFT JOIN articles a ON a.feed_id = f.id
         GROUP BY f.id
         ORDER BY f.folder = '', f.folder COLLATE NOCASE, f.title COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    let unread: i64 = feeds.iter().map(|f| f.4).sum();
    let mut sources = vec![SourceRow {
        key: String::new(),
        label: "Everything".into(),
        depth: 0,
        feed_id: 0,
        folder: String::new(),
        unread,
        error: String::new(),
    }];
    let mut folders: Vec<String> = Vec::new();
    for (id, _, folder, error, n) in &feeds {
        if !folder.is_empty() && folders.last() != Some(folder) {
            folders.push(folder.clone());
            sources.push(SourceRow {
                key: format!("d:{folder}"),
                label: folder.clone(),
                depth: 0,
                feed_id: 0,
                folder: folder.clone(),
                unread: feeds.iter().filter(|f| &f.2 == folder).map(|f| f.4).sum(),
                error: String::new(),
            });
        }
        sources.push(SourceRow {
            key: format!("f:{id}"),
            label: names.get(id).cloned().unwrap_or_default(),
            depth: if folder.is_empty() { 0 } else { 1 },
            feed_id: *id,
            folder: folder.clone(),
            unread: *n,
            error: error.clone(),
        });
    }

    // ---- the list ----
    let (feed, folder) = source_filter(&s.source);
    let like = if s.query.is_empty() { String::new() } else { format!("%{}%", s.query) };
    let seen = if s.seen.is_empty() {
        "0".to_string()
    } else {
        s.seen.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
    };
    // A search looks through everything, read or not.
    let list: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ROW_COLS} FROM articles a JOIN feeds f ON f.id = a.feed_id
         WHERE (?1 = 0 OR a.feed_id = ?1)
           AND (?2 = '' OR f.folder = ?2)
           AND (?3 = '' OR a.title LIKE ?3 OR a.body LIKE ?3)
           AND (?3 != '' OR ?4 = 0 OR a.read = 0 OR a.id IN ({seen}))
         ORDER BY a.published DESC LIMIT 400"
    )))
    .bind(feed)
    .bind(&folder)
    .bind(&like)
    .bind(s.unread_only)
    .fetch_all(pool)
    .await?;
    let list_title = if !s.query.is_empty() {
        format!("“{}”", s.query)
    } else if feed > 0 {
        names.get(&feed).cloned().unwrap_or_default()
    } else if !folder.is_empty() {
        folder.clone()
    } else {
        "Everything".into()
    };

    // ---- read later ----
    let saved: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ROW_COLS} FROM articles a
         WHERE a.saved = 1 AND (?1 = 'all'
            OR (?1 = 'started' AND a.progress > 0 AND a.progress < 0.98)
            OR (?1 = 'new' AND a.progress = 0)
            OR (?1 = 'done' AND a.progress >= 0.98))
         ORDER BY a.saved_at DESC"
    )))
    .bind(&s.saved_filter)
    .fetch_all(pool)
    .await?;
    let saved_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM articles WHERE saved = 1")
        .fetch_one(pool)
        .await?;

    // ---- highlights ----
    let hl: Vec<(i64, i64, String, String, i64, String, i64)> = sqlx::query_as(
        "SELECT h.id, h.article_id, h.quote, h.note, a.feed_id, a.title, h.created
         FROM highlights h JOIN articles a ON a.id = h.article_id
         ORDER BY h.created DESC",
    )
    .fetch_all(pool)
    .await?;
    let highlighted_articles = hl.iter().map(|h| h.1).collect::<HashSet<_>>().len() as i64;

    let (mut brief, _) = brief(pool, &names).await?;
    // Newsletters: the three newest unread from the feeds that are one.
    let letters: Vec<String> = feeds
        .iter()
        .filter(|(_, url, folder, ..)| feeds::is_newsletter(url, folder))
        .map(|(id, ..)| id.to_string())
        .collect();
    if !letters.is_empty() {
        let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {ROW_COLS} FROM articles a WHERE a.read = 0 AND a.feed_id IN ({})
             ORDER BY a.published DESC LIMIT 3",
            letters.join(",")
        )))
        .fetch_all(pool)
        .await?;
        brief.newsletters = rows.into_iter().map(|r| row(r, &names)).collect();
    }

    let open = if s.open > 0 { article_view(pool, s.open, &names).await? } else { None };

    let (summarizer_url, summarizer) = summarizer();
    let mail = mail_account();
    Ok(FeedsState {
        mail: mail.as_ref().map(|m| format!("{} · {}", m.user, m.folder)).unwrap_or_default(),
        mail_host: mail.as_ref().map(|m| m.host.clone()).unwrap_or_default(),
        mail_folder: mail.map(|m| m.folder).unwrap_or_default(),
        summarizer,
        summarizer_url,
        tab: s.tab,
        unread,
        has_feeds: !feeds.is_empty(),
        notice: s.notice,
        last_refresh: s.last_refresh,
        brief,
        sources,
        source: s.source,
        list_title,
        unread_only: s.unread_only,
        query: s.query,
        list: list.into_iter().map(|r| row(r, &names)).collect(),
        open,
        saved: saved.into_iter().map(|r| row(r, &names)).collect(),
        saved_total,
        saved_filter: s.saved_filter,
        highlights: hl
            .into_iter()
            .map(|(id, article_id, quote, note, feed_id, title, created)| HighlightRow {
                id,
                article_id,
                quote,
                note,
                feed_id,
                source: names.get(&feed_id).cloned().unwrap_or_default(),
                title,
                created,
            })
            .collect(),
        highlighted_articles,
        folders,
    })
}

async fn article_view(pool: &SqlitePool, id: i64, names: &HashMap<i64, String>) -> Result<Option<ArticleView>> {
    type Full = (i64, String, String, String, i64, String, String, String, bool, f64, bool, String);
    let r: Option<Full> = sqlx::query_as(
        "SELECT feed_id, title, author, url, published, image, body, summary, saved, progress, full, summary_by
         FROM articles WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((feed_id, title, author, url, published, image, body, summary, saved, progress, full, by)) = r else {
        return Ok(None);
    };
    let highlights: Vec<String> = sqlx::query_scalar("SELECT quote FROM highlights WHERE article_id = ? ORDER BY id")
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(Some(ArticleView {
        id,
        feed_id,
        source: names.get(&feed_id).cloned().unwrap_or_default(),
        title,
        author,
        url,
        published,
        minutes: minutes(body.len() as i64),
        image,
        paragraphs: body.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect(),
        summary: summary.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect(),
        saved,
        progress,
        full,
        highlights,
        summary_model: by == "model",
    }))
}

/// Today: the unread stories of the last day and a half, the ones the most
/// sources are telling first. Also returns every article id the brief covers,
/// for "Mark the rest read".
async fn brief(pool: &SqlitePool, names: &HashMap<i64, String>) -> Result<(Brief, Vec<i64>)> {
    type BriefRow = (i64, i64, String, i64, String, String, i64);
    let window = |since: i64| {
        sqlx::query_as::<_, BriefRow>(
            "SELECT id, feed_id, title, published, image, summary, length(body) FROM articles
             WHERE read = 0 AND published >= ? ORDER BY published DESC LIMIT 300",
        )
        .bind(since)
    };
    let mut rows: Vec<BriefRow> = window(feeds::now() - 36 * 3600).fetch_all(pool).await?;
    // An evening away leaves the window thin. A week is still news; an empty
    // brief is not a brief.
    if rows.len() < 5 {
        rows = window(feeds::now() - 7 * 86_400).fetch_all(pool).await?;
    }
    let heads: Vec<(i64, &str, &str)> = rows.iter().map(|r| (r.1, r.2.as_str(), r.5.as_str())).collect();
    let mut groups = feeds::cluster(&heads);
    groups.sort_by_key(|g| {
        let tellers = g.iter().map(|&i| rows[i].1).collect::<HashSet<_>>().len();
        let newest = g.iter().map(|&i| rows[i].3).max().unwrap_or(0);
        (Reverse(tellers), Reverse(newest))
    });
    groups.truncate(12);

    let mut stories = Vec::with_capacity(groups.len());
    let mut words = 0usize;
    for g in &groups {
        let mut members: Vec<&BriefRow> = g.iter().map(|&i| &rows[i]).collect();
        members.sort_by_key(|r| Reverse(r.3));
        // The plainest headline: the shortest that is still a sentence.
        let title = members
            .iter()
            .map(|r| r.2.as_str())
            .filter(|t| t.split_whitespace().count() >= 4)
            .min_by_key(|t| t.len())
            .unwrap_or(members[0].2.as_str())
            .to_string();
        let richest = members.iter().max_by_key(|r| r.6).copied().unwrap_or(members[0]);
        let bullets: Vec<String> = richest.5.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
        let mut sources: Vec<String> = Vec::new();
        for m in &members {
            let n = names.get(&m.1).cloned().unwrap_or_default();
            if !sources.contains(&n) {
                sources.push(n);
            }
        }
        words += title.split_whitespace().count()
            + bullets.iter().map(|b| b.split_whitespace().count()).sum::<usize>();
        stories.push(Story {
            article_id: members[0].0,
            title,
            bullets,
            sources,
            image: members.iter().find(|r| !r.4.is_empty()).map(|r| r.4.clone()).unwrap_or_default(),
        });
    }
    let ids = groups.iter().flatten().map(|&i| rows[i].0).collect();
    Ok((
        Brief {
            stories,
            // 200 wpm to read it, 170 to hear it.
            minutes: (words / 200).max(1) as i64,
            listen_s: (words * 60 / 170) as i64,
            newsletters: Vec::new(),
        },
        ids,
    ))
}
