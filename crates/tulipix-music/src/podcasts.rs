//! `np.p4.music.podcasts` — RSS subscription, episode list, download manager.
//!
//! A dependency-light RSS reader: extracts the channel title and per-episode
//! guid / enclosure URL / pubDate / duration without pulling an XML crate
//! (podcast feeds are flat enough). Persists subscriptions + episodes and
//! tracks downloaded paths for auto-cleanup.

use anyhow::Result;
use sqlx::SqlitePool;

/// Schema for the standalone `podcasts.db` section. Self-contained — no `items`
/// foreign key — which is why podcasts can live in their own file. Columns that
/// were historically added to `music.db` via `ALTER TABLE` (category,
/// description, episode description/image_url) are inlined here.
pub const PODCASTS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS podcasts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_url     TEXT    NOT NULL UNIQUE,
    title        TEXT,
    author       TEXT,
    image_url    TEXT,
    custom_image TEXT,
    category     TEXT,
    description  TEXT,
    last_checked INTEGER
);

CREATE TABLE IF NOT EXISTS podcast_episodes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    podcast_id      INTEGER NOT NULL REFERENCES podcasts(id) ON DELETE CASCADE,
    guid            TEXT    NOT NULL,
    title           TEXT,
    audio_url       TEXT    NOT NULL,
    published       INTEGER,
    duration_s      REAL,
    description     TEXT,
    image_url       TEXT,
    downloaded_path TEXT,
    position_s      REAL    NOT NULL DEFAULT 0,
    played          INTEGER NOT NULL DEFAULT 0,
    UNIQUE(podcast_id, guid)
);
CREATE INDEX IF NOT EXISTS podcast_episodes_pod_idx ON podcast_episodes(podcast_id, published DESC);
CREATE INDEX IF NOT EXISTS podcast_episodes_dl_idx  ON podcast_episodes(downloaded_path);

-- Trends metadata cache: fetched once from the baked feed list
-- (resources/podcast-feeds.txt), then
-- loaded from here on every launch so we never re-fetch the network on startup.
CREATE TABLE IF NOT EXISTS podcast_trends (
    feed_url   TEXT PRIMARY KEY,
    title      TEXT,
    author     TEXT,
    category   TEXT,
    art_path   TEXT,
    fetched_at INTEGER
);
"#;

/// Apply the podcasts schema to a (podcasts.db) pool. Idempotent.
pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(PODCASTS_SCHEMA).execute(pool).await?;
    // Migrate pre-existing podcasts.db files that lack the custom_image column.
    // A user-set thumbnail lives here so a feed refresh (which overwrites
    // image_url) can never clobber it. Ignore the error when it already exists.
    let _ = sqlx::query("ALTER TABLE podcasts ADD COLUMN custom_image TEXT").execute(pool).await;
    // `home_pinned` marks shows the user added to Home "Your shows". Ignore error
    // when the column already exists.
    let _ = sqlx::query("ALTER TABLE podcasts ADD COLUMN home_pinned INTEGER NOT NULL DEFAULT 0").execute(pool).await;
    // When the offline copy was stored — drives the Downloads "Downloaded" sort.
    let _ = sqlx::query("ALTER TABLE podcast_episodes ADD COLUMN downloaded_at INTEGER").execute(pool).await;
    // Per-show listening settings. A show is the unit here, not the app: one
    // podcast is comfortable at 1.5x and the next is unlistenable above 1.2,
    // and "skip the first 45 seconds" is a fact about a jingle, not a taste.
    // `speed = 0` means "follow the section speed", which is why the default
    // is 0 and not 1.
    for sql in [
        "ALTER TABLE podcasts ADD COLUMN speed REAL NOT NULL DEFAULT 0",
        "ALTER TABLE podcasts ADD COLUMN skip_intro_s INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE podcasts ADD COLUMN auto_dl INTEGER NOT NULL DEFAULT 0",
        // 0 = keep every download this show has.
        "ALTER TABLE podcasts ADD COLUMN keep_last INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = sqlx::query(sql).execute(pool).await;
    }
    Ok(())
}

