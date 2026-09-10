//! A one-file HTTP origin, so a DLNA renderer can be handed a local track.
//!
//! Casting video never needed this: the Videos section casts streams, which
//! already have a URL a speaker can fetch. Music is local files, and a renderer
//! does not read your disk — it pulls over HTTP. So the file has to be
//! published for as long as the cast lasts, and unpublished after.
//!
//! Hand-rolled rather than axum, which the workspace does carry, because the
//! whole surface is one route with one method and a `Range` header. A framework
//! here would be more lines than the server.
//!
//! What keeps this from being a hole in the side of the machine:
//!
//! - It serves exactly one path at a time, and that path is a random 128-bit
//!   token. Nothing enumerable, nothing guessable, no directory.
//! - It serves exactly one *file* at a time — the one currently casting. There
//!   is no way to ask it for another, because the mapping is a single slot
//!   rather than a lookup.
//! - It stops when the cast stops.
//!
//! It binds `0.0.0.0` because the renderer is another machine on the network,
//! which is the entire point. That is the same exposure `tulipix-transfer`
//! takes, and unlike transfer this one holds nothing but the track you are
//! listening to.

use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The single published file and the token it answers to. The port is not on
/// here: it belongs to the listener, which outlives any one file.
struct Published {
    token: String,
    path: PathBuf,
}

fn slot() -> &'static Mutex<Option<Published>> {
    static S: OnceLock<Mutex<Option<Published>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// Whether the accept loop is already up. The listener outlives any one cast:
/// re-binding a port per track would make the renderer chase a moving target.
fn running() -> &'static std::sync::atomic::AtomicBool {
    static R: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    &R
}

/// A 128-bit path segment. `getrandom` rather than a counter or a hash of the
/// filename: the token is the only thing standing between the file and the
/// network, so it has to be unguessable rather than merely unique.
fn token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Never observed in practice; a time-based fallback is still far better
        // than a fixed string, and the alternative is refusing to cast.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes[..16].copy_from_slice(&nanos.to_le_bytes()[..16]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What a renderer should be told the file is. Guessed from the extension,
/// because that is all we know and all it needs — every renderer treats this as
/// a hint and sniffs the stream anyway.
fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "wma" => "audio/x-ms-wma",
        "aiff" | "aif" => "audio/aiff",
        _ => "audio/mpeg",
    }
}

/// This machine's address on the network the renderer is on.
fn lan_ip() -> Option<String> {
    tulipix_transfer::net::interfaces()
        .first()
        .map(|(_, ip)| ip.to_string())
}

/// Publish `path` and return the URL a renderer can fetch it from.
///
/// Replaces whatever was published before: one cast at a time, so the previous
/// track's token stops working the moment the next one starts.
pub async fn publish(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(anyhow!("that track is not on disk any more"));
    }
    let ip = lan_ip().ok_or_else(|| {
        anyhow!("this machine has no network address a speaker could reach")
    })?;

    let port = ensure_server().await?;
    let token = token();
    if let Ok(mut g) = slot().lock() {
        *g = Some(Published { token: token.clone(), path: path.to_path_buf() });
    }
    Ok(format!("http://{ip}:{port}/{token}"))
}

/// Stop publishing. The listener stays up — it costs one idle socket and saves
/// the next cast a round of port negotiation — but there is nothing behind it.
pub fn unpublish() {
    if let Ok(mut g) = slot().lock() {
        *g = None;
    }
}

/// The port the accept loop is on, once it is up.
static PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// Bind once, on an ephemeral port, and keep the accept loop for the process.
///
/// The listener outlives any one cast on purpose: a renderer that has been
/// handed `http://host:41234/…` and then finds the next track on a different
/// port is a renderer chasing a moving target.
async fn ensure_server() -> Result<u16> {
    if running().load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(PORT.load(std::sync::atomic::Ordering::SeqCst));
    }
    let listener = TcpListener::bind("0.0.0.0:0").await?;
    let port = listener.local_addr()?.port();
    PORT.store(port, std::sync::atomic::Ordering::SeqCst);
    running().store(true, std::sync::atomic::Ordering::SeqCst);

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    tokio::spawn(async move {
                        if let Err(e) = handle(stream, peer).await {
                            tracing::debug!(error = %e, %peer, "cast: request failed");
                        }
                    });
                }
                Err(e) => {
                    tracing::debug!(error = %e, "cast: accept failed");
                    running().store(false, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            }
        }
    });
    Ok(port)
}

