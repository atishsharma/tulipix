//! Where uploads land. One folder, flat, nothing routed by type.

use crate::names::{safe_name, unique_in};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// A `.part` older than this was abandoned by a phone that walked out of range.
const STALE_AFTER: u64 = 24 * 3600;

/// One upload in flight. Writes to `{name}.part` and renames on completion, so a
/// library scan never sees a half-written file.
pub struct Sink {
    file: tokio::fs::File,
    part: PathBuf,
    final_dir: PathBuf,
    name: String,
    pub written: u64,
}

impl Sink {
    pub async fn create(dir: &Path, raw_name: &str) -> Result<Self> {
        tokio::fs::create_dir_all(dir).await?;
        let name = safe_name(raw_name);
        // The `.part` itself can collide when the same name is sent twice in a
        // row; claim a free one so two uploads never write to one handle.
        let part_name = unique_in(dir, &format!("{name}.part"));
        let part = dir.join(part_name);
        let file = tokio::fs::File::create(&part).await?;
        Ok(Self { file, part, final_dir: dir.to_path_buf(), name, written: 0 })
    }

    pub async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        self.file.write_all(chunk).await?;
        self.written += chunk.len() as u64;
        Ok(())
    }

    /// Flush, then claim a name that is free *now* — another upload may have
    /// taken the obvious one while this was streaming.
    pub async fn finish(mut self) -> Result<PathBuf> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        drop(self.file);

        let final_name = unique_in(&self.final_dir, &self.name);
        let dest = self.final_dir.join(final_name);
        tokio::fs::rename(&self.part, &dest).await?;
        Ok(dest)
    }

    /// The client vanished or the disk filled. Leave nothing behind.
    pub async fn abort(self) {
        drop(self.file);
        let _ = tokio::fs::remove_file(&self.part).await;
    }
}

/// Run at server start: clear `.part` files no client is coming back for.
pub fn sweep_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("part") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age.as_secs() > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether the inbox can actually be written to. Checked when the section opens
/// so the Receive pane can say which path failed rather than failing per upload.
pub fn writable(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".tulipix-write-probe");
    std::fs::write(&probe, b"")?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_completed_upload_lands_under_its_safe_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Sink::create(dir.path(), "../../etc/passwd").await.unwrap();
        sink.write(b"hello ").await.unwrap();
        sink.write(b"world").await.unwrap();
        let done = sink.finish().await.unwrap();

        assert_eq!(done.file_name().unwrap(), "passwd");
        assert_eq!(std::fs::read_to_string(&done).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn an_upload_in_flight_is_invisible_to_a_library_scan() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Sink::create(dir.path(), "big.mkv").await.unwrap();
        sink.write(b"partial").await.unwrap();

        // Mid-flight the only file present is the .part, so a scan triggered by
        // something else never indexes a half-written file.
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["big.mkv.part"]);

        sink.finish().await.unwrap();
        assert!(dir.path().join("big.mkv").exists());
        assert!(!dir.path().join("big.mkv.part").exists());
    }

    #[tokio::test]
    async fn an_abandoned_upload_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let mut sink = Sink::create(dir.path(), "x.bin").await.unwrap();
        sink.write(b"half").await.unwrap();
        sink.abort().await;

        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn an_upload_never_overwrites_what_is_already_there() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"original").unwrap();

        let mut sink = Sink::create(dir.path(), "a.txt").await.unwrap();
        sink.write(b"new").await.unwrap();
        let done = sink.finish().await.unwrap();

        assert_eq!(done.file_name().unwrap(), "a (2).txt");
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "original");
    }

    #[tokio::test]
    async fn two_uploads_of_one_name_at_once_keep_separate_part_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Sink::create(dir.path(), "same.bin").await.unwrap();
        let mut b = Sink::create(dir.path(), "same.bin").await.unwrap();
        a.write(b"aaaa").await.unwrap();
        b.write(b"bb").await.unwrap();

        let pa = a.finish().await.unwrap();
        let pb = b.finish().await.unwrap();
        assert_ne!(pa, pb);
        assert_eq!(std::fs::read(&pa).unwrap(), b"aaaa");
        assert_eq!(std::fs::read(&pb).unwrap(), b"bb");
    }

    #[tokio::test]
    async fn stale_part_files_are_swept_but_fresh_ones_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.mkv.part"), b"x").unwrap();
        std::fs::write(dir.path().join("new.mkv.part"), b"x").unwrap();

        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
        filetime::set_file_mtime(
            dir.path().join("old.mkv.part"),
            filetime::FileTime::from_system_time(old),
        )
        .unwrap();

        sweep_stale(dir.path());

        assert!(!dir.path().join("old.mkv.part").exists());
        assert!(dir.path().join("new.mkv.part").exists());
    }

    #[test]
    fn an_unwritable_inbox_is_reported_rather_than_discovered_per_upload() {
        let dir = tempfile::tempdir().unwrap();
        assert!(writable(dir.path()).is_ok());
        // A path whose parent is a file cannot be created.
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        assert!(writable(&file.join("inbox")).is_err());
    }
}
