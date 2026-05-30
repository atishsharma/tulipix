//! `np.p4.books.metadata` — ComicVine / OpenLibrary / Google Books scraper.
//!
//! Builds the lookup URLs and parses each provider's response shape into a
//! common [`ScrapedMeta`]. Network IO is a thin reqwest call; URL construction
//! (ComicVine needs an API key + filter syntax; OpenLibrary keys by ISBN) and
//! response parsing are the testable, bug-prone parts.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScrapedMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub published: Option<i64>,
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

/// OpenLibrary lookup by ISBN (no key).
pub fn openlibrary_url(isbn: &str) -> String {
    format!("https://openlibrary.org/api/books?bibkeys=ISBN:{}&format=json&jscmd=data", enc(isbn))
}

/// Google Books volume search by title.
pub fn google_books_url(title: &str) -> String {
    format!("https://www.googleapis.com/books/v1/volumes?q={}&maxResults=5", enc(title))
}

/// ComicVine issue search (requires an API key).
pub fn comicvine_url(api_key: &str, query: &str) -> String {
    format!("https://comicvine.gamespot.com/api/search/?api_key={}&format=json&resources=issue&query={}", enc(api_key), enc(query))
}

#[derive(Debug, Deserialize)]
struct GbResponse { #[serde(default)] items: Vec<GbItem> }
#[derive(Debug, Deserialize)]
struct GbItem { #[serde(rename = "volumeInfo")] volume_info: GbVolume }
#[derive(Debug, Deserialize)]
struct GbVolume {
    #[serde(default)] title: Option<String>,
    #[serde(default)] authors: Vec<String>,
    #[serde(default)] description: Option<String>,
    #[serde(rename = "publishedDate", default)] published_date: Option<String>,
}

/// Parse the first Google Books volume into common metadata.
pub fn parse_google_books(json: &str) -> Option<ScrapedMeta> {
    let r: GbResponse = serde_json::from_str(json).ok()?;
    let v = r.items.into_iter().next()?.volume_info;
    Some(ScrapedMeta {
        title: v.title,
        author: if v.authors.is_empty() { None } else { Some(v.authors.join(", ")) },
        description: v.description,
        cover_url: None,
        published: v.published_date.as_deref().and_then(|d| d.get(0..4)).and_then(|y| y.parse().ok()),
    })
}

/// Apply scraped metadata to a book row (only overwriting empty fields).
pub async fn apply(pool: &SqlitePool, item_id: i64, m: &ScrapedMeta) -> Result<()> {
    sqlx::query(
        "UPDATE book_meta SET
            title = COALESCE(title, ?), author = COALESCE(author, ?),
            description = COALESCE(description, ?), published = COALESCE(published, ?)
         WHERE item_id = ?",
    ).bind(&m.title).bind(&m.author).bind(&m.description).bind(m.published).bind(item_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_book};

    #[test]
    fn url_builders() {
        assert!(openlibrary_url("978-0").contains("ISBN:978-0"));
        assert!(google_books_url("Dune Messiah").contains("Dune%20Messiah"));
        assert!(comicvine_url("KEY", "Saga").contains("api_key=KEY"));
    }

    #[test]
    fn parse_gb() {
        let json = r#"{"items":[{"volumeInfo":{"title":"Dune","authors":["Frank Herbert"],"description":"Sci-fi","publishedDate":"1965-08-01"}}]}"#;
        let m = parse_google_books(json).unwrap();
        assert_eq!(m.title.as_deref(), Some("Dune"));
        assert_eq!(m.author.as_deref(), Some("Frank Herbert"));
        assert_eq!(m.published, Some(1965));
    }

    #[tokio::test]
    async fn apply_fills_empty_only() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/d.epub", "epub", false).await;
        sqlx::query("UPDATE book_meta SET title = 'Existing' WHERE item_id = ?").bind(id).execute(&pool).await.unwrap();
        apply(&pool, id, &ScrapedMeta { title: Some("New".into()), author: Some("A".into()), ..Default::default() }).await.unwrap();
        let (title, author): (String, String) = sqlx::query_as("SELECT title, author FROM book_meta WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(title, "Existing"); // not overwritten
        assert_eq!(author, "A");       // filled
    }
}
