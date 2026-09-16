//! What `yt-dlp -J` says a video can be played as: the rows of the format
//! panel, and the selector strings that turn a chosen row back into streams.
//!
//! Only split streams are rows. YouTube serves picture and sound separately
//! above 360p, so a muxed format is never the answer to "which quality" --
//! format 18 is what the old Watch button was silently getting.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoFormat {
    pub id: String,
    pub height: u32,
    pub fps: u32,
    /// Codec family: `avc1`, `vp09`, `av01`, or the raw prefix for anything else.
    pub vcodec: String,
    pub ext: String,
    pub tbr_kbps: u32,
    pub hdr: bool,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub id: String,
    /// Codec family: `opus`, `mp4a`, or the raw prefix.
    pub acodec: String,
    pub ext: String,
    pub abr_kbps: u32,
    pub rate_hz: u32,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start_s: f64,
    pub title: String,
}

/// A caption track YouTube serves as WebVTT.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Caption {
    /// `en`, `en-GB`, `pt-BR` …
    pub lang: String,
    /// "English", "English (auto-generated)".
    pub name: String,
    pub url: String,
    /// Speech recognition or machine translation rather than the uploader's.
    pub auto: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub video: Vec<VideoFormat>,
    pub audio: Vec<AudioFormat>,
    pub chapters: Vec<Chapter>,
    pub duration_s: f64,
    pub channel_id: String,
    // The video's page. Defaulted, so rows saved before them still load.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub description: String,
    /// `YYYYMMDD`, or "".
    #[serde(default)]
    pub upload_date: String,
    /// -1 when YouTube does not say.
    #[serde(default)]
    pub views: i64,
    #[serde(default)]
    pub likes: i64,
    /// The uploader's tracks, and the speech-recognition one in the video's
    /// own language. Not the hundred machine translations. The URLs expire
    /// within hours, so they are read from a fresh answer, never a saved one.
    #[serde(default)]
    pub captions: Vec<Caption>,
}

/// WebVTT tracks: every uploaded one, and the automatic ones whose language
/// is the video's own (`en-orig`, or the plain code when that is all there is).
fn captions(j: &Value) -> Vec<Caption> {
    let vtt = |tracks: &Value| {
        tracks.as_array().and_then(|t| {
            t.iter()
                .find(|x| x["ext"] == "vtt")
                .map(|x| (text(x, "url"), text(x, "name")))
                .filter(|(u, _)| !u.is_empty())
        })
    };
    let mut out = Vec::new();
    if let Some(map) = j["subtitles"].as_object() {
        for (lang, tracks) in map {
            if lang == "live_chat" {
                continue;
            }
            if let Some((url, name)) = vtt(tracks) {
                out.push(Caption { lang: lang.clone(), name, url, auto: false });
            }
        }
    }
    let spoken = j["language"].as_str().unwrap_or("");
    if let Some(map) = j["automatic_captions"].as_object() {
        let key = [format!("{spoken}-orig"), spoken.to_string()]
            .into_iter()
            .find(|k| !spoken.is_empty() && map.contains_key(k))
            .or_else(|| map.keys().find(|k| k.ends_with("-orig")).cloned());
        if let Some(key) = key {
            if let Some((url, name)) = vtt(&map[&key]) {
                let lang = key.trim_end_matches("-orig").to_string();
                out.push(Caption { lang, name, url, auto: true });
            }
        }
    }
    out
}

/// The track to offer while watching: the uploader's in `lang` (`en` also
/// takes `en-GB`), else the automatic one in `lang`, else the uploader's in
/// any language when there is exactly one.
pub fn pick_caption<'a>(captions: &'a [Caption], lang: &str) -> Option<&'a Caption> {
    let is = |c: &Caption| c.lang == lang || c.lang.split('-').next() == Some(lang);
    captions
        .iter()
        .find(|c| !c.auto && is(c))
        .or_else(|| captions.iter().find(|c| c.auto && is(c)))
        .or_else(|| {
            let manual: Vec<_> = captions.iter().filter(|c| !c.auto).collect();
            (manual.len() == 1).then(|| manual[0])
        })
}

