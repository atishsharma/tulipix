use crate::paths;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// LRU cap (bytes) for thumbnail cache.
pub const CACHE_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024; // 5 GB

/// Thumb cache key — first 12 hex chars of SHA-256(path:mtime:size).
pub fn key(path: &Path, mtime: i64, size: u64) -> String {
    let mut h = Sha256::new();
    h.update(path.display().to_string().as_bytes());
    h.update(format!(":{mtime}:{size}").as_bytes());
    let d = h.finalize();
    let mut out = String::with_capacity(24);
    for &b in d.iter().take(6) {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

pub fn thumb_path(k: &str) -> Option<PathBuf> {
    // PNG, not WebP: ffmpeg picks the encoder from this extension, and the
    // `image` crate (Slint's file decoder) reads PNG without the WebP-decode
    // quirks that broke audio/video thumbnails ("Invalid Huffman code").
    paths::thumbs_dir().map(|d| d.join(format!("{k}.png")))
}

/// Clear entire cache directory.
pub fn clear_cache() -> std::io::Result<()> {
    if let Some(d) = paths::thumbs_dir() {
        if d.exists() {
            std::fs::remove_dir_all(&d)?;
            std::fs::create_dir_all(&d)?;
        }
    }
    Ok(())
}

/// Return total bytes in cache.
pub fn cache_size() -> std::io::Result<u64> {
    let Some(d) = paths::thumbs_dir() else { return Ok(0); };
    if !d.exists() { return Ok(0); }
    let mut total = 0u64;
    for e in std::fs::read_dir(d)? {
        let e = e?;
        if let Ok(m) = e.metadata() {
            if m.is_file() { total += m.len(); }
        }
    }
    Ok(total)
}

/// Type-of-file → which renderer should produce the thumb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbKind {
    Photo,       // jpeg/png/heic/avif/jpegxl/raw
    Video,       // mp4/mkv/mov via ffmpeg first frame
    Audio,       // mp3/flac/m4a → embedded album art
    Book,        // epub cover / cbz first page / pdf page1
    Subtitle,    // srt/vtt — small monogram
    Playlist,    // m3u/pls — synthetic icon
    Archive,     // zip/tar — synthetic icon
    Doc,         // pdf/docx/odt — page1 thumbnail
    OsFallback,  // delegated to OS icon-theme
}

pub fn kind_for(ext: &str) -> ThumbKind {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "heic" | "heif" | "avif" | "jxl"
        | "tif" | "tiff" | "bmp" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "raf"
        | "rw2" | "orf" | "pef" => ThumbKind::Photo,
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "mpg" | "mpeg" | "ts" | "m2ts"
        | "wmv" | "flv" | "3gp" | "ogv" => ThumbKind::Video,
        "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "aiff" | "wma" | "ape"
        | "wv" | "mka" | "mpc" | "tta" | "dsf" | "dff" => ThumbKind::Audio,
        "epub" | "mobi" | "azw" | "azw3" | "cbz" | "cbr" | "fb2" => ThumbKind::Book,
        "srt" | "vtt" | "ass" | "ssa" | "sub" => ThumbKind::Subtitle,
        "m3u" | "m3u8" | "pls" | "xspf" => ThumbKind::Playlist,
        "zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "rar" => ThumbKind::Archive,
        "pdf" | "docx" | "doc" | "odt" | "rtf" | "txt" | "md" => ThumbKind::Doc,
        _ => ThumbKind::OsFallback,
    }
}

/// User-set universal tools directory (Settings → "Tools directory"). Cached so
/// the hot thumbnail path doesn't re-read settings.json per render. Initialised
/// lazily from `TULIPIX_TOOL_DIR` (env override) then the saved setting; the
/// Settings panel calls `set_tool_dir` to apply a change without a restart.
fn tool_dir_cell() -> &'static std::sync::RwLock<Option<Option<PathBuf>>> {
    static C: std::sync::OnceLock<std::sync::RwLock<Option<Option<PathBuf>>>> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::RwLock::new(None))
}

fn load_tool_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("TULIPIX_TOOL_DIR") {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() { return Some(p); }
    }
    let dir = crate::settings::Settings::load().ok()?.text("tools.bin-dir");
    let dir = dir.trim();
    if dir.is_empty() { None } else { Some(PathBuf::from(dir)) }
}