/// One attribute value out of an XML tag body (naive, double quotes required —
/// what every OPML exporter emits).
fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let k = format!("{name}=\"");
    let s = tag.find(&k)? + k.len();
    let e = tag[s..].find('"')? + s;
    Some(tag[s..e]
        .replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
        .replace("&quot;", "\"").replace("&#39;", "'"))
}

/// `(title, feed_url)` pairs out of an OPML file — every `<outline>` carrying
/// an `xmlUrl` attribute, any nesting depth (np.p5.music.podcast-opml).
pub fn parse_opml(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<outline") {
        let tag_end = rest[s..].find('>').map(|e| s + e).unwrap_or(rest.len());
        let tag = &rest[s..tag_end];
        if let Some(url) = xml_attr(tag, "xmlUrl") {
            let title = xml_attr(tag, "title").or_else(|| xml_attr(tag, "text")).unwrap_or_default();
            if !url.trim().is_empty() { out.push((title, url)); }
        }
        rest = &rest[tag_end..];
    }
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Serialize `(title, feed_url)` subscriptions to OPML 2.0 for export.
pub fn to_opml(subs: &[(String, String)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head><title>Tulipix podcast subscriptions</title></head>\n  <body>\n");
    for (title, url) in subs {
        s.push_str(&format!(
            "    <outline type=\"rss\" text=\"{t}\" title=\"{t}\" xmlUrl=\"{u}\"/>\n",
            t = xml_escape(title), u = xml_escape(url)));
    }
    s.push_str("  </body>\n</opml>\n");
    s
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub audio_url: String,
    pub duration_s: Option<f64>,
    /// `pubDate` parsed to a Unix epoch (seconds).
    pub published: Option<i64>,
    /// Show notes / summary (HTML stripped to plain text) — reused as the
    /// episode "transcript" panel in the UI.
    pub description: String,
    /// Per-episode `itunes:image href`, when present.
    pub image_url: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub author: String,
    pub image_url: String,
    pub category: String,
    pub description: String,
    pub episodes: Vec<ParsedEpisode>,
}

/// Text between the first `<tag ...>` and its closing `</tag>` within `hay`.
fn tag_text<'a>(hay: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = hay.find(&open)?;
    let after_open = hay[start..].find('>')? + start + 1;
    let close = hay[after_open..].find(&format!("</{tag}>"))? + after_open;
    Some(hay[after_open..close].trim())
}

/// Strip a leading `<![CDATA[ ... ]]>` wrapper.
fn uncdata(s: &str) -> String {
    s.trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>").trim().to_string()
}

/// Value of `attr="..."` inside the first `<tag ...>` of `hay`.
fn tag_attr(hay: &str, tag: &str, attr: &str) -> Option<String> {
    let start = hay.find(&format!("<{tag}"))?;
    let slice = &hay[start..];
    let end = slice.find('>')?;
    let open = &slice[..end];
    let key = format!("{attr}=\"");
    let ai = open.find(&key)? + key.len();
    let rest = &open[ai..];
    let vend = rest.find('"')?;
    Some(rest[..vend].to_string())
}

/// Parse `HH:MM:SS`, `MM:SS`, or bare seconds → seconds.
pub fn parse_itunes_duration(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok()).collect();
    let nums = nums?;
    Some(match nums.as_slice() {
        [s] => *s,
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    })
}

