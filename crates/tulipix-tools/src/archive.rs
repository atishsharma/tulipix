//! `np.p4.tools.archive` — create, list and extract zip archives.
//!
//! On the `zip` crate the workspace already builds for EPUB and CBZ, at the
//! same version and the same `deflate`-only feature set, so this adds a use
//! rather than a dependency.
//!
//! **Listing is separate from extracting** because the preview needs the
//! listing and must not write anything: [`list`] opens the archive, reads the
//! central directory and closes it. The pane shows what would come out before
//! anything comes out.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// One member of an archive, as the preview and the extractor both see it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The name as stored, which is what a person recognises.
    pub name: String,
    /// Where it would actually be written, relative to the destination —
    /// `None` for a name that tries to escape it.
    pub safe: Option<String>,
    pub bytes: u64,
    pub packed: u64,
    pub is_dir: bool,
}

/// What is inside an archive, without unpacking any of it.
pub fn list(path: &str) -> Result<Vec<Entry>> {
    let file = std::fs::File::open(path).with_context(|| format!("{path} could not be opened"))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("{path} is not a zip archive this can read"))?;
    let mut out = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else { continue };
        out.push(Entry {
            name: entry.name().to_string(),
            // `enclosed_name` is the crate's own answer to zip-slip: it returns
            // None for anything with `..` or an absolute root in it. A member
            // named `../../.bashrc` is the reason this is not just the name.
            safe: entry
                .enclosed_name()
                .map(|p| p.to_string_lossy().to_string()),
            bytes: entry.size(),
            packed: entry.compressed_size(),
            is_dir: entry.is_dir(),
        });
    }
    Ok(out)
}

/// Every file under `root`, as `(absolute path, name inside the archive)`.
pub fn walk_for_archive(root: &str, cap: usize) -> Vec<(String, String)> {
    let base = Path::new(root);
    let stem = base
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "archive".into());
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            if out.len() >= cap {
                return out;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base) {
                // Nested under the folder's own name, so extracting does not
                // spray the contents across whatever directory you are in.
                let name = format!("{stem}/{}", rel.to_string_lossy().replace('\\', "/"));
                out.push((path.to_string_lossy().to_string(), name));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// The members a "create" job writes, from whichever source the form filled in.
///
/// The preview lists exactly this, so what you see is what goes in.
pub fn members(folder: &str, files: &[String], cap: usize) -> Vec<(String, String)> {
    if !folder.trim().is_empty() {
        return walk_for_archive(folder, cap);
    }
    files
        .iter()
        .take(cap)
        .map(|f| {
            let name = Path::new(f)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| f.clone());
            (f.clone(), name)
        })
        .collect()
}

fn options(store: bool) -> zip::write::SimpleFileOptions {
    let method = if store {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    };
    zip::write::SimpleFileOptions::default().compression_method(method)
}

