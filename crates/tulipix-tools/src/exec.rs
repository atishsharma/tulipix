//! Executor — turns a queued `(kind, spec_json)` job into the concrete
//! command(s) the worker runs. Pure planning + progress parsing here; the
//! actual process spawning + binary resolution (`tool_bin`) lives in the app's
//! worker loop so this crate stays free of the app's runtime.
//!
//! Every A/V op reuses the existing per-tool argv builders (no logic
//! duplication); image + native ops are assembled here against the same
//! builders. `plan()` is the single entry point both the GUI and CLI worker
//! call.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::{
    burn_subs, compress_audio, compress_video, convert, extract, normalize, resize, thumbnail, trim,
    watermark,
};

/// One unit of work for a job. The worker resolves the binary, appends its own
/// progress flags (for ffmpeg), spawns, and reports progress per the variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// ffmpeg op. `args` is the operation argv (no `-progress`/`-y` — the worker
    /// adds those). `duration_input` is the file whose duration scales progress;
    /// `duration_s`, when set, overrides it with a known output duration (trim
    /// writes `end-start` seconds, not the whole input).
    Ffmpeg { args: Vec<String>, duration_input: Option<String>, duration_s: Option<f64> },
    /// yt-dlp op. Progress parsed from `[download]  NN.N%`.
    YtDlp { args: Vec<String> },
    /// In-process op the worker implements directly (some shell out to ffmpeg).
    Native(Native),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Native {
    Hash { files: Vec<String>, algo: String, manifest: Option<String> },
    FolderDiff { a: String, b: String },
    Rename { dir: String, pattern: String, start: usize },
    Merge { inputs: Vec<String>, output: String },
    /// Extract 16 kHz mono wav, then whisper-cli → SRT (the app resolves
    /// whisper-cli + the bundled model). `language` is a whisper code or "auto";
    /// `translate` uses whisper's built-in speech-translation → English.
    Transcribe { input: String, output: String, translate: bool, language: String },
    /// ffprobe report (format + streams) → text file beside source.
    MediaInfo { input: String, output: String },
    /// Contact sheet — app probes duration, builds the tile filter, runs ffmpeg.
    ContactSheet { input: String, output: String, cols: u32, rows: u32 },
    /// Clear the app's thumbnail/temp cache; report bytes freed.
    CacheClean,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string())
}
fn req(v: &Value, k: &str) -> Result<String> {
    s(v, k).filter(|x| !x.trim().is_empty()).ok_or_else(|| anyhow!("missing field: {k}"))
}
fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())))
}
fn boolean(v: &Value, k: &str) -> bool {
    v.get(k).map(|x| x.as_bool().unwrap_or_else(|| x.as_str() == Some("true"))).unwrap_or(false)
}

