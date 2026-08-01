//! Finding, fetching and caching cover art.
//!
//! Ported from Tomesole (MIT). Search results carry no cover, so a cover costs
//! a request for the record page followed by a request for the image itself.
//! That is two round trips per book, so every answer — including "this book has
//! no cover" — is cached on disk and never asked for twice.
//!
//! The URL of the image comes out of a page served by a mirror, which makes it
//! attacker-chosen. It is fetched through the same validated client as
//! everything else, so the SSRF guard applies, and then:
//!
//! * the image must live on the same host as the mirror it came from;
//! * the response has to be small — a cover is tens of kilobytes;
//! * it has to actually start with an image signature, so an HTML captcha page
//!   or a disguised payload is discarded rather than cached;
//! * nothing is written outside the cache directory, and filenames come from
//!   the MD5, never from the URL.
//!
//! Where upstream decodes JPEG itself for terminal drawing, this returns the
//! path of the cached file: Slint loads and decodes images, so pulling in a
//! decoder here would be redundant.

use std::path::{Path, PathBuf};

use ureq::http::Uri;

use crate::error::{Context, Result};
use crate::model::{Book, is_md5};
use crate::net::{Http, join_uri};
use crate::{bail, err, html, libgen, net};

/// Covers are small. Anything larger is not a thumbnail and is not wanted.
const MAX_COVER_BYTES: u64 = 4 * 1024 * 1024;

/// A zero-byte file recording that a book has no cover, so the record page is
/// not fetched again on every visit.
const MISSING: &str = "none";

/// The image formats we are willing to keep, identified by signature rather
/// than by what the mirror claims in `Content-Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Png,
    /// GIF or WebP — Slint reads both.
    Other,
}

impl Kind {
    fn extension(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpg",
            Kind::Png => "png",
            Kind::Other => "img",
        }
    }

    /// Identify image bytes, rejecting anything that is not an image at all —
    /// an error page, a captcha, or something pretending to be a picture.
    fn classify(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Kind::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Kind::Png)
        } else if bytes.starts_with(b"GIF8")
            || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()))
        {
            Some(Kind::Other)
        } else {
            None
        }
    }
}

/// Where cached covers live.
pub fn cache_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("genesis-covers"))
}

/// Where a cover for this MD5 lives, or `None` when the key is not one.
///
/// Every caller reaches here with an MD5 that came from a mirror or a history
/// row, and this is the only place in the module that turns one into a path, so
/// the check belongs here rather than at each entry point.
fn cache_path(md5: &str, extension: &str) -> Option<PathBuf> {
    if !is_md5(md5) {
        return None;
    }
    cache_dir().map(|d| d.join(format!("{md5}.{extension}")))
}

/// The cover already on disk for this MD5, without going near the network.
///
/// `Some(None)` means "looked before, there is none"; `None` means "not cached,
/// go and look".
pub fn cached(md5: &str) -> Option<Option<PathBuf>> {
    if cache_path(md5, MISSING).is_some_and(|p| p.exists()) {
        return Some(None);
    }
    for extension in ["jpg", "png", "img"] {
        if let Some(path) = cache_path(md5, extension)
            && path.exists()
            && std::fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false)
        {
            return Some(Some(path));
        }
    }
    None
}

/// Fetch a book's cover, from the cache when possible.
///
/// `Ok(None)` means the lookup succeeded and the book simply has no cover;
/// that answer is cached too.
pub fn fetch(http: &Http, base: &Uri, book: &Book) -> Result<Option<PathBuf>> {
    if !is_md5(&book.md5) {
        bail!("a cover needs a valid MD5");
    }
    if let Some(cached) = cached(&book.md5) {
        return Ok(cached);
    }

    let found = match find_url(http, base, book)? {
        // A record page has no business pointing its cover at another host,
        // and the one reason it would is to have us fetch something from
        // somewhere else on its behalf.
        Some((url, from)) if same_host(base, &url) => download(http, &url, &from)?,
        Some(_) | None => None,
    };
    Ok(store(&book.md5, found))
}

/// Locate the cover image on a record page, and say which page it was on.
///
/// `file.php` is the record page proper. `ads.php` also carries the cover but
/// mints a single-use download key as a side effect, so it is only the
/// fallback, for records whose file id the search row did not carry.
pub fn record_url(base: &Uri, book: &Book) -> Result<Uri> {
    match &book.file_id {
        Some(id) if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) => {
            join_uri(base, &format!("file.php?id={id}"))
        }
        _ => libgen::ads_url(base, &book.md5),
    }
}

fn find_url(http: &Http, base: &Uri, book: &Book) -> Result<Option<(Uri, Uri)>> {
    let page_url = record_url(base, book)?;
    let page = http.get_text(&page_url)?;
    let Some(src) = pick_cover_src(&page) else {
        return Ok(None);
    };
    Ok(Some((join_uri(base, &src)?, page_url)))
}

/// Choose the cover out of every image on a record page.
///
/// Libgen pages are full of icons, flags and spacers. The cover is the one
/// under a `covers/` path; failing that, an image whose name says so.
pub fn pick_cover_src(page: &str) -> Option<String> {
    let images = html::img_srcs(page);

    let looks_like_cover = |src: &&String| {
        let lower = src.to_lowercase();
        lower.contains("/covers/") || lower.contains("cover")
    };
    let is_image_file = |src: &&String| {
        let lower = src.to_lowercase();
        [".jpg", ".jpeg", ".png", ".gif", ".webp"].iter().any(|ext| lower.contains(ext))
    };

    images
        .iter()
        .find(|src| looks_like_cover(src) && is_image_file(src))
        .or_else(|| images.iter().find(looks_like_cover))
        .cloned()
}

