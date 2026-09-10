//! `np.p4.music.headphone-eq` — AutoEq presets, auto-detected by OS audio
//! device name.
//!
//! Parses AutoEq's ParametricEQ text format into preamp + peaking filters and
//! matches the connected output device's name against the preset catalogue by
//! token overlap (so "Sony WH-1000XM4 Hands-Free" still finds "Sony WH-1000XM4").
//!
//! [`match_device`] wants a catalogue this tree does not ship — AutoEq is
//! thousands of files, and bundling them is a decision about the installer, not
//! about music. So the way in is the other one: point at a `ParametricEQ.txt`
//! you downloaded yourself, and [`to_bands`] flattens its filters onto the ten
//! bands the app's equalizer actually has. Auto-detection can come later
//! without changing any of this.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeakFilter {
    pub fc_hz: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParametricEq {
    pub preamp_db: f64,
    pub filters: Vec<PeakFilter>,
}

/// Parse AutoEq `ParametricEQ.txt` content.
pub fn parse_autoeq(text: &str) -> ParametricEq {
    let mut eq = ParametricEq::default();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Preamp:") {
            eq.preamp_db = rest.trim().trim_end_matches("dB").trim().parse().unwrap_or(0.0);
        } else if l.starts_with("Filter") && l.contains("PK") {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let grab = |key: &str| toks.iter().position(|t| *t == key).and_then(|i| toks.get(i + 1)).and_then(|v| v.parse::<f64>().ok());
            if let (Some(fc), Some(gain), Some(q)) = (grab("Fc"), grab("Gain"), grab("Q")) {
                eq.filters.push(PeakFilter { fc_hz: fc, gain_db: gain, q });
            }
        }
    }
    eq
}

fn tokens(s: &str) -> Vec<String> {
    s.to_ascii_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| t.to_string()).collect()
}

/// Best-matching preset key for an OS device name, by shared-token count.
/// Returns `None` if nothing shares a meaningful token.
pub fn match_device<'a>(device_name: &str, preset_keys: &'a [String]) -> Option<&'a String> {
    let dev = tokens(device_name);
    let mut best: Option<(&String, usize)> = None;
    for key in preset_keys {
        let kt = tokens(key);
        let overlap = kt.iter().filter(|t| dev.contains(t)).count();
        if overlap > 0 && best.is_none_or(|(_, n)| overlap > n) {
            best = Some((key, overlap));
        }
    }
    best.map(|(k, _)| k)
}

/// The sample rate the response is evaluated at. Any rate gives the same
/// answer well below Nyquist, and every band we sample is.
const FS: f64 = 48_000.0;

/// Gain in dB that one RBJ peaking filter contributes at `f_hz`.
///
/// The textbook biquad, evaluated on the unit circle rather than approximated:
/// a Gaussian bump in log-frequency is close but drifts at high Q, which is
/// exactly where AutoEq's corrective filters live.
fn peak_db(p: &PeakFilter, f_hz: f64) -> f64 {
    let a = 10.0_f64.powf(p.gain_db / 40.0);
    let w0 = std::f64::consts::TAU * p.fc_hz / FS;
    let alpha = w0.sin() / (2.0 * p.q.max(1e-6));
    let (b0, b1, b2) = (1.0 + alpha * a, -2.0 * w0.cos(), 1.0 - alpha * a);
    let (a0, a1, a2) = (1.0 + alpha / a, -2.0 * w0.cos(), 1.0 - alpha / a);

    let w = std::f64::consts::TAU * f_hz / FS;
    let (c1, s1) = (w.cos(), w.sin());
    let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
    let num = ((b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2)).sqrt();
    let den = ((a0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2)).sqrt();
    if den <= f64::EPSILON {
        return 0.0;
    }
    20.0 * (num / den).log10()
}

/// The whole curve's gain in dB at `f_hz`. Filters cascade, so their
/// magnitudes multiply and their decibels add.
pub fn response_db(eq: &ParametricEq, f_hz: f64) -> f64 {
    eq.filters.iter().map(|p| peak_db(p, f_hz)).sum()
}

