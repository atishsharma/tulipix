//! `np.p4.tools.convert` — format convert any → any with smart defaults.
//!
//! Routes a source to the right tool (ffmpeg for A/V, image-rs for stills) and
//! picks a sane default codec for the target container so the user doesn't have
//! to. The media-class detection + default-codec map are the testable core.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Media { Video, Audio, Image, Unknown }

pub fn media_of(ext: &str) -> Media {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "mp4"|"mkv"|"mov"|"avi"|"webm"|"m4v"|"wmv"|"flv" => Media::Video,
        "mp3"|"flac"|"m4a"|"aac"|"ogg"|"opus"|"wav"|"aiff" => Media::Audio,
        "jpg"|"jpeg"|"png"|"webp"|"avif"|"gif"|"bmp"|"tiff" => Media::Image,
        _ => Media::Unknown,
    }
}

/// Default codec/encoder for a target container extension.
pub fn default_codec(target_ext: &str) -> Option<&'static str> {
    Some(match target_ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "libx264",
        "mkv" => "libx265",
        "webm" => "libvpx-vp9",
        "mp3" => "libmp3lame",
        "m4a" | "aac" => "aac",
        "opus" => "libopus",
        "ogg" => "libvorbis",
        "flac" => "flac",
        _ => return None,
    })
}

/// ffmpeg argv for an A/V convert with the smart-default codec (overridable).
pub fn av_args(input: &str, target_ext: &str, out: &str) -> Vec<String> {
    let mut a = vec!["-i".into(), input.into()];
    if let Some(codec) = default_codec(target_ext) {
        let flag = if media_of(target_ext) == Media::Audio { "-c:a" } else { "-c:v" };
        a.push(flag.into());
        a.push(codec.into());
    }
    a.push(out.into());
    a
}

/// Is this a supported, sensible conversion (same media class, or audio
/// extracted from video)?
pub fn is_valid(from_ext: &str, to_ext: &str) -> bool {
    let (f, t) = (media_of(from_ext), media_of(to_ext));
    matches!((f, t),
        (Media::Video, Media::Video) | (Media::Video, Media::Audio) |
        (Media::Audio, Media::Audio) | (Media::Image, Media::Image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_classification() {
        assert_eq!(media_of("MKV"), Media::Video);
        assert_eq!(media_of(".flac"), Media::Audio);
        assert_eq!(media_of("png"), Media::Image);
    }

    #[test]
    fn smart_defaults_and_validity() {
        assert_eq!(default_codec("webm"), Some("libvpx-vp9"));
        assert!(av_args("a.mov", "mp4", "o.mp4").contains(&"libx264".to_string()));
        assert!(is_valid("mkv", "mp3"));    // extract audio
        assert!(!is_valid("mp3", "mp4"));   // audio → video nonsense
        assert!(is_valid("png", "webp"));
    }
}
