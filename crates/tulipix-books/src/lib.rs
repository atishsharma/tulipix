//! Books engine — the data + format layer behind the Books section rebuild
//! (docs/books-redesign/). No UI here: `tulipix-sec-books` wires these
//! functions to the Slint components.
//!
//! Standalone `books.db` (no `items` FK — same model as podcasts/radio).
//! Formats: EPUB (reflow + paginate), PDF (raster pages via pdftoppm),
//! CBZ/CBR (comic page images), MOBI/AZW3 (text extract via `mobi`).

pub mod annotations;
pub mod calibre;
pub mod comicinfo;
pub mod covers;
pub mod epub;
pub mod fb2;
pub mod home;
pub mod kokoro;
pub mod library;
pub mod lookup;
pub mod metadata;
pub mod paginate;
pub mod prefs;
pub mod progress;
pub mod render;
pub mod scan;
pub mod schema;
pub mod speech;
pub mod summary;
pub mod toc;
pub mod tts;

/// Deterministic cover hue for a book without artwork (hash of the title into
/// a fixed palette). Shared by the Rust placeholder-cover bake and the UI's
/// flat fallback so both always agree on a book's colour.
pub fn cover_hue_rgb(title: &str) -> [u8; 3] {
    const HUES: [[u8; 3]; 10] = [
        [108, 77, 246], [224, 81, 143], [47, 191, 113], [245, 166, 35], [58, 134, 255],
        [239, 71, 111], [17, 138, 178], [131, 56, 236], [255, 107, 107], [32, 201, 151],
    ];
    let h = title.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    HUES[h as usize % HUES.len()]
}

/// Lower-case canonical format tag for a path, if it is a supported book.
pub fn format_of(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "epub" => Some("epub"),
        "pdf"  => Some("pdf"),
        "djvu" | "djv" => Some("djvu"),
        "cbz"  => Some("cbz"),
        "cbr"  => Some("cbr"),
        "cb7"  => Some("cb7"),
        "cbt"  => Some("cbt"),
        "fb2"  => Some("fb2"),
        "mobi" => Some("mobi"),
        "azw3" | "azw" => Some("azw3"),
        // "<name>.fb2.zip" — the usual distribution form for FictionBook.
        "zip" => path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| s.to_ascii_lowercase().ends_with(".fb2"))
            .map(|_| "fb2"),
        _ => None,
    }
}

/// Is this a fixed-page (rasterised) format? Those take the image reader
/// rather than the reflow paginator.
pub fn is_fixed_page(format: &str) -> bool {
    matches!(format, "pdf" | "djvu" | "cbz" | "cbr" | "cb7" | "cbt")
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
        "fb2" => book_chapters(path, format).into_iter().map(|(_, t)| t).collect(),
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
        "fb2" => fb2::read_xml(path).map(|x| fb2::chapters(&x)).unwrap_or_default(),
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

/// Decode a standard-base64 string (whitespace ignored). Small enough to avoid
/// a base64 crate dependency — used for the embedded hero art and for FB2's
/// inline `<binary>` images.
pub(crate) fn b64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    out
}

/// Decode a base64 PNG into `(rgba8, width, height)` for `Image::from_rgba8`.
/// Returns `None` on any decode failure (the caller falls back to no art).
pub fn decode_png_b64(b64: &str) -> Option<(Vec<u8>, u32, u32)> {
    let bytes = b64_decode(b64);
    let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    Some((img.into_raw(), w, h))
}
