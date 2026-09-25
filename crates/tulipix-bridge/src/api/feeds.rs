// The Feeds section: sites, blogs and newsletters in one reader.
//
// Four tabs — Today's brief, Unread (sources · list · reader), Read later and
// Highlights — over one snapshot. The parsing, extraction, summaries and story
// grouping are `crate::feeds`; this file keeps the session (which tab, which
// source, which article is open) and maps rows into what Dart draws.
//
// Nothing refreshes on its own schedule here. The page asks on open and every
// half hour while it is built, which is what the Slint podcast refresh does
// for its own feeds.

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
            let (added, failed) = feeds::refresh_all(pool, &feeds::client()).await?;
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

    Ok(FeedsState {
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
    type Full = (i64, String, String, String, i64, String, String, String, bool, f64, bool);
    let r: Option<Full> = sqlx::query_as(
        "SELECT feed_id, title, author, url, published, image, body, summary, saved, progress, full
         FROM articles WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((feed_id, title, author, url, published, image, body, summary, saved, progress, full)) = r else {
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
    let heads: Vec<(i64, &str)> = rows.iter().map(|r| (r.1, r.2.as_str())).collect();
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
