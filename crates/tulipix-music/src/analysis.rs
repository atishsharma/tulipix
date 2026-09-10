//! `np.p4.music.analysis` — the audio pass `bpm_key` and `dr_meter` were
//! written to receive.
//!
//! Both of those modules say the decode "happens in the worker" and own only
//! the maths and the columns. There was no worker. This is it: one ffmpeg
//! decode per track, and DR, tempo and key all read from that single buffer —
//! decoding a file three times to answer three questions about it would be the
//! whole cost of the scan, repeated.
//!
//! None of this is a mastering-grade measurement, and it is not trying to be.
//! DR follows the TT/Pleasurize shape, tempo is autocorrelation over an onset
//! envelope, and key is Krumhansl–Schmuckler over a Goertzel chroma. They are
//! good enough to sort a library by, which is what they are for.

use std::path::Path;

use anyhow::Result;

use crate::{bpm_key, dr_meter, waveform};

/// What one pass yields. Every field is optional because a track can be too
/// short, too quiet or too strange for any one of them while the others still
/// hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub dr: Option<f64>,
    pub bpm: Option<f64>,
    /// Camelot code, e.g. "8A".
    pub camelot: Option<String>,
    /// The peak envelope, so the caller can seed the waveform cache from the
    /// same decode instead of running ffmpeg again for it.
    pub envelope: Vec<u8>,
    /// `SPEC_BANDS` levels per column, `SPEC_HZ` columns a second, row-major by
    /// column. Empty when the track was too short for even one window.
    pub spectrogram: Vec<u8>,
    /// A 44-dimensional summary of how the track sounds: the mean level of each
    /// spectrogram band, then its chroma. Unit length, so cosine is the whole
    /// comparison. Empty when there was no spectrum to summarise.
    pub fingerprint: Vec<f32>,
}

/// Decode `src` once and measure everything.
///
/// Blocking, like every other decode in this workspace: callers on a runtime
/// put it on `spawn_blocking`.
pub fn analyse(src: &Path) -> Result<Analysis> {
    let rate = waveform::ANALYSIS_RATE;
    let pcm = waveform::pcm(src, rate)?;
    if pcm.is_empty() {
        anyhow::bail!("no audio in {}", src.display());
    }
    let spec = spectrogram(&pcm, rate);
    Ok(Analysis {
        dr: dynamic_range(&pcm),
        bpm: tempo(&pcm, rate),
        camelot: key_of(&pcm, rate),
        envelope: waveform::envelope(&pcm),
        fingerprint: fingerprint(&spec, chroma(&pcm, rate)),
        spectrogram: spec,
    })
}

/// The model name a fingerprint from this module is stored under.
///
/// `embeddings::similar` only compares vectors sharing a model, which is what
/// keeps these 44 numbers from being cosine-compared against a 512-dimension
/// CLAP embedding if one is ever computed for the same track.
pub const FINGERPRINT_MODEL: &str = "dsp-v1";

/// Store whatever came back, in the columns the two modules already own.
pub async fn store(pool: &sqlx::SqlitePool, item_id: i64, a: &Analysis) -> Result<()> {
    if let Some(dr) = a.dr {
        dr_meter::store(pool, item_id, dr).await?;
    }
    if a.bpm.is_some() || a.camelot.is_some() {
        bpm_key::store(pool, item_id, a.bpm, a.camelot.as_deref()).await?;
    }
    if !a.fingerprint.is_empty() {
        crate::embeddings::store(pool, item_id, FINGERPRINT_MODEL, &a.fingerprint).await?;
    }
    Ok(())
}

// ------------------------------------------------------------ dynamic range --

/// Three-second blocks, which is the window the DR spec uses.
const DR_BLOCK_SECS: f64 = 3.0;

