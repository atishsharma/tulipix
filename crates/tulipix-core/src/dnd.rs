//! Drag-and-drop: in-bound resolver.
//!
//! When the OS drops files INTO Tulipix, we add the path to the nearest
//! watched library — never copying bytes. If no library covers the path,
//! we offer to add the containing folder as a new watched location.

use crate::libraries::{LibrariesConfig, Section};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropResolution {
    /// Path falls inside an existing watched library — index in place.
    AddedToExisting { library_id: String, path: PathBuf },
    /// No watched library matches — prompt user to add containing folder.
    PromptAddLocation { suggested_root: PathBuf, suggested_section: Section },
}

pub fn resolve_drop(cfg: &LibrariesConfig, dropped: &Path, default_section: Section) -> DropResolution {
    for lib in &cfg.libraries {
        if dropped.starts_with(&lib.path) {
            return DropResolution::AddedToExisting {
                library_id: lib.id.clone(),
                path: dropped.to_path_buf(),
            };
        }
    }
    let root = dropped.parent().unwrap_or(dropped).to_path_buf();
    DropResolution::PromptAddLocation { suggested_root: root, suggested_section: default_section }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::libraries::Library;
    use std::time::SystemTime;

    fn lib(id: &str, path: &str, section: Section) -> Library {
        Library {
            id: id.into(),
            path: PathBuf::from(path),
            section,
            last_scan: Some(SystemTime::now()),
            item_count: 0, size_bytes: 0,
            exclude_globs: vec![],
            cadence_override: None,
            realtime_notify: true,
        }
    }

    #[test]
    fn drop_into_existing_library() {
        let cfg = LibrariesConfig {
            libraries: vec![lib("photos1", "/home/user/Pictures", Section::Photos)],
            ..Default::default()
        };
        let res = resolve_drop(&cfg, Path::new("/home/user/Pictures/a.jpg"), Section::Photos);
        match res {
            DropResolution::AddedToExisting { library_id, .. } => assert_eq!(library_id, "photos1"),
            _ => panic!("expected AddedToExisting"),
        }
    }

    #[test]
    fn drop_outside_prompts_add() {
        let cfg = LibrariesConfig::default();
        let res = resolve_drop(&cfg, Path::new("/tmp/foo/bar.jpg"), Section::Photos);
        match res {
            DropResolution::PromptAddLocation { suggested_root, .. } => {
                assert_eq!(suggested_root, PathBuf::from("/tmp/foo"));
            }
            _ => panic!("expected PromptAddLocation"),
        }
    }
}
