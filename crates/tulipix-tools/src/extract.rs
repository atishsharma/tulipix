//! `np.p4.tools.extract` — audio tracks, embedded subtitles, chapters,
//! attachments (fonts).
//!
//! Builds the ffmpeg argv to pull one stream/attachment out of a container by
//! index, plus the chapter-dump argv. Stream selection (`-map 0:a:N`) is the
//! fiddly part and is unit-tested.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind { Audio, Subtitle, Attachment }

/// argv to extract stream #`index` of `kind` from `input` to `out`.
pub fn extract_stream_args(input: &str, kind: Kind, index: u32, out: &str) -> Vec<String> {
    match kind {
        Kind::Attachment => vec![
            "-dump_attachment:t".into(), out.into(),
            "-i".into(), input.into(),
        ],
        // Audio: stream-copy (lossless). Subtitles: re-encode — `-c copy` of an
        // ass/mov_text stream into a .srt output fails; text subs convert fine.
        Kind::Audio => vec![
            "-i".into(), input.into(),
            "-map".into(), format!("0:a:{index}"),
            "-c".into(), "copy".into(),
            out.into(),
        ],
        Kind::Subtitle => vec![
            "-i".into(), input.into(),
            "-map".into(), format!("0:s:{index}"),
            out.into(),
        ],
    }
}

/// argv to dump chapters as ffmetadata.
pub fn dump_chapters_args(input: &str, out: &str) -> Vec<String> {
    vec!["-i".into(), input.into(), "-f".into(), "ffmetadata".into(), out.into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_correct_stream() {
        let a = extract_stream_args("in.mkv", Kind::Audio, 1, "a.mka");
        assert!(a.windows(2).any(|w| w == ["-map", "0:a:1"]));
        let s = extract_stream_args("in.mkv", Kind::Subtitle, 0, "s.srt");
        assert!(s.windows(2).any(|w| w == ["-map", "0:s:0"]));
    }

    #[test]
    fn attachment_and_chapters() {
        assert!(extract_stream_args("in.mkv", Kind::Attachment, 0, "font.ttf").contains(&"-dump_attachment:t".to_string()));
        assert!(dump_chapters_args("in.mkv", "ch.txt").contains(&"ffmetadata".to_string()));
    }
}
