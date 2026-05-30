use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Compute SHA-256 of file contents, streaming.
pub fn sha256_file(p: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hex(b: impl AsRef<[u8]>) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let b = b.as_ref();
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(T[(x >> 4) as usize] as char);
        s.push(T[(x & 0x0f) as usize] as char);
    }
    s
}

#[derive(Debug, Clone)]
pub struct ProxyRow {
    pub path: PathBuf,
    pub inode: u64,
    pub size: u64,
    pub mtime_unix: i64,
    pub sha256: Option<String>,
}

/// Build a proxy row from filesystem stat — never copies bytes.
pub fn proxy_stat(p: &Path) -> std::io::Result<ProxyRow> {
    let meta = fs::metadata(p)?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&meta);
    #[cfg(not(unix))]
    let inode = 0u64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Ok(ProxyRow {
        path: p.to_path_buf(),
        inode,
        size: meta.len(),
        mtime_unix: mtime,
        sha256: None,
    })
}

pub fn glob_match(pat: &str, path: &str) -> bool {
    // Tiny shell-glob matcher (*, ?, **) — good enough for exclude rules.
    fn m(pat: &[u8], s: &[u8]) -> bool {
        let mut pi = 0; let mut si = 0;
        let mut star: Option<(usize, usize)> = None;
        while si < s.len() {
            if pi < pat.len() && (pat[pi] == s[si] || pat[pi] == b'?') {
                pi += 1; si += 1;
            } else if pi < pat.len() && pat[pi] == b'*' {
                star = Some((pi, si));
                pi += 1;
            } else if let Some((p, ss)) = star {
                pi = p + 1; si = ss + 1; star = Some((p, si));
            } else {
                return false;
            }
        }
        while pi < pat.len() && pat[pi] == b'*' { pi += 1; }
        pi == pat.len()
    }
    m(pat.as_bytes(), path.as_bytes())
}
