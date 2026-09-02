//! `np.p4.tools.merge` — merge files.
//!
//! Video: lossless `ffmpeg concat` demuxer when inputs share a codec, else a
//! transcode concat. Audio: same. Photo: panorama stitch (delegated to the
//! stitcher; here we build its input list). This owns argv + the concat-list
//! file contents ffmpeg's concat demuxer needs.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MergeKind {
    Video,
    Audio,
}

/// Contents of the ffmpeg concat-demuxer list file (one `file '...'` per line,
/// single-quotes escaped).
pub fn concat_list(paths: &[&str]) -> String {
    paths
        .iter()
        .map(|p| format!("file '{}'", p.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// argv for a lossless concat (stream copy) from a list file.
pub fn concat_copy_args(list_file: &str, out: &str) -> Vec<String> {
    vec![
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_file.into(),
        "-c".into(),
        "copy".into(),
        out.into(),
    ]
}

/// argv for a transcode concat (mixed codecs) — re-encode to a common target.
pub fn concat_transcode_args(list_file: &str, out: &str, kind: MergeKind) -> Vec<String> {
    let mut a = vec![
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_file.into(),
    ];
    match kind {
        MergeKind::Video => {
            a.extend(["-c:v".into(), "libx264".into(), "-c:a".into(), "aac".into()]);
        }
        MergeKind::Audio => {
            a.extend(["-c:a".into(), "libmp3lame".into()]);
        }
    }
    a.push(out.into());
    a
}

/// Everything that has to agree before two clips can be joined by copying
/// their streams rather than re-encoding them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipSpec {
    pub v_codec: String,
    pub a_codec: String,
    pub width: u32,
    pub height: u32,
    /// ffprobe's `sample_aspect_ratio`, verbatim.
    pub sar: String,
    pub pix_fmt: String,
    pub sample_rate: u32,
    pub channels: u32,
}

impl ClipSpec {
    /// Read one out of `ffprobe -show_streams -of json`. First video stream and
    /// first audio stream, which is what the concat demuxer's header describes.
    pub fn from_probe_json(v: &serde_json::Value) -> Self {
        let mut c = ClipSpec::default();
        for st in v["streams"].as_array().into_iter().flatten() {
            let name = st["codec_name"].as_str().unwrap_or_default().to_string();
            // Cover art is a video stream. An MP3 with one and an MP3 without
            // would otherwise read as "different video codec", which is both
            // baffling and wrong: the picture is not what the concat header
            // describes.
            if st["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 1 {
                continue;
            }
            match st["codec_type"].as_str() {
                Some("video") if c.v_codec.is_empty() => {
                    c.v_codec = name;
                    c.width = st["width"].as_u64().unwrap_or(0) as u32;
                    c.height = st["height"].as_u64().unwrap_or(0) as u32;
                    c.sar = st["sample_aspect_ratio"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    c.pix_fmt = st["pix_fmt"].as_str().unwrap_or_default().to_string();
                }
                Some("audio") if c.a_codec.is_empty() => {
                    c.a_codec = name;
                    c.sample_rate = st["sample_rate"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    c.channels = st["channels"].as_u64().unwrap_or(0) as u32;
                }
                _ => {}
            }
        }
        c
    }
}

/// Can we losslessly concat?
///
/// Sharing a codec is not enough, which is what this used to ask. The concat
/// demuxer writes one header, from the first input, and then appends the rest
/// of the packets underneath it — so two h264 clips at 640x480 and 1280x720
/// copy without a word of complaint into a file that *says* 640x480 and turns
/// into something else half way through. Players disagree about what to do
/// with that; ffmpeg's own decoder logs "video parameters changed" and
/// reconfigures. Every field here is one that lives in that header.
pub fn can_stream_copy(clips: &[ClipSpec]) -> bool {
    !clips.is_empty() && clips.iter().all(|c| *c == clips[0])
}

/// Which field of `other` disagrees with `first`, in the words the message
/// shows. `None` when they match.
pub fn first_difference(first: &ClipSpec, other: &ClipSpec) -> Option<(String, String, String)> {
    let fields: [(&str, String, String); 8] = [
        ("video codec", first.v_codec.clone(), other.v_codec.clone()),
        ("audio codec", first.a_codec.clone(), other.a_codec.clone()),
        ("width", first.width.to_string(), other.width.to_string()),
        ("height", first.height.to_string(), other.height.to_string()),
        ("pixel aspect", first.sar.clone(), other.sar.clone()),
        ("pixel format", first.pix_fmt.clone(), other.pix_fmt.clone()),
        (
            "sample rate",
            first.sample_rate.to_string(),
            other.sample_rate.to_string(),
        ),
        (
            "channel count",
            first.channels.to_string(),
            other.channels.to_string(),
        ),
    ];
    fields
        .into_iter()
        .find(|(_, a, b)| a != b)
        .map(|(what, a, b)| (what.to_string(), a, b))
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

    fn clip(w: u32, h: u32) -> ClipSpec {
        ClipSpec {
            v_codec: "h264".into(),
            a_codec: "aac".into(),
            width: w,
            height: h,
            sar: "1:1".into(),
            pix_fmt: "yuv420p".into(),
            sample_rate: 48_000,
            channels: 2,
        }
    }

    #[test]
    fn copy_vs_transcode_choice() {
        assert!(can_stream_copy(&[clip(1920, 1080), clip(1920, 1080)]));
        assert!(concat_copy_args("l.txt", "o.mp4").contains(&"copy".to_string()));
        assert!(
            concat_transcode_args("l.txt", "o.mp4", MergeKind::Video)
                .contains(&"libx264".to_string())
        );
    }

    #[test]
    fn one_codec_in_common_is_not_enough_to_copy() {
        // Both h264, both aac, and the join still produces a file that lies
        // about its own dimensions from the halfway mark on.
        let (a, b) = (clip(640, 480), clip(1280, 720));
        assert!(!can_stream_copy(&[a.clone(), b.clone()]));
        let (what, from, to) = first_difference(&a, &b).expect("they differ");
        assert_eq!(
            (what.as_str(), from.as_str(), to.as_str()),
            ("width", "640", "1280")
        );

        // Same size, different shape: an anamorphic clip beside a square one.
        let mut anam = clip(1920, 1080);
        anam.sar = "4:3".into();
        assert!(!can_stream_copy(&[clip(1920, 1080), anam.clone()]));
        assert_eq!(
            first_difference(&clip(1920, 1080), &anam).unwrap().0,
            "pixel aspect"
        );

        // The same hole on the audio side: two MP3s share a header too, and
        // the second half of a 44.1 kHz file appended to a 48 kHz one plays at
        // the wrong speed.
        let mut slow = clip(0, 0);
        slow.sample_rate = 44_100;
        assert!(!can_stream_copy(&[clip(0, 0), slow.clone()]));
        assert_eq!(
            first_difference(&clip(0, 0), &slow).unwrap().0,
            "sample rate"
        );

        assert_eq!(first_difference(&clip(640, 480), &clip(640, 480)), None);
    }

    #[test]
    fn a_clip_spec_reads_the_streams_that_end_up_in_the_header() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"streams":[
                 {"codec_type":"video","codec_name":"h264","width":720,"height":576,
                  "sample_aspect_ratio":"16:15","pix_fmt":"yuv420p"},
                 {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2},
                 {"codec_type":"video","codec_name":"mjpeg","width":64,"height":64,
                  "disposition":{"attached_pic":1}}
               ]}"#,
        )
        .unwrap();
        let c = ClipSpec::from_probe_json(&v);
        assert_eq!((c.width, c.height, c.sar.as_str()), (720, 576, "16:15"));
        assert_eq!((c.sample_rate, c.channels), (48_000, 2));
        // The cover art is a video stream and is not what the header
        // describes.
        assert_eq!(c.v_codec, "h264");

        // Two MP3s, one tagged with a picture and one not, are still the same
        // kind of thing.
        let bare: serde_json::Value = serde_json::from_str(
            r#"{"streams":[{"codec_type":"audio","codec_name":"mp3","sample_rate":"44100","channels":2}]}"#,
        )
        .unwrap();
        let arted: serde_json::Value = serde_json::from_str(
            r#"{"streams":[
                 {"codec_type":"audio","codec_name":"mp3","sample_rate":"44100","channels":2},
                 {"codec_type":"video","codec_name":"mjpeg","width":600,"height":600,
                  "disposition":{"attached_pic":1}}
               ]}"#,
        )
        .unwrap();
        assert!(can_stream_copy(&[
            ClipSpec::from_probe_json(&bare),
            ClipSpec::from_probe_json(&arted)
        ]));
    }
}
