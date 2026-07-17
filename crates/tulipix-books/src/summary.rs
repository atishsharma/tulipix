//! Online book summary for the detail popup (R1). Best-effort fetch of a
//! description by title + author, cached in `books.db` by the caller
//! (`library::set_summary`) so it's shown from disk next time. Key-less public
//! APIs; failure returns `None`, never an error.
//!
//! - Google Books (volumes search → volumeInfo.description)
//! - Open Library (search.json → works/<key>.json → description) as fallback

use serde_json::Value;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Tulipix/1.0 (books reader)")
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default()
}

/// Fetch a book description by title (+ optional author). Tries Google Books
/// first, then Open Library. Returns trimmed plain text.
pub async fn fetch(title: &str, author: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() {
        return None;
    }
    let c = client();
    if let Some(s) = google_books(&c, title, author).await {
        return Some(s);
    }
    open_library(&c, title, author).await
}

async fn google_books(c: &reqwest::Client, title: &str, author: &str) -> Option<String> {
    let mut q = format!("intitle:{title}");
    if !author.trim().is_empty() {
        q.push_str(&format!("+inauthor:{author}"));
    }
    let url = format!(
        "https://www.googleapis.com/books/v1/volumes?q={}&maxResults=1",
        urlencoding::encode(&q)
    );
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    let desc = v
        .get("items")?
        .get(0)?
        .get("volumeInfo")?
        .get("description")?
        .as_str()?
        .trim()
        .to_string();
    (!desc.is_empty()).then_some(desc)
}

async fn open_library(c: &reqwest::Client, title: &str, author: &str) -> Option<String> {
    let mut url = format!(
        "https://openlibrary.org/search.json?title={}&limit=1&fields=key",
        urlencoding::encode(title)
    );
    if !author.trim().is_empty() {
        url.push_str(&format!("&author={}", urlencoding::encode(author)));
    }
    let v: Value = c.get(url).send().await.ok()?.json().await.ok()?;
    // docs[0].key is a work key like "/works/OL12345W".
    let key = v.get("docs")?.get(0)?.get("key")?.as_str()?;
    let work: Value = c
        .get(format!("https://openlibrary.org{key}.json"))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    // description is either a string or { type, value }.
    let desc = match work.get("description")? {
        Value::String(s) => s.trim().to_string(),
        v => v.get("value")?.as_str()?.trim().to_string(),
    };
    (!desc.is_empty()).then_some(desc)
}