/// Fetch the image itself, refusing anything that is not one.
///
/// `from` is the record page the image was named on. Mirrors hotlink-protect
/// covers and answer a request without a `Referer` with a zero-byte file.
fn download(http: &Http, url: &Uri, from: &Uri) -> Result<Option<(Kind, Vec<u8>)>> {
    let response = http.get_referred(url, from)?;
    if !(200..300).contains(&response.status) {
        return Ok(None);
    }
    // The header is the mirror's to choose, so it is a reason to stop early,
    // never a reason to trust what follows.
    if let Some(length) = response.content_length
        && length > MAX_COVER_BYTES
    {
        return Ok(None);
    }
    if response.content_type.as_deref().is_some_and(|ct| ct.to_ascii_lowercase().contains("text/html"))
    {
        return Ok(None);
    }

    let bytes = response
        .body
        .into_with_config()
        .limit(MAX_COVER_BYTES)
        .read_to_vec()
        .with_context(|| format!("could not read cover response body from {url}"))?;

    Ok(Kind::classify(&bytes).map(|kind| (kind, bytes)))
}

/// Remember a lookup. Best effort: a cover is not worth an error.
fn store(md5: &str, found: Option<(Kind, Vec<u8>)>) -> Option<PathBuf> {
    let dir = cache_dir()?;
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    match found {
        Some((kind, bytes)) => {
            let path = cache_path(md5, kind.extension())?;
            std::fs::write(&path, &bytes).ok()?;
            Some(path)
        }
        None => {
            if let Some(path) = cache_path(md5, MISSING) {
                let _ = std::fs::write(path, b"");
            }
            None
        }
    }
}

/// Reject an image URL that points somewhere other than the mirror it came
/// from, which is the only reason a record page would name another host.
pub fn same_host(base: &Uri, image: &Uri) -> bool {
    match (net::host_of(base), net::host_of(image)) {
        (Ok(a), Ok(b)) => a.eq_ignore_ascii_case(&b),
        _ => false,
    }
}

/// Empty the cover cache.
///
/// Cleared alongside the download history: which covers have been fetched is a
/// record of what someone has been looking at, and "forget my downloads"
/// plainly means that too.
pub fn clear_cache() -> Result<()> {
    let Some(dir) = cache_dir() else { return Ok(()) };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(err!("could not clear {}: {e}", dir.display())),
    }
}

/// How much of the cover cache is on disk.
pub fn cache_size() -> (usize, u64) {
    let Some(dir) = cache_dir() else { return (0, 0) };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut count = 0;
    let mut bytes = 0;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            count += 1;
            bytes += meta.len();
        }
    }
    (count, bytes)
}

/// True when `path` is inside the cover cache — the one thing the UI is allowed
/// to load from here.
pub fn is_cached_path(path: &Path) -> bool {
    cache_dir().is_some_and(|dir| path.starts_with(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_real_image_signatures_and_rejects_the_rest() {
        assert_eq!(Kind::classify(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Kind::Jpeg));
        assert_eq!(Kind::classify(b"\x89PNG\r\n\x1a\n\0"), Some(Kind::Png));
        assert_eq!(Kind::classify(b"GIF89a"), Some(Kind::Other));
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.push(0);
        assert_eq!(Kind::classify(&webp), Some(Kind::Other));
        // An HTML error page served with an image content-type is not a cover.
        assert_eq!(Kind::classify(b"<!DOCTYPE html><html>"), None);
        assert_eq!(Kind::classify(b""), None);
    }

    #[test]
    fn picks_the_cover_out_of_a_page_full_of_icons() {
        let page = r#"
            <img src="/img/flag_en.png">
            <img src="/static/spacer.gif">
            <img src="/covers/1234/abcdef.jpg">
            <img src="/img/logo.png">
        "#;
        assert_eq!(pick_cover_src(page).as_deref(), Some("/covers/1234/abcdef.jpg"));
    }

    #[test]
    fn a_page_with_no_cover_says_so() {
        assert_eq!(pick_cover_src(r#"<img src="/img/logo.png">"#), None);
    }

    #[test]
    fn cover_paths_are_keyed_on_the_md5_never_on_the_url() {
        // A key that is not an MD5 yields no path at all, so nothing can be
        // written outside the cache directory.
        assert!(cache_path("../../etc/passwd", "jpg").is_none());
        assert!(cache_path("", "jpg").is_none());
        let ok = cache_path("1b9159991f7fb1b3910c0be9ebf7e595", "jpg");
        assert!(ok.is_some_and(|p| p.ends_with("1b9159991f7fb1b3910c0be9ebf7e595.jpg")));
    }

    #[test]
    fn a_cover_hosted_off_the_mirror_is_refused() {
        let base: Uri = "https://libgen.li/".parse().unwrap();
        assert!(same_host(&base, &"https://libgen.li/covers/a.jpg".parse().unwrap()));
        assert!(!same_host(&base, &"https://evil.example/covers/a.jpg".parse().unwrap()));
    }
}