/// The request line and headers, up to the blank line. Capped: a renderer
/// sends a few hundred bytes, and anything sending more is not one.
async fn read_head(stream: &mut TcpStream) -> Result<String> {
    const CAP: usize = 8 * 1024;
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while buf.len() < CAP {
        let n = stream.read(&mut byte).await?;
        if n == 0 {
            break;
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") || buf.ends_with(b"\n\n") {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `bytes=START-` or `bytes=START-END`. Anything else — multiple ranges, a
/// suffix range — is answered with the whole file, which is legal and is what
/// every renderer that sends those can cope with.
fn parse_range(head: &str, len: u64) -> Option<(u64, u64)> {
    let line = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))?;
    let spec = line.split_once('=')?.1.trim();
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let start: u64 = a.trim().parse().ok()?;
    if start >= len {
        return None;
    }
    let end = match b.trim() {
        "" => len - 1,
        v => v.parse::<u64>().ok()?.min(len - 1),
    };
    (end >= start).then_some((start, end))
}

async fn handle(mut stream: TcpStream, _peer: SocketAddr) -> Result<()> {
    let head = read_head(&mut stream).await?;
    let mut lines = head.lines();
    let request = lines.next().unwrap_or_default();
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or_default().to_ascii_uppercase();
    let target = parts.next().unwrap_or_default();

    // HEAD is not optional: renderers probe with it before they commit, and one
    // that gets a 405 decides the source is unplayable without ever asking for
    // the audio.
    if method != "GET" && method != "HEAD" {
        return respond_status(&mut stream, "405 Method Not Allowed").await;
    }
    let asked = target.trim_start_matches('/');

    // Resolved to an owned answer before anything is awaited. A `MutexGuard`
    // merely in lexical scope across an `.await` makes the whole future
    // non-`Send`, and this one is spawned — the same trap `api/mdl.rs` keeps
    // its `with()` helper for.
    let found = {
        let g = slot().lock().map_err(|_| anyhow!("poisoned"))?;
        match g.as_ref() {
            // Constant-time is overkill for a 128-bit token nobody can probe
            // faster than the network allows, but the comparison is on the
            // whole string rather than a prefix.
            Some(p) if p.token == asked => Some((p.path.clone(), mime_of(&p.path))),
            _ => None,
        }
    };
    let Some((path, mime)) = found else {
        return respond_status(&mut stream, "404 Not Found").await;
    };

    let mut file = tokio::fs::File::open(&path).await?;
    let len = file.metadata().await?.len();
    let range = parse_range(&head, len);

    let (status, start, count) = match range {
        Some((a, b)) => ("206 Partial Content", a, b - a + 1),
        None => ("200 OK", 0, len),
    };

    let mut headers = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {count}\r\n\
         Accept-Ranges: bytes\r\n\
         Connection: close\r\n\
         transferMode.dlna.org: Streaming\r\n"
    );
    if let Some((a, b)) = range {
        headers.push_str(&format!("Content-Range: bytes {a}-{b}/{len}\r\n"));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).await?;

    if method == "HEAD" {
        return Ok(stream.shutdown().await?);
    }
    if start > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    // The write fails when the renderer hangs up — a stop, a skip, a device
    // going to sleep — and that is a normal end rather than an error.
    let mut body = file.take(count);
    let _ = tokio::io::copy(&mut body, &mut stream).await;
    let _ = stream.shutdown().await;
    Ok(())
}

