//! Turning a 4KHDHub download link into something mpv can open.
//!
//! Ported from MovieBox-Tui `src/providers/fourkhdhub/hubcloud.rs` (MIT OR
//! Apache-2.0). The site never links a file directly: a download row points at
//! a HubCloud `/drive/` page, which points at a `sportverse.` resolver page,
//! which finally lists the mirrors. HubDrive adds one more hop in front of that.
//!
//! Every URL that comes out of a scraped page is validated before it is
//! fetched or handed to a player — the pages are an untrusted source, and the
//! only reason to follow one of their links is to play video.

use super::dom;
use crate::stream::StreamError;
use std::net::IpAddr;

/// A resolved playback candidate: `(url, label)`, best first.
pub type Candidate = (String, String);

/// Follow a download link to playable URLs.
///
/// The chain taken depends on the host, and a link that is already a direct
/// file is passed through — the site does list a few of those.
pub async fn resolve(
    http: &reqwest::Client,
    url: &str,
    label: &str,
) -> Result<Vec<Candidate>, StreamError> {
    let host = host_of(url).unwrap_or_default();
    if host.contains("hubcloud.") {
        resolve_hubcloud(http, url).await
    } else if host.contains("hubdrive.") {
        resolve_hubdrive(http, url).await
    } else {
        Ok(vec![(playable(url)?, label.to_string())])
    }
}

/// HubCloud `/drive/` page → `sportverse.` resolver page → mirror list.
async fn resolve_hubcloud(
    http: &reqwest::Client,
    drive_url: &str,
) -> Result<Vec<Candidate>, StreamError> {
    require(drive_url, "hubcloud.", "/drive/")?;
    let drive_html = fetch(http, drive_url).await?;

    // Any https link on `a#download`, not one named host. The resolver has been
    // `sportverse.` and it will be something else next quarter — pinning the
    // name turned a routine domain rotation into "4KHDHub playback returns 404
    // and nothing else changed". The link is followed through `playable`
    // anyway, which is where the actual safety lives.
    let resolver_url = dom::find_all(&drive_html, "a", None)
        .into_iter()
        .filter(|a| dom::attr(a.attrs, "id").as_deref() == Some("download"))
        .filter_map(|a| dom::attr(a.attrs, "href"))
        .find(|href| href.starts_with("https://"))
        .ok_or(StreamError::ApiStatus(404))?;

    let html = fetch(http, &resolver_url).await?;

    // The best mirrors are only in a script variable, never in a link.
    let mut scored: Vec<(u8, String, String)> = script_pixeldrain_urls(&html)
        .into_iter()
        .map(|url| (0, url, "PixelDrain".to_string()))
        .collect();
    scored.extend(dom::find_all(&html, "a", None).into_iter().filter_map(|a| {
        let href = dom::attr(a.attrs, "href")?;
        let label = dom::text(a.inner);
        let url = playable(&href).ok()?;
        // PixelDrain's viewer page is not the file; its API path is.
        let url = pixeldrain_api_url(&url).unwrap_or(url);
        Some((score(&url, &label), url, tidy_label(&label)))
    }));

    scored.sort_by_key(|c| c.0);
    let mut out: Vec<Candidate> = Vec::new();
    for (_, url, label) in scored {
        if !out.iter().any(|(seen, _)| *seen == url) {
            out.push((url, label));
        }
    }
    if out.is_empty() {
        return Err(StreamError::ApiStatus(404));
    }
    Ok(out)
}

/// HubDrive `/file/` page → the HubCloud link it wraps → the chain above.
async fn resolve_hubdrive(
    http: &reqwest::Client,
    file_url: &str,
) -> Result<Vec<Candidate>, StreamError> {
    require(file_url, "hubdrive.", "/file/")?;
    let html = fetch(http, file_url).await?;
    let hubcloud = dom::find_all(&html, "a", None)
        .into_iter()
        .filter_map(|a| dom::attr(a.attrs, "href"))
        .find(|href| {
            host_of(href).map(|h| h.contains("hubcloud.")).unwrap_or(false)
                && href.contains("/drive/")
        })
        .ok_or(StreamError::ApiStatus(404))?;
    Box::pin(resolve_hubcloud(http, &hubcloud)).await
}

