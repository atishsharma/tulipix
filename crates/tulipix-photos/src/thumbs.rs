//! Async thumbnail pipeline — 256/512/1024 JPEG. HEIC/RAW dispatch to the
//! bundled ffmpeg build (which is configured with libheif + libraw); plain
//! formats (JPEG/PNG/WebP/BMP/TIFF/GIF) decode in-process via the `image`
//! crate so cold cases don't pay the subprocess cost.

use anyhow::{Context, Result};
use image::{imageops::FilterType, ImageFormat};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tulipix_core::paths;
use tulipix_core::thumbs::key;

/// Standard thumb sizes used by the timeline + viewer. Order matters — render
/// progressively from smallest to largest so the UI can paint as each tier
/// lands.
pub const SIZES: &[u32] = &[256, 512, 1024];

pub fn cache_dir() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("thumbs").join("photos"))
}

pub fn thumb_path(src: &Path, mtime: i64, size: u64, dim: u32) -> Option<PathBuf> {
    let k = key(src, mtime, size);
    cache_dir().map(|d| d.join(format!("{k}-{dim}.jpg")))
}

/// True if the decoder for `ext` is in-process. The pipeline falls back to
/// `ffmpeg` for everything else.
///
/// Delegates to `tulipix_core::thumbs`, which owns the thumbnail cache and
/// makes this same choice on the live path — two copies of the table would
/// eventually disagree about one extension, and the two answers would be a
/// silent behaviour difference rather than a build error.
pub fn is_native_decode(ext: &str) -> bool {
    matches!(tulipix_core::thumbs::photo_decoder(ext), tulipix_core::thumbs::PhotoDecoder::InProcess)
}

pub fn needs_ffmpeg_decode(ext: &str) -> bool {
    // Not simply `!is_native_decode`: this answers "is this a photo ffmpeg must
    // handle", so extensions that are not photos at all answer false.
    matches!(tulipix_core::thumbs::kind_for(ext), tulipix_core::thumbs::ThumbKind::Photo)
        && !is_native_decode(ext)
}

#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    pub written: u32,
    pub skipped_cached: u32,
}

/// Render every standard size for one photo. Idempotent — sizes that already
/// exist on disk are skipped.
pub fn render_all(src: &Path) -> Result<RenderStats> {
    let meta = std::fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    let mtime = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len();
    let mut stats = RenderStats::default();
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

    let out_dir = cache_dir().context("no cache dir")?;
    std::fs::create_dir_all(&out_dir)?;

    // Decode once at the biggest tier, then progressively scale down.
    let largest = SIZES.iter().copied().max().unwrap_or(1024);
    let largest_path = thumb_path(src, mtime, size, largest).context("no path")?;
    if !largest_path.exists() {
        if is_native_decode(&ext) {
            decode_native_to_jpeg(src, &largest_path, largest)?;
        } else if needs_ffmpeg_decode(&ext) {
            decode_via_ffmpeg(src, &largest_path, largest)?;
        } else {
            anyhow::bail!("unsupported extension: {ext}");
        }
        stats.written += 1;
    } else {
        stats.skipped_cached += 1;
    }

    // Smaller sizes derive from the cached largest tier — much cheaper than
    // re-decoding the original for each.
    let mut largest_img: Option<image::DynamicImage> = None;
    for &dim in SIZES.iter().filter(|d| **d < largest) {
        let p = thumb_path(src, mtime, size, dim).context("no path")?;
        if p.exists() { stats.skipped_cached += 1; continue; }
        let big = match largest_img.as_ref() {
            Some(img) => img.clone(),
            None => {
                let img = image::open(&largest_path)?;
                largest_img = Some(img.clone());
                img
            }
        };
        let scaled = big.resize(dim, dim, FilterType::Triangle);
        scaled.save_with_format(&p, ImageFormat::Jpeg)?;
        stats.written += 1;
    }
    Ok(stats)
}

fn decode_native_to_jpeg(src: &Path, out: &Path, max_dim: u32) -> Result<()> {
    let img = image::open(src).with_context(|| format!("decode {}", src.display()))?;
    let scaled = img.resize(max_dim, max_dim, FilterType::Triangle);
    if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
    scaled.save_with_format(out, ImageFormat::Jpeg)?;
    Ok(())
}

fn decode_via_ffmpeg(src: &Path, out: &Path, max_dim: u32) -> Result<()> {
    if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
    let mut c = std::process::Command::new("ffmpeg");
    c.args([
        "-y", "-loglevel", "error", "-i",
    ]);
    c.arg(src);
    c.args([
        "-vf",
        &format!(
            "scale='min({d},iw)':'min({d},ih)':force_original_aspect_ratio=decrease",
            d = max_dim
        ),
        "-frames:v",
        "1",
    ]);
    c.arg(out);
    let s = c.status().context("spawn ffmpeg")?;
    if !s.success() { anyhow::bail!("ffmpeg exit {s}"); }
    Ok(())
}

/// Garbage-collect cached thumbs that don't reference a known item path.
/// `keep_paths` is the set of `(abs_path, mtime, size)` triples that should
/// stay. Everything else under `cache_dir/photos/` is deleted.
pub fn vacuum(keep: &HashSet<String>) -> Result<u64> {
    let Some(d) = cache_dir() else { return Ok(0); };
    if !d.exists() { return Ok(0); }
    let mut freed = 0u64;
    for entry in std::fs::read_dir(&d)? {
        let entry = entry?;
        let name = entry.file_name().into_string().unwrap_or_default();
        // Strip the size suffix to extract the cache key.
        let stem = name.split('-').next().unwrap_or("").to_string();
        if !keep.contains(&stem) {
            if let Ok(m) = entry.metadata() {
                freed += m.len();
            }
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(freed)
}

/// Convenience: collect cache-key set for every photo currently in `items`.
pub async fn live_keys(pool: &sqlx::SqlitePool) -> Result<HashSet<String>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT abs_path, mtime, size FROM items WHERE section='photos' AND missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(p, m, s)| key(Path::new(&p), m, s as u64))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ext_classification() {
        assert!(is_native_decode("jpg"));
        assert!(is_native_decode("PNG"));
        assert!(needs_ffmpeg_decode("HEIC"));
        assert!(needs_ffmpeg_decode("arw"));
        assert!(!is_native_decode("arw"));
        assert!(!needs_ffmpeg_decode("jpg"));
    }
    #[test]
    fn render_native_jpeg() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CACHE_HOME", tmp.path()); }
        // Build a 64x64 RGB JPEG in-process.
        let img = image::RgbImage::from_pixel(64, 64, image::Rgb([10, 20, 30]));
        let src = tmp.path().join("in.jpg");
        img.save(&src).unwrap();
        let s = render_all(&src).unwrap();
        assert!(s.written >= 1);
        // Re-running is idempotent.
        let s2 = render_all(&src).unwrap();
        assert_eq!(s2.written, 0);
        assert!(s2.skipped_cached >= 1);
    }
}
