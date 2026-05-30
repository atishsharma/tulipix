//! Cold-start benchmark hook. The app calls `record_start()` as early as
//! possible in `main()` and `mark_interactive()` from the Slint compositor
//! on the first frame paint. When `TULIPIX_COLD_START_BENCH=1` is set, the
//! delta is emitted as a tracing event and the process exits 0 immediately.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub fn record_start() {
    let _ = START.set(Instant::now());
}

pub fn elapsed_ms() -> u64 {
    START.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0)
}

pub fn bench_mode() -> bool {
    std::env::var("TULIPIX_COLD_START_BENCH").ok().as_deref() == Some("1")
}

/// Call from the first-frame paint hook. In bench mode emits the event and
/// exits the process with status 0; in normal mode just logs.
pub fn mark_interactive() {
    let ms = elapsed_ms();
    tracing::info!(cold_start_ms = ms, "cold_start_ms={ms}");
    if bench_mode() {
        // Flush stdout/stderr before exiting so the bench script sees the line.
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::process::exit(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn record_then_elapsed_monotonic() {
        record_start();
        // OnceLock is initialised once per process; second call is a no-op.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(elapsed_ms() >= 1);
    }
    #[test] fn bench_mode_reads_env() {
        // Env-var read; we just exercise the function, value depends on caller.
        let _ = bench_mode();
    }
}
