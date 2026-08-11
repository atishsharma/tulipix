//! `np.p4.music.mini-player` — floating always-on-top mini-player.
//!
//! 1:1 square window with proportional resize and an OS always-on-top hint.
//! This owns the geometry math (aspect-locked, clamped) and the window-flag
//! set the platform layer applies; the actual window lives in tulipix-app.

use serde::{Deserialize, Serialize};

pub const MIN_SIDE: f64 = 160.0;
pub const MAX_SIDE: f64 = 600.0;

/// Size class of the desktop widget window (`ui/mini_widget.slint`).
///
/// Stored as a name rather than an index so a reordering of the Settings picker
/// cannot silently turn one style into another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MiniStyle {
    /// Art + track + seek + full transport, the default.
    Bar,
    /// Big square art with the transport below it.
    Square,
    /// Collapsed: art, track, play, and a chevron that slides the window
    /// buttons in and out of the right edge.
    Pill,
}

impl MiniStyle {
    /// Anything unrecognised — including the empty string a first run gives
    /// back — is Bar, which is what the widget opens as.
    pub fn from_name(name: &str) -> Self {
        match name {
            "square" => Self::Square,
            "pill" => Self::Pill,
            _ => Self::Bar,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Square => "square",
            Self::Pill => "pill",
        }
    }

    /// The design reference in logical px — the baseline `MiniWidget` scales its
    /// contents against (`base-w` / `base-h` there must match these). Slint does
    /// NOT bind the window to them: a binding would fight the compositor and the
    /// widget could never be resized.
    pub fn base_size(self) -> (f64, f64) {
        match self {
            // 420 wide, not 380: the chrome cluster went from three buttons to
            // five (theme + next layout), and at 380 the title row was left
            // eliding after about six characters. 212 tall, not 176: the bar
            // now stacks five rows in its right column — mark + window buttons,
            // track, seek, transport, volume — where it used to overlay the
            // chrome on the top-right corner and hang the volume off a flyout.
            // The artwork fills the height, so it grows with it.
            Self::Bar => (420.0, 212.0),
            Self::Square => (280.0, 496.0),
            // 300 wide, not the 260 it was drawn at: even collapsed the pill
            // carries play plus the chevron. 80 tall, not 56: a button label
            // needs somewhere to go, and at 56 there was room neither above a
            // centred button nor below it, so every tooltip came out clipped.
            Self::Pill => (300.0, 80.0),
        }
    }

    /// What the window actually opens at: the reference, `DEFAULT_SCALE` larger.
    /// The widget derives `uiscale` from its own width over `base-w`, so opening
    /// bigger scales every glyph and control with it rather than leaving margin.
    pub fn window_size(self) -> (f64, f64) {
        let (w, h) = self.base_size();
        (w * DEFAULT_SCALE, h * DEFAULT_SCALE)
    }

    /// Lower bound for the window, matching `min-width`/`min-height` in
    /// `MiniWidget`. The widget scales its contents off the window width, so a
    /// resize below this would start clipping controls instead of shrinking
    /// them.
    pub fn min_size(self) -> (f64, f64) {
        let (w, h) = self.base_size();
        (w * MIN_SCALE, h * MIN_SCALE)
    }

    /// Upper bound, matching the `uiscale` clamp in `MiniWidget`. Past this the
    /// contents stop growing and the window would just gain empty margin.
    pub fn max_size(self) -> (f64, f64) {
        let (w, h) = self.base_size();
        (w * MAX_SCALE, h * MAX_SCALE)
    }

    /// Aspect-locked resize from the corner grip. `cur_w` is the window's width
    /// right now; `dx`/`dy` are the cursor's residual offset from where it
    /// grabbed the grip, in logical px.
    ///
    /// The ratio comes from the *style*, not from the window's current
    /// proportions, so the widget snaps back to its designed shape instead of
    /// compounding whatever drift a previous drag left behind. The widget is
    /// not resizable from its edges at all — only this — precisely so no
    /// off-ratio state can be reached.
    pub fn resize_locked(self, cur_w: f64, dx: f64, dy: f64) -> (f64, f64) {
        self.resize_locked_with_extra(cur_w, dx, dy, 0.0)
    }

