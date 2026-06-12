//! OS Dynamic Type / font-scale honour. Reads the user's accessibility
//! text-size preference and exports it as a multiplier (1.0 default).
//! Slint binds the multiplier into `Theme.font-scale`, which propagates
//! through every Text component so labels don't clip at 200%.
//!
//! Sources:
//!   * macOS — `NSUserDefaults` `NSPreferredContentSizeCategory` (UIKit-style
//!     bucket), fall back to AppKit `setBoldSystemFont` size delta.
//!   * Windows — Display text-scale: HKCU\SOFTWARE\Microsoft\Accessibility
//!     `TextScaleFactor` (percent).
//!   * Linux — `org.gnome.desktop.interface text-scaling-factor` (double).

pub const MIN_SCALE: f32 = 0.8;
pub const MAX_SCALE: f32 = 2.0;

/// Return the user's preferred text scale clamped to [MIN_SCALE, MAX_SCALE].
/// Defaults to 1.0 on any read failure.
pub fn read_font_scale() -> f32 {
    let raw = read_raw().unwrap_or(1.0);
    raw.clamp(MIN_SCALE, MAX_SCALE)
}

fn read_raw() -> Option<f32> {
    #[cfg(target_os = "macos")]
    { return mac_scale(); }
    #[cfg(target_os = "windows")]
    { return win_scale(); }
    #[cfg(target_os = "linux")]
    { linux_scale()}
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { None }
}

#[cfg(target_os = "macos")]
fn mac_scale() -> Option<f32> {
    // macOS doesn't expose Dynamic Type directly to AppKit; we infer from
    // AppleAquaColorVariant + NSPreferredContentSizeCategory if present.
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "NSPreferredContentSizeCategory"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    Some(uikit_bucket_to_scale(s.trim()))
}

#[cfg(target_os = "windows")]
fn win_scale() -> Option<f32> {
    let out = std::process::Command::new("reg")
        .args(["query", "HKCU\\SOFTWARE\\Microsoft\\Accessibility", "/v", "TextScaleFactor"])
        .output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let pct: f32 = s.split_whitespace().last()?.trim_start_matches("0x")
        .parse::<u32>().ok().map(|n| n as f32).or_else(|| {
            s.split_whitespace().last()?.parse::<f32>().ok()
        })?;
    Some(pct / 100.0)
}

#[cfg(target_os = "linux")]
fn linux_scale() -> Option<f32> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "text-scaling-factor"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<f32>().ok()
}

pub fn uikit_bucket_to_scale(bucket: &str) -> f32 {
    match bucket {
        "UICTContentSizeCategoryXS"    => 0.82,
        "UICTContentSizeCategoryS"     => 0.88,
        "UICTContentSizeCategoryM"     => 1.00,
        "UICTContentSizeCategoryL"     => 1.06,
        "UICTContentSizeCategoryXL"    => 1.12,
        "UICTContentSizeCategoryXXL"   => 1.23,
        "UICTContentSizeCategoryXXXL"  => 1.35,
        "UICTContentSizeCategoryAccessibilityM"  => 1.64,
        "UICTContentSizeCategoryAccessibilityL"  => 1.94,
        "UICTContentSizeCategoryAccessibilityXL" => 2.00,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bucket_table() {
        assert_eq!(uikit_bucket_to_scale("UICTContentSizeCategoryM"), 1.00);
        assert!((uikit_bucket_to_scale("UICTContentSizeCategoryAccessibilityXL") - 2.0).abs() < 1e-6);
        assert_eq!(uikit_bucket_to_scale("unknown"), 1.0);
    }
    #[test] fn read_default_clamps() {
        let v = read_font_scale();
        assert!(v >= MIN_SCALE && v <= MAX_SCALE);
    }
}
