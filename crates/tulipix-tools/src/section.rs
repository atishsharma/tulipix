//! `np.p4.tools.section` — Tools sidebar entry + landing page.
//!
//! The landing page groups operations into categories (File ops · Video ·
//! Audio · Photo · Subtitles · Queue). This owns the category taxonomy and the
//! op→category mapping the landing grid renders from.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category { FileOps, Video, Audio, Photo, Subtitles, Queue }

impl Category {
    pub const ALL: [Category; 6] = [Category::FileOps, Category::Video, Category::Audio, Category::Photo, Category::Subtitles, Category::Queue];
    pub fn label(self) -> &'static str {
        match self {
            Category::FileOps => "File ops", Category::Video => "Video", Category::Audio => "Audio",
            Category::Photo => "Photo", Category::Subtitles => "Subtitles", Category::Queue => "Queue",
        }
    }
}

/// Category for an operation kind (matches [`crate::cli`] op names).
pub fn category_of(op: &str) -> Category {
    match op {
        "rename" | "merge" | "split" | "hash" | "folder-diff" => Category::FileOps,
        "compress-video" | "trim" | "convert" | "thumbnail" | "download" | "download-live" | "download-playlist" => Category::Video,
        "compress-audio" | "normalize" | "extract" => Category::Audio,
        "compress-photo" | "resize" | "watermark" | "pdf" => Category::Photo,
        "transcribe" | "burn-subs" => Category::Subtitles,
        _ => Category::Queue,
    }
}

/// Op kinds shown under a category on the landing page.
pub fn ops_in(category: Category) -> Vec<&'static str> {
    const OPS: &[&str] = &[
        "rename","merge","split","hash","folder-diff",
        "compress-video","trim","convert","thumbnail","download","download-live","download-playlist",
        "compress-audio","normalize","extract",
        "compress-photo","resize","watermark","pdf",
        "transcribe","burn-subs",
    ];
    OPS.iter().copied().filter(|op| category_of(op) == category).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categorization() {
        assert_eq!(category_of("rename"), Category::FileOps);
        assert_eq!(category_of("transcribe"), Category::Subtitles);
        assert_eq!(category_of("compress-photo"), Category::Photo);
        assert!(ops_in(Category::Audio).contains(&"normalize"));
        assert_eq!(Category::ALL.len(), 6);
    }
}
