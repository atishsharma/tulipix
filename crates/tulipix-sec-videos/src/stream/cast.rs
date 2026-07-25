//! Casting a stream to a DLNA renderer on the LAN.
//!
//! Simpler than the music path: a Stream URL is already a public HTTP URL, so
//! the renderer fetches it directly and there is no local media server to stand
//! up. Discovery and the SOAP bodies come from `tulipix_music::cast`, which is
//! protocol code with nothing audio-specific in it.

use super::*;
use tulipix_music::cast as dlna;

/// Renderers found by the last scan, in the order the UI lists them.
static TARGETS: OnceLock<Mutex<Vec<dlna::CastDevice>>> = OnceLock::new();
fn targets() -> &'static Mutex<Vec<dlna::CastDevice>> {
    TARGETS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Look for renderers and fill the picker. SSDP is UDP broadcast with a short
/// listen window, so this blocks a worker thread rather than the UI.
pub fn stream_cast_discover(weak: slint::Weak<MainWindow>) {
    set_status(&weak, "Looking for devices…", true);
    tokio::runtime::Handle::current().spawn(async move {
        let found = tokio::task::spawn_blocking(discover).await.unwrap_or_default();
        let names: Vec<String> = found.iter().map(|d| short_name(&d.name)).collect();
        if let Ok(mut g) = targets().lock() {
            *g = found;
        }
        let count = names.len();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<slint::SharedString> = names.into_iter().map(Into::into).collect();
            w.set_video_stream_cast_devices(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_video_stream_status(
                match count {
                    0 => "No cast devices found on this network.".to_string(),
                    1 => "1 device found.".to_string(),
                    n => format!("{n} devices found."),
                }
                .into(),
            );
            w.set_video_stream_busy(false);
        });
    });
}

/// One SSDP M-SEARCH round. Blocking; call it off the UI thread.
fn discover() -> Vec<dlna::CastDevice> {
    use std::net::UdpSocket;
    let mut out: Vec<dlna::CastDevice> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out };
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(600)));
    let _ = sock.send_to(dlna::ssdp_msearch().as_bytes(), "239.255.255.250:1900");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let raw = String::from_utf8_lossy(&buf[..n]);
        if let Some(dev) = dlna::parse_ssdp_response(&raw) {
            if !out.iter().any(|d| d.location == dev.location) {
                out.push(dev);
            }
        }
    }
    out
}

/// SSDP `SERVER` headers read like "Linux/4.9 UPnP/1.0 Sony/1.0" — keep the
/// part with a name in it rather than the kernel version.
fn short_name(raw: &str) -> String {
    let cleaned = raw
        .split_whitespace()
        .filter(|part| !part.starts_with("UPnP/") && !part.starts_with("Linux/"))
        .collect::<Vec<_>>()
        .join(" ");
    let name = if cleaned.trim().is_empty() { raw } else { &cleaned };
    truncate_chars(name.trim(), 40)
}

/// Send the armed stream to the picked renderer.
pub fn stream_cast_to(weak: slint::Weak<MainWindow>, device: String) {
    let index = current_index();
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "Pick a stream first.", false);
        return;
    };
    let Some(dev) = targets()
        .lock()
        .ok()
        .and_then(|g| g.iter().find(|d| short_name(&d.name) == device).cloned())
    else {
        set_status(&weak, "That device is no longer listed — scan again.", false);
        return;
    };
    set_status(&weak, format!("Casting to {device}…"), true);

    tokio::runtime::Handle::current().spawn(async move {
        let client = tulipix_core::net::http().clone();
        // The device description says where its AVTransport control endpoint is.
        let desc = match client.get(&dev.location).send().await {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(_) => String::new(),
        };
        let Some(ctl) = dlna::parse_control_url(&desc) else {
            return set_status(&weak, "That device cannot play video (no AVTransport).", false);
        };
        let ctl = dlna::resolve_url(&dev.location, &ctl);

        for (action, body) in [
            ("SetAVTransportURI", dlna::soap_set_uri(0, &file.url)),
            ("Play", dlna::soap_play(0)),
        ] {
            let ok = client
                .post(&ctl)
                .header("SOAPACTION", dlna::soap_action_header(action))
                .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
                .body(body)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if !ok {
                return set_status(&weak, format!("{device} refused the stream."), false);
            }
        }
        if let Ok(mut g) = session().lock() {
            *g = Some(ctl);
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_stream_cast_active(true);
            w.set_video_stream_cast_target(device.clone().into());
            w.set_video_stream_status(format!("Casting to {device}.").into());
            w.set_video_stream_busy(false);
        });
    });
}

/// Control endpoint of the session in progress, so Stop can reach it.
static SESSION: OnceLock<Mutex<Option<String>>> = OnceLock::new();
fn session() -> &'static Mutex<Option<String>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

/// Stop the renderer and hand playback back to this machine.
pub fn stream_cast_stop(weak: slint::Weak<MainWindow>) {
    let Some(ctl) = session().lock().ok().and_then(|g| g.clone()) else {
        let _ = weak.upgrade_in_event_loop(|w| w.set_video_stream_cast_active(false));
        return;
    };
    tokio::runtime::Handle::current().spawn(async move {
        let _ = tulipix_core::net::http()
            .post(&ctl)
            .header("SOAPACTION", dlna::soap_action_header("Stop"))
            .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
            .body(dlna::soap_stop(0))
            .send()
            .await;
        if let Ok(mut g) = session().lock() {
            *g = None;
        }
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_cast_active(false);
            w.set_video_stream_cast_target("".into());
            w.set_video_stream_status("Cast stopped.".into());
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_drop_the_boilerplate_headers() {
        assert_eq!(short_name("Linux/4.9 UPnP/1.0 Sony/1.0"), "Sony/1.0");
        assert_eq!(short_name("Samsung TV"), "Samsung TV");
        // Nothing left after filtering: keep the original rather than a blank row.
        assert_eq!(short_name("UPnP/1.0"), "UPnP/1.0");
        assert_eq!(short_name(&"x".repeat(60)).chars().count(), 41); // truncated + ellipsis
    }
}