/// Confirm a candidate actually serves media, following the one redirect
/// wrapper the resolver pages like to put in the way.
///
/// A `Range: bytes=0-0` is enough: it costs one byte, settles the redirect
/// chain, and shows the content type. HTML or a zip coming back means the
/// "file" is another landing page, and handing that to mpv shows a blank window
/// rather than an error.
pub async fn preflight(http: &reqwest::Client, url: &str) -> Result<String, StreamError> {
    let url = playable(url)?;
    let resp = http.get(&url).header(reqwest::header::RANGE, "bytes=0-0").send().await?;
    if !resp.status().is_success() && !resp.status().is_redirection() {
        return Err(StreamError::ApiStatus(resp.status().as_u16()));
    }
    let final_url = playable(resp.url().as_str())?;
    if !is_media(&resp) {
        // Some wrappers carry the real file in a `link=` parameter.
        let wrapped = reqwest::Url::parse(&final_url)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "link")
                    .map(|(_, v)| v.into_owned())
            })
            .filter(|v| v.starts_with("https://"))
            .ok_or(StreamError::ApiStatus(415))?;
        let wrapped = playable(&wrapped)?;
        let resp = http.get(&wrapped).header(reqwest::header::RANGE, "bytes=0-0").send().await?;
        if !is_media(&resp) {
            return Err(StreamError::ApiStatus(415));
        }
        return playable(resp.url().as_str());
    }
    Ok(final_url)
}

fn is_media(resp: &reqwest::Response) -> bool {
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    !(ct.contains("text/html") || ct.contains("application/zip") || ct.contains("text/plain"))
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<String, StreamError> {
    let resp = http.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(StreamError::ApiStatus(resp.status().as_u16()));
    }
    Ok(resp.text().await?)
}

// ---- URL rules ----

/// A URL is only followed if it is https, names a public host, and does not
/// look like an archive or a session endpoint.
///
/// The private-address check is the important one: these URLs come from a
/// scraped page, and without it the page could point the app at something on
/// the machine's own network.
pub fn playable(raw: &str) -> Result<String, StreamError> {
    let url = reqwest::Url::parse(raw).map_err(|_| StreamError::ApiStatus(400))?;
    if url.scheme() != "https" {
        return Err(StreamError::ApiStatus(400));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path().to_ascii_lowercase();
    let private = host == "localhost"
        || host.ends_with(".local")
        || host.parse::<IpAddr>().is_ok_and(|ip| !is_public(ip));
    if host.is_empty()
        || private
        || path.ends_with(".zip")
        || path.contains("login.php")
        || path.contains("logout")
    {
        return Err(StreamError::ApiStatus(400));
    }
    Ok(url.to_string())
}

/// `host_marker` is matched anywhere in the host, not only at its start: the
/// site links `www.hubcloud.…` and regional prefixes as freely as the bare
/// name, and `starts_with` rejected those.
///
/// This is a routing check, not a safety one — it decides which hop chain to
/// run, and the page that supplied the URL is untrusted either way. Everything
/// that is actually handed to a player goes through [`playable`].
fn require(raw: &str, host_marker: &str, path_prefix: &str) -> Result<(), StreamError> {
    let url = reqwest::Url::parse(raw).map_err(|_| StreamError::ApiStatus(400))?;
    let ok = url.scheme() == "https"
        && url.host_str().unwrap_or_default().contains(host_marker)
        && url.path().starts_with(path_prefix);
    ok.then_some(()).ok_or(StreamError::ApiStatus(400))
}

fn host_of(raw: &str) -> Option<String> {
    reqwest::Url::parse(raw).ok()?.host_str().map(str::to_ascii_lowercase)
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            !(a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_unspecified())
        }
        IpAddr::V6(a) => {
            !(a.is_loopback()
                || a.is_unspecified()
                || a.is_unique_local()
                // fe80::/10 — the v6 half of the private-address guard, and the
                // one address family a link-local host is reachable on without
                // any routing at all.
                || a.is_unicast_link_local())
        }
    }
}

/// PixelDrain's viewer URL rewritten to its direct-download API path.
///
/// Any `pixeldrain.` host and either shape of path: the resolver serves `.dev`
/// and `.com` interchangeably and links both the viewer (`/u/`) and the file
/// (`/api/file/`). Matching one host and one path meant the fastest mirror on
/// the page was silently skipped whenever the page used the other spelling.
fn pixeldrain_api_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    if !host.contains("pixeldrain.") {
        return None;
    }
    let path = url.path();
    let id = match path.strip_prefix("/u/") {
        Some(rest) => rest,
        None => path.strip_prefix("/api/file/")?,
    }
    .trim_matches('/');
    let clean =
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    clean.then(|| format!("https://{host}/api/file/{id}?download"))
}

/// PixelDrain links the resolver page keeps in a script variable rather than in
/// a link. They are usually the fastest mirror, so they are worth digging out.
///
/// The whole page is scanned rather than only what follows `var pxl`: the
/// variable has been renamed before, and a prefix scan costs nothing since
/// every hit still has to survive [`pixeldrain_api_url`].
fn script_pixeldrain_urls(html: &str) -> Vec<String> {
    const PREFIXES: [&str; 4] = [
        "https://pixeldrain.dev/u/",
        "https://pixeldrain.com/u/",
        "https://pixeldrain.dev/api/file/",
        "https://pixeldrain.com/api/file/",
    ];
    let mut out: Vec<String> = Vec::new();
    for prefix in PREFIXES {
        let mut rest = html;
        while let Some(at) = rest.find(prefix) {
            let candidate = &rest[at..];
            // `<` and `\` end a URL too — the first because the script tag
            // closed, the second because JSON escaped the quote.
            let end = candidate
                .find(|c: char| {
                    c == '"' || c == '\'' || c.is_whitespace() || c == '<' || c == '\\'
                })
                .unwrap_or(candidate.len());
            if let Some(url) = pixeldrain_api_url(&candidate[..end])
                && !out.contains(&url)
            {
                out.push(url);
            }
            rest = &candidate[end..];
        }
    }
    out
}

