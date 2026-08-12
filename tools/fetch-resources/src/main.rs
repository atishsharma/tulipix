use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

mod audit;

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(rename = "binary")]
    binaries: Vec<Binary>,
}

#[derive(Debug, Deserialize)]
struct Binary {
    name: String,
    version: String,
    sources: Vec<Source>,
}

#[derive(Debug, Deserialize)]
struct Source {
    os: String,    // linux | windows | macos
    arch: String,  // x86_64 | aarch64
    url: String,
    /// Second source for the same artifact, tried only after `url` has failed
    /// every retry. For `latest = true` sources, where there is no sha to
    /// disagree with; a pinned source would fail its hash check on a mirror
    /// that ships a different build, which is the correct outcome.
    #[serde(default)]
    fallback_url: Option<String>,
    sha256: String,
    #[serde(default)]
    archive: Option<String>, // tar.xz | zip | tar.gz | none
    #[serde(default)]
    extract: Vec<String>,    // paths inside archive to keep
    #[serde(default)]
    #[allow(dead_code)] // manifest field reserved for build-from-source sources (whisper)
    build: Option<String>,   // "make" | "cmake" | ... — build from source after extract
    #[serde(default)]
    latest: bool,            // rolling "latest" asset — skip the sha pin, always re-fetch
}

