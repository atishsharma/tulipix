//! Books engine — the data + format layer behind the Books section rebuild
//! (docs/books-redesign/). No UI here: `tulipix-sec-books` wires these
//! functions to the Slint components.
//!
//! Standalone `books.db` (no `items` FK — same model as podcasts/radio).
//! Formats: EPUB (reflow + paginate), PDF (raster pages via pdftoppm),
//! CBZ/CBR (comic page images), MOBI/AZW3 (text extract via `mobi`).

pub mod annotations;
pub mod covers;
pub mod epub;
pub mod home;
pub mod library;
pub mod metadata;
pub mod paginate;
pub mod prefs;
pub mod progress;
pub mod render;
pub mod scan;
pub mod schema;
pub mod toc;
pub mod tts;

/// Lower-case canonical format tag for a path, if it is a supported book.
pub fn format_of(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "epub" => Some("epub"),
        "pdf"  => Some("pdf"),
        "cbz"  => Some("cbz"),
        "cbr"  => Some("cbr"),
        "mobi" => Some("mobi"),
        "azw3" | "azw" => Some("azw3"),
        _ => None,
    }
}

/// Per-chapter plain text for a book, ready for [`paginate::paginate`].
///
/// EPUB → one entry per spine chapter; MOBI/AZW3 → a single flattened entry.
/// Image-based formats (PDF/CBZ/CBR) return empty — those need the raster
/// reader, not the reflow engine.
pub fn book_text(path: &std::path::Path, format: &str) -> Vec<String> {
    match format {
        "epub" => epub::load_chapters(path)
            .unwrap_or_default()
            .into_iter()
            .map(|c| c.text)
            .collect(),
        "mobi" | "azw3" => mobi::Mobi::from_path(path)
            .ok()
            .map(|m| vec![epub::html_to_text(&m.content_as_string_lossy())])
            .filter(|v| !v[0].trim().is_empty())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Per-chapter `(title, text)` for a book. Title is the chapter's heading
/// (EPUB `<title>`/`<h1>`/`<h2>`), empty when absent — the reader renders it as
/// a centered heading on the chapter's first page. Text is the same plain text
/// [`book_text`] returns.
pub fn book_chapters(path: &std::path::Path, format: &str) -> Vec<(String, String)> {
    match format {
        "epub" => epub::load_chapters(path)
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.title, c.text))
            .collect(),
        _ => book_text(path, format).into_iter().map(|t| (String::new(), t)).collect(),
    }
}

/// Books cover/page cache root: `~/.cache/Tulipix/books/`.
pub fn cache_dir() -> Option<std::path::PathBuf> {
    let d = tulipix_core::paths::cache_dir()?.join("books");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Short stable hash of a string (cache file keys).
pub fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(s.as_bytes());
    h.iter().take(8).map(|b| format!("{b:02x}")).collect()
}
