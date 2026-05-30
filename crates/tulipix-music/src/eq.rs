//! `np.p4.music.eq` — 10-band equalizer via mpv's `anequalizer` filter.
//!
//! Owns the ISO 10-band center frequencies, named presets, gain clamping
//! (±12 dB), and the `af` filter-graph string handed to libmpv.

use serde::{Deserialize, Serialize};

/// ISO standard 10-band center frequencies (Hz).
pub const BANDS_HZ: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
pub const MAX_GAIN_DB: f64 = 12.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Equalizer {
    /// Per-band gains in dB, index-aligned to [`BANDS_HZ`].
    pub gains_db: [f64; 10],
    pub enabled: bool,
}

impl Default for Equalizer {
    fn default() -> Self { Self { gains_db: [0.0; 10], enabled: false } }
}

impl Equalizer {
    pub fn flat() -> Self { Self::default() }

    pub fn preset(name: &str) -> Option<Self> {
        let g = match name.to_ascii_lowercase().as_str() {
            "flat"   => [0.0; 10],
            "rock"   => [4.0, 3.0, 1.5, 0.0, -1.0, -1.0, 1.5, 3.0, 4.0, 4.5],
            "pop"    => [-1.0, 0.0, 2.0, 3.0, 3.5, 2.5, 0.0, -1.0, -1.0, -1.5],
            "jazz"   => [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.5, 3.0],
            "bass"   => [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "treble" => [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.5, 6.0],
            _ => return None,
        };
        Some(Self { gains_db: g, enabled: true })
    }

    /// Clamp every band into ±[`MAX_GAIN_DB`].
    pub fn clamp(&mut self) {
        for g in self.gains_db.iter_mut() { *g = g.clamp(-MAX_GAIN_DB, MAX_GAIN_DB); }
    }

    /// mpv `af` filter-graph value. Empty string when disabled / flat → caller
    /// should clear the filter.
    pub fn mpv_af(&self) -> String {
        if !self.enabled || self.gains_db.iter().all(|g| *g == 0.0) { return String::new(); }
        let chans = BANDS_HZ.iter().zip(self.gains_db.iter())
            .map(|(f, g)| format!("c0 f={f} w={width} g={g}|c1 f={f} w={width} g={g}", width = (*f as f64 * 0.7) as u32, f = f, g = g))
            .collect::<Vec<_>>().join("|");
        // mpv's option parser treats `|`/`=`/space specially; length-quote the
        // whole params value (`%N%…`) so it survives `--af=` and IPC alike.
        format!("anequalizer=params=%{}%{}", chans.len(), chans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_emits_no_filter() {
        assert_eq!(Equalizer::flat().mpv_af(), "");
    }

    #[test]
    fn preset_lookup() {
        assert!(Equalizer::preset("rock").is_some());
        assert!(Equalizer::preset("nope").is_none());
        let af = Equalizer::preset("bass").unwrap().mpv_af();
        assert!(af.starts_with("anequalizer="));
        assert!(af.contains("f=31"));
    }

    #[test]
    fn clamp_caps_gain() {
        let mut eq = Equalizer { gains_db: [99.0; 10], enabled: true };
        eq.clamp();
        assert!(eq.gains_db.iter().all(|g| *g == MAX_GAIN_DB));
    }
}
