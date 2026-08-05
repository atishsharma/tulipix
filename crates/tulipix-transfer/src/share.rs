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
    /// The device token this file is for, or `None` for "any paired device".
    ///
    /// Stamped when the file is added, from whatever the tray was pointed at
    /// then — not looked up at download time. Change the target, add more
    /// files, and each keeps the audience it was chosen for; re-pointing the
    /// tray must not silently re-address what is already in it.
    pub to: Option<String>,
}

impl Item {
    /// May `token` see this file? A file for everyone is visible to every paired
    /// device; a file addressed to one device is visible to that device only.
    pub fn visible_to(&self, token: &str) -> bool {
        match &self.to {
            None => true,
            Some(t) => t == token,
        }
    }
}

#[derive(Default)]
pub struct Tray {
    items: Vec<Item>,
    next_id: u64,
    /// Who the next file added is for. `None` is everyone, which is where the
    /// tray starts and where forgetting the targeted device puts it back.
    target: Option<String>,
}

impl Tray {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Only what this device is allowed to see.
    ///
    /// Both lifetimes named rather than elided: the filter closure holds `token`,
    /// so the returned iterator borrows it as well as `self`, and an elided
    /// return type would not admit that.
    pub fn items_for<'a>(&'a self, token: &'a str) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |i| i.visible_to(token))
    }

    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Point the tray at one device, or at everyone with `None`.
    pub fn set_target(&mut self, token: Option<String>) {
        self.target = token.filter(|t| !t.is_empty());
    }

    /// A device was forgotten (or expired). Anything addressed only to it is now
    /// addressed to nobody — it would sit in the tray visible to no one at all,
    /// which reads as a file that failed to send. Widen it back to everyone and
    /// drop the target if that is where it was pointed.
    pub fn release(&mut self, token: &str) {
        for item in &mut self.items {
            if item.to.as_deref() == Some(token) {
                item.to = None;
            }
        }
        if self.target.as_deref() == Some(token) {
            self.target = None;
        }
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
        self.items.push(Item {
            id: self.next_id,
            name,
            bytes: meta.len(),
            path: path.to_path_buf(),
            to: self.target.clone(),
        });
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
mod target_tests {
    use super::*;

    fn tray_with(target: Option<&str>, dir: &std::path::Path, name: &str) -> Tray {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        let mut t = Tray::default();
        t.set_target(target.map(str::to_string));
        t.add(&p);
        t
    }

    #[test]
    fn an_untargeted_file_is_for_everyone() {
        let dir = tempfile::tempdir().unwrap();
        let tray = tray_with(None, dir.path(), "all.bin");
        assert!(tray.items()[0].visible_to("phone-a"));
        assert!(tray.items()[0].visible_to("phone-b"));
    }

    #[test]
    fn a_targeted_file_is_for_one_device_only() {
        let dir = tempfile::tempdir().unwrap();
        let tray = tray_with(Some("phone-a"), dir.path(), "just-a.bin");
        assert!(tray.items()[0].visible_to("phone-a"));
        assert!(!tray.items()[0].visible_to("phone-b"));
        assert_eq!(tray.items_for("phone-b").count(), 0);
        assert_eq!(tray.items_for("phone-a").count(), 1);
    }

    /// Re-pointing the tray must not re-address what is already in it.
    #[test]
    fn the_target_is_stamped_at_add_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut tray = Tray::default();
        for (target, name) in [(None, "everyone.bin"), (Some("phone-a"), "mine.bin")] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            tray.set_target(target.map(str::to_string));
            tray.add(&p);
        }
        tray.set_target(Some("phone-b".into()));
        assert_eq!(tray.items_for("phone-b").count(), 1, "only the everyone file");
        assert_eq!(tray.items_for("phone-a").count(), 2, "everyone plus its own");
    }

    /// A file addressed to a device that no longer exists is visible to nobody,
    /// which reads as a send that failed rather than one that was cancelled.
    #[test]
    fn forgetting_a_device_widens_its_files_back_out() {
        let dir = tempfile::tempdir().unwrap();
        let mut tray = tray_with(Some("phone-a"), dir.path(), "orphan.bin");
        tray.release("phone-a");
        assert!(tray.items()[0].visible_to("phone-b"));
        assert_eq!(tray.target(), None, "and the tray stops pointing at it");
    }

    /// An empty string arrives from the UI when the "Everyone" chip is picked;
    /// it must mean everyone, not a device whose token is "".
    #[test]
    fn an_empty_target_is_everyone() {
        let mut tray = Tray::default();
        tray.set_target(Some(String::new()));
        assert_eq!(tray.target(), None);
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
