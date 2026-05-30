//! `np.p4.tools.merge` — merge files.
//!
//! Video: lossless `ffmpeg concat` demuxer when inputs share a codec, else a
//! transcode concat. Audio: same. Photo: panorama stitch (delegated to the
//! stitcher; here we build its input list). This owns argv + the concat-list
//! file contents ffmpeg's concat demuxer needs.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergeKind { Video, Audio }

/// Contents of the ffmpeg concat-demuxer list file (one `file '...'` per line,
/// single-quotes escaped).
pub fn concat_list(paths: &[&str]) -> String {
    paths.iter().map(|p| format!("file '{}'", p.replace('\'', "'\\''"))).collect::<Vec<_>>().join("\n") + "\n"
}

/// argv for a lossless concat (stream copy) from a list file.
pub fn concat_copy_args(list_file: &str, out: &str) -> Vec<String> {
    vec![
        "-f".into(), "concat".into(), "-safe".into(), "0".into(),
        "-i".into(), list_file.into(),
        "-c".into(), "copy".into(),
        out.into(),
    ]
}

/// argv for a transcode concat (mixed codecs) — re-encode to a common target.
pub fn concat_transcode_args(list_file: &str, out: &str, kind: MergeKind) -> Vec<String> {
    let mut a = vec!["-f".into(), "concat".into(), "-safe".into(), "0".into(), "-i".into(), list_file.into()];
    match kind {
        MergeKind::Video => { a.extend(["-c:v".into(), "libx264".into(), "-c:a".into(), "aac".into()]); }
        MergeKind::Audio => { a.extend(["-c:a".into(), "libmp3lame".into()]); }
    }
    a.push(out.into());
    a
}

/// Can we losslessly concat? Only when every input shares one codec id.
pub fn can_stream_copy(codecs: &[&str]) -> bool {
    !codecs.is_empty() && codecs.iter().all(|c| *c == codecs[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_escapes_quotes() {
        let l = concat_list(&["/a b.mp4", "/o'clock.mp4"]);
        assert!(l.contains("file '/a b.mp4'"));
        assert!(l.contains(r"o'\''clock"));
    }

    #[test]
    fn copy_vs_transcode_choice() {
        assert!(can_stream_copy(&["h264", "h264"]));
        assert!(!can_stream_copy(&["h264", "hevc"]));
        assert!(concat_copy_args("l.txt", "o.mp4").contains(&"copy".to_string()));
        assert!(concat_transcode_args("l.txt", "o.mp4", MergeKind::Video).contains(&"libx264".to_string()));
    }
}
