//! The Archive section's store: a catalogue of the folders and drives you add,
//! what is inside their zips, checksums, and what those say about duplicates
//! and files that changed on their own. `api::archive` scans and maps.

use std::path::{Path, PathBuf};

use anyhow::Result;
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS roots (
    id      INTEGER PRIMARY KEY,
    path    TEXT    NOT NULL UNIQUE,
    label   TEXT    NOT NULL DEFAULT '',
    added   INTEGER NOT NULL,
    scanned INTEGER NOT NULL DEFAULT 0,
    checked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS files (
    id      INTEGER PRIMARY KEY,
    root_id INTEGER NOT NULL,
    path    TEXT    NOT NULL UNIQUE,
    name    TEXT    NOT NULL,
    kind    TEXT    NOT NULL,
    size    INTEGER NOT NULL,
    mtime   INTEGER NOT NULL,
    sha     TEXT    NOT NULL DEFAULT '',   -- SHA-256, once taken
    sha_at  INTEGER NOT NULL DEFAULT 0,    -- the mtime it was taken at
    state   TEXT    NOT NULL DEFAULT ''    -- '' | changed | missing | broken
);
CREATE INDEX IF NOT EXISTS files_root_idx ON files(root_id);
CREATE INDEX IF NOT EXISTS files_size_idx ON files(size);
CREATE INDEX IF NOT EXISTS files_sha_idx ON files(sha);

-- What a zip holds, so a search finds a file inside one.
CREATE TABLE IF NOT EXISTS entries (
    file_id INTEGER NOT NULL,
    name    TEXT    NOT NULL,
    size    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS entries_file_idx ON entries(file_id);

-- Names, of files (inner '') and of what is inside zips (inner = the name).
CREATE VIRTUAL TABLE IF NOT EXISTS names_fts USING fts5(
    name, file_id UNINDEXED, inner UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The kinds the library groups by, with their labels.
pub const KINDS: &[(&str, &str)] = &[
    ("archive", "Archives"),
    ("disk", "Disk images"),
    ("installer", "Installers"),
    ("document", "Documents"),
    ("picture", "Pictures"),
    ("video", "Videos"),
    ("audio", "Audio"),
    ("other", "Other"),
];

/// A file's kind, from its name.
pub fn kind_of(name: &str) -> &'static str {
    let low = name.to_lowercase();
    // Two-part endings first: a .tar.gz is an archive, not a gz.
    if [".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2"].iter().any(|e| low.ends_with(e)) {
        return "archive";
    }
    let ext = low.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "cbz" | "cbr" => "archive",
        "iso" | "img" | "dmg" | "vhd" | "vhdx" | "qcow2" | "vmdk" => "disk",
        "deb" | "rpm" | "appimage" | "exe" | "msi" | "apk" | "pkg" | "flatpakref" | "snap" => "installer",
        "pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "txt" | "md" | "rtf" | "csv"
        | "epub" | "json" | "xml" | "html" => "document",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "raw" | "cr2" | "nef" | "dng" | "svg" => {
            "picture"
        }
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" | "flv" => "video",
        "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wav" | "wma" => "audio",
        _ => "other",
    }
}

/// Folders a catalogue does not go into: hidden ones, and the build and cache
/// folders that hold thousands of files nobody archives.
pub fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(name, "node_modules" | "target" | "__pycache__" | "venv" | "Cache" | "cache" | "$RECYCLE.BIN" | "System Volume Information")
}

/// Every file under `root` as (path, size, mtime), not following links.
pub fn walk(root: &Path) -> Vec<(PathBuf, i64, i64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                if !skip_dir(&name) {
                    stack.push(e.path());
                }
            } else if ft.is_file()
                && let Ok(m) = e.metadata()
            {
                let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
                out.push((e.path(), m.len() as i64, mtime));
            }
        }
    }
    out
}

/// SHA-256 of a file, as hex, read in pieces.
pub fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Read every member of a zip through, which checks each one's CRC. The
/// reason, when one does not read.
pub fn test_zip(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut z = match zip::ZipArchive::new(file) {
        Ok(z) => z,
        Err(e) => return Some(format!("not readable as a zip: {e}")),
    };
    let mut sink = vec![0u8; 1 << 16];
    for i in 0..z.len() {
        let mut m = match z.by_index(i) {
            Ok(m) => m,
            Err(e) => return Some(format!("member {i} unreadable: {e}")),
        };
        let name = m.name().to_string();
        loop {
            match m.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Some(format!("{name}: {e}")),
            }
        }
    }
    None
}

/// "212 KB", "1.2 GB".
pub fn size_label(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    match b {
        b if b < 1024.0 => format!("{b:.0} B"),
        b if b < 1024.0 * 1024.0 => format!("{:.0} KB", b / 1024.0),
        b if b < 1024.0 * 1024.0 * 1024.0 => format!("{:.1} MB", b / 1024.0 / 1024.0),
        b => format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_name() {
        assert_eq!(kind_of("accounts-2019.7z"), "archive");
        assert_eq!(kind_of("backup.TAR.GZ"), "archive");
        assert_eq!(kind_of("Fedora-43.iso"), "disk");
        assert_eq!(kind_of("discord-0.0.98.deb"), "installer");
        assert_eq!(kind_of("invoice.pdf"), "document");
        assert_eq!(kind_of("README"), "other");
        assert!(skip_dir(".git") && skip_dir("node_modules") && !skip_dir("Photos"));
        assert_eq!(size_label(212 * 1024), "212 KB");
        assert_eq!(size_label(1_288_490_189), "1.2 GB");
    }

    #[test]
    fn a_walk_a_hash_and_a_zip_test() {
        let dir = std::env::temp_dir().join(format!("tulipix-archive-{}", now()));
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join(".hidden").join("b.txt"), b"x").unwrap();
        let files = walk(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(sha256(&dir.join("a.txt")).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // A zip with one member reads clean; the same bytes cut short do not.
        let zpath = dir.join("t.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zpath).unwrap());
            z.start_file("hello.txt", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut z, &[b'h'; 4000]).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(test_zip(&zpath), None);
        let bytes = std::fs::read(&zpath).unwrap();
        std::fs::write(&zpath, &bytes[..bytes.len() / 2]).unwrap();
        assert!(test_zip(&zpath).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
