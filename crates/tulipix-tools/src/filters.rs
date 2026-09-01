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
    // `afftdn`'s nr is in dB and refuses 0. The floor has to survive the
    // formatting as well as the clamp: 0.01 printed to one decimal is "0.0",
    // which is the value being guarded against.
    let nr = reduction_db.clamp(0.1, 97.0);
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

// ------------------------------------------------------------- speed ---

/// `atempo` only accepts 0.5..2.0, so anything outside that is a chain of
/// stages whose product is the rate asked for. 4x is `atempo=2,atempo=2`;
/// 0.25x is `atempo=0.5,atempo=0.5`. Getting this wrong is silent — ffmpeg
/// rejects the filter and the job fails with a message about a filter graph,
/// which tells the user nothing.
pub fn atempo_chain(rate: f64) -> String {
    let mut rate = rate.clamp(0.05, 16.0);
    let mut stages: Vec<String> = Vec::new();
    while rate > 2.0 {
        stages.push("atempo=2.0".into());
        rate /= 2.0;
    }
    while rate < 0.5 {
        stages.push("atempo=0.5".into());
        rate *= 2.0;
    }
    // A rate of exactly 1 still needs a filter, because the caller is building
    // a graph and an empty link is a syntax error.
    stages.push(format!("atempo={rate:.4}"));
    stages.join(",")
}

/// Speed the picture up or down. `setpts` is the inverse of the rate: playing
/// twice as fast means each frame is presented half as far along.
pub fn setpts(rate: f64) -> String {
    let rate = if rate > 0.0 { rate } else { 1.0 };
    format!("setpts={:.6}*PTS", 1.0 / rate)
}

/// Resample without correcting the pitch — the chipmunk, on purpose. The
/// sample rate is multiplied and then declared back down, which is exactly
/// what playing a record at the wrong speed does.
pub fn asetrate(rate: f64, source_hz: u32) -> String {
    let rate = if rate > 0.0 { rate } else { 1.0 };
    format!(
        "asetrate={},aresample={source_hz}",
        (source_hz as f64 * rate).round() as u64
    )
}

// -------------------------------------------------------------- fades ---

/// Fade up from black and down to it, and the same on the sound.
///
/// The fade-out has to know where the end is, and ffmpeg's `fade` filter takes
/// a start time rather than an offset from the end — so the duration is
/// required, and a caller without one gets only the fade in.
pub fn fade_filters(in_s: f64, out_s: f64, duration_s: Option<f64>) -> (String, String) {
    let mut video: Vec<String> = Vec::new();
    let mut audio: Vec<String> = Vec::new();
    if in_s > 0.0 {
        video.push(format!("fade=t=in:st=0:d={in_s}"));
        audio.push(format!("afade=t=in:st=0:d={in_s}"));
    }
    if out_s > 0.0 {
        if let Some(total) = duration_s.filter(|d| *d > out_s) {
            let at = total - out_s;
            video.push(format!("fade=t=out:st={at:.3}:d={out_s}"));
            audio.push(format!("afade=t=out:st={at:.3}:d={out_s}"));
        }
    }
    (
        if video.is_empty() {
            "null".into()
        } else {
            video.join(",")
        },
        if audio.is_empty() {
            "anull".into()
        } else {
            audio.join(",")
        },
    )
}

// ------------------------------------------------------------ picture ---

/// A flat border. `bottom` widens the bottom edge on its own, the way a print
/// is mounted with room for a caption.
pub fn border_filter(width: u32, bottom: u32, colour: &str) -> String {
    let w = width;
    let b = width + bottom;
    format!(
        "pad=iw+{}:ih+{}:{w}:{w}:{}",
        w * 2,
        w + b,
        safe_colour(colour)
    )
}

/// ffmpeg takes a colour name or `0xRRGGBB`; a leading `#` is what everyone
/// types and what ffmpeg refuses.
pub fn safe_colour(colour: &str) -> String {
    let c = colour.trim();
    if let Some(hex) = c.strip_prefix('#') {
        format!("0x{hex}")
    } else if c.is_empty() {
        "white".into()
    } else {
        c.to_string()
    }
}

/// Brightness, contrast, saturation and gamma in one `eq`.
pub fn eq_filter(brightness: f64, contrast: f64, saturation: f64, gamma: f64) -> String {
    format!(
        "eq=brightness={:.3}:contrast={:.3}:saturation={:.3}:gamma={:.3}",
        brightness.clamp(-1.0, 1.0),
        contrast.clamp(0.0, 3.0),
        saturation.clamp(0.0, 3.0),
        gamma.clamp(0.1, 3.0)
    )
}

/// The named looks. Sepia is a colour matrix rather than "grey plus a tint",
/// which is the version that comes out muddy.
pub fn look_filter(look: &str) -> String {
    match look {
        "sepia" => "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131".into(),
        "negative" => "negate".into(),
        "warm" => "colortemperature=temperature=8000".into(),
        "cool" => "colortemperature=temperature=4000".into(),
        _ => "hue=s=0".into(),
    }
}

/// Unsharp masking. The matrix size must be odd, and ffmpeg errors rather than
/// rounding, so it is rounded here.
pub fn sharpen_filter(amount: f64, radius: u32) -> String {
    let r = (radius.clamp(3, 13) | 1).to_string();
    format!("unsharp={r}:{r}:{:.2}:{r}:{r}:0", amount.clamp(0.0, 5.0))
}