/// `vp9` and `vp09.00.40.08` are the same codec to anyone choosing between them.
fn family(codec: &str) -> String {
    let head = codec.split('.').next().unwrap_or(codec);
    match head {
        "vp9" | "vp09" => "vp09".into(),
        other => other.to_string(),
    }
}

fn text(f: &Value, key: &str) -> String {
    f[key].as_str().unwrap_or("").to_string()
}

/// `none` and a missing key mean the same thing: this stream has no such track.
fn has(f: &Value, key: &str) -> bool {
    f[key].as_str().is_some_and(|c| !c.is_empty() && c != "none")
}

fn is_hls(f: &Value) -> bool {
    f["protocol"].as_str().is_some_and(|p| p.starts_with("m3u8"))
}

fn round(v: &Value) -> u32 {
    v.as_f64().unwrap_or(0.0).round().max(0.0) as u32
}

pub fn parse_info(j: &Value) -> StreamInfo {
    let formats = j["formats"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let video_only = |f: &Value| has(f, "vcodec") && !has(f, "acodec");
    let audio_only = |f: &Value| has(f, "acodec") && !has(f, "vcodec");
    // HLS rows duplicate the https ones at a higher nominal bitrate; they are
    // only worth listing when there is nothing else, which is a live stream.
    let https_video = formats.iter().any(|f| video_only(f) && !is_hls(f));

    let mut video = Vec::new();
    let mut audio = Vec::new();
    for f in formats {
        let id = text(f, "format_id");
        if id.is_empty() || id.starts_with("sb") {
            continue;
        }
        let bytes = f["filesize"].as_u64().or_else(|| f["filesize_approx"].as_u64());
        if video_only(f) {
            let Some(height) = f["height"].as_u64() else { continue };
            if is_hls(f) && https_video {
                continue;
            }
            video.push(VideoFormat {
                id,
                height: height as u32,
                fps: round(&f["fps"]),
                vcodec: family(f["vcodec"].as_str().unwrap_or("")),
                ext: text(f, "ext"),
                tbr_kbps: round(&f["tbr"]),
                hdr: f["dynamic_range"].as_str().is_some_and(|d| d != "SDR"),
                bytes,
            });
        } else if audio_only(f) && !is_hls(f) {
            let abr = if f["abr"].is_number() { &f["abr"] } else { &f["tbr"] };
            audio.push(AudioFormat {
                id,
                acodec: family(f["acodec"].as_str().unwrap_or("")),
                ext: text(f, "ext"),
                abr_kbps: round(abr),
                rate_hz: f["asr"].as_u64().unwrap_or(0) as u32,
                bytes,
            });
        }
    }

    // Best bitrate first within each (height, fps, family), then keep one.
    video.sort_by(|a, b| {
        b.height
            .cmp(&a.height)
            .then(b.fps.cmp(&a.fps))
            .then(a.vcodec.cmp(&b.vcodec))
            .then(b.tbr_kbps.cmp(&a.tbr_kbps))
    });
    video.dedup_by(|later, kept| {
        later.height == kept.height && later.fps == kept.fps && later.vcodec == kept.vcodec
    });
    audio.sort_by(|a, b| b.abr_kbps.cmp(&a.abr_kbps));

    let chapters = j["chapters"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| Chapter {
                    start_s: c["start_time"].as_f64().unwrap_or(0.0),
                    title: text(c, "title"),
                })
                .collect()
        })
        .unwrap_or_default();

    StreamInfo {
        video,
        audio,
        chapters,
        duration_s: j["duration"].as_f64().unwrap_or(0.0),
        channel_id: text(j, "channel_id"),
        title: text(j, "title"),
        channel: j["channel"].as_str().or(j["uploader"].as_str()).unwrap_or("").to_string(),
        description: text(j, "description"),
        upload_date: text(j, "upload_date"),
        views: j["view_count"].as_i64().unwrap_or(-1),
        likes: j["like_count"].as_i64().unwrap_or(-1),
        captions: captions(j),
    }
}

