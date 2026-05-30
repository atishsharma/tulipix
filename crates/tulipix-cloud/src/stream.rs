//! `np.p4.cloud.stream` — stream cloud video/audio via libmpv.
//!
//! Spins up `rclone serve http` for the remote, then hands mpv the resulting
//! URL. HTTP range requests give mpv seek. This owns the serve argv (reused
//! from [`crate::http`] but tuned for streaming) and the mpv option set that
//! enables byte-range seeking over HTTP.

use crate::http;

/// `rclone serve http` argv tuned for media streaming (no read-only restriction
/// difference, but larger read-ahead for smooth playback).
pub fn serve_args(remote: &str, port: u16) -> Vec<String> {
    let mut a = http::serve_args(remote, port);
    a.push("--vfs-read-chunk-size".into());
    a.push("32M".into());
    a
}

/// mpv options to play a streamed URL with seek over HTTP ranges.
pub fn mpv_options(url: &str) -> Vec<String> {
    vec![
        format!("--stream-lavf-o=seekable=1"),
        "--cache=yes".into(),
        "--demuxer-seekable-cache=yes".into(),
        url.to_string(),
    ]
}

/// Build the playable URL for a media path on the serve.
pub fn media_url(port: u16, remote_path: &str) -> String {
    http::file_url(port, remote_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_adds_chunk_readahead() {
        let a = serve_args("gdrive", 8089);
        assert!(a.windows(2).any(|w| w == ["--vfs-read-chunk-size", "32M"]));
    }

    #[test]
    fn mpv_opts_enable_seek() {
        let o = mpv_options("http://127.0.0.1:8089/a.mkv");
        assert!(o.iter().any(|s| s.contains("seekable=1")));
        assert_eq!(o.last().unwrap(), "http://127.0.0.1:8089/a.mkv");
    }

    #[test]
    fn url_via_http_module() {
        assert_eq!(media_url(8089, "/v/clip.mp4"), "http://127.0.0.1:8089/v/clip.mp4");
    }
}
