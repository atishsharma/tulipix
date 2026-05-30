//! `np.p4.tools.transcribe` — standalone audio/video → SRT via whisper.cpp.
//!
//! Auto-tiers the model by available RAM (so it works on a laptop and a
//! workstation alike) and builds the whisper.cpp argv. No Videos-section file
//! is required — any media path works. SRT timestamp formatting lives here too
//! since it's a common off-by-one source.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tier { Tiny, Base, Small, Medium, LargeV3 }

impl Tier {
    pub fn model_file(self) -> &'static str {
        match self {
            Tier::Tiny => "ggml-tiny.bin", Tier::Base => "ggml-base.bin",
            Tier::Small => "ggml-small.bin", Tier::Medium => "ggml-medium.bin",
            Tier::LargeV3 => "ggml-large-v3.bin",
        }
    }
}

/// Pick a model tier from available RAM (MB).
pub fn auto_tier(ram_mb: u64) -> Tier {
    match ram_mb {
        0..=2047 => Tier::Tiny,
        2048..=4095 => Tier::Base,
        4096..=8191 => Tier::Small,
        8192..=16383 => Tier::Medium,
        _ => Tier::LargeV3,
    }
}

/// whisper.cpp argv to transcribe `input` to SRT at `out` (without `.srt`).
pub fn args(model_path: &str, input: &str, out_prefix: &str, threads: u32) -> Vec<String> {
    vec![
        "-m".into(), model_path.into(),
        "-f".into(), input.into(),
        "-osrt".into(),
        "-of".into(), out_prefix.into(),
        "-t".into(), threads.max(1).to_string(),
    ]
}

/// Format milliseconds as an SRT timestamp `HH:MM:SS,mmm`.
pub fn srt_timestamp(ms: i64) -> String {
    let ms = ms.max(0);
    let (h, rem) = (ms / 3_600_000, ms % 3_600_000);
    let (m, rem) = (rem / 60_000, rem % 60_000);
    let (s, milli) = (rem / 1000, rem % 1000);
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_scales_with_ram() {
        assert_eq!(auto_tier(1024), Tier::Tiny);
        assert_eq!(auto_tier(6000), Tier::Small);
        assert_eq!(auto_tier(32000), Tier::LargeV3);
    }

    #[test]
    fn srt_format() {
        assert_eq!(srt_timestamp(3_661_123), "01:01:01,123");
        assert_eq!(srt_timestamp(0), "00:00:00,000");
    }

    #[test]
    fn argv_emits_srt() {
        let a = args("m.bin", "in.mkv", "out", 4);
        assert!(a.contains(&"-osrt".to_string()));
        assert!(a.windows(2).any(|w| w == ["-t", "4"]));
    }
}
