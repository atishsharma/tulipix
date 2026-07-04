//! `np.p5.music.cast` — audio casting to LAN renderers (DLNA/UPnP first;
//! Chromecast/AirPlay device kinds reserved).
//!
//! Pure builders + a small SSDP response parser. main.rs performs the actual
//! UDP discovery and SOAP HTTP POSTs using these strings.

#[derive(Debug, Clone, PartialEq)]
pub enum CastKind {
    Dlna,
    Chromecast,
    AirPlay,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CastDevice {
    pub name: String,
    /// Device-description URL (DLNA) or control endpoint.
    pub location: String,
    pub kind: CastKind,
}

/// SSDP M-SEARCH datagram for UPnP MediaRenderers. CRLF line endings, trailing
/// blank line as the protocol requires.
pub fn ssdp_msearch() -> String {
    "M-SEARCH * HTTP/1.1\r\n\
     HOST: 239.255.255.250:1900\r\n\
     MAN: \"ssdp:discover\"\r\n\
     MX: 2\r\n\
     ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
     \r\n"
        .to_string()
}

/// Parse an SSDP unicast response into a `CastDevice` (DLNA). Returns `None`
/// when there's no `LOCATION` header.
pub fn parse_ssdp_response(raw: &str) -> Option<CastDevice> {
    let mut location = None;
    let mut server = None;
    let mut usn = None;
    for line in raw.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let v = v.trim();
        match k.trim().to_ascii_uppercase().as_str() {
            "LOCATION" => location = Some(v.to_string()),
            "SERVER" => server = Some(v.to_string()),
            "USN" => usn = Some(v.to_string()),
            _ => {}
        }
    }
    let location = location?;
    let name = server
        .or(usn)
        .unwrap_or_else(|| "DLNA renderer".to_string());
    Some(CastDevice { name, location, kind: CastKind::Dlna })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn soap_envelope(action: &str, inner: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body>\
         <u:{action} xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">{inner}</u:{action}>\
         </s:Body></s:Envelope>"
    )
}

/// DLNA AVTransport `SetAVTransportURI` body.
pub fn soap_set_uri(instance_id: u32, media_url: &str) -> String {
    let inner = format!(
        "<InstanceID>{instance_id}</InstanceID>\
         <CurrentURI>{}</CurrentURI>\
         <CurrentURIMetaData></CurrentURIMetaData>",
        xml_escape(media_url)
    );
    soap_envelope("SetAVTransportURI", &inner)
}

/// DLNA AVTransport `Play` body (Speed 1).
pub fn soap_play(instance_id: u32) -> String {
    let inner = format!("<InstanceID>{instance_id}</InstanceID><Speed>1</Speed>");
    soap_envelope("Play", &inner)
}

/// DLNA AVTransport `Stop` body.
pub fn soap_stop(instance_id: u32) -> String {
    let inner = format!("<InstanceID>{instance_id}</InstanceID>");
    soap_envelope("Stop", &inner)
}

/// DLNA AVTransport `Pause` body.
pub fn soap_pause(instance_id: u32) -> String {
    let inner = format!("<InstanceID>{instance_id}</InstanceID>");
    soap_envelope("Pause", &inner)
}

/// Value for the HTTP `SOAPACTION` header (double-quoted).
pub fn soap_action_header(action: &str) -> String {
    format!("\"urn:schemas-upnp-org:service:AVTransport:1#{action}\"")
}

/// Extract the AVTransport `controlURL` from a device-description document
/// (the XML at the SSDP `LOCATION`). Tag-soup scan, no XML dep: find the
/// service block whose serviceType mentions AVTransport, then its controlURL.
pub fn parse_control_url(desc_xml: &str) -> Option<String> {
    let at = desc_xml.find("urn:schemas-upnp-org:service:AVTransport")?;
    let rest = &desc_xml[at..];
    let start = rest.find("<controlURL>")? + "<controlURL>".len();
    let end = rest[start..].find("</controlURL>")? + start;
    let url = rest[start..end].trim();
    if url.is_empty() { None } else { Some(url.to_string()) }
}

/// Join a (possibly relative) controlURL against the description URL's origin.
pub fn resolve_url(location: &str, control: &str) -> String {
    if control.starts_with("http://") || control.starts_with("https://") {
        return control.to_string();
    }
    // origin = scheme://host:port of the LOCATION
    let origin = location.find("://")
        .and_then(|i| location[i + 3..].find('/').map(|j| &location[..i + 3 + j]))
        .unwrap_or(location);
    if control.starts_with('/') {
        format!("{origin}{control}")
    } else {
        format!("{origin}/{control}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msearch_well_formed() {
        let m = ssdp_msearch();
        assert!(m.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(m.contains("urn:schemas-upnp-org:device:MediaRenderer:1"));
        assert!(m.ends_with("\r\n\r\n"));
    }

    #[test]
    fn parses_location() {
        let resp = "HTTP/1.1 200 OK\r\n\
            CACHE-CONTROL: max-age=1800\r\n\
            LOCATION: http://192.168.1.50:8200/rootDesc.xml\r\n\
            SERVER: Linux/3.x UPnP/1.0 MiniDLNA/1.2\r\n\
            ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        let d = parse_ssdp_response(resp).unwrap();
        assert_eq!(d.location, "http://192.168.1.50:8200/rootDesc.xml");
        assert_eq!(d.kind, CastKind::Dlna);
        assert!(d.name.contains("MiniDLNA"));
    }

    #[test]
    fn no_location_is_none() {
        assert!(parse_ssdp_response("HTTP/1.1 200 OK\r\nST: foo\r\n\r\n").is_none());
    }

    #[test]
    fn set_uri_carries_url_escaped() {
        let b = soap_set_uri(0, "http://h/a?x=1&y=2");
        assert!(b.contains("SetAVTransportURI"));
        assert!(b.contains("http://h/a?x=1&amp;y=2"));
    }

    #[test]
    fn action_header_format() {
        assert_eq!(
            soap_action_header("Play"),
            "\"urn:schemas-upnp-org:service:AVTransport:1#Play\""
        );
    }

    #[test]
    fn control_url_from_description() {
        let xml = "<root><serviceList>\
            <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType>\
            <controlURL>/RC/control</controlURL></service>\
            <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType>\
            <controlURL>/AVT/control</controlURL></service>\
            </serviceList></root>";
        assert_eq!(parse_control_url(xml), Some("/AVT/control".to_string()));
        assert_eq!(parse_control_url("<root/>"), None);
    }

    #[test]
    fn url_resolution() {
        assert_eq!(resolve_url("http://10.0.0.5:8200/rootDesc.xml", "/AVT/ctl"),
                   "http://10.0.0.5:8200/AVT/ctl");
        assert_eq!(resolve_url("http://10.0.0.5:8200/rootDesc.xml", "AVT/ctl"),
                   "http://10.0.0.5:8200/AVT/ctl");
        assert_eq!(resolve_url("http://10.0.0.5:8200/d.xml", "http://10.0.0.5:9000/c"),
                   "http://10.0.0.5:9000/c");
    }
}
