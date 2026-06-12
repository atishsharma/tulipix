//! DLNA / UPnP MediaServer — exposes Photos/Videos/Music to LAN renderers
//! (TVs, PlayStation, Sonos). The protocol layer (SSDP discovery + SOAP
//! ContentDirectory) is large and lives in an external crate that we'll
//! gate behind a cargo feature when packaging. This module focuses on the
//! parts the rest of the app cares about:
//!
//!  - building the DIDL-Lite XML for an item / container,
//!  - the ContentDirectory tree the renderer browses,
//!  - the URL the renderer fetches bytes from (we proxy through the local
//!    HTTP server so transcode-on-the-fly works).
//!
//! Local LAN feature — no account, no cap gate beyond local full access;
//! a dedicated `dlna.server` cap will be added in the np.p4 caps sweep.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DlnaItem {
    pub id: String,           // ContentDirectory ObjectID
    pub parent_id: String,
    pub title: String,
    pub mime: String,
    pub size_bytes: i64,
    pub duration_s: Option<f64>,
    pub stream_url: String,
    pub upnp_class: UpnpClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum UpnpClass {
    VideoItem,
    AudioItem,
    ImageItem,
}

impl UpnpClass {
    pub fn as_str(self) -> &'static str {
        match self {
            UpnpClass::VideoItem => "object.item.videoItem",
            UpnpClass::AudioItem => "object.item.audioItem.musicTrack",
            UpnpClass::ImageItem => "object.item.imageItem.photo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DlnaContainer {
    pub id: String,
    pub parent_id: String,
    pub title: String,
    pub child_count: i64,
}

#[derive(Debug, Default, Clone)]
pub struct ServerConfig {
    pub friendly_name: String,
    pub uuid: String,
    pub bind: String, // ip:port
}

impl ServerConfig {
    pub fn http_base(&self) -> String {
        format!("http://{}", self.bind)
    }

    pub fn description_xml(&self) -> String {
        // SCPDURL etc would be filled in by the protocol crate; this is the
        // minimum the renderer needs to identify us in SSDP responses.
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>{name}</friendlyName>
    <manufacturer>Tulipix</manufacturer>
    <modelName>Tulipix MediaServer</modelName>
    <UDN>uuid:{uuid}</UDN>
  </device>
</root>"#,
            name = xml_escape(&self.friendly_name),
            uuid = xml_escape(&self.uuid),
        )
    }
}

