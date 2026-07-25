//! What the desktop is offering right now. A session thing: closing the section
//! empties it, because a share is a session, not a setting.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Item {
    /// Stable for the life of the tray. Downloads reference this, never a path.
    pub id: u64,
    pub name: String,
    pub bytes: u64,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct Tray {
    items: Vec<Item>,
    next_id: u64,
}

impl Tray {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// A file is added directly; a folder contributes the files inside it, one
    /// level deep. Recursing further would let one click share a home directory.
    pub fn add(&mut self, path: &Path) {
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(path) else { return };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    self.add_file(&p);
                }
            }
        } else {
            self.add_file(path);
        }
    }

    fn add_file(&mut self, path: &Path) {
        if self.items.iter().any(|i| i.path == path) {
            return;
        }
        let Ok(meta) = std::fs::metadata(path) else { return };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        self.next_id += 1;
        self.items.push(Item { id: self.next_id, name, bytes: meta.len(), path: path.to_path_buf() });
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|i| i.id != id);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn by_id(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }

    /// The path for a download, or `None` if the file has since gone away.
    /// Checked at request time: the tray can be minutes old.
    pub fn readable(&self, id: u64) -> Option<PathBuf> {
        let item = self.by_id(id)?;
        item.path.is_file().then(|| item.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_a_file_records_its_size_and_display_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("song.mp3");
        std::fs::write(&p, vec![0u8; 1234]).unwrap();

        let mut tray = Tray::default();
        tray.add(&p);

        assert_eq!(tray.items().len(), 1);
        assert_eq!(tray.items()[0].name, "song.mp3");
        assert_eq!(tray.items()[0].bytes, 1234);
    }

    #[test]
    fn adding_a_folder_takes_the_files_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("album")).unwrap();
        std::fs::write(dir.path().join("album/a.mp3"), b"a").unwrap();
        std::fs::write(dir.path().join("album/b.mp3"), b"b").unwrap();

        let mut tray = Tray::default();
        tray.add(&dir.path().join("album"));

        let mut names: Vec<_> = tray.items().iter().map(|i| i.name.clone()).collect();
        names.sort();
        assert_eq!(names, ["a.mp3", "b.mp3"]);
    }

    #[test]
    fn the_same_file_added_twice_appears_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        std::fs::write(&p, b"x").unwrap();

        let mut tray = Tray::default();
        tray.add(&p);
        tray.add(&p);
        assert_eq!(tray.items().len(), 1);
    }

    #[test]
    fn ids_are_stable_across_a_removal() {
        let dir = tempfile::tempdir().unwrap();
        for n in ["a", "b", "c"] {
            std::fs::write(dir.path().join(n), b"x").unwrap();
        }
        let mut tray = Tray::default();
        for n in ["a", "b", "c"] {
            tray.add(&dir.path().join(n));
        }
        let c_id = tray.items()[2].id;
        tray.remove(tray.items()[0].id);
        // Removing "a" must not renumber "c" — a download in flight holds an id.
        assert_eq!(tray.by_id(c_id).unwrap().name, "c");
    }

    #[test]
    fn a_file_deleted_from_disk_stops_being_served() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("gone.bin");
        std::fs::write(&p, b"x").unwrap();
        let mut tray = Tray::default();
        tray.add(&p);
        let id = tray.items()[0].id;
        std::fs::remove_file(&p).unwrap();
        assert!(tray.readable(id).is_none());
    }
}