fn dynamic_range(pcm: &[i16]) -> Option<f64> {
    let block = (waveform::ANALYSIS_RATE as f64 * DR_BLOCK_SECS) as usize;
    if pcm.len() < block {
        return None; // shorter than one window: there is nothing to spread
    }
    let mut peak = 0.0f64;
    let mut blocks = Vec::with_capacity(pcm.len() / block + 1);
    for chunk in pcm.chunks(block) {
        let mut sum = 0.0f64;
        for s in chunk {
            let v = *s as f64 / 32_768.0;
            sum += v * v;
            let a = v.abs();
            if a > peak {
                peak = a;
            }
        }
        blocks.push(dr_meter::rms_to_dbfs((sum / chunk.len() as f64).sqrt()));
    }
    dr_meter::dr_score(&blocks, dr_meter::rms_to_dbfs(peak))
}

// -------------------------------------------------------------------- tempo --

/// The tempo range worth searching. Below 60 and above 190 an autocorrelation
/// peak is nearly always a half- or double-time image of the real one.
const BPM_MIN: f64 = 60.0;
const BPM_MAX: f64 = 190.0;

/// Onset envelope + autocorrelation.
///
/// The envelope is the frame-to-frame *rise* in energy: a beat is where the
/// signal gets suddenly louder, and using the energy itself would lock onto
/// loud passages rather than onsets.
fn tempo(pcm: &[i16], rate: u32) -> Option<f64> {
    // ~86 frames a second, which resolves a 190 BPM beat to a few frames.
    let hop = (rate / 86).max(1) as usize;
    if pcm.len() < hop * 64 {
        return None;
    }
    let mut energy: Vec<f64> = pcm
        .chunks(hop)
        .map(|c| c.iter().map(|s| (*s as f64 / 32_768.0).powi(2)).sum::<f64>() / c.len() as f64)
        .collect();
    // Half-wave rectified difference: keep the rises, discard the decays.
    for i in (1..energy.len()).rev() {
        energy[i] = (energy[i] - energy[i - 1]).max(0.0);
    }
    energy[0] = 0.0;

    let mean = energy.iter().sum::<f64>() / energy.len() as f64;
    if mean <= f64::EPSILON {
        return None; // silence, or a signal with no transients at all
    }
    for e in energy.iter_mut() {
        *e -= mean; // centre it, so the correlation is not dominated by the DC term
    }

    let fps = rate as f64 / hop as f64;
    let lag_min = (fps * 60.0 / BPM_MAX).floor().max(1.0) as usize;
    let lag_max = (fps * 60.0 / BPM_MIN).ceil() as usize;
    if lag_max >= energy.len() {
        return None;
    }

    let mut best = (0usize, f64::MIN);
    for lag in lag_min..=lag_max {
        let n = energy.len() - lag;
        let mut acc = 0.0;
        for i in 0..n {
            acc += energy[i] * energy[i + lag];
        }
        // Normalised by overlap, or long lags lose to short ones on sample
        // count rather than on how periodic the signal actually is.
        let score = acc / n as f64;
        if score > best.1 {
            best = (lag, score);
        }
    }
    (best.0 > 0).then(|| 60.0 * fps / best.0 as f64)
}

// ---------------------------------------------------------------------- key --

/// Krumhansl–Schmuckler profiles, major and minor, starting on the tonic.
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Chroma over four octaves, then the best-correlating profile.
fn key_of(pcm: &[i16], rate: u32) -> Option<String> {
    let chroma = chroma(pcm, rate)?;
    let total: f64 = chroma.iter().sum();
    if total <= f64::EPSILON {
        return None;
    }

    let mut best: Option<(f64, u8, bool)> = None;
    for tonic in 0..12u8 {
        for (profile, major) in [(&MAJOR, true), (&MINOR, false)] {
            // Rotate the profile to the candidate tonic and correlate.
            let score = (0..12)
                .map(|i| chroma[(i + tonic as usize) % 12] * profile[i])
                .sum::<f64>();
            if best.is_none_or(|(b, _, _)| score > b) {
                best = Some((score, tonic, major));
            }
        }
    }
    let (_, tonic, major) = best?;
    bpm_key::camelot(tonic, major)
}