/// The "watch at" default: a height cap and a codec family. A format id cannot
/// be the default, because ids differ from one video to the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchPref {
    /// 0 = no cap.
    pub height: u32,
    /// `any`, `avc1`, `vp09` or `av01`. `any` is the default and means the
    /// cheapest to decode: H.264, which nearly every GPU decodes in hardware,
    /// then VP9, then whatever is best. YouTube's own best is usually AV1,
    /// which most machines decode in software -- on a 2015 laptop that is a
    /// core and more at 1080p60, and 4K does not keep up at all.
    pub codec: String,
}

impl Default for WatchPref {
    fn default() -> Self {
        WatchPref { height: 0, codec: "any".into() }
    }
}

impl WatchPref {
    /// `"1080/avc1"`. Anything unreadable is the default; an unknown codec is `any`.
    pub fn parse(s: &str) -> WatchPref {
        let Some((h, c)) = s.split_once('/') else { return WatchPref::default() };
        let Ok(height) = h.trim().parse::<u32>() else { return WatchPref::default() };
        let codec = match c.trim() {
            "avc1" | "vp09" | "av01" => c.trim().to_string(),
            _ => "any".into(),
        };
        WatchPref { height, codec }
    }

    pub fn to_pref_string(&self) -> String {
        format!("{}/{}", self.height, self.codec)
    }
}

/// A format id is digits, letters and dashes (`137`, `251-drc`). Anything else
/// is not one, and is not worth handing to a process as a selector.
fn is_format_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The `-f` selector for watching. A picked row plays exactly that picture with
/// the best sound; no pick means the remembered preference, falling back step by
/// step so a video without that codec or height still plays.
pub fn watch_selector(format_id: &str, pref: &WatchPref) -> String {
    if is_format_id(format_id) {
        return format!("{format_id}+bestaudio/best");
    }
    let cap = if pref.height > 0 { format!("[height<={}]", pref.height) } else { String::new() };
    let mut alts = Vec::new();
    // Above 1080p H.264 does not exist: a cap over it asks for the taller
    // picture first, VP9 before AV1, and falls back to the 1080p chain.
    if pref.codec == "any" && pref.height > 1080 {
        let tall = format!("[height<={}][height>1080]", pref.height);
        alts.push(format!("bv{tall}[vcodec^=vp]+ba"));
        alts.push(format!("bv{tall}+ba"));
        let hd = WatchPref { height: 1080, codec: "any".into() };
        alts.extend(watch_selector("", &hd).split('/').map(String::from));
        alts.push("b".into());
        alts.dedup();
        return alts.join("/");
    }
    let prefixes: &[&str] = match pref.codec.as_str() {
        "any" => &["avc1", "vp"],
        "vp09" => &["vp"],
        other => &[other],
    };
    for prefix in prefixes {
        alts.push(format!("bv{cap}[vcodec^={prefix}]+ba"));
    }
    alts.push(format!("bv{cap}+ba"));
    alts.push(format!("b{cap}"));
    if !cap.is_empty() {
        alts.push("b".into());
    }
    alts.join("/")
}

/// The `-f` selector for listening.
pub fn audio_selector(format_id: &str) -> String {
    if is_format_id(format_id) {
        format!("{format_id}/bestaudio/best")
    } else {
        "bestaudio/best".into()
    }
}

/// The itag a googlevideo URL was issued for: `?itag=251` on a stream,
/// `/itag/96/` on a manifest. `None` for anything else, a local file included.
pub fn itag_of(url: &str) -> Option<String> {
    let digits = |s: &str| {
        let d: String = s.chars().take_while(char::is_ascii_digit).collect();
        (!d.is_empty()).then_some(d)
    };
    if let Some(i) = url.find("/itag/") {
        return digits(&url[i + 6..]);
    }
    url.split(['?', '&']).find_map(|kv| kv.strip_prefix("itag=")).and_then(digits)
}

/// A stream as the player bar names it: `1080p60 avc1`, `Opus 160k`, or
/// `itag 18` when the video's format list is unknown or the stream is not a
/// row in it (a muxed fallback).
pub fn stream_label(info: Option<&StreamInfo>, itag: &str) -> String {
    if let Some(i) = info {
        if let Some(v) = i.video.iter().find(|f| f.id == itag) {
            let fps = if v.fps > 30 { v.fps.to_string() } else { String::new() };
            return format!("{}p{fps} {}", v.height, v.vcodec);
        }
        if let Some(a) = i.audio.iter().find(|f| f.id == itag) {
            let name = match a.acodec.as_str() {
                "opus" => "Opus".to_string(),
                "mp4a" => "AAC".to_string(),
                other => other.to_uppercase(),
            };
            return format!("{name} {}k", a.abr_kbps);
        }
    }
    match known_itag(itag) {
        Some(name) => name.to_string(),
        None => format!("itag {itag}"),
    }
}

