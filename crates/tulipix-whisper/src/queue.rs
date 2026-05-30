//! whisper.cpp subprocess driver + FIFO transcribe queue.
//!
//! Two layers:
//!   * `WhisperRunner` — fires one `whisper.cpp` invocation per job and writes
//!     `.srt` + `.vtt` + `.json` sidecars next to the source.
//!   * `TranscribeQueue` — bounded FIFO with a single in-flight worker so we
//!     never starve playback. Jobs land via `enqueue()`, optionally
//!     `cancel()` while still pending, and `drain()` pops finished reports.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub const DEFAULT_QUEUE_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubFormat { Srt, Vtt, Json }

impl SubFormat {
    pub fn extension(self) -> &'static str {
        match self { Self::Srt => "srt", Self::Vtt => "vtt", Self::Json => "json" }
    }
    pub fn whisper_flag(self) -> &'static str {
        match self { Self::Srt => "--output-srt", Self::Vtt => "--output-vtt", Self::Json => "--output-json" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscribeJob {
    pub item_id: i64,
    pub src: PathBuf,
    pub model_path: PathBuf,
    pub language: Option<String>,
    pub formats: Vec<SubFormat>,
    pub threads: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus { Pending, Running, Cancelled, Failed, Done }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobReport {
    pub item_id: i64,
    pub status: JobStatus,
    pub outputs: Vec<PathBuf>,
    pub error: Option<String>,
}

fn bundled_whisper() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|p| p.parent()).and_then(|p| p.parent());
    let os_arch =
        if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") { "linux-aarch64" }
        else if cfg!(target_os = "linux") { "linux-x86_64" }
        else if cfg!(target_os = "windows") { "windows-x86_64" }
        else if cfg!(target_arch = "aarch64") { "macos-aarch64" }
        else { "macos-x86_64" };
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    dir.map(|d| d.join("resources").join("bin").join(os_arch).join(format!("whisper{ext}")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("whisper"))
}

pub struct WhisperRunner {
    binary: PathBuf,
}

impl WhisperRunner {
    pub fn new() -> Self { Self { binary: bundled_whisper() } }
    pub fn with_binary(p: impl Into<PathBuf>) -> Self { Self { binary: p.into() } }

    /// Build the argv whisper.cpp would be invoked with. Pure — does not spawn.
    pub fn argv(&self, job: &TranscribeJob, out_dir: &Path) -> Vec<String> {
        let stem = job.src.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
        let out_base = out_dir.join(stem);
        let mut v: Vec<String> = vec![
            "-m".into(), job.model_path.to_string_lossy().into_owned(),
            "-f".into(), job.src.to_string_lossy().into_owned(),
            "-of".into(), out_base.to_string_lossy().into_owned(),
            "-t".into(), job.threads.max(1).to_string(),
        ];
        if let Some(lang) = &job.language {
            v.push("-l".into()); v.push(lang.clone());
        }
        for f in &job.formats { v.push(f.whisper_flag().to_string()); }
        v
    }

    pub fn run(&self, job: &TranscribeJob) -> Result<Vec<PathBuf>> {
        let dir = job.src.parent().unwrap_or(Path::new("."));
        let argv = self.argv(job, dir);
        let out = Command::new(&self.binary)
            .args(argv.iter().map(String::as_str))
            .output()
            .with_context(|| format!("spawn {}", self.binary.display()))?;
        if !out.status.success() {
            anyhow::bail!("whisper exit {} — {}", out.status, String::from_utf8_lossy(&out.stderr));
        }
        Ok(self.expected_outputs(job, dir))
    }

    pub fn expected_outputs(&self, job: &TranscribeJob, out_dir: &Path) -> Vec<PathBuf> {
        let stem = job.src.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
        job.formats.iter()
            .map(|f| out_dir.join(format!("{stem}.{}", f.extension())))
            .collect()
    }
}

impl Default for WhisperRunner { fn default() -> Self { Self::new() } }

#[derive(Default)]
struct QueueState {
    pending: VecDeque<TranscribeJob>,
    cancelled: std::collections::HashSet<i64>,
    in_flight: Option<i64>,
    finished: Vec<JobReport>,
}

#[derive(Clone)]
pub struct TranscribeQueue {
    inner: Arc<Mutex<QueueState>>,
    cap: usize,
}

impl Default for TranscribeQueue {
    fn default() -> Self { Self::with_capacity(DEFAULT_QUEUE_CAP) }
}

impl TranscribeQueue {
    pub fn with_capacity(cap: usize) -> Self {
        Self { inner: Arc::new(Mutex::new(QueueState::default())), cap }
    }

    pub fn enqueue(&self, job: TranscribeJob) -> Result<()> {
        let mut s = self.inner.lock().unwrap();
        if s.pending.len() >= self.cap { anyhow::bail!("queue full ({})", self.cap); }
        s.pending.push_back(job);
        Ok(())
    }

    pub fn cancel(&self, item_id: i64) {
        let mut s = self.inner.lock().unwrap();
        s.cancelled.insert(item_id);
        s.pending.retain(|j| j.item_id != item_id);
    }

    pub fn next(&self) -> Option<TranscribeJob> {
        let mut s = self.inner.lock().unwrap();
        if s.in_flight.is_some() { return None; }
        while let Some(j) = s.pending.pop_front() {
            if s.cancelled.remove(&j.item_id) {
                s.finished.push(JobReport { item_id: j.item_id, status: JobStatus::Cancelled, outputs: vec![], error: None });
                continue;
            }
            s.in_flight = Some(j.item_id);
            return Some(j);
        }
        None
    }

    pub fn complete(&self, report: JobReport) {
        let mut s = self.inner.lock().unwrap();
        if s.in_flight == Some(report.item_id) { s.in_flight = None; }
        s.finished.push(report);
    }

    pub fn drain(&self) -> Vec<JobReport> {
        let mut s = self.inner.lock().unwrap();
        std::mem::take(&mut s.finished)
    }

    pub fn stats(&self) -> (usize, bool, usize) {
        let s = self.inner.lock().unwrap();
        (s.pending.len(), s.in_flight.is_some(), s.finished.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: i64) -> TranscribeJob {
        TranscribeJob {
            item_id: id,
            src: PathBuf::from(format!("/tmp/{id}.wav")),
            model_path: PathBuf::from("/m/ggml-tiny.bin"),
            language: Some("en".into()),
            formats: vec![SubFormat::Srt, SubFormat::Vtt],
            threads: 4,
        }
    }

    #[test]
    fn argv_contains_required_flags() {
        let r = WhisperRunner::with_binary("whisper");
        let argv = r.argv(&job(1), Path::new("/out"));
        assert!(argv.windows(2).any(|w| w[0] == "-m" && w[1] == "/m/ggml-tiny.bin"));
        assert!(argv.iter().any(|s| s == "--output-srt"));
        assert!(argv.iter().any(|s| s == "--output-vtt"));
        assert!(argv.iter().any(|s| s == "-l"));
    }

    #[test]
    fn fifo_pop_order() {
        let q = TranscribeQueue::default();
        q.enqueue(job(1)).unwrap();
        q.enqueue(job(2)).unwrap();
        let n = q.next().unwrap();
        assert_eq!(n.item_id, 1);
        // second next() blocks behind in-flight item
        assert!(q.next().is_none());
        q.complete(JobReport { item_id: 1, status: JobStatus::Done, outputs: vec![], error: None });
        assert_eq!(q.next().unwrap().item_id, 2);
    }

    #[test]
    fn cancel_drops_pending_and_emits_report() {
        let q = TranscribeQueue::default();
        q.enqueue(job(1)).unwrap();
        q.enqueue(job(2)).unwrap();
        q.cancel(2);
        assert_eq!(q.next().unwrap().item_id, 1);
        q.complete(JobReport { item_id: 1, status: JobStatus::Done, outputs: vec![], error: None });
        assert!(q.next().is_none());
        let drained = q.drain();
        assert!(drained.iter().any(|r| r.item_id == 1 && r.status == JobStatus::Done));
    }

    #[test]
    fn capacity_rejects_overflow() {
        let q = TranscribeQueue::with_capacity(2);
        q.enqueue(job(1)).unwrap();
        q.enqueue(job(2)).unwrap();
        assert!(q.enqueue(job(3)).is_err());
    }

    #[test]
    fn expected_outputs_named_after_stem() {
        let r = WhisperRunner::with_binary("w");
        let out = r.expected_outputs(&job(7), Path::new("/o"));
        assert!(out.iter().any(|p| p == Path::new("/o/7.srt")));
        assert!(out.iter().any(|p| p == Path::new("/o/7.vtt")));
    }
}
