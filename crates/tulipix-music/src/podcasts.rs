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

-- Trends metadata cache: fetched once from the baked feed list (podc.md), then
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
    Ok(())
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
fn strip_html(s: &str) -> String {
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
}
