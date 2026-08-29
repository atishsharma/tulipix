//! `np.p5.music.podcast-trends` — the baked podcast directory behind the
//! Trends grid.
//!
//! Feed URLs come from `resources/podcast-feeds.txt` (one per line, `#` and
//! blanks ignored) so the page stays populated after a full library reset —
//! there is nothing in `podcasts.db` for it to lose. Per-feed metadata is
//! fetched once and persisted in `podcast_trends`, then read from there on
//! every launch, so a cold start costs no network.
//!
//! This lives in the domain crate rather than a front-end one because both
//! builds show the same directory and share `podcasts.db`: metadata fetched by
//! one is already cached for the other.

use std::collections::HashMap;
use std::path::PathBuf;

use sqlx::SqlitePool;

const FEEDS_RAW: &str = include_str!("../../../resources/podcast-feeds.txt");

/// The baked directory, in file order.
pub fn feed_urls() -> Vec<String> {
    FEEDS_RAW
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("http"))
        .map(|s| s.to_string())
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct TrendMeta {
    pub feed_url: String,
    pub title: String,
    pub author: String,
    pub category: String,
    pub art: Option<PathBuf>,
}

/// Local file, or a cached download of a remote URL. Artwork keys are caller-
/// chosen and stable (`trend-3`), so a second visit re-uses the file.
pub async fn cache_art(client: &reqwest::Client, key: &str, src: &str) -> Option<PathBuf> {
    if src.is_empty() {
        return None;
    }
    let p = std::path::Path::new(src);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if !src.starts_with("http") {
        return None;
    }
    let dir = tulipix_core::paths::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("podcast_art");
    let _ = std::fs::create_dir_all(&dir);
    let ext = src
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 4 && !e.contains('/'))
        .unwrap_or("jpg");
    let dest = dir.join(format!("{key}.{ext}"));
    if dest.exists() {
        return Some(dest);
    }
    let bytes = client
        .get(src)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    std::fs::write(&dest, &bytes).ok()?;
    Some(dest)
}

/// Rows already in `podcast_trends`, keyed by feed URL.
///
/// A row whose title is empty or still the feed URL is the placeholder a FAILED
/// fetch used to leave behind. Because [`build`] only fetches feeds missing
/// from this map, one bad moment would pin that card to "no artwork, a URL for
/// a name" forever — so those are treated as absent and retried.
pub async fn load_cached(pool: &SqlitePool) -> HashMap<String, TrendMeta> {
    let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT feed_url, COALESCE(title,''), COALESCE(author,''), COALESCE(category,''), art_path \
         FROM podcast_trends",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let mut out = HashMap::new();
    for (feed_url, title, author, category, art_path) in rows {
        if title.trim().is_empty() || title == feed_url {
            continue;
        }
        let art = art_path
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists());
        out.insert(
            feed_url.clone(),
            TrendMeta { feed_url, title, author, category, art },
        );
    }
    out
}

/// Fetch one feed's card metadata. `ok` is false when the feed was unreachable:
/// the card still needs *something* to show this session (the host reads better
/// than a raw URL), but the row must not be persisted.
async fn fetch_one(client: &reqwest::Client, idx: usize, url: String) -> (bool, TrendMeta) {
    let (mut title, mut author, mut category, mut art) =
        (String::new(), String::new(), String::new(), None);
    if let Ok(resp) = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await
    {
        if let Ok(xml) = resp.text().await {
            let feed = crate::podcasts::parse_feed(&xml);
            title = feed.title.unwrap_or_default();
            author = feed.author;
            category = feed.category;
            art = cache_art(client, &format!("trend-{idx}"), &feed.image_url).await;
        }
    }
    let ok = !title.is_empty();
    if title.is_empty() {
        title = url.split('/').nth(2).unwrap_or(url.as_str()).to_string();
    }
    if category.is_empty() {
        category = "Other".to_string();
    }
    (ok, TrendMeta { feed_url: url, title, author, category, art })
}

/// The whole directory, in feed-list order: cached rows, plus a concurrent
/// fetch of whatever is missing, persisted as it lands.
pub async fn build(pool: &SqlitePool, client: &reqwest::Client) -> Vec<TrendMeta> {
    let feeds = feed_urls();
    let mut stored = load_cached(pool).await;

    let handles: Vec<_> = feeds
        .iter()
        .enumerate()
        .filter(|(_, u)| !stored.contains_key(*u))
        .map(|(i, u)| {
            let (client, url) = (client.clone(), u.clone());
            tokio::spawn(async move { fetch_one(&client, i, url).await })
        })
        .collect();

    let (mut fetched, mut failed) = (Vec::new(), Vec::new());
    for h in handles {
        if let Ok((ok, m)) = h.await {
            if ok {
                fetched.push(m);
            } else {
                failed.push(m);
            }
        }
    }
    if !failed.is_empty() {
        tracing::warn!(count = failed.len(), "podcast trends: feeds unreachable, will retry next visit");
    }

    if !fetched.is_empty() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for m in &fetched {
            let art_s = m.art.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            let _ = sqlx::query(
                "INSERT INTO podcast_trends (feed_url, title, author, category, art_path, fetched_at) \
                 VALUES (?,?,?,?,?,?) \
                 ON CONFLICT(feed_url) DO UPDATE SET title=excluded.title, author=excluded.author, \
                    category=excluded.category, art_path=excluded.art_path, fetched_at=excluded.fetched_at",
            )
            .bind(&m.feed_url).bind(&m.title).bind(&m.author)
            .bind(&m.category).bind(&art_s).bind(now)
            .execute(pool)
            .await;
        }
        for m in fetched {
            stored.insert(m.feed_url.clone(), m);
        }
    }
    // Unreachable feeds stay session-only, so the grid has a card for them and
    // the next visit tries the network again.
    for m in failed {
        stored.entry(m.feed_url.clone()).or_insert(m);
    }
    feeds.iter().filter_map(|u| stored.get(u).cloned()).collect()
}

/// Which feed URLs are already subscribed, so a Trends card can say so.
pub async fn subscribed_urls(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT feed_url FROM podcasts")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_is_urls_only() {
        let feeds = feed_urls();
        assert!(!feeds.is_empty(), "the baked directory must not be empty");
        assert!(feeds.iter().all(|u| u.starts_with("http")), "comments and blanks are dropped");
        let mut sorted = feeds.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), feeds.len(), "no duplicate feeds");
    }

    #[tokio::test]
    async fn placeholder_rows_are_retried_not_trusted() {
        let (_t, pool) = crate::schema::tests::open_pool().await;
        for (url, title) in [
            ("https://good.example/f.xml", "A Real Show"),
            ("https://bad.example/f.xml", "https://bad.example/f.xml"),
            ("https://empty.example/f.xml", ""),
        ] {
            sqlx::query("INSERT INTO podcast_trends (feed_url, title) VALUES (?, ?)")
                .bind(url).bind(title).execute(&pool).await.unwrap();
        }
        let cached = load_cached(&pool).await;
        assert_eq!(cached.len(), 1, "only the row with a real title counts as cached");
        assert!(cached.contains_key("https://good.example/f.xml"));
    }
}
