//! Robotic (non-AI) read-aloud via espeak-ng. Zero model, zero ONNX — compiles
//! and runs in every build. Produces a WAV the reader plays through mpv, same
//! path as the neural (Kokoro) engine, so per-sentence highlight sync matches.
//!
//! espeak-ng is resolved via [`tulipix_core::thumbs::tool_bin`], so a copy
//! bundled in `resources/bin/<os-arch>/` (release packaging) is used with **no
//! user install**; it falls back to a `PATH` espeak-ng otherwise. A bundled
//! `resources/bin/<os-arch>/espeak-ng-data` directory is passed via `--path` so
//! the bundled binary finds its phoneme dictionaries.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Bundled espeak data dir (`--path` value) — the directory *containing*
/// `espeak-ng-data`, if one is bundled. None → espeak uses its own default.
fn espeak_data_path() -> Option<PathBuf> {
    tulipix_core::thumbs::bundled_file("espeak-ng-data")
        .and_then(|d| d.parent().map(Path::to_path_buf))
}

/// An espeak-ng `Command` pointed at the bundled binary (or PATH) with the
/// bundled data dir wired in. Shared by the robotic engine and Kokoro's
/// phonemizer so both honor a bundled copy.
pub fn espeak_command() -> Command {
    let mut c = Command::new(tulipix_core::thumbs::tool_bin("espeak-ng"));
    if let Some(p) = espeak_data_path() {
        c.arg(format!("--path={}", p.display()));
    }
    c
}

/// Is espeak-ng runnable (bundled or on PATH)?
pub fn espeak_available() -> bool {
    espeak_command()
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Render `text` to a WAV via espeak-ng. `female` picks a female voice preset,
/// `wpm` is speaking rate (words/min). Returns true on success.
pub fn espeak_to_wav(text: &str, female: bool, wpm: u32, out: &Path) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let voice = if female { "en-us+f3" } else { "en-us+m3" };
    let wpm = wpm.clamp(80, 400).to_string();
    espeak_command()
        .args(["-v", voice, "-s", &wpm, "-w"])
        .arg(out)
        .arg(text)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        && out.exists()
}