/// Crude HTML→text: drop tags, collapse whitespace, decode a few entities.
/// Show notes are HTML; this keeps them readable in the transcript panel.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">")
        .replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Days from civil date to the Unix epoch (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse an RFC-822 `pubDate` (e.g. `Wed, 15 Jun 2022 19:00:00 GMT`) → epoch
/// seconds. Timezone offsets beyond GMT are ignored (good enough for ordering).
pub fn parse_rss_date(s: &str) -> Option<i64> {
    const MON: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let s = s.trim();
    let body = s.split_once(", ").map(|(_, r)| r).unwrap_or(s);
    let mut it = body.split_whitespace();
    let day: i64 = it.next()?.parse().ok()?;
    let mon = it.next()?.to_ascii_lowercase();
    let month = MON.iter().position(|m| mon.starts_with(m))? as i64 + 1;
    let year: i64 = it.next()?.parse().ok()?;
    let (mut h, mut mi, mut se) = (0i64, 0i64, 0i64);
    if let Some(t) = it.next() {
        let mut tp = t.split(':');
        h = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        mi = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        se = tp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    }
    Some(days_from_civil(year, month, day) * 86400 + h * 3600 + mi * 60 + se)
}

pub fn parse_feed(xml: &str) -> ParsedFeed {
    // A web page has a <title> too: without this, a pasted Spotify or Apple
    // page "subscribes" as an empty show named after the page.
    if !xml.contains("<rss") && !xml.contains("<channel") {
        return ParsedFeed::default();
    }
    // Channel-level metadata is everything before the first <item>.
    let channel = xml.split("<item").next().unwrap_or(xml);
    let title = tag_text(channel, "title").map(uncdata);
    let author = tag_text(channel, "itunes:author")
        .or_else(|| tag_text(channel, "managingEditor"))
        .map(uncdata).unwrap_or_default();
    let image_url = tag_attr(channel, "itunes:image", "href")
        .or_else(|| tag_text(channel, "url").map(uncdata))
        .unwrap_or_default();
    let category = tag_attr(channel, "itunes:category", "text").map(|s| uncdata(&s)).unwrap_or_default();
    let description = tag_text(channel, "description")
        .or_else(|| tag_text(channel, "itunes:summary"))
        .map(uncdata).map(|d| strip_html(&d)).unwrap_or_default();
    let mut episodes = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<item") {
        let after = &rest[s..];
        let Some(e) = after.find("</item>") else { break };
        let block = &after[..e + "</item>".len()];
        if let Some(url) = tag_attr(block, "enclosure", "url") {
            let guid = tag_text(block, "guid").map(uncdata).unwrap_or_else(|| url.clone());
            let etitle = tag_text(block, "title").map(uncdata).unwrap_or_default();
            let dur = tag_text(block, "itunes:duration").and_then(parse_itunes_duration);
            let published = tag_text(block, "pubDate").and_then(parse_rss_date);
            let edesc = tag_text(block, "itunes:summary")
                .or_else(|| tag_text(block, "description"))
                .map(uncdata).map(|d| strip_html(&d)).unwrap_or_default();
            let eimg = tag_attr(block, "itunes:image", "href").unwrap_or_default();
            episodes.push(ParsedEpisode {
                guid, title: etitle, audio_url: url, duration_s: dur,
                published, description: edesc, image_url: eimg,
            });
        }
        rest = &after[e + "</item>".len()..];
    }
    ParsedFeed { title, author, image_url, category, description, episodes }
}

/// Whatever the user pasted, as an RSS URL. Apple Podcasts and Spotify links
/// name a show but are web pages; both resolve through Apple's public
/// directory, which carries each show's `feedUrl`. Anything else is returned
/// as given, on the assumption it already is a feed.
pub async fn resolve_feed_url(client: &reqwest::Client, url: &str) -> Result<String> {
    let url = url.trim();
    let Ok(parsed) = reqwest::Url::parse(url) else { return Ok(url.to_string()) };
    let host = parsed.host_str().unwrap_or_default();

    if host == "podcasts.apple.com" || host == "itunes.apple.com" {
        let id = apple_podcast_id(&parsed)
            .ok_or_else(|| anyhow::anyhow!("that Apple Podcasts link names no show"))?;
        let v: serde_json::Value = client
            .get("https://itunes.apple.com/lookup")
            .query(&[("id", id), ("entity", "podcast")])
            .send().await?.error_for_status()?.json().await?;
        return v["results"].as_array()
            .and_then(|r| r.iter().find_map(|r| r["feedUrl"].as_str()))
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("Apple lists no public feed for that show"));
    }

    // spotify.link / spotify.app.link are short links that redirect to open.spotify.com.
    if host == "open.spotify.com" || host == "spotify.link" || host == "spotify.app.link" {
        // The plain UA is what gets the server-rendered page with meta tags;
        // a full browser UA gets the empty web-player shell.
        let resp = client.get(url).header(reqwest::header::USER_AGENT, "Mozilla/5.0")
            .send().await?.error_for_status()?;
        let path = resp.url().path().to_string();
        if !path.contains("/show/") && !path.contains("/episode/") {
            anyhow::bail!("that Spotify link is not a podcast show or episode");
        }
        let html = resp.text().await?;
        let name = spotify_show_name(&html)
            .ok_or_else(|| anyhow::anyhow!("could not read the show name off that Spotify page"))?;
        let v: serde_json::Value = client
            .get("https://itunes.apple.com/search")
            .query(&[("term", name.as_str()), ("entity", "podcast"), ("limit", "25")])
            .send().await?.error_for_status()?.json().await?;
        return feed_for_name(&v, &name).ok_or_else(|| anyhow::anyhow!(
            "no public feed found for “{name}” — it may be a Spotify exclusive"));
    }

    Ok(url.to_string())
}

