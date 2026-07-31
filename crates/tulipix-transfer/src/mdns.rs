//! Publishing `tulipix.local` so the phone has a name to type instead of four
//! numbers, and so the certificate's primary name resolves to something.
//!
//! Deliberately best-effort. mDNS is the least reliable part of this feature:
//! Chrome on Android has historically not resolved `.local` names at all, some
//! routers filter multicast between wireless clients, and a phone with no
//! resolver simply gets nothing. So the name is an extra, never the path — the
//! QR carries the IP address, which always works and is in the same
//! certificate. Nothing here returning `None` costs anything but the nicer URL.

use std::net::IpAddr;

/// Alive for as long as the server is. Dropping it unregisters the record and
/// shuts the daemon down, which is what stops a stale `tulipix.local` pointing
/// at a machine that has stopped sharing.
pub struct Advert {
    daemon: mdns_sd::ServiceDaemon,
    full_name: String,
}

impl Drop for Advert {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.full_name);
        let _ = self.daemon.shutdown();
    }
}

/// Announce this machine as `tulipix.local` on every address it answers on.
pub fn advertise(ips: &[IpAddr], port: u16, secure: bool) -> Option<Advert> {
    // Loopback is in the certificate but has no business on the wire: a phone
    // resolving tulipix.local to 127.0.0.1 would be talking to itself.
    let addrs: Vec<IpAddr> = ips.iter().copied().filter(|ip| !ip.is_loopback()).collect();
    if addrs.is_empty() {
        return None;
    }

    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(error = %e, "transfer: no mDNS, tulipix.local will not resolve");
            return None;
        }
    };

    // The service type only decides how a browser-of-services lists us. What
    // actually matters is the host name, which is what carries the A records.
    let ty = if secure { "_https._tcp.local." } else { "_http._tcp.local." };
    let info = match mdns_sd::ServiceInfo::new(
        ty,
        "Tulipix Transfer",
        &format!("{}.", crate::tls::HOST),
        &addrs[..],
        port,
        None,
    ) {
        Ok(i) => i,
        Err(e) => {
            tracing::debug!(error = %e, "transfer: mDNS record refused");
            let _ = daemon.shutdown();
            return None;
        }
    };

    let full_name = info.get_fullname().to_string();
    if let Err(e) = daemon.register(info) {
        tracing::debug!(error = %e, "transfer: mDNS registration failed");
        let _ = daemon.shutdown();
        return None;
    }

    tracing::info!(host = %crate::tls::HOST, "transfer: advertised over mDNS");
    Some(Advert { daemon, full_name })
}
