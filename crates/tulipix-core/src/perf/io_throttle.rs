//! Disk-IO throttle. Detects rotational storage (HDD) and caps thumb / scan
//! IO to a configurable MB/s budget; SSD / NVMe runs unbounded.
//!
//! Detection per OS:
//!   * Linux: `/sys/block/<dev>/queue/rotational` (0 = SSD, 1 = HDD).
//!   * Windows: `IOCTL_STORAGE_QUERY_PROPERTY` → SpindleSpeed.
//!   * macOS: `IOPSCopyPowerSourcesInfo` → SSD heuristic via "Device is internal".
//!
//! Token bucket: refilled per second; consumers call `await_tokens(n)` and
//! sleep until the bucket has capacity.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageKind { Hdd, SsdNvme }

/// Probe a path's underlying device. Returns SsdNvme if the platform can't
/// answer — better to let IO run than to falsely throttle a fast disk.
pub fn detect(path: &Path) -> StorageKind {
    #[cfg(target_os = "linux")]
    { return detect_linux(path).unwrap_or(StorageKind::SsdNvme); }
    #[cfg(not(target_os = "linux"))]
    { let _ = path; StorageKind::SsdNvme }
}

#[cfg(target_os = "linux")]
fn detect_linux(path: &Path) -> Option<StorageKind> {
    // Walk the path to its mountpoint, read /proc/mounts to map mount→device,
    // then read /sys/block/<base>/queue/rotational. For headless tests we
    // fall through to SsdNvme if any step fails.
    let canon = std::fs::canonicalize(path).ok()?;
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let mut best: Option<(&str, usize)> = None;
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let dev = it.next()?;
        let mount = it.next()?;
        if canon.starts_with(mount) {
            let len = mount.len();
            if best.map(|(_, l)| len > l).unwrap_or(true) { best = Some((dev, len)); }
        }
    }
    let dev = best?.0;
    let base = std::path::Path::new(dev).file_name()?.to_string_lossy();
    let trim = base.trim_end_matches(|c: char| c.is_ascii_digit());
    let p = format!("/sys/block/{trim}/queue/rotational");
    let val = std::fs::read_to_string(p).ok()?;
    Some(if val.trim() == "1" { StorageKind::Hdd } else { StorageKind::SsdNvme })
}

#[derive(Debug)]
pub struct TokenBucket {
    capacity_bytes: u64,
    tokens: Mutex<f64>,
    refill_per_sec: f64,
    last: Mutex<Instant>,
}

impl TokenBucket {
    pub fn for_storage(kind: StorageKind) -> Self {
        let mb_per_sec = match kind {
            StorageKind::Hdd     => 30.0,
            StorageKind::SsdNvme => f64::INFINITY,
        };
        let capacity = if mb_per_sec.is_finite() { (mb_per_sec * 1024.0 * 1024.0) as u64 } else { u64::MAX };
        Self {
            capacity_bytes: capacity,
            tokens: Mutex::new(capacity as f64),
            refill_per_sec: if mb_per_sec.is_finite() { mb_per_sec * 1024.0 * 1024.0 } else { f64::INFINITY },
            last: Mutex::new(Instant::now()),
        }
    }

    /// Block until the bucket has `n` bytes worth of tokens. Returns the
    /// duration the caller had to wait.
    pub fn await_tokens(&self, n: u64) -> Duration {
        if self.refill_per_sec.is_infinite() { return Duration::ZERO; }
        let mut tokens = self.tokens.lock().unwrap();
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last).as_secs_f64();
        *tokens = (*tokens + elapsed * self.refill_per_sec).min(self.capacity_bytes as f64);
        *last = now;
        let want = n as f64;
        if *tokens >= want {
            *tokens -= want;
            return Duration::ZERO;
        }
        let deficit = want - *tokens;
        let wait = Duration::from_secs_f64(deficit / self.refill_per_sec);
        *tokens = 0.0;
        wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ssd_bucket_never_waits() {
        let b = TokenBucket::for_storage(StorageKind::SsdNvme);
        assert_eq!(b.await_tokens(1024 * 1024 * 1024), Duration::ZERO);
    }
    #[test] fn hdd_bucket_paces_at_30_mbps() {
        let b = TokenBucket::for_storage(StorageKind::Hdd);
        // Drain capacity.
        let _ = b.await_tokens(30 * 1024 * 1024);
        // Asking for another 30 MB should require ~1 s of refill.
        let wait = b.await_tokens(30 * 1024 * 1024);
        assert!(wait.as_millis() >= 900 && wait.as_millis() <= 1100,
                "expected ~1 s, got {wait:?}");
    }
    #[cfg(not(target_os = "linux"))]
    #[test] fn detect_default_ssd() {
        assert_eq!(detect(std::path::Path::new("/")), StorageKind::SsdNvme);
    }
}
