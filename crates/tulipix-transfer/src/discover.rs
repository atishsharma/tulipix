//! Finding other tulipix machines, the mirror of what `mdns.rs` publishes.
//!
//! Best-effort for exactly the reasons `mdns.rs` gives: multicast is filtered
//! on plenty of networks, and a browse that returns nothing is the normal case
//! on a hostile one. Discovery is a convenience on top of the address-and-PIN
//! path, never a replacement for it — nothing here returning an empty list may
//! stop a person typing an address.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// Another tulipix, seen on this network. Not yet trusted — pairing is what
/// makes it usable, and the UI must show the difference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub host: String,
    pub ip: IpAddr,
    pub port: u16,
}

/// Whether a record belongs in the peer list.
///
/// Our own advert comes back to us — mDNS has no notion of "everyone but me" —
/// and loopback would be this process talking to itself.
pub(crate) fn keep(found: &Found, own: &[IpAddr]) -> bool {
    !found.ip.is_loopback() && !own.contains(&found.ip)
}

/// Alive for as long as the caller holds it. Dropping it shuts the daemon down,
/// the same contract `mdns::Advert` has.
pub struct Browser {
    daemon: mdns_sd::ServiceDaemon,
    seen: Arc<Mutex<HashMap<String, Vec<Found>>>>,
    own: Vec<IpAddr>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

impl Browser {
    /// The peers seen so far, ordered by host so the grid does not reshuffle
    /// itself between polls.
    pub fn peers(&self) -> Vec<Found> {
        let Ok(seen) = self.seen.lock() else {
            return Vec::new();
        };
        let mut out: Vec<Found> = seen
            .values()
            .flat_map(|v| v.iter().cloned().filter(|f| keep(f, &self.own)))
            .collect();
        out.sort_by(|a, b| a.host.cmp(&b.host).then(a.ip.cmp(&b.ip)));
        out
    }
}

/// Start browsing. `own` is this machine's addresses, so its own advert is
/// filtered out. `None` when there is no mDNS at all, which is not an error.
pub fn browse(secure: bool, own: Vec<IpAddr>) -> Option<Browser> {
    let daemon = match mdns_sd::ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::debug!(error = %e, "transfer: no mDNS, peers will not be discovered");
            return None;
        }
    };

    let ty = if secure { "_https._tcp.local." } else { "_http._tcp.local." };
    let rx = match daemon.browse(ty) {
        Ok(rx) => rx,
        Err(e) => {
            tracing::debug!(error = %e, "transfer: mDNS browse refused");
            let _ = daemon.shutdown();
            return None;
        }
    };

    let seen: Arc<Mutex<HashMap<String, Vec<Found>>>> = Arc::new(Mutex::new(HashMap::new()));
    let sink = Arc::clone(&seen);

    std::thread::spawn(move || {
        while let Ok(event) = rx.recv() {
            match event {
                mdns_sd::ServiceEvent::ServiceResolved(info) => {
                    let host = info.get_hostname().trim_end_matches('.').to_string();
                    // Only our own kind. Another vendor's _https._tcp advert is
                    // not a tulipix and must never appear as a peer.
                    if host != crate::tls::HOST {
                        continue;
                    }
                    let port = info.get_port();
                    let addrs: Vec<Found> = info
                        .get_addresses()
                        .iter()
                        .map(|ip| Found { host: host.clone(), ip: *ip, port })
                        .collect();
                    if let Ok(mut map) = sink.lock() {
                        map.insert(info.get_fullname().to_string(), addrs);
                    }
                }
                mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => {
                    if let Ok(mut map) = sink.lock() {
                        map.remove(&fullname);
                    }
                }
                _ => {}
            }
        }
    });

    Some(Browser { daemon, seen, own })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn found(ip: [u8; 4], port: u16) -> Found {
        Found { host: "tulipix.local".into(), ip: IpAddr::V4(Ipv4Addr::from(ip)), port }
    }

    #[test]
    fn a_browser_does_not_list_the_machine_it_is_running_on() {
        let own = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 24))];
        assert!(!keep(&found([192, 168, 1, 24], 8420), &own), "our own address is not a peer");
        assert!(keep(&found([192, 168, 1, 31], 8420), &own), "another machine is");
    }

    #[test]
    fn loopback_is_never_a_peer() {
        let own = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 24))];
        assert!(
            !keep(&found([127, 0, 0, 1], 8420), &own),
            "loopback would be talking to ourselves"
        );
    }
}