/// Write `members` into a new zip at `out`. Returns how many went in.
///
/// `progress` is called after each member with `(done, total)` and returns
/// false to stop — the caller's cancel check, kept out of this module so it
/// stays free of the app's runtime.
pub fn create(
    members: &[(String, String)],
    out: &str,
    store: bool,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<usize> {
    if members.is_empty() {
        bail!("nothing to put in the archive");
    }
    let file = std::fs::File::create(out).with_context(|| format!("{out} could not be written"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = options(store);
    let mut done = 0usize;
    for (path, name) in members {
        if !progress(done, members.len()) {
            bail!("cancelled");
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        zip.start_file(name.clone(), opts)
            .with_context(|| format!("{name} could not be added"))?;
        zip.write_all(&bytes)
            .with_context(|| format!("{name} could not be written"))?;
        done += 1;
    }
    zip.finish().context("the archive could not be closed")?;
    Ok(done)
}

/// Unpack into `dir`. Members whose names try to escape it are skipped, not
/// written somewhere else and not treated as an error the whole job fails on.
pub fn extract(
    input: &str,
    dir: &str,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<(usize, usize)> {
    let file =
        std::fs::File::open(input).with_context(|| format!("{input} could not be opened"))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("{input} is not a zip archive this can read"))?;
    let root = PathBuf::from(dir);
    std::fs::create_dir_all(&root).with_context(|| format!("{dir} could not be created"))?;

    let total = zip.len();
    let (mut written, mut refused) = (0usize, 0usize);
    for i in 0..total {
        if !progress(i, total) {
            bail!("cancelled");
        }
        let Ok(mut entry) = zip.by_index(i) else {
            continue;
        };
        let Some(rel) = entry.enclosed_name() else {
            refused += 1;
            continue;
        };
        let target = root.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).ok();
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut bytes).is_err() {
            refused += 1;
            continue;
        }
        if std::fs::write(&target, &bytes).is_ok() {
            written += 1;
        } else {
            refused += 1;
        }
    }
    Ok((written, refused))
}

/// Read an archive and write it out again at a different compression. Used to
/// squeeze a stored archive, or to open one up for a system that cannot inflate.
pub fn repack(
    input: &str,
    out: &str,
    store: bool,
    mut progress: impl FnMut(usize, usize) -> bool,
) -> Result<usize> {
    if input == out {
        bail!("repack cannot write over the archive it is reading");
    }
    let file =
        std::fs::File::open(input).with_context(|| format!("{input} could not be opened"))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("{input} is not a zip archive this can read"))?;
    let target =
        std::fs::File::create(out).with_context(|| format!("{out} could not be written"))?;
    let mut writer = zip::ZipWriter::new(target);
    let opts = options(store);

    let total = zip.len();
    let mut done = 0usize;
    for i in 0..total {
        if !progress(i, total) {
            bail!("cancelled");
        }
        let Ok(mut entry) = zip.by_index(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let name = rel.to_string_lossy().to_string();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        writer.start_file(name, opts)?;
        writer.write_all(&bytes)?;
        done += 1;
    }
    writer.finish().context("the archive could not be closed")?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &Path) -> String {
        let path = dir.join("sample.zip");
        let members = vec![
            (write_temp(dir, "a.txt", b"hello"), "a.txt".to_string()),
            (
                write_temp(dir, "b.txt", b"world"),
                "notes/b.txt".to_string(),
            ),
        ];
        create(&members, &path.to_string_lossy(), false, |_, _| true).unwrap();
        path.to_string_lossy().to_string()
    }

    fn write_temp(dir: &Path, name: &str, body: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn round_trips_through_a_real_archive() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = sample(dir.path());

        let entries = list(&zip_path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].name, "notes/b.txt");
        assert_eq!(entries[1].bytes, 5);
        assert!(entries.iter().all(|e| e.safe.is_some()));

        let out = dir.path().join("out");
        let (written, refused) = extract(&zip_path, &out.to_string_lossy(), |_, _| true).unwrap();
        assert_eq!((written, refused), (2, 0));
        assert_eq!(std::fs::read(out.join("notes/b.txt")).unwrap(), b"world");
    }

    #[test]
    fn a_folder_goes_in_under_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("holiday");
        std::fs::create_dir_all(src.join("raw")).unwrap();
        std::fs::write(src.join("one.jpg"), b"1").unwrap();
        std::fs::write(src.join("raw/two.dng"), b"2").unwrap();

        let nested = members(&src.to_string_lossy(), &[], 100);
        let names: Vec<&str> = nested.iter().map(|(_, n)| n.as_str()).collect();
        // Nested, so extracting does not spray the contents across the folder
        // you happen to be standing in.
        assert_eq!(names, vec!["holiday/one.jpg", "holiday/raw/two.dng"]);

        // A file list keeps bare names instead.
        let loose = members("", &["/some/where/x.txt".to_string()], 100);
        assert_eq!(loose[0].1, "x.txt");
    }

    #[test]
    fn cancelling_stops_between_members() {
        let dir = tempfile::tempdir().unwrap();
        let members = vec![
            (write_temp(dir.path(), "a.txt", b"a"), "a.txt".into()),
            (write_temp(dir.path(), "b.txt", b"b"), "b.txt".into()),
        ];
        let out = dir.path().join("half.zip");
        let err = create(&members, &out.to_string_lossy(), false, |done, _| done == 0).unwrap_err();
        assert!(err.to_string().contains("cancelled"));
    }

    #[test]
    fn repack_will_not_write_over_what_it_is_reading() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = sample(dir.path());
        assert!(repack(&zip_path, &zip_path, true, |_, _| true).is_err());

        let stored = dir.path().join("stored.zip");
        let n = repack(&zip_path, &stored.to_string_lossy(), true, |_, _| true).unwrap();
        assert_eq!(n, 2);
        assert_eq!(list(&stored.to_string_lossy()).unwrap().len(), 2);
    }
}
