//! `np.p4.books.scan` — EPUB / CBZ / CBR / PDF / MOBI / AZW3 / FB2 scan +
//! metadata extraction.
//!
//! Detects the format from the extension, pulls baseline metadata (EPUB OPF
//! parse for title/author/language; comics get series/issue from the filename)
//! and writes a `book_meta` row. Page-image extraction for CBZ/CBR/PDF is done
//! lazily by the reader; this records the format + identity.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format { Epub, Cbz, Cbr, Pdf, Mobi, Azw3, Fb2, Djvu }

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Epub => "epub", Format::Cbz => "cbz", Format::Cbr => "cbr",
            Format::Pdf => "pdf", Format::Mobi => "mobi", Format::Azw3 => "azw3",
            Format::Fb2 => "fb2", Format::Djvu => "djvu",
        }
    }
    pub fn is_comic(self) -> bool { matches!(self, Format::Cbz | Format::Cbr) }
    /// Formats the reader shows as reflowable text (np.p5.books.formats/.pdf).
    /// DjVu is raster-only — it pages like a comic, never reflows.
    pub fn is_text(self) -> bool {
        matches!(self, Format::Epub | Format::Pdf | Format::Mobi | Format::Azw3 | Format::Fb2)
    }
}

pub fn format_from_ext(path: &str) -> Option<Format> {
    // `.fb2.zip` is the common FB2 distribution wrapper.
    if path.to_ascii_lowercase().ends_with(".fb2.zip") {
        return Some(Format::Fb2);
    }
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "epub" => Format::Epub,
        "cbz"  => Format::Cbz,
        "cbr"  => Format::Cbr,
        "pdf"  => Format::Pdf,
        "mobi" => Format::Mobi,
        "azw3" | "azw" => Format::Azw3,
        "fb2"  => Format::Fb2,
        "djvu" | "djv" => Format::Djvu,
        _ => return None,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BookMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
}

/// Minimal EPUB OPF (`content.opf`) parse: `<dc:title>`, `<dc:creator>`,
/// `<dc:language>`. Enough to populate the library without a full EPUB crate.
pub fn parse_opf(opf_xml: &str) -> BookMeta {
    let dc = |tag: &str| -> Option<String> {
        let open = format!("<dc:{tag}");
        let s = opf_xml.find(&open)?;
        let after = &opf_xml[s..];
        let gt = after.find('>')? + 1;
        let close = after[gt..].find("</")? + gt;
        let v = after[gt..close].trim();
        if v.is_empty() { None } else { Some(v.to_string()) }
    };
    BookMeta { title: dc("title"), author: dc("creator"), language: dc("language") }
}

pub async fn upsert(pool: &SqlitePool, item_id: i64, fmt: Format, meta: &BookMeta) -> Result<()> {
    sqlx::query(
        "INSERT INTO book_meta (item_id, format, title, author, language, is_comic) VALUES (?,?,?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET format=excluded.format, title=excluded.title,
            author=excluded.author, language=excluded.language, is_comic=excluded.is_comic",
    ).bind(item_id).bind(fmt.as_str()).bind(&meta.title).bind(&meta.author).bind(&meta.language)
     .bind(fmt.is_comic() as i64).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn ext_detection() {
        assert_eq!(format_from_ext("/b/x.EPUB"), Some(Format::Epub));
        assert!(format_from_ext("/b/x.cbz").unwrap().is_comic());
        assert_eq!(format_from_ext("/b/x.txt"), None);
    }

    #[test]
    fn opf_parse() {
        let opf = r#"<package><metadata>
            <dc:title>The Title</dc:title>
            <dc:creator opf:role="aut">Jane Doe</dc:creator>
            <dc:language>en</dc:language>
        </metadata></package>"#;
        let m = parse_opf(opf);
        assert_eq!(m.title.as_deref(), Some("The Title"));
        assert_eq!(m.author.as_deref(), Some("Jane Doe"));
        assert_eq!(m.language.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn upsert_sets_comic_flag() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/b/c.cbz', 0, 1, 0, 'books', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/b/c.cbz'").fetch_one(&pool).await.unwrap();
        upsert(&pool, id, Format::Cbz, &BookMeta::default()).await.unwrap();
        let comic: i64 = sqlx::query_scalar("SELECT is_comic FROM book_meta WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(comic, 1);
    }
}
