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
    /// adds those). `duration_input` is the file whose duration scales progress.
    Ffmpeg { args: Vec<String>, duration_input: Option<String> },
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
    /// whisper-cli + the bundled model).
    Transcribe { input: String, output: String },
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
    let one = |args: Vec<String>, dur: Option<String>| vec![Step::Ffmpeg { args, duration_input: dur }];
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
            Ok(one(args, Some(input)))
        }
        "resize" => {
            let input = req(spec, "input")?;
            let out = req(spec, "output")?;
            // The form supplies the resolved target dims directly (it knows the
            // mode + source size); fall back to a longest-edge cap.
            let (w, h) = match (num(spec, "w"), num(spec, "h")) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w as u32, h as u32),
                _ => {
                    let edge = num(spec, "edge").unwrap_or(1920.0) as u32;
                    resize::target_dims(edge, edge, resize::ResizeMode::LongestEdge(edge))
                }
            };
            let vf = resize::scale_filter(w, h);
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
            Ok(one(vec!["-i".into(), input.clone(), "-af".into(), filter, out], Some(input)))
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
            if kind == "download_live" { args.insert(0, "--live-from-start".into()); }
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
        })]),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plans_compress_video() {
        let steps = plan("compress_video", &json!({"input":"a.mp4","output":"o.mkv","codec":"av1","crf":30})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { args, duration_input } => {
                assert!(args.contains(&"libaom-av1".to_string()));
                assert!(args.contains(&"30".to_string()));
                assert_eq!(duration_input.as_deref(), Some("a.mp4"));
            }
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
    }
}
