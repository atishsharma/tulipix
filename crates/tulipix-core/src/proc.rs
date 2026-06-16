//! Cross-platform child-process helpers.
//!
//! On Windows a GUI app (`windows_subsystem = "windows"`) has no console, so
//! every console-subsystem child it spawns (yt-dlp, ffmpeg, mpv, powershell,
//! …) pops its own black console window for the lifetime of the process. The
//! `CREATE_NO_WINDOW` creation flag suppresses that window. On other platforms
//! this is a no-op.

/// Windows `CREATE_NO_WINDOW` process creation flag.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Suppress the console window for a spawned child process on Windows.
///
/// Apply to **every** `Command` before `spawn`/`output`/`status`:
///
/// ```ignore
/// use tulipix_core::proc::NoWindow;
/// Command::new("yt-dlp").arg("…").no_window().output()?;
/// ```
pub trait NoWindow {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}

impl NoWindow for tokio::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        self.creation_flags(CREATE_NO_WINDOW)
    }
    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}
