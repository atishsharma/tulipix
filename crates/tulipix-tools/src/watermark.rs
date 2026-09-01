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

/// `(x, y)` for `drawtext`, which names things differently from `overlay`.
///
/// This is the bug that put every text watermark off the canvas: `overlay`
/// calls the base `W,H` and the thing being placed `w,h`, while `drawtext`
/// calls the base `w,h` and the text `text_w,text_h`. Bottom-right came out as
/// `W-w-28`, which drawtext reads as width minus width minus 28 — twenty-eight
/// pixels off the left edge, not the right.
fn text_xy(pos: Position, margin: u32) -> (String, String) {
    let m = margin;
    match pos {
        Position::TopLeft => (format!("{m}"), format!("{m}")),
        Position::TopRight => (format!("w-text_w-{m}"), format!("{m}")),
        Position::BottomLeft => (format!("{m}"), format!("h-text_h-{m}")),
        Position::BottomRight => (format!("w-text_w-{m}"), format!("h-text_h-{m}")),
        Position::Center => ("(w-text_w)/2".into(), "(h-text_h)/2".into()),
    }
}

/// `-vf drawtext` value for a text watermark.
pub fn text_filter(text: &str, pos: Position, margin: u32, opacity: f64, size: u32) -> String {
    let o = opacity.clamp(0.0, 1.0);
    let (x, y) = text_xy(pos, margin);
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

    #[test]
    fn text_is_placed_with_drawtext_variables_not_overlay_ones() {
        let t = text_filter("hi", Position::BottomRight, 28, 1.0, 24);
        // `W-w` is overlay's language and evaluates to zero in drawtext, which
        // is how the watermark ended up off the left edge of every picture.
        assert!(!t.contains("W-w"), "{t}");
        assert!(t.contains("x=w-text_w-28"), "{t}");
        assert!(t.contains("y=h-text_h-28"), "{t}");
        assert!(text_filter("hi", Position::Center, 0, 1.0, 24).contains("(w-text_w)/2"));
    }
}
