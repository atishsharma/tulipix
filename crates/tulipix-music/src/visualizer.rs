//! `np.p5.music.visualizer` — now-playing spectrum / waveform math.
//!
//! Pure helpers: the UI polls these and feeds whatever it has (real PCM when
//! available, otherwise a synthetic animation). No audio IO lives here.

pub const DEFAULT_BARS: usize = 24;

#[derive(Debug, Clone)]
pub struct Spectrum {
    /// Per-bar level, each clamped to 0.0..=1.0.
    pub bars: Vec<f32>,
}

impl Spectrum {
    pub fn new(n: usize) -> Self {
        Spectrum { bars: vec![0.0; n] }
    }

    /// Smooth toward `target`: rising bars move up by `attack` of the gap,
    /// falling bars move down by `decay` of the gap. Resizes to match target.
    pub fn update(&mut self, target: &[f32], attack: f32, decay: f32) {
        if self.bars.len() != target.len() {
            self.bars.resize(target.len(), 0.0);
        }
        for (cur, &t) in self.bars.iter_mut().zip(target) {
            let t = t.clamp(0.0, 1.0);
            let rate = if t > *cur { attack } else { decay };
            *cur = (*cur + (t - *cur) * rate.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        }
    }
}

/// Bucket `samples` into `n_bars` contiguous bands, take per-band RMS, and
/// normalize so the loudest band sits near 1.0. Empty input → zeros.
pub fn bars_from_samples(samples: &[f32], n_bars: usize) -> Vec<f32> {
    if n_bars == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![0.0; n_bars];
    }
    let mut bars = vec![0.0f32; n_bars];
    let per = (samples.len() as f32 / n_bars as f32).max(1.0);
    for (i, bar) in bars.iter_mut().enumerate() {
        let start = (i as f32 * per) as usize;
        let end = (((i + 1) as f32 * per) as usize).min(samples.len()).max(start + 1).min(samples.len());
        let slice = &samples[start..end];
        if slice.is_empty() {
            continue;
        }
        let sum_sq: f32 = slice.iter().map(|s| s * s).sum();
        *bar = (sum_sq / slice.len() as f32).sqrt();
    }
    let peak = bars.iter().cloned().fold(0.0f32, f32::max);
    if peak > f32::EPSILON {
        for b in &mut bars {
            *b = (*b / peak).clamp(0.0, 1.0);
        }
    }
    bars
}

/// Deterministic animated fallback when no PCM is available — gives the UI a
/// lively always-on visualizer keyed on elapsed time.
pub fn synthetic_bars(n_bars: usize, t_seconds: f64) -> Vec<f32> {
    (0..n_bars)
        .map(|i| {
            let phase = i as f64 * 0.6;
            let a = (t_seconds * 3.1 + phase).sin();
            let b = (t_seconds * 1.7 - phase * 0.5).sin();
            (0.5 + 0.30 * a + 0.18 * b).clamp(0.0, 1.0) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_flat() {
        let b = bars_from_samples(&[0.0; 1000], DEFAULT_BARS);
        assert_eq!(b.len(), DEFAULT_BARS);
        assert!(b.iter().all(|&x| x.abs() < 1e-6));
    }

    #[test]
    fn loud_signal_present_and_clamped() {
        let sig: Vec<f32> = (0..1000).map(|i| if i % 2 == 0 { 0.8 } else { -0.8 }).collect();
        let b = bars_from_samples(&sig, 8);
        assert_eq!(b.len(), 8);
        assert!(b.iter().all(|&x| (0.0..=1.0).contains(&x)));
        assert!(b.iter().cloned().fold(0.0f32, f32::max) > 0.9);
    }

    #[test]
    fn empty_input_zeros() {
        assert_eq!(bars_from_samples(&[], 5), vec![0.0; 5]);
    }

    #[test]
    fn update_rises_and_clamps() {
        let mut s = Spectrum::new(3);
        s.update(&[1.0, 1.0, 1.0], 0.5, 0.5);
        assert!(s.bars.iter().all(|&x| x > 0.0 && x <= 1.0));
        s.update(&[2.0, 2.0, 2.0], 1.0, 1.0); // target clamped to 1.0
        assert!(s.bars.iter().all(|&x| (x - 1.0).abs() < 1e-6));
    }

    #[test]
    fn synthetic_in_range() {
        let b = synthetic_bars(DEFAULT_BARS, 4.2);
        assert_eq!(b.len(), DEFAULT_BARS);
        assert!(b.iter().all(|&x| (0.0..=1.0).contains(&x)));
    }
}
