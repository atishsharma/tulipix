//! Folder chores — the four File ops that are arithmetic over a directory
//! listing and nothing else.
//!
//! Every one of them is planned before it is run, and the plan is what the
//! preview draws: `duplicates` returns the groups, `sort_plan` returns the
//! moves, `empty_dirs` returns the folders. The worker then carries out
//! exactly the list the user read. Nothing here writes to disk except
//! [`apply_moves`] and [`remove_dirs`], which are the two that say so in
//! their names.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::folder_diff::walk;

/// One set of files with identical contents. `paths` is sorted, so the first
/// is stable across runs and can be called "the one to keep".
#[derive(Debug, Clone, PartialEq)]
pub struct Dup {
    pub bytes: u64,
    pub paths: Vec<String>,
}

impl Dup {
    /// What deleting every copy but the first would free.
    pub fn wasted(&self) -> u64 {
        self.bytes * (self.paths.len().saturating_sub(1)) as u64
    }
}

/// Files under `root` that share contents.
///
/// Size first, hash second: two files of different lengths cannot be equal, so
/// a folder of ten thousand photos costs ten thousand `stat` calls and a
/// handful of reads. Hashing everything would be correct and would also read
/// the whole library off the disk to answer a question that size settles.
pub fn duplicates(root: &str, cap: usize) -> Vec<Dup> {
    let listing = walk(root, cap);
    let mut by_size: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for (rel, (bytes, _)) in listing {
        // Empty files are all identical and never interesting.
        if bytes == 0 {
            continue;
        }
        by_size
            .entry(bytes)
            .or_default()
            .push(format!("{}/{}", root.trim_end_matches('/'), rel));
    }

    let mut out: Vec<Dup> = Vec::new();
    for (bytes, paths) in by_size {
        if paths.len() < 2 {
            continue;
        }
        let mut by_hash: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for p in paths {
            let Some(digest) = memoised_hash(&p) else {
                continue;
            };
            by_hash.entry(digest).or_default().push(p);
        }
        for (_, mut group) in by_hash {
            if group.len() < 2 {
                continue;
            }
            group.sort();
            out.push(Dup {
                bytes,
                paths: group,
            });
        }
    }
    // Biggest waste first: that is the order someone reclaiming space reads in.
    out.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then(a.paths[0].cmp(&b.paths[0]))
    });
    out
}

/// Which folder a file is filed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    /// `JPG/`, `PDF/`, `no extension/`.
    Extension,
    /// `2026/2026-09/` from the modification time.
    Date,
    /// `A/`, `B/`, `#/` from the first character.
    Letter,
}

impl SortBy {
    pub fn parse(s: &str) -> SortBy {
        match s {
            "date" => SortBy::Date,
            "letter" => SortBy::Letter,
            _ => SortBy::Extension,
        }
    }
}

/// Where every loose file in `dir` would go. Never recurses: sorting a tree
/// into folders and then sorting those folders again is not what anyone means.
///
/// Returns `(from, to)` absolute paths, sorted by destination so the preview
/// reads as the folders it is about to make.
pub fn sort_plan(dir: &str, by: SortBy, cap: usize) -> Vec<(String, String)> {
    let root = dir.trim_end_matches('/');
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten().take(cap) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let folder = match by {
            SortBy::Extension => path
                .extension()
                .map(|e| e.to_string_lossy().to_uppercase())
                .unwrap_or_else(|| "no extension".into()),
            SortBy::Letter => {
                let first = name.chars().next().unwrap_or('#').to_ascii_uppercase();
                if first.is_ascii_alphabetic() {
                    first.to_string()
                } else {
                    "#".into()
                }
            }
            SortBy::Date => month_folder(&path),
        };
        out.push((
            path.to_string_lossy().to_string(),
            format!("{root}/{folder}/{name}"),
        ));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// `2026/2026-09` from a file's modification time, in UTC.
///
/// Days are not worth the calendar arithmetic here and months are: a folder
/// per day of a decade is thirty-six hundred folders.
fn month_folder(path: &Path) -> String {
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m) = year_month(secs);
    format!("{y:04}/{y:04}-{m:02}")
}

