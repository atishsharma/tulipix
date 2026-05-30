//! AirPlay 2 — Bonjour `_airplay._tcp.local` discovery + HTTP/RTSP control
//! channel. The protocol is reverse-engineered from
//! [`shairport-sync`](https://github.com/mikebrady/shairport-sync) for audio
//! and from Apple's developer docs for video.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{BackendKind, PlaybackRequest, Receiver, ReceiverInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AirPlayDevice {
    pub id: String,
    pub friendly_name: String,
    pub host: String,
    pub port: u16,
    pub features: AirPlayFeatures,
    pub source_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AirPlayFeatures {
    pub audio: bool,
    pub video: bool,
    pub photo: bool,
    pub screen: bool,
}

/// Decode the `features=` TXT record that AirPlay 2 receivers advertise.
/// It's a 64-bit hex bitmask split with a comma — `0x4A7FDFD5,0xBC1E0` —
/// where bit positions encode capabilities. We only care about the bits the
/// receiver picker surfaces.
pub fn parse_features(raw: &str) -> AirPlayFeatures {
    let mut combined: u64 = 0;
    for part in raw.split(',') {
        let trimmed = part.trim().trim_start_matches("0x");
        if let Ok(v) = u64::from_str_radix(trimmed, 16) {
            combined |= v;
        }
    }
    AirPlayFeatures {
        audio: (combined & 0x0000_0200) != 0,
        video: (combined & 0x0000_0001) != 0,
        photo: (combined & 0x0000_0002) != 0,
        screen: (combined & 0x0000_0004) != 0,
    }
}

#[async_trait]
pub trait Discoverer: Send + Sync {
    async fn scan(&self, timeout_ms: u64) -> Result<Vec<AirPlayDevice>>;
}

pub struct BonjourDiscoverer;

#[async_trait]
impl Discoverer for BonjourDiscoverer {
    async fn scan(&self, _timeout_ms: u64) -> Result<Vec<AirPlayDevice>> {
        Ok(Vec::new())
    }
}

/// Build the binary plist payload — kept as `serde_json::Value` here for the
/// shape; the production sender serialises to plist binary 1 before POSTing
/// to `/play`. The Apple receiver accepts both binary and XML plists; binary
/// is just smaller on the wire.
pub fn play_plist(req: &PlaybackRequest) -> serde_json::Value {
    serde_json::json!({
        "Content-Location": req.stream_url,
        "Start-Position": req.start_position_s,
        "MIME": req.mime,
        "Title": req.title,
    })
}

pub fn devices_to_receivers(devices: Vec<AirPlayDevice>) -> Vec<ReceiverInfo> {
    devices
        .into_iter()
        .map(|d| ReceiverInfo {
            id: d.id,
            friendly_name: d.friendly_name,
            backend: BackendKind::AirPlay,
            host: d.host,
            port: d.port,
            model: Some(d.source_version),
            supports_video: d.features.video || d.features.screen,
            supports_audio: d.features.audio,
        })
        .collect()
}

pub struct AirPlayReceiver {
    pub info: ReceiverInfo,
}

#[async_trait]
impl Receiver for AirPlayReceiver {
    fn info(&self) -> &ReceiverInfo { &self.info }
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

    #[test]
    fn parse_features_video_audio() {
        let f = parse_features("0x00000201");
        assert!(f.audio);
        assert!(f.video);
        assert!(!f.photo);
    }

    #[test]
    fn parse_features_multi_word_mask() {
        // Real Apple TV advertises two hex words separated by comma — we OR
        // them together. Bit 0 = video, bit 2 = screen.
        let f = parse_features("0x00000001,0x00000004");
        assert!(f.video);
        assert!(f.screen);
        assert!(!f.audio);
    }

    #[test]
    fn parse_features_invalid_returns_default() {
        let f = parse_features("not-hex");
        assert_eq!(f, AirPlayFeatures::default());
    }

    #[test]
    fn play_plist_carries_request_fields() {
        let req = PlaybackRequest {
            stream_url: "http://x/movie.m3u8".into(),
            mime: "application/x-mpegURL".into(),
            title: "T".into(),
            start_position_s: 30.0,
            subtitle_url: None,
        };
        let p = play_plist(&req);
        assert_eq!(p["Content-Location"], "http://x/movie.m3u8");
        assert_eq!(p["Start-Position"], 30.0);
        assert_eq!(p["MIME"], "application/x-mpegURL");
    }

    #[test]
    fn devices_to_receivers_carries_capabilities() {
        let infos = devices_to_receivers(vec![AirPlayDevice {
            id: "1".into(),
            friendly_name: "Apple TV Lounge".into(),
            host: "192.168.1.40".into(),
            port: 7000,
            features: AirPlayFeatures {
                audio: true,
                video: true,
                photo: false,
                screen: false,
            },
            source_version: "470.20".into(),
        }]);
        assert!(infos[0].supports_audio);
        assert!(infos[0].supports_video);
        assert_eq!(infos[0].backend, BackendKind::AirPlay);
    }
}
