//! `np.p4.cloud.http` — no-mount HTTP fallback when FUSE/WinFsp is unavailable.
//!
//! Serves a remote over local HTTP (`rclone serve http`) so the app can read
//! it by URL when a kernel mount isn't possible (no FUSE, no WinFsp, locked-down
//! OS). Owns the argv, the local URL, and the fallback decision.

/// Should we fall back to HTTP serve? True when neither mount backend is usable.
pub fn should_fallback(fuse_available: bool, winfsp_available: bool) -> bool {
    !(fuse_available || winfsp_available)
}

/// `rclone serve http remote: --addr 127.0.0.1:PORT` argv (loopback only).
pub fn serve_args(remote: &str, port: u16) -> Vec<String> {
    vec![
        "serve".into(), "http".into(),
        format!("{remote}:"),
        "--addr".into(), format!("127.0.0.1:{port}"),
        "--read-only".into(),
    ]
}

/// Base URL the app reads files from once the serve is up.
pub fn base_url(port: u16) -> String { format!("http://127.0.0.1:{port}") }

/// Full URL for a remote path under the serve root (path-encoded).
pub fn file_url(port: u16, remote_path: &str) -> String {
    let enc = remote_path.split('/').map(|seg| {
        seg.bytes().map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        }).collect::<String>()
    }).collect::<Vec<_>>().join("/");
    format!("{}/{}", base_url(port), enc.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_only_when_no_mount() {
        assert!(should_fallback(false, false));
        assert!(!should_fallback(true, false));
        assert!(!should_fallback(false, true));
    }

    #[test]
    fn serve_binds_loopback() {
        let a = serve_args("gdrive", 8088);
        assert!(a.contains(&"127.0.0.1:8088".to_string()));
        assert!(a.contains(&"--read-only".to_string()));
    }

    #[test]
    fn file_url_encodes_spaces() {
        assert_eq!(file_url(8088, "/Movies/My Film.mkv"), "http://127.0.0.1:8088/Movies/My%20Film.mkv");
    }
}
