//! Cross-platform transport for mpv's JSON IPC (`--input-ipc-server`).
//!
//! mpv exposes the same JSON protocol on every OS but over a different
//! primitive: a Unix domain socket on Linux/macOS, a named pipe
//! (`\\.\pipe\NAME`) on Windows. This module hides that split so the player /
//! music code talks to one [`endpoint`] + [`connect`] pair regardless of host.
//!
//! [`connect`] returns a concrete [`IpcConn`] (a `UnixStream` or a `File` over
//! the pipe) — both are `Read + Write + Send`, so call sites can
//! `BufReader::new(conn)` and `conn.write_all(..)` unchanged.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The connected IPC stream — a Unix socket on Unix, a named-pipe file handle
/// on Windows. Both implement `Read + Write + Send`.
#[cfg(not(windows))]
pub type IpcConn = std::os::unix::net::UnixStream;
#[cfg(windows)]
pub type IpcConn = std::fs::File;

/// Build the IPC endpoint to hand mpv via `--input-ipc-server={path}` and to
/// connect our client to. `prefix` is a short tag (e.g. `tulipix-music`); the
/// pid keeps concurrent app instances from colliding.
///
/// * Unix — a `.sock` file under the temp dir.
/// * Windows — a named pipe under `\\.\pipe\`.
pub fn endpoint(prefix: &str) -> PathBuf {
    let pid = std::process::id();
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\{prefix}-{pid}"))
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join(format!("{prefix}-{pid}.sock"))
    }
}

/// Remove a stale endpoint before (re)spawning mpv. On Unix this unlinks the
/// leftover socket file; on Windows named pipes are kernel objects with no
/// filesystem entry, so this is a no-op.
pub fn cleanup(path: &Path) {
    #[cfg(not(windows))]
    {
        let _ = std::fs::remove_file(path);
    }
    #[cfg(windows)]
    {
        let _ = path; // pipes vanish when the owning process exits
    }
}

/// Connect to a running mpv's IPC endpoint, retrying for ~3 s because the
/// socket/pipe is not ready the instant mpv is spawned. Folds in the old
/// "poll until the socket exists" loop the call sites used to do by hand.
pub fn connect(path: &Path) -> io::Result<IpcConn> {
    let mut last = io::Error::new(io::ErrorKind::NotConnected, "mpv ipc: not attempted");
    for _ in 0..60 {
        match connect_once(path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = e;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last)
}

#[cfg(not(windows))]
fn connect_once(path: &Path) -> io::Result<IpcConn> {
    std::os::unix::net::UnixStream::connect(path)
}

#[cfg(windows)]
fn connect_once(path: &Path) -> io::Result<IpcConn> {
    // A Win32 named pipe client is just an OpenOptions read+write open of the
    // pipe path; the resulting File is a duplex byte stream like UnixStream.
    std::fs::OpenOptions::new().read(true).write(true).open(path)
}