/// Civil year and month from a Unix timestamp, UTC.
///
/// Howard Hinnant's `civil_from_days`, which is exact for every date this will
/// ever see and needs no crate. Pulling in a date library to name a folder
/// would be the tail wagging the dog.
fn year_month(secs: i64) -> (i64, u32) {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32)
}

/// Carry out a [`sort_plan`], creating the destination folders as it goes.
///
/// `progress` is called after each move and returning false stops the run —
/// half a sort is a mess, but a cancel that ignores you is worse.
pub fn apply_moves(
    moves: &[(String, String)],
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<usize> {
    let total = moves.len();
    let mut done = 0usize;
    for (from, to) in moves {
        if let Some(parent) = Path::new(to).parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not make {}", parent.display()))?;
        }
        // Same filesystem in every realistic case, and a rename is atomic.
        // Across a mount point it fails with EXDEV, and then it is a copy.
        if std::fs::rename(from, to).is_err() {
            std::fs::copy(from, to).with_context(|| format!("could not move {from}"))?;
            std::fs::remove_file(from).ok();
        }
        done += 1;
        if !progress(done, total) {
            break;
        }
    }
    Ok(done)
}

/// Every folder under `root` that holds no files at any depth, deepest first.
///
/// Deepest first is what makes one pass enough: emptying `a/b/c` is what makes
/// `a/b` empty, and a list built shallowest-first would have to be walked
/// again.
pub fn empty_dirs(root: &str, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_empty(Path::new(root), &mut out, cap, 0);
    // Never offer to delete the folder the user pointed at.
    out.retain(|p| p != root.trim_end_matches('/'));
    out.sort_by_key(|p| std::cmp::Reverse(p.matches('/').count()));
    out
}

/// Returns whether `dir` holds a file anywhere beneath it.
fn collect_empty(dir: &Path, out: &mut Vec<String>, cap: usize, depth: usize) -> bool {
    if depth > 40 || out.len() >= cap {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        // Unreadable is not empty. Deleting what we could not look inside is
        // the one outcome this must never produce.
        return true;
    };
    let mut has_files = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if collect_empty(&path, out, cap, depth + 1) {
                has_files = true;
            }
        } else {
            has_files = true;
        }
    }
    if !has_files {
        out.push(dir.to_string_lossy().to_string());
    }
    has_files
}

/// Delete folders, in the order given.
pub fn remove_dirs(dirs: &[String]) -> usize {
    dirs.iter()
        .filter(|d| std::fs::remove_dir(d).is_ok())
        .count()
}

/// Every file under `root` as CSV: path, bytes, modified.
///
/// The header is written even when the folder is empty, because a CSV with no
/// header is a file nothing will open.
pub fn listing_csv(root: &str, cap: usize) -> String {
    let mut out = String::from("path,bytes,modified\n");
    for (rel, (bytes, mtime)) in walk(root, cap) {
        let (y, m) = year_month(mtime);
        out.push_str(&format!("{},{bytes},{y:04}-{m:02}\n", csv_field(&rel)));
    }
    out
}