/// `1200361736` out of `…/podcast/the-daily/id1200361736` (episode links keep it too).
fn apple_podcast_id(url: &reqwest::Url) -> Option<&str> {
    url.path_segments()?.rev().find_map(|s| {
        s.strip_prefix("id").filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// `content` of `<meta property="{prop}" content="…">`.
fn og_meta(html: &str, prop: &str) -> Option<String> {
    let at = html.find(&format!("property=\"{prop}\""))?;
    let start = html[..at].rfind('<')?;
    let end = html[at..].find('>')? + at;
    xml_attr(&html[start..end], "content")
}

/// The show a Spotify page belongs to. A show page titles itself with the
/// show; an episode page titles itself with the episode and describes itself
/// as `Show · Episode`.
fn spotify_show_name(html: &str) -> Option<String> {
    let desc = og_meta(html, "og:description").unwrap_or_default();
    let segs: Vec<&str> = desc.split(" · ").map(str::trim).collect();
    let name = if segs.len() >= 2 && segs[1] == "Episode" {
        segs[0].to_string()
    } else {
        og_meta(html, "og:title")?
    };
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// The `feedUrl` of the search result whose name is exactly `name`. Apple's
/// search is fuzzy — "The Daily" also returns The Journal and Up First — so
/// the first hit is not good enough.
// ponytail: two shows with the identical name pick the first; compare the
// Spotify page's author against artistName if that ever bites.
fn feed_for_name(v: &serde_json::Value, name: &str) -> Option<String> {
    v["results"].as_array()?.iter()
        .filter(|r| r["collectionName"].as_str().is_some_and(|n| n.trim().eq_ignore_ascii_case(name)))
        .find_map(|r| r["feedUrl"].as_str().map(String::from))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Subscribe (or refresh) a feed and upsert its episodes. Returns podcast id.
pub async fn subscribe(pool: &SqlitePool, feed_url: &str, feed: &ParsedFeed) -> Result<i64> {
    subscribe_with_progress(pool, feed_url, feed, |_, _| {}).await
}

/// Like [`subscribe`], but invokes `on_progress(done, total)` after each episode
/// upsert so the UI can show a live progress bar while a feed loads.
pub async fn subscribe_with_progress<F: FnMut(usize, usize)>(
    pool: &SqlitePool, feed_url: &str, feed: &ParsedFeed, mut on_progress: F,
) -> Result<i64> {
    sqlx::query(
        "INSERT INTO podcasts (feed_url, title, author, image_url, category, description, last_checked)
         VALUES (?,?,?,?,?,?,?)
         ON CONFLICT(feed_url) DO UPDATE SET
            title=excluded.title, author=excluded.author, image_url=excluded.image_url,
            category=excluded.category, description=excluded.description, last_checked=excluded.last_checked")
        .bind(feed_url).bind(&feed.title).bind(&feed.author).bind(&feed.image_url)
        .bind(&feed.category).bind(&feed.description).bind(now()).execute(pool).await?;
    let pid: i64 = sqlx::query_scalar("SELECT id FROM podcasts WHERE feed_url = ?").bind(feed_url).fetch_one(pool).await?;
    let total = feed.episodes.len();
    // Insert episodes in parallel chunks, each chunk wrapped in its own
    // transaction. Batching commits (instead of one autocommit per row) is the
    // big win; the chunks run concurrently (WAL + busy_timeout make this safe)
    // and the pool serialises the actual writes. Subscribing a 300-episode feed
    // drops from "insert one by one" to a handful of batched commits.
    if total > 0 {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        // ~6 chunks (capped so we never exhaust the connection pool).
        let chunks = total.min(6).max(1);
        let chunk_size = total.div_ceil(chunks);
        let eps = Arc::new(feed.episodes.clone());
        let done = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for start in (0..total).step_by(chunk_size) {
            let end = (start + chunk_size).min(total);
            let pool = pool.clone();
            let eps = eps.clone();
            let done = done.clone();
            handles.push(tokio::spawn(async move {
                let mut tx = pool.begin().await?;
                for ep in &eps[start..end] {
                    // Upsert keeps user state (played/position/downloaded_path)
                    // while refreshing title/description as the feed evolves.
                    sqlx::query(
                        "INSERT INTO podcast_episodes (podcast_id, guid, title, audio_url, duration_s, published, description, image_url)
                         VALUES (?,?,?,?,?,?,?,?)
                         ON CONFLICT(podcast_id, guid) DO UPDATE SET
                            title=excluded.title, duration_s=excluded.duration_s,
                            published=excluded.published, description=excluded.description, image_url=excluded.image_url")
                        .bind(pid).bind(&ep.guid).bind(&ep.title).bind(&ep.audio_url)
                        .bind(ep.duration_s).bind(ep.published).bind(&ep.description).bind(&ep.image_url)
                        .execute(&mut *tx).await?;
                    done.fetch_add(1, Ordering::Relaxed);
                }
                tx.commit().await?;
                Ok::<(), sqlx::Error>(())
            }));
        }
        // Await each chunk, reporting cumulative progress as they land.
        for h in handles {
            h.await.map_err(|e| sqlx::Error::Protocol(e.to_string()))??;
            on_progress(done.load(Ordering::Relaxed), total);
        }
    }
    // Keep the table bounded on high-volume feeds (newest N + anything the user
    // touched). This is what stops podcasts.db ballooning over time.
    let _ = prune_feed(pool, pid, DEFAULT_KEEP_PER_FEED).await;
    Ok(pid)
}

/// Default cap on stored episodes per feed (see [`prune_feed`]).
pub const DEFAULT_KEEP_PER_FEED: i64 = 300;

/// Cap stored episodes for one feed: keep the newest `keep`, plus any that are
/// downloaded or partially played, deleting the rest. Returns rows removed.
pub async fn prune_feed(pool: &SqlitePool, podcast_id: i64, keep: i64) -> Result<u64> {
    let res = sqlx::query(
        "DELETE FROM podcast_episodes
         WHERE podcast_id = ?1
           AND downloaded_path IS NULL
           AND COALESCE(position_s, 0) = 0
           AND played = 0
           AND id NOT IN (
               SELECT id FROM podcast_episodes
               WHERE podcast_id = ?1
               ORDER BY COALESCE(published, 0) DESC, id DESC
               LIMIT ?2)")
        .bind(podcast_id).bind(keep).execute(pool).await?;
    Ok(res.rows_affected())
}

pub async fn mark_downloaded(pool: &SqlitePool, episode_id: i64, path: &str) -> Result<()> {
    sqlx::query("UPDATE podcast_episodes SET downloaded_path = ?, downloaded_at = strftime('%s','now') WHERE id = ?")
        .bind(path).bind(episode_id).execute(pool).await?;
    Ok(())
}

/// Played-and-downloaded episode ids older than `cutoff` plays — the
/// auto-cleanup candidates.
pub async fn cleanup_candidates(pool: &SqlitePool) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as("SELECT id, downloaded_path FROM podcast_episodes WHERE played = 1 AND downloaded_path IS NOT NULL")
        .fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    const FEED: &str = r#"<rss><channel><title>My Show</title>
      <item><title><![CDATA[Ep 1]]></title><guid>g1</guid>
        <enclosure url="https://x/1.mp3" type="audio/mpeg"/>
        <itunes:duration>01:02:03</itunes:duration></item>
      <item><title>Ep 2</title><guid>g2</guid>
        <enclosure url="https://x/2.mp3" type="audio/mpeg"/></item>
    </channel></rss>"#;

    #[test]
    fn parses_channel_and_items() {
        let f = parse_feed(FEED);
        assert_eq!(f.title.as_deref(), Some("My Show"));
        assert_eq!(f.episodes.len(), 2);
        assert_eq!(f.episodes[0].audio_url, "https://x/1.mp3");
        assert_eq!(f.episodes[0].duration_s, Some(3723.0));
        assert_eq!(f.episodes[1].guid, "g2");
    }

    #[test]
    fn web_page_is_not_a_feed() {
        let f = parse_feed("<html><head><title>The Daily | Podcast on Spotify</title></head></html>");
        assert_eq!(f, ParsedFeed::default());
    }

    #[test]
    fn apple_id_from_show_and_episode_links() {
        let id = |u: &str| apple_podcast_id(&reqwest::Url::parse(u).unwrap()).map(String::from);
        assert_eq!(id("https://podcasts.apple.com/us/podcast/the-daily/id1200361736").as_deref(), Some("1200361736"));
        assert_eq!(id("https://podcasts.apple.com/us/podcast/x/id1200361736?i=1000123").as_deref(), Some("1200361736"));
        assert_eq!(id("https://podcasts.apple.com/us/browse"), None);
    }

    #[test]
    fn spotify_show_name_from_show_and_episode_pages() {
        // Shapes as served to a `Mozilla/5.0` UA, September 2026.
        let show = r#"<meta property="og:title" content="The Daily"/>
            <meta property="og:description" content="Podcast · The New York Times · This is what the news should sound like."/>"#;
        let episode = r#"<meta property="og:title" content="48 Days Until the Midterms"/>
            <meta property="og:description" content="The Daily · Episode"/>"#;
        let escaped = r#"<meta property="og:title" content="Tom &amp; Jerry"/>"#;
        assert_eq!(spotify_show_name(show).as_deref(), Some("The Daily"));
        assert_eq!(spotify_show_name(episode).as_deref(), Some("The Daily"));
        assert_eq!(spotify_show_name(escaped).as_deref(), Some("Tom & Jerry"));
        assert_eq!(spotify_show_name("<title>Spotify – Web Player</title>"), None);
    }

    #[test]
    fn feed_for_name_wants_the_exact_show() {
        let v = serde_json::json!({"results": [
            {"collectionName": "The Journal.", "feedUrl": "https://j/rss"},
            {"collectionName": "The Daily", "feedUrl": "https://d/rss"},
        ]});
        assert_eq!(feed_for_name(&v, "the daily").as_deref(), Some("https://d/rss"));
        assert_eq!(feed_for_name(&v, "Up First"), None);
    }

    #[test]
    fn duration_formats() {
        assert_eq!(parse_itunes_duration("90"), Some(90.0));
        assert_eq!(parse_itunes_duration("2:30"), Some(150.0));
        assert_eq!(parse_itunes_duration("1:00:00"), Some(3600.0));
    }

    #[tokio::test]
    async fn subscribe_upserts_episodes() {
        let (_t, pool) = open_pool().await;
        let f = parse_feed(FEED);
        let pid = subscribe(&pool, "https://x/feed.xml", &f).await.unwrap();
        // re-subscribe = no duplicates
        subscribe(&pool, "https://x/feed.xml", &f).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?").bind(pid).fetch_one(&pool).await.unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn opml_roundtrip() {
        let subs = vec![
            ("My Show".to_string(), "https://x/feed.xml".to_string()),
            ("A & B <news>".to_string(), "https://y/f?a=1&b=2".to_string()),
        ];
        let xml = to_opml(&subs);
        assert_eq!(parse_opml(&xml), subs);
        // Nested folders + text-only outlines are tolerated.
        let foreign = r#"<opml><body><outline text="folder">
          <outline type="rss" text="Z" xmlUrl="https://z/rss"/></outline></body></opml>"#;
        assert_eq!(parse_opml(foreign), vec![("Z".to_string(), "https://z/rss".to_string())]);
    }
}
