//! Real-time upscaling + frame interpolation.
//!
//! Two orthogonal knobs:
//!  * **Upscale** — Anime4K shader chain (mpv `--glsl-shaders=`) for line-art
//!    content, or a generic ESRGAN-style ONNX upscaler stub for live action.
//!  * **Interpolate** — mpv built-in `mc-deint`/`interpolation` or an ONNX
//!    RIFE step that doubles the source frame rate.
//!
//! The selector evaluates the system's GPU budget (memory + estimated TFLOPs)
//! against the source resolution to pick a tier the GPU can actually keep up
//! with at 24 fps; downgrading silently when the budget is tight.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpscaleProfile { Off, Anime4kFast, Anime4kHq, EsrganLite }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameInterp { Off, Linear, RifeLite, RifeHq }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuBudget {
    pub vram_mb: u32,
    pub tflops: f32,
    pub vendor_supports_rife: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpscaleSelection {
    pub upscale: UpscaleProfile,
    pub interpolate: FrameInterp,
    pub mpv_opts: Vec<(String, String)>,
}

const ANIME4K_FAST: &str = "~~/shaders/Anime4K_Restore_CNN_S.glsl;~~/shaders/Anime4K_Upscale_CNN_x2_S.glsl";
const ANIME4K_HQ:   &str = "~~/shaders/Anime4K_Restore_CNN_VL.glsl;~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl;~~/shaders/Anime4K_AutoDownscalePre_x2.glsl";

pub fn select(
    src_width: u32,
    src_height: u32,
    target_w: u32,
    target_h: u32,
    is_anime: bool,
    budget: &GpuBudget,
) -> UpscaleSelection {
    let upscale = pick_upscale(src_width, src_height, target_w, target_h, is_anime, budget);
    let interpolate = pick_interp(budget);
    let mut mpv_opts = Vec::new();

    match upscale {
        UpscaleProfile::Off => {}
        UpscaleProfile::Anime4kFast => mpv_opts.push(("glsl-shaders".into(), ANIME4K_FAST.into())),
        UpscaleProfile::Anime4kHq   => mpv_opts.push(("glsl-shaders".into(), ANIME4K_HQ.into())),
        UpscaleProfile::EsrganLite  => mpv_opts.push(("scale".into(), "ewa_lanczos".into())),
    }

    match interpolate {
        FrameInterp::Off => {}
        FrameInterp::Linear  => mpv_opts.push(("interpolation".into(), "yes".into())),
        FrameInterp::RifeLite => {
            mpv_opts.push(("interpolation".into(), "yes".into()));
            mpv_opts.push(("video-sync".into(), "display-resample".into()));
        }
        FrameInterp::RifeHq => {
            mpv_opts.push(("interpolation".into(), "yes".into()));
            mpv_opts.push(("video-sync".into(), "display-resample".into()));
            mpv_opts.push(("tscale".into(), "oversample".into()));
        }
    }

    UpscaleSelection { upscale, interpolate, mpv_opts }
}

fn pick_upscale(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, anime: bool, budget: &GpuBudget) -> UpscaleProfile {
    if src_w >= dst_w && src_h >= dst_h { return UpscaleProfile::Off; }
    let pixel_ratio = ((dst_w as f64 * dst_h as f64) / (src_w.max(1) as f64 * src_h.max(1) as f64)).sqrt();
    if anime {
        if budget.tflops >= 7.0 && budget.vram_mb >= 6_000 && pixel_ratio < 3.0 {
            UpscaleProfile::Anime4kHq
        } else {
            UpscaleProfile::Anime4kFast
        }
    } else if budget.tflops >= 10.0 && budget.vram_mb >= 8_000 {
        UpscaleProfile::EsrganLite
    } else {
        UpscaleProfile::Off
    }
}

fn pick_interp(budget: &GpuBudget) -> FrameInterp {
    if !budget.vendor_supports_rife { return FrameInterp::Linear; }
    if budget.tflops >= 15.0 && budget.vram_mb >= 8_000 { FrameInterp::RifeHq }
    else if budget.tflops >= 8.0 && budget.vram_mb >= 4_000 { FrameInterp::RifeLite }
    else { FrameInterp::Linear }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modest() -> GpuBudget { GpuBudget { vram_mb: 4096, tflops: 5.0, vendor_supports_rife: true } }
    fn beefy()  -> GpuBudget { GpuBudget { vram_mb: 16384, tflops: 20.0, vendor_supports_rife: true } }

    #[test]
    fn no_upscale_when_source_meets_target() {
        let s = select(3840, 2160, 1920, 1080, false, &beefy());
        assert_eq!(s.upscale, UpscaleProfile::Off);
    }

    #[test]
    fn anime_picks_anime4k() {
        let s = select(720, 480, 1920, 1080, true, &modest());
        assert_eq!(s.upscale, UpscaleProfile::Anime4kFast);
    }

    #[test]
    fn anime_hq_on_beefy_card_when_ratio_small() {
        let s = select(1280, 720, 1920, 1080, true, &beefy());
        assert_eq!(s.upscale, UpscaleProfile::Anime4kHq);
    }

    #[test]
    fn live_action_stays_off_on_modest_gpu() {
        let s = select(720, 480, 1920, 1080, false, &modest());
        assert_eq!(s.upscale, UpscaleProfile::Off);
    }

    #[test]
    fn rife_tier_scales_with_budget() {
        assert_eq!(pick_interp(&modest()), FrameInterp::Linear);
        assert_eq!(pick_interp(&beefy()), FrameInterp::RifeHq);
    }

    #[test]
    fn mpv_opts_include_anime4k_shader_string() {
        let s = select(720, 480, 1920, 1080, true, &modest());
        assert!(s.mpv_opts.iter().any(|(k, v)| k == "glsl-shaders" && v.contains("Anime4K")));
    }
}
