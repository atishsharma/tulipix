//! AV1 hardware decode + encode picker.
//!
//! Decode side is mpv's job — we just probe the host and tell the transcoder +
//! Tools UI which codec to advertise. Encode side picks the best AV1 encoder
//! available:
//!
//!   1. Intel QSV  — `av1_qsv`           (Arc & 11th-gen iGPU+)
//!   2. NVENC      — `av1_nvenc`         (RTX 40+)
//!   3. AMD AMF    — `av1_amf`           (RDNA3+)
//!   4. SVT-AV1    — `libsvtav1`         (CPU, fast)
//!   5. libaom     — `libaom-av1`        (CPU, slow fallback)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Av1Encoder {
    QsvAv1,
    NvencAv1,
    AmfAv1,
    SvtAv1,
    LibAomAv1,
}

impl Av1Encoder {
    pub fn ffmpeg_codec(self) -> &'static str {
        match self {
            Av1Encoder::QsvAv1 => "av1_qsv",
            Av1Encoder::NvencAv1 => "av1_nvenc",
            Av1Encoder::AmfAv1 => "av1_amf",
            Av1Encoder::SvtAv1 => "libsvtav1",
            Av1Encoder::LibAomAv1 => "libaom-av1",
        }
    }
    pub fn is_hardware(self) -> bool {
        matches!(self, Av1Encoder::QsvAv1 | Av1Encoder::NvencAv1 | Av1Encoder::AmfAv1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Av1Decoder {
    /// VAAPI / D3D11VA / VideoToolbox handle the lifting transparently.
    HwIntel,
    HwNvidia,
    HwAmd,
    HwApple,
    Dav1d, // CPU fallback baked into libmpv
}

impl Av1Decoder {
    pub fn label(self) -> &'static str {
        match self {
            Av1Decoder::HwIntel => "intel",
            Av1Decoder::HwNvidia => "nvidia",
            Av1Decoder::HwAmd => "amd",
            Av1Decoder::HwApple => "apple",
            Av1Decoder::Dav1d => "dav1d",
        }
    }
    pub fn is_hardware(self) -> bool {
        !matches!(self, Av1Decoder::Dav1d)
    }
}

/// What the host advertises — usually pieced together from libmpv's
/// `codec_info` plus ffmpeg's `-encoders` output. The picker takes this as
/// input so unit tests can synthesise weird machines (Nvidia + Intel, Apple
/// with no GPU AV1, etc.) without poking the real environment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostAv1Caps {
    pub intel_av1_decode: bool,
    pub nvidia_av1_decode: bool,
    pub amd_av1_decode: bool,
    pub apple_av1_decode: bool,
    pub intel_av1_encode: bool,
    pub nvidia_av1_encode: bool,
    pub amd_av1_encode: bool,
    pub has_svt_av1: bool,
    pub has_libaom_av1: bool,
}

pub fn pick_decoder(caps: &HostAv1Caps) -> Av1Decoder {
    if caps.intel_av1_decode {
        Av1Decoder::HwIntel
    } else if caps.nvidia_av1_decode {
        Av1Decoder::HwNvidia
    } else if caps.amd_av1_decode {
        Av1Decoder::HwAmd
    } else if caps.apple_av1_decode {
        Av1Decoder::HwApple
    } else {
        Av1Decoder::Dav1d
    }
}

/// Returns `None` when neither hardware NOR a CPU AV1 encoder is available
/// — caller falls back to H.265.
pub fn pick_encoder(caps: &HostAv1Caps) -> Option<Av1Encoder> {
    if caps.intel_av1_encode {
        Some(Av1Encoder::QsvAv1)
    } else if caps.nvidia_av1_encode {
        Some(Av1Encoder::NvencAv1)
    } else if caps.amd_av1_encode {
        Some(Av1Encoder::AmfAv1)
    } else if caps.has_svt_av1 {
        Some(Av1Encoder::SvtAv1)
    } else if caps.has_libaom_av1 {
        Some(Av1Encoder::LibAomAv1)
    } else {
        None
    }
}

/// Quality preset for the user-facing dropdown. Each preset maps to a CRF
/// value tuned per encoder so the same "Balanced" pick yields comparable
/// bitrate across encoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preset {
    Highest,
    Balanced,
    Smallest,
}