/// Plan the steps for a job kind + its JSON spec.
pub fn plan(kind: &str, spec: &Value) -> Result<Vec<Step>> {
    let one = |args: Vec<String>, dur: Option<String>| vec![Step::Ffmpeg { args, duration_input: dur, duration_s: None }];
    match kind {
        "compress_video" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let codec = match s(spec, "codec").as_deref() {
                Some("h265") => compress_video::Codec::H265,
                Some("av1") => compress_video::Codec::Av1,
                _ => compress_video::Codec::H264,
            };
            let crf = num(spec, "crf").unwrap_or(23.0) as u8;
            Ok(one(compress_video::crf_args(&input, codec, crf, &out), Some(input)))
        }
        "compress_audio" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let codec = match s(spec, "codec").as_deref() {
                Some("aac") => compress_audio::AudioCodec::Aac,
                Some("opus") => compress_audio::AudioCodec::Opus,
                Some("vorbis") => compress_audio::AudioCodec::Vorbis,
                _ => compress_audio::AudioCodec::Mp3,
            };
            let kbps = num(spec, "kbps").unwrap_or(192.0) as u32;
            Ok(one(compress_audio::args(&input, codec, compress_audio::Rate::Cbr(kbps), &out), Some(input)))
        }
        "compress_photo" => {
            // mozjpeg/cjpeg isn't bundled — encode all photo formats via ffmpeg
            // so a single resolved binary covers JPEG/WebP/AVIF.
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let q = num(spec, "quality").unwrap_or(82.0).clamp(1.0, 100.0) as u32;
            let args = match s(spec, "format").as_deref() {
                Some("webp") => vec!["-i".into(), input.clone(), "-c:v".into(), "libwebp".into(), "-quality".into(), q.to_string(), out],
                Some("avif") => vec!["-i".into(), input.clone(), "-c:v".into(), "libaom-av1".into(), "-crf".into(), (63 - q * 63 / 100).to_string(), out],
                _ => { let qv = 2 + (31 - 2) * (100 - q) / 100; vec!["-i".into(), input.clone(), "-q:v".into(), qv.to_string(), out] }
            };
            Ok(one(args, None))
        }
        "convert" => {
            let input = req(spec, "input")?;
            let target = req(spec, "target_ext")?;
            let out = req(spec, "output")?;
            match convert::media_of(&target) {
                convert::Media::Image => Ok(one(vec!["-i".into(), input, out], None)),
                _ => Ok(one(convert::av_args(&input, &target, &out), Some(input))),
            }
        }
        "trim" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let start = num(spec, "start_s").unwrap_or(0.0);
            let end = num(spec, "end_s").context("missing end_s")?;
            let args = if boolean(spec, "lossless") {
                trim::lossless_args(&input, start, end, &out)
            } else {
                trim::precise_args(&input, start, end, &out)
            };
            // Progress must scale by the cut's length — ffmpeg's out_time only
            // reaches end-start, never the full input duration.
            Ok(vec![Step::Ffmpeg { args, duration_input: None, duration_s: Some((end - start).max(0.001)) }])
        }
        "resize" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let w = num(spec, "w").unwrap_or(1920.0).max(1.0) as u32;
            let h = num(spec, "h").unwrap_or(1080.0).max(1.0) as u32;
            // "fit" keeps aspect inside the W×H box (default); "exact" may
            // distort; width/height scale one edge, the other follows (even).
            let vf = match s(spec, "mode").as_deref() {
                Some("exact") => resize::scale_filter(w, h),
                Some("width") => format!("scale={w}:-2"),
                Some("height") => format!("scale=-2:{h}"),
                _ => format!("scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2"),
            };
            Ok(one(vec!["-i".into(), input, "-vf".into(), vf, out], None))
        }
        "thumbnail" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let at = num(spec, "at_s").unwrap_or(1.0);
            let width = num(spec, "width").unwrap_or(320.0) as u32;
            Ok(one(thumbnail::thumb_args(&input, at, width, &out), None))
        }
        "extract" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let kind = match s(spec, "stream").as_deref() {
                Some("subtitle") => extract::Kind::Subtitle,
                Some("attachment") => extract::Kind::Attachment,
                _ => extract::Kind::Audio,
            };
            let index = num(spec, "index").unwrap_or(0.0) as u32;
            Ok(one(extract::extract_stream_args(&input, kind, index, &out), None))
        }
        "normalize" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let lufs = num(spec, "lufs").unwrap_or(-23.0);
            let filter = normalize::loudnorm_measure_filter(lufs); // single-pass loudnorm
            // Copy the video stream (incl. mp3 cover art) — only audio changes;
            // without this ffmpeg silently re-encoded the whole video track.
            Ok(one(vec!["-i".into(), input.clone(), "-af".into(), filter, "-c:v".into(), "copy".into(), out], Some(input)))
        }
        "watermark" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            let text = req(spec, "text")?;
            let filter = watermark::text_filter(&text, watermark::Position::BottomRight, 24, 0.85, 28);
            Ok(one(vec!["-i".into(), input, "-vf".into(), filter, out], None))
        }
        "burn_subs" => {
            let input = req(spec, "input")?;
            let sub = req(spec, "sub")?;
            let out = req(spec, "output")?;
            Ok(one(burn_subs::args(&input, &sub, None, &out), Some(input)))
        }
        "split" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?; // template e.g. .../clip_%03d.mp4
            let every = num(spec, "every_s").unwrap_or(60.0);
            Ok(one(vec![
                "-i".into(), input.clone(), "-f".into(), "segment".into(),
                "-segment_time".into(), every.to_string(), "-c".into(), "copy".into(),
                "-reset_timestamps".into(), "1".into(), out,
            ], Some(input)))
        }
        "download" | "download_playlist" | "download_live" => {
            let url = req(spec, "url")?;
            let out_template = s(spec, "out_template").unwrap_or_else(|| "%(title)s.%(ext)s".into());
            let dl = download_spec(spec, url, out_template);
            let mut args = dl.args();
            match kind {
                "download_live" => args.insert(0, "--live-from-start".into()),
                // A watch?v=…&list=… URL must not pull the whole playlist in the
                // single-video tool — and must pull all of it in the playlist tool.
                "download_playlist" => args.insert(0, "--yes-playlist".into()),
                _ => args.insert(0, "--no-playlist".into()),
            }
            Ok(vec![Step::YtDlp { args }])
        }
        "hash" => {
            let files: Vec<String> = spec.get("files").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let files = if files.is_empty() { vec![req(spec, "input")?] } else { files };
            Ok(vec![Step::Native(Native::Hash {
                files, algo: s(spec, "algo").unwrap_or_else(|| "sha256".into()),
                manifest: s(spec, "manifest"),
            })])
        }
        "transcribe" => Ok(vec![Step::Native(Native::Transcribe {
            input: req(spec, "input")?, output: req(spec, "output")?,
            translate: boolean(spec, "translate"),
            language: s(spec, "language").unwrap_or_else(|| "auto".into()),
        })]),
        "mediainfo" => Ok(vec![Step::Native(Native::MediaInfo {
            input: req(spec, "input")?, output: req(spec, "output")?,
        })]),
        "contact_sheet" => Ok(vec![Step::Native(Native::ContactSheet {
            input: req(spec, "input")?, output: req(spec, "output")?,
            cols: num(spec, "cols").unwrap_or(4.0).max(1.0) as u32,
            rows: num(spec, "rows").unwrap_or(4.0).max(1.0) as u32,
        })]),
        "cache_clean" => Ok(vec![Step::Native(Native::CacheClean)]),
        "folder_diff" => Ok(vec![Step::Native(Native::FolderDiff {
            a: req(spec, "a")?, b: req(spec, "b")?,
        })]),
        "rename" => Ok(vec![Step::Native(Native::Rename {
            dir: req(spec, "dir")?, pattern: req(spec, "pattern")?,
            start: num(spec, "start").unwrap_or(1.0) as usize,
        })]),
        "merge" => {
            let inputs: Vec<String> = spec.get("inputs").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if inputs.len() < 2 { return Err(anyhow!("merge needs at least 2 inputs")); }
            Ok(vec![Step::Native(Native::Merge { inputs, output: req(spec, "output")? })])
        }
        other => Err(anyhow!("unknown tool kind: {other}")),
    }
}

