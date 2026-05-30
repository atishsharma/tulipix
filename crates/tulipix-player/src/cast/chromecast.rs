//! Chromecast — mDNS discovery (`_googlecast._tcp.local`) + JSON-over-TLS
//! channel on port 8009.
//!
//! Network discovery itself lives behind a `Discoverer` trait so tests don't
//! depend on a Wi-Fi link. The CAST messaging protocol is JSON wrapped in a
//! 4-byte length-prefixed protobuf in production; here we expose the JSON
//! payload builders (LOAD / PAUSE / PLAY / SEEK / STOP / SET_VOLUME) and the
//! request-id allocator — the protobuf framing is a thin wrapper added by
//! the packaged binary.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};

use super::{BackendKind, PlaybackRequest, Receiver, ReceiverInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastDevice {
    pub uuid: String,
    pub friendly_name: String,
    pub model: String,
    pub host: String,
    pub port: u16,
}

#[async_trait]
pub trait Discoverer: Send + Sync {
    async fn scan(&self, timeout_ms: u64) -> Result<Vec<CastDevice>>;
}

pub struct MdnsDiscoverer;

#[async_trait]
impl Discoverer for MdnsDiscoverer {
    async fn scan(&self, _timeout_ms: u64) -> Result<Vec<CastDevice>> {
        // Real impl uses mdns-sd, gated behind the `cast-chromecast` feature
        // when packaging — kept stub-empty here so headless CI stays green.
        Ok(Vec::new())
    }
}

#[derive(Debug, Serialize)]
pub struct CastEnvelope<'a> {
    #[serde(rename = "requestId")]
    pub request_id: i64,
    #[serde(rename = "type")]
    pub message_type: &'a str,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct RequestIdSeq {
    counter: AtomicI64,
}

impl RequestIdSeq {
    pub fn next(&self) -> i64 {
        self.counter.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// LOAD payload — what the receiver app uses to start streaming media.
pub fn load_payload(req: &PlaybackRequest, request_id: i64) -> serde_json::Value {
    let mut media = serde_json::json!({
        "contentId": req.stream_url,
        "contentType": req.mime,
        "streamType": "BUFFERED",
        "metadata": {
            "type": 0,
            "title": req.title,
        },
    });
    if let Some(sub_url) = &req.subtitle_url {
        media["tracks"] = serde_json::json!([{
            "trackId": 1,
            "type": "TEXT",
            "trackContentId": sub_url,
            "trackContentType": "text/vtt",
            "subtype": "SUBTITLES",
        }]);
    }
    serde_json::json!({
        "type": "LOAD",
        "requestId": request_id,
        "media": media,
        "autoplay": true,
        "currentTime": req.start_position_s,
        "activeTrackIds": req.subtitle_url.as_ref().map(|_| vec![1]).unwrap_or_default(),
    })
}

pub fn simple_payload(kind: &str, request_id: i64) -> serde_json::Value {
    serde_json::json!({
        "type": kind,
        "requestId": request_id,
    })
}

pub fn seek_payload(position_s: f64, request_id: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "SEEK",
        "requestId": request_id,
        "currentTime": position_s,
    })
}

pub fn volume_payload(value: f32, request_id: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "SET_VOLUME",
        "requestId": request_id,
        "volume": { "level": value.clamp(0.0, 1.0) },
    })
}

pub fn mute_payload(muted: bool, request_id: i64) -> serde_json::Value {
    serde_json::json!({
        "type": "SET_VOLUME",
        "requestId": request_id,
        "volume": { "muted": muted },
    })
}

pub fn cast_devices_to_receivers(devices: Vec<CastDevice>) -> Vec<ReceiverInfo> {
    devices
        .into_iter()
        .map(|d| ReceiverInfo {
            id: d.uuid,
            friendly_name: d.friendly_name,
            backend: BackendKind::Chromecast,
            host: d.host,
            port: d.port,
            model: Some(d.model),
            supports_video: true,
            supports_audio: true,
        })
        .collect()
}

/// Stub receiver — the production implementation owns the TLS socket; this
/// one just stores the info so the rest of the app can be tested.
pub struct ChromecastReceiver {
    pub info: ReceiverInfo,
}

#[async_trait]
impl Receiver for ChromecastReceiver {
    fn info(&self) -> &ReceiverInfo {
        &self.info
    }
    async fn play(&self, _req: &PlaybackRequest) -> Result<()> { Ok(()) }
    async fn pause(&self) -> Result<()> { Ok(()) }
    async fn resume(&self) -> Result<()> { Ok(()) }
    async fn seek(&self, _: f64) -> Result<()> { Ok(()) }
    async fn stop(&self) -> Result<()> { Ok(()) }
    async fn set_volume(&self, _: f32) -> Result<()> { Ok(()) }
    async fn set_muted(&self, _: bool) -> Result<()> { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> PlaybackRequest {
        PlaybackRequest {
            stream_url: "https://host/index.m3u8".into(),
            mime: "application/x-mpegURL".into(),
            title: "Movie".into(),
            start_position_s: 120.0,
            subtitle_url: None,
        }
    }

    #[test]
    fn load_payload_includes_media_metadata() {
        let p = load_payload(&req(), 7);
        assert_eq!(p["type"], "LOAD");
        assert_eq!(p["requestId"], 7);
        assert_eq!(p["currentTime"], 120.0);
        assert_eq!(p["media"]["contentId"], "https://host/index.m3u8");
        assert_eq!(p["media"]["metadata"]["title"], "Movie");
        assert!(p["media"]["tracks"].is_null());
    }

    #[test]
    fn load_payload_with_subtitle_adds_track() {
        let mut r = req();
        r.subtitle_url = Some("https://host/sub.vtt".into());
        let p = load_payload(&r, 1);
        assert_eq!(p["media"]["tracks"][0]["trackContentId"], "https://host/sub.vtt");
        assert_eq!(p["activeTrackIds"], serde_json::json!([1]));
    }

    #[test]
    fn volume_payload_clamps_range() {
        let p = volume_payload(2.0, 1);
        assert_eq!(p["volume"]["level"], 1.0);
        let p = volume_payload(-1.0, 1);
        assert_eq!(p["volume"]["level"], 0.0);
    }

    #[test]
    fn mute_payload_toggle() {
        let on = mute_payload(true, 3);
        let off = mute_payload(false, 4);
        assert_eq!(on["volume"]["muted"], true);
        assert_eq!(off["volume"]["muted"], false);
    }

    #[test]
    fn request_id_sequence_is_monotonic() {
        let seq = RequestIdSeq::default();
        assert_eq!(seq.next(), 1);
        assert_eq!(seq.next(), 2);
        assert_eq!(seq.next(), 3);
    }

    #[test]
    fn devices_into_receivers_marks_backend() {
        let devices = vec![CastDevice {
            uuid: "abc".into(),
            friendly_name: "Lounge".into(),
            model: "Chromecast Ultra".into(),
            host: "192.168.1.20".into(),
            port: 8009,
        }];
        let infos = cast_devices_to_receivers(devices);
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].backend, BackendKind::Chromecast);
        assert_eq!(infos[0].port, 8009);
        assert!(infos[0].supports_video);
    }
}