pub fn crf_for(encoder: Av1Encoder, preset: Preset) -> i32 {
    match (encoder, preset) {
        (Av1Encoder::SvtAv1, Preset::Highest) => 22,
        (Av1Encoder::SvtAv1, Preset::Balanced) => 30,
        (Av1Encoder::SvtAv1, Preset::Smallest) => 40,
        (Av1Encoder::LibAomAv1, Preset::Highest) => 24,
        (Av1Encoder::LibAomAv1, Preset::Balanced) => 32,
        (Av1Encoder::LibAomAv1, Preset::Smallest) => 42,
        // Hardware encoders use rate-control quality (RCQ) on a 0..51 scale
        // similar to x264 — values picked to match the CPU defaults' bitrate.
        (Av1Encoder::QsvAv1, Preset::Highest) => 24,
        (Av1Encoder::QsvAv1, Preset::Balanced) => 30,
        (Av1Encoder::QsvAv1, Preset::Smallest) => 40,
        (Av1Encoder::NvencAv1, Preset::Highest) => 24,
        (Av1Encoder::NvencAv1, Preset::Balanced) => 30,
        (Av1Encoder::NvencAv1, Preset::Smallest) => 40,
        (Av1Encoder::AmfAv1, Preset::Highest) => 24,
        (Av1Encoder::AmfAv1, Preset::Balanced) => 30,
        (Av1Encoder::AmfAv1, Preset::Smallest) => 40,
    }
}

/// Build the ffmpeg `-c:v <codec> [-crf N | -global_quality N] -preset M`
/// argument tail. Caller composes the input + output args around it.
pub fn ffmpeg_args(encoder: Av1Encoder, preset: Preset) -> Vec<String> {
    let crf = crf_for(encoder, preset);
    let mut args = vec!["-c:v".to_string(), encoder.ffmpeg_codec().to_string()];
    if encoder.is_hardware() {
        args.extend(["-global_quality".to_string(), crf.to_string()]);
    } else {
        args.extend(["-crf".to_string(), crf.to_string()]);
        let speed = match encoder {
            Av1Encoder::SvtAv1 => match preset {
                Preset::Highest => "4",
                Preset::Balanced => "8",
                Preset::Smallest => "10",
            },
            Av1Encoder::LibAomAv1 => match preset {
                Preset::Highest => "4",
                Preset::Balanced => "6",
                Preset::Smallest => "8",
            },
            _ => unreachable!(),
        };
        args.extend(["-preset".to_string(), speed.to_string()]);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_decoder_prefers_intel() {
        let mut caps = HostAv1Caps {
            intel_av1_decode: true,
            nvidia_av1_decode: true,
            ..Default::default()
        };
        assert_eq!(pick_decoder(&caps), Av1Decoder::HwIntel);
        caps.intel_av1_decode = false;
        assert_eq!(pick_decoder(&caps), Av1Decoder::HwNvidia);
        caps.nvidia_av1_decode = false;
        assert_eq!(pick_decoder(&caps), Av1Decoder::Dav1d);
    }

    #[test]
    fn pick_encoder_hardware_first_then_cpu() {
        let caps = HostAv1Caps {
            intel_av1_encode: false,
            nvidia_av1_encode: true,
            has_svt_av1: true,
            ..Default::default()
        };
        assert_eq!(pick_encoder(&caps), Some(Av1Encoder::NvencAv1));
        let cpu_only = HostAv1Caps {
            has_svt_av1: true,
            ..Default::default()
        };
        assert_eq!(pick_encoder(&cpu_only), Some(Av1Encoder::SvtAv1));
    }

    #[test]
    fn pick_encoder_no_av1_returns_none() {
        let caps = HostAv1Caps::default();
        assert!(pick_encoder(&caps).is_none());
    }

    #[test]
    fn ffmpeg_args_cpu_uses_crf_and_preset() {
        let args = ffmpeg_args(Av1Encoder::SvtAv1, Preset::Balanced);
        assert_eq!(args[0], "-c:v");
        assert_eq!(args[1], "libsvtav1");
        assert!(args.iter().any(|a| a == "-crf"));
        assert!(args.iter().any(|a| a == "-preset"));
    }

    #[test]
    fn ffmpeg_args_hw_uses_global_quality_not_preset() {
        let args = ffmpeg_args(Av1Encoder::NvencAv1, Preset::Highest);
        assert!(args.iter().any(|a| a == "-global_quality"));
        assert!(!args.iter().any(|a| a == "-preset"));
    }

    #[test]
    fn crf_monotonic_balanced_between_high_and_small() {
        for enc in [
            Av1Encoder::SvtAv1,
            Av1Encoder::LibAomAv1,
            Av1Encoder::QsvAv1,
            Av1Encoder::NvencAv1,
            Av1Encoder::AmfAv1,
        ] {
            let h = crf_for(enc, Preset::Highest);
            let b = crf_for(enc, Preset::Balanced);
            let s = crf_for(enc, Preset::Smallest);
            assert!(h < b && b < s, "crf must increase with smaller-target for {enc:?}");
        }
    }

    #[test]
    fn hardware_flag_round_trip() {
        assert!(Av1Encoder::QsvAv1.is_hardware());
        assert!(Av1Encoder::NvencAv1.is_hardware());
        assert!(!Av1Encoder::SvtAv1.is_hardware());
        assert!(Av1Decoder::HwIntel.is_hardware());
        assert!(!Av1Decoder::Dav1d.is_hardware());
    }
}
