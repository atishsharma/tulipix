//! Turning a name chosen by a network peer into one this filesystem accepts.
//!
//! Every rule here runs on every platform, not behind a `cfg`. A file uploaded
//! on Linux gets copied to a Windows machine eventually, and a Linux-only rule
//! would not have caught it on the way in.

use std::path::Path;

/// Longest name we will write, in bytes. Filesystems commonly stop at 255; the
/// margin leaves room for a " (12)" collision suffix.
const MAX_LEN: usize = 200;

/// Reserved on Windows with or without an extension: `CON.txt` is still `CON`.
const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub fn safe_name(raw: &str) -> String {
    // Keep only what follows the last separator, checking both kinds: the peer
    // may be a Windows machine and we may not be.
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);

    if base.chars().all(|c| c == '.') {
        return "upload".to_string();
    }

    let mut out: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\' => '_',
            c if (c as u32) < 0x20 || c == '\u{7f}' => '_',
            c => c,
        })
        .collect();

    // Windows trims these on write, which would silently merge two names.
    out = out.trim_end_matches(['.', ' ']).to_string();
    if out.is_empty() {
        return "upload".to_string();
    }

    // Rebuilt rather than assigned in place: `stem`/`ext` borrow `out`.
    let escaped = {
        let (stem, ext) = split_ext(&out);
        if DEVICE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(stem)) {
            format!("{stem}_{ext}")
        } else {
            out.clone()
        }
    };

    truncate(&escaped)
}

/// `("report", ".txt")`. A leading dot is part of the stem, so `.gitignore`
/// keeps its name rather than becoming an extension.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Trim the stem, never the extension, and never mid-character.
fn truncate(name: &str) -> String {
    if name.len() <= MAX_LEN {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    let room = MAX_LEN.saturating_sub(ext.len());
    let mut end = room.min(stem.len());
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &stem[..end], ext)
}

/// A name that does not yet exist in `dir`. A transfer must never destroy a
/// file that is already there, so this suffixes rather than overwrites.
pub fn unique_in(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name);
    for n in 2..1000 {
        let candidate = format!("{stem} ({n}){ext}");
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem} {}{ext}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_every_path_component() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(r"..\..\windows\system32\cmd.exe"), "cmd.exe");
        assert_eq!(safe_name("/absolute/path/song.mp3"), "song.mp3");
    }

    #[test]
    fn refuses_names_that_are_only_dots() {
        assert_eq!(safe_name("."), "upload");
        assert_eq!(safe_name(".."), "upload");
        assert_eq!(safe_name(""), "upload");
    }

    #[test]
    fn replaces_characters_windows_rejects() {
        assert_eq!(safe_name(r#"a<b>c:d"e|f?g*h.txt"#), "a_b_c_d_e_f_g_h.txt");
        assert_eq!(safe_name("bell\u{7}null\u{0}.txt"), "bell_null_.txt");
    }

    #[test]
    fn strips_trailing_dots_and_spaces_windows_would_drop() {
        // Windows silently trims these, so "a.txt." and "a.txt" are the same
        // file there and different files everywhere else.
        assert_eq!(safe_name("report.txt..."), "report.txt");
        assert_eq!(safe_name("report.txt   "), "report.txt");
    }

    #[test]
    fn escapes_windows_device_names_with_or_without_extension() {
        assert_eq!(safe_name("CON"), "CON_");
        assert_eq!(safe_name("con.txt"), "con_.txt");
        assert_eq!(safe_name("COM9.mp3"), "COM9_.mp3");
        assert_eq!(safe_name("LPT1"), "LPT1_");
        // Not a device name: only COM1-COM9 and LPT1-LPT9 are reserved.
        assert_eq!(safe_name("COM10.txt"), "COM10.txt");
        assert_eq!(safe_name("CONCERT.mp3"), "CONCERT.mp3");
    }

    #[test]
    fn caps_length_but_keeps_the_extension() {
        let long = format!("{}.mp3", "a".repeat(400));
        let out = safe_name(&long);
        assert!(out.len() <= 200, "{} bytes", out.len());
        assert!(out.ends_with(".mp3"), "{out}");
    }

    #[test]
    fn never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"first").unwrap();
        assert_eq!(unique_in(dir.path(), "a.txt"), "a (2).txt");

        std::fs::write(dir.path().join("a (2).txt"), b"second").unwrap();
        assert_eq!(unique_in(dir.path(), "a.txt"), "a (3).txt");

        assert_eq!(unique_in(dir.path(), "fresh.txt"), "fresh.txt");
    }

    #[test]
    fn suffixes_a_name_that_has_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(unique_in(dir.path(), "README"), "README (2)");
    }
}
