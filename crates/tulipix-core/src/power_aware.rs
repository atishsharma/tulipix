//! Battery + network awareness. Pure decision module: the platform
//! crate reads `BATTERYSTATUS_*` / `nw_path_is_expensive` /
//! `NM ConnectionMetered` and feeds the snapshot here. Settings
//! provides per-policy overrides ("Always index regardless of
//! battery"). Workers query `should_pause_indexer` / `should_defer_downloads`
//! before scheduling.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerSource { Ac, Battery, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkCost { Unmetered, Metered, Offline, Unknown }

/// 0..=100; None when the host reports no battery (desktop).
pub type BatteryPercent = Option<u8>;

#[derive(Debug, Clone, Copy)]
pub struct PowerSnapshot {
    pub source: PowerSource,
    pub battery: BatteryPercent,
    pub network: NetworkCost,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[derive(Default)]
pub struct PowerOverrides {
    /// Force background workers to keep running even on low battery.
    pub always_index: bool,
    /// Allow large downloads on metered links.
    pub allow_metered_downloads: bool,
}


/// Read the host's power source.
///
/// This module was written as a pure decision layer expecting "the platform
/// crate" to feed it a snapshot; no such reader was ever wired, so every
/// consumer of `should_pause` would have had to invent one. Linux is read here
/// directly because `/sys/class/power_supply` needs nothing but a file read.
///
/// Other platforms answer `Unknown`, which every policy above treats as "do not
/// pause" — the safe direction: a desktop that cannot report a battery should
/// not have its background work switched off on suspicion.
pub fn power_source() -> PowerSource {
    #[cfg(target_os = "linux")]
    {
        let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
            return PowerSource::Unknown;
        };
        let mut saw_battery = false;
        for e in dir.flatten() {
            let p = e.path();
            let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            match kind.trim() {
                // A mains supply that reports online settles it immediately.
                "Mains" | "USB" | "USB_PD" | "USB_PD_DRP" => {
                    if std::fs::read_to_string(p.join("online"))
                        .map(|s| s.trim() == "1")
                        .unwrap_or(false)
                    {
                        return PowerSource::Ac;
                    }
                }
                "Battery" => saw_battery = true,
                _ => {}
            }
        }
        // A battery present with no mains online means running off it. No
        // battery at all is a desktop, which is always on mains.
        if saw_battery { PowerSource::Battery } else { PowerSource::Ac }
    }
    #[cfg(not(target_os = "linux"))]
    {
        PowerSource::Unknown
    }
}

/// Battery charge 0..=100, or `None` when the host reports no battery.
pub fn battery_percent() -> BatteryPercent {
    #[cfg(target_os = "linux")]
    {
        let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
        for e in dir.flatten() {
            let p = e.path();
            if std::fs::read_to_string(p.join("type")).unwrap_or_default().trim() != "Battery" {
                continue;
            }
            if let Ok(cap) = std::fs::read_to_string(p.join("capacity")) {
                if let Ok(pct) = cap.trim().parse::<u8>() {
                    return Some(pct.min(100));
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerClass { Indexer, Transcoder, ToolsBackground, Download, RealtimeNotify }

const BATTERY_PAUSE_THRESHOLD: u8 = 20;

pub fn should_pause(class: WorkerClass, snap: PowerSnapshot, ov: PowerOverrides) -> bool {
    // Realtime watcher never pauses — losing notify events corrupts the proxy index.
    if class == WorkerClass::RealtimeNotify { return false; }

    if class == WorkerClass::Download {
        return matches!(snap.network, NetworkCost::Metered | NetworkCost::Offline) && !ov.allow_metered_downloads;
    }

    if ov.always_index { return false; }

    match (snap.source, snap.battery) {
        (PowerSource::Battery, Some(pct)) if pct < BATTERY_PAUSE_THRESHOLD => true,
        _ => false,
    }
}

pub fn should_defer_downloads(snap: PowerSnapshot, ov: PowerOverrides) -> bool {
    if ov.allow_metered_downloads { return false; }
    matches!(snap.network, NetworkCost::Metered)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision { Run, Pause, Defer }

pub fn decide(class: WorkerClass, snap: PowerSnapshot, ov: PowerOverrides) -> Decision {
    if class == WorkerClass::Download && should_defer_downloads(snap, ov) { return Decision::Defer; }
    if should_pause(class, snap, ov) { Decision::Pause } else { Decision::Run }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snap(source: PowerSource, battery: BatteryPercent, network: NetworkCost) -> PowerSnapshot {
        PowerSnapshot { source, battery, network }
    }
    #[test] fn low_battery_pauses_indexer() {
        let s = snap(PowerSource::Battery, Some(15), NetworkCost::Unmetered);
        assert!(should_pause(WorkerClass::Indexer, s, PowerOverrides::default()));
        assert!(should_pause(WorkerClass::Transcoder, s, PowerOverrides::default()));
        assert!(should_pause(WorkerClass::ToolsBackground, s, PowerOverrides::default()));
    }
    #[test] fn realtime_never_pauses() {
        let s = snap(PowerSource::Battery, Some(5), NetworkCost::Offline);
        assert!(!should_pause(WorkerClass::RealtimeNotify, s, PowerOverrides::default()));
    }
    #[test] fn always_index_override_wins_for_workers_not_downloads() {
        let s = snap(PowerSource::Battery, Some(5), NetworkCost::Metered);
        let ov = PowerOverrides { always_index: true, allow_metered_downloads: false };
        assert!(!should_pause(WorkerClass::Indexer, s, ov));
        // Download still defers because override is separate.
        assert!(should_pause(WorkerClass::Download, s, ov));
    }
    #[test] fn ac_never_pauses_for_battery_reason() {
        let s = snap(PowerSource::Ac, None, NetworkCost::Unmetered);
        assert!(!should_pause(WorkerClass::Indexer, s, PowerOverrides::default()));
    }
    #[test] fn metered_defers_downloads() {
        let s = snap(PowerSource::Ac, None, NetworkCost::Metered);
        assert_eq!(decide(WorkerClass::Download, s, PowerOverrides::default()), Decision::Defer);
        let ov = PowerOverrides { allow_metered_downloads: true, ..Default::default() };
        assert_eq!(decide(WorkerClass::Download, s, ov), Decision::Run);
    }
    #[test] fn offline_pauses_downloads_unconditionally() {
        let s = snap(PowerSource::Ac, None, NetworkCost::Offline);
        assert!(should_pause(WorkerClass::Download, s, PowerOverrides { allow_metered_downloads: false, ..Default::default() }));
    }
}
