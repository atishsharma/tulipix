//! hwdec=auto-safe — pick the hardware decoder mpv should try, with a
//! software fallback banner reason.
//!
//! mpv exposes `hwdec=auto-safe` which already does the negotiation
//! internally; this module mirrors the decision tree so the UI can show a
//! "Falling back to software decode — codec/X not supported by your GPU"
//! banner *before* playback even starts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HwDecBackend {
    /// macOS hardware decode (Apple Silicon + Intel).
    VideoToolbox,
    /// Windows hardware decode.
    D3d11va,
    /// Linux NVIDIA path.
    Nvdec,
    /// Linux Intel/AMD (or VA-API capable GPU).
    Vaapi,
    /// Vulkan video decode — mpv 0.37+, GPU-driver gated.
    VulkanVideo,
    /// Software decode (no hwdec).
    None,
}

impl HwDecBackend {
    pub fn mpv_flag(self) -> &'static str {
        match self {
            Self::VideoToolbox => "videotoolbox",
            Self::D3d11va     => "d3d11va",
            Self::Nvdec       => "nvdec",
            Self::Vaapi       => "vaapi",
            Self::VulkanVideo => "vulkan",
            Self::None        => "no",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor { Apple, Nvidia, Amd, Intel, Unknown }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuProbe {
    pub vendor: Vendor,
    pub has_vulkan_video: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec { H264, Hevc, Av1, Vp9, Mpeg2, Other }

impl Codec {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "h264" | "avc" => Self::H264,
            "hevc" | "h265" => Self::Hevc,
            "av1" => Self::Av1,
            "vp9" => Self::Vp9,
            "mpeg2video" | "mpeg2" => Self::Mpeg2,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HwDecDecision {
    pub backend: HwDecBackend,
    pub reason: String,
    pub software_fallback: bool,
}

/// Pick the hwdec backend. Returns `HwDecBackend::None` with a human reason
/// when no accelerator handles the codec.
pub fn decide(probe: &GpuProbe, codec: Codec) -> HwDecDecision {
    // macOS — VideoToolbox covers everything mpv supports.
    if cfg!(target_os = "macos") {
        let supported = matches!(codec, Codec::H264 | Codec::Hevc | Codec::Av1 | Codec::Vp9 | Codec::Mpeg2);
        return if supported {
            HwDecDecision { backend: HwDecBackend::VideoToolbox, reason: "VideoToolbox handles this codec".into(), software_fallback: false }
        } else {
            HwDecDecision { backend: HwDecBackend::None, reason: format!("VideoToolbox does not handle {:?}", codec), software_fallback: true }
        };
    }

    // Vendor preference order on Linux/Windows.
    match probe.vendor {
        Vendor::Nvidia => {
            // NVDEC supports H264/HEVC/VP9/AV1 (RTX 40+ for AV1).
            match codec {
                Codec::H264 | Codec::Hevc | Codec::Vp9 => decision(HwDecBackend::Nvdec, "NVDEC supports this codec"),
                Codec::Av1 => decision(HwDecBackend::Nvdec, "NVDEC handles AV1 on RTX 40+ — auto-safe will downgrade otherwise"),
                _ => fallback(format!("NVDEC cannot handle {codec:?}")),
            }
        }
        Vendor::Amd | Vendor::Intel | Vendor::Unknown => {
            if cfg!(target_os = "windows") {
                match codec {
                    Codec::H264 | Codec::Hevc | Codec::Vp9 | Codec::Av1 => decision(HwDecBackend::D3d11va, "D3D11VA handles this codec"),
                    _ => fallback(format!("D3D11VA cannot handle {codec:?}")),
                }
            } else if probe.has_vulkan_video {
                decision(HwDecBackend::VulkanVideo, "Vulkan video decode available")
            } else {
                match codec {
                    Codec::H264 | Codec::Hevc | Codec::Vp9 | Codec::Av1 | Codec::Mpeg2 => decision(HwDecBackend::Vaapi, "VA-API supports this codec"),
                    _ => fallback(format!("VA-API cannot handle {codec:?}")),
                }
            }
        }
        Vendor::Apple => decision(HwDecBackend::VideoToolbox, "VideoToolbox"),
    }
}

fn decision(b: HwDecBackend, why: &str) -> HwDecDecision {
    HwDecDecision { backend: b, reason: why.into(), software_fallback: false }
}

fn fallback(why: String) -> HwDecDecision {
    HwDecDecision { backend: HwDecBackend::None, reason: why, software_fallback: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nvidia() -> GpuProbe { GpuProbe { vendor: Vendor::Nvidia, has_vulkan_video: false } }
    fn intel_vulkan() -> GpuProbe { GpuProbe { vendor: Vendor::Intel, has_vulkan_video: true } }

    #[test]
    fn codec_parse_known_names() {
        assert_eq!(Codec::parse("h264"), Codec::H264);
        assert_eq!(Codec::parse("HEVC"), Codec::Hevc);
        assert_eq!(Codec::parse("av1"), Codec::Av1);
        assert_eq!(Codec::parse("vp9"), Codec::Vp9);
        assert_eq!(Codec::parse("flv1"), Codec::Other);
    }

    #[test]
    fn nvidia_h264_picks_nvdec() {
        if cfg!(target_os = "macos") { return; }
        let d = decide(&nvidia(), Codec::H264);
        assert_eq!(d.backend, HwDecBackend::Nvdec);
        assert!(!d.software_fallback);
    }

    #[test]
    fn vulkan_video_preferred_when_available() {
        if cfg!(target_os = "macos") || cfg!(target_os = "windows") { return; }
        let d = decide(&intel_vulkan(), Codec::Hevc);
        assert_eq!(d.backend, HwDecBackend::VulkanVideo);
    }

    #[test]
    fn other_codec_falls_back_to_sw() {
        if cfg!(target_os = "macos") { return; }
        let d = decide(&nvidia(), Codec::Other);
        assert_eq!(d.backend, HwDecBackend::None);
        assert!(d.software_fallback);
        assert!(d.reason.to_lowercase().contains("nvdec"));
    }
}