fn tool_dir() -> Option<PathBuf> {
    if let Some(cached) = tool_dir_cell().read().ok().and_then(|g| g.clone()) {
        return cached;
    }
    let v = load_tool_dir();
    if let Ok(mut g) = tool_dir_cell().write() { *g = Some(v.clone()); }
    v
}

/// Update the cached tools directory (called after the user edits the setting).
/// `dir` empty/None clears it back to bundle + PATH resolution.
pub fn set_tool_dir(dir: Option<&str>) {
    let v = dir.map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from);
    if let Ok(mut g) = tool_dir_cell().write() { *g = Some(v); }
}

/// Resolve a tool binary, in priority order:
///   1. the user's universal tools directory (Settings) if it holds a native copy
///   2. the bundled per-OS copy (`resources/bin/<os-arch>/`) if present + native
///   3. the bare name (found on PATH).
/// Used across the app (ffmpeg/ffprobe/yt-dlp/exiftool/mpv).
pub fn tool_bin(name: &str) -> PathBuf {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    if let Some(dir) = tool_dir() {
        let cand = dir.join(format!("{name}{ext}"));
        if cand.exists() && is_native_executable(&cand) && starts(&cand) { return cand; }
    }
    bundled_bin(name).unwrap_or_else(|| PathBuf::from(name))
}

