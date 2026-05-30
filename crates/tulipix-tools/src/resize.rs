//! `np.p4.tools.resize` — image resize batch.
//!
//! Distinct from compress (changes dimensions, keeps quality). Modes:
//! longest-edge cap, exact W×H, percentage, and fit-within a box. Computes the
//! target dimensions (preserving aspect where required) and the ffmpeg `scale`
//! filter.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResizeMode {
    LongestEdge(u32),
    Exact { w: u32, h: u32 },
    Percent(f64),
    FitWithin { w: u32, h: u32 },
}

/// Resolve target `(w, h)` for a source `(sw, sh)`.
pub fn target_dims(sw: u32, sh: u32, mode: ResizeMode) -> (u32, u32) {
    let (sw_f, sh_f) = (sw as f64, sh as f64);
    match mode {
        ResizeMode::Exact { w, h } => (w.max(1), h.max(1)),
        ResizeMode::Percent(p) => (((sw_f * p).round() as u32).max(1), ((sh_f * p).round() as u32).max(1)),
        ResizeMode::LongestEdge(edge) => {
            let scale = edge as f64 / sw_f.max(sh_f);
            if scale >= 1.0 { (sw, sh) } else { ((sw_f * scale).round() as u32, (sh_f * scale).round() as u32) }
        }
        ResizeMode::FitWithin { w, h } => {
            let scale = (w as f64 / sw_f).min(h as f64 / sh_f);
            if scale >= 1.0 { (sw, sh) } else { ((sw_f * scale).round() as u32, (sh_f * scale).round() as u32) }
        }
    }
}

/// ffmpeg `scale` filter for resolved dims.
pub fn scale_filter(w: u32, h: u32) -> String { format!("scale={w}:{h}") }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_edge_caps_landscape() {
        assert_eq!(target_dims(4000, 2000, ResizeMode::LongestEdge(1000)), (1000, 500));
        // upscale not applied
        assert_eq!(target_dims(500, 500, ResizeMode::LongestEdge(1000)), (500, 500));
    }

    #[test]
    fn fit_within_box() {
        // 4000x2000 into 1000x1000 → limited by width
        assert_eq!(target_dims(4000, 2000, ResizeMode::FitWithin { w: 1000, h: 1000 }), (1000, 500));
    }

    #[test]
    fn percent_and_exact() {
        assert_eq!(target_dims(1000, 800, ResizeMode::Percent(0.5)), (500, 400));
        assert_eq!(target_dims(1000, 800, ResizeMode::Exact { w: 320, h: 240 }), (320, 240));
        assert_eq!(scale_filter(320, 240), "scale=320:240");
    }
}