async fn respond_status(stream: &mut TcpStream, status: &str) -> Result<()> {
    stream
        .write_all(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let _ = stream.shutdown().await;
    Ok(())
}

// ── finding the speakers ────────────────────────────────────────────────────
//
// Lifted out of `vid_stream`, which had it to itself. Both sections ask the
// same question of the same protocol, and two copies of an SSDP loop is two
// places to fix a renderer that answers slowly.

/// Ask the network who can play things, and wait a couple of seconds for the
/// answers. Blocking: SSDP is a UDP round trip with no async story worth the
/// dependency, so callers put it on a blocking thread.
pub(crate) fn discover() -> Vec<tulipix_music::cast::CastDevice> {
    use std::net::UdpSocket;
    use tulipix_music::cast as dlna;
    let mut out: Vec<dlna::CastDevice> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out };
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(600)));
    let _ = sock.send_to(dlna::ssdp_msearch().as_bytes(), "239.255.255.250:1900");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        let Ok((n, _)) = sock.recv_from(&mut buf) else { continue };
        let raw = String::from_utf8_lossy(&buf[..n]);
        if let Some(dev) = dlna::parse_ssdp_response(&raw) {
            if !out.iter().any(|d| d.location == dev.location) {
                out.push(dev);
            }
        }
    }
    out
}

/// SSDP `SERVER` headers read like "Linux/4.9 UPnP/1.0 Sony/1.0" — keep the
/// part with a name in it rather than the kernel version.
pub(crate) fn short_name(raw: &str) -> String {
    let cleaned = raw
        .split_whitespace()
        .filter(|part| !part.starts_with("UPnP/") && !part.starts_with("Linux/"))
        .collect::<Vec<_>>()
        .join(" ");
    let name = if cleaned.trim().is_empty() { raw } else { &cleaned };
    let name = name.trim();
    if name.chars().count() <= 40 {
        name.to_string()
    } else {
        name.chars().take(40).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_loses_the_kernel_and_keeps_the_maker() {
        assert_eq!(short_name("Linux/4.9 UPnP/1.0 Sony/1.0"), "Sony/1.0");
        // Nothing left after filtering means the raw string was all version
        // strings, and a version string beats an empty row in a picker.
        assert_eq!(short_name("UPnP/1.0"), "UPnP/1.0");
        assert_eq!(short_name(&"x".repeat(80)).chars().count(), 40);
    }

    #[test]
    fn a_token_is_long_and_never_the_same_twice() {
        let a = token();
        let b = token();
        assert_eq!(a.len(), 32, "128 bits as hex");
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn mime_follows_the_extension_and_guesses_when_it_cannot() {
        assert_eq!(mime_of(Path::new("/m/a.flac")), "audio/flac");
        assert_eq!(mime_of(Path::new("/m/a.OPUS")), "audio/opus");
        assert_eq!(mime_of(Path::new("/m/a.m4a")), "audio/mp4");
        // Unknown is not "refuse": renderers sniff, and the header is a hint.
        assert_eq!(mime_of(Path::new("/m/a.weird")), "audio/mpeg");
        assert_eq!(mime_of(Path::new("/m/no-extension")), "audio/mpeg");
    }

    #[test]
    fn an_open_range_runs_to_the_end() {
        let head = "GET /x HTTP/1.1\r\nRange: bytes=100-\r\n\r\n";
        assert_eq!(parse_range(head, 1000), Some((100, 999)));
    }

    #[test]
    fn a_closed_range_is_clamped_to_the_file() {
        let head = "GET /x HTTP/1.1\r\nrange: bytes=10-99999\r\n\r\n";
        assert_eq!(parse_range(head, 1000), Some((10, 999)));
    }

    #[test]
    fn a_range_past_the_end_is_no_range_rather_than_an_empty_one() {
        // Answering 206 with zero bytes makes a renderer think the track is
        // over. Falling back to the whole file is the recoverable answer.
        let head = "GET /x HTTP/1.1\r\nRange: bytes=5000-\r\n\r\n";
        assert_eq!(parse_range(head, 1000), None);
    }

    #[test]
    fn the_shapes_we_do_not_serve_fall_back_to_the_whole_file() {
        let multi = "GET /x HTTP/1.1\r\nRange: bytes=0-10,20-30\r\n\r\n";
        assert_eq!(parse_range(multi, 1000), None);
        let suffix = "GET /x HTTP/1.1\r\nRange: bytes=-500\r\n\r\n";
        assert_eq!(parse_range(suffix, 1000), None);
        let none = "GET /x HTTP/1.1\r\nHost: a\r\n\r\n";
        assert_eq!(parse_range(none, 1000), None);
    }
}