/// Blur, pixelate or black out one rectangle, leaving the rest untouched.
///
/// The region is cut out, mangled, and laid back over the original. Applying
/// the effect to the whole frame and masking it would be one filter shorter
/// and would also blur the whole frame if the mask were ever wrong.
pub fn censor_filter(style: &str, x: u32, y: u32, w: u32, h: u32) -> String {
    let w = w.max(1);
    let h = h.max(1);
    let effect = match style {
        "solid" => "drawbox=x=0:y=0:w=iw:h=ih:color=black:t=fill".to_string(),
        "blur" => "boxblur=luma_radius=min(iw\\,ih)/8:luma_power=2".to_string(),
        // Down to blocks and back up with no smoothing, which is what
        // pixelation is.
        _ => {
            let blocks = 12u32;
            format!(
                "scale={}:{}:flags=neighbor,scale={w}:{h}:flags=neighbor",
                (w / blocks).max(1),
                (h / blocks).max(1)
            )
        }
    };
    format!("[0:v]crop={w}:{h}:{x}:{y},{effect}[patch];[0:v][patch]overlay={x}:{y}")
}

/// Silence removal. `middle` decides whether the gaps inside are cut too, or
/// only the dead air at each end.
pub fn silence_filter(threshold_db: f64, min_ms: f64, middle: bool) -> String {
    let d = (min_ms.max(0.0) / 1000.0).max(0.05);
    let db = threshold_db.clamp(-90.0, -5.0);
    if middle {
        // -1 means "every period", which is what cutting the middle means.
        format!(
            "silenceremove=start_periods=1:start_duration={d}:start_threshold={db}dB:stop_periods=-1:stop_duration={d}:stop_threshold={db}dB"
        )
    } else {
        format!(
            "silenceremove=start_periods=1:start_duration={d}:start_threshold={db}dB:stop_periods=1:stop_duration={d}:stop_threshold={db}dB:detection=peak"
        )
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn atempo_stays_inside_the_range_ffmpeg_accepts() {
        assert_eq!(atempo_chain(1.5), "atempo=1.5000");
        assert_eq!(atempo_chain(4.0), "atempo=2.0,atempo=2.0000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5000");
        // Every stage in every chain is inside 0.5..2.0 — the whole point.
        for rate in [0.1, 0.3, 0.5, 1.0, 2.0, 3.0, 8.0] {
            for stage in atempo_chain(rate).split(',') {
                let v: f64 = stage.trim_start_matches("atempo=").parse().unwrap();
                assert!((0.5..=2.0).contains(&v), "{rate} produced {stage}");
            }
        }
    }

    #[test]
    fn the_chain_multiplies_back_to_the_rate_asked_for() {
        for rate in [0.25, 0.75, 1.0, 1.5, 3.0, 4.0] {
            let product: f64 = atempo_chain(rate)
                .split(',')
                .map(|s| s.trim_start_matches("atempo=").parse::<f64>().unwrap())
                .product();
            assert!((product - rate).abs() < 0.001, "{rate} became {product}");
        }
    }

    #[test]
    fn setpts_is_the_inverse_of_the_rate() {
        assert_eq!(setpts(2.0), "setpts=0.500000*PTS");
        assert_eq!(setpts(0.5), "setpts=2.000000*PTS");
        // A zero would divide by zero rather than error.
        assert_eq!(setpts(0.0), "setpts=1.000000*PTS");
    }

    #[test]
    fn a_fade_out_needs_to_know_where_the_end_is() {
        let (v, a) = fade_filters(1.0, 2.0, Some(60.0));
        assert!(v.contains("st=0:d=1"));
        assert!(v.contains("st=58.000:d=2"));
        assert!(a.starts_with("afade"));
        // No duration: the fade in still happens, the fade out cannot.
        let (v, _) = fade_filters(1.0, 2.0, None);
        assert!(!v.contains("t=out"));
        // Nothing asked for is still a valid link in the graph.
        assert_eq!(
            fade_filters(0.0, 0.0, Some(60.0)),
            ("null".into(), "anull".into())
        );
    }

    #[test]
    fn a_hash_colour_becomes_one_ffmpeg_accepts() {
        assert_eq!(safe_colour("#ff8800"), "0xff8800");
        assert_eq!(safe_colour("white"), "white");
        assert_eq!(safe_colour("  "), "white");
    }

    #[test]
    fn a_border_grows_the_canvas_by_twice_its_width() {
        // 40 all round, 20 extra at the bottom: 80 wider, 100 taller, and the
        // picture sits 40 in from the left and top.
        assert_eq!(
            border_filter(40, 20, "#000000"),
            "pad=iw+80:ih+100:40:40:0x000000"
        );
    }

    #[test]
    fn sharpen_rounds_its_matrix_up_to_an_odd_number() {
        assert!(sharpen_filter(1.0, 4).starts_with("unsharp=5:5:"));
        assert!(sharpen_filter(1.0, 99).starts_with("unsharp=13:13:"));
    }

    #[test]
    fn the_censor_puts_the_patch_back_where_it_came_from() {
        let f = censor_filter("blur", 100, 50, 200, 80);
        assert!(f.contains("crop=200:80:100:50"));
        assert!(f.ends_with("overlay=100:50"));
        // The blur's own comma is escaped, or it would end the filter early.
        assert!(f.contains("min(iw\\,ih)"));
    }

    #[test]
    fn trimming_only_the_ends_does_not_ask_for_every_period() {
        assert!(!silence_filter(-50.0, 500.0, false).contains("stop_periods=-1"));
        assert!(silence_filter(-50.0, 500.0, true).contains("stop_periods=-1"));
    }
}