/// Energy per pitch class, summed over octaves 2..5.
///
/// Goertzel rather than an FFT: twelve pitch classes across four octaves is
/// 48 fixed frequencies, and a filter bank for 48 known bins is a few lines
/// against a dependency and a windowing scheme.
fn chroma(pcm: &[i16], rate: u32) -> Option<[f64; 12]> {
    // A window near a second, taken every few seconds: a key is a property of
    // the whole track, and correlating every sample of it costs far more than
    // it changes the answer.
    let win = rate as usize;
    if pcm.len() < win {
        return None;
    }
    let step = win * 3;
    let mut out = [0.0f64; 12];
    let mut windows = 0;

    let mut at = 0;
    while at + win <= pcm.len() {
        let slice = &pcm[at..at + win];
        for pc in 0..12usize {
            for octave in 2..6u32 {
                // A4 = 440 Hz is pitch class 9 in octave 4.
                let semitones = pc as f64 - 9.0 + 12.0 * (octave as f64 - 4.0);
                let freq = 440.0 * 2f64.powf(semitones / 12.0);
                if freq * 2.0 >= rate as f64 {
                    continue; // above Nyquist: nothing real up there
                }
                out[pc] += goertzel(slice, rate, freq);
            }
        }
        windows += 1;
        at += step;
    }
    (windows > 0).then_some(out)
}

/// Goertzel magnitude-squared for one frequency.
fn goertzel(samples: &[i16], rate: u32, freq: f64) -> f64 {
    let k = 2.0 * std::f64::consts::PI * freq / rate as f64;
    let coeff = 2.0 * k.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for x in samples {
        let s0 = (*x as f64 / 32_768.0) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)
}

// -------------------------------------------------------------- spectrogram --

/// Bands in a stored spectrogram.
///
/// The visualiser draws 24 by default and this is the source it will draw from,
/// so 32 leaves room to regroup without a re-scan while staying a byte per band.
pub const SPEC_BANDS: usize = 32;

/// Columns a second.
///
/// Not a frame rate: the UI runs at 60 and smooths between columns with the
/// attack/decay ballistics `visualizer::Spectrum::update` already applies. A
/// real analyser looks like this after its ballistics anyway, and 10 Hz keeps a
/// four-minute track near 75 kB instead of half a megabyte.
pub const SPEC_HZ: u32 = 10;

/// The lowest band's centre. Below this is room rumble and DC, not music.
const SPEC_LO_HZ: f64 = 40.0;
/// The highest band's centre, subject to Nyquist.
const SPEC_HI_HZ: f64 = 10_000.0;
/// How far below the track's own peak band reads as silence.
const SPEC_FLOOR_DB: f64 = 60.0;