    /// As `resize_locked`, with `extra_ref` REFERENCE px of width that is not
    /// part of the aspect ratio — the pill's extended button cluster. The
    /// current width is converted back to its collapsed equivalent before the
    /// ratio and the clamp are applied, and the extension is re-added at the
    /// new scale, so dragging the corner while the cluster is out keeps the
    /// designed shape instead of snapping the window to the collapsed width.
    /// With `extra_ref` of 0 this is exactly `resize_locked`.
    pub fn resize_locked_with_extra(self, cur_w: f64, dx: f64, dy: f64, extra_ref: f64) -> (f64, f64) {
        let (bw, bh) = self.base_size();
        let ratio = bh / bw;
        let scale = cur_w / (bw + extra_ref);
        // Convert the vertical delta to its width equivalent so the two axes
        // are comparable, then follow whichever the cursor moved further along.
        let dy_as_w = dy / ratio;
        let delta = if dx.abs() >= dy_as_w.abs() { dx } else { dy_as_w };
        let w = (scale * bw + delta).clamp(bw * MIN_SCALE, bw * MAX_SCALE);
        (w + extra_ref * (w / bw), w * ratio)
    }
}

/// Resize bounds as a multiple of a style's base size. Kept in step with the
/// `uiscale` clamp and `min-width`/`min-height` in `ui/mini_widget.slint`.
const MIN_SCALE: f64 = 0.7;
const MAX_SCALE: f64 = 3.0;

/// Extra width the pill takes when its button cluster is showing, in reference
/// px — five 24px buttons and the gaps between them, plus the row's own spacing.
/// The pill does NOT reflow to fit these: the window grows sideways, so the
/// title keeps the width it had. `MiniWidget` scales the pill off its height for
/// exactly this reason.
pub const PILL_CLUSTER: f64 = 148.0;
/// The same, where the pin button is hidden (Wayland has no stacking request).
pub const PILL_CLUSTER_NO_PIN: f64 = 119.0;

/// How much larger than the design reference the widget opens. The reference
/// sizes were drawn for a 1× desktop and read as a stamp on anything bigger.
pub const DEFAULT_SCALE: f64 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MiniPlayer {
    /// Side length in logical px (window is square).
    pub side: f64,
    pub always_on_top: bool,
    pub visible: bool,
}

impl Default for MiniPlayer {
    fn default() -> Self { Self { side: 240.0, always_on_top: true, visible: false } }
}

impl MiniPlayer {
    /// Proportional resize from a drag on either edge: take the larger delta so
    /// the square tracks the cursor, then clamp.
    pub fn resize(&mut self, dw: f64, dh: f64) {
        let delta = if dw.abs() >= dh.abs() { dw } else { dh };
        self.side = (self.side + delta).clamp(MIN_SIDE, MAX_SIDE);
    }

    /// Window size — always square.
    pub fn window_size(&self) -> (f64, f64) { (self.side, self.side) }

    /// Platform window flags to apply (order-stable).
    pub fn window_flags(&self) -> Vec<&'static str> {
        let mut f = vec!["no-maximize", "keep-aspect"];
        if self.always_on_top { f.push("always-on-top"); }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_square_and_clamps() {
        let mut m = MiniPlayer::default();
        m.resize(80.0, 10.0); // horizontal drag wins
        assert_eq!(m.window_size(), (320.0, 320.0));
        m.resize(9999.0, 0.0);
        assert_eq!(m.side, MAX_SIDE);
        m.resize(-9999.0, 0.0);
        assert_eq!(m.side, MIN_SIDE);
    }