/// RFC 4180: quote when the value holds a comma, a quote or a newline, and
/// double the quotes inside.
fn csv_field(v: &str) -> String {
    if v.contains(',') || v.contains('"') || v.contains('\n') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// A file's hash, remembered for as long as the app runs.
///
/// The preview and the run call `duplicates` with the same folder, and a
/// slider or a toggle moving re-runs the preview — so without this, ticking
/// "delete the extra copies" would re-read every candidate off the disk. The
/// key includes size and modification time, so a file that changes is hashed
/// again rather than remembered wrongly.
fn memoised_hash(path: &str) -> Option<String> {
    use std::sync::{Mutex, OnceLock};

    static SEEN: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    let cache = SEEN.get_or_init(Default::default);

    let meta = std::fs::metadata(path).ok()?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = format!("{path}|{}|{stamp}", meta.len());

    if let Ok(guard) = cache.lock() {
        if let Some(known) = guard.get(&key) {
            return Some(known.clone());
        }
    }
    let digest = crate::hash::sha256_file_hex(path).ok()?;
    if let Ok(mut guard) = cache.lock() {
        // A folder of a million files would otherwise grow this without limit;
        // clearing beats evicting, since the next walk refills what it needs.
        if guard.len() > 100_000 {
            guard.clear();
        }
        guard.insert(key, digest.clone());
    }
    Some(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &[u8]) -> String {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn duplicates_group_by_contents_not_by_name() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a.txt", b"hello");
        write(d.path(), "nested/b.txt", b"hello");
        write(d.path(), "c.txt", b"different");
        // Same length as "different", different contents — the size pass has
        // to hand this to the hash pass rather than call it a duplicate.
        write(d.path(), "d.txt", b"difFerent");

        let dups = duplicates(d.path().to_str().unwrap(), 1000);
        assert_eq!(dups.len(), 1);
        assert_eq!(dups[0].paths.len(), 2);
        assert!(
            dups[0]
                .paths
                .iter()
                .all(|p| p.ends_with("a.txt") || p.ends_with("b.txt"))
        );
        assert_eq!(dups[0].wasted(), 5);
    }

    #[test]
    fn an_empty_file_is_not_a_duplicate_of_every_other_empty_file() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "a.log", b"");
        write(d.path(), "b.log", b"");
        assert!(duplicates(d.path().to_str().unwrap(), 1000).is_empty());
    }

    #[test]
    fn sorting_by_extension_names_the_folder_after_it() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "one.JPG", b"x");
        write(d.path(), "two.jpg", b"x");
        write(d.path(), "readme", b"x");
        let plan = sort_plan(d.path().to_str().unwrap(), SortBy::Extension, 1000);
        assert_eq!(plan.len(), 3);
        // Case folds together: JPG and jpg are one folder, not two.
        assert_eq!(
            plan.iter().filter(|(_, to)| to.contains("/JPG/")).count(),
            2
        );
        assert!(plan.iter().any(|(_, to)| to.contains("/no extension/")));
    }

    #[test]
    fn the_civil_calendar_lands_on_the_right_month() {
        // 2026-09-01T00:00:00Z, and the last second of the month before it.
        assert_eq!(year_month(1_788_220_800), (2026, 9));
        assert_eq!(year_month(1_788_220_799), (2026, 8));
        assert_eq!(year_month(0), (1970, 1));
        // A leap day, which is where naive month arithmetic goes wrong.
        assert_eq!(year_month(1_709_164_800), (2024, 2));
        // 1 March, and the second before it. Hinnant's year starts in March so
        // that the leap day falls at the end of it, which makes this boundary
        // the one where an off-by-one in the shift shows up as a wrong year.
        assert_eq!(year_month(1_772_323_200), (2026, 3));
        assert_eq!(year_month(1_772_323_199), (2026, 2));
    }

    #[test]
    fn empty_folders_come_back_deepest_first_and_never_include_the_root() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b/c")).unwrap();
        std::fs::create_dir_all(d.path().join("keep")).unwrap();
        write(d.path(), "keep/file.txt", b"x");

        let empties = empty_dirs(d.path().to_str().unwrap(), 1000);
        assert!(!empties.iter().any(|p| p.ends_with("keep")));
        assert!(!empties.contains(&d.path().to_string_lossy().to_string()));
        // a/b/c is deeper than a/b, which is deeper than a.
        assert!(empties[0].ends_with("a/b/c"));
        assert_eq!(empties.len(), 3);
    }

    #[test]
    fn a_listing_quotes_the_commas_it_finds() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
