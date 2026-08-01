//! `np.p4.music.player` — mpv audio path config (gapless, crossfade,
//! ReplayGain).
//!
//! The actual playback is an out-of-process mpv driven over its IPC socket
//! (tulipix_common::player); this module owns the option set we hand mpv
//! and the ReplayGain gain→volume math, both of which need to be exactly right
//! and are easy to unit-test in isolation.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ReplayGainMode { Off, Track, Album }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Gapless playback (mpv keeps the demuxer warm across tracks).
    pub gapless: bool,
    /// Crossfade duration in seconds; 0 disables.
    pub crossfade_s: f64,
    pub replaygain: ReplayGainMode,
    /// Extra pre-amp applied on top of RG, in dB.
    pub preamp_db: f64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { gapless: true, crossfade_s: 0.0, replaygain: ReplayGainMode::Off, preamp_db: 0.0 }
    }
}

impl AudioConfig {
    /// mpv `--key=value` options for this config (order-stable).
    pub fn mpv_options(&self) -> Vec<String> {
        let mut v = vec![
            format!("--gapless-audio={}", if self.gapless { "yes" } else { "no" }),
            format!("--replaygain={}", match self.replaygain {
                ReplayGainMode::Off => "no",
                ReplayGainMode::Track => "track",
                ReplayGainMode::Album => "album",
            }),
        ];
        if self.preamp_db != 0.0 {
            v.push(format!("--replaygain-preamp={}", self.preamp_db));
        }
        v
    }
}

/// dB gain → linear volume multiplier. Used to apply a tag's ReplayGain value
/// when mpv RG is off or the file has no RG tags but we computed one.
pub fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Effective volume multiplier for a track given its RG tag + config.
pub fn effective_gain(cfg: &AudioConfig, track_gain_db: Option<f64>, album_gain_db: Option<f64>) -> f64 {
    let rg = match cfg.replaygain {
        ReplayGainMode::Off => 0.0,
        ReplayGainMode::Track => track_gain_db.unwrap_or(0.0),
        ReplayGainMode::Album => album_gain_db.or(track_gain_db).unwrap_or(0.0),
    };
    db_to_linear(rg + cfg.preamp_db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_reflect_mode() {
        let cfg = AudioConfig { replaygain: ReplayGainMode::Album, gapless: true, ..Default::default() };
        let opts = cfg.mpv_options();
        assert!(opts.contains(&"--gapless-audio=yes".to_string()));
        assert!(opts.contains(&"--replaygain=album".to_string()));
    }

    #[test]
    fn db_zero_is_unity() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-9);
        assert!((db_to_linear(6.0) - 1.995).abs() < 0.01); // +6 dB ≈ 2×
    }

    #[test]
    fn album_mode_prefers_album_gain() {
        let cfg = AudioConfig { replaygain: ReplayGainMode::Album, ..Default::default() };
        let g = effective_gain(&cfg, Some(-3.0), Some(-6.0));
        assert!((g - db_to_linear(-6.0)).abs() < 1e-9);
    }

    #[test]
    fn off_mode_is_unity() {
        let cfg = AudioConfig::default();
        assert!((effective_gain(&cfg, Some(-9.0), None) - 1.0).abs() < 1e-9);
    }
}
