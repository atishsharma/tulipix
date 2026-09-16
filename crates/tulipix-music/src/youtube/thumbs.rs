//! Fetch remote YouTube/Piped thumbnails + channel avatars into a per-id cache
//! file. The cache key is a hash of the remote URL, so the same image is fetched
//! once.
//!
//! **JPEG, not PNG.** These are photographic — video stills and album art — and
//! PNG stores them losslessly for no benefit: a 1024² frame is 1–2 MB as PNG
//! against ~120 KB as JPEG at [`QUALITY`]. Not webp, because the in-app decode
//! path is unreliable on it. Note this is a *disk* win only; the decoded RGBA
//! a thumbnail occupies in memory is identical whatever the file format.
//!
//! Everything here is capped at [`MAX_EDGE`]. It did not used to be: whatever
//! resolution the remote served was decoded and re-encoded verbatim, and
//! YouTube Music serves square album art up to 4153×4153. One such thumbnail
//! landed on disk as a 28 MB PNG — larger than the JPEG it came from — and cost
//! 69 MB of RGBA the moment anything decoded it. A cache of 313 of them measured
//! 445 MB on disk and **3,472 MB decoded**, which is where the app's resident
//! memory was going.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Longest edge, in pixels, that any cached thumbnail is allowed to keep.
///
/// 1024 covers the largest cell any layout draws (and a HiDPI backing scale on
/// top of it) while bounding one decoded image at 1024×1024×4 = 4 MB.
pub const MAX_EDGE: u32 = 1024;

/// JPEG quality for cache writes. 85 is the usual "no visible artefacts on
/// photographic content" point; these are never re-encoded from, so generation
/// loss is not a concern.
pub const QUALITY: u8 = 85;

/// Shrink to fit `MAX_EDGE` on the long edge, preserving aspect. Images already
/// within the cap are returned untouched.
fn clamp_edge(img: image::DynamicImage) -> image::DynamicImage {
    if img.width().max(img.height()) <= MAX_EDGE {
        return img;
    }
    // Lanczos3: these are album covers and channel avatars sitting still on a
    // page, so the sharper filter is worth it — this runs once per image, ever.
    img.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Lanczos3)
}

/// Write `img` as JPEG to `path`, via a temp file and a rename so a crash
/// mid-write leaves the previous file rather than a truncated one.
///
/// Flattened to RGB first: the JPEG encoder rejects an alpha channel outright,
/// and these images have nothing meaningful in one.
fn write_jpeg(img: &image::DynamicImage, path: &Path) -> Result<()> {
    let tmp = path.with_extension("jpg.tmp");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let rgb = img.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, QUALITY)
            .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Stable per-URL cache file inside `dir`, always `.jpg`.
pub fn cache_path(dir: &Path, url: &str) -> PathBuf {
    // FNV-1a 64-bit — no extra deps, good enough for a filename key.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{h:016x}.jpg"))
}

/// The path a build before the JPEG switch would have written for the same URL.
fn legacy_path(p: &Path) -> PathBuf {
    p.with_extension("png")
}

/// The file that actually holds `path`'s image, tolerating the extension change.
///
/// `youtube.db` stores thumbnail *paths* — `cached_videos.thumb_path`, the subs
/// table's `avatar_path` — so rows written before the switch point at `.png`
/// files, and rows written after point at `.jpg`. Neither is refetched on a
/// miss; a wrong extension is a permanently blank tile. Try the other one before
/// giving up, and the format change needs no database migration.
pub fn resolve(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    let flipped = match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") => path.with_extension("png"),
        Some("png") => path.with_extension("jpg"),
        _ => return path.to_path_buf(),
    };
    if flipped.exists() { flipped } else { path.to_path_buf() }
}

/// Fetch `url` into the cache if it isn't there already, and return the cached
/// path. Errors on fetch/decode failure; the caller decides whether to fall back
/// to a placeholder glyph.
pub async fn fetch_thumb(client: &reqwest::Client, dir: &Path, url: &str) -> Result<String> {
    let out = cache_path(dir, url);
    if out.exists() {
        return Ok(out.to_string_lossy().into_owned());
    }
    // A PNG left by an older build is the same image under a different name —
    // transcode it rather than going back to the network.
    let legacy = legacy_path(&out);
    if legacy.exists() {
        if let Ok(img) = image::open(&legacy) {
            if write_jpeg(&clamp_edge(img), &out).is_ok() {
                let _ = std::fs::remove_file(&legacy);
                return Ok(out.to_string_lossy().into_owned());
            }
        }
    }
    std::fs::create_dir_all(dir)?;
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    write_jpeg(&clamp_edge(image::load_from_memory(&bytes)?), &out)?;
    Ok(out.to_string_lossy().into_owned())
}

