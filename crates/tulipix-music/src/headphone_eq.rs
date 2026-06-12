//! `np.p4.music.headphone-eq` — AutoEq presets, auto-detected by OS audio
//! device name.
//!
//! Parses AutoEq's ParametricEQ text format into preamp + peaking filters and
//! matches the connected output device's name against the preset catalogue by
//! token overlap (so "Sony WH-1000XM4 Hands-Free" still finds "Sony WH-1000XM4").

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

#[cfg(test)]
mod tests {
    use super::*;

    const AEQ: &str = "Preamp: -6.5 dB\nFilter 1: ON PK Fc 21 Hz Gain 6.7 dB Q 0.7\nFilter 2: ON PK Fc 105 Hz Gain -1.5 dB Q 1.0\n";

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
