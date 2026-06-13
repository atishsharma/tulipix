//! Fetch remote YouTube/Piped thumbnails + channel avatars and re-encode them as
//! PNG into a per-id cache file. PNG (not webp) because the in-app image decode
//! path is unreliable on webp. The cache key is a hash of the remote URL so the
//! same image is fetched once.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Stable per-URL cache file inside `dir`, always `.png`.
pub fn cache_path(dir: &Path, url: &str) -> PathBuf {
    // FNV-1a 64-bit — no extra deps, good enough for a filename key.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{h:016x}.png"))
}

/// Fetch `url` and write a PNG to the cache (if not already present). Returns the
/// cached path as a String, or an error if the fetch/decode fails. Caller decides
/// whether to fall back to a placeholder glyph.
pub async fn fetch_png(client: &reqwest::Client, dir: &Path, url: &str) -> Result<String> {
    let out = cache_path(dir, url);
    if out.exists() {
        return Ok(out.to_string_lossy().into_owned());
    }
    std::fs::create_dir_all(dir)?;
    let bytes = client.get(url).send().await?.error_for_status()?.bytes().await?;
    let img = image::load_from_memory(&bytes)?;
    img.save_with_format(&out, image::ImageFormat::Png)?;
    Ok(out.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_is_stable_png() {
        let dir = Path::new("/tmp/yt");
        let a = cache_path(dir, "https://t/abc.jpg");
        let b = cache_path(dir, "https://t/abc.jpg");
        let c = cache_path(dir, "https://t/other.jpg");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.extension().unwrap() == "png");
        assert!(a.starts_with(dir));
    }
}