/// Flatten a parametric curve onto fixed band centres, e.g. `eq::BANDS_HZ`.
///
/// This is a lossy fit and says so: a ten-band graphic equalizer cannot hold a
/// twelve-filter correction with a Q of 4 on it. Sampling the real response at
/// each centre keeps the broad tilt — which is most of what a headphone
/// correction is — and loses the narrow notches. Better than not having it, and
/// not to be confused with running the filters themselves.
///
/// The preamp rides along: AutoEq sets it to stop the boosted bands clipping,
/// and dropping it would give the curve without the headroom it needs.
pub fn to_bands(eq: &ParametricEq, centres_hz: &[u32]) -> Vec<f64> {
    centres_hz
        .iter()
        .map(|f| eq.preamp_db + response_db(eq, *f as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AEQ: &str = "Preamp: -6.5 dB\nFilter 1: ON PK Fc 21 Hz Gain 6.7 dB Q 0.7\nFilter 2: ON PK Fc 105 Hz Gain -1.5 dB Q 1.0\n";

    const CENTRES: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

    #[test]
    fn a_peak_is_its_own_gain_at_its_own_centre() {
        let p = PeakFilter { fc_hz: 1000.0, gain_db: 6.0, q: 1.0 };
        assert!((peak_db(&p, 1000.0) - 6.0).abs() < 0.01, "{}", peak_db(&p, 1000.0));
    }

    #[test]
    fn a_peak_is_nothing_three_decades_away() {
        let p = PeakFilter { fc_hz: 1000.0, gain_db: 6.0, q: 1.0 };
        assert!(peak_db(&p, 31.0).abs() < 0.1);
        assert!(peak_db(&p, 16_000.0).abs() < 0.2);
    }

    #[test]
    fn a_cut_reads_as_a_cut() {
        let p = PeakFilter { fc_hz: 4000.0, gain_db: -8.0, q: 2.0 };
        assert!((peak_db(&p, 4000.0) + 8.0).abs() < 0.01);
    }

    #[test]
    fn filters_add_in_decibels() {
        let eq = ParametricEq {
            preamp_db: 0.0,
            filters: vec![
                PeakFilter { fc_hz: 1000.0, gain_db: 3.0, q: 1.0 },
                PeakFilter { fc_hz: 1000.0, gain_db: 2.0, q: 1.0 },
            ],
        };
        assert!((response_db(&eq, 1000.0) - 5.0).abs() < 0.02);
    }

    #[test]
    fn bands_carry_the_preamp_and_the_shape() {
        let eq = parse_autoeq(AEQ);
        let bands = to_bands(&eq, &CENTRES);
        assert_eq!(bands.len(), 10);
        // The 21 Hz boost reaches 31 Hz; the 105 Hz cut lands near 125 Hz.
        assert!(bands[0] > bands[2], "low shelf should sit above the dip");
        // Nothing in this preset touches 16 kHz, so it is the preamp alone.
        assert!((bands[9] + 6.5).abs() < 0.2, "{}", bands[9]);
    }

    #[test]
    fn parses_preamp_and_filters() {
        let eq = parse_autoeq(AEQ);
        assert!((eq.preamp_db + 6.5).abs() < 1e-9);
        assert_eq!(eq.filters.len(), 2);
        assert_eq!(eq.filters[0].fc_hz, 21.0);
        assert!((eq.filters[1].gain_db + 1.5).abs() < 1e-9);
    }

    #[test]
    fn device_match_by_tokens() {
        let keys = vec!["Sony WH-1000XM4".to_string(), "Sennheiser HD 600".to_string()];
        assert_eq!(match_device("Sony WH-1000XM4 Hands-Free AG", &keys), Some(&keys[0]));
        assert!(match_device("Generic USB Audio", &keys).is_none());
    }
}