/// YouTube's long-standing itags, for a stream whose video has no saved
/// format list (played before it was ever looked at, or offline since).
fn known_itag(itag: &str) -> Option<&'static str> {
    Some(match itag {
        "18" => "360p avc1",
        "22" => "720p avc1",
        "139" => "AAC 48k",
        "140" => "AAC 129k",
        "141" => "AAC 255k",
        "249" => "Opus 50k",
        "250" => "Opus 70k",
        "251" => "Opus 160k",
        "160" => "144p avc1",
        "133" => "240p avc1",
        "134" => "360p avc1",
        "135" => "480p avc1",
        "136" => "720p avc1",
        "137" => "1080p avc1",
        "298" => "720p60 avc1",
        "299" => "1080p60 avc1",
        "278" => "144p vp9",
        "242" => "240p vp9",
        "243" => "360p vp9",
        "244" => "480p vp9",
        "247" => "720p vp9",
        "248" => "1080p vp9",
        "271" => "1440p vp9",
        "313" => "2160p vp9",
        "302" => "720p60 vp9",
        "303" => "1080p60 vp9",
        "308" => "1440p60 vp9",
        "315" => "2160p60 vp9",
        "394" => "144p av01",
        "395" => "240p av01",
        "396" => "360p av01",
        "397" => "480p av01",
        "398" => "720p av01",
        "399" => "1080p av01",
        "400" => "1440p av01",
        "401" => "2160p av01",
        _ => return None,
    })
}

/// When a flat listing entry went up, unix seconds: `timestamp`, else the
/// `upload_date` day at midnight UTC. Only there when the listing was asked for
/// with `youtubetab:approximate_date`.
pub fn entry_published(e: &Value) -> Option<i64> {
    if let Some(t) = e["timestamp"].as_i64().filter(|t| *t > 0) {
        return Some(t);
    }
    let d = e["upload_date"].as_str().filter(|d| d.len() == 8)?;
    let (y, m, day): (i64, i64, i64) = (d[..4].parse().ok()?, d[4..6].parse().ok()?, d[6..].parse().ok()?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400)
}

