//! `np.p4.tools.rename` — batch rename pattern engine.
//!
//! Pattern tokens `{tag}` substitute from per-file metadata (EXIF/ID3/MP4),
//! `{n}` / `{n:03}` insert a zero-padded sequence number, and the extension is
//! preserved. Produces a dry-run preview of `(old, new)` pairs and flags
//! collisions before anything touches disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Expand a pattern for one file. `tags` supplies `{tag}` values; `seq` feeds
/// `{n}` / `{n:0W}`. Unknown tags expand to empty. Extension is appended from
/// the source path.
pub fn expand(pattern: &str, tags: &HashMap<String, String>, seq: usize, src: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut token = String::new();
            while let Some(&n) = chars.peek() {
                chars.next();
                if n == '}' { break; }
                token.push(n);
            }
            out.push_str(&expand_token(&token, tags, seq));
        } else {
            out.push(c);
        }
    }
    let stem = sanitize(&out);
    match Path::new(src).extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    }
}

fn expand_token(token: &str, tags: &HashMap<String, String>, seq: usize) -> String {
    if let Some(width) = token.strip_prefix("n:0") {
        let w: usize = width.parse().unwrap_or(0);
        return format!("{seq:0w$}", w = w);
    }
    if token == "n" { return seq.to_string(); }
    tags.get(token).cloned().unwrap_or_default()
}

/// Strip characters illegal in filenames across OSes.
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect::<String>().trim().to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub renames: Vec<(String, String)>, // (old_path, new_name)
    pub collisions: Vec<String>,        // new names produced more than once
}

/// The files a rename operates on: every file directly in `dir`, sorted by
/// path, each carrying its own stem as the `{name}` tag.
///
/// Both the preview and the run call this. They used to read the directory
/// each their own way, which is the difference that makes a preview show
/// `shot_01` where the run writes `shot_02`.
pub fn scan(dir: &str) -> std::io::Result<Vec<(String, HashMap<String, String>)>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect();
    paths.sort();
    Ok(paths.iter().map(|p| {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut tags = HashMap::new();
        tags.insert("name".to_string(), stem);
        (p.to_string_lossy().to_string(), tags)
    }).collect())
}

/// Dry-run a batch. `files`: `(path, tags)`. Sequence starts at `start`.
pub fn dry_run(files: &[(String, HashMap<String, String>)], pattern: &str, start: usize) -> Preview {
    let mut renames = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, (path, tags)) in files.iter().enumerate() {
        let new = expand(pattern, tags, start + i, path);
        *seen.entry(new.clone()).or_default() += 1;
        renames.push((path.clone(), new));
    }
    let collisions = seen.into_iter().filter(|(_, c)| *c > 1).map(|(k, _)| k).collect();
    Preview { renames, collisions }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn expands_tags_seq_and_ext() {
        let t = tags(&[("artist", "Band"), ("title", "Song")]);
        let n = expand("{artist} - {title} {n:03}", &t, 7, "/m/x.flac");
        assert_eq!(n, "Band - Song 007.flac");
    }

    #[test]
    fn sanitizes_illegal_chars() {
        let t = tags(&[("title", "a/b:c")]);
        assert_eq!(expand("{title}", &t, 1, "x.mp3"), "a_b_c.mp3");
    }

    #[test]
    fn detects_collisions() {
        let files = vec![
            ("/a.jpg".to_string(), tags(&[("d", "2020")])),
            ("/b.jpg".to_string(), tags(&[("d", "2020")])),
        ];
        let p = dry_run(&files, "{d}", 1); // both → "2020.jpg"
        assert_eq!(p.collisions, vec!["2020.jpg"]);
        // with a sequence, no collision
        let p2 = dry_run(&files, "{d}-{n}", 1);
        assert!(p2.collisions.is_empty());
    }
}