/// Preference order between mirrors, lowest first. Learned from upstream: these
/// hosts differ by more than an order of magnitude in throughput.
fn score(url: &str, label: &str) -> u8 {
    let v = format!("{url} {label}").to_ascii_lowercase();
    if v.contains("pixeldrain") || v.contains("pixel.hubcloud") {
        0
    } else if v.contains("gpdl.") || v.contains("googleusercontent") {
        1
    } else if v.contains("workers.dev") || v.contains("r2.dev") {
        2
    } else if v.contains("latent.click") || v.contains("fsl") {
        3
    } else {
        4
    }
}

fn tidy_label(label: &str) -> String {
    let clean = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() { "Direct".into() } else { clean }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_media_urls_are_followed() {
        assert!(playable("https://cdn.example/file.mkv").is_ok());
        assert!(playable("http://cdn.example/file.mkv").is_err(), "plain http is refused");
        // The decisive case: a scraped page must not be able to aim the app at
        // the machine's own network.
        assert!(playable("https://127.0.0.1/file.mkv").is_err());
        assert!(playable("https://192.168.1.10/file.mkv").is_err());
        assert!(playable("https://localhost/file.mkv").is_err());
        assert!(playable("https://nas.local/file.mkv").is_err());
        assert!(playable("https://cdn.example/pack.zip").is_err());
        assert!(playable("https://cdn.example/logout").is_err());
        // fe80::/10 is private on the one interface that needs no routing.
        assert!(playable("https://[fe80::1]/file.mkv").is_err());
    }

    #[test]
    fn pixeldrain_viewer_links_are_rewritten_to_the_file() {
        assert_eq!(
            pixeldrain_api_url("https://pixeldrain.dev/u/aB3-x_9").as_deref(),
            Some("https://pixeldrain.dev/api/file/aB3-x_9?download"),
        );
        // Both spellings of the host, both shapes of path.
        assert_eq!(
            pixeldrain_api_url("https://pixeldrain.com/u/aB3-x_9").as_deref(),
            Some("https://pixeldrain.com/api/file/aB3-x_9?download"),
        );
        assert_eq!(
            pixeldrain_api_url("https://pixeldrain.com/api/file/xyz").as_deref(),
            Some("https://pixeldrain.com/api/file/xyz?download"),
        );
        assert!(pixeldrain_api_url("https://pixeldrain.dev/u/../etc").is_none());
        assert!(pixeldrain_api_url("https://elsewhere.dev/u/abc").is_none());
    }

    #[test]
    fn script_only_mirrors_are_recovered() {
        let html = r#"<script>var pxl1 = "https://pixeldrain.dev/u/aaa";
                      var anything = 'https://pixeldrain.com/u/bbb';
                      var esc = "https://pixeldrain.dev/u/ccc\";</script>"#;
        let found = script_pixeldrain_urls(html);
        // Not tied to `var pxl`, and the JSON-escaped quote ends the URL rather
        // than being swallowed into the id.
        assert!(found.contains(&"https://pixeldrain.dev/api/file/aaa?download".to_string()));
        assert!(found.contains(&"https://pixeldrain.com/api/file/bbb?download".to_string()));
        assert!(found.contains(&"https://pixeldrain.dev/api/file/ccc?download".to_string()));
    }

    #[test]
    fn mirror_preference_puts_the_fast_hosts_first() {
        assert!(score("https://pixeldrain.dev/api/file/x", "") < score("https://slow.host/x", ""));
        assert!(score("https://gpdl.example/x", "") < score("https://cdn.workers.dev/x", ""));
    }

    #[test]
    fn resolver_hops_insist_on_the_right_host_and_path() {
        assert!(require("https://hubcloud.one/drive/abc", "hubcloud.", "/drive/").is_ok());
        // A subdomain the site actually links has to route, not 400.
        assert!(require("https://www.hubcloud.one/drive/abc", "hubcloud.", "/drive/").is_ok());
        assert!(require("https://evil.example/drive/abc", "hubcloud.", "/drive/").is_err());
        assert!(require("https://hubcloud.one/other/abc", "hubcloud.", "/drive/").is_err());
        assert!(require("http://hubcloud.one/drive/abc", "hubcloud.", "/drive/").is_err());
    }
}
