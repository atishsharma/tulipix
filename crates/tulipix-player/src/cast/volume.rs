//! Remote volume + mute. Each backend has its own SET_VOLUME shape but the
//! UI surface is one slider + one mute toggle, so we route through a single
//! [`VolumeController`] that translates to/from the receiver protocol.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use super::BackendKind;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumeState {
    pub level: f32, // 0.0..=1.0
    pub muted: bool,
}

impl Default for VolumeState {
    fn default() -> Self {
        Self { level: 0.5, muted: false }
    }
}

impl VolumeState {
    pub fn clamped(self) -> Self {
        Self {
            level: self.level.clamp(0.0, 1.0),
            muted: self.muted,
        }
    }
}

/// Encode the state into the wire-level payload for a backend. Returns
/// `serde_json::Value` for Chromecast, the SOAP body for DLNA, and the
/// RTSP / `/volume` body for AirPlay.
pub enum WirePayload {
    Json(serde_json::Value),
    Soap(String),
    Form(String),
}

pub fn encode(backend: BackendKind, state: VolumeState, request_id: i64) -> WirePayload {
    let s = state.clamped();
    match backend {
        BackendKind::Chromecast => WirePayload::Json(serde_json::json!({
            "type": "SET_VOLUME",
            "requestId": request_id,
            "volume": { "level": s.level, "muted": s.muted },
        })),
        BackendKind::Dlna => WirePayload::Soap(format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body>
  <u:SetVolume xmlns:u="urn:schemas-upnp-org:service:RenderingControl:1">
    <InstanceID>0</InstanceID>
    <Channel>Master</Channel>
    <DesiredVolume>{vol}</DesiredVolume>
  </u:SetVolume>
  <u:SetMute xmlns:u="urn:schemas-upnp-org:service:RenderingControl:1">
    <InstanceID>0</InstanceID>
    <Channel>Master</Channel>
    <DesiredMute>{mute}</DesiredMute>
  </u:SetMute>
</s:Body>
</s:Envelope>"#,
            vol = (s.level * 100.0) as i32,
            mute = if s.muted { 1 } else { 0 },
        )),
        BackendKind::AirPlay => {
            // AirPlay 1: `volume\n<float>` POSTed to /volume; AirPlay 2 wraps
            // it in a plist, but the float is the same so we leave the plist
            // wrapping to the sender.
            let raw = if s.muted { -144.0 } else { airplay_db_for(s.level) };
            WirePayload::Form(format!("volume\n{raw}\n"))
        }
    }
}

/// Convert 0..1 → AirPlay's dB range. Apple's docs put the slider at
/// [-30, 0] dB with -144 reserved for "muted".
fn airplay_db_for(level: f32) -> f32 {
    let l = level.clamp(0.0, 1.0);
    if l <= 0.001 {
        return -144.0;
    }
    -30.0 + (30.0 * l)
}

/// Parse the SET_VOLUME response a Chromecast pushes after the receiver
/// applies the value — used for the UI slider's "is it actually at 0.5?"
/// reconciliation.
pub fn parse_chromecast_status(v: &serde_json::Value) -> Result<VolumeState> {
    let level = v
        .pointer("/status/volume/level")
        .and_then(|x| x.as_f64())
        .ok_or_else(|| anyhow!("missing level"))?;
    let muted = v
        .pointer("/status/volume/muted")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    Ok(VolumeState { level: level as f32, muted }.clamped())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_level_in_range() {
        assert_eq!(VolumeState { level: 1.5, muted: false }.clamped().level, 1.0);
        assert_eq!(VolumeState { level: -0.5, muted: false }.clamped().level, 0.0);
    }

    #[test]
    fn chromecast_payload_carries_level_and_mute() {
        let WirePayload::Json(p) = encode(BackendKind::Chromecast, VolumeState { level: 0.4, muted: true }, 5)
        else { panic!("expected json") };
        assert_eq!(p["requestId"], 5);
        let level = p["volume"]["level"].as_f64().unwrap();
        assert!((level - 0.4).abs() < 1e-5);
        assert_eq!(p["volume"]["muted"], true);
    }

    #[test]
    fn dlna_payload_carries_volume_and_mute() {
        let WirePayload::Soap(s) = encode(BackendKind::Dlna, VolumeState { level: 0.5, muted: false }, 0)
        else { panic!("expected soap") };
        assert!(s.contains("<DesiredVolume>50</DesiredVolume>"));
        assert!(s.contains("<DesiredMute>0</DesiredMute>"));
    }

    #[test]
    fn airplay_muted_uses_sentinel_db() {
        let WirePayload::Form(body) = encode(BackendKind::AirPlay, VolumeState { level: 0.5, muted: true }, 0)
        else { panic!("expected form") };
        assert!(body.contains("-144"));
    }

    #[test]
    fn airplay_db_clamps_at_min_and_max() {
        assert_eq!(airplay_db_for(0.0), -144.0);
        assert_eq!(airplay_db_for(1.0), 0.0);
        assert!((airplay_db_for(0.5) + 15.0).abs() < 1e-4);
    }

    #[test]
    fn parse_chromecast_status_extracts_level_and_mute() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"status":{"volume":{"level":0.3,"muted":true}}}"#,
        )
        .unwrap();
        let state = parse_chromecast_status(&v).unwrap();
        assert!((state.level - 0.3).abs() < 1e-6);
        assert!(state.muted);
    }

    #[test]
    fn parse_chromecast_status_missing_level_errors() {
        let v: serde_json::Value = serde_json::from_str(r#"{"status":{"volume":{}}}"#).unwrap();
        assert!(parse_chromecast_status(&v).is_err());
    }
}
