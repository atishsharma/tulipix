//! Migration-import wizard. Reads from a foreign library (Plex
//! `com.plexapp.plugins.library.db`, Picasa `.picasa.ini`, iTunes
//! XML, foobar2000 database, Lightroom `.lrcat`), parses the bits we
//! map onto Tulipix's proxy model, and returns a `MigrationPlan` for
//! dry-run review before commit. The actual writes land in the
//! per-section crates; this module only plans.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind { Plex, Picasa, ITunesXml, Foobar2000, Lightroom }

impl SourceKind {
    pub fn detect(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if name.ends_with(".lrcat") { return Some(Self::Lightroom); }
        if name == ".picasa.ini" || name == "picasa.ini" { return Some(Self::Picasa); }
        if name == "itunes music library.xml" || name == "itunes library.xml" || name.ends_with("itunes-library.xml") {
            return Some(Self::ITunesXml);
        }
        if name == "com.plexapp.plugins.library.db" { return Some(Self::Plex); }
        if name.ends_with(".fb2k-dsp") || name == "library.dat" { return Some(Self::Foobar2000); }
        None
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MigrationPlan {
    pub source: Option<SourceKind>,
    pub source_path: Option<PathBuf>,
    pub photos: u32,
    pub videos: u32,
    pub music_tracks: u32,
    pub albums: u32,
    pub playlists: u32,
    pub ratings: u32,
    pub face_clusters: u32,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
}

pub fn dry_run(source: SourceKind, src: &Path) -> Result<MigrationPlan> {
    let mut plan = MigrationPlan { source: Some(source), source_path: Some(src.to_path_buf()), ..Default::default() };
    if !src.exists() { return Err(anyhow!("source not found: {}", src.display())); }
    match source {
        SourceKind::Picasa     => plan_picasa(src, &mut plan)?,
        SourceKind::ITunesXml  => plan_itunes(src, &mut plan)?,
        SourceKind::Plex       => plan_plex(src, &mut plan)?,
        SourceKind::Lightroom  => plan_lightroom(src, &mut plan)?,
        SourceKind::Foobar2000 => plan_foobar(src, &mut plan)?,
    }
    Ok(plan)
}

fn plan_picasa(path: &Path, plan: &mut MigrationPlan) -> Result<()> {
    // .picasa.ini layout: [<filename>]\nstar=yes\nfaces=rect64(...)|<personid>\n
    // Multiple file sections per dir; we count distinct file-section headers and
    // any face= line as a clustered face.
    let text = std::fs::read_to_string(path)?;
    let mut current_is_file = false;
    let mut faces = 0u32;
    for line in text.lines() {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current_is_file = !inner.starts_with("Picasa") && !inner.starts_with("Contacts2") && !inner.is_empty();
            if current_is_file && is_image_name(inner) { plan.photos += 1; }
            continue;
        }
        if current_is_file {
            if t.starts_with("star=yes") { plan.ratings += 1; }
            if t.starts_with("faces=") { faces += 1; }
        }
    }
    plan.face_clusters = faces;
    Ok(())
}

fn plan_itunes(path: &Path, plan: &mut MigrationPlan) -> Result<()> {
    // iTunes Library XML is an Apple plist — count <key>Tracks</key> dict entries
    // and <key>Playlists</key> array entries by tag tally. Robust enough for dry-run.
    let text = std::fs::read_to_string(path)?;
    let tracks = text.matches("<key>Track ID</key>").count() as u32;
    let playlists = text.matches("<key>Playlist ID</key>").count() as u32;
    let rated = text.matches("<key>Rating</key>").count() as u32;
    plan.music_tracks = tracks;
    plan.playlists = playlists;
    plan.ratings = rated;
    Ok(())
}

fn plan_plex(path: &Path, plan: &mut MigrationPlan) -> Result<()> {
    // Plex ships SQLite. We can't pull rusqlite into core without bloat, so
    // dry-run counts file-magic + reports the schema as a warning until the
    // dedicated importer runs.
    let mut buf = [0u8; 16];
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut buf)?;
    if n < 16 || &buf[..15] != b"SQLite format 3" {
        return Err(anyhow!("not a SQLite db: {}", path.display()));
    }
    plan.warnings.push("Plex library detected; full row counts require SQLite open (Phase 2 importer).".into());
    Ok(())
}

