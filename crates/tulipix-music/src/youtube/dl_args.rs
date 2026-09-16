//! What a download is asked to become, and the yt-dlp arguments that make it.
//!
//! The panel speaks in presets ("MP3 · 320 kbps", "MP4 · 1080p · H.264"), the
//! bridge passes their keys, and everything that knows what `-x` or
//! `--merge-output-format` means lives here, where it can be tested without
//! spawning anything.

use std::path::Path;

use crate::youtube::formats::StreamInfo;

/// Where downloads go until the user says otherwise. `channel,uploader` because
/// some uploads have no channel name and yt-dlp would write `NA`.
pub const DEFAULT_TEMPLATE: &str = "~/Music/YouTube/%(channel,uploader)s/%(title)s.%(ext)s";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioPreset {
    Mp3,
    M4a,
    Opus,
    Flac,
    Wav,
    Vorbis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPreset {
    Mp4(u32),
    Av1(u32),
    Webm(u32),
    MkvBest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Audio(AudioPreset),
    Video(VideoPreset),
}

impl Preset {
    /// Every preset the panel offers, audio first, in display order.
    pub fn all() -> Vec<Preset> {
        use AudioPreset::*;
        use VideoPreset::*;
        vec![
            Preset::Audio(Mp3),
            Preset::Audio(M4a),
            Preset::Audio(Opus),
            Preset::Audio(Flac),
            Preset::Audio(Wav),
            Preset::Audio(Vorbis),
            Preset::Video(Mp4(1080)),
            Preset::Video(Mp4(720)),
            Preset::Video(Mp4(480)),
            Preset::Video(Av1(1080)),
            Preset::Video(Webm(1440)),
            Preset::Video(MkvBest),
        ]
    }

    pub fn key(&self) -> String {
        match self {
            Preset::Audio(AudioPreset::Mp3) => "mp3".into(),
            Preset::Audio(AudioPreset::M4a) => "m4a".into(),
            Preset::Audio(AudioPreset::Opus) => "opus".into(),
            Preset::Audio(AudioPreset::Flac) => "flac".into(),
            Preset::Audio(AudioPreset::Wav) => "wav".into(),
            Preset::Audio(AudioPreset::Vorbis) => "vorbis".into(),
            Preset::Video(VideoPreset::Mp4(h)) => format!("mp4-{h}"),
            Preset::Video(VideoPreset::Av1(h)) => format!("av1-{h}"),
            Preset::Video(VideoPreset::Webm(h)) => format!("webm-{h}"),
            Preset::Video(VideoPreset::MkvBest) => "mkv-best".into(),
        }
    }

    /// A key back into a preset. Only keys `all()` produces parse, so a stale
    /// pref or a hand-edited settings file cannot ask for `mp4-99999`.
    pub fn parse(key: &str) -> Option<Preset> {
        Preset::all().into_iter().find(|p| p.key() == key)
    }

    pub fn is_audio(&self) -> bool {
        matches!(self, Preset::Audio(_))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Preset::Audio(AudioPreset::Mp3) => "MP3",
            Preset::Audio(AudioPreset::M4a) => "M4A",
            Preset::Audio(AudioPreset::Opus) => "Opus",
            Preset::Audio(AudioPreset::Flac) => "FLAC",
            Preset::Audio(AudioPreset::Wav) => "WAV",
            Preset::Audio(AudioPreset::Vorbis) => "Vorbis",
            Preset::Video(VideoPreset::Mp4(_) | VideoPreset::Av1(_)) => "MP4",
            Preset::Video(VideoPreset::Webm(_)) => "WebM",
            Preset::Video(VideoPreset::MkvBest) => "MKV",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Preset::Audio(AudioPreset::Mp3) => "320 kbps".into(),
            Preset::Audio(AudioPreset::M4a) => "AAC · original".into(),
            Preset::Audio(AudioPreset::Opus) => "original stream".into(),
            Preset::Audio(AudioPreset::Flac) => "lossless container".into(),
            Preset::Audio(AudioPreset::Wav) => "uncompressed".into(),
            Preset::Audio(AudioPreset::Vorbis) => "Ogg".into(),
            Preset::Video(VideoPreset::Mp4(h)) => format!("{h}p · H.264"),
            Preset::Video(VideoPreset::Av1(h)) => format!("{h}p · AV1"),
            Preset::Video(VideoPreset::Webm(h)) => format!("{h}p · VP9"),
            Preset::Video(VideoPreset::MkvBest) => "best · any codec".into(),
        }
    }

    /// "MP3 · 320 kbps": the job list and the Downloads badge.
    pub fn label(&self) -> String {
        format!("{} · {}", self.name(), self.detail())
    }

    /// What a user should know before picking this one. Empty for most.
    pub fn note(&self) -> &'static str {
        match self {
            Preset::Audio(AudioPreset::Mp3) => {
                "Re-encoded from the source stream: a bigger file, not a better one."
            }
            Preset::Audio(AudioPreset::Flac) => {
                "FLAC keeps what the source has. It cannot restore what the source lost."
            }
            Preset::Audio(AudioPreset::Wav) => "Uncompressed, with no cover art: a very large file.",
            _ => "",
        }
    }

    /// yt-dlp refuses to embed a thumbnail into WAV or WebM, and the refusal
    /// fails the whole download rather than skipping the cover.
    pub fn embeds_cover(&self) -> bool {
        !matches!(
            self,
            Preset::Audio(AudioPreset::Wav) | Preset::Video(VideoPreset::Webm(_))
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extras {
    pub cover: bool,
    pub tags: bool,
    pub chapters: bool,
    pub sponsorblock: bool,
    pub subs: bool,
}

impl Default for Extras {
    fn default() -> Self {
        Extras { cover: true, tags: true, chapters: true, sponsorblock: false, subs: false }
    }
}

impl Extras {
    /// `"cover,tags,chapters"`: the names of the ones that are on.
    pub fn parse(csv: &str) -> Extras {
        let on = |name: &str| csv.split(',').any(|x| x.trim() == name);
        Extras {
            cover: on("cover"),
            tags: on("tags"),
            chapters: on("chapters"),
            sponsorblock: on("sponsorblock"),
            subs: on("subs"),
        }
    }

    pub fn to_pref_string(&self) -> String {
        [
            (self.cover, "cover"),
            (self.tags, "tags"),
            (self.chapters, "chapters"),
            (self.sponsorblock, "sponsorblock"),
            (self.subs, "subs"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(",")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadSpec {
    pub preset: Preset,
    pub extras: Extras,
    pub template: String,
}

/// `~/Music/…` against `home`. yt-dlp does its own `~` expansion, but only on
/// some platforms and not inside every option, so it is done here once.
pub fn expand_home(template: &str, home: &Path) -> String {
    if template == "~" {
        return home.to_string_lossy().into_owned();
    }
    match template.strip_prefix("~/") {
        Some(rest) => home.join(rest).to_string_lossy().into_owned(),
        None => template.to_string(),
    }
}

/// A file-name piece as yt-dlp would write it: no path separators or
/// characters Windows refuses, and never empty.
fn path_safe(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned.is_empty() { "NA".into() } else { cleaned }
}

/// The download template filled in by hand, for a file that is already on
/// disk (a cached video being kept) and so never goes through yt-dlp.
/// Understands the fields `DEFAULT_TEMPLATE` uses; any other `%(…)s` becomes
/// `NA`, which is what yt-dlp writes for a field it does not have.
pub fn render_path(template: &str, home: &Path, channel: &str, title: &str, id: &str, ext: &str) -> std::path::PathBuf {
    let mut out = String::new();
    let mut rest = expand_home(template, home);
    while let Some(at) = rest.find("%(") {
        out.push_str(&rest[..at]);
        let Some(end) = rest[at..].find(")s") else {
            out.push_str(&rest[at..]);
            rest.clear();
            break;
        };
        let field = &rest[at + 2..at + end];
        let value = match field {
            "channel,uploader" | "channel" | "uploader" => path_safe(channel),
            "title" => path_safe(title),
            "id" => path_safe(id),
            "ext" => ext.to_string(),
            _ => "NA".to_string(),
        };
        out.push_str(&value);
        rest = rest[at + end + 2..].to_string();
    }
    out.push_str(&rest);
    std::path::PathBuf::from(out)
}

/// The whole argument list for one download, URL last. `path_file` receives
/// the final file path: `--print` would give it to us on stdout, but `--print`
/// implies `--quiet`, and quiet is the end of the progress bar.
pub fn build_args(spec: &DownloadSpec, url: &str, home: &Path, path_file: &Path) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--newline".into(),
        "--no-playlist".into(),
        "-o".into(),
        expand_home(&spec.template, home),
        "--print-to-file".into(),
        "after_move:filepath".into(),
        path_file.to_string_lossy().into_owned(),
    ];
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|x| x.to_string()));
    match spec.preset {
        Preset::Audio(AudioPreset::Mp3) => {
            push(&["-f", "bestaudio", "-x", "--audio-format", "mp3", "--audio-quality", "320K"])
        }
        Preset::Audio(AudioPreset::M4a) => {
            push(&["-f", "bestaudio[ext=m4a]/bestaudio", "-x", "--audio-format", "m4a"])
        }
        Preset::Audio(AudioPreset::Opus) => {
            push(&["-f", "bestaudio[acodec=opus]/bestaudio", "-x", "--audio-format", "opus"])
        }
        Preset::Audio(AudioPreset::Flac) => push(&["-f", "bestaudio", "-x", "--audio-format", "flac"]),
        Preset::Audio(AudioPreset::Wav) => push(&["-f", "bestaudio", "-x", "--audio-format", "wav"]),
        Preset::Audio(AudioPreset::Vorbis) => {
            push(&["-f", "bestaudio", "-x", "--audio-format", "vorbis"])
        }
        Preset::Video(VideoPreset::Mp4(h)) => push(&[
            "-f",
            format!("bv[height<={h}][vcodec^=avc1]+ba[ext=m4a]/b[height<={h}]").as_str(),
            "--merge-output-format",
            "mp4",
        ]),
        Preset::Video(VideoPreset::Av1(h)) => push(&[
            "-f",
            format!("bv[height<={h}][vcodec^=av01]+ba[ext=m4a]/b[height<={h}]").as_str(),
            "--merge-output-format",
            "mp4",
        ]),
        Preset::Video(VideoPreset::Webm(h)) => push(&[
            "-f",
            format!("bv[height<={h}][ext=webm]+ba[ext=webm]/b[height<={h}]").as_str(),
            "--merge-output-format",
            "webm",
        ]),
        Preset::Video(VideoPreset::MkvBest) => {
            push(&["-f", "bv+ba/b", "--merge-output-format", "mkv"])
        }
    }
    let x = spec.extras;
    if x.cover && spec.preset.embeds_cover() {
        push(&["--embed-thumbnail"]);
        if spec.preset.is_audio() {
            // A WebP cover is one most car stereos and phones will not show.
            push(&["--convert-thumbnails", "jpg"]);
        }
    }
    if x.tags {
        push(&["--embed-metadata"]);
    }
    if x.chapters {
        push(&["--embed-chapters"]);
    }
    if x.sponsorblock {
        push(&["--sponsorblock-remove", "sponsor,selfpromo,intro"]);
    }
    if x.subs && !spec.preset.is_audio() {
        push(&["--embed-subs", "--sub-langs", "en"]);
    }
    a.push(url.to_string());
    a
}

/// Roughly how big the file will be. Stream sizes when yt-dlp knows them,
/// duration × bitrate otherwise; `None` when neither is known or the video
/// has no stream that fits the preset.
pub fn estimate_bytes(preset: &Preset, info: &StreamInfo) -> Option<u64> {
    let dur = info.duration_s;
    let by_rate = |kbps: u32| (dur > 0.0).then(|| (dur * kbps as f64 * 1000.0 / 8.0) as u64);
    let audio = |codec: Option<&str>| {
        info.audio
            .iter()
            .find(|a| codec.is_none_or(|c| a.acodec == c))
            .or(info.audio.first())
            .and_then(|a| a.bytes.or_else(|| by_rate(a.abr_kbps)))
    };
    let video = |cap: u32, codec: Option<&str>| {
        info.video
            .iter()
            .find(|v| v.height <= cap && codec.is_none_or(|c| v.vcodec == c))
            .and_then(|v| v.bytes.or_else(|| by_rate(v.tbr_kbps)))
    };
    match preset {
        Preset::Audio(AudioPreset::Mp3) => by_rate(320),
        Preset::Audio(AudioPreset::M4a) => audio(Some("mp4a")),
        Preset::Audio(AudioPreset::Opus) => audio(Some("opus")),
        // Measured, not derived: 48 kHz stereo decoded from a lossy source
        // lands around 800 kbps as FLAC; WAV is 48 000 × 16 × 2.
        Preset::Audio(AudioPreset::Flac) => by_rate(800),
        Preset::Audio(AudioPreset::Wav) => by_rate(1536),
        Preset::Audio(AudioPreset::Vorbis) => by_rate(160),
        Preset::Video(VideoPreset::Mp4(h)) => Some(video(*h, Some("avc1"))? + audio(Some("mp4a"))?),
        Preset::Video(VideoPreset::Av1(h)) => Some(video(*h, Some("av01"))? + audio(Some("mp4a"))?),
        Preset::Video(VideoPreset::Webm(h)) => Some(video(*h, Some("vp09"))? + audio(Some("opus"))?),
        Preset::Video(VideoPreset::MkvBest) => Some(video(u32::MAX, None)? + audio(None)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec(key: &str, extras: Extras) -> DownloadSpec {
        DownloadSpec { preset: Preset::parse(key).unwrap(), extras, template: DEFAULT_TEMPLATE.into() }
    }

    fn none() -> Extras {
        Extras { cover: false, tags: false, chapters: false, sponsorblock: false, subs: false }
    }

    fn args(key: &str, extras: Extras) -> Vec<String> {
        build_args(&spec(key, extras), "https://youtu.be/x", Path::new("/home/me"), Path::new("/tmp/p.txt"))
    }

    /// The slice of `args` from `-f` up to (not including) the first extra or the URL.
    fn format_part(a: &[String]) -> Vec<&str> {
        let at = a.iter().position(|x| x == "-f").unwrap();
        a[at..]
            .iter()
            .take_while(|x| !x.starts_with("--embed") && !x.starts_with("--sponsor") && !x.starts_with("https://"))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn every_preset_key_round_trips() {
        let keys: Vec<String> = Preset::all().iter().map(Preset::key).collect();
        assert_eq!(keys.len(), 12);
        for k in &keys {
            assert_eq!(Preset::parse(k).unwrap().key(), *k);
        }
        assert_eq!(Preset::parse("mp4-99999"), None);
        assert_eq!(Preset::parse(""), None);
    }

    #[test]
    fn audio_presets_extract_to_their_format() {
        assert_eq!(format_part(&args("mp3", none())), ["-f", "bestaudio", "-x", "--audio-format", "mp3", "--audio-quality", "320K"]);
        assert_eq!(format_part(&args("m4a", none())), ["-f", "bestaudio[ext=m4a]/bestaudio", "-x", "--audio-format", "m4a"]);
        assert_eq!(format_part(&args("opus", none())), ["-f", "bestaudio[acodec=opus]/bestaudio", "-x", "--audio-format", "opus"]);
        assert_eq!(format_part(&args("flac", none())), ["-f", "bestaudio", "-x", "--audio-format", "flac"]);
        assert_eq!(format_part(&args("wav", none())), ["-f", "bestaudio", "-x", "--audio-format", "wav"]);
        assert_eq!(format_part(&args("vorbis", none())), ["-f", "bestaudio", "-x", "--audio-format", "vorbis"]);
    }

    #[test]
    fn video_presets_merge_to_their_container() {
        assert_eq!(format_part(&args("mp4-720", none())), ["-f", "bv[height<=720][vcodec^=avc1]+ba[ext=m4a]/b[height<=720]", "--merge-output-format", "mp4"]);
        assert_eq!(format_part(&args("av1-1080", none())), ["-f", "bv[height<=1080][vcodec^=av01]+ba[ext=m4a]/b[height<=1080]", "--merge-output-format", "mp4"]);
        assert_eq!(format_part(&args("webm-1440", none())), ["-f", "bv[height<=1440][ext=webm]+ba[ext=webm]/b[height<=1440]", "--merge-output-format", "webm"]);
        assert_eq!(format_part(&args("mkv-best", none())), ["-f", "bv+ba/b", "--merge-output-format", "mkv"]);
    }

    #[test]
    fn fixed_head_and_url_last() {
        let a = args("mp3", Extras::default());
        assert_eq!(
            &a[..7],
            ["--newline", "--no-playlist", "-o", "/home/me/Music/YouTube/%(channel,uploader)s/%(title)s.%(ext)s", "--print-to-file", "after_move:filepath", "/tmp/p.txt"]
        );
        assert_eq!(a.last().unwrap(), "https://youtu.be/x");
    }

    #[test]
    fn extras_apply_only_where_yt_dlp_accepts_them() {
        let all = Extras { cover: true, tags: true, chapters: true, sponsorblock: true, subs: true };
        let has = |a: &[String], x: &str| a.iter().any(|y| y == x);

        let mp3 = args("mp3", all);
        assert!(has(&mp3, "--embed-thumbnail") && has(&mp3, "--convert-thumbnails"));
        assert!(has(&mp3, "--embed-metadata") && has(&mp3, "--embed-chapters"));
        assert!(has(&mp3, "--sponsorblock-remove"));
        assert!(!has(&mp3, "--embed-subs"), "no subtitles in an audio file");

        let mp4 = args("mp4-1080", all);
        assert!(has(&mp4, "--embed-thumbnail") && !has(&mp4, "--convert-thumbnails"));
        assert!(has(&mp4, "--embed-subs"));

        // Both refuse a thumbnail, and the refusal fails the download.
        assert!(!has(&args("wav", all), "--embed-thumbnail"));
        assert!(!has(&args("webm-1440", all), "--embed-thumbnail"));

        let bare = args("opus", none());
        assert!(!bare.iter().any(|x| x.starts_with("--embed") || x.starts_with("--sponsor")));
    }

    #[test]
    fn home_expands_only_at_the_start() {
        let home = PathBuf::from("/home/me");
        assert_eq!(expand_home("~/Music/%(title)s.%(ext)s", &home), "/home/me/Music/%(title)s.%(ext)s");
        assert_eq!(expand_home("~", &home), "/home/me");
        assert_eq!(expand_home("/data/~/x", &home), "/data/~/x");
        assert_eq!(expand_home("~bob/x", &home), "~bob/x");
    }

    #[test]
    fn render_path_fills_the_template_safely() {
        let home = PathBuf::from("/home/me");
        assert_eq!(
            render_path(DEFAULT_TEMPLATE, &home, "AC/DC", "Back in Black?", "x", "opus"),
            PathBuf::from("/home/me/Music/YouTube/AC_DC/Back in Black_.opus")
        );
        assert_eq!(
            render_path("/data/%(id)s [%(upload_date)s].%(ext)s", &home, "", "", "abc", "mkv"),
            PathBuf::from("/data/abc [NA].mkv")
        );
        assert_eq!(render_path("~/x/%(channel)s/%(title)s.%(ext)s", &home, " ", "...", "i", "mp3"), PathBuf::from("/home/me/x/NA/NA.mp3"));
        // An unclosed field is left as typed rather than eating the rest.
        assert_eq!(render_path("/a/%(title", &home, "", "", "", ""), PathBuf::from("/a/%(title"));
    }

    #[test]
    fn extras_round_trip_through_prefs() {
        let x = Extras { cover: true, tags: false, chapters: true, sponsorblock: true, subs: false };
        assert_eq!(x.to_pref_string(), "cover,chapters,sponsorblock");
        assert_eq!(Extras::parse(&x.to_pref_string()), x);
        assert_eq!(Extras::parse(""), none());
    }

    #[test]
    fn estimates_prefer_stream_sizes() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/yt_info.json")).unwrap();
        let info = crate::youtube::formats::parse_info(&v);
        let est = |k: &str| estimate_bytes(&Preset::parse(k).unwrap(), &info);
        // Known sizes: 136 (720p avc1) + 140 (AAC).
        assert_eq!(est("mp4-720"), Some(26_455_880 + 3_449_447));
        // 1440 has no avc1, so the cap walks down to 1080p avc1 (137).
        assert_eq!(est("mp4-1080"), Some(80_911_999 + 3_449_447));
        assert_eq!(est("webm-1440"), Some(151_103_346 + 3_433_755));
        assert_eq!(est("mkv-best"), Some(240_334_643 + 3_449_447));
        assert_eq!(est("opus"), Some(3_433_755));
        // Rate-derived: 213 s × 320 kbps.
        assert_eq!(est("mp3"), Some(8_520_000));
        assert_eq!(estimate_bytes(&Preset::parse("mp3").unwrap(), &StreamInfo::default()), None);
    }
}
