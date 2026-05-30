//! `np.p4.tools.burn-subs` — burn-in subtitles to video.
//!
//! ffmpeg `-vf subtitles=` hard-burns a subtitle track for devices without
//! soft-sub support, applying style overrides (`force_style`). Owns the path
//! escaping the `subtitles` filter requires (colons + backslashes are special).

/// Escape a path for use inside the `subtitles=` filter value.
pub fn escape_filter_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace(':', "\\:").replace('\'', "\\'")
}

/// Build the `-vf` filter, optionally with an ASS `force_style` string.
pub fn subtitles_filter(sub_path: &str, force_style: Option<&str>) -> String {
    let p = escape_filter_path(sub_path);
    match force_style {
        Some(style) if !style.is_empty() => format!("subtitles='{p}':force_style='{style}'"),
        _ => format!("subtitles='{p}'"),
    }
}

/// Full ffmpeg argv to burn `sub_path` into `input` → `out`.
pub fn args(input: &str, sub_path: &str, force_style: Option<&str>, out: &str) -> Vec<String> {
    vec![
        "-i".into(), input.into(),
        "-vf".into(), subtitles_filter(sub_path, force_style),
        "-c:a".into(), "copy".into(),
        out.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_windows_path() {
        let e = escape_filter_path(r"C:\subs\a.srt");
        assert!(e.contains("C\\:"));
        assert!(e.contains("\\\\subs"));
    }

    #[test]
    fn filter_with_style() {
        let f = subtitles_filter("/s/a.ass", Some("FontSize=28"));
        assert!(f.contains("force_style='FontSize=28'"));
        let plain = subtitles_filter("/s/a.srt", None);
        assert!(!plain.contains("force_style"));
        assert!(args("in.mp4", "/s/a.srt", None, "o.mp4").contains(&"-vf".to_string()));
    }
}
