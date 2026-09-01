//! `np.p4.tools.filters` — the `-vf` and `-af` strings the newer video and
//! audio operations are made of.
//!
//! One module rather than one per operation, because these are all the same
//! shape: take a control or two off a form and produce a filter chain. The
//! argv around them lives in [`crate::exec`]; what is testable is the string,
//! and that is what is tested here.
//!
//! **Commas.** A comma separates filters in a chain, so a comma *inside* a
//! filter's own argument has to be escaped as `\,` — the same escaping
//! [`crate::watermark`] does for colons in `drawtext`. Getting this wrong does
//! not fail loudly; ffmpeg reads the rest of the expression as a second filter
//! and reports something unrelated.

/// A quarter-turn count and the two mirrors, as one chain.
///
/// `transpose=1` is 90° clockwise and `transpose=2` is anticlockwise; there is
/// no 180 transpose, so that one is two of them.
pub fn orient_filter(quarter_turns: u32, hflip: bool, vflip: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    match quarter_turns % 4 {
        1 => parts.push("transpose=1"),
        2 => {
            parts.push("transpose=1");
            parts.push("transpose=1");
        }
        3 => parts.push("transpose=2"),
        _ => {}
    }
    if hflip {
        parts.push("hflip");
    }
    if vflip {
        parts.push("vflip");
    }
    if parts.is_empty() {
        // An empty -vf is an error; `null` is the pass-through.
        "null".to_string()
    } else {
        parts.join(",")
    }
}

/// Crop to an explicit box, in source pixels.
pub fn crop_box(w: u32, h: u32, x: u32, y: u32) -> String {
    format!("crop={}:{}:{}:{}", w.max(2), h.max(2), x, y)
}

/// Crop to an aspect ratio, centred, taking it off whichever pair of edges has
/// the slack. This is the one that removes letterbox and pillarbox bars
/// without anyone having to measure them.
pub fn crop_to_aspect(rw: u32, rh: u32) -> String {
    let (rw, rh) = (rw.max(1), rh.max(1));
    // `min` needs a comma, and a bare comma would end the filter here.
    format!("crop=w=min(iw\\,ih*{rw}/{rh}):h=min(ih\\,iw*{rh}/{rw})",)
}

/// `"16:9"` / `"4x3"` / `"1"` → the pair. `None` for anything else, which the
/// caller reads as "the user is cropping by hand".
pub fn parse_aspect(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let (a, b) = s
        .split_once(':')
        .or_else(|| s.split_once('x'))
        .or_else(|| s.split_once('/'))?;
    let (a, b) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    if a == 0 || b == 0 { None } else { Some((a, b)) }
}

/// Spatial and temporal denoise, plus the deblocker that undoes what a low
/// bitrate did to a source before it ever reached us.
///
/// `hqdn3d`'s four knobs default to `4:3:6:4.5`; one control scales all four,
/// because separate luma and chroma spatial sliders is a question nobody
/// standing in front of this form can answer.
pub fn denoise_filter(strength: f64, deblock: bool) -> String {
    let f = strength.clamp(0.0, 10.0) / 4.0;
    let mut out = format!(
        "hqdn3d={:.2}:{:.2}:{:.2}:{:.2}",
        4.0 * f,
        3.0 * f,
        6.0 * f,
        4.5 * f
    );
    if deblock {
        out.push_str(",deblock=filter=weak:block=4");
    }
    out
}

/// Broadband noise reduction, and the high-pass that takes out handling rumble
/// and air conditioning — which is most of what people mean by "noisy".
pub fn audio_denoise_filter(reduction_db: f64, rumble: bool) -> String {
    // `afftdn`'s nr is in dB and refuses 0.
    let nr = reduction_db.clamp(0.01, 97.0);
    let core = format!("afftdn=nr={nr:.1}:nf=-25");
    if rumble {
        format!("highpass=f=80,{core}")
    } else {
        core
    }
}

/// The subtitle codec a container can carry. mp4 has only its own text format;
/// Matroska and WebM take SRT as-is.
pub fn soft_sub_codec(container_ext: &str) -> &'static str {
    match container_ext
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" | "mov" => "mov_text",
        _ => "srt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_chains_and_never_empties() {
        assert_eq!(orient_filter(1, false, false), "transpose=1");
        assert_eq!(orient_filter(2, false, false), "transpose=1,transpose=1");
        assert_eq!(orient_filter(3, false, false), "transpose=2");
        assert_eq!(orient_filter(0, true, true), "hflip,vflip");
        // Nothing selected still has to be a valid filter.
        assert_eq!(orient_filter(0, false, false), "null");
        assert_eq!(orient_filter(4, false, false), "null");
    }

    #[test]
    fn aspect_crop_escapes_its_comma() {
        let f = crop_to_aspect(16, 9);
        assert!(f.contains("\\,"), "an unescaped comma would end the filter");
        assert!(!f.contains("min(iw,"), "{f}");
        assert_eq!(parse_aspect("16:9"), Some((16, 9)));
        assert_eq!(parse_aspect("4x3"), Some((4, 3)));
        assert_eq!(parse_aspect("manual"), None);
        assert_eq!(parse_aspect("0:9"), None);
    }

    #[test]
    fn crop_box_refuses_a_zero_edge() {
        assert_eq!(crop_box(1920, 800, 0, 140), "crop=1920:800:0:140");
        assert_eq!(crop_box(0, 0, 0, 0), "crop=2:2:0:0");
    }

    #[test]
    fn denoise_scales_all_four_knobs_together() {
        // The default strength reproduces hqdn3d's own defaults.
        assert_eq!(denoise_filter(4.0, false), "hqdn3d=4.00:3.00:6.00:4.50");
        assert!(denoise_filter(4.0, true).contains("deblock="));
        // Zero is a no-op rather than an error.
        assert_eq!(denoise_filter(0.0, false), "hqdn3d=0.00:0.00:0.00:0.00");
    }

    #[test]
    fn audio_denoise_takes_the_rumble_first() {
        let f = audio_denoise_filter(12.0, true);
        assert!(f.starts_with("highpass=f=80,"), "{f}");
        assert!(f.contains("nr=12.0"));
        // afftdn refuses nr=0.
        assert!(!audio_denoise_filter(0.0, false).contains("nr=0.0"));
    }

    #[test]
    fn subtitle_codec_follows_the_container() {
        assert_eq!(soft_sub_codec("mp4"), "mov_text");
        assert_eq!(soft_sub_codec(".MOV"), "mov_text");
        assert_eq!(soft_sub_codec("mkv"), "srt");
    }
}