fn download_spec(spec: &Value, url: String, out_template: String) -> crate::download::DownloadSpec {
    crate::download::DownloadSpec {
        url,
        max_height: num(spec, "max_height").map(|h| h as u32),
        audio_only: boolean(spec, "audio_only"),
        embed_subs: boolean(spec, "embed_subs"),
        embed_thumbnail: boolean(spec, "embed_thumbnail"),
        cookies_from: s(spec, "cookies_from").filter(|x| !x.is_empty()),
        out_template,
    }
}

/// Parse an ffmpeg `-progress pipe:1` line → fraction `0..1` given the total
/// duration. Recognises `out_time_us=`/`out_time_ms=`. Returns None for other
/// lines. `progress=end` → Some(1.0).
pub fn parse_ffmpeg_progress(line: &str, duration_s: f64) -> Option<f64> {
    let line = line.trim();
    if line == "progress=end" { return Some(1.0); }
    if duration_s <= 0.0 { return None; }
    let (k, v) = line.split_once('=')?;
    let secs = match k {
        "out_time_us" => v.trim().parse::<f64>().ok()? / 1_000_000.0,
        "out_time_ms" => v.trim().parse::<f64>().ok()? / 1_000_000.0, // ffmpeg's "ms" is really µs
        _ => return None,
    };
    Some((secs / duration_s).clamp(0.0, 1.0))
}

