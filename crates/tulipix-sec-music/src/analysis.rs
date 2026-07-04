//! `np.p4.music.bpm-key` + `np.p4.music.dr-meter` — local audio analysis.
//!
//! Decodes a track to mono f32 PCM via the bundled ffmpeg (`-f f32le`), then
//! runs three pure-Rust passes over the samples:
//!
//! * **BPM** — spectral-energy flux onset envelope, autocorrelated over the
//!   60–200 BPM lag range (octave-error corrected toward 80–160).
//! * **Musical key** — 36-bin Goertzel chromagram (C3..B5) folded to 12 pitch
//!   classes, correlated against the Krumhansl–Schmuckler major/minor
//!   profiles; the winner maps to `key_label` + Camelot via `bpm_key`.
//! * **DR** — 3-second block RMS + track peak through `dr_meter::dr_score`.
//!
//! Everything is stored on `track_meta` (`bpm`, `music_key`, `dr_score`) via
//! the tulipix-music modules, so results survive file moves (proxy model).

use tulipix_core::proc::NoWindow;

const SR: u32 = 22_050;
/// Analyze at most this many seconds (from 30s in, to skip intros) — bounds
/// both ffmpeg decode time and memory (~120s × 22050 × 4B ≈ 10.6 MB).
const MAX_S: u32 = 120;

/// Decode `path` to mono f32 at [`SR`]; `None` when ffmpeg fails/missing.
pub fn decode_mono(path: &std::path::Path) -> Option<Vec<f32>> {
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let mut cmd = std::process::Command::new(ff);
    cmd.no_window();
    let out = cmd
        .arg("-v").arg("error")
        .arg("-ss").arg("30").arg("-t").arg(MAX_S.to_string())
        .arg("-i").arg(path)
        .arg("-ac").arg("1").arg("-ar").arg(SR.to_string())
        .arg("-f").arg("f32le").arg("pipe:1")
        .output().ok()?;
    if out.stdout.len() < 4 * SR as usize {
        // Under a second decoded from 30s in — short track; retry from 0.
        let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
        let mut cmd = std::process::Command::new(ff);
        cmd.no_window();
        let out2 = cmd
            .arg("-v").arg("error").arg("-t").arg(MAX_S.to_string())
            .arg("-i").arg(path)
            .arg("-ac").arg("1").arg("-ar").arg(SR.to_string())
            .arg("-f").arg("f32le").arg("pipe:1")
            .output().ok()?;
        return Some(bytes_to_f32(&out2.stdout));
    }
    Some(bytes_to_f32(&out.stdout))
}

fn bytes_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// Onset envelope: per-hop RMS energy, half-wave-rectified first difference.
fn onset_flux(samples: &[f32], frame: usize, hop: usize) -> Vec<f32> {
    let mut energies = Vec::with_capacity(samples.len() / hop);
    let mut i = 0;
    while i + frame <= samples.len() {
        let e: f32 = samples[i..i + frame].iter().map(|s| s * s).sum::<f32>() / frame as f32;
        energies.push(e.sqrt());
        i += hop;
    }
    let mut flux = vec![0.0f32; energies.len()];
    for i in 1..energies.len() {
        flux[i] = (energies[i] - energies[i - 1]).max(0.0);
    }
    flux
}

/// BPM via autocorrelation of the onset envelope over the 60–200 BPM range.
pub fn detect_bpm(samples: &[f32]) -> Option<f64> {
    const FRAME: usize = 1024;
    const HOP: usize = 512;
    let flux = onset_flux(samples, FRAME, HOP);
    if flux.len() < 64 { return None; }
    let hop_s = HOP as f64 / SR as f64;
    // Lag bounds for 200..60 BPM.
    let min_lag = (60.0 / 200.0 / hop_s) as usize;
    let max_lag = ((60.0 / 60.0) / hop_s) as usize;
    if max_lag >= flux.len() { return None; }
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let f: Vec<f32> = flux.iter().map(|v| v - mean).collect();
    let mut best = (0usize, f32::MIN);
    for lag in min_lag..=max_lag {
        let mut acc = 0.0f32;
        for i in lag..f.len() { acc += f[i] * f[i - lag]; }
        // Slight preference for shorter lags (higher BPM) to counter the
        // autocorrelation's natural bias toward multiples.
        let score = acc / (f.len() - lag) as f32 * (1.0 + 0.1 * (max_lag - lag) as f32 / max_lag as f32);
        if score > best.1 { best = (lag, score); }
    }
    if best.1 <= 0.0 { return None; }
    let mut bpm = 60.0 / (best.0 as f64 * hop_s);
    // Octave-error fold into the common 80..160 window.
    while bpm >= 160.0 { bpm /= 2.0; }
    while bpm < 80.0 { bpm *= 2.0; }
    Some((bpm * 10.0).round() / 10.0)
}

