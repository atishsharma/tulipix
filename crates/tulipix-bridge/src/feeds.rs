//! Feeds: the sites, blogs and newsletters you follow, kept in `feeds.db`.
//!
//! Here rather than in a domain crate because there is no Slint Feeds section
//! to share it with: the bridge is its only caller, the same reason
//! `fin_sheet` and the `vid_*` modules live here. `api::feeds` maps what
//! crosses to Dart.
//!
//! Built from what exists. The XML is read with the Books crate's scanners and
//! flattened with its `html_to_text`, the sentence splitter is the one
//! read-aloud uses, and RFC-822 dates fall back to the podcast parser's. What is
//! new is small: telling RSS, Atom and JSON Feed apart, finding the prose on a
//! web page, a summary, and grouping one story told by several sources.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use tulipix_books::epub::{attr, html_to_text, tags, unescape};

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS feeds (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    url           TEXT    NOT NULL UNIQUE,
    site          TEXT    NOT NULL DEFAULT '',
    title         TEXT    NOT NULL DEFAULT '',
    folder        TEXT    NOT NULL DEFAULT '',
    etag          TEXT    NOT NULL DEFAULT '',
    modified      TEXT    NOT NULL DEFAULT '',
    last_checked  INTEGER NOT NULL DEFAULT 0,
    error         TEXT    NOT NULL DEFAULT '',
    added         INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS articles (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_id    INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    guid       TEXT    NOT NULL,
    url        TEXT    NOT NULL DEFAULT '',
    title      TEXT    NOT NULL DEFAULT '',
    author     TEXT    NOT NULL DEFAULT '',
    published  INTEGER NOT NULL DEFAULT 0,
    image      TEXT    NOT NULL DEFAULT '',
    -- Plain text, paragraphs separated by a blank line.
    body       TEXT    NOT NULL DEFAULT '',
    -- One sentence per line; empty when the text is too short to summarise.
    summary    TEXT    NOT NULL DEFAULT '',
    -- The page itself has been fetched for this one, whatever it yielded.
    full       INTEGER NOT NULL DEFAULT 0,
    read       INTEGER NOT NULL DEFAULT 0,
    saved      INTEGER NOT NULL DEFAULT 0,
    saved_at   INTEGER NOT NULL DEFAULT 0,
    -- How far down the reader got, 0..1. Read later's Started / Finished.
    progress   REAL    NOT NULL DEFAULT 0,
    UNIQUE(feed_id, guid)
);
CREATE INDEX IF NOT EXISTS articles_feed_idx   ON articles(feed_id, published DESC);
CREATE INDEX IF NOT EXISTS articles_unread_idx ON articles(read, published DESC);
CREATE INDEX IF NOT EXISTS articles_saved_idx  ON articles(saved, saved_at DESC);

CREATE TABLE IF NOT EXISTS highlights (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    article_id INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    quote      TEXT    NOT NULL,
    note       TEXT    NOT NULL DEFAULT '',
    created    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS highlights_article_idx ON highlights(article_id);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── parsing ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    pub title: String,
    pub site: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Item {
    pub guid: String,
    pub url: String,
    pub title: String,
    pub author: String,
    /// Unix seconds; 0 when the feed gave no date anyone could read.
    pub published: i64,
    pub image: String,
    /// The body as the feed sent it: HTML, or plain text for a JSON Feed's
    /// `content_text`. Either flattens through `html_to_text`.
    pub html: String,
}

/// RSS (0.9x, 1.0 and 2.0), Atom, or JSON Feed. None for anything else — a web
/// page most often, which the caller then searches for a feed link.
pub fn parse(body: &str) -> Option<Parsed> {
    let body = body.trim_start().trim_start_matches('\u{feff}');
    if body.starts_with('{') {
        return parse_json(body);
    }
    let has = |name: &str| !tags(body, name).is_empty();
    if has("rss") || has("rdf:RDF") || has("channel") {
        return Some(parse_rss(body));
    }
    if has("feed") {
        return Some(parse_atom(body));
    }
    None
}

fn parse_rss(xml: &str) -> Parsed {
    let channel = xml.split("<item").next().unwrap_or(xml);
    Parsed {
        title: line(&inner(channel, "title").unwrap_or_default()),
        site: inner(channel, "link").unwrap_or_default(),
        items: blocks(xml, "item")
            .into_iter()
            .map(|b| {
                let html = inner(b, "content:encoded")
                    .filter(|s| !s.is_empty())
                    .or_else(|| inner(b, "description"))
                    .unwrap_or_default();
                let url = inner(b, "link")
                    .filter(|s| !s.is_empty())
                    .or_else(|| inner(b, "guid").filter(|g| g.starts_with("http")))
                    .unwrap_or_default();
                let title = line(&inner(b, "title").unwrap_or_default());
                Item {
                    guid: inner(b, "guid")
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| if url.is_empty() { title.clone() } else { url.clone() }),
                    published: inner(b, "pubDate")
                        .or_else(|| inner(b, "dc:date"))
                        .map(|d| date(&d))
                        .unwrap_or(0),
                    author: line(
                        &inner(b, "dc:creator")
                            .or_else(|| inner(b, "author"))
                            .unwrap_or_default(),
                    ),
                    image: media_image(b).unwrap_or_else(|| first_img(&html)),
                    url,
                    title,
                    html,
                }
            })
            .collect(),
    }
}

fn parse_atom(xml: &str) -> Parsed {
    let head = xml.split("<entry").next().unwrap_or(xml);
    Parsed {
        title: line(&inner(head, "title").unwrap_or_default()),
        site: alt_link(head),
        items: blocks(xml, "entry")
            .into_iter()
            .map(|b| {
                let url = alt_link(b);
                let html = inner(b, "content")
                    .filter(|s| !s.is_empty())
                    .or_else(|| inner(b, "summary"))
                    .unwrap_or_default();
                Item {
                    guid: inner(b, "id").filter(|s| !s.is_empty()).unwrap_or_else(|| url.clone()),
                    title: line(&inner(b, "title").unwrap_or_default()),
                    author: inner(b, "author")
                        .and_then(|a| inner(&a, "name"))
                        .map(|a| line(&a))
                        .unwrap_or_default(),
                    published: inner(b, "published")
                        .or_else(|| inner(b, "updated"))
                        .map(|d| date(&d))
                        .unwrap_or(0),
                    image: media_image(b).unwrap_or_else(|| first_img(&html)),
                    url,
                    html,
                }
            })
            .collect(),
    }
}

fn parse_json(body: &str) -> Option<Parsed> {
    use serde_json::Value;
    let v: Value = serde_json::from_str(body).ok()?;
    if !v.get("version").and_then(Value::as_str).is_some_and(|s| s.contains("jsonfeed")) {
        return None;
    }
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let items = v
        .get("items")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|it| {
                    let url = s(it, "url");
                    let id = match it.get("id") {
                        Some(Value::String(x)) => x.clone(),
                        Some(Value::Number(n)) => n.to_string(),
                        _ => url.clone(),
                    };
                    let author = it
                        .get("authors")
                        .and_then(|a| a.get(0))
                        .or_else(|| it.get("author"))
                        .map(|a| s(a, "name"))
                        .unwrap_or_default();
                    Item {
                        guid: id,
                        title: line(&s(it, "title")),
                        author,
                        published: date(&s(it, "date_published")),
                        image: [s(it, "image"), s(it, "banner_image")]
                            .into_iter()
                            .find(|x| !x.is_empty())
                            .unwrap_or_default(),
                        html: [s(it, "content_html"), s(it, "content_text"), s(it, "summary")]
                            .into_iter()
                            .find(|x| !x.is_empty())
                            .unwrap_or_default(),
                        url,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Parsed { title: line(&s(&v, "title")), site: s(&v, "home_page_url"), items })
}

/// Each `<name …>…</name>` block, tags included. `<item` must end there:
/// RSS 1.0's `<items>` is not an item.
fn blocks<'a>(xml: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i..];
        if !after[open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>') {
            rest = &after[open.len()..];
            continue;
        }
        let Some(e) = after.find(&close) else { break };
        out.push(&after[..e + close.len()]);
        rest = &after[e + close.len()..];
    }
    out
}