fn host_os() -> &'static str {
    if cfg!(target_os = "linux") { "linux" }
    else if cfg!(target_os = "windows") { "windows" }
    else if cfg!(target_os = "macos") { "macos" }
    else { "unknown" }
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") { "x86_64" }
    else if cfg!(target_arch = "aarch64") { "aarch64" }
    else { "unknown" }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let dry_run = std::env::args().any(|a| a == "--dry-run");
    let audit_only = std::env::args().any(|a| a == "--audit");
    let only: Option<String> = std::env::args()
        .skip_while(|a| a != "--only")
        .nth(1);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let manifest_path = root.join("resources/binaries.toml");
    let out_dir = root.join(format!("resources/bin/{}-{}", host_os(), host_arch()));
    fs::create_dir_all(&out_dir)?;

    if audit_only {
        let r = audit::audit_dir(&out_dir, host_os())?;
        audit::print_report(&r);
        if r.over_budget { std::process::exit(2); }
        return Ok(());
    }

    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let mf: Manifest = toml::from_str(&text)?;

    for bin in &mf.binaries {
        if let Some(o) = &only { if o != &bin.name { continue; } }
        let Some(src) = bin.sources.iter().find(|s| s.os == host_os() && s.arch == host_arch())
        else {
            tracing::warn!(name=%bin.name, "no source for {}-{}", host_os(), host_arch());
            continue;
        };
        // "latest" assets roll, so never short-circuit on the marker and never
        // pin a sha (the upstream hash changes with every release).
        let target_marker = out_dir.join(format!(".{}-{}.ok", bin.name, bin.version));
        if target_marker.exists() && !dry_run && !src.latest {
            tracing::info!(name=%bin.name, "already fetched");
            continue;
        }
        tracing::info!(name=%bin.name, version=%bin.version, url=%src.url, latest=src.latest, "fetch");
        if dry_run { continue; }

        // Real UA — SourceForge/CDNs 403 the default reqwest agent from CI IPs.
        let client = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) tulipix-fetch/1.0")
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        let bytes = fetch(&client, &src.url, src.fallback_url.as_deref())?;
        if !src.latest {
            let mut h = Sha256::new();
            h.update(&bytes);
            let got = hex::encode_lower(h.finalize());
            if got != src.sha256.to_lowercase() {
                bail!("sha256 mismatch for {}: want {} got {}", bin.name, src.sha256, got);
            }
        } else {
            tracing::info!(name=%bin.name, "latest asset — sha pin skipped");
        }
        // Extract usable files per `archive` + `extract`, falling back to the
        // raw blob for unknown/none archive types (e.g. the macOS .pkg).
        match src.archive.as_deref() {
            Some("tar.gz") => { extract_targz(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted tar.gz"); }
            Some("tar.xz") => { extract_tarxz(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted tar.xz"); }
            Some("zip") => { extract_zip(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted zip"); }
            Some("7z") => { extract_7z(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted 7z"); }
            _ => {
                // Executables keep the tool's bare name so tool_bin() finds them
                // (`yt-dlp-latest.bin` was invisible to the app). Models keep the
                // versioned `<name>-<version>.bin` the loaders look up.
                let raw_path = if bin.name.starts_with("ggml") {
                    out_dir.join(format!("{}-{}.bin", bin.name, bin.version))
                } else if src.url.ends_with(".exe") {
                    out_dir.join(format!("{}.exe", bin.name))
                } else {
                    out_dir.join(&bin.name)
                };
                fs::write(&raw_path, &bytes)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&raw_path, fs::Permissions::from_mode(0o755));
                }
                tracing::info!(name=%bin.name, archive=?src.archive, "saved raw (no extractor)");
            }
        }
        fs::write(&target_marker, b"ok")?;
        tracing::info!(name=%bin.name, "verified + saved");
    }

    Ok(())
}

/// Download `url`, retrying transient failures, then `fallback` the same way.
///
/// Every host in the manifest has failed a release build at least once by
/// answering a CI runner and nobody else: SourceForge 403s, johnvansickle 415s,
/// and gyan.dev returned 503 during v0.8.0. A single unretried GET turns any of
/// those minutes-long blips into a dead hour-long build, so transient statuses
/// (5xx, 408, 429) and connection errors are retried before the source is
/// called broken. A 404 is not transient and fails immediately.
fn fetch(
    client: &reqwest::blocking::Client,
    url: &str,
    fallback: Option<&str>,
) -> Result<Vec<u8>> {
    const TRIES: u32 = 3;
    let mut last: Option<anyhow::Error> = None;

    for u in std::iter::once(url).chain(fallback) {
        // Resolved once per source, not per attempt: an asset on a private repo
        // has to be fetched through the API, with a token.
        let (target, token) = match api_asset(client, u) {
            Some((api, t)) => {
                tracing::info!(url = %u, "private release asset — going through the API");
                (api, Some(t))
            }
            None => (u.to_string(), None),
        };
        for attempt in 1..=TRIES {
            let mut req = client.get(&target);
            if let Some(t) = &token {
                req = req
                    .header("Accept", "application/octet-stream")
                    .header("Authorization", format!("Bearer {t}"));
            }
            let got = req
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.bytes())
                .map(|b| b.to_vec());
            match got {
                Ok(b) => {
                    if u != url {
                        tracing::warn!(primary = %url, used = %u, "primary source failed — fell back");
                    }
                    return Ok(b);
                }
                Err(e) => {
                    let transient = e.is_timeout()
                        || e.is_connect()
                        || e.status().is_none_or(|s| {
                            s.is_server_error() || s == 408 || s == 429
                        });
                    tracing::warn!(url = %u, attempt, transient, error = %e, "fetch failed");
                    last = Some(e.into());
                    if !transient { break; }
                    if attempt < TRIES {
                        std::thread::sleep(std::time::Duration::from_secs(3 * attempt as u64));
                    }
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("fetch {url}: no attempt was made")))
}

/// Turn a `github.com/<owner>/<repo>/releases/download/<tag>/<file>` URL into
/// the API asset URL that can actually be downloaded, plus the token to do it
/// with. Returns `None` for every other URL, and when no token is in the
/// environment.
///
/// This repo is private, and a private repo answers an anonymous GET on a
/// release download URL with **404** — which is what killed the Windows job of
/// v0.9.0 on the exiftool zip cached under the `tooling-cache` release. The
/// documented way in is `/repos/{o}/{r}/releases/tags/{tag}`, then the asset's
/// own `url` with `Accept: application/octet-stream`. reqwest drops the
/// Authorization header when GitHub redirects to its (already signed) storage
/// host, so the token never leaves github.com.
fn api_asset(client: &reqwest::blocking::Client, url: &str) -> Option<(String, String)> {
    let token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty())?;
    let rest = url.strip_prefix("https://github.com/")?;
    let (owner, rest) = rest.split_once('/')?;
    let (repo, rest) = rest.split_once("/releases/download/")?;
    let (tag, file) = rest.split_once('/')?;

    #[derive(Deserialize)]
    struct Rel { assets: Vec<Asset> }
    #[derive(Deserialize)]
    struct Asset { name: String, url: String }

    let rel: Rel = client
        .get(format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| tracing::warn!(url = %url, error = %e, "release lookup failed — trying the plain URL"))
        .ok()?;
    let asset = rel.assets.into_iter().find(|a| a.name == file)?;
    Some((asset.url, token))
}

/// Drop the leading path component (e.g. the `Image-ExifTool-13.58/` wrapper).
fn strip_top(p: &str) -> String {
    match p.split_once('/') { Some((_, rest)) => rest.to_string(), None => p.to_string() }
}

/// Whether `rel` (top-stripped) is one of the wanted entries. A trailing `/`
/// matches a whole subtree; otherwise an exact path match.
fn want(rel: &str, extract: &[String]) -> bool {
    if extract.is_empty() { return true; }
    extract.iter().any(|e| if e.ends_with('/') { rel.starts_with(e.as_str()) } else { rel == e.as_str() })
}

/// Destination name for an extracted entry. Normalises the Windows standalone
/// `exiftool(-k).exe` to `exiftool.exe`.
fn out_name(rel: &str) -> String {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    let lower = base.to_lowercase();
    if lower.contains("exiftool") && lower.ends_with(".exe") {
        return "exiftool.exe".into();
    }
    // Media tools live under `bin/` in their archives (BtbN ffmpeg, mpv) — flatten
    // to the bare basename so tool_bin finds them at resources/bin/<os-arch>/<name>.
    let stem = lower.trim_end_matches(".exe");
    if matches!(stem, "ffmpeg" | "ffprobe" | "ffplay" | "mpv") {
        return base.to_string();
    }
    rel.to_string()
}

fn extract_targz(bytes: &[u8], out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    extract_tar(flate2::read::GzDecoder::new(bytes), out_dir, extract)
}

fn extract_tarxz(bytes: &[u8], out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    extract_tar(xz2::read::XzDecoder::new(bytes), out_dir, extract)
}

fn extract_tar(reader: impl Read, out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    let mut ar = tar::Archive::new(reader);
    for entry in ar.entries()? {
        let mut e = entry?;
        let path = e.path()?.to_string_lossy().into_owned();
        let rel = strip_top(&path);
        if rel.is_empty() || !want(&rel, extract) { continue; }
        if e.header().entry_type().is_dir() { continue; }
        let dest = out_dir.join(out_name(&rel));
        if let Some(p) = dest.parent() { fs::create_dir_all(p)?; }
        e.unpack(&dest).with_context(|| format!("unpack {rel}"))?;
    }
    Ok(())
}

/// Extract a `.7z` (mpv Windows builds ship this way). sevenz-rust has no
/// selective API, so unpack to a temp dir, then copy the wanted entries
/// (flattened) into `out_dir` and drop the temp.
fn extract_7z(bytes: &[u8], out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    let tmp = out_dir.join(".7z-tmp");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    sevenz_rust::decompress(std::io::Cursor::new(bytes), &tmp)
        .map_err(|e| anyhow::anyhow!("7z decompress: {e}"))?;
    copy_wanted(&tmp, &tmp, out_dir, extract)?;
    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

/// Recursively copy files under `base` whose path (relative to `root`, top
/// component stripped) matches `extract`, flattening via `out_name`.
fn copy_wanted(root: &std::path::Path, base: &std::path::Path, out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    for entry in fs::read_dir(base)? {
        let p = entry?.path();
        if p.is_dir() { copy_wanted(root, &p, out_dir, extract)?; continue; }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let rel = strip_top(&rel);
        if rel.is_empty() || !want(&rel, extract) { continue; }
        let dest = out_dir.join(out_name(&rel));
        if let Some(par) = dest.parent() { fs::create_dir_all(par)?; }
        fs::copy(&p, &dest).with_context(|| format!("copy {rel}"))?;
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        if f.is_dir() { continue; }
        let Some(name) = f.enclosed_name().map(|p| p.to_string_lossy().into_owned()) else { continue; };
        let rel = strip_top(&name.replace('\\', "/"));
        if rel.is_empty() || !want(&rel, extract) { continue; }
        let dest = out_dir.join(out_name(&rel));
        if let Some(p) = dest.parent() { fs::create_dir_all(p)?; }
        let mut out = fs::File::create(&dest).with_context(|| format!("create {}", dest.display()))?;
        std::io::copy(&mut f, &mut out)?;
        // Preserve the exec bit — zip extraction dropped it and the bundled
        // rclone came out non-executable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = f.unix_mode().unwrap_or(0o755);
            let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(if mode & 0o111 != 0 || f.unix_mode().is_none() { 0o755 } else { mode }));
        }
    }
    Ok(())
}

mod hex {
    pub fn encode_lower(b: impl AsRef<[u8]>) -> String {
        const T: &[u8; 16] = b"0123456789abcdef";
        let b = b.as_ref();
        let mut out = String::with_capacity(b.len() * 2);
        for &x in b {
            out.push(T[(x >> 4) as usize] as char);
            out.push(T[(x & 0x0f) as usize] as char);
        }
        out
    }
}

#[allow(dead_code)]
fn _force_read_trait(mut r: impl Read) -> std::io::Result<()> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf)
}