/// Parse a yt-dlp progress line (`[download]  42.7% of …`) → fraction.
pub fn parse_ytdlp_progress(line: &str) -> Option<f64> {
    let line = line.trim();
    let rest = line.strip_prefix("[download]")?.trim_start();
    let pct = rest.split('%').next()?.trim();
    pct.parse::<f64>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))
}

/// Everything a yt-dlp progress line carries: fraction done, total size, speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YtdlpStats {
    pub frac: f64,
    pub total_bytes: Option<u64>,
    pub bytes_per_s: Option<u64>,
}

/// Parse a size token like `10.51MiB` / `~1.2GiB` / `831.15KiB` → bytes.
fn parse_ytdlp_size(s: &str) -> Option<u64> {
    let s = s.trim_start_matches('~');
    for (unit, mul) in [("GiB", 1u64 << 30), ("MiB", 1u64 << 20), ("KiB", 1u64 << 10), ("B", 1)] {
        if let Some(n) = s.strip_suffix(unit) {
            return n.parse::<f64>().ok().map(|v| (v * mul as f64) as u64);
        }
    }
    None
}

/// Parse a full yt-dlp progress line
/// (`[download]  42.7% of ~10.51MiB at 2.11MiB/s ETA 00:05`) → stats.
pub fn parse_ytdlp_stats(line: &str) -> Option<YtdlpStats> {
    let rest = line.trim().strip_prefix("[download]")?.trim_start();
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let mut frac = None;
    let mut total = None;
    let mut speed = None;
    for (i, t) in toks.iter().enumerate() {
        if let Some(p) = t.strip_suffix('%') {
            frac = p.parse::<f64>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0));
        } else if *t == "of" {
            total = toks.get(i + 1).and_then(|s| parse_ytdlp_size(s));
        } else if *t == "at" {
            speed = toks.get(i + 1).and_then(|s| parse_ytdlp_size(s.trim_end_matches("/s")));
        }
    }
    Some(YtdlpStats { frac: frac?, total_bytes: total, bytes_per_s: speed })
}

/// Parse a yt-dlp line that names an output file. Fragments (`x.f399.mp4`)
/// appear here too — callers keep every candidate and filter to files that
/// still exist once the job ends (yt-dlp deletes the parts after merging).
pub fn parse_ytdlp_dest(line: &str) -> Option<String> {
    let l = line.trim();
    if let Some(p) = l.strip_prefix("[download] Destination: ") { return Some(p.trim().to_string()); }
    if let Some(p) = l.strip_prefix("[ExtractAudio] Destination: ") { return Some(p.trim().to_string()); }
    if let Some(rest) = l.strip_prefix("[Merger] Merging formats into \"") {
        return rest.rsplit_once('"').map(|(p, _)| p.to_string());
    }
    if let Some(p) = l.strip_prefix("[download] ").and_then(|r| r.strip_suffix(" has already been downloaded")) {
        return Some(p.trim().to_string());
    }
    None
}

