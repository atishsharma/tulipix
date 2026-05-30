//! `np.p4.tools.watermark` — batch PNG logo or text overlay on export.
//!
//! Builds the ffmpeg `overlay` (image) / `drawtext` (text) filter with a
//! position preset → x/y expression mapping and an opacity control. Applies to
//! photos and video frames alike.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position { TopLeft, TopRight, BottomLeft, BottomRight, Center }

impl Position {
    /// (x, y) ffmpeg overlay expressions for `margin` px from the edges.
    /// `W,H` = base size, `w,h` = overlay size (ffmpeg-provided variables).
    pub fn xy(self, margin: u32) -> (String, String) {
        let m = margin;
        match self {
            Position::TopLeft => (format!("{m}"), format!("{m}")),
            Position::TopRight => (format!("W-w-{m}"), format!("{m}")),
            Position::BottomLeft => (format!("{m}"), format!("H-h-{m}")),
            Position::BottomRight => (format!("W-w-{m}"), format!("H-h-{m}")),
            Position::Center => ("(W-w)/2".into(), "(H-h)/2".into()),
        }
    }
}

/// `-filter_complex` value to overlay a PNG at `pos` with `opacity` (0..1).
pub fn image_filter(pos: Position, margin: u32, opacity: f64) -> String {
    let o = opacity.clamp(0.0, 1.0);
    let (x, y) = pos.xy(margin);
    // fade the logo's alpha by opacity, then overlay
    format!("[1:v]format=rgba,colorchannelmixer=aa={o}[wm];[0:v][wm]overlay={x}:{y}")
}

/// `-vf drawtext` value for a text watermark.
pub fn text_filter(text: &str, pos: Position, margin: u32, opacity: f64, size: u32) -> String {
    let o = opacity.clamp(0.0, 1.0);
    let (x, y) = pos.xy(margin);
    let esc = text.replace('\'', r"\'").replace(':', r"\:");
    format!("drawtext=text='{esc}':x={x}:y={y}:fontsize={size}:fontcolor=white@{o}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_expressions() {
        assert_eq!(Position::BottomRight.xy(10), ("W-w-10".into(), "H-h-10".into()));
        assert_eq!(Position::Center.xy(0).0, "(W-w)/2");
    }

    #[test]
    fn filters_carry_opacity() {
        let f = image_filter(Position::TopLeft, 12, 0.5);
        assert!(f.contains("aa=0.5"));
        assert!(f.contains("overlay=12:12"));
        let t = text_filter("© Me", Position::Center, 0, 0.8, 24);
        assert!(t.contains("fontcolor=white@0.8"));
    }
}