    #[test]
    fn style_names_round_trip_and_size() {
        for s in [MiniStyle::Bar, MiniStyle::Square, MiniStyle::Pill] {
            assert_eq!(MiniStyle::from_name(s.name()), s);
            let (w, h) = s.window_size();
            assert!(w > 0.0 && h > 0.0);
        }
        // Unknown + empty both land on the default rather than panicking.
        assert_eq!(MiniStyle::from_name(""), MiniStyle::Bar);
        assert_eq!(MiniStyle::from_name("nonsense"), MiniStyle::Bar);
        // The pill is the only class shorter than it is wide by 3:1 — the
        // collapsed one. Guards against a copy-paste swap of the tuples.
        let (pw, ph) = MiniStyle::Pill.window_size();
        assert!(pw > ph * 3.0);
        // Square is the only class taller than it is wide.
        let (sw, sh) = MiniStyle::Square.window_size();
        assert!(sh > sw);
        // A min is always under its base, or the window could not open at size.
        for s in [MiniStyle::Bar, MiniStyle::Square, MiniStyle::Pill] {
            let ((w, h), (mw, mh)) = (s.window_size(), s.min_size());
            assert!(mw < w && mh < h);
        }
    }

    #[test]
    fn pill_extension_rides_the_ratio_without_joining_it() {
        let s = MiniStyle::Pill;
        let (bw, bh) = s.base_size();
        let extra = PILL_CLUSTER;
        // Extended and left alone: width carries the cluster, height does not.
        let (w, h) = s.resize_locked_with_extra(bw + extra, 0.0, 0.0, extra);
        assert!((w - (bw + extra)).abs() < 1e-9, "extension changed on a no-op drag");
        assert!((h - bh).abs() < 1e-9, "the cluster leaked into the height");
        // Dragged out: the pill part keeps its ratio, the cluster scales with it.
        let (w2, h2) = s.resize_locked_with_extra(bw + extra, 60.0, 0.0, extra);
        let pill_w = w2 - extra * (h2 / bh);
        assert!((h2 / pill_w - bh / bw).abs() < 1e-9, "pill drifted off ratio");
        // Zero extension is the plain lock, exactly.
        for (dx, dy) in [(40.0, 3.0), (-25.0, 0.0), (0.0, 0.0)] {
            assert_eq!(s.resize_locked_with_extra(bw, dx, dy, 0.0), s.resize_locked(bw, dx, dy));
        }
    }

    #[test]
    fn opens_scaled_up_and_inside_the_resize_range() {
        for s in [MiniStyle::Bar, MiniStyle::Square, MiniStyle::Pill] {
            let (bw, bh) = s.base_size();
            let (w, h) = s.window_size();
            assert!((w - bw * DEFAULT_SCALE).abs() < 1e-9);
            assert!((h - bh * DEFAULT_SCALE).abs() < 1e-9);
            // The size it opens at must itself be draggable both ways, or the
            // grip would be dead in one direction from the moment it appears.
            let ((minw, _), (maxw, _)) = (s.min_size(), s.max_size());
            assert!(minw < w && w < maxw);
        }
    }

    #[test]
    fn corner_resize_holds_the_ratio_and_clamps() {
        for s in [MiniStyle::Bar, MiniStyle::Square, MiniStyle::Pill] {
            let (bw, bh) = s.window_size();
            let ratio = bh / bw;
            // Ratio holds whichever axis the cursor leads on, and either way.
            for (dx, dy) in [(60.0, 4.0), (4.0, 60.0), (-40.0, -2.0), (0.0, 0.0)] {
                let (w, h) = s.resize_locked(bw, dx, dy);
                assert!((h / w - ratio).abs() < 1e-9, "{s:?} drifted off ratio");
            }
            // Dragging in past the floor / out past the ceiling both stop.
            let close = |a: (f64, f64), b: (f64, f64)| {
                (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
            };
            assert!(close(s.resize_locked(bw, -9999.0, 0.0), s.min_size()));
            assert!(close(s.resize_locked(bw, 9999.0, 0.0), s.max_size()));
            // A width that drifted off ratio is pulled back onto it rather than
            // having the drift carried forward — height follows width, always.
            let (w, h) = s.resize_locked(bw + 40.0, 0.0, 0.0);
            assert!((h - w * ratio).abs() < 1e-9 && (w - bw - 40.0).abs() < 1e-9);
        }
    }

    #[test]
    fn flags_include_on_top_when_set() {
        let m = MiniPlayer::default();
        assert!(m.window_flags().contains(&"always-on-top"));
        let m2 = MiniPlayer { always_on_top: false, ..Default::default() };
        assert!(!m2.window_flags().contains(&"always-on-top"));
    }
}
