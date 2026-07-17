//! Online book metadata for the detail popup + tiles (R1/P6). Best-effort fetch
//! of description, publication date, and average rating by title + author,
//! cached in `books.db` by the caller (`library::set_metadata`) so it's shown
//! from disk next time. Key-less public APIs; each source fills whatever gaps
//! remain, and any failure is silently skipped (never an error).
//!
//! Sources (5-method chain, in order; first non-empty wins per field):
//!  1. Google Books   — description · publishedDate · averageRating
//!  2. Open Library   — description · first_publish_year · ratings average
//!  3. Apple iTunes   — description · releaseDate (ebook entity)
//!  4. Wikipedia REST — description (extract) fallback
//!  5. Local file metadata — `published` (merged in by the caller from
//!     `metadata::extract`, so it's the last resort for the date).

use serde_json::Value;

/// Merged online metadata. Empty strings / 0 = not found by any source.
#[derive(Debug, Default, Clone)]
pub struct OnlineMeta {
    pub summary: String,
    pub published: String, // year or ISO-ish date, as the source gives it
    pub rating: f64,       // average rating 0‥5 (0 = unknown)
}

impl OnlineMeta {
    fn is_complete(&self) -> bool {
        !self.summary.is_empty() && !self.published.is_empty() && self.rating > 0.0
    }
    /// Fill only the still-empty fields from `other`.
    fn merge(&mut self, other: OnlineMeta) {
        if self.summary.is_empty() {
            self.summary = other.summary;
        }
        if self.published.is_empty() {
            self.published = other.published;
        }
        if self.rating <= 0.0 {
            self.rating = other.rating;
        }
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Tulipix/1.0 (books reader)")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default()
}

/// Back-compat: summary-only fetch (detail popup's old entry point).
pub async fn fetch(title: &str, author: &str) -> Option<String> {
    let m = fetch_meta(title, author).await;
    (!m.summary.is_empty()).then_some(m.summary)
}

/// Full metadata fetch across the 5-source chain, short-circuiting once every
/// field is filled.
pub async fn fetch_meta(title: &str, author: &str) -> OnlineMeta {
    let title = title.trim();
    let mut out = OnlineMeta::default();
    if title.is_empty() {
        return out;
    }
    let c = client();
    if let Some(m) = google_books(&c, title, author).await {
        out.merge(m);
    }
    if out.is_complete() {
        return out;
    }
    if let Some(m) = open_library(&c, title, author).await {
        out.merge(m);
    }
    if out.is_complete() {
        return out;
    }
    if let Some(m) = itunes(&c, title, author).await {
        out.merge(m);
    }
    if out.summary.is_empty() {
        if let Some(s) = wikipedia(&c, title).await {
            out.summary = s;
        }
    }
    out
}

async fn google_books(c: &reqwest::Client, title: &str, author: &str) -> Option<OnlineMeta> {
    let mut q = format!("intitle:{title}");
    if !author.trim().is_empty() {
        q.push_str(&format!("+inauthor:{author}"));
    }
    let url = format!(
        "https://www.googleapis.com/books/v1/volumes?q={}&maxResults=1",
        urlencoding::encode(&q)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let info = v.get("items")?.get(0)?.get("volumeInfo")?;
    Some(OnlineMeta {
        summary: info.get("description").and_then(|d| d.as_str()).unwrap_or("").trim().to_string(),
        published: info.get("publishedDate").and_then(|d| d.as_str()).unwrap_or("").to_string(),
        rating: info.get("averageRating").and_then(|r| r.as_f64()).unwrap_or(0.0),
    })
}

async fn open_library(c: &reqwest::Client, title: &str, author: &str) -> Option<OnlineMeta> {
    let mut url = format!(
        "https://openlibrary.org/search.json?title={}&limit=1&fields=key,first_publish_year,ratings_average",
        urlencoding::encode(title)
    );
    if !author.trim().is_empty() {
        url.push_str(&format!("&author={}", urlencoding::encode(author)));
    }
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let doc = v.get("docs")?.get(0)?;
    let key = doc.get("key")?.as_str()?; // "/works/OL…W"
    let published = doc
        .get("first_publish_year")
        .and_then(|y| y.as_i64())
        .map(|y| y.to_string())
        .unwrap_or_default();
    let rating = doc.get("ratings_average").and_then(|r| r.as_f64()).unwrap_or(0.0);
    // Work description (string or {value}).
    let mut summary = String::new();
    if let Ok(resp) = c.get(format!("https://openlibrary.org{key}.json")).send().await {
        if let Ok(work) = resp.json::<Value>().await {
            summary = match work.get("description") {
                Some(Value::String(s)) => s.trim().to_string(),
                Some(v) => v.get("value").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
                None => String::new(),
            };
        }
    }
    Some(OnlineMeta { summary, published, rating })
}

async fn itunes(c: &reqwest::Client, title: &str, author: &str) -> Option<OnlineMeta> {
    let term = if author.trim().is_empty() { title.to_string() } else { format!("{title} {author}") };
    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=ebook&limit=1",
        urlencoding::encode(&term)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let r = v.get("results")?.get(0)?;
    let published = r
        .get("releaseDate")
        .and_then(|d| d.as_str())
        .map(|d| d.chars().take(10).collect()) // ISO date → yyyy-mm-dd
        .unwrap_or_default();
    let summary = r.get("description").and_then(|d| d.as_str()).unwrap_or("").trim().to_string();
    // iTunes descriptions are HTML-ish; strip the common tags cheaply.
    let summary = summary.replace("<br>", "\n").replace("<br />", "\n");
    let summary = strip_tags(&summary);
    Some(OnlineMeta { summary, published, rating: 0.0 })
}

async fn wikipedia(c: &reqwest::Client, title: &str) -> Option<String> {
    let url = format!(
        "https://en.wikipedia.org/api/rest_v1/page/summary/{}",
        urlencoding::encode(title)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let s = v.get("extract")?.as_str()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Cheap HTML tag stripper (iTunes descriptions).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}