/// Whether a binary actually starts, as opposed to merely being a native
/// executable file.
///
/// A bundled tool whose shared libraries are missing passes every static check:
/// the file exists, and its magic bytes say ELF. It execs fine too — the
/// dynamic loader is what fails, after the spawn has already been reported as
/// successful, so the caller sees a tool that launched and instantly vanished
/// with no error anywhere. That is what a mislaid `lib/` directory in a release
/// tarball did to mpv: playback silently did nothing while a working
/// `/usr/bin/mpv` sat unused on PATH.
///
/// Run the candidate once and treat only "command not executable" (127) as
/// broken. The flag is irrelevant — a binary that links will exit with some
/// other status even if it dislikes the argument, and one that cannot link
/// never reaches its own argument parsing. Answers are cached, so each tool
/// costs at most one probe per session.
fn starts(path: &Path) -> bool {
    use crate::proc::NoWindow;
    static PROBED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, bool>>,
    > = std::sync::OnceLock::new();
    let cell = PROBED.get_or_init(Default::default);
    if let Ok(g) = cell.lock() {
        if let Some(&known) = g.get(path) { return known; }
    }
    // 127 is the unix "could not execute" status; windows reports a missing DLL
    // as STATUS_DLL_NOT_FOUND / STATUS_DLL_INIT_FAILED instead.
    const BROKEN: [i32; 3] = [127, 0xC000_0135u32 as i32, 0xC000_0142u32 as i32];
    let ok = match std::process::Command::new(path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .no_window()
        .status()
    {
        Ok(st) => !st.code().is_some_and(|c| BROKEN.contains(&c)),
        // Could not even be spawned (not executable, wrong arch).
        Err(_) => false,
    };
    if let Ok(mut g) = cell.lock() { g.insert(path.to_path_buf(), ok); }
    ok
}

/// Path to a bundled per-OS binary used by thumb renderers (ffmpeg, exiftool).
/// Walks up from `current_exe()` looking for `resources/bin/<os-arch>/<name>`
/// so both the production install layout (`<install>/bin/tulipix` →
/// `<install>/resources/...`) and the dev tree (`target/debug/tulipix` →
/// `<repo>/resources/...`) resolve.
fn bundled_bin(name: &str) -> Option<PathBuf> {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    walk_bundled(&format!("{name}{ext}"))
        .filter(|p| is_native_executable(p))
        .filter(|p| starts(p))
}

/// Path to any bundled per-OS file (e.g. the whisper model) — same walk as
/// `bundled_bin` but without the native-executable check, and no `.exe` suffix.
/// Works in both the installed layout and the dev tree, unlike the
/// compile-time `CARGO_MANIFEST_DIR` path in tulipix-common.
pub fn bundled_file(name: &str) -> Option<PathBuf> {
    walk_bundled(name)
}

fn walk_bundled(file: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let os_arch = if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") { "linux-aarch64" }
        else if cfg!(target_os = "linux") { "linux-x86_64" }
        else if cfg!(target_os = "windows") { "windows-x86_64" }
        else if cfg!(target_arch = "aarch64") { "macos-aarch64" }
        else { "macos-x86_64" };
    let tail = std::path::Path::new("resources").join("bin").join(os_arch).join(file);
    let mut cursor = exe.parent();
    while let Some(dir) = cursor {
        let cand = dir.join(&tail);
        if cand.exists() { return Some(cand); }
        cursor = dir.parent();
    }
    None
}

/// True only if `path` looks like a runnable native binary for this OS. Guards
/// against a bundle that shipped the still-compressed artifact (the fetched
/// `*.bin` is XZ until `just fetch` decompresses it) — handing that to
/// `Command` yields "exec format error". When false, callers fall back to the
/// system binary on PATH.
fn is_native_executable(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false; };
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() { return false; }
    if cfg!(target_os = "windows") {
        &magic[..2] == b"MZ"
    } else if cfg!(target_os = "macos") {
        // Mach-O (LE/BE/64) or universal "fat" binary.
        matches!(magic, [0xCF, 0xFA, 0xED, 0xFE] | [0xCE, 0xFA, 0xED, 0xFE]
            | [0xFE, 0xED, 0xFA, 0xCF] | [0xCA, 0xFE, 0xBA, 0xBE])
    } else {
        magic == [0x7F, b'E', b'L', b'F']
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThumbSpec {
    pub kind: ThumbKind,
    pub width: u32,
    pub height: u32,
}

impl Default for ThumbSpec {
    fn default() -> Self { Self { kind: ThumbKind::Photo, width: 320, height: 320 } }
}

#[derive(Debug, Clone)]
pub struct ThumbResult {
    pub path: PathBuf,
    pub key: String,
    pub kind: ThumbKind,
}

/// Render (or return cached) thumb for `src`. Idempotent: re-running with the
/// same (path, mtime, size) hits the cache. Returns `Ok(None)` if the kind
/// dispatches to the OS icon fallback (callers paint that themselves).
pub fn render_or_cache(src: &Path, spec: ThumbSpec) -> Result<Option<ThumbResult>> {
    let meta = std::fs::metadata(src)?;
    let mtime = meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let k = key(src, mtime, meta.len());
    let Some(out) = thumb_path(&k) else { anyhow::bail!("no thumbs dir"); };
    if out.exists() {
        return Ok(Some(ThumbResult { path: out, key: k, kind: spec.kind }));
    }
    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
    // Render into a temp sibling, then atomically rename in. A process killed
    // mid-render therefore never leaves a truncated PNG cached under `out`
    // (which would be reused forever via the exists() check above).
    // Keep `.png` as the trailing extension — ffmpeg infers the output format
    // from it. (A `.tmp` suffix would make ffmpeg write an unknown/garbage
    // format that then fails to decode.)
    let tmp = out.with_extension("tmp.png");
    let r = match spec.kind {
        ThumbKind::Photo  => render_photo(src, &tmp, spec),
        ThumbKind::Video  => render_video(src, &tmp, spec),
        ThumbKind::Audio  => render_audio(src, &tmp, spec),
        ThumbKind::Book   => render_book(src, &tmp, spec),
        ThumbKind::Doc    => render_doc(src, &tmp, spec),
        ThumbKind::Subtitle | ThumbKind::Playlist | ThumbKind::Archive => {
            render_synthetic(src, &tmp, spec)
        }
        ThumbKind::OsFallback => return Ok(None),
    };
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, &out)?;
    Ok(Some(ThumbResult { path: out, key: k, kind: spec.kind }))
}

fn run_ok(mut cmd: Command) -> Result<()> {
    use crate::proc::NoWindow;
    cmd.no_window();
    let status = cmd.status()?;
    if !status.success() { anyhow::bail!("renderer exited {status}"); }
    Ok(())
}

fn render_photo(src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    // Use ffmpeg's image2 demuxer — handles JPEG/PNG/WebP/HEIC/AVIF/RAW via libheif/libraw
    // if the bundled build was compiled with them. The default BtbN GPL build covers HEIC + AVIF.
    let ff = bundled_bin("ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let mut c = Command::new(ff);
    c.args([
        "-y", "-loglevel", "error", "-i",
    ]);
    c.arg(src);
    c.args([
        "-vf", &format!("scale='min({w},iw)':'min({h},ih)':force_original_aspect_ratio=decrease",
            w = spec.width, h = spec.height),
        "-frames:v", "1",
    ]);
    c.arg(out);
    run_ok(c)?;
    // Reject a degenerate output (ffmpeg can exit 0 yet write a tiny invalid
    // PNG for unusual inputs) so the caller's pure-Rust fallback runs instead
    // of caching an undecodable thumb forever.
    match std::fs::metadata(out) {
        Ok(m) if m.len() >= 256 => Ok(()),
        _ => anyhow::bail!("ffmpeg produced a degenerate thumbnail"),
    }
}

fn render_video(src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    // No fixed `-ss` pre-seek: seeking past the end of a short clip makes
    // ffmpeg exit 0 while writing nothing. The `thumbnail` filter scans the
    // opening frames and picks a representative (non-uniform) one, so even a
    // 1-second clip yields output.
    let ff = bundled_bin("ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let mut c = Command::new(ff);
    c.args(["-y", "-loglevel", "error", "-i"]);
    c.arg(src);
    c.args([
        "-vf", &format!("thumbnail,scale={}:{}:force_original_aspect_ratio=decrease",
            spec.width, spec.height),
        "-frames:v", "1",
    ]);
    c.arg(out);
    run_ok(c)?;
    // Guard: ffmpeg occasionally reports success without producing a file
    // (no decodable frame). Treat a missing/empty output as a render failure
    // so the caller's fallback runs instead of caching a dangling path.
    match std::fs::metadata(out) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => anyhow::bail!("ffmpeg produced no thumbnail frame"),
    }
}

fn render_audio(src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    // Embedded album-art is its own stream in MP3/FLAC/M4A.
    let ff = bundled_bin("ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let mut c = Command::new(ff);
    c.args(["-y", "-loglevel", "error", "-i"]);
    c.arg(src);
    // Center-crop to the target aspect (cover), not letterbox: a 16:9 YouTube-video
    // thumbnail embedded as cover art becomes a clean square, so the library tiles
    // and now-playing art never show black bars (np: square-art).
    c.args(["-map", "0:v?", "-vf", &format!(
        "crop='min(iw,ih*{w}/{h})':'min(ih,iw*{h}/{w})',scale={w}:{h}",
        w = spec.width, h = spec.height)]);
    c.arg(out);
    // Audio without embedded art falls through to synthetic. ffmpeg exits 0
    // even when `0:v?` matched no stream — it then writes a degenerate 67-byte
    // PNG that fails to decode. Treat a missing/tiny output as "no art" and
    // fall back to the valid synthetic placeholder.
    if run_ok(c).is_err() { return render_synthetic(src, out, spec); }
    match std::fs::metadata(out) {
        Ok(m) if m.len() >= 256 => Ok(()),
        _ => { let _ = std::fs::remove_file(out); render_synthetic(src, out, spec) }
    }
}

fn render_book(_src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    // EPUB cover / CBZ first page / PDF page-1 are handled by section-specific
    // crates (tulipix-books / tulipix-photos). At this layer we drop a synthetic
    // placeholder so the dispatcher always produces a file.
    write_placeholder(out, spec, b"BOOK")
}

fn render_doc(_src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    write_placeholder(out, spec, b"DOC")
}

fn render_synthetic(_src: &Path, out: &Path, spec: ThumbSpec) -> Result<()> {
    write_placeholder(out, spec, b"FILE")
}

/// Minimal 1×1 transparent PNG placeholder. The UI scales the 1px up to the
/// tile size. These bytes are a zlib-verified valid PNG — the previous
/// hand-rolled array had a malformed IDAT deflate stream that the `image`
/// crate rejected with "Corrupt deflate stream / InvalidUncompressedBlockLength",
/// so every fallback tile showed up blank.
fn write_placeholder(out: &Path, _spec: ThumbSpec, _label: &[u8]) -> Result<()> {
    const BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
        0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00,
        0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0x60, 0x60, 0x60, 0x60,
        0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0x7A, 0xA8, 0x57, 0x50, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    std::fs::write(out, BYTES).map_err(Into::into)
}

/// Look up a native OS icon for a file type that has no bespoke thumb.
///
/// Returns `Some(path)` to a cached PNG copy of the icon, ready for the
/// renderer; `None` when the platform call failed or returned nothing.
///
/// macOS: NSWorkspace.iconForFile via `qlmanage -t` if available, else NSImage.
/// Windows: SHGetFileInfo + IExtractIcon → PNG cache.
/// Linux: GTK icon theme lookup (gtk-icon-lookup) or `xdg-icon-resource find`.
///
/// Pure-Rust per-OS bindings sit in `tulipix-platform`; this function shells
/// to the canonical CLI as a fallback so headless tests pass.
pub fn os_icon_for(path: &Path) -> Option<PathBuf> {
    let mime_ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let out = paths::thumbs_dir()?.join(format!("icon-{mime_ext}.png"));
    if out.exists() { return Some(out); }
    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent).ok()?; }

    #[cfg(target_os = "linux")]
    {
        // Try `gio` first (returns the freedesktop icon name), then map to a
        // theme path via `gtk-update-icon-cache`/`xdg-icon-resource`.
        if let Ok(o) = Command::new("xdg-mime").args(["query", "filetype"]).arg(path).output() {
            if let Ok(mime) = String::from_utf8(o.stdout) {
                let mime = mime.trim();
                if !mime.is_empty() {
                    let icon = mime.replace('/', "-");
                    if let Some(p) = find_xdg_icon(&icon) {
                        let _ = std::fs::copy(&p, &out);
                        return Some(out);
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // qlmanage -t -s 128 -o <dir> <path> writes <path>.png alongside.
        let dir = out.parent()?;
        let _ = Command::new("qlmanage")
            .args(["-t", "-s", "128", "-o"]).arg(dir).arg(path)
            .status();
        if out.exists() { return Some(out); }
    }
    #[cfg(target_os = "windows")]
    {
        // SHGetFileInfo + GdiplusBitmap::Save is implemented in tulipix-platform
        // (real call gated behind a platform feature). Fall through to None here
        // so the universal pipeline draws a synthetic glyph instead.
        let _ = path;
    }
    None
}

#[cfg(target_os = "linux")]
fn find_xdg_icon(name: &str) -> Option<PathBuf> {
    let theme = std::env::var("XCURSOR_THEME").ok()
        .or_else(|| std::env::var("GTK_THEME").ok())
        .unwrap_or_else(|| "hicolor".into());
    let roots = [
        "/usr/share/icons",
        "/usr/local/share/icons",
        "/usr/share/pixmaps",
    ];
    for root in roots {
        for size in ["128x128", "96x96", "64x64", "48x48", "scalable"] {
            let p = PathBuf::from(root).join(&theme).join(size).join("mimetypes").join(format!("{name}.png"));
            if p.exists() { return Some(p); }
            let p = PathBuf::from(root).join(&theme).join(size).join("mimetypes").join(format!("{name}.svg"));
            if p.exists() { return Some(p); }
        }
    }
    None
}

/// LRU eviction down to `CACHE_CAP_BYTES`. Drops oldest-accessed files first.
pub fn evict_lru() -> std::io::Result<u64> {
    let Some(d) = paths::thumbs_dir() else { return Ok(0); };
    if !d.exists() { return Ok(0); }
    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for e in std::fs::read_dir(&d)? {
        let e = e?;
        let m = e.metadata()?;
        if !m.is_file() { continue; }
        let len = m.len();
        let at = m.accessed().or_else(|_| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        total += len;
        entries.push((e.path(), len, at));
    }
    if total <= CACHE_CAP_BYTES { return Ok(0); }
    entries.sort_by_key(|(_, _, at)| *at);
    let mut freed = 0u64;
    for (p, len, _) in entries {
        if total <= CACHE_CAP_BYTES { break; }
        let _ = std::fs::remove_file(&p);
        total = total.saturating_sub(len);
        freed += len;
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_stable() {
        let p = Path::new("/foo/bar.jpg");
        assert_eq!(key(p, 1000, 42), key(p, 1000, 42));
        assert_ne!(key(p, 1001, 42), key(p, 1000, 42));
    }
    #[test]
    fn ext_mapping() {
        assert_eq!(kind_for("jpg"), ThumbKind::Photo);
        assert_eq!(kind_for("mkv"), ThumbKind::Video);
        assert_eq!(kind_for("epub"), ThumbKind::Book);
        assert_eq!(kind_for("xyz"), ThumbKind::OsFallback);
    }

    #[test]
    fn os_fallback_returns_none_when_dispatched() {
        // OsFallback kind short-circuits the pipeline; callers paint icon via os_icon_for.
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.xyz");
        std::fs::write(&f, b"x").unwrap();
        let r = render_or_cache(&f, ThumbSpec { kind: ThumbKind::OsFallback, ..Default::default() }).unwrap();
        assert!(r.is_none());
    }
}