/// Parse yt-dlp's playlist item marker (`[download] Downloading item 3 of 12`)
/// → (item, total). Lets the worker scale per-item percentages into overall
/// playlist progress instead of stalling at the first item's 100%.
pub fn parse_ytdlp_item(line: &str) -> Option<(u32, u32)> {
    let rest = line.trim().strip_prefix("[download]")?.trim_start();
    let rest = rest.strip_prefix("Downloading item")?.trim_start();
    let mut it = rest.split_whitespace();
    let n: u32 = it.next()?.parse().ok()?;
    if it.next()? != "of" { return None; }
    let m: u32 = it.next()?.parse().ok()?;
    Some((n, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plans_compress_video() {
        let steps = plan("compress_video", &json!({"input":"a.mp4","output":"o.mkv","codec":"av1","crf":30})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { args, duration_input, duration_s } => {
                assert!(args.contains(&"libaom-av1".to_string()));
                assert!(args.contains(&"30".to_string()));
                assert_eq!(duration_input.as_deref(), Some("a.mp4"));
                assert_eq!(*duration_s, None);
            }
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn trim_progress_scales_by_cut_length() {
        let steps = plan("trim", &json!({"input":"a.mp4","output":"o.mp4","start_s":10,"end_s":40})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { duration_s, .. } => assert_eq!(*duration_s, Some(30.0)),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn resize_default_keeps_aspect() {
        let steps = plan("resize", &json!({"input":"a.jpg","output":"o.jpg","w":1280,"h":720})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { args, .. } =>
                assert!(args.iter().any(|a| a.contains("force_original_aspect_ratio=decrease"))),
            _ => panic!("expected ffmpeg"),
        }
        let exact = plan("resize", &json!({"input":"a.jpg","output":"o.jpg","w":100,"h":100,"mode":"exact"})).unwrap();
        match &exact[0] {
            Step::Ffmpeg { args, .. } => assert!(args.contains(&"scale=100:100".to_string())),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn plans_download_and_native() {
        let d = plan("download", &json!({"url":"https://x/y","max_height":1080})).unwrap();
        assert!(matches!(&d[0], Step::YtDlp { args } if args.iter().any(|a| a.contains("height<=?1080"))));
        let h = plan("hash", &json!({"input":"f.bin","algo":"sha256"})).unwrap();
        assert!(matches!(&h[0], Step::Native(Native::Hash { .. })));
    }

    #[test]
    fn missing_field_errors() {
        assert!(plan("trim", &json!({"input":"a.mp4","output":"o.mp4"})).is_err()); // no end_s
        assert!(plan("merge", &json!({"inputs":["only-one.mp4"]})).is_err());
    }

    #[test]
    fn progress_parsers() {
        assert_eq!(parse_ffmpeg_progress("out_time_us=5000000", 10.0), Some(0.5));
        assert_eq!(parse_ffmpeg_progress("progress=end", 10.0), Some(1.0));
        assert_eq!(parse_ffmpeg_progress("bitrate=N/A", 10.0), None);
        assert_eq!(parse_ytdlp_progress("[download]  42.0% of 10MiB"), Some(0.42));
        assert_eq!(parse_ytdlp_progress("[info] something"), None);
        assert_eq!(parse_ytdlp_item("[download] Downloading item 3 of 12"), Some((3, 12)));
        assert_eq!(parse_ytdlp_item("[download]  42.0% of 10MiB"), None);
    }

    #[test]
    fn ytdlp_stats_full_line() {
        let s = parse_ytdlp_stats("[download]  42.7% of ~10.51MiB at 2.00MiB/s ETA 00:05").unwrap();
        assert!((s.frac - 0.427).abs() < 1e-9);
        assert_eq!(s.total_bytes, Some((10.51 * 1048576.0) as u64));
        assert_eq!(s.bytes_per_s, Some(2 * 1048576));
        // terminal form: "100% of 10.51MiB in 00:05"
        let done = parse_ytdlp_stats("[download] 100% of 10.51MiB in 00:05").unwrap();
        assert_eq!(done.frac, 1.0);
        assert!(parse_ytdlp_stats("[info] whatever").is_none());
    }

    #[test]
    fn ytdlp_dest_lines() {
        assert_eq!(parse_ytdlp_dest("[download] Destination: /d/x.f399.mp4").as_deref(), Some("/d/x.f399.mp4"));
        assert_eq!(parse_ytdlp_dest("[Merger] Merging formats into \"/d/x.mp4\"").as_deref(), Some("/d/x.mp4"));
        assert_eq!(parse_ytdlp_dest("[ExtractAudio] Destination: /d/x.opus").as_deref(), Some("/d/x.opus"));
        assert_eq!(parse_ytdlp_dest("[download] /d/x.mp4 has already been downloaded").as_deref(), Some("/d/x.mp4"));
        assert_eq!(parse_ytdlp_dest("[download]  42.0% of 10MiB"), None);
    }

    #[test]
    fn download_playlist_flags() {
        let single = plan("download", &json!({"url":"https://x/y"})).unwrap();
        assert!(matches!(&single[0], Step::YtDlp { args } if args[0] == "--no-playlist"));
        let pl = plan("download_playlist", &json!({"url":"https://x/pl"})).unwrap();
        assert!(matches!(&pl[0], Step::YtDlp { args } if args[0] == "--yes-playlist"));
        let live = plan("download_live", &json!({"url":"https://x/l"})).unwrap();
        assert!(matches!(&live[0], Step::YtDlp { args } if args[0] == "--live-from-start"));
    }
}