/// The text of the first `<name>` element in `hay`, CDATA unwrapped and
/// entities decoded. Empty for a self-closing one, None for none at all.
fn inner(hay: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let mut base = 0;
    let start = loop {
        let at = base + hay[base..].find(&open)?;
        if hay[at + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>' || c == '/') {
            break at;
        }
        base = at + open.len();
    };
    let gt = start + hay[start..].find('>')?;
    if hay[..gt].ends_with('/') {
        return Some(String::new());
    }
    let body = &hay[gt + 1..];
    let end = body.find(&format!("</{name}>"))?;
    Some(text(&body[..end]))
}

/// Element content as text: CDATA sections kept verbatim, everything around
/// them entity-decoded.
fn text(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw.trim();
    while let Some(i) = rest.find("<![CDATA[") {
        out.push_str(&unescape(&rest[..i]));
        let after = &rest[i + "<![CDATA[".len()..];
        let Some(e) = after.find("]]>") else {
            out.push_str(after);
            return out.trim().to_string();
        };
        out.push_str(&after[..e]);
        rest = &after[e + "]]>".len()..];
    }
    out.push_str(&unescape(rest));
    out.trim().to_string()
}

/// A title or a name on one line, with any markup an Atom `type="html"`
/// title carried taken out.
fn line(s: &str) -> String {
    let s = if s.contains('<') { html_to_text(s) } else { s.to_string() };
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Atom's `rel="alternate"` link, or the first link with no `rel` at all.
fn alt_link(hay: &str) -> String {
    let links = tags(hay, "link");
    links
        .iter()
        .find(|t| matches!(attr(t, "rel").as_deref(), None | Some("alternate")))
        .or(links.first())
        .and_then(|t| attr(t, "href"))
        .unwrap_or_default()
}

fn media_image(b: &str) -> Option<String> {
    let image_like = |t: &str| {
        attr(t, "medium").as_deref() == Some("image")
            || attr(t, "type").is_some_and(|m| m.starts_with("image/"))
    };
    tags(b, "media:thumbnail")
        .iter()
        .chain(tags(b, "media:content").iter().filter(|t| image_like(t)))
        .find_map(|t| attr(t, "url"))
        .or_else(|| tags(b, "enclosure").iter().filter(|t| image_like(t)).find_map(|t| attr(t, "url")))
        .or_else(|| tags(b, "itunes:image").iter().find_map(|t| attr(t, "href")))
        .filter(|u| u.starts_with("http"))
}

fn first_img(html: &str) -> String {
    tags(html, "img")
        .iter()
        .find_map(|t| attr(t, "src"))
        .filter(|u| u.starts_with("http"))
        .unwrap_or_default()
}

/// RFC 3339 (Atom, JSON Feed, `dc:date`) or RFC 2822 (RSS), with the podcast
/// parser for the RSS dates that are not quite either.
fn date(s: &str) -> i64 {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc3339(s)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(s))
        .map(|d| d.timestamp())
        .ok()
        .or_else(|| tulipix_music::podcasts::parse_rss_date(s))
        .unwrap_or(0)
}

