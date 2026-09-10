//! `np.p4.music.waveform` — the loudness envelope behind the seekbar.
//!
//! A flat progress line says how far through you are and nothing else. The
//! track's own shape says where the quiet intro ends, where the drop is, and
//! how long the outro runs — so you can land on the chorus instead of hunting
//! for it.
//!
//! Computed once and cached on disk. The decode is ffmpeg's, not ours: it is
//! already bundled and already resolved through `tool_bin` for thumbnails, and
//! a second audio decoder in this workspace would be a dependency earning
//! nothing. What comes back over the pipe is mono 16-bit PCM at a low sample
//! rate, which is all a 400-column envelope needs.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Columns in a stored envelope. Wide enough that a full-width seekbar on a
/// 4K display still has more than one sample per pixel-pair, small enough that
/// the whole thing is 400 bytes.
pub const BUCKETS: usize = 400;

/// Decode rate. The envelope is a peak-per-bucket, not a spectrum, so this only
/// has to be high enough that a bucket holds enough samples for the peak to be
/// meaningful: at 8 kHz a four-minute track gives ~4,800 samples per bucket.
pub const RATE: u32 = 8_000;

/// The rate the analysis pass asks for. Higher than the envelope needs, because
/// chroma has to see up to the fifth octave and 8 kHz puts Nyquist below it.
pub const ANALYSIS_RATE: u32 = 22_050;

/// Where envelopes live. One file per track, named by a digest of the path and
/// what the file was when we read it.
pub fn cache_dir() -> Option<PathBuf> {
    let d = tulipix_core::paths::cache_dir()?.join("music_waveform");
    std::fs::create_dir_all(&d).ok();
    Some(d)
}

/// Cache key: path, size and mtime together, so a re-rip or a re-tag at the
/// same path produces a different key rather than serving the old shape.
fn key_for(src: &Path) -> String {
    let meta = std::fs::metadata(src).ok();
    let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    use md5::Digest;
    let digest = md5::Md5::digest(format!("{}|{len}|{mtime}", src.to_string_lossy()).as_bytes());
    // Hex, because the value becomes a filename.
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Where one cached artefact for `src` lives. The envelope and the spectrogram
/// share a key and differ only by extension, so a re-rip invalidates both.
fn cache_path(src: &Path, ext: &str) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("{}.{ext}", key_for(src))))
}

/// The cached envelope for `src`, or `None` if it has not been computed.
pub fn cached(src: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(cache_path(src, "bin")?).ok()?;
    (bytes.len() == BUCKETS).then_some(bytes)
}

/// The cached spectrogram for `src`, or `None` on a miss.
///
/// Unlike the envelope this has no fixed length — it is `bands x columns` and
/// the column count follows the track's duration — so the only check available
/// is that it divides evenly into whole columns. A truncated write (a crash
/// mid-flush) fails that and is treated as a miss.
pub fn spec_cached(src: &Path, bands: usize) -> Option<Vec<u8>> {
    let bytes = std::fs::read(cache_path(src, "spec")?).ok()?;
    (bands > 0 && !bytes.is_empty() && bytes.len() % bands == 0).then_some(bytes)
}

/// Write a spectrogram computed by the analysis pass into the cache.
pub fn spec_put(src: &Path, spec: &[u8]) {
    if spec.is_empty() {
        return;
    }
    if let Some(p) = cache_path(src, "spec") {
        let _ = std::fs::write(p, spec);
    }
}

/// The envelope for `src`, computing and caching it on a miss.
///
/// Blocking: it waits on ffmpeg and reads its whole output. Callers on an async
/// runtime must put it on `spawn_blocking`, the same as every other decode in
/// this workspace.
pub fn peaks(src: &Path) -> Result<Vec<u8>> {
    if let Some(hit) = cached(src) {
        return Ok(hit);
    }
    let out = envelope(&pcm(src, RATE)?);
    if let Some(p) = cache_path(src, "bin") {
        // A failed write is a slower next open, not an error worth failing the
        // seekbar over.
        let _ = std::fs::write(p, &out);
    }
    Ok(out)
}

/// Write an envelope computed elsewhere into the cache.
///
/// The analysis pass decodes at its own rate and gets the envelope for free
/// from the same buffer; this lets it deposit the result rather than making
/// `peaks` run ffmpeg a second time over the same file.
pub fn put(src: &Path, env: &[u8]) {
    if env.len() != BUCKETS {
        return;
    }
    if let Some(p) = cache_path(src, "bin") {
        let _ = std::fs::write(p, env);
    }
}

/// Mono 16-bit PCM at `rate`, straight off ffmpeg.
///
/// Public because the analysis pass wants the samples themselves, not an
/// envelope: DR, tempo and key all read the same buffer, and decoding a track
/// three times to answer three questions about it would be the whole cost of
/// the scan repeated.
pub fn pcm(src: &Path, rate: u32) -> Result<Vec<i16>> {
    use std::process::{Command, Stdio};

    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let mut c = Command::new(ff);
    c.args(["-v", "quiet", "-nostdin", "-i"]);
    c.arg(src);
    c.args([
        "-vn", // cover art is a video stream; decoding it would be a picture in the pipe
        "-ac", "1",
        "-ar", &rate.to_string(),
        "-f", "s16le",
        "-",
    ]);
    c.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());

    let out = c
        .output()
        .with_context(|| format!("ffmpeg on {}", src.display()))?;
    if !out.status.success() {
        anyhow::bail!("ffmpeg exited {}", out.status);
    }
    let bytes = out.stdout;
    if bytes.len() < 2 {
        anyhow::bail!("no audio in {}", src.display());
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect())
}

/// Fold a decoded buffer into `BUCKETS` peak columns, 0..255.
pub fn envelope(samples: &[i16]) -> Vec<u8> {
    let n = samples.len();
    if n == 0 {
        return vec![0; BUCKETS];
    }
    let mut env = vec![0u8; BUCKETS];
    // Peak, not mean: an RMS envelope of a loud master is a flat bar, and what
    // the eye is looking for on a seekbar is the transient.
    for (b, slot) in env.iter_mut().enumerate() {
        let lo = n * b / BUCKETS;
        let hi = (n * (b + 1) / BUCKETS).max(lo + 1).min(n);
        let mut peak: i32 = 0;
        for s in &samples[lo..hi] {
            // saturating: i16::MIN has no positive counterpart, and .abs() on
            // it overflows.
            let a = (*s as i32).saturating_abs();
            if a > peak {
                peak = a;
            }
        }
        *slot = ((peak * 255) / 32_767).clamp(0, 255) as u8;
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_changes_with_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.flac");
        std::fs::write(&f, b"one").unwrap();
        let first = key_for(&f);
        std::fs::write(&f, b"a longer body").unwrap();
        // Size alone is enough to move the key here; mtime granularity is one
        // second and a test writes faster than that.
        assert_ne!(first, key_for(&f), "a changed file must not reuse an envelope");
    }

    #[test]
    fn a_missing_file_still_produces_a_key() {
        assert!(!key_for(Path::new("/nope/gone.flac")).is_empty());
    }
}
