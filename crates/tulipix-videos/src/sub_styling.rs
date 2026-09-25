//! Subtitle styling — engine-independent.
//!
//! The struct here is what the user edits in Settings → Subtitles, persisted
//! as JSON, and projected onto mpv's subtitle renderer by `to_mpv_options()`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAnchor { Top, Center, Bottom }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubEdge { None, Outline, Shadow, Box }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleStyle {
    pub font_family: String,
    pub font_size_px: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: String,        // #RRGGBB
    pub opacity: f32,         // 0.0-1.0
    pub edge: SubEdge,
    pub edge_color: String,
    pub anchor: SubAnchor,
    /// Vertical offset as a fraction of the player height; negative pushes
    /// the cue toward the top, positive toward the bottom.
    pub vertical_offset: f32,
    pub line_spacing: f32,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font_family: "Sora".into(),
            font_size_px: 28.0,
            bold: false, italic: false,
            color: "#FFFFFF".into(), opacity: 1.0,
            edge: SubEdge::Outline, edge_color: "#000000".into(),
            anchor: SubAnchor::Bottom, vertical_offset: 0.08,
            line_spacing: 1.2,
        }
    }
}

impl SubtitleStyle {
    pub fn clamp(&mut self) {
        self.font_size_px = self.font_size_px.clamp(8.0, 96.0);
        self.opacity = self.opacity.clamp(0.0, 1.0);
        self.vertical_offset = self.vertical_offset.clamp(-0.5, 0.5);
        self.line_spacing = self.line_spacing.clamp(0.8, 3.0);
    }

    pub fn to_mpv_options(&self) -> Vec<(String, String)> {
        let mut opts = vec![
            ("sub-font".into(), self.font_family.clone()),
            ("sub-font-size".into(), format!("{:.1}", self.font_size_px)),
            ("sub-color".into(), with_alpha(&self.color, self.opacity)),
            ("sub-align-x".into(), "center".into()),
            ("sub-align-y".into(), match self.anchor {
                SubAnchor::Top => "top".into(),
                SubAnchor::Center => "center".into(),
                SubAnchor::Bottom => "bottom".into(),
            }),
        ];
        if self.bold { opts.push(("sub-bold".into(), "yes".into())); }
        if self.italic { opts.push(("sub-italic".into(), "yes".into())); }
        match self.edge {
            SubEdge::Outline => opts.push(("sub-border-color".into(), self.edge_color.clone())),
            SubEdge::Shadow  => opts.push(("sub-shadow-color".into(), self.edge_color.clone())),
            SubEdge::Box     => opts.push(("sub-back-color".into(), self.edge_color.clone())),
            SubEdge::None    => {}
        }
        opts.push(("sub-pos".into(), format!("{:.0}", (0.5 + self.vertical_offset) * 100.0)));
        opts
    }
}

fn with_alpha(hex: &str, opacity: f32) -> String {
    let a = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    if hex.len() == 7 && hex.starts_with('#') {
        format!("{}{:02X}", hex, a)
    } else {
        hex.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_pass_validation() {
        let mut s = SubtitleStyle::default();
        s.clamp();
        assert!(s.font_size_px > 0.0);
        assert!(s.opacity <= 1.0);
    }

    #[test]
    fn clamp_pulls_outliers_back() {
        let mut s = SubtitleStyle { font_size_px: 200.0, opacity: 5.0, vertical_offset: 9.0, line_spacing: 0.0, ..SubtitleStyle::default() };
        s.clamp();
        assert_eq!(s.font_size_px, 96.0);
        assert_eq!(s.opacity, 1.0);
        assert_eq!(s.vertical_offset, 0.5);
        assert!(s.line_spacing >= 0.8);
    }

    #[test]
    fn mpv_options_emit_font_and_anchor() {
        let s = SubtitleStyle::default();
        let opts = s.to_mpv_options();
        assert!(opts.iter().any(|(k, v)| k == "sub-font" && v == "Sora"));
        assert!(opts.iter().any(|(k, v)| k == "sub-align-y" && v == "bottom"));
    }

    #[test]
    fn box_edge_routes_to_back_color() {
        let mut s = SubtitleStyle::default();
        s.edge = SubEdge::Box;
        assert!(s.to_mpv_options().iter().any(|(k, _)| k == "sub-back-color"));
    }

    #[test]
    fn alpha_appended_to_color() {
        let mut s = SubtitleStyle::default();
        s.opacity = 0.5;
        let opts = s.to_mpv_options();
        let color = opts.iter().find(|(k, _)| k == "sub-color").unwrap().1.clone();
        assert!(color.ends_with("80") || color.ends_with("7F"), "expected ~50% alpha hex byte, got {color}");
    }
}
