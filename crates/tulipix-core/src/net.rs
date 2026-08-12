//! Shared HTTP clients.
//!
//! Every call site used to build its own `reqwest::Client::new()` — 62 of them.
//! That has two costs. `Client::new()` sets **no timeout of any kind**, so a
//! host that accepts the connection and then goes silent parks the task
//! forever; a download queue or a metadata fetch waiting on it has no way to
//! notice. And a fresh client means a fresh connection pool, so the many small
//! requests these paths make to the same host each pay a new TCP and TLS
//! handshake.
//!
//! A `reqwest::Client` is already an `Arc` internally and is designed to be
//! created once and reused, so these are process-wide.
//!
//! Two clients, because one timeout policy does not fit both shapes:
//!
//! * [`http`] — API calls, metadata, artwork, subtitle tracks. Small, bounded
//!   responses, so a total timeout is right.
//! * [`http_stream`] — file transfers. A total timeout is *wrong* here: it
//!   would abort a large but perfectly healthy download partway through. What
//!   should be bounded is silence, so this one caps connect time and the gap
//!   between reads, with no ceiling on overall duration.

use std::sync::OnceLock;
use std::time::Duration;

/// Agent string for third-party media hosts.
///
/// Podcast CDNs (Megaphone, Libsyn, rss.com, Buzzsprout), the image hosts their
/// artwork sits on, and most Icecast/Shoutcast radio servers run a bot filter in
/// front of the file. An agent they do not recognise is answered with `403
/// Forbidden`, and a few reset the connection outright — which is what the
/// "connection refused on 443" and "403" reports on Podcasts and Radio are.
/// `Tulipix/0.1 (github…)` reads as a crawler to every one of those filters.
///
/// So feeds, episode audio, artwork and radio streams identify as a browser.
/// Hosts that *want* an identifying agent — MusicBrainz, radio-browser, LRCLIB —
/// keep sending their own; this is only for the media hosts that block us.
pub const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// Client for ordinary requests: bounded end to end.
///
/// Use for anything whose response is small enough that taking more than a few
/// seconds means something is wrong.
pub fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        build(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .connect_timeout(Duration::from_secs(8)),
        )
    })
}

/// Client for large transfers: bounded silence, unbounded length.
///
/// Caps the connect and the wait between reads. A slow-but-alive server keeps
/// its download; a server that stops sending is dropped.
pub fn http_stream() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        build(
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(30)),
        )
    })
}

/// Shared tail of both builders: connection reuse, and a fall back to a default
/// client if the TLS backend cannot be initialised — a request that then fails
/// is better than a panic at first use.
fn build(b: reqwest::ClientBuilder) -> reqwest::Client {
    b.pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(4)
        .tcp_keepalive(Duration::from_secs(30))
        .user_agent(concat!("tulipix/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_are_reused_not_rebuilt() {
        // Same instance each call: the pool is the point.
        assert!(std::ptr::eq(http(), http()));
        assert!(std::ptr::eq(http_stream(), http_stream()));
    }

    #[test]
    fn the_two_clients_are_distinct() {
        assert!(!std::ptr::eq(http(), http_stream()));
    }
}
