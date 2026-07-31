//! 4KHDHub — the Stream tab's second catalogue.
//!
//! Ported from MovieBox-Tui v0.1.7 `src/providers/fourkhdhub` (MIT OR
//! Apache-2.0, https://github.com/mesamirh/MovieBox-Tui).
//!
//! Nothing about this source is an API. It is a scraped site, so a page-layout
//! change breaks it in a way no amount of care here prevents — which is exactly
//! why it is a *second* source rather than a replacement, and why the base URL
//! is user-editable the same way the MovieBox host pool is.

pub mod dom;
mod hubcloud;
mod parse;

pub use hubcloud::playable;
pub use parse::Release;

use crate::stream::{Details, SearchHit, StreamError, StreamFile};

/// First-run base URL. Editable from the Stream tab → Servers, because this
/// site changes domain roughly as often as the MovieBox hosts rotate.
pub const DEFAULT_BASE: &str = "https://4khdhub.one/";

/// Settings key for the base URL override.
pub const BASE_KEY: &str = "stream.fourk_base";

/// The configured base, or [`DEFAULT_BASE`]. Trailing slash guaranteed, so
/// joining a path is concatenation and not a special case.
pub fn base() -> String {
    let stored = tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(BASE_KEY).cloned())
        .unwrap_or_default();
    let raw = stored.trim();
    let raw = if raw.is_empty() { DEFAULT_BASE } else { raw };
    // Only https: everything this site hands back is a link that will be
    // followed, and a plaintext hop is one an ISP can rewrite.
    if !raw.starts_with("https://") {
        return DEFAULT_BASE.to_string();
    }
    if raw.ends_with('/') { raw.to_string() } else { format!("{raw}/") }
}

/// Store a base URL override. Blank clears it back to [`DEFAULT_BASE`].
pub fn set_base(url: &str) -> Result<(), String> {
    let raw = url.trim();
    if !raw.is_empty() && !raw.starts_with("https://") {
        return Err("The address has to start with https://".into());
    }
    let mut s = tulipix_core::settings::Settings::load().map_err(|e| e.to_string())?;
    if raw.is_empty() {
        s.advanced.remove(BASE_KEY);
    } else {
        s.advanced.insert(BASE_KEY.to_string(), raw.to_string());
    }
    s.save().map_err(|e| e.to_string())
}

#[derive(Clone)]
pub struct FourKClient {
    http: reqwest::Client,
    base: String,
}

impl FourKClient {
    pub fn new() -> Result<Self, StreamError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .connect_timeout(std::time::Duration::from_secs(5))
            // A scraped site serves a different page to something that does not
            // look like a browser.
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        Ok(Self { http, base: base() })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn host(&self) -> String {
        reqwest::Url::parse(&self.base)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default()
    }

    /// Resolve a scraped path against the base, refusing anything that would
    /// leave the site.
    fn page_url(&self, path: &str) -> Result<String, StreamError> {
        let base = reqwest::Url::parse(&self.base).map_err(|_| StreamError::NoHosts)?;
        let joined = base.join(path.trim_start_matches('/')).map_err(|_| StreamError::NoHosts)?;
        if joined.host_str() != base.host_str() {
            return Err(StreamError::NoHosts);
        }
        Ok(joined.to_string())
    }

    async fn page(&self, url: &str) -> Result<String, StreamError> {
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(StreamError::ApiStatus(resp.status().as_u16()));
        }
        Ok(resp.text().await?)
    }

    /// The site answers a whole search on one page, so anything past page one is
    /// empty rather than a second request that would return the same results.
    pub async fn search(&self, query: &str, page: usize) -> Result<Vec<SearchHit>, StreamError> {
        if page > 1 {
            return Ok(Vec::new());
        }
        let mut url = reqwest::Url::parse(&self.base).map_err(|_| StreamError::NoHosts)?;
        url.query_pairs_mut().append_pair("s", query);
        let html = self.page(url.as_str()).await?;
        Ok(parse::parse_search(&html, &self.host()))
    }

    pub async fn details(&self, id: &str) -> Result<Details, StreamError> {
        let html = self.page(&self.page_url(id)?).await?;
        parse::parse_details(id, &html).ok_or(StreamError::ApiStatus(404))
    }

    /// Files for one episode; `season`/`episode` both 0 for a film.
    ///
    /// `resolution` narrows client-side, matching how the MovieBox client
    /// behaves — an empty string keeps every rung.
    pub async fn resources(
        &self,
        id: &str,
        season: usize,
        episode: usize,
        resolution: &str,
    ) -> Result<Vec<StreamFile>, StreamError> {
        let html = self.page(&self.page_url(id)?).await?;
        let releases = parse::parse_releases(&html, season as i64, episode as i64);
        let mut files = parse::releases_to_files(&releases);
        if let Ok(want) = resolution.trim().parse::<i64>() {
            files.retain(|f| f.resolution == want);
        }
        Ok(files)
    }

    /// Follow a download link to something mpv can actually open.
    ///
    /// Called at play time rather than at listing time: each link is a two- or
    /// three-hop chain, and resolving twenty of them to paint a quality list
    /// would cost twenty chains to use one.
    pub async fn playable_url(&self, resolver_url: &str) -> Result<String, StreamError> {
        let candidates = hubcloud::resolve(&self.http, resolver_url, "Source").await?;
        let mut last = StreamError::HostsExhausted;
        for (url, _) in candidates {
            match hubcloud::preflight(&self.http, &url).await {
                Ok(playable) => return Ok(playable),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_always_ends_in_a_slash_so_joining_is_concatenation() {
        // `base()` reads settings, so exercise the normalisation directly.
        let normalise = |raw: &str| {
            if !raw.starts_with("https://") {
                DEFAULT_BASE.to_string()
            } else if raw.ends_with('/') {
                raw.to_string()
            } else {
                format!("{raw}/")
            }
        };
        assert_eq!(normalise("https://x.dev"), "https://x.dev/");
        assert_eq!(normalise("https://x.dev/"), "https://x.dev/");
        // A plaintext override is refused rather than followed.
        assert_eq!(normalise("http://x.dev"), DEFAULT_BASE);
    }

    #[test]
    fn a_scraped_path_cannot_walk_off_the_site() {
        let c = FourKClient { http: reqwest::Client::new(), base: DEFAULT_BASE.into() };
        assert!(c.page_url("/movie/dune").is_ok());
        assert!(c.page_url("movie/dune").is_ok(), "a bare path is still same-site");
        assert!(c.page_url("https://evil.example/x").is_err());
    }

    #[test]
    fn setting_a_plaintext_base_is_refused_with_a_reason() {
        assert!(set_base("http://x.dev").is_err());
    }
}
