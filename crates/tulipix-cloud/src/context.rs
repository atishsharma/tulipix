//! `np.p4.cloud.context` — right-click context-menu parity on remotes.
//!
//! Mounted remotes get the full OS file action set (Reveal in mount path, Copy
//! mount URL, Open With… via OS associations). No-mount (HTTP) remotes get a
//! reduced set (Copy `rclone://` URL, Download). This module decides which menu
//! items apply to a given entry + mount state, so the UI stays consistent.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    RevealInMount,
    CopyMountUrl,
    OpenWith,
    CopyRcloneUrl,
    Download,
    Rename,
    Delete,
}

/// Build the context menu for an entry given mount state + whether it's a dir.
pub fn menu_for(mounted: bool, is_dir: bool) -> Vec<Action> {
    let mut m = Vec::new();
    if mounted {
        m.push(Action::RevealInMount);
        m.push(Action::CopyMountUrl);
        if !is_dir { m.push(Action::OpenWith); }
    } else {
        m.push(Action::CopyRcloneUrl);
        if !is_dir { m.push(Action::Download); }
    }
    m.push(Action::Rename);
    m.push(Action::Delete);
    m
}

/// `rclone://remote/path` URL for the no-mount Copy action.
pub fn rclone_url(remote: &str, path: &str) -> String {
    format!("rclone://{remote}/{}", path.trim_start_matches('/'))
}

/// Local filesystem path of an entry on a mounted remote.
pub fn mount_path(mount_root: &str, remote_path: &str) -> String {
    format!("{}/{}", mount_root.trim_end_matches('/'), remote_path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounted_file_menu() {
        let m = menu_for(true, false);
        assert!(m.contains(&Action::RevealInMount));
        assert!(m.contains(&Action::OpenWith));
        assert!(!m.contains(&Action::Download));
    }

    #[test]
    fn nomount_menu_offers_download() {
        let m = menu_for(false, false);
        assert!(m.contains(&Action::CopyRcloneUrl));
        assert!(m.contains(&Action::Download));
        assert!(!m.contains(&Action::RevealInMount));
    }

    #[test]
    fn dir_has_no_openwith_or_download() {
        assert!(!menu_for(true, true).contains(&Action::OpenWith));
        assert!(!menu_for(false, true).contains(&Action::Download));
    }

    #[test]
    fn url_and_path_builders() {
        assert_eq!(rclone_url("gdrive", "/a/b.txt"), "rclone://gdrive/a/b.txt");
        assert_eq!(mount_path("/mnt/g/", "/a/b.txt"), "/mnt/g/a/b.txt");
    }
}
