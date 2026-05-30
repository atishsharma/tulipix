//! Cloud section app layer: spawn the bundled/system rclone binary and feed its
//! output to the `tulipix-cloud` parsers. Argv construction + output parsing
//! live in the crate (testable); the subprocess spawn lives here. Runs blocking
//! `std::process` — callers wrap in `spawn_blocking`.

use anyhow::{bail, Result};
use std::path::PathBuf;

fn rclone() -> PathBuf {
    tulipix_core::thumbs::tool_bin("rclone")
}

/// Is an rclone binary reachable (bundled or on PATH)?
pub fn available() -> bool {
    std::process::Command::new(rclone())
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run rclone with `args`, returning stdout. Errors carry rclone's stderr.
pub fn run(args: &[String]) -> Result<String> {
    let out = std::process::Command::new(rclone()).args(args).output()?;
    if !out.status.success() {
        bail!("rclone {:?}: {}", args, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
