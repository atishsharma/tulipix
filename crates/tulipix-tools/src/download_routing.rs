//! `np.p4.tools.download.routing` — auto-route downloads to the right library.
//!
//! After a download finishes, route the file by its type: video → Videos,
//! audio → Music, image → Photos. Resolves the destination library directory
//! from the output extension.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section { Videos, Music, Photos, Unsorted }

impl Section {
    pub fn dir_name(self) -> &'static str {
        match self { Section::Videos => "Videos", Section::Music => "Music", Section::Photos => "Photos", Section::Unsorted => "Downloads" }
    }
}

/// Route by file extension.
pub fn route(ext: &str) -> Section {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "mp4"|"mkv"|"webm"|"mov"|"m4v"|"avi" => Section::Videos,
        "opus"|"mp3"|"m4a"|"aac"|"flac"|"ogg"|"wav" => Section::Music,
        "jpg"|"jpeg"|"png"|"webp"|"gif" => Section::Photos,
        _ => Section::Unsorted,
    }
}

/// Destination path: `<library_root>/<Section>/<filename>`.
pub fn dest_path(library_root: &str, filename: &str, ext: &str) -> String {
    format!("{}/{}/{}", library_root.trim_end_matches('/'), route(ext).dir_name(), filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_type() {
        assert_eq!(route("mkv"), Section::Videos);
        assert_eq!(route(".OPUS"), Section::Music);
        assert_eq!(route("png"), Section::Photos);
        assert_eq!(route("zip"), Section::Unsorted);
    }

    #[test]
    fn dest_under_section_dir() {
        assert_eq!(dest_path("/lib/", "song.opus", "opus"), "/lib/Music/song.opus");
    }
}