/// Open a cached thumbnail, rewriting the file if it predates [`MAX_EDGE`] or
/// the JPEG switch.
///
/// `fetch_thumb` returns early when its cache file exists, so an oversized entry
/// written by an older build would otherwise never be replaced — the disk cost
/// and the decode cost would both stay. Fixing it on first read costs no network
/// round trip. A rewritten `.png` is replaced by its `.jpg` twin, which
/// [`resolve`] keeps reachable from the path still stored in the database.
pub fn open_capped(path: &Path) -> Result<image::DynamicImage> {
    let found = resolve(path);
    let img = image::open(&found)?;
    let is_png = found.extension().and_then(|e| e.to_str()) == Some("png");
    if img.width().max(img.height()) <= MAX_EDGE && !is_png {
        return Ok(img);
    }
    let small = clamp_edge(img);
    let jpg = found.with_extension("jpg");
    if write_jpeg(&small, &jpg).is_ok() && is_png {
        let _ = std::fs::remove_file(&found);
    }
    Ok(small)
}

/// YouTube's own still for a video, for rows stored without a thumbnail (an
/// imported playlist, a cache entry written before its metadata was known).
pub fn video_thumb_url(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
}

/// A channel listing's avatar and banner URLs, from the `thumbnails` yt-dlp
/// puts on a channel tab. Either may be missing (a channel with no banner).
pub fn channel_art(j: &serde_json::Value) -> (Option<String>, Option<String>) {
    let thumbs = j["thumbnails"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let url = |t: &serde_json::Value| t["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    let by_id = |id: &str| thumbs.iter().find(|t| t["id"].as_str() == Some(id)).and_then(url);
    let size = |t: &serde_json::Value| (t["width"].as_u64().unwrap_or(0), t["height"].as_u64().unwrap_or(0));
    // The sized copies: square ones are avatars, wide ones banners.
    let largest = |wide: bool| {
        thumbs
            .iter()
            .filter(|t| {
                let (w, h) = size(t);
                h > 0 && if wide { w > h * 3 } else { w == h }
            })
            .max_by_key(|t| size(t).0)
            .and_then(url)
    };
    (
        by_id("avatar_uncropped").or_else(|| largest(false)),
        by_id("banner_uncropped").or_else(|| largest(true)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_stable_jpeg() {
        let dir = Path::new("/tmp/yt");
        let a = cache_path(dir, "https://t/abc.jpg");
        let b = cache_path(dir, "https://t/abc.jpg");
        let c = cache_path(dir, "https://t/other.jpg");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.extension().unwrap() == "jpg");
        assert!(a.starts_with(dir));
    }

    /// A path with no file on disk resolves to itself; a `.jpg` whose only file
    /// is the legacy `.png` resolves across. This is what keeps thumbnail paths
    /// already stored in youtube.db working after the format switch.
    #[test]
    fn resolve_falls_back_across_the_extension() {
        let dir = std::env::temp_dir().join("tulipix-thumb-resolve-test");
        let _ = std::fs::create_dir_all(&dir);
        let jpg = dir.join("aaaa.jpg");
        let png = dir.join("aaaa.png");
        let _ = std::fs::remove_file(&jpg);
        let _ = std::fs::remove_file(&png);

        assert_eq!(resolve(&jpg), jpg, "nothing on disk → unchanged");

        std::fs::write(&png, b"x").unwrap();
        assert_eq!(resolve(&jpg), png, "only the legacy file exists → use it");

        std::fs::write(&jpg, b"x").unwrap();
        assert_eq!(resolve(&jpg), jpg, "both exist → prefer the canonical one");

        let _ = std::fs::remove_file(&jpg);
        let _ = std::fs::remove_file(&png);
    }

    #[test]
    fn channel_art_prefers_the_uncropped_originals() {
        let j = serde_json::json!({"thumbnails": [
            {"url": "https://b/1060", "width": 1060, "height": 175, "id": "0"},
            {"url": "https://b/2560", "width": 2560, "height": 424, "id": "1"},
            {"url": "https://b/orig", "id": "banner_uncropped"},
            {"url": "https://a/900", "width": 900, "height": 900, "id": "7"},
            {"url": "https://a/orig", "id": "avatar_uncropped"}
        ]});
        assert_eq!(channel_art(&j), (Some("https://a/orig".into()), Some("https://b/orig".into())));

        let sized = serde_json::json!({"thumbnails": [
            {"url": "https://b/1060", "width": 1060, "height": 175, "id": "0"},
            {"url": "https://b/2560", "width": 2560, "height": 424, "id": "1"},
            {"url": "https://a/88", "width": 88, "height": 88, "id": "6"},
            {"url": "https://a/900", "width": 900, "height": 900, "id": "7"}
        ]});
        assert_eq!(channel_art(&sized), (Some("https://a/900".into()), Some("https://b/2560".into())));
        assert_eq!(channel_art(&serde_json::json!({})), (None, None));
    }
}
