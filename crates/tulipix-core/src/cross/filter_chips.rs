//! `np.p4.chips` — file-type filter chips.
//!
//! The shared chip bar (Photos / Videos / Audio / Documents / PDFs / Archives)
//! maps each chip to an extension set and a predicate. Multiple chips combine
//! as a union (OR). Used by search results, browse views, and storage insights.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Chip { Photos, Videos, Audio, Documents, Pdfs, Archives }

impl Chip {
    pub const ALL: [Chip; 6] = [Chip::Photos, Chip::Videos, Chip::Audio, Chip::Documents, Chip::Pdfs, Chip::Archives];

    pub fn label(self) -> &'static str {
        match self {
            Chip::Photos => "Photos", Chip::Videos => "Videos", Chip::Audio => "Audio",
            Chip::Documents => "Documents", Chip::Pdfs => "PDFs", Chip::Archives => "Archives",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Chip::Photos => &["jpg","jpeg","png","gif","webp","heic","tiff","bmp","raw","dng"],
            Chip::Videos => &["mp4","mkv","mov","avi","webm","m4v","wmv","flv"],
            Chip::Audio  => &["mp3","flac","alac","m4a","aac","ogg","opus","wav","aiff","dsf"],
            Chip::Documents => &["doc","docx","odt","rtf","txt","md","epub"],
            Chip::Pdfs   => &["pdf"],
            Chip::Archives => &["zip","rar","7z","tar","gz","bz2","xz","cbz","cbr"],
        }
    }

    pub fn matches(self, path: &str) -> bool {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        self.extensions().contains(&ext.as_str())
    }
}

/// Does `path` pass the active chip set? Empty set = everything passes.
pub fn passes(active: &[Chip], path: &str) -> bool {
    active.is_empty() || active.iter().any(|c| c.matches(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_matching() {
        assert!(Chip::Photos.matches("/x/a.JPG"));
        assert!(Chip::Pdfs.matches("doc.pdf"));
        assert!(!Chip::Audio.matches("movie.mp4"));
        assert!(Chip::Archives.matches("comic.cbz"));
    }

    #[test]
    fn union_semantics() {
        assert!(passes(&[], "anything.xyz"));            // no filter
        assert!(passes(&[Chip::Photos, Chip::Audio], "song.flac"));
        assert!(!passes(&[Chip::Photos], "song.flac"));
        assert_eq!(Chip::ALL.len(), 6);
    }
}
