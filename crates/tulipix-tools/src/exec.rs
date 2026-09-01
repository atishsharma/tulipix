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
    burn_subs, compress_audio, compress_video, convert, extract, filters, normalize, resize,
    thumbnail, trim, watermark,
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
    /// A binary the app does not bundle — pandoc, ebook-convert, demucs,
    /// ffsubsync. The catalogue's `needs` keeps these tiles greyed until the
    /// tool is on PATH, so this step only ever runs when it resolves.
    ///
    /// No progress is parsed: none of the four report it in a shape worth a
    /// parser, and a step that sits at its starting percentage is honest where
    /// a fabricated one is not. The log carries whatever the tool prints.
    Tool { bin: &'static str, args: Vec<String> },
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
    /// Make `b` match `a`. The worker runs `folder_diff::plan_mirror`'s actions
    /// one at a time, so a cancel lands between files rather than halfway
    /// through one.
    Mirror { a: String, b: String, delete_extra: bool },
    /// Zip a folder, or a list of files. `root` wins when both are filled in;
    /// the form says so and the preview shows which one it took.
    ArchiveCreate { root: String, files: Vec<String>, output: String, store: bool },
    ArchiveExtract { input: String, dir: String },
    ArchiveRepack { input: String, output: String, store: bool },
    /// Page numbers, or a word, on every page in the range.
    PdfStamp {
        input: String,
        output: String,
        ranges: String,
        pattern: String,
        corner: String,
        size: f64,
    },
    /// The three page operations, which differ only in what they do with the
    /// range. `ranges` stays a string: expanding a blank one to "every page"
    /// and clamping it to the document both need the page count, and that is
    /// only known once the file is open.
    PdfPages { op: PdfPageOp, input: String, output: String, ranges: String, turn: i64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PdfPageOp { Keep, Drop, Rotate }

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

/// The output an operation writes when the form left the field blank.
///
/// Twenty of the twenty-three original forms label their output field
/// "blank = beside source", and every one of them then called `req(spec,
/// "output")`, which fails on blank. `split` was found the same way in phase 2
/// and fixed by making its field required; that was the right answer for a
/// `%03d` template and the wrong one for everything else, because "beside the
/// source" is a name this can simply work out.
///
/// Path arithmetic only — nothing here touches the disk.
fn output_or_beside(spec: &Value, kind: &str, ext: Option<&str>) -> Result<String> {
    if let Some(explicit) = s(spec, "output").filter(|x| !x.trim().is_empty()) {
        return Ok(explicit);
    }
    let input = req(spec, "input")?;
    let path = std::path::Path::new(&input);
    let stem = path
        .file_stem()
        .map(|x| x.to_string_lossy().to_string())
        .ok_or_else(|| anyhow!("cannot name an output beside {input}"))?;
    let ext = match ext {
        Some(e) => e.trim_start_matches('.').to_ascii_lowercase(),
        None => path
            .extension()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_else(|| "out".into()),
    };
    // The same name the save dialog suggests, so choosing a path by hand and
    // leaving it blank do not produce two different files.
    let name = format!("{stem}-{}.{ext}", kind.replace('_', "-"));
    Ok(path.with_file_name(name).to_string_lossy().to_string())
}

/// Plan the steps for a job kind + its JSON spec.
pub fn plan(kind: &str, spec: &Value) -> Result<Vec<Step>> {
    let one = |args: Vec<String>, dur: Option<String>| vec![Step::Ffmpeg { args, duration_input: dur, duration_s: None }];
    match kind {
        "compress_video" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
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
            let codec_ext = match s(spec, "codec").as_deref() {
                Some("aac") => "m4a",
                Some("opus") => "opus",
                Some("vorbis") => "ogg",
                _ => "mp3",
            };
            let out = output_or_beside(spec, kind, Some(codec_ext))?;
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
            let format = s(spec, "format").unwrap_or_else(|| "jpg".into());
            let out = output_or_beside(spec, kind, Some(&format))?;
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
            let out = output_or_beside(spec, kind, Some(&target))?;
            match convert::media_of(&target) {
                convert::Media::Image => Ok(one(vec!["-i".into(), input, out], None)),
                _ => Ok(one(convert::av_args(&input, &target, &out), Some(input))),
            }
        }
        "audio_convert" => {
            let input = req(spec, "input")?;
            let target = req(spec, "target_ext")?;
            let out = output_or_beside(spec, kind, Some(&target))?;
            let mut args = vec!["-i".into(), input.clone(), "-vn".into()];
            if let Some(codec) = convert::default_codec(&target) {
                args.extend(["-c:a".into(), codec.to_string()]);
            }
            // FLAC and WAV are lossless. A bitrate on them is meaningless, and
            // ffmpeg complains rather than ignoring it.
            if !matches!(target.as_str(), "flac" | "wav" | "aiff") {
                let kbps = num(spec, "kbps").unwrap_or(192.0).clamp(32.0, 512.0) as u32;
                args.extend(["-b:a".into(), format!("{kbps}k")]);
            }
            args.push(out);
            Ok(one(args, Some(input)))
        }
        "image_convert" => {
            let input = req(spec, "input")?;
            let target = req(spec, "target_ext")?;
            let out = output_or_beside(spec, kind, Some(&target))?;
            let q = num(spec, "quality").unwrap_or(85.0).clamp(1.0, 100.0) as u32;
            let args = match target.as_str() {
                // PNG is lossless. Wiring the quality slider to anything here
                // would be a lie told by the form.
                "png" => vec!["-i".into(), input, out],
                "webp" => vec![
                    "-i".into(), input, "-c:v".into(), "libwebp".into(),
                    "-quality".into(), q.to_string(), out,
                ],
                "avif" => vec![
                    "-i".into(), input, "-c:v".into(), "libaom-av1".into(),
                    "-crf".into(), (63 - q * 63 / 100).to_string(), out,
                ],
                _ => {
                    let qv = 2 + (31 - 2) * (100 - q) / 100;
                    vec!["-i".into(), input, "-q:v".into(), qv.to_string(), out]
                }
            };
            Ok(one(args, None))
        }
        "trim" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
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
            let out = output_or_beside(spec, kind, None)?;
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
        "crop" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let vf = match s(spec, "aspect").as_deref().and_then(filters::parse_aspect) {
                Some((rw, rh)) => filters::crop_to_aspect(rw, rh),
                // "manual", and anything unparseable, means the box below.
                None => filters::crop_box(
                    num(spec, "w").unwrap_or(0.0).max(0.0) as u32,
                    num(spec, "h").unwrap_or(0.0).max(0.0) as u32,
                    num(spec, "x").unwrap_or(0.0).max(0.0) as u32,
                    num(spec, "y").unwrap_or(0.0).max(0.0) as u32,
                ),
            };
            Ok(one(vec!["-i".into(), input.clone(), "-vf".into(), vf, out], Some(input)))
        }
        "rotate" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let turn = num(spec, "turn").unwrap_or(0.0).max(0.0) as u32;
            let vf = filters::orient_filter(turn / 90, boolean(spec, "hflip"), boolean(spec, "vflip"));
            Ok(one(vec!["-i".into(), input.clone(), "-vf".into(), vf, out], Some(input)))
        }
        "denoise" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let vf = filters::denoise_filter(
                num(spec, "strength").unwrap_or(4.0),
                boolean(spec, "deblock"),
            );
            Ok(one(vec!["-i".into(), input.clone(), "-vf".into(), vf, out], Some(input)))
        }
        "thumbnail" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, Some("jpg"))?;
            let at = num(spec, "at_s").unwrap_or(1.0);
            let width = num(spec, "width").unwrap_or(320.0) as u32;
            Ok(one(thumbnail::thumb_args(&input, at, width, &out), None))
        }
        "extract" => {
            let input = req(spec, "input")?;
            let (stream, ext) = match s(spec, "stream").as_deref() {
                Some("subtitle") => (extract::Kind::Subtitle, "srt"),
                Some("attachment") => (extract::Kind::Attachment, "bin"),
                _ => (extract::Kind::Audio, "m4a"),
            };
            let out = output_or_beside(spec, kind, Some(ext))?;
            let kind = stream;
            let index = num(spec, "index").unwrap_or(0.0) as u32;
            Ok(one(extract::extract_stream_args(&input, kind, index, &out), None))
        }
        "normalize" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let lufs = num(spec, "lufs").unwrap_or(-23.0);
            let filter = normalize::loudnorm_measure_filter(lufs); // single-pass loudnorm
            // Copy the video stream (incl. mp3 cover art) — only audio changes;
            // without this ffmpeg silently re-encoded the whole video track.
            Ok(one(vec!["-i".into(), input.clone(), "-af".into(), filter, "-c:v".into(), "copy".into(), out], Some(input)))
        }
        "denoise_audio" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let af = filters::audio_denoise_filter(
                num(spec, "reduction").unwrap_or(12.0),
                boolean(spec, "rumble"),
            );
            // Copy the video stream for the same reason normalize does: only
            // the audio is being changed, and re-encoding the picture to fix
            // some hiss is not what anyone asked for.
            Ok(one(
                vec![
                    "-i".into(), input.clone(), "-af".into(), af,
                    "-c:v".into(), "copy".into(), out,
                ],
                Some(input),
            ))
        }
        "watermark" => {
            let input = req(spec, "input")?;
            let out = output_or_beside(spec, kind, None)?;
            let text = req(spec, "text")?;
            let filter = watermark::text_filter(&text, watermark::Position::BottomRight, 24, 0.85, 28);
            Ok(one(vec!["-i".into(), input, "-vf".into(), filter, out], None))
        }
        "burn_subs" => {
            let input = req(spec, "input")?;
            let sub = req(spec, "sub")?;
            let out = output_or_beside(spec, kind, None)?;
            Ok(one(burn_subs::args(&input, &sub, None, &out), Some(input)))
        }
        "add_subs" => {
            let input = req(spec, "input")?;
            let sub = req(spec, "sub")?;
            let out = output_or_beside(spec, kind, None)?;
            // What a container can carry, not what the file happens to be:
            // mp4 takes only its own text format.
            let ext = std::path::Path::new(&out)
                .extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default();
            let lang = s(spec, "language").unwrap_or_else(|| "und".into());
            Ok(one(
                vec![
                    "-i".into(), input.clone(), "-i".into(), sub,
                    "-map".into(), "0".into(), "-map".into(), "1".into(),
                    "-c".into(), "copy".into(),
                    "-c:s".into(), filters::soft_sub_codec(&ext).into(),
                    "-metadata:s:s:0".into(), format!("language={lang}"),
                    out,
                ],
                Some(input),
            ))
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
            // The one choke point for all three download tools: the user's
            // cookies and any player-client override, applied here so no
            // front-end has to remember to. `DownloadSpec::args` stays pure and
            // testable; only a job that named its own browser keeps that
            // instead — an explicit per-job choice outranks the global one.
            //
            // Spliced in at the FRONT, never appended: `DownloadSpec::args`
            // puts the URL last and sec-tools reads `args.last()` to name the
            // queue row after the video. Options may sit anywhere before it.
            let mut extra = Vec::new();
            if dl.cookies_from.is_none() {
                extra.extend(tulipix_core::ytdlp::cookie_args());
            }
            extra.extend(tulipix_core::ytdlp::player_client_args());
            args.splice(0..0, extra);
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
            input: req(spec, "input")?, output: output_or_beside(spec, kind, Some("srt"))?,
            translate: boolean(spec, "translate"),
            language: s(spec, "language").unwrap_or_else(|| "auto".into()),
        })]),
        "mediainfo" => Ok(vec![Step::Native(Native::MediaInfo {
            input: req(spec, "input")?, output: output_or_beside(spec, kind, Some("txt"))?,
        })]),
        "contact_sheet" => Ok(vec![Step::Native(Native::ContactSheet {
            input: req(spec, "input")?, output: output_or_beside(spec, kind, Some("jpg"))?,
            cols: num(spec, "cols").unwrap_or(4.0).max(1.0) as u32,
            rows: num(spec, "rows").unwrap_or(4.0).max(1.0) as u32,
        })]),
        "cache_clean" => Ok(vec![Step::Native(Native::CacheClean)]),
        "archive_create" => {
            let files: Vec<String> = spec.get("files").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let root = s(spec, "folder").unwrap_or_default();
            if root.trim().is_empty() && files.is_empty() {
                return Err(anyhow!("choose a folder or some files to archive"));
            }
            Ok(vec![Step::Native(Native::ArchiveCreate {
                root, files, output: req(spec, "output")?,
                store: boolean(spec, "store"),
            })])
        }
        "archive_extract" => Ok(vec![Step::Native(Native::ArchiveExtract {
            input: req(spec, "input")?, dir: req(spec, "dir")?,
        })]),
        "archive_repack" => Ok(vec![Step::Native(Native::ArchiveRepack {
            input: req(spec, "input")?, output: req(spec, "output")?,
            store: boolean(spec, "store"),
        })]),
        "pdf_stamp" => Ok(vec![Step::Native(Native::PdfStamp {
            input: req(spec, "input")?,
            output: output_or_beside(spec, kind, Some("pdf"))?,
            ranges: s(spec, "ranges").unwrap_or_default(),
            pattern: req(spec, "pattern")?,
            corner: s(spec, "corner").unwrap_or_else(|| "bottom_centre".into()),
            size: num(spec, "size").unwrap_or(10.0),
        })]),
        "mirror" => Ok(vec![Step::Native(Native::Mirror {
            a: req(spec, "a")?, b: req(spec, "b")?,
            delete_extra: boolean(spec, "delete_extra"),
        })]),
        "pdf_pages" | "pdf_delete" | "pdf_rotate" => {
            let op = match kind {
                "pdf_delete" => PdfPageOp::Drop,
                "pdf_rotate" => PdfPageOp::Rotate,
                _ => PdfPageOp::Keep,
            };
            // Rotate is the one that may name no pages, because blank means
            // every page. Keeping or deleting nothing is a mistake, not a
            // shorthand.
            let ranges = match op {
                PdfPageOp::Rotate => s(spec, "ranges").unwrap_or_default(),
                _ => req(spec, "ranges")?,
            };
            Ok(vec![Step::Native(Native::PdfPages {
                op,
                input: req(spec, "input")?,
                output: output_or_beside(spec, kind, Some("pdf"))?,
                ranges,
                turn: crate::pdf::normalise_turn(num(spec, "turn").unwrap_or(90.0) as i64),
            })])
        }
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
        // ---------------------------------------------- optional binaries --
        // Four operations that shell out to something the app does not ship.
        // Each is argv and nothing else: the tool already does the work, and
        // wrapping it in Rust would only add a second place for it to be wrong.
        "doc_convert" => {
            let input = req(spec, "input")?;
            let fmt = s(spec, "format").unwrap_or_else(|| "docx".into());
            let output = output_or_beside(spec, kind, Some(&fmt))?;
            // `-s` writes a whole document rather than a fragment. It matters
            // for html and latex and is ignored by the binary formats.
            Ok(vec![Step::Tool { bin: "pandoc", args: vec!["-s".into(), input, "-o".into(), output] }])
        }
        "ebook_convert" => {
            let input = req(spec, "input")?;
            let fmt = s(spec, "format").unwrap_or_else(|| "epub".into());
            let output = output_or_beside(spec, kind, Some(&fmt))?;
            // ebook-convert takes both paths positionally and reads the format
            // off the output extension.
            let mut args = vec![input, output];
            if let Some(title) = s(spec, "title").filter(|x| !x.trim().is_empty()) {
                args.push("--title".into());
                args.push(title);
            }
            if let Some(author) = s(spec, "author").filter(|x| !x.trim().is_empty()) {
                args.push("--authors".into());
                args.push(author);
            }
            Ok(vec![Step::Tool { bin: "ebook-convert", args }])
        }
        "stems" => {
            let input = req(spec, "input")?;
            // demucs writes a tree, not a file: `<dir>/<model>/<track>/vocals.*`.
            // So the output field is a folder, and blank means beside the source.
            let dir = match s(spec, "output").filter(|x| !x.trim().is_empty()) {
                Some(d) => d,
                None => std::path::Path::new(&input)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".into()),
            };
            let mut args = vec!["-o".into(), dir];
            if s(spec, "mode").as_deref() == Some("vocals") {
                args.push("--two-stems".into());
                args.push("vocals".into());
            }
            if boolean(spec, "mp3") {
                args.push("--mp3".into());
            }
            args.push(input);
            Ok(vec![Step::Tool { bin: "demucs", args }])
        }
        "subs_sync" => {
            let input = req(spec, "input")?;
            let subs = req(spec, "sub")?;
            // Named off the subtitle rather than the video: the result is a
            // subtitle, and beside the video is where the player looks for it.
            let output = match s(spec, "output").filter(|x| !x.trim().is_empty()) {
                Some(o) => o,
                None => output_or_beside(&serde_json::json!({ "input": subs }), kind, Some("srt"))?,
            };
            Ok(vec![Step::Tool {
                bin: "ffsubsync",
                args: vec![input, "-i".into(), subs, "-o".into(), output],
            }])
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

    #[test]
    fn a_blank_output_names_itself_beside_the_source() {
        // Twenty forms promise "blank = beside source" and `req(spec, "output")`
        // made every one of them fail instead. Same bug `split` had.
        let v = plan("compress_video", &json!({"input":"/m/holiday.mkv","crf":22})).unwrap();
        match &v[0] {
            Step::Ffmpeg { args, .. } => assert_eq!(args.last().unwrap(), "/m/holiday-compress-video.mkv"),
            _ => panic!("expected ffmpeg"),
        }
        // The extension follows the target format where there is one.
        let c = plan("convert", &json!({"input":"/m/clip.mov","target_ext":"mp4"})).unwrap();
        match &c[0] {
            Step::Ffmpeg { args, .. } => assert_eq!(args.last().unwrap(), "/m/clip-convert.mp4"),
            _ => panic!("expected ffmpeg"),
        }
        // An explicit path still wins.
        let e = plan("resize", &json!({"input":"/m/a.jpg","output":"/other/b.jpg"})).unwrap();
        match &e[0] {
            Step::Ffmpeg { args, .. } => assert_eq!(args.last().unwrap(), "/other/b.jpg"),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn the_page_operations_carry_their_range_unexpanded() {
        let keep = plan("pdf_pages", &json!({"input":"/d/scan.pdf","ranges":"1-3,7"})).unwrap();
        match &keep[0] {
            Step::Native(Native::PdfPages { op, ranges, output, .. }) => {
                assert_eq!(*op, PdfPageOp::Keep);
                assert_eq!(ranges, "1-3,7");
                assert_eq!(output, "/d/scan-pdf-pages.pdf");
            }
            _ => panic!("expected a native pdf step"),
        }
        // Blank means every page, but only for the one where that is sensible.
        let turn = plan("pdf_rotate", &json!({"input":"/d/scan.pdf","turn":"270"})).unwrap();
        match &turn[0] {
            Step::Native(Native::PdfPages { op, ranges, turn, .. }) => {
                assert_eq!(*op, PdfPageOp::Rotate);
                assert!(ranges.is_empty());
                assert_eq!(*turn, 270);
            }
            _ => panic!("expected a native pdf step"),
        }
        // Deleting nothing, or keeping nothing, is a mistake rather than a
        // shorthand for everything.
        assert!(plan("pdf_delete", &json!({"input":"/d/scan.pdf"})).is_err());
        assert!(plan("pdf_pages", &json!({"input":"/d/scan.pdf"})).is_err());
    }

    #[test]
    fn an_archive_needs_something_to_put_in_it() {
        assert!(plan("archive_create", &json!({"output":"/o.zip"})).is_err());
        let folder = plan("archive_create", &json!({"folder":"/pics","output":"/o.zip"})).unwrap();
        assert!(matches!(&folder[0], Step::Native(Native::ArchiveCreate { root, .. }) if root == "/pics"));
        // A folder wins over a file list, and the form says so.
        let both = plan("archive_create", &json!({
            "folder":"/pics","files":["/a.txt"],"output":"/o.zip"
        })).unwrap();
        match &both[0] {
            Step::Native(Native::ArchiveCreate { root, files, .. }) => {
                assert_eq!(root, "/pics");
                assert_eq!(files.len(), 1);
            }
            _ => panic!("expected a native archive step"),
        }
        // A file list alone is enough.
        assert!(plan("archive_create", &json!({"files":["/a.txt"],"output":"/o.zip"})).is_ok());
    }

    #[test]
    fn a_stamp_defaults_to_every_page() {
        let steps = plan("pdf_stamp", &json!({"input":"/d/scan.pdf","pattern":"Page {n}"})).unwrap();
        match &steps[0] {
            Step::Native(Native::PdfStamp { ranges, pattern, corner, output, .. }) => {
                assert!(ranges.is_empty(), "blank means every page");
                assert_eq!(pattern, "Page {n}");
                assert_eq!(corner, "bottom_centre");
                assert_eq!(output, "/d/scan-pdf-stamp.pdf");
            }
            _ => panic!("expected a native stamp step"),
        }
        // Without something to write there is nothing to stamp.
        assert!(plan("pdf_stamp", &json!({"input":"/d/scan.pdf"})).is_err());
    }

    #[test]
    fn mirror_will_not_delete_unless_it_was_asked() {
        let steps = plan("mirror", &json!({"a":"/one","b":"/two"})).unwrap();
        match &steps[0] {
            Step::Native(Native::Mirror { delete_extra, .. }) =>
                assert!(!delete_extra, "the destructive half is off by default"),
            _ => panic!("expected a native mirror step"),
        }
        let on = plan("mirror", &json!({"a":"/one","b":"/two","delete_extra":true})).unwrap();
        assert!(matches!(&on[0], Step::Native(Native::Mirror { delete_extra: true, .. })));
        assert!(plan("mirror", &json!({"a":"/one"})).is_err());
    }

    #[test]
    fn crop_takes_a_ratio_or_a_box() {
        let ratio = plan("crop", &json!({"input":"/m/a.mp4","aspect":"16:9"})).unwrap();
        match &ratio[0] {
            Step::Ffmpeg { args, .. } => {
                let vf = args.iter().position(|a| a == "-vf").unwrap();
                // An unescaped comma there would end the filter early.
                assert!(args[vf + 1].starts_with("crop=w=min(iw\\,"), "{}", args[vf + 1]);
            }
            _ => panic!("expected ffmpeg"),
        }
        let manual = plan("crop", &json!({"input":"/m/a.mp4","aspect":"manual","w":1920,"h":800,"x":0,"y":140})).unwrap();
        match &manual[0] {
            Step::Ffmpeg { args, .. } => assert!(args.contains(&"crop=1920:800:0:140".to_string())),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn rotate_and_denoise_build_their_chains() {
        let r = plan("rotate", &json!({"input":"/m/a.mp4","turn":"270","hflip":"true"})).unwrap();
        match &r[0] {
            Step::Ffmpeg { args, .. } => assert!(args.contains(&"transpose=2,hflip".to_string())),
            _ => panic!("expected ffmpeg"),
        }
        let d = plan("denoise", &json!({"input":"/m/a.mp4","strength":8,"deblock":true})).unwrap();
        match &d[0] {
            Step::Ffmpeg { args, .. } => assert!(
                args.iter().any(|a| a.starts_with("hqdn3d=8.00") && a.contains("deblock=")),
                "{args:?}"
            ),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn a_soft_subtitle_track_copies_everything_else() {
        let steps = plan("add_subs", &json!({"input":"/m/film.mp4","sub":"/m/film.srt","language":"fra"})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { args, .. } => {
                // mp4 carries only its own text format.
                assert!(args.windows(2).any(|w| w == ["-c:s", "mov_text"]));
                assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
                assert!(args.contains(&"language=fra".to_string()));
                assert_eq!(args.last().unwrap(), "/m/film-add-subs.mp4");
            }
            _ => panic!("expected ffmpeg"),
        }
        // Matroska takes SRT as it is.
        let mkv = plan("add_subs", &json!({"input":"/m/film.mkv","sub":"/m/film.srt"})).unwrap();
        match &mkv[0] {
            Step::Ffmpeg { args, .. } => assert!(args.windows(2).any(|w| w == ["-c:s", "srt"])),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn the_converters_know_what_is_lossless() {
        let mp3 = plan("audio_convert", &json!({"input":"/m/a.wav","target_ext":"mp3","kbps":256})).unwrap();
        match &mp3[0] {
            Step::Ffmpeg { args, .. } => {
                assert!(args.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
                assert!(args.contains(&"256k".to_string()));
            }
            _ => panic!("expected ffmpeg"),
        }
        let flac = plan("audio_convert", &json!({"input":"/m/a.wav","target_ext":"flac","kbps":256})).unwrap();
        match &flac[0] {
            Step::Ffmpeg { args, .. } =>
                assert!(!args.iter().any(|a| a == "-b:a"), "a bitrate on FLAC is meaningless"),
            _ => panic!("expected ffmpeg"),
        }
        let png = plan("image_convert", &json!({"input":"/m/a.jpg","target_ext":"png","quality":50})).unwrap();
        match &png[0] {
            Step::Ffmpeg { args, .. } =>
                assert!(!args.iter().any(|a| a == "-q:v"), "PNG has no quality to set"),
            _ => panic!("expected ffmpeg"),
        }
        let webp = plan("image_convert", &json!({"input":"/m/a.jpg","target_ext":"webp","quality":50})).unwrap();
        match &webp[0] {
            Step::Ffmpeg { args, .. } => assert!(args.windows(2).any(|w| w == ["-quality", "50"])),
            _ => panic!("expected ffmpeg"),
        }
    }

    #[test]
    fn cleaning_noise_leaves_the_picture_alone() {
        let steps = plan("denoise_audio", &json!({"input":"/m/talk.mp4","reduction":18,"rumble":true})).unwrap();
        match &steps[0] {
            Step::Ffmpeg { args, .. } => {
                assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
                assert!(args.iter().any(|a| a.starts_with("highpass=f=80,afftdn=nr=18.0")), "{args:?}");
            }
            _ => panic!("expected ffmpeg"),
        }
    }

    // ------------------------------------------------- optional binaries --

    fn tool_argv(kind: &str, spec: Value) -> (&'static str, Vec<String>) {
        match plan(kind, &spec).unwrap().remove(0) {
            Step::Tool { bin, args } => (bin, args),
            other => panic!("expected an external tool, got {other:?}"),
        }
    }

    #[test]
    fn pandoc_takes_its_format_from_the_name_it_writes() {
        let (bin, args) = tool_argv("doc_convert", json!({"input":"/n/notes.md","format":"docx"}));
        assert_eq!(bin, "pandoc");
        // Blank output, so the extension has to come from the chosen format —
        // pandoc reads the format off the output name and nothing else.
        assert_eq!(args, vec!["-s", "/n/notes.md", "-o", "/n/notes-doc-convert.docx"]);
    }

    #[test]
    fn metadata_only_goes_to_ebook_convert_when_it_was_typed() {
        let (_, bare) = tool_argv("ebook_convert", json!({"input":"/b/x.mobi","format":"epub"}));
        assert_eq!(bare, vec!["/b/x.mobi", "/b/x-ebook-convert.epub"]);
        // An empty box must not become `--title ""`, which would wipe the title
        // the book already has.
        let (_, blank) =
            tool_argv("ebook_convert", json!({"input":"/b/x.mobi","format":"epub","title":"  "}));
        assert_eq!(blank, bare);
        let (_, named) =
            tool_argv("ebook_convert", json!({"input":"/b/x.mobi","format":"epub","title":"Dune"}));
        assert!(named.windows(2).any(|w| w == ["--title", "Dune"]));
    }

    #[test]
    fn demucs_is_given_a_folder_and_the_track_goes_last() {
        let (bin, args) = tool_argv("stems", json!({"input":"/m/song.flac","mode":"vocals"}));
        assert_eq!(bin, "demucs");
        assert_eq!(args.first().map(String::as_str), Some("-o"));
        // Blank output means beside the source — the folder, not the file.
        assert_eq!(args[1], "/m");
        assert!(args.windows(2).any(|w| w == ["--two-stems", "vocals"]));
        assert_eq!(args.last().map(String::as_str), Some("/m/song.flac"));

        let (_, four) = tool_argv("stems", json!({"input":"/m/song.flac","mode":"four","mp3":true}));
        assert!(!four.iter().any(|a| a == "--two-stems"));
        assert!(four.iter().any(|a| a == "--mp3"));
    }

    #[test]
    fn a_synced_subtitle_is_named_after_the_subtitle() {
        // The video is the reference, not the thing being written. Naming the
        // output after it would put an .srt beside the film called
        // `film-subs-sync.srt` and leave the actual subtitle out of its name.
        let (bin, args) =
            tool_argv("subs_sync", json!({"input":"/v/film.mkv","sub":"/v/dutch.srt"}));
        assert_eq!(bin, "ffsubsync");
        assert_eq!(args, vec!["/v/film.mkv", "-i", "/v/dutch.srt", "-o", "/v/dutch-subs-sync.srt"]);
    }

    // ------------------------------------------------------ catalogue ↔ plan --

    /// A spec with every field of an op filled in with something of the right
    /// shape. Not necessarily meaningful — the agreement test only cares which
    /// kinds `plan` recognises, not what it builds for them.
    fn dummy_spec(op: &crate::catalog::OpDef) -> Value {
        use crate::catalog::FieldKind;
        let mut m = serde_json::Map::new();
        for f in op.fields {
            let v = match f.kind {
                FieldKind::Files => json!(["/tmp/a.mp4", "/tmp/b.mp4"]),
                FieldKind::Toggle => json!(f.value),
                FieldKind::Number | FieldKind::Slider => {
                    json!(if f.value.is_empty() { "1" } else { f.value })
                }
                _ => json!(if f.value.is_empty() { "/tmp/x.mp4" } else { f.value }),
            };
            m.insert(f.key.to_string(), v);
        }
        // A blank output is legal now — `output_or_beside` names it — but this
        // spec fills it in so the agreement test is about which kinds `plan`
        // knows, and nothing else.
        m.entry("output".to_string()).or_insert(json!("/tmp/out.mp4"));
        Value::Object(m)
    }

    #[test]
    fn every_catalogued_op_has_a_plan_arm() {
        for op in crate::catalog::CATALOG {
            let err = plan(op.kind, &dummy_spec(op))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                !err.starts_with("unknown tool kind"),
                "{} is in the catalogue but plan() has no arm for it",
                op.kind
            );
        }
    }

    #[test]
    fn retired_kinds_stay_retired() {
        // `metadata` and `pdf` were listed as CLI subcommands for months with
        // no planner arm behind either. The reverse direction — a plan arm with
        // no catalogue entry — cannot be enumerated without a second list, so
        // this guards the two that actually went wrong.
        for gone in ["metadata", "pdf", "queue"] {
            assert!(crate::catalog::get(gone).is_none(), "{gone} is back in the catalogue");
            assert!(plan(gone, &json!({})).is_err(), "plan() accepts {gone}");
        }
    }
}
