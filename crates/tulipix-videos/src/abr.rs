//! Adaptive bitrate ladder — pre-transcode HLS variants at fixed rungs
//! (240p / 480p / 720p / 1080p / 2160p). The player probes bandwidth and
//! picks the highest variant whose bitrate fits within a safety margin so
//! the buffer can absorb spikes without rebuffer pauses.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rung {
    Audio,
    P240,
    P480,
    P720,
    P1080,
    P1440,
    P2160,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub rung: Rung,
    pub width: i32,
    pub height: i32,
    /// Average bitrate the encoder targets, bits/sec.
    pub bitrate_bps: i64,
    /// HLS playlist filename written under the per-item ABR folder.
    pub playlist: &'static str,
}

pub const LADDER: &[Variant] = &[
    Variant { rung: Rung::Audio,  width: 0,    height: 0,    bitrate_bps: 128_000,    playlist: "audio.m3u8" },
    Variant { rung: Rung::P240,   width: 426,  height: 240,  bitrate_bps: 400_000,    playlist: "240p.m3u8" },
    Variant { rung: Rung::P480,   width: 854,  height: 480,  bitrate_bps: 1_200_000,  playlist: "480p.m3u8" },
    Variant { rung: Rung::P720,   width: 1280, height: 720,  bitrate_bps: 3_000_000,  playlist: "720p.m3u8" },
    Variant { rung: Rung::P1080,  width: 1920, height: 1080, bitrate_bps: 6_000_000,  playlist: "1080p.m3u8" },
    Variant { rung: Rung::P1440,  width: 2560, height: 1440, bitrate_bps: 12_000_000, playlist: "1440p.m3u8" },
    Variant { rung: Rung::P2160,  width: 3840, height: 2160, bitrate_bps: 25_000_000, playlist: "2160p.m3u8" },
];

/// Safety headroom — we only step up when the probe shows the network can
/// carry the next rung's bitrate × this multiplier. 1.4 = ~40 % spare so a
/// burst doesn't drop us back down immediately.
pub const HEADROOM: f64 = 1.4;

/// Pick the highest rung whose `bitrate_bps × HEADROOM <= probe_bps` AND
/// whose `height <= source_height`. Always returns at least the audio rung.
pub fn pick_variant(source_height: i32, probe_bps: i64) -> Variant {
    let mut best = LADDER[0]; // audio
    for v in LADDER {
        if v.height > source_height {
            continue;
        }
        let need = (v.bitrate_bps as f64 * HEADROOM) as i64;
        if need <= probe_bps {
            best = *v;
        }
    }
    best
}

#[derive(Debug, Default)]
pub struct BandwidthProbe {
    samples_bps: Vec<i64>,
    cap: usize,
}

impl BandwidthProbe {
    pub fn new(window: usize) -> Self {
        Self {
            samples_bps: Vec::with_capacity(window),
            cap: window.max(1),
        }
    }

    pub fn record(&mut self, segment_bytes: i64, downloaded_ms: i64) {
        if downloaded_ms <= 0 {
            return;
        }
        let bps = (segment_bytes as f64 * 8.0 * 1000.0 / downloaded_ms as f64) as i64;
        if self.samples_bps.len() == self.cap {
            self.samples_bps.remove(0);
        }
        self.samples_bps.push(bps);
    }

    /// Returns the harmonic mean — gives extra weight to the slow samples so
    /// a single fast segment doesn't trick the picker into stepping up.
    pub fn estimate_bps(&self) -> Option<i64> {
        if self.samples_bps.is_empty() {
            return None;
        }
        let sum_inv: f64 = self
            .samples_bps
            .iter()
            .map(|s| 1.0 / (*s as f64).max(1.0))
            .sum();
        Some((self.samples_bps.len() as f64 / sum_inv) as i64)
    }
}

/// Compose the HLS master playlist that references every variant the user
/// has actually transcoded so far. `available` lets the caller skip rungs
/// they haven't generated yet.
pub fn master_playlist(available: &[Variant], language_tag: &str) -> String {
    let mut s = String::from(
        "#EXTM3U\n#EXT-X-VERSION:6\n",
    );
    if let Some(audio) = available.iter().find(|v| v.rung == Rung::Audio) {
        s.push_str(&format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a1\",NAME=\"Default\",LANGUAGE=\"{lang}\",DEFAULT=YES,AUTOSELECT=YES,URI=\"{uri}\"\n",
            lang = language_tag,
            uri = audio.playlist,
        ));
    }
    for v in available {
        if v.rung == Rung::Audio {
            continue;
        }
        s.push_str(&format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={bw},RESOLUTION={w}x{h},AUDIO=\"a1\",CODECS=\"avc1.640028,mp4a.40.2\"\n{uri}\n",
            bw = v.bitrate_bps,
            w = v.width,
            h = v.height,
            uri = v.playlist,
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_caps_at_source_height() {
        // Source is 720p — never serve 1080p+ even on a fat pipe.
        let v = pick_variant(720, 100_000_000);
        assert_eq!(v.height, 720);
    }

    #[test]
    fn pick_steps_up_with_bandwidth() {
        let low = pick_variant(2160, 500_000);
        assert_eq!(low.rung, Rung::Audio); // 240p needs 400k * 1.4 = 560k

        let mid = pick_variant(2160, 5_000_000);
        assert_eq!(mid.height, 720);

        let high = pick_variant(2160, 50_000_000);
        assert_eq!(high.height, 2160);
    }

    #[test]
    fn pick_returns_audio_rung_under_floor() {
        let v = pick_variant(2160, 10_000);
        assert_eq!(v.rung, Rung::Audio);
    }

    #[test]
    fn bandwidth_probe_harmonic_mean_favours_slow_samples() {
        let mut p = BandwidthProbe::new(4);
        // 1 MB in 1 s = 8 Mbps, 1 MB in 10 s = 800 Kbps
        p.record(1_000_000, 1_000);
        p.record(1_000_000, 10_000);
        let est = p.estimate_bps().unwrap();
        // Arithmetic mean = ~4.4 Mbps; harmonic should be much closer to the slow sample.
        assert!(est < 2_000_000, "harmonic mean drags toward the slow sample, got {est}");
        assert!(est > 700_000);
    }

    #[test]
    fn probe_window_evicts_oldest() {
        let mut p = BandwidthProbe::new(2);
        p.record(1_000_000, 1_000);
        p.record(1_000_000, 1_000);
        p.record(1_000_000, 10_000); // pushes first 8 Mbps sample out
        let est = p.estimate_bps().unwrap();
        assert!(est < 4_000_000);
    }

    #[test]
    fn probe_handles_zero_ms() {
        let mut p = BandwidthProbe::new(2);
        p.record(1_000_000, 0); // ignored
        assert!(p.estimate_bps().is_none());
    }

    #[test]
    fn master_playlist_references_audio_and_video() {
        let pl = master_playlist(
            &[LADDER[0], LADDER[3], LADDER[4]], // audio + 720p + 1080p
            "en",
        );
        assert!(pl.contains("TYPE=AUDIO"));
        assert!(pl.contains("RESOLUTION=1280x720"));
        assert!(pl.contains("RESOLUTION=1920x1080"));
        assert!(pl.contains("AUDIO=\"a1\""));
    }

    #[test]
    fn ladder_bitrates_monotonic() {
        for w in LADDER.windows(2) {
            assert!(w[0].bitrate_bps < w[1].bitrate_bps, "{:?}", w);
        }
    }
}
