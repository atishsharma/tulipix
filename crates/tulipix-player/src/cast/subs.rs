//! Subtitle-track injection onto cast receivers — none of Chromecast,
//! AirPlay 2, or DLNA accept arbitrary on-disk SRT/VTT files; they want a
//! URL the receiver can pull. The host already runs an HTTPS server for
//! casting (see `tulipix-sync::remote_stream`), so we mint a per-cast
//! subtitle URL that wraps an on-the-fly converter:
//!
//!  - SRT → WebVTT for Chromecast (`text/vtt`)
//!  - SRT → WebVTT for AirPlay 2 over HLS subtitle group
//!  - SRT served as-is for DLNA (the renderer either grabs the
//!    `text/srt` sidecar or does without)

use serde::{Deserialize, Serialize};

use super::BackendKind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiverSub {
    pub url: String,
    pub mime: String,
    pub language: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    Srt,
    WebVtt,
    Ass,
}

impl InputFormat {
    pub fn from_extension(ext: &str) -> Option<Self> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "srt" => InputFormat::Srt,
            "vtt" => InputFormat::WebVtt,
            "ass" | "ssa" => InputFormat::Ass,
            _ => return None,
        })
    }
}

/// Pick the wire format for a backend + a source-track format. Returns
/// `None` when the backend can't render the track (e.g. ASS on Chromecast).
pub fn target_mime(backend: BackendKind, input: InputFormat) -> Option<&'static str> {
    match (backend, input) {
        (BackendKind::Chromecast, InputFormat::Srt | InputFormat::WebVtt) => Some("text/vtt"),
        (BackendKind::AirPlay, InputFormat::Srt | InputFormat::WebVtt) => Some("text/vtt"),
        (BackendKind::Dlna, InputFormat::Srt) => Some("text/srt"),
        (BackendKind::Dlna, InputFormat::WebVtt) => Some("text/vtt"),
        // ASS rasterisation has to happen client-side; we drop the track and
        // fall back to whatever's burned in.
        _ => None,
    }
}

/// Tiny SRT → VTT converter — enough to serve Chromecast / AirPlay without
/// pulling a parser crate. Comma → dot in timecodes, plain WEBVTT header,
/// blank lines between cues preserved.
pub fn srt_to_vtt(srt: &str) -> String {
    let mut out = String::with_capacity(srt.len() + 10);
    out.push_str("WEBVTT\n\n");
    for line in srt.lines() {
        if line.contains(" --> ") {
            out.push_str(&line.replace(',', "."));
        } else if line.trim().chars().all(|c| c.is_ascii_digit()) && !line.is_empty() {
            // Drop the per-cue numeric index — VTT doesn't need it.
            continue;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Build the URL the receiver should fetch. Wraps the local stream token
/// store so subtitles share the same revocation lifecycle as the video
/// stream.
pub fn sub_url(stream_base: &str, item_id: i64, track_id: i64, target_ext: &str) -> String {
    format!("{stream_base}/{item_id}/subs/{track_id}.{target_ext}")
}

pub fn build_receiver_sub(
    backend: BackendKind,
    stream_base: &str,
    item_id: i64,
    track_id: i64,
    language: &str,
    label: &str,
    input: InputFormat,
) -> Option<ReceiverSub> {
    let mime = target_mime(backend, input)?;
    let ext = match mime {
        "text/vtt" => "vtt",
        "text/srt" => "srt",
        _ => "txt",
    };
    Some(ReceiverSub {
        url: sub_url(stream_base, item_id, track_id, ext),
        mime: mime.to_string(),
        language: language.to_string(),
        label: label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_to_vtt_strips_indices_and_dots_commas() {
        let srt = "1\n00:00:01,000 --> 00:00:03,500\nHello world\n\n2\n00:00:04,000 --> 00:00:05,000\nLine two";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:03.500"));
        assert!(!vtt.contains(",000"));
        assert!(!vtt.lines().any(|l| l == "1" || l == "2"));
        assert!(vtt.contains("Hello world"));
    }

    #[test]
    fn target_mime_table() {
        assert_eq!(
            target_mime(BackendKind::Chromecast, InputFormat::Srt),
            Some("text/vtt")
        );
        assert_eq!(
            target_mime(BackendKind::AirPlay, InputFormat::WebVtt),
            Some("text/vtt")
        );
        assert_eq!(target_mime(BackendKind::Dlna, InputFormat::Srt), Some("text/srt"));
        assert!(target_mime(BackendKind::Chromecast, InputFormat::Ass).is_none());
    }

    #[test]
    fn input_format_from_extension_roundtrip() {
        assert_eq!(InputFormat::from_extension("SRT"), Some(InputFormat::Srt));
        assert_eq!(InputFormat::from_extension("vtt"), Some(InputFormat::WebVtt));
        assert_eq!(InputFormat::from_extension("ass"), Some(InputFormat::Ass));
        assert!(InputFormat::from_extension("idx").is_none());
    }

    #[test]
    fn sub_url_includes_id_and_track() {
        let url = sub_url("https://host/stream", 42, 1, "vtt");
        assert_eq!(url, "https://host/stream/42/subs/1.vtt");
    }

    #[test]
    fn build_receiver_sub_returns_none_for_ass_on_chromecast() {
        assert!(build_receiver_sub(
            BackendKind::Chromecast,
            "https://host/stream",
            1,
            1,
            "en",
            "English",
            InputFormat::Ass,
        )
        .is_none());
    }

    #[test]
    fn build_receiver_sub_emits_vtt_url_for_chromecast() {
        let sub = build_receiver_sub(
            BackendKind::Chromecast,
            "https://host/stream",
            42,
            7,
            "es",
            "Español",
            InputFormat::Srt,
        )
        .unwrap();
        assert_eq!(sub.mime, "text/vtt");
        assert_eq!(sub.url, "https://host/stream/42/subs/7.vtt");
        assert_eq!(sub.language, "es");
    }
}
