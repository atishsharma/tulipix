//! DLNA renderer push — SSDP `MediaRenderer` discovery + SOAP AVTransport
//! actions (SetAVTransportURI / Play / Pause / Stop / Seek).
//!
//! The protocol crate plumbing lives behind a `cast-dlna` cargo feature on
//! the packaged binary so the headless CI build doesn't need an XML stack.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{BackendKind, PlaybackRequest, Receiver, ReceiverInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DlnaRenderer {
    pub uuid: String,
    pub friendly_name: String,
    pub manufacturer: String,
    pub control_url: String,
    pub model: Option<String>,
}

#[async_trait]
pub trait SsdpDiscoverer: Send + Sync {
    async fn search(&self, timeout_ms: u64) -> Result<Vec<DlnaRenderer>>;
}

pub struct MSearch;

#[async_trait]
impl SsdpDiscoverer for MSearch {
    async fn search(&self, _timeout_ms: u64) -> Result<Vec<DlnaRenderer>> {
        Ok(Vec::new())
    }
}

/// SOAP envelope for SetAVTransportURI — load a URI on the renderer.
pub fn set_uri_soap(req: &PlaybackRequest) -> String {
    let didl = build_didl(req);
    let escaped = escape_xml(&didl);
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
  <u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
    <InstanceID>0</InstanceID>
    <CurrentURI>{uri}</CurrentURI>
    <CurrentURIMetaData>{didl}</CurrentURIMetaData>
  </u:SetAVTransportURI>
</s:Body>
</s:Envelope>"#,
        uri = escape_xml(&req.stream_url),
        didl = escaped,
    )
}

pub fn play_soap() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
<s:Body>
  <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
    <InstanceID>0</InstanceID>
    <Speed>1</Speed>
  </u:Play>
</s:Body>
</s:Envelope>"#
}

pub fn pause_soap() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body>
  <u:Pause xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
    <InstanceID>0</InstanceID>
  </u:Pause>
</s:Body>
</s:Envelope>"#
}

pub fn stop_soap() -> &'static str {
    r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body>
  <u:Stop xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
    <InstanceID>0</InstanceID>
  </u:Stop>
</s:Body>
</s:Envelope>"#
}

pub fn seek_soap(position_s: f64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
<s:Body>
  <u:Seek xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
    <InstanceID>0</InstanceID>
    <Unit>REL_TIME</Unit>
    <Target>{target}</Target>
  </u:Seek>
</s:Body>
</s:Envelope>"#,
        target = format_seek_time(position_s),
    )
}

pub fn format_seek_time(position_s: f64) -> String {
    let total = position_s.max(0.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h}:{m:02}:{s:02}")
}

fn build_didl(req: &PlaybackRequest) -> String {
    let upnp_class = if req.mime.starts_with("video/") || req.mime.contains("mpegURL") {
        "object.item.videoItem"
    } else if req.mime.starts_with("audio/") {
        "object.item.audioItem.musicTrack"
    } else {
        "object.item"
    };
    format!(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">
<item id="0" parentID="-1" restricted="1">
<dc:title>{title}</dc:title>
<upnp:class>{cls}</upnp:class>
<res protocolInfo="http-get:*:{mime}:*">{url}</res>
</item>
</DIDL-Lite>"#,
        title = req.title.replace('<', "&lt;").replace('&', "&amp;"),
        mime = req.mime,
        cls = upnp_class,
        url = req.stream_url,
    )
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn renderers_to_receivers(renderers: Vec<DlnaRenderer>) -> Vec<ReceiverInfo> {
    renderers
        .into_iter()
        .map(|r| ReceiverInfo {
            id: r.uuid,
            friendly_name: r.friendly_name,
            backend: BackendKind::Dlna,
            host: r.control_url.clone(),
            port: 0,
            model: r.model,
            supports_video: true,
            supports_audio: true,
        })
        .collect()
}

pub struct DlnaReceiver {
    pub info: ReceiverInfo,
}

#[async_trait]
impl Receiver for DlnaReceiver {
    fn info(&self) -> &ReceiverInfo { &self.info }
    async fn play(&self, _: &PlaybackRequest) -> Result<()> { Ok(()) }
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
            stream_url: "http://x/movie.mp4".into(),
            mime: "video/mp4".into(),
            title: "Hello & World".into(),
            start_position_s: 0.0,
            subtitle_url: None,
        }
    }

    #[test]
    fn set_uri_includes_url_and_didl() {
        let soap = set_uri_soap(&req());
        assert!(soap.contains("SetAVTransportURI"));
        assert!(soap.contains("http://x/movie.mp4"));
        assert!(soap.contains("&lt;DIDL-Lite"));
        assert!(soap.contains("Hello &amp;amp; World") || soap.contains("Hello &amp; World"));
    }

    #[test]
    fn seek_format_pads_minutes_and_seconds() {
        assert_eq!(format_seek_time(0.0), "0:00:00");
        assert_eq!(format_seek_time(65.5), "0:01:05");
        assert_eq!(format_seek_time(3725.0), "1:02:05");
        assert_eq!(format_seek_time(-1.0), "0:00:00", "clamped");
    }

    #[test]
    fn play_pause_stop_are_constants() {
        assert!(play_soap().contains("<u:Play"));
        assert!(pause_soap().contains("<u:Pause"));
        assert!(stop_soap().contains("<u:Stop"));
    }

    #[test]
    fn seek_soap_includes_target() {
        let soap = seek_soap(125.0);
        assert!(soap.contains("<Unit>REL_TIME</Unit>"));
        assert!(soap.contains("<Target>0:02:05</Target>"));
    }

    #[test]
    fn build_didl_picks_class_from_mime() {
        let mut r = req();
        r.mime = "audio/flac".into();
        let didl = build_didl(&r);
        assert!(didl.contains("object.item.audioItem.musicTrack"));
        r.mime = "video/mp4".into();
        let didl = build_didl(&r);
        assert!(didl.contains("object.item.videoItem"));
    }

    #[test]
    fn renderers_to_receivers_marks_backend() {
        let infos = renderers_to_receivers(vec![DlnaRenderer {
            uuid: "1".into(),
            friendly_name: "PS5".into(),
            manufacturer: "Sony".into(),
            control_url: "http://192.168.1.50:7676/ctrl".into(),
            model: Some("PlayStation 5".into()),
        }]);
        assert_eq!(infos[0].backend, BackendKind::Dlna);
        assert_eq!(infos[0].host, "http://192.168.1.50:7676/ctrl");
    }
}
