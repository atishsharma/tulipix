//! Which address to put in the URL. std has no API for this, hence `if-addrs`.

use std::net::Ipv4Addr;

/// Whether an address is one only the local network can reach: RFC1918
/// (10/8, 172.16/12, 192.168/16) plus RFC6598 shared space (100.64/10), which
/// some tethering and carrier setups hand out.
///
/// Anything else is a globally routable address, and machines do have them — a
/// VPS, a modem in bridge mode, an ISP that puts a public IPv4 straight on your
/// NIC. This section is local-only, so those are not "less preferred", they are
/// not offered at all. Putting one in a QR code would be an invitation to the
/// whole internet, guarded by six digits.
pub fn is_local(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || (o[0] == 100 && (64..128).contains(&o[1]))
}

/// Interfaces that carry a private address no phone can reach: container
/// bridges, VM host-only networks, VPN tunnels. `docker0` is `172.17.0.1` —
/// every bit as RFC1918 as the Wi-Fi link, and completely useless here, so
/// ranking by address alone will happily put it first.
///
/// Demoted rather than dropped: a deliberately-configured bridge is still a
/// network someone might be sharing over, and the Send pane offers a picker.
fn looks_virtual(name: &str) -> bool {
    const VIRTUAL: [&str; 12] = [
        "docker", "br-", "bridge", "virbr", "veth", "vmnet", "vboxnet", "tun", "tap", "wg",
        "zt", "tailscale",
    ];
    let name = name.to_ascii_lowercase();
    VIRTUAL.iter().any(|p| name.starts_with(p))
        // macOS names VPN tunnels utun0..N, but also uses utun for AirDrop.
        || (name.starts_with("utun") && cfg!(target_os = "macos"))
}

/// Addresses a phone on this network could reach, best first. Both a hotspot
/// and a Wi-Fi link commonly qualify at once, which is why this returns a list.
///
/// Ordering, best first: a real link on an RFC1918 address, then a real link on
/// shared space, then anything virtual. The phone-as-hotspot case — the laptop
/// sitting on `192.168.43.x` or `172.20.10.x` with the phone as gateway — lands
/// at the top, which is the whole point.
pub fn usable(all: Vec<(String, Ipv4Addr)>) -> Vec<(String, Ipv4Addr)> {
    let mut out: Vec<(String, Ipv4Addr)> = all
        .into_iter()
        .filter(|(_, ip)| is_local(ip) && !ip.is_unspecified())
        .collect();
    // RFC1918 ahead of shared space: a 100.64 address is far more likely to be
    // a carrier's NAT than the Wi-Fi the phone is sitting on.
    out.sort_by_key(|(name, ip)| (looks_virtual(name), !ip.is_private()));
    out
}

/// The real lookup. Kept apart from `usable` so the ordering rules stay testable.
pub fn interfaces() -> Vec<(String, Ipv4Addr)> {
    let Ok(addrs) = if_addrs::get_if_addrs() else { return Vec::new() };
    usable(
        addrs
            .into_iter()
            .filter_map(|i| match i.ip() {
                std::net::IpAddr::V4(v4) => Some((i.name, v4)),
                std::net::IpAddr::V6(_) => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    #[test]
    fn loopback_and_link_local_are_never_offered() {
        let all = vec![
            ("lo".to_string(), v4(127, 0, 0, 1)),
            ("wlan0".to_string(), v4(169, 254, 3, 4)),
            ("wlan1".to_string(), v4(192, 168, 1, 20)),
        ];
        let out = usable(all);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, v4(192, 168, 1, 20));
    }

    #[test]
    fn a_routable_address_is_never_offered() {
        // A machine really can hold one of these — a VPS, or an ISP handing a
        // public IPv4 straight to the NIC. Offering it would publish the share
        // to the internet, so it is dropped rather than ranked last.
        let all = vec![
            ("eth0".to_string(), v4(9, 9, 9, 9)),
            ("eth1".to_string(), v4(203, 0, 113, 7)),
            ("wlan0".to_string(), v4(10, 0, 0, 5)),
        ];
        let out = usable(all);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, v4(10, 0, 0, 5));
    }

    #[test]
    fn every_private_range_is_recognised_and_nothing_else_is() {
        for ip in [v4(10, 0, 0, 5), v4(172, 16, 4, 1), v4(172, 31, 255, 254), v4(192, 168, 1, 20)] {
            assert!(is_local(&ip), "{ip} should be local");
        }
        // 172.15 and 172.32 sit just outside the /12.
        for ip in [v4(172, 15, 0, 1), v4(172, 32, 0, 1), v4(8, 8, 8, 8), v4(203, 0, 113, 7)] {
            assert!(!is_local(&ip), "{ip} should not be local");
        }
    }

    #[test]
    fn shared_address_space_counts_as_local_but_ranks_last() {
        // RFC6598 — unreachable from the internet by definition, so it is safe
        // to offer, but it is more often a carrier's NAT than the phone's Wi-Fi.
        assert!(is_local(&v4(100, 100, 0, 1)));
        assert!(!is_local(&v4(100, 128, 0, 1)));
        assert!(!is_local(&v4(100, 63, 0, 1)));

        let out = usable(vec![
            ("cg0".to_string(), v4(100, 100, 0, 1)),
            ("wlan0".to_string(), v4(192, 168, 1, 20)),
        ]);
        assert_eq!(out[0].1, v4(192, 168, 1, 20));
    }

    #[test]
    fn a_hotspot_and_a_wifi_link_are_both_kept() {
        // Both qualify, so the UI shows a picker rather than the code guessing.
        let all = vec![
            ("wlan0".to_string(), v4(192, 168, 1, 20)),
            ("ap0".to_string(), v4(192, 168, 43, 1)),
        ];
        assert_eq!(usable(all).len(), 2);
    }

    #[test]
    fn a_container_bridge_never_outranks_a_real_link() {
        // The phone-as-hotspot case: the laptop is a client on the phone's AP,
        // and docker0 holds an equally-private address that no phone can reach.
        // getifaddrs order is not stable, so both orders are tested.
        let bridge = ("docker0".to_string(), v4(172, 17, 0, 1));
        let hotspot = ("wlan0".to_string(), v4(192, 168, 43, 137));

        for all in [vec![bridge.clone(), hotspot.clone()], vec![hotspot.clone(), bridge.clone()]] {
            let out = usable(all);
            assert_eq!(out[0].1, v4(192, 168, 43, 137), "bridge won the default");
            assert_eq!(out.len(), 2, "the bridge is demoted, not hidden");
        }
    }

    #[test]
    fn an_ios_personal_hotspot_client_address_is_offered() {
        // 172.20.10.0/28 — inside 172.16/12, so it must survive the filter.
        let out = usable(vec![("en0".to_string(), v4(172, 20, 10, 3))]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn vpn_and_vm_interfaces_rank_below_everything_real() {
        let out = usable(vec![
            ("tun0".to_string(), v4(10, 8, 0, 2)),
            ("virbr0".to_string(), v4(192, 168, 122, 1)),
            ("wlan0".to_string(), v4(192, 168, 43, 137)),
        ]);
        assert_eq!(out[0].0, "wlan0");
    }
}
