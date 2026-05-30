//! `np.p4.cloud.preview` — in-app preview without full download.
//!
//! Images get a ranged thumbnail fetch, PDFs a per-page fetch, text/JSON a
//! capped head read. This picks the preview strategy from the file extension
//! and builds the HTTP `Range` header so we never pull the whole object.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PreviewKind { Image, Pdf, Text, Unsupported }

pub fn kind_for(path: &str) -> PreviewKind {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" => PreviewKind::Image,
        "pdf" => PreviewKind::Pdf,
        "txt" | "md" | "json" | "csv" | "log" | "xml" | "yaml" | "yml" => PreviewKind::Text,
        _ => PreviewKind::Unsupported,
    }
}

/// HTTP `Range` header value for the first `n` bytes (thumbnail / head read).
pub fn range_header(n: u64) -> String {
    format!("bytes=0-{}", n.saturating_sub(1))
}

/// Byte budget for a preview of this kind. `None` = not previewable.
pub fn preview_budget(kind: PreviewKind) -> Option<u64> {
    match kind {
        PreviewKind::Image => Some(256 * 1024),  // enough for a thumbnail
        PreviewKind::Pdf => Some(2 * 1024 * 1024), // first pages
        PreviewKind::Text => Some(64 * 1024),
        PreviewKind::Unsupported => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_detection() {
        assert_eq!(kind_for("a/b.PNG"), PreviewKind::Image);
        assert_eq!(kind_for("doc.pdf"), PreviewKind::Pdf);
        assert_eq!(kind_for("notes.md"), PreviewKind::Text);
        assert_eq!(kind_for("movie.mkv"), PreviewKind::Unsupported);
    }

    #[test]
    fn range_and_budget() {
        assert_eq!(range_header(1024), "bytes=0-1023");
        assert!(preview_budget(PreviewKind::Image).is_some());
        assert!(preview_budget(PreviewKind::Unsupported).is_none());
    }
}
