//! System-accent following — Material You / macOS controlAccentColor /
//! Windows 11 UISettings.AccentColor / GNOME accent-color. Read-only probe;
//! the app reroutes its section-accent palette into ui/tokens.slint based
//! on the user's Settings → Appearance preference (Section / System / Custom).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccentSource {
    /// Per-section palette (default — violet/indigo/pink/teal/amber).
    Section,
    /// Single OS-provided accent for every section.
    System,
    /// User-picked color in Settings.
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb { pub r: u8, pub g: u8, pub b: u8 }

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self { Self { r, g, b } }
    pub fn to_css(self) -> String { format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b) }
}

/// OS accent probe. Returns `None` on platforms where the API is unavailable;
/// caller falls back to the Tulipix default violet.
pub fn read_system_accent() -> Option<Rgb> {
    #[cfg(target_os = "macos")]
    { return read_macos(); }
    #[cfg(target_os = "windows")]
    { return read_windows(); }
    #[cfg(target_os = "linux")]
    { return read_linux(); }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { None }
}

#[cfg(target_os = "macos")]
fn read_macos() -> Option<Rgb> {
    // NSColor.controlAccentColor lives on the AppKit main thread. The
    // SwiftUI side caches the value into UserDefaults under AppleAccentColor
    // (-1 graphite, 0 red, 1 orange, 2 yellow, 3 green, 4 blue, 5 purple, 6 pink, 7 multi).
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleAccentColor"])
        .output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let idx: i32 = s.trim().parse().ok()?;
    Some(match idx {
        -1 => Rgb::new(0x8d, 0x8d, 0x8d), // graphite
         0 => Rgb::new(0xe0, 0x38, 0x3e), // red
         1 => Rgb::new(0xf5, 0x82, 0x1f), // orange
         2 => Rgb::new(0xf7, 0xba, 0x15), // yellow
         3 => Rgb::new(0x62, 0xba, 0x46), // green
         4 => Rgb::new(0x00, 0x7a, 0xff), // blue
         5 => Rgb::new(0xa3, 0x36, 0xa0), // purple
         6 => Rgb::new(0xf7, 0x4f, 0x9e), // pink
         _ => Rgb::new(0x00, 0x7a, 0xff),
    })
}

#[cfg(target_os = "windows")]
fn read_windows() -> Option<Rgb> {
    // Win11 UISettings.GetColorValue(UIColorType.Accent) returns sRGB; the
    // WinRT binding lives in the windows crate but to keep this stub
    // portable we shell to `reg query` against DWM\AccentColor.
    let out = std::process::Command::new("reg")
        .args(["query", "HKCU\\Software\\Microsoft\\Windows\\DWM", "/v", "AccentColor"])
        .output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let hex = s.split_whitespace().last()?;
    let v = u32::from_str_radix(hex.trim_start_matches("0x"), 16).ok()?;
    // DWM stores ABGR.
    let r = (v & 0xff) as u8;
    let g = ((v >> 8) & 0xff) as u8;
    let b = ((v >> 16) & 0xff) as u8;
    Some(Rgb::new(r, g, b))
}

#[cfg(target_os = "linux")]
fn read_linux() -> Option<Rgb> {
    // GNOME 47 exposes `org.gnome.desktop.interface accent-color`; KDE 6 ships
    // `kdeglobals` [General] AccentColor=r,g,b.
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "accent-color"]).output()
    {
        let s = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_string();
        if let Some(rgb) = gnome_accent_name_to_rgb(&s) { return Some(rgb); }
    }
    None
}

fn gnome_accent_name_to_rgb(name: &str) -> Option<Rgb> {
    Some(match name {
        "blue"    => Rgb::new(0x35, 0x84, 0xe4),
        "teal"    => Rgb::new(0x2a, 0xa1, 0xb3),
        "green"   => Rgb::new(0x33, 0xd1, 0x7a),
        "yellow"  => Rgb::new(0xf6, 0xd3, 0x2d),
        "orange"  => Rgb::new(0xff, 0x79, 0x00),
        "red"     => Rgb::new(0xe0, 0x1b, 0x24),
        "pink"    => Rgb::new(0xff, 0x67, 0x8f),
        "purple"  => Rgb::new(0x91, 0x41, 0xac),
        "slate"   => Rgb::new(0x6c, 0x71, 0x82),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rgb_css_round_trip() {
        assert_eq!(Rgb::new(0xff, 0x00, 0x80).to_css(), "#ff0080");
    }
    #[test] fn gnome_accent_name_lookup() {
        assert_eq!(gnome_accent_name_to_rgb("teal").unwrap(), Rgb::new(0x2a, 0xa1, 0xb3));
        assert!(gnome_accent_name_to_rgb("unknown").is_none());
    }
}
