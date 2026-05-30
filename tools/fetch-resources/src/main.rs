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
    sha256: String,
    #[serde(default)]
    archive: Option<String>, // tar.xz | zip | tar.gz | none
    #[serde(default)]
    extract: Vec<String>,    // paths inside archive to keep
    #[serde(default)]
    build: Option<String>,   // "make" | "cmake" | ... — build from source after extract
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
        let target_marker = out_dir.join(format!(".{}-{}.ok", bin.name, bin.version));
        if target_marker.exists() && !dry_run {
            tracing::info!(name=%bin.name, "already fetched");
            continue;
        }
        tracing::info!(name=%bin.name, version=%bin.version, url=%src.url, "fetch");
        if dry_run { continue; }

        let bytes = reqwest::blocking::get(&src.url)?.error_for_status()?.bytes()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        let got = hex::encode_lower(h.finalize());
        if got != src.sha256.to_lowercase() {
            bail!("sha256 mismatch for {}: want {} got {}", bin.name, src.sha256, got);
        }
        // Extract usable files per `archive` + `extract`, falling back to the
        // raw blob for unknown/none archive types (e.g. the macOS .pkg).
        match src.archive.as_deref() {
            Some("tar.gz") => { extract_targz(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted tar.gz"); }
            Some("zip") => { extract_zip(&bytes, &out_dir, &src.extract)?;
                tracing::info!(name=%bin.name, "extracted zip"); }
            _ => {
                let raw_path = out_dir.join(format!("{}-{}.bin", bin.name, bin.version));
                fs::write(&raw_path, &bytes)?;
                tracing::info!(name=%bin.name, archive=?src.archive, "saved raw (no extractor)");
            }
        }
        fs::write(&target_marker, b"ok")?;
        tracing::info!(name=%bin.name, "verified + saved");
    }

    Ok(())
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
    if base.to_lowercase().contains("exiftool") && base.to_lowercase().ends_with(".exe") {
        return "exiftool.exe".into();
    }
    rel.to_string()
}

fn extract_targz(bytes: &[u8], out_dir: &std::path::Path, extract: &[String]) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut ar = tar::Archive::new(gz);
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
