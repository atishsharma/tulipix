//! `np.p4.books.reader.typography` — EPUB typography controls.
//!
//! Font size, line spacing, margins, and a font-family picker (Serif /
//! Sans-serif / OpenDyslexic). EPUB content renders in a styled view; this
//! produces the injected CSS and clamps every value to a sane range so a user
//! can't make text unreadable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum FontFamily { Serif, SansSerif, OpenDyslexic }

impl FontFamily {
    pub fn css_stack(self) -> &'static str {
        match self {
            FontFamily::Serif => "Georgia, 'Times New Roman', serif",
            FontFamily::SansSerif => "'Helvetica Neue', Arial, sans-serif",
            FontFamily::OpenDyslexic => "'OpenDyslexic', sans-serif",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Typography {
    pub font_px: f64,     // 12..32
    pub line_height: f64, // 1.0..2.5
    pub margin_pct: f64,  // 0..25 (% of viewport width each side)
    pub family: FontFamily,
}

impl Default for Typography {
    fn default() -> Self { Self { font_px: 18.0, line_height: 1.5, margin_pct: 8.0, family: FontFamily::Serif } }
}

impl Typography {
    pub fn clamped(mut self) -> Self {
        self.font_px = self.font_px.clamp(12.0, 32.0);
        self.line_height = self.line_height.clamp(1.0, 2.5);
        self.margin_pct = self.margin_pct.clamp(0.0, 25.0);
        self
    }

    /// CSS injected into the reader's content frame.
    pub fn to_css(self) -> String {
        let c = self.clamped();
        format!(
            "body {{ font-family: {}; font-size: {}px; line-height: {}; margin: 0 {}%; }}",
            c.family.css_stack(), c.font_px, c.line_height, c.margin_pct
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_reflects_settings() {
        let t = Typography { font_px: 20.0, line_height: 1.8, margin_pct: 10.0, family: FontFamily::OpenDyslexic };
        let css = t.to_css();
        assert!(css.contains("font-size: 20px"));
        assert!(css.contains("line-height: 1.8"));
        assert!(css.contains("OpenDyslexic"));
        assert!(css.contains("margin: 0 10%"));
    }

    #[test]
    fn values_clamp() {
        let t = Typography { font_px: 99.0, line_height: 0.1, margin_pct: 99.0, family: FontFamily::Serif }.clamped();
        assert_eq!(t.font_px, 32.0);
        assert_eq!(t.line_height, 1.0);
        assert_eq!(t.margin_pct, 25.0);
    }
}
