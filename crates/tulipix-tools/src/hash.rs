//! `np.p4.tools.hash` — hash compute + verify; manifest generation.
//!
//! Computes SHA-256 (via `sha2`) and CRC-32 (for `.sfv`) natively; SHA-1 / MD5
//! / xxHash route to their backends in production. Generates GNU-coreutils-style
//! `.sha256` / `.md5` and `.sfv` manifests and verifies a manifest against
//! recomputed digests, reporting per-file pass/fail.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo { Sha256, Sha1, Md5, XxHash, Crc32 }

impl Algo {
    pub fn manifest_ext(self) -> &'static str {
        match self { Algo::Sha256 => "sha256", Algo::Md5 => "md5", Algo::Crc32 => "sfv", _ => "sum" }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Streaming SHA-256 of a file — constant memory, so multi-GB media never gets
/// slurped into RAM. `should_stop` is polled between chunks; returning `true`
/// aborts and yields `Ok(None)` (canceled).
pub fn sha256_file_hex_with(
    path: &str,
    mut should_stop: impl FnMut() -> bool,
) -> std::io::Result<Option<String>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    loop {
        if should_stop() { return Ok(None); }
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    Ok(Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect()))
}

/// Streaming SHA-256 of a file (no cancellation).
pub fn sha256_file_hex(path: &str) -> std::io::Result<String> {
    Ok(sha256_file_hex_with(path, || false)?.expect("no cancel"))
}

/// CRC-32 (IEEE) for SFV manifests.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// One manifest line. SFV is `filename hex`; the rest are `hex  filename`.
pub fn manifest_line(algo: Algo, hex: &str, filename: &str) -> String {
    match algo {
        Algo::Crc32 => format!("{filename} {}", hex.to_uppercase()),
        _ => format!("{hex}  {filename}"),
    }
}

/// Parse a `.sha256`/`.md5` manifest → `(filename, expected_hex_lowercase)`.
pub fn parse_manifest(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with(';') { return None; } // ';' = SFV comment
        let (hex, file) = l.split_once("  ").or_else(|| l.split_once(' '))?;
        Some((file.trim().to_string(), hex.trim().to_lowercase()))
    }).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct VerifyResult { pub file: String, pub ok: bool }

/// Verify entries: `compute` returns the recomputed hex for a filename (or
/// `None` if missing). Comparison is case-insensitive.
pub fn verify(entries: &[(String, String)], mut compute: impl FnMut(&str) -> Option<String>) -> Vec<VerifyResult> {
    entries.iter().map(|(file, expected)| {
        let ok = compute(file).is_some_and(|got| got.eq_ignore_ascii_case(expected));
        VerifyResult { file: file.clone(), ok }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc")
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn crc32_known_vector() {
        // CRC-32("123456789") = 0xCBF43926
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn manifest_roundtrip_and_verify() {
        let hex = sha256_hex(b"hello");
        let line = manifest_line(Algo::Sha256, &hex, "hello.txt");
        let parsed = parse_manifest(&line);
        assert_eq!(parsed[0].0, "hello.txt");
        let res = verify(&parsed, |f| if f == "hello.txt" { Some(sha256_hex(b"hello")) } else { None });
        assert!(res[0].ok);
        // tampered content fails
        let bad = verify(&parsed, |_| Some(sha256_hex(b"world")));
        assert!(!bad[0].ok);
    }

    #[test]
    fn sfv_line_format() {
        assert_eq!(manifest_line(Algo::Crc32, "cbf43926", "a.bin"), "a.bin CBF43926");
    }
}