/// Build a DIDL-Lite `<item>` element. The renderer parses this and asks for
/// the `<res>` URL — we serve bytes (or transcoded HLS) from that URL.
pub fn didl_item(item: &DlnaItem) -> String {
    format!(
        r#"<item id="{id}" parentID="{pid}" restricted="1">
  <dc:title>{title}</dc:title>
  <upnp:class>{cls}</upnp:class>
  <res protocolInfo="http-get:*:{mime}:*"{dur}{size}>{url}</res>
</item>"#,
        id = xml_escape(&item.id),
        pid = xml_escape(&item.parent_id),
        title = xml_escape(&item.title),
        cls = item.upnp_class.as_str(),
        mime = xml_escape(&item.mime),
        dur = item
            .duration_s
            .map(|s| format!(r#" duration="{}""#, format_duration(s)))
            .unwrap_or_default(),
        size = if item.size_bytes > 0 { format!(r#" size="{}""#, item.size_bytes) } else { Default::default() },
        url = xml_escape(&item.stream_url),
    )
}

pub fn didl_container(c: &DlnaContainer) -> String {
    format!(
        r#"<container id="{id}" parentID="{pid}" childCount="{n}" restricted="1">
  <dc:title>{title}</dc:title>
  <upnp:class>object.container</upnp:class>
</container>"#,
        id = xml_escape(&c.id),
        pid = xml_escape(&c.parent_id),
        n = c.child_count,
        title = xml_escape(&c.title),
    )
}

/// Wrap a sequence of DIDL elements in the required envelope.
pub fn wrap_didl(elements: &[String]) -> String {
    let mut s = String::from(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite"
 xmlns:dc="http://purl.org/dc/elements/1.1/"
 xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">"#,
    );
    for e in elements {
        s.push_str(e);
    }
    s.push_str("</DIDL-Lite>");
    s
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn format_duration(s: f64) -> String {
    let total = s.max(0.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let secs = total % 60;
    format!("{h}:{m:02}:{secs:02}.000")
}

/// Map MIME → renderer-friendly mime + the upnp class. Some renderers want
/// `video/x-mkv` rather than `video/x-matroska`, etc. This is the right place
/// to bolt on those quirks.
pub fn normalise_mime(mime: &str) -> (String, UpnpClass) {
    let lower = mime.to_ascii_lowercase();
    let cls = if lower.starts_with("video/") {
        UpnpClass::VideoItem
    } else if lower.starts_with("audio/") {
        UpnpClass::AudioItem
    } else {
        UpnpClass::ImageItem
    };
    let normalised = match lower.as_str() {
        "video/x-matroska" => "video/x-mkv".to_string(),
        "video/quicktime" => "video/mp4".to_string(),
        _ => lower,
    };
    (normalised, cls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_xml_includes_device_metadata() {
        let cfg = ServerConfig {
            friendly_name: "Tulipix on Mac".into(),
            uuid: "abc-123".into(),
            bind: "0.0.0.0:8200".into(),
        };
        let xml = cfg.description_xml();
        assert!(xml.contains("Tulipix on Mac"));
        assert!(xml.contains("uuid:abc-123"));
        assert!(xml.contains("MediaServer:1"));
    }

    #[test]
    fn didl_escapes_xml_special_chars() {
        let item = DlnaItem {
            id: "id1".into(),
            parent_id: "0".into(),
            title: "Hello & <World>".into(),
            mime: "video/mp4".into(),
            size_bytes: 1024,
            duration_s: Some(3661.0),
            stream_url: "http://x?q=1&r=2".into(),
            upnp_class: UpnpClass::VideoItem,
        };
        let xml = didl_item(&item);
        assert!(xml.contains("Hello &amp; &lt;World&gt;"));
        assert!(xml.contains("&amp;r=2"));
        assert!(xml.contains(r#"duration="1:01:01.000""#));
        assert!(xml.contains(r#"size="1024""#));
    }

    #[test]
    fn didl_container_emits_child_count() {
        let c = DlnaContainer {
            id: "100".into(),
            parent_id: "0".into(),
            title: "Movies".into(),
            child_count: 42,
        };
        let xml = didl_container(&c);
        assert!(xml.contains(r#"childCount="42""#));
        assert!(xml.contains("object.container"));
    }

    #[test]
    fn wrap_didl_includes_namespaces() {
        let s = wrap_didl(&["<item/>".into()]);
        assert!(s.contains("xmlns:upnp="));
        assert!(s.contains("xmlns:dc="));
        assert!(s.contains("<item/>"));
        assert!(s.ends_with("</DIDL-Lite>"));
    }

    #[test]
    fn normalise_mime_picks_class() {
        assert_eq!(normalise_mime("video/mp4").1, UpnpClass::VideoItem);
        assert_eq!(normalise_mime("audio/flac").1, UpnpClass::AudioItem);
        assert_eq!(normalise_mime("image/jpeg").1, UpnpClass::ImageItem);
    }

    #[test]
    fn normalise_mime_renderer_quirks() {
        assert_eq!(normalise_mime("video/x-matroska").0, "video/x-mkv");
        assert_eq!(normalise_mime("video/QuickTime").0, "video/mp4");
    }

    #[test]
    fn format_duration_handles_zero_and_hours() {
        assert_eq!(format_duration(0.0), "0:00:00.000");
        assert_eq!(format_duration(3725.0), "1:02:05.000");
    }
}
