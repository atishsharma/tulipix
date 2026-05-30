//! Streaming whisper — partial cues land while the video keeps playing.
//!
//! Sliding window: capture audio in fixed `chunk_s` slices, hand each chunk
//! to whisper.cpp with `--realtime`, emit `PartialCue`s as they come back.
//! If the CPU is too slow to keep up (decode pace falls behind playback by
//! `MAX_LAG_RATIO`), the strategy downgrades to a batch transcribe so the
//! viewer at least gets full cues at the end of the file rather than a
//! growing stutter.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const DEFAULT_CHUNK_S: f64 = 2.5;
pub const DEFAULT_OVERLAP_S: f64 = 0.5;
pub const MAX_LAG_RATIO: f64 = 1.5;
pub const MIN_CPU_HEADROOM: f64 = 0.20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingMode {
    /// Sliding window — partial cues update live.
    Sliding,
    /// Batch — single full-file pass; final cues only.
    Batch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingConfig {
    pub chunk_s: f64,
    pub overlap_s: f64,
    pub language: Option<String>,
    pub threads: u32,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self { chunk_s: DEFAULT_CHUNK_S, overlap_s: DEFAULT_OVERLAP_S, language: None, threads: 4 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialCue {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    /// True while whisper is still revising this cue; false once committed.
    pub provisional: bool,
}

/// Decision input from the controller: how recently audio decoded vs how
/// recently whisper produced a cue.
#[derive(Debug, Clone)]
pub struct LagSample {
    pub decoded_position_s: f64,
    pub transcribed_position_s: f64,
    pub cpu_headroom: f64,
}

pub fn pick_mode(sample: &LagSample) -> StreamingMode {
    let lag = (sample.decoded_position_s - sample.transcribed_position_s).max(0.0);
    let baseline = (sample.decoded_position_s.max(1.0)) * (MAX_LAG_RATIO - 1.0);
    if lag > baseline || sample.cpu_headroom < MIN_CPU_HEADROOM {
        StreamingMode::Batch
    } else {
        StreamingMode::Sliding
    }
}

/// Tracks sliding-window cue emission. Provisional cues from overlapping
/// windows replace earlier ones with the same start; committed cues stay.
pub struct StreamingBuffer {
    cues: VecDeque<PartialCue>,
    config: StreamingConfig,
    last_decoded_at: Instant,
}

impl StreamingBuffer {
    pub fn new(config: StreamingConfig) -> Self {
        Self { cues: VecDeque::new(), config, last_decoded_at: Instant::now() }
    }

    pub fn push(&mut self, cue: PartialCue) {
        if let Some(existing) = self.cues.iter_mut()
            .rfind(|c| (c.start_s - cue.start_s).abs() < self.config.overlap_s)
        {
            // Keep the later, fuller text — whisper.cpp lengthens cues as
            // context grows. Once a cue is committed it's frozen.
            if !existing.provisional { return; }
            *existing = cue;
            return;
        }
        // Mark any older provisional cues as committed before pushing.
        for prev in self.cues.iter_mut() {
            if prev.end_s + self.config.overlap_s < cue.start_s {
                prev.provisional = false;
            }
        }
        self.cues.push_back(cue);
        self.last_decoded_at = Instant::now();
    }

    pub fn commit_through(&mut self, t: f64) {
        for c in self.cues.iter_mut() { if c.end_s <= t { c.provisional = false; } }
    }

    pub fn drain_committed(&mut self) -> Vec<PartialCue> {
        let mut out = Vec::new();
        while matches!(self.cues.front(), Some(c) if !c.provisional) {
            out.push(self.cues.pop_front().unwrap());
        }
        out
    }

    pub fn snapshot(&self) -> Vec<PartialCue> { self.cues.iter().cloned().collect() }

    pub fn stale_for(&self) -> Duration { self.last_decoded_at.elapsed() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_sliding_with_healthy_lag() {
        let s = LagSample { decoded_position_s: 30.0, transcribed_position_s: 28.0, cpu_headroom: 0.4 };
        assert_eq!(pick_mode(&s), StreamingMode::Sliding);
    }

    #[test]
    fn pick_batch_when_too_far_behind() {
        let s = LagSample { decoded_position_s: 30.0, transcribed_position_s: 5.0, cpu_headroom: 0.6 };
        assert_eq!(pick_mode(&s), StreamingMode::Batch);
    }

    #[test]
    fn pick_batch_when_cpu_starved() {
        let s = LagSample { decoded_position_s: 10.0, transcribed_position_s: 9.5, cpu_headroom: 0.05 };
        assert_eq!(pick_mode(&s), StreamingMode::Batch);
    }

    #[test]
    fn provisional_cue_replaced_by_later_overlap() {
        let mut buf = StreamingBuffer::new(StreamingConfig::default());
        buf.push(PartialCue { start_s: 0.0, end_s: 2.0, text: "Hel-".into(), provisional: true });
        buf.push(PartialCue { start_s: 0.0, end_s: 2.5, text: "Hello".into(), provisional: true });
        let snap = buf.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].text, "Hello");
    }

    #[test]
    fn commit_through_freezes_old_cues() {
        let mut buf = StreamingBuffer::new(StreamingConfig::default());
        buf.push(PartialCue { start_s: 0.0, end_s: 2.0, text: "a".into(), provisional: true });
        buf.push(PartialCue { start_s: 3.0, end_s: 5.0, text: "b".into(), provisional: true });
        buf.commit_through(2.5);
        let drained = buf.drain_committed();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].text, "a");
    }

    #[test]
    fn committed_cue_not_overwritten() {
        let mut buf = StreamingBuffer::new(StreamingConfig::default());
        buf.push(PartialCue { start_s: 0.0, end_s: 2.0, text: "first".into(), provisional: false });
        buf.push(PartialCue { start_s: 0.0, end_s: 2.0, text: "second".into(), provisional: true });
        let snap = buf.snapshot();
        assert_eq!(snap[0].text, "first");
    }
}