/// A log-spaced band filter over each window of the track.
///
/// Goertzel again rather than an FFT, for the same reason chroma uses it: the
/// band centres are 32 known frequencies, and a filter bank for 32 known bins
/// is a few lines against a dependency. It costs more than an FFT would per
/// window and still disappears next to the ffmpeg decode that produced `pcm`.
///
/// Returns `bands x columns` bytes, row-major by column: column `c` band `b` is
/// at `c * SPEC_BANDS + b`. Levels are dB relative to the loudest band anywhere
/// in the track, so a quiet track still fills the display — this is a shape, not
/// a calibrated measurement.
pub fn spectrogram(pcm: &[i16], rate: u32) -> Vec<u8> {
    let win = (rate / SPEC_HZ).max(1) as usize;
    let cols = pcm.len() / win;
    if cols == 0 {
        return Vec::new();
    }

    // Band centres, log-spaced, each dropped if it would sit above Nyquist.
    let nyquist = rate as f64 / 2.0;
    let hi = SPEC_HI_HZ.min(nyquist * 0.9);
    let freqs: Vec<f64> = (0..SPEC_BANDS)
        .map(|b| {
            let t = b as f64 / (SPEC_BANDS - 1).max(1) as f64;
            SPEC_LO_HZ * (hi / SPEC_LO_HZ).powf(t)
        })
        .collect();

    let mut power = vec![0.0f64; cols * SPEC_BANDS];
    let mut peak = 0.0f64;
    for c in 0..cols {
        let slice = &pcm[c * win..(c + 1) * win];
        for (b, &f) in freqs.iter().enumerate() {
            let p = goertzel(slice, rate, f);
            power[c * SPEC_BANDS + b] = p;
            if p > peak {
                peak = p;
            }
        }
    }
    if peak <= f64::EPSILON {
        return vec![0; cols * SPEC_BANDS]; // silence: a flat floor, not an error
    }

    power
        .into_iter()
        .map(|p| {
            // 10*log10 of a magnitude-squared, i.e. dB below the track's peak.
            let db = 10.0 * (p / peak).max(1e-12).log10();
            let t = (db + SPEC_FLOOR_DB) / SPEC_FLOOR_DB;
            (t.clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect()
}

// -------------------------------------------------------------- fingerprint --

/// A unit-length summary of a track's sound: mean level per spectrogram band,
/// then chroma. Cosine between two of these is the whole comparison.
///
/// Not an acoustic fingerprint in the Chromaprint sense — it throws away all
/// time structure, so it cannot tell two different takes of the same song apart
/// from a remaster of one of them. What it is good at is the job it has: paired
/// with a duration match, recognising the same recording across bitrates and
/// misspelt tags, where the spectrum is near-identical and the length is exact.
fn fingerprint(spec: &[u8], chroma: Option<[f64; 12]>) -> Vec<f32> {
    if spec.is_empty() || spec.len() % SPEC_BANDS != 0 {
        return Vec::new();
    }
    let cols = spec.len() / SPEC_BANDS;
    let mut out = vec![0.0f32; SPEC_BANDS + 12];
    for c in 0..cols {
        for b in 0..SPEC_BANDS {
            out[b] += spec[c * SPEC_BANDS + b] as f32;
        }
    }
    for v in out.iter_mut().take(SPEC_BANDS) {
        *v /= (cols * 255) as f32;
    }

    if let Some(ch) = chroma {
        // Chroma arrives as raw summed energy whose scale depends on the track's
        // length and loudness; normalising it to its own max puts it on the same
        // 0..1 footing as the band means, so neither half dominates the cosine.
        let top = ch.iter().cloned().fold(0.0f64, f64::max);
        if top > f64::EPSILON {
            for (i, v) in ch.iter().enumerate() {
                out[SPEC_BANDS + i] = (*v / top) as f32;
            }
        }
    }

    // Centre each half on its own mean before normalising.
    //
    // Without this every component is non-negative, so every vector points into
    // the same orthant and the cosine between two unrelated tracks sits around
    // 0.95 — there is no room left to tell "similar" from "the same recording".
    // Centring restores the full -1..1 range, and doing it per half keeps a
    // bright track from shifting its chroma. This is why the comparison is
    // cosine and not a distance: the scale is gone by design, only shape is left.
    centre(&mut out[..SPEC_BANDS]);
    centre(&mut out[SPEC_BANDS..]);

    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return Vec::new(); // perfectly flat: no shape to remember
    }
    for v in out.iter_mut() {
        *v /= norm;
    }
    out
}

/// Subtract the mean in place.
fn centre(v: &mut [f32]) {
    if v.is_empty() {
        return;
    }
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    for x in v.iter_mut() {
        *x -= mean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at `freq`, `secs` long.
    fn sine(rate: u32, freq: f64, secs: f64) -> Vec<i16> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / rate as f64;
                ((t * freq * 2.0 * std::f64::consts::PI).sin() * 12_000.0) as i16
            })
            .collect()
    }

    #[test]
    fn goertzel_finds_its_own_tone() {
        let rate = 22_050;
        let s = sine(rate, 440.0, 0.5);
        let on = goertzel(&s, rate, 440.0);
        let off = goertzel(&s, rate, 700.0);
        assert!(on > off * 50.0, "440 Hz tone: on={on} off={off}");
    }

    #[test]
    fn a_pure_a_reads_as_an_a_key() {
        // Not a chord, so major/minor is a coin toss — but the tonic should be
        // A, and both A keys are Camelot 11 (11B major, 11A minor).
        let rate = 22_050;
        let s = sine(rate, 440.0, 4.0);
        let k = key_of(&s, rate).expect("a tone should resolve to a key");
        assert!(k.starts_with("11"), "expected an A key, got {k}");
    }

    #[test]
    fn a_steady_pulse_reads_as_its_own_tempo() {
        // 120 BPM = a click every 0.5 s.
        let rate = 22_050;
        let mut pcm = vec![0i16; rate as usize * 8];
        let mut at = 0usize;
        while at < pcm.len() {
            for i in at..(at + 900).min(pcm.len()) {
                pcm[i] = 20_000;
            }
            at += rate as usize / 2;
        }
        let bpm = tempo(&pcm, rate).expect("a pulse train should have a tempo");
        assert!((bpm - 120.0).abs() < 4.0, "expected ~120 BPM, got {bpm}");
    }

    #[test]
    fn silence_has_no_tempo_and_no_key() {
        let quiet = vec![0i16; 22_050 * 4];
        assert!(tempo(&quiet, 22_050).is_none());
        assert!(key_of(&quiet, 22_050).is_none());
    }

    #[test]
    fn a_spectrogram_puts_a_tone_in_the_right_band() {
        let rate = 22_050;
        let s = sine(rate, 1_000.0, 3.0);
        let spec = spectrogram(&s, rate);
        assert_eq!(spec.len() % SPEC_BANDS, 0);
        assert_eq!(spec.len() / SPEC_BANDS, 30, "3 seconds at 10 columns a second");

        // The loudest band of the first column should be the one nearest 1 kHz.
        let col = &spec[..SPEC_BANDS];
        let loudest = (0..SPEC_BANDS).max_by_key(|b| col[*b]).unwrap();
        let hi = 10_000f64.min(rate as f64 / 2.0 * 0.9);
        let expect = (0..SPEC_BANDS)
            .min_by(|a, b| {
                let f = |i: &usize| {
                    let t = *i as f64 / (SPEC_BANDS - 1) as f64;
                    (40.0 * (hi / 40.0f64).powf(t) - 1_000.0).abs()
                };
                f(a).total_cmp(&f(b))
            })
            .unwrap();
        assert!(
            loudest.abs_diff(expect) <= 1,
            "1 kHz tone landed in band {loudest}, expected about {expect}"
        );
    }

    #[test]
    fn silence_and_scraps_produce_no_spectrogram_to_speak_of() {
        // Shorter than one column: nothing to draw, and not an error.
        assert!(spectrogram(&[0i16; 10], 22_050).is_empty());
        // A full second of silence is columns of floor, not an empty vector —
        // the visualiser still has to draw the quiet part of a track.
        let quiet = spectrogram(&vec![0i16; 22_050], 22_050);
        assert_eq!(quiet.len(), SPEC_BANDS * 10);
        assert!(quiet.iter().all(|b| *b == 0));
    }

    #[test]
    fn fingerprints_separate_different_sounds() {
        let rate = 22_050;
        let low = sine(rate, 200.0, 4.0);
        let high = sine(rate, 4_000.0, 4.0);
        let fp = |pcm: &[i16]| fingerprint(&spectrogram(pcm, rate), chroma(pcm, rate));

        let a = fp(&low);
        let b = fp(&high);
        assert_eq!(a.len(), SPEC_BANDS + 12);
        // Unit length, so cosine is the whole comparison.
        assert!((a.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);

        let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        assert!(cos(&a, &a) > 0.99, "a track matches itself");
        // The centring in `fingerprint` is what buys this gap: without it every
        // component is non-negative and two unrelated tracks still score ~0.95.
        assert!(cos(&a, &b) < 0.5, "200 Hz and 4 kHz should not look alike: {}", cos(&a, &b));
    }

    #[test]
    fn a_short_clip_has_no_dynamic_range() {
        assert!(dynamic_range(&vec![0i16; 100]).is_none());
    }
}