// ── the web ─────────────────────────────────────────────────────────────────

/// A feed that is bigger than this is a mistake, or not a feed.
const MAX_BODY: usize = 12 * 1024 * 1024;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Tulipix/1.0 (feed reader)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default()
}

/// Only web addresses leave this module — a feed's `<link>` is somebody
/// else's text, and it ends up handed to the system's opener.
pub fn is_web(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<(String, String)> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let at = resp.url().to_string();
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_BODY {
        bail!("that address sent {} MB, which is not a feed", bytes.len() / (1024 * 1024));
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), at))
}

/// Whatever was pasted — a feed, or a page that names one in a
/// `<link rel="alternate">` — as the feed's address and its first parse.
pub async fn discover(client: &reqwest::Client, input: &str) -> Result<(String, Parsed)> {
    let mut url = input.trim().to_string();
    if !url.contains("://") {
        url = format!("https://{url}");
    }
    if !is_web(&url) {
        bail!("only http and https addresses can be followed");
    }
    let (body, at) = get_text(client, &url).await?;
    if let Some(p) = parse(&body) {
        return Ok((at, p));
    }
    let base = reqwest::Url::parse(&at)?;
    for t in tags(&body, "link") {
        let rel = attr(t, "rel").unwrap_or_default().to_ascii_lowercase();
        let kind = attr(t, "type").unwrap_or_default().to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "alternate")
            || !(kind.contains("rss") || kind.contains("atom") || kind.contains("feed+json"))
        {
            continue;
        }
        let Some(feed) = attr(t, "href").and_then(|h| base.join(&h).ok()) else { continue };
        let (body, at) = get_text(client, feed.as_str()).await?;
        if let Some(p) = parse(&body) {
            return Ok((at, p));
        }
    }
    bail!("no feed found at that address")
}