fn plan_lightroom(path: &Path, plan: &mut MigrationPlan) -> Result<()> {
    let mut buf = [0u8; 16];
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut buf)?;
    if n < 16 || &buf[..15] != b"SQLite format 3" {
        return Err(anyhow!("lrcat is not SQLite: {}", path.display()));
    }
    plan.warnings.push("Lightroom catalog detected; collections + virtual copies import in Phase 2.".into());
    Ok(())
}

fn plan_foobar(path: &Path, plan: &mut MigrationPlan) -> Result<()> {
    // foobar2000 library is a proprietary binary stream — we only confirm the file
    // exists and report a warning. Real parsing lands with the music importer.
    let size = std::fs::metadata(path)?.len();
    if size == 0 { return Err(anyhow!("empty foobar2000 db")); }
    plan.warnings.push(format!("foobar2000 db detected ({} bytes); parser ships with music importer.", size));
    Ok(())
}

fn is_image_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".heic", ".webp", ".tif", ".tiff", ".raf", ".cr2", ".cr3", ".nef", ".dng", ".arw"]
        .iter().any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    #[test] fn detects_picasa() {
        assert_eq!(SourceKind::detect(Path::new("/x/.picasa.ini")), Some(SourceKind::Picasa));
        assert_eq!(SourceKind::detect(Path::new("/x/iTunes Music Library.xml")), Some(SourceKind::ITunesXml));
        assert_eq!(SourceKind::detect(Path::new("/x/something.lrcat")), Some(SourceKind::Lightroom));
        assert_eq!(SourceKind::detect(Path::new("/x/com.plexapp.plugins.library.db")), Some(SourceKind::Plex));
    }
    #[test] fn picasa_plan_counts_files_stars_faces() {
        let d = tempdir().unwrap();
        let p = d.path().join(".picasa.ini");
        std::fs::write(&p, "[Picasa]\nname=Camera\n\n[IMG_001.JPG]\nstar=yes\nfaces=rect64(00)|abc\n\n[IMG_002.png]\nfaces=rect64(00)|def\n").unwrap();
        let plan = dry_run(SourceKind::Picasa, &p).unwrap();
        assert_eq!(plan.photos, 2);
        assert_eq!(plan.ratings, 1);
        assert_eq!(plan.face_clusters, 2);
    }
    #[test] fn itunes_xml_counts_tags() {
        let d = tempdir().unwrap();
        let p = d.path().join("itunes-library.xml");
        std::fs::write(&p, "<plist><dict><key>Tracks</key><dict>\n<key>Track ID</key><integer>1</integer><key>Rating</key><integer>100</integer>\n<key>Track ID</key><integer>2</integer>\n</dict><key>Playlists</key><array><dict><key>Playlist ID</key><integer>9</integer></dict></array></dict></plist>").unwrap();
        let plan = dry_run(SourceKind::ITunesXml, &p).unwrap();
        assert_eq!(plan.music_tracks, 2);
        assert_eq!(plan.playlists, 1);
        assert_eq!(plan.ratings, 1);
    }
    #[test] fn plex_requires_sqlite_magic() {
        let d = tempdir().unwrap();
        let p = d.path().join("com.plexapp.plugins.library.db");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"SQLite format 3\0extra").unwrap();
        let plan = dry_run(SourceKind::Plex, &p).unwrap();
        assert!(plan.warnings.iter().any(|w| w.contains("Plex")));
        // Wrong magic fails.
        let bad = d.path().join("bad.db");
        std::fs::write(&bad, b"NOTSQLITE").unwrap();
        assert!(dry_run(SourceKind::Plex, &bad).is_err());
    }
    #[test] fn missing_source_errors() {
        assert!(dry_run(SourceKind::Picasa, Path::new("/no/such/.picasa.ini")).is_err());
    }
}