/// Goertzel power of frequency `f_hz` over `s`.
fn goertzel(s: &[f32], f_hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f_hz / SR as f64;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in s {
        let s0 = x as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Krumhansl–Schmuckler key profiles (major / minor), C-rooted.
const KS_MAJ: [f64; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const KS_MIN: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn pearson(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da * db).sqrt() }
}

/// Peak-normalized 12-bin chromagram over 4-second windows (hop 2 s, ≤15
/// windows) — shared by key detection and the sonic embedding.
fn chroma12(samples: &[f32]) -> Option<[f64; 12]> {
    if samples.len() < SR as usize { return None; }
    let win = 4 * SR as usize;
    let hop = 2 * SR as usize;
    let mut chroma = [0.0f64; 12];
    let mut windows = 0;
    let mut i = 0;
    while i + win <= samples.len() && windows < 15 {
        let seg = &samples[i..i + win];
        for pc in 0..12u32 {
            for oct in 0..3u32 {
                // MIDI 48 (C3) .. 83 (B5): f = 440 · 2^((m − 69)/12)
                let midi = 48 + pc + 12 * oct;
                let f = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
                chroma[pc as usize] += goertzel(seg, f);
            }
        }
        windows += 1;
        i += hop;
    }
    if windows == 0 { return None; }
    let max = chroma.iter().cloned().fold(0.0f64, f64::max);
    if max <= 0.0 { return None; }
    for c in chroma.iter_mut() { *c /= max; }
    Some(chroma)
}

/// `(pitch_class 0..11, is_major)` via a 3-octave chromagram + KS correlation.
pub fn detect_key(samples: &[f32]) -> Option<(u8, bool)> {
    let chroma = chroma12(samples)?;
    // Correlate against all 24 rotated profiles.
    let mut best: (u8, bool, f64) = (0, true, f64::MIN);
    for root in 0..12usize {
        let mut maj = [0.0; 12];
        let mut min = [0.0; 12];
        for i in 0..12 {
            maj[(i + root) % 12] = KS_MAJ[i];
            min[(i + root) % 12] = KS_MIN[i];
        }
        let cm = pearson(&chroma, &maj);
        let cn = pearson(&chroma, &min);
        if cm > best.2 { best = (root as u8, true, cm); }
        if cn > best.2 { best = (root as u8, false, cn); }
    }
    (best.2 > 0.0).then_some((best.0, best.1))
}

/// TT-style DR score: 3-second block RMS + peak → `dr_meter::dr_score`.
pub fn detect_dr(samples: &[f32]) -> Option<f64> {
    let block = 3 * SR as usize;
    if samples.len() < block { return None; }
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak <= 0.0 { return None; }
    let peak_dbfs = 20.0 * (peak as f64).log10();
    let blocks: Vec<f64> = samples.chunks(block)
        .filter(|c| c.len() == block)
        .map(|c| {
            let rms = (c.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt();
            tulipix_music::dr_meter::rms_to_dbfs(rms)
        })
        .collect();
    tulipix_music::dr_meter::dr_score(&blocks, peak_dbfs).map(|d| (d * 10.0).round() / 10.0)
}

/// `np.p4.music.embeddings` (dsp-v1) — a hand-rolled sonic descriptor for
/// "Sonic Similar", no model download needed. 43 dims:
///
/// * 12 chroma (harmony) · weight 0.8
/// * 24 log-spaced spectral band energies 55 Hz–7 kHz (timbre) · weight 1.0
/// * 3 rhythm (BPM + onset-flux mean/std) · weight 0.6
/// * 3 dynamics (DR, RMS, crest factor) · weight 0.5
/// * 1 zero-crossing rate (brightness) · weight 0.5
///
/// Each group is unit-normalized before weighting so cosine distance mixes
/// harmony/timbre/rhythm on comparable scales. Tracks embedded with a real
/// CLAP/PANNs model later use a different `model` tag — the similarity query
/// never mixes spaces.
pub fn dsp_embedding(samples: &[f32]) -> Option<Vec<f32>> {
    if samples.len() < 4 * SR as usize { return None; }

    // Timbre: Goertzel probes at 24 log-spaced centre frequencies, averaged
    // over up to 8 spread-out 2-second windows.
    let win = 2 * SR as usize;
    let n_windows = ((samples.len() - win) / win).clamp(1, 8);
    let step = (samples.len() - win) / n_windows;
    let mut bands = [0.0f64; 24];
    for wi in 0..n_windows {
        let seg = &samples[wi * step..wi * step + win];
        for (bi, band) in bands.iter_mut().enumerate() {
            // 55 Hz · (7000/55)^(bi/23) — log spacing across the audible core.
            let f = 55.0 * (7000.0f64 / 55.0).powf(bi as f64 / 23.0);
            *band += goertzel(seg, f);
        }
    }
    // Log-compress (energies span orders of magnitude), then unit-normalize.
    for b in bands.iter_mut() { *b = (*b + 1e-9).ln().max(0.0); }

    let chroma = chroma12(samples)?;
    let bpm = detect_bpm(samples);
    let dr = detect_dr(samples);

    // Onset-flux statistics (rhythmic density/steadiness).
    let flux = onset_flux(samples, 1024, 512);
    let fmean = if flux.is_empty() { 0.0 } else { flux.iter().sum::<f32>() / flux.len() as f32 };
    let fstd = if flux.len() < 2 { 0.0 } else {
        (flux.iter().map(|v| (v - fmean).powi(2)).sum::<f32>() / flux.len() as f32).sqrt()
    };

    // Dynamics + brightness.
    let rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
    let crest = if rms > 0.0 { (peak / rms).min(20.0) / 20.0 } else { 0.0 };
    let zcr = samples.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f64
        / samples.len() as f64;

    // Assemble: unit-normalize each group, then apply the group weight.
    fn push_group(out: &mut Vec<f32>, group: &[f64], weight: f64) {
        let norm = group.iter().map(|v| v * v).sum::<f64>().sqrt();
        let n = if norm > 0.0 { norm } else { 1.0 };
        for v in group { out.push((v / n * weight) as f32); }
    }
    let mut v = Vec::with_capacity(43);
    push_group(&mut v, &bands, 1.0);
    push_group(&mut v, &chroma, 0.8);
    push_group(&mut v, &[bpm.unwrap_or(120.0) / 200.0, fmean as f64, fstd as f64], 0.6);
    push_group(&mut v, &[dr.unwrap_or(8.0) / 20.0, rms, crest], 0.5);
    push_group(&mut v, &[zcr], 0.5);
    Some(v)
}

/// Full result for one track.
pub struct Analysis {
    pub bpm: Option<f64>,
    pub key_pc: Option<(u8, bool)>,
    pub dr: Option<f64>,
}

/// Blocking: decode + all three passes. Run on a worker thread.
pub fn analyze_file(path: &std::path::Path) -> Option<Analysis> {
    let samples = decode_mono(path)?;
    if samples.is_empty() { return None; }
    Some(Analysis {
        bpm: detect_bpm(&samples),
        key_pc: detect_key(&samples),
        dr: detect_dr(&samples),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 120 BPM click track: a short decaying burst every 0.5 s.
    fn click_track(secs: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; secs * SR as usize];
        let period = SR as usize / 2;
        for start in (0..v.len()).step_by(period) {
            for i in 0..800.min(v.len() - start) {
                v[start + i] = (1.0 - i as f32 / 800.0) * (i as f32 * 0.9).sin();
            }
        }
        v
    }

    #[test]
    fn bpm_of_click_track_is_120() {
        let bpm = detect_bpm(&click_track(30)).unwrap();
        assert!((bpm - 120.0).abs() < 3.0, "bpm was {bpm}");
    }

    /// A-minor triad (A3 C4 E4) sines — detected key should be A (pc 9).
    #[test]
    fn key_of_a_minor_triad() {
        let n = 20 * SR as usize;
        let mut v = vec![0.0f32; n];
        for (i, s) in v.iter_mut().enumerate() {
            let t = i as f64 / SR as f64;
            let f = |hz: f64| (2.0 * std::f64::consts::PI * hz * t).sin();
            *s = (0.5 * f(220.0) + 0.35 * f(261.63) + 0.35 * f(329.63)) as f32;
        }
        let (pc, _major) = detect_key(&v).unwrap();
        assert_eq!(pc, 9, "expected pitch class A");
    }

    #[test]
    fn dr_flat_sine_is_low() {
        // Constant-level sine: peak ≈ RMS + 3 dB → DR ≈ 3.
        let n = 12 * SR as usize;
        let v: Vec<f32> = (0..n).map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SR as f32).sin()).collect();
        let dr = detect_dr(&v).unwrap();
        assert!(dr < 4.5, "dr was {dr}");
    }
}