enum Fetched {
    Unchanged,
    Fresh { parsed: Parsed, etag: String, modified: String },
}

/// One conditional GET. A server that answers 304 costs a round trip and no
/// parse.
async fn fetch_feed(client: &reqwest::Client, url: &str, etag: &str, modified: &str) -> Result<Fetched> {
    use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
    let mut req = client.get(url);
    if !etag.is_empty() {
        req = req.header(IF_NONE_MATCH, etag);
    }
    if !modified.is_empty() {
        req = req.header(IF_MODIFIED_SINCE, modified);
    }
    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::Unchanged);
    }
    let resp = resp.error_for_status()?;
    let header = |h| {
        resp.headers()
            .get(h)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let (etag, modified) = (header(ETAG), header(LAST_MODIFIED));
    let bytes = resp.bytes().await?;
    if bytes.len() > MAX_BODY {
        bail!("the feed is larger than a feed should be");
    }
    let parsed = parse(&String::from_utf8_lossy(&bytes)).context("the address no longer serves a feed")?;
    Ok(Fetched::Fresh { parsed, etag, modified })
}

// ── the store ───────────────────────────────────────────────────────────────

/// Follow a feed, or move one already followed into `folder`.
///
/// Anything older than a week arrives already read: following a site should
/// not bury the morning under its archive.
pub async fn follow(pool: &SqlitePool, url: &str, folder: &str, p: &Parsed) -> Result<i64> {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO feeds (url, site, title, folder, added, last_checked) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(url) DO UPDATE SET folder = excluded.folder
         RETURNING id",
    )
    .bind(url)
    .bind(&p.site)
    .bind(&p.title)
    .bind(folder.trim())
    .bind(now())
    .bind(now())
    .fetch_one(pool)
    .await?;
    store_items(pool, id, &p.items).await?;
    sqlx::query("UPDATE articles SET read = 1 WHERE feed_id = ? AND published < ?")
        .bind(id)
        .bind(now() - 7 * 86_400)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn store_items(pool: &SqlitePool, feed_id: i64, items: &[Item]) -> Result<usize> {
    let now = now();
    let mut added = 0usize;
    let mut tx = pool.begin().await?;
    for it in items {
        let body = html_to_text(&it.html);
        let title = if it.title.is_empty() { clip(&body, 90) } else { it.title.clone() };
        // A date in the future is a feed's clock, not news from tomorrow; one
        // with no date at all is news from when we first saw it.
        let published = if it.published > 0 { it.published.min(now) } else { now };
        let r = sqlx::query(
            "INSERT OR IGNORE INTO articles (feed_id, guid, url, title, author, published, image, body, summary)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(feed_id)
        .bind(&it.guid)
        .bind(if is_web(&it.url) { it.url.as_str() } else { "" })
        .bind(&title)
        .bind(&it.author)
        .bind(published)
        .bind(&it.image)
        .bind(&body)
        .bind(summarise(&body).join("\n"))
        .execute(&mut *tx)
        .await?;
        added += r.rows_affected() as usize;
    }
    tx.commit().await?;
    Ok(added)
}

/// Every feed, a handful at a time. Returns (new articles, feeds that failed);
/// a failure is kept on its feed so the sources list can say which.
pub async fn refresh_all(pool: &SqlitePool, client: &reqwest::Client) -> Result<(usize, usize)> {
    let feeds: Vec<(i64, String, String, String)> =
        sqlx::query_as("SELECT id, url, etag, modified FROM feeds").fetch_all(pool).await?;
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(6));
    let mut set = tokio::task::JoinSet::new();
    for (id, url, etag, modified) in feeds {
        let (client, gate) = (client.clone(), gate.clone());
        set.spawn(async move {
            let _slot = gate.acquire().await;
            (id, fetch_feed(&client, &url, &etag, &modified).await)
        });
    }
    let (mut added, mut failed) = (0usize, 0usize);
    while let Some(done) = set.join_next().await {
        let Ok((id, result)) = done else {
            failed += 1;
            continue;
        };
        match result {
            Ok(Fetched::Unchanged) => {
                sqlx::query("UPDATE feeds SET last_checked = ?, error = '' WHERE id = ?")
                    .bind(now())
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
            Ok(Fetched::Fresh { parsed, etag, modified }) => {
                added += store_items(pool, id, &parsed.items).await?;
                sqlx::query(
                    "UPDATE feeds SET last_checked = ?, error = '', etag = ?, modified = ?,
                     title = CASE WHEN title = '' THEN ? ELSE title END,
                     site  = CASE WHEN site  = '' THEN ? ELSE site  END
                     WHERE id = ?",
                )
                .bind(now())
                .bind(etag)
                .bind(modified)
                .bind(&parsed.title)
                .bind(&parsed.site)
                .bind(id)
                .execute(pool)
                .await?;
            }
            Err(e) => {
                failed += 1;
                sqlx::query("UPDATE feeds SET last_checked = ?, error = ? WHERE id = ?")
                    .bind(now())
                    .bind(format!("{e:#}"))
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
        }
    }
    // Read, not kept, not quoted, and two months old: gone. Everything else
    // stays for as long as the feed does.
    sqlx::query(
        "DELETE FROM articles WHERE read = 1 AND saved = 0 AND published < ?
         AND id NOT IN (SELECT article_id FROM highlights)",
    )
    .bind(now() - 60 * 86_400)
    .execute(pool)
    .await?;
    Ok((added, failed))
}

/// Fetch the article's own page and keep its prose when that is more than the
/// feed sent. Marked as tried either way, so an excerpt-only site is asked once.
pub async fn fetch_full(pool: &SqlitePool, client: &reqwest::Client, id: i64) -> Result<()> {
    let (url, body, image): (String, String, String) =
        sqlx::query_as("SELECT url, body, image FROM articles WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
    let mark = || sqlx::query("UPDATE articles SET full = 1 WHERE id = ?").bind(id);
    if !is_web(&url) {
        mark().execute(pool).await?;
        return Ok(());
    }
    let html = match get_text(client, &url).await {
        Ok((html, _)) => html,
        Err(e) => {
            mark().execute(pool).await?;
            return Err(e);
        }
    };
    let text = readable(&html);
    let image = if image.is_empty() { og_image(&html) } else { image };
    if text.len() > body.len() {
        sqlx::query("UPDATE articles SET body = ?, summary = ?, image = ?, full = 1 WHERE id = ?")
            .bind(&text)
            .bind(summarise(&text).join("\n"))
            .bind(image)
            .bind(id)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE articles SET image = ?, full = 1 WHERE id = ?")
            .bind(image)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ── reading the page ────────────────────────────────────────────────────────

/// The prose of a web page.
///
/// ponytail: a heuristic, not Readability — the `<article>` (else `<main>`,
/// else the page) with its chrome cut out, keeping paragraphs that read like
/// sentences. Port Mozilla's scoring if sites keep leaking menus into it.
pub fn readable(html: &str) -> String {
    const CHROME: [&str; 10] =
        ["script", "style", "nav", "header", "footer", "aside", "form", "noscript", "figure", "button"];
    let prose = |region: &str| {
        html_to_text(&drop_elements(region, &CHROME))
            .split("\n\n")
            .filter(|p| is_prose(p))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let inside = region(html, "article").or_else(|| region(html, "main")).map(prose).unwrap_or_default();
    // A teaser `<article>` on a page whose story sits outside it.
    if inside.len() >= 600 { inside } else { prose(html) }
}

fn is_prose(p: &str) -> bool {
    let words = p.split_whitespace().count();
    words >= 12
        || (words >= 5 && p.trim_end().ends_with(|c: char| matches!(c, '.' | '!' | '?' | '"' | '”')))
}

/// From the first `<tag>` to the last `</tag>`.
fn region<'a>(html: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = html
        .match_indices(&open)
        .map(|(i, _)| i)
        .find(|&i| html[i + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>'))?;
    let end = html.rfind(&format!("</{tag}>"))?;
    (end > start).then(|| &html[start..end])
}

fn drop_elements(html: &str, names: &[&str]) -> String {
    let mut src = html.to_string();
    for name in names {
        let open = format!("<{name}");
        let close = format!("</{name}>");
        let mut from = 0;
        while let Some(rel) = src[from..].find(&open) {
            let i = from + rel;
            if !src[i + open.len()..].starts_with(|c: char| c.is_whitespace() || c == '>') {
                from = i + open.len();
                continue;
            }
            let Some(j) = src[i..].find(&close) else { break };
            src.replace_range(i..i + j + close.len(), " ");
            from = i;
        }
    }
    src
}

fn og_image(html: &str) -> String {
    tags(html, "meta")
        .iter()
        .find(|t| {
            attr(t, "property").or_else(|| attr(t, "name")).as_deref() == Some("og:image")
        })
        .and_then(|t| attr(t, "content"))
        .filter(|u| u.starts_with("http"))
        .unwrap_or_default()
}

// ── summaries and stories ───────────────────────────────────────────────────

const STOP: [&str; 48] = [
    "that", "this", "with", "from", "have", "were", "they", "their", "there", "would", "could",
    "should", "which", "what", "when", "where", "while", "about", "after", "before", "into",
    "than", "then", "them", "these", "those", "been", "being", "will", "just", "also", "more",
    "most", "some", "such", "only", "over", "very", "your", "says", "said", "like", "make",
    "made", "does", "because", "other", "here",
];

/// Lower-case words of four letters or more, stop words out, a trailing
/// plural `s` off — enough stemming to match "rules" with "rule".
fn keywords(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_lowercase)
        .filter(|w| !STOP.contains(&w.as_str()))
        .map(|w| {
            if w.chars().count() > 4 && w.ends_with('s') && !w.ends_with("ss") {
                w[..w.len() - 1].to_string()
            } else {
                w
            }
        })
        .collect()
}

/// Three sentences that carry the article.
///
/// ponytail: extractive (Luhn: frequent words, a thumb on the scale for the
/// lede), written on this computer and instant. An on-device model replaces
/// this function when tulipix-ai has one to lend.
pub fn summarise(text: &str) -> Vec<String> {
    let sents: Vec<String> = tulipix_books::tts::sentences(text)
        .into_iter()
        .map(|s| s.text)
        .filter(|s| s.split_whitespace().count() >= 6)
        .collect();
    if sents.len() < 5 {
        return Vec::new();
    }
    let keyed: Vec<Vec<String>> = sents.iter().map(|s| keywords(s)).collect();
    let mut freq: HashMap<&str, f64> = HashMap::new();
    for w in keyed.iter().flatten() {
        *freq.entry(w.as_str()).or_default() += 1.0;
    }
    let mut scored: Vec<(usize, f64)> = keyed
        .iter()
        .enumerate()
        .map(|(i, ws)| {
            let sum: f64 = ws.iter().map(|w| freq[w.as_str()]).sum();
            let score = sum / (ws.len().max(1) as f64).sqrt();
            (i, if i < 3 { score * 1.5 } else { score })
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut pick: Vec<usize> = scored.iter().take(3).map(|(i, _)| *i).collect();
    pick.sort_unstable();
    pick.into_iter().map(|i| clip(&sents[i], 260)).collect()
}

/// Headlines from different feeds that tell the same story, as groups of
/// indices into `items` (`(feed id, title)`). Every index lands in exactly one
/// group; a story nobody else covered is a group of one.
///
/// ponytail: title-word overlap, O(n²) over the brief's window (a few hundred
/// headlines). Photos AI's CLIP text encoder is the upgrade when two sources
/// describe one event in no common words.
pub fn cluster(items: &[(i64, &str)]) -> Vec<Vec<usize>> {
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let keys: Vec<HashSet<String>> = items.iter().map(|(_, t)| keywords(t).into_iter().collect()).collect();
    let mut parent: Vec<usize> = (0..items.len()).collect();
    for a in 0..items.len() {
        for b in a + 1..items.len() {
            if items[a].0 == items[b].0 {
                continue;
            }
            let shared = keys[a].intersection(&keys[b]).count();
            if shared >= 2 && shared * 2 >= keys[a].len().min(keys[b].len()) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..items.len() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups.into_values().collect()
}

/// A newsletter is a folder you named so, or a host that only sends them.
/// Mail-only newsletters need a mail bridge, which is not built.
pub fn is_newsletter(url: &str, folder: &str) -> bool {
    const HOSTS: [&str; 6] =
        ["substack.com", "buttondown.", "beehiiv.com", "ghost.io", "kill-the-newsletter.com", "newsletter"];
    folder.eq_ignore_ascii_case("newsletters") || HOSTS.iter().any(|h| url.contains(h))
}

pub fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

// ── out to Books and Markdown ───────────────────────────────────────────────

/// The article as FictionBook 2: one XML file the Books scanner already reads,
/// with no zip or image pipeline to write.
pub fn fb2(title: &str, author: &str, body: &str) -> String {
    let e = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let paras: String = body
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .map(|p| format!("<p>{}</p>\n", e(p.trim())))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\">\n\
         <description><title-info><genre>nonfiction</genre>\
         <author><nickname>{a}</nickname></author>\
         <book-title>{t}</book-title><lang>en</lang></title-info></description>\n\
         <body><title><p>{t}</p></title><section>\n{paras}</section></body>\n\
         </FictionBook>\n",
        a = e(author),
        t = e(title),
    )
}

/// A file name from a title: letters, digits, spaces and dashes, 80 at most.
pub fn file_stem(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' })
        .collect();
    let s = clip(&s.split_whitespace().collect::<Vec<_>>().join(" "), 80);
    let s = s.trim_end_matches('…').trim().to_string();
    if s.is_empty() { "Article".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?><rss version="2.0"
      xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel>
      <title>Circuit</title><link>https://circuit.example</link>
      <atom:link href="https://circuit.example/feed" rel="self"/>
      <item><title>Right to repair &amp; you</title><link>https://circuit.example/a</link>
        <guid isPermaLink="false">a-1</guid><pubDate>Sat, 19 Sep 2026 07:00:00 +0000</pubDate>
        <description>short</description>
        <content:encoded><![CDATA[<p>Full <b>body</b> here.</p><img src="https://img.example/x.jpg">]]></content:encoded>
      </item>
      <item><title>No link</title><guid>https://circuit.example/b</guid></item>
    </channel></rss>"#;

    #[test]
    fn rss_items() {
        let p = parse(RSS).unwrap();
        assert_eq!(p.title, "Circuit");
        assert_eq!(p.site, "https://circuit.example");
        assert_eq!(p.items.len(), 2);
        let a = &p.items[0];
        assert_eq!(a.title, "Right to repair & you");
        assert_eq!(a.guid, "a-1");
        assert_eq!(a.url, "https://circuit.example/a");
        assert_eq!(html_to_text(&a.html), "Full body here.");
        assert_eq!(a.image, "https://img.example/x.jpg");
        assert_eq!(a.published, 1_789_801_200);
        // The guid stands in for a missing link when it is an address.
        assert_eq!(p.items[1].url, "https://circuit.example/b");
    }

    #[test]
    fn atom_entries() {
        let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title type="html">Deep &lt;em&gt;Stack&lt;/em&gt;</title>
          <link rel="self" href="https://d.example/atom"/><link href="https://d.example/"/>
          <entry><id>tag:d,1</id><title>Cells</title><link rel="alternate" href="https://d.example/1"/>
            <author><name>Ada</name></author><updated>2026-09-19T08:00:00Z</updated>
            <summary>Short.</summary><content type="html">&lt;p&gt;Long.&lt;/p&gt;</content></entry></feed>"#;
        let p = parse(xml).unwrap();
        assert_eq!(p.title, "Deep Stack");
        assert_eq!(p.site, "https://d.example/");
        let e = &p.items[0];
        assert_eq!((e.guid.as_str(), e.url.as_str(), e.author.as_str()), ("tag:d,1", "https://d.example/1", "Ada"));
        assert_eq!(html_to_text(&e.html), "Long.");
        assert_eq!(e.published, 1_789_804_800);
    }

    #[test]
    fn json_feed_and_not_a_feed() {
        let j = r#"{"version":"https://jsonfeed.org/version/1.1","title":"M","items":[{"id":7,"url":"https://m.example/7","content_text":"Hi."}]}"#;
        let p = parse(j).unwrap();
        assert_eq!(p.items[0].guid, "7");
        assert!(parse("<!doctype html><html><head><title>x</title></head></html>").is_none());
    }

    #[test]
    fn readable_keeps_prose_and_drops_chrome() {
        let body = "This sentence is long enough to count as real prose on a page. ".repeat(12);
        let html = format!(
            "<html><nav><p>Home About Contact and a great many other links nobody reads at all</p></nav>\
             <article><h1>T</h1><p>{body}</p><p>Share</p></article><footer><p>© 2026 all rights reserved by whoever wrote it</p></footer></html>"
        );
        let text = readable(&html);
        assert!(text.starts_with("This sentence"));
        assert!(!text.contains("Share") && !text.contains("Contact") && !text.contains("rights"));
    }

    #[test]
    fn stories_group_across_feeds_only() {
        let items = [
            (1, "The EU's new right-to-repair rules take effect"),
            (2, "Right to repair arrives in Europe"),
            (1, "Right to repair: what the rules mean"),
            (3, "Harbour railway reopens in Bristol"),
        ];
        let mut groups = cluster(&items);
        groups.iter_mut().for_each(|g| g.sort_unstable());
        groups.sort();
        assert_eq!(groups, vec![vec![0, 1, 2], vec![3]]);
    }

    #[test]
    fn summary_picks_three_in_order() {
        let text = "Batteries must be replaceable with common tools from today onwards. \
            Spare parts must be sold for seven years after a phone leaves the shelves. \
            Repair manuals have to be public for every phone sold in Europe. \
            Some makers pair parts to the phone in software, which the rules allow. \
            A repair shop owner in Lyon called it the right law with the wrong definition. \
            Repair groups expect the first legal challenge about batteries within months.";
        let s = summarise(text);
        assert_eq!(s.len(), 3);
        let pos: Vec<usize> = s.iter().map(|x| text.find(x.as_str()).unwrap()).collect();
        assert!(pos.windows(2).all(|w| w[0] < w[1]));
        assert!(summarise("Too short. To summarise.").is_empty());
    }

    #[test]
    fn fb2_is_what_books_reads() {
        let x = fb2("A & B", "Circuit", "One.\n\nTwo <three>.");
        let m = tulipix_books::fb2::meta(&x);
        assert_eq!((m.title.as_str(), m.author.as_str()), ("A & B", "Circuit"));
        assert_eq!(file_stem("A/B: c?"), "A B c");
    }
}