/// The stream URLs in a `yt-dlp -f … -J` answer: `(picture, sound)` for a
/// split stream, `(the only stream, None)` for a muxed or audio-only one.
pub fn resolved_urls(j: &Value) -> (Option<String>, Option<String>) {
    let url = |f: &Value| f["url"].as_str().filter(|u| !u.is_empty()).map(String::from);
    match j["requested_formats"].as_array() {
        Some(parts) if !parts.is_empty() => (parts.first().and_then(url), parts.get(1).and_then(url)),
        _ => (url(j), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dQw4w9WgXcQ` as served on 2026-09-16, trimmed to the keys read here.
    /// The two chapters are synthetic; the real video has none.
    fn fixture() -> StreamInfo {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/yt_info.json")).unwrap();
        parse_info(&v)
    }

    #[test]
    fn video_rows_are_https_video_only_one_per_height_fps_codec() {
        let i = fixture();
        // 144..1080 have avc1 + vp09 + av01 (6 × 3), 1440 and 2160 have vp09 + av01.
        assert_eq!(i.video.len(), 22);
        assert!(i.video.iter().all(|f| !f.id.starts_with("sb")), "storyboards dropped");
        assert!(i.video.iter().all(|f| f.id != "18"), "muxed 360p dropped");
        assert!(i.video.iter().all(|f| f.id != "270" && f.id != "614"), "HLS dropped when https exists");
        assert_eq!(i.video[0].height, 2160);
        let r1080 = i.video.iter().find(|f| f.id == "137").unwrap();
        assert_eq!((r1080.height, r1080.fps, r1080.vcodec.as_str(), r1080.ext.as_str()), (1080, 25, "avc1", "mp4"));
        assert_eq!(r1080.tbr_kbps, 3038);
        assert_eq!(r1080.bytes, Some(80_911_999));
        assert!(!r1080.hdr);
        // `vp9` and `vp09.00…` are one family.
        assert_eq!(i.video.iter().find(|f| f.id == "248").unwrap().vcodec, "vp09");
    }

    #[test]
    fn audio_rows_are_https_audio_only_best_first() {
        let i = fixture();
        let ids: Vec<&str> = i.audio.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["140", "251", "250", "139", "249"]);
        let opus = &i.audio[1];
        assert_eq!((opus.acodec.as_str(), opus.ext.as_str(), opus.abr_kbps, opus.rate_hz), ("opus", "webm", 129, 48_000));
    }

    #[test]
    fn duration_channel_and_chapters() {
        let i = fixture();
        assert_eq!(i.duration_s, 213.0);
        assert_eq!(i.channel_id, "UCuAXFkgsw1L7xaCfnd5JJOw");
        assert_eq!(i.chapters, vec![
            Chapter { start_s: 0.0, title: "Intro".into() },
            Chapter { start_s: 43.0, title: "Chorus".into() },
        ]);
    }

    #[test]
    fn a_live_stream_with_only_hls_keeps_its_hls_rows() {
        let v = serde_json::json!({"formats": [
            {"format_id": "301", "vcodec": "avc1.64002a", "acodec": "mp4a.40.2", "height": 1080, "protocol": "m3u8_native"},
            {"format_id": "300", "vcodec": "avc1.4d4020", "acodec": "none", "height": 720, "fps": 60, "tbr": 2900.0, "protocol": "m3u8_native"}
        ]});
        let i = parse_info(&v);
        assert_eq!(i.video.len(), 1);
        assert_eq!((i.video[0].id.as_str(), i.video[0].fps), ("300", 60));
    }

    #[test]
    fn nonsense_is_empty_not_a_panic() {
        assert_eq!(parse_info(&serde_json::json!({})), StreamInfo::default());
        assert_eq!(parse_info(&serde_json::json!([1, 2])), StreamInfo::default());
    }

    #[test]
    fn watch_pref_round_trips_and_rejects_junk() {
        let p = WatchPref::parse("1080/avc1");
        assert_eq!(p, WatchPref { height: 1080, codec: "avc1".into() });
        assert_eq!(p.to_pref_string(), "1080/avc1");
        assert_eq!(WatchPref::parse("garbage"), WatchPref::default());
        assert_eq!(WatchPref::parse("720/h265"), WatchPref { height: 720, codec: "any".into() });
    }

    #[test]
    fn a_picked_row_is_that_row_plus_best_audio() {
        let pref = WatchPref::default();
        assert_eq!(watch_selector("399", &pref), "399+bestaudio/best");
        // Anything that is not a format id is ignored rather than passed to yt-dlp.
        assert_eq!(watch_selector("399;rm", &pref), "bv[vcodec^=avc1]+ba/bv[vcodec^=vp]+ba/bv+ba/b");
    }

    #[test]
    fn the_default_is_split_streams_capped_by_the_pref() {
        // `any` asks for what decodes in hardware first, not YouTube's best.
        assert_eq!(watch_selector("", &WatchPref::default()), "bv[vcodec^=avc1]+ba/bv[vcodec^=vp]+ba/bv+ba/b");
        assert_eq!(
            watch_selector("", &WatchPref { height: 1080, codec: "any".into() }),
            "bv[height<=1080][vcodec^=avc1]+ba/bv[height<=1080][vcodec^=vp]+ba/bv[height<=1080]+ba/b[height<=1080]/b"
        );
        assert_eq!(
            watch_selector("", &WatchPref { height: 1080, codec: "avc1".into() }),
            "bv[height<=1080][vcodec^=avc1]+ba/bv[height<=1080]+ba/b[height<=1080]/b"
        );
        // yt-dlp spells VP9 both `vp9` and `vp09.…`; `^=vp` catches both.
        assert_eq!(
            watch_selector("", &WatchPref { height: 0, codec: "vp09".into() }),
            "bv[vcodec^=vp]+ba/bv+ba/b"
        );
    }

    #[test]
    fn audio_selector_honours_a_pick_and_falls_back() {
        assert_eq!(audio_selector(""), "bestaudio/best");
        assert_eq!(audio_selector("251"), "251/bestaudio/best");
        assert_eq!(audio_selector("25 1"), "bestaudio/best");
    }

    #[test]
    fn names_the_playing_stream() {
        assert_eq!(
            itag_of("https://rr1.googlevideo.com/videoplayback?expire=1&itag=251&source=youtube").as_deref(),
            Some("251")
        );
        assert_eq!(
            itag_of("https://manifest.googlevideo.com/api/manifest/hls_playlist/itag/96/source/youtube").as_deref(),
            Some("96")
        );
        assert_eq!(itag_of("/home/me/.cache/youtube_cache/abc.opus"), None);

        let info = fixture();
        let opus = info.audio.iter().find(|a| a.acodec == "opus").unwrap();
        assert_eq!(stream_label(Some(&info), &opus.id), format!("Opus {}k", opus.abr_kbps));
        let v = &info.video[0];
        assert!(stream_label(Some(&info), &v.id).starts_with(&format!("{}p", v.height)));
        assert_eq!(stream_label(Some(&info), "18"), "360p avc1", "dropped from the rows, still named");
        assert_eq!(stream_label(None, "251"), "Opus 160k");
        assert_eq!(stream_label(None, "9999"), "itag 9999");
    }

    #[test]
    fn a_json_answer_names_picture_then_sound() {
        let merged = serde_json::json!({
            "url": null,
            "requested_formats": [{"url": "https://v/itag=137"}, {"url": "https://a/itag=251"}],
        });
        assert_eq!(
            resolved_urls(&merged),
            (Some("https://v/itag=137".into()), Some("https://a/itag=251".into()))
        );
        let single = serde_json::json!({"url": "https://a/itag=251", "format_id": "251"});
        assert_eq!(resolved_urls(&single), (Some("https://a/itag=251".into()), None));
        assert_eq!(resolved_urls(&serde_json::json!({})), (None, None));
    }

    #[test]
    fn above_1080p_asks_for_the_taller_picture_first() {
        let sel = watch_selector("", &WatchPref { height: 1440, codec: "any".into() });
        assert_eq!(
            sel,
            "bv[height<=1440][height>1080][vcodec^=vp]+ba/bv[height<=1440][height>1080]+ba/\
             bv[height<=1080][vcodec^=avc1]+ba/bv[height<=1080][vcodec^=vp]+ba/bv[height<=1080]+ba/b[height<=1080]/b"
        );
    }

    #[test]
    fn captions_are_the_uploaders_and_the_spoken_language() {
        let j = serde_json::json!({
            "language": "en",
            "subtitles": {
                "de": [{"ext": "json3", "url": "x"}, {"ext": "vtt", "url": "https://de", "name": "German"}],
                "live_chat": [{"ext": "vtt", "url": "https://chat"}],
            },
            "automatic_captions": {
                "en-orig": [{"ext": "vtt", "url": "https://en", "name": "English (Original)"}],
                "fr": [{"ext": "vtt", "url": "https://fr"}],
            },
        });
        let c = captions(&j);
        assert_eq!(c.len(), 2, "no chat, no translations");
        assert_eq!(pick_caption(&c, "en").map(|x| x.url.as_str()), Some("https://en"));
        assert_eq!(pick_caption(&c, "de").map(|x| x.url.as_str()), Some("https://de"));
        // Nothing in Japanese: the one uploaded track.
        assert_eq!(pick_caption(&c, "ja").map(|x| x.url.as_str()), Some("https://de"));
        assert!(pick_caption(&[], "en").is_none());
    }

    #[test]
    fn listing_entries_are_dated() {
        assert_eq!(entry_published(&serde_json::json!({"timestamp": 1_700_000_000})), Some(1_700_000_000));
        assert_eq!(entry_published(&serde_json::json!({"upload_date": "19700102"})), Some(86_400));
        assert_eq!(entry_published(&serde_json::json!({"upload_date": "20240301"})), Some(1_709_251_200));
        assert_eq!(entry_published(&serde_json::json!({"title": "x"})), None);
    }
}
