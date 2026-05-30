//! `np.p4.cloud.sync` — one-way + bisync with bandwidth throttle + schedule.
//!
//! Builds `rclone sync` (one-way mirror) and `rclone bisync` (two-way) argv
//! with `--bwlimit`, and parses a human throttle string ("2M", "500k") into
//! the rclone form. Schedule cadence is a simple interval the core scheduler
//! polls.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction { OneWay, BiSync }

/// Parse a human bandwidth string into rclone's `--bwlimit` value. Accepts
/// `"2M"`, `"500k"`, `"1.5M"`, `"off"`; returns `None` if unparseable.
pub fn parse_bwlimit(s: &str) -> Option<String> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("off") || t == "0" { return Some("off".into()); }
    let (num, unit) = t.split_at(t.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(t.len()));
    let n: f64 = num.trim().parse().ok()?;
    if n <= 0.0 { return None; }
    let u = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "k" | "kib" | "kb" => "k",
        "m" | "mib" | "mb" => "M",
        "g" | "gib" | "gb" => "G",
        _ => return None,
    };
    Some(format!("{}{u}", trim_float(n)))
}

fn trim_float(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Build the sync argv. `bwlimit` is an already-validated rclone value.
pub fn sync_args(dir: Direction, src: &str, dst: &str, bwlimit: Option<&str>) -> Vec<String> {
    let mut a = match dir {
        Direction::OneWay => vec!["sync".into(), src.into(), dst.into()],
        Direction::BiSync => vec!["bisync".into(), src.into(), dst.into(), "--resync-mode".into(), "newer".into()],
    };
    if let Some(bw) = bwlimit { a.push("--bwlimit".into()); a.push(bw.into()); }
    a.push("--transfers".into()); a.push("4".into());
    a
}

/// Is a scheduled run due? `last_run` + `interval_s` ≤ now.
pub fn is_due(last_run_unix: i64, interval_s: i64, now_unix: i64) -> bool {
    interval_s > 0 && now_unix >= last_run_unix + interval_s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bwlimit_parsing() {
        assert_eq!(parse_bwlimit("2M"), Some("2M".into()));
        assert_eq!(parse_bwlimit("500k"), Some("500k".into()));
        assert_eq!(parse_bwlimit("1.5M"), Some("1.5M".into()));
        assert_eq!(parse_bwlimit("off"), Some("off".into()));
        assert_eq!(parse_bwlimit("750"), Some("750k".into())); // bare = KiB
        assert_eq!(parse_bwlimit("nonsense"), None);
    }

    #[test]
    fn argv_shapes() {
        let a = sync_args(Direction::OneWay, "gdrive:", "/local", Some("2M"));
        assert_eq!(a[0], "sync");
        assert!(a.windows(2).any(|w| w == ["--bwlimit", "2M"]));
        let b = sync_args(Direction::BiSync, "a:", "b:", None);
        assert_eq!(b[0], "bisync");
        assert!(!b.contains(&"--bwlimit".to_string()));
    }

    #[test]
    fn schedule_due() {
        assert!(is_due(1000, 60, 1060));
        assert!(!is_due(1000, 60, 1059));
        assert!(!is_due(1000, 0, 9999)); // disabled
    }
}
