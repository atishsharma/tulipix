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

/// Extract a `name="…" content="…"` attribute from a single tag slice.
fn tag_attr(tag: &str, attr: &str) -> Option<String> {
    let key = format!("{attr}=");
    let p = tag.find(&key)?;
    let after = tag[p + key.len()..].trim_start();
    let q = after.chars().next()?;
    if q != '"' && q != '\'' { return None; }
    let rest = &after[1..];
    let e = rest.find(q)?;
    Some(rest[..e].to_string())
}

/// Calibre series metadata from an EPUB OPF:
/// `<meta name="calibre:series" content="…"/>` + `calibre:series_index`.
pub fn parse_calibre_series(opf_xml: &str) -> Option<(String, f64)> {
    let content_of = |name: &str| -> Option<String> {
        let mut idx = 0;
        while let Some(rel) = opf_xml[idx..].find("<meta") {
            let at = idx + rel;
            let end = opf_xml[at..].find('>').map(|e| at + e).unwrap_or(opf_xml.len());
            let tag = &opf_xml[at..end];
            if tag.contains(&format!("name=\"{name}\"")) || tag.contains(&format!("name='{name}'")) {
                if let Some(c) = tag_attr(tag, "content") { return Some(c); }
            }
            idx = end + 1;
        }
        None
    };
    let name = content_of("calibre:series")?;
    let name = name.trim();
    if name.is_empty() { return None; }
    let index = content_of("calibre:series_index")
        .and_then(|s| s.trim().parse::<f64>().ok()).unwrap_or(1.0);
    Some((name.to_string(), index))
}

/// Conservative series detection from a filename stem. Only fires on a clear
/// `#n` marker — either parenthetical (`Title (Ram Chandra #3)`) or trailing
/// (`Ram Chandra #3`) — so unrelated books are never grouped by accident.
pub fn detect_series_from_filename(stem: &str) -> Option<(String, f64)> {
    let tail = |s: &str| -> Option<(String, f64)> {
        let p = s.rfind('#')?;
        let after = s[p + 1..].trim();
        let tok: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let idx = tok.parse::<f64>().ok()?;
        let name = s[..p].trim().trim_end_matches([',', '-', '·', ':']).trim().to_string();
        if name.is_empty() { None } else { Some((name, idx)) }
    };
    let s = stem.trim();
    if let (Some(o), Some(c)) = (s.rfind('('), s.rfind(')')) {
        if o < c {
            if let Some(hit) = tail(s[o + 1..c].trim()) { return Some(hit); }
        }
    }
    tail(s)
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

    #[test]
    fn calibre_series_parse() {
        let opf = r#"<package><metadata>
            <meta name="calibre:series" content="Ram Chandra"/>
            <meta name="calibre:series_index" content="3"/>
        </metadata></package>"#;
        assert_eq!(parse_calibre_series(opf), Some(("Ram Chandra".to_string(), 3.0)));
        assert_eq!(parse_calibre_series("<package></package>"), None);
    }

    #[test]
    fn filename_series_detection() {
        assert_eq!(detect_series_from_filename("Scion of Ikshvaku (Ram Chandra #1)"),
            Some(("Ram Chandra".to_string(), 1.0)));
        assert_eq!(detect_series_from_filename("Ram Chandra #3"),
            Some(("Ram Chandra".to_string(), 3.0)));
        assert_eq!(detect_series_from_filename("A Fine Balance"), None);
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
