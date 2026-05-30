//! Third-party metadata + lookup providers. Two flavours:
//!   * MirrorEndpoint  — open service with optional self-host / mirror URL
//!     and a built-in per-service request budget (Nominatim, radio-browser,
//!     AutoEq DB, TMDB image base).
//!   * ScraperProvider — keyed integration (Discogs, AniDB, AniList,
//!     Subscene/Addic7ed, Trakt.tv, ListenBrainz). Stores the user key in
//!     the OS keychain at `tulipix.api.<id>`; never in settings.json.
//!
//! Pure config + rate-limit accounting + TOS gate. Network IO lives in the
//! per-section crates that actually invoke the providers.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ─── Mirror endpoints ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MirrorService { Nominatim, RadioBrowser, AutoEq, TmdbImageBase }

impl MirrorService {
    pub fn id(self) -> &'static str {
        match self {
            Self::Nominatim     => "nominatim",
            Self::RadioBrowser  => "radio-browser",
            Self::AutoEq        => "autoeq",
            Self::TmdbImageBase => "tmdb-image-base",
        }
    }
    pub fn default_url(self) -> &'static str {
        match self {
            Self::Nominatim     => "https://nominatim.openstreetmap.org",
            Self::RadioBrowser  => "https://all.api.radio-browser.info",
            Self::AutoEq        => "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master",
            Self::TmdbImageBase => "https://image.tmdb.org/t/p",
        }
    }
    /// Public-instance rate cap. Mirrors that require manual override
    /// (radio-browser, AutoEq) carry `None` here — caller-side budgets only
    /// kick in when the URL points back at the public default.
    pub fn public_rate_per_sec(self) -> Option<u32> {
        match self {
            Self::Nominatim     => Some(1),   // Nominatim TOS: 1 req/sec
            Self::RadioBrowser  => Some(5),
            Self::AutoEq        => None,      // raw.githubusercontent.com — GitHub limits
            Self::TmdbImageBase => None,      // CDN
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorEndpoint {
    pub service_id: String,
    pub url: String,
}

impl MirrorEndpoint {
    pub fn defaults() -> Vec<Self> {
        [MirrorService::Nominatim, MirrorService::RadioBrowser, MirrorService::AutoEq, MirrorService::TmdbImageBase]
            .into_iter()
            .map(|s| Self { service_id: s.id().into(), url: s.default_url().into() })
            .collect()
    }
    pub fn is_default(&self, service: MirrorService) -> bool { self.url == service.default_url() }
}

/// Mini token-bucket honouring per-service rate caps. Mirrors with a
/// custom URL bypass the cap entirely (self-host owns its own throttle).
#[derive(Debug)]
pub struct RateBudget {
    pub service: MirrorService,
    pub min_interval: Option<Duration>,
    last_unix_ms: std::sync::Mutex<Option<u64>>,
}

impl RateBudget {
    pub fn for_endpoint(service: MirrorService, ep: &MirrorEndpoint) -> Self {
        let cap = if ep.is_default(service) { service.public_rate_per_sec() } else { None };
        let min_interval = cap.map(|n| Duration::from_millis((1000 / n.max(1)) as u64));
        Self { service, min_interval, last_unix_ms: std::sync::Mutex::new(None) }
    }
    /// Returns `Ok(wait)` — caller sleeps `wait` before issuing the request.
    /// `wait == 0` means dispatch immediately.
    pub fn acquire(&self, now_ms: u64) -> Duration {
        let Some(interval) = self.min_interval else { return Duration::ZERO; };
        let mut slot = self.last_unix_ms.lock().unwrap();
        let Some(last) = *slot else { *slot = Some(now_ms); return Duration::ZERO; };
        let earliest = last + interval.as_millis() as u64;
        if now_ms >= earliest { *slot = Some(now_ms); return Duration::ZERO; }
        let wait = Duration::from_millis(earliest - now_ms);
        *slot = Some(earliest);
        wait
    }
}

// ─── Keyed scrapers ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScraperService {
    Discogs,
    AniDb,
    AniList,
    Subscene,
    Addic7ed,
    Trakt,
    ListenBrainz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind { ApiKey, OAuth, None }

#[derive(Debug, Clone)]
pub struct ScraperDescriptor {
    pub service: ScraperService,
    pub id: &'static str,
    pub auth: AuthKind,
    /// True when the provider must stay off until the user accepts the
    /// terms-of-service notice (scrapers in legal grey areas).
    pub requires_tos_optin: bool,
    pub keychain_entry: &'static str,
}

impl ScraperService {
    pub fn descriptor(self) -> ScraperDescriptor {
        match self {
            Self::Discogs      => ScraperDescriptor { service: self, id: "discogs",      auth: AuthKind::ApiKey, requires_tos_optin: false, keychain_entry: "tulipix.api.discogs" },
            Self::AniDb        => ScraperDescriptor { service: self, id: "anidb",        auth: AuthKind::ApiKey, requires_tos_optin: false, keychain_entry: "tulipix.api.anidb" },
            Self::AniList      => ScraperDescriptor { service: self, id: "anilist",      auth: AuthKind::OAuth,  requires_tos_optin: false, keychain_entry: "tulipix.api.anilist" },
            Self::Subscene     => ScraperDescriptor { service: self, id: "subscene",     auth: AuthKind::None,   requires_tos_optin: true,  keychain_entry: "tulipix.api.subscene" },
            Self::Addic7ed     => ScraperDescriptor { service: self, id: "addic7ed",     auth: AuthKind::None,   requires_tos_optin: true,  keychain_entry: "tulipix.api.addic7ed" },
            Self::Trakt        => ScraperDescriptor { service: self, id: "trakt",        auth: AuthKind::OAuth,  requires_tos_optin: false, keychain_entry: "tulipix.api.trakt" },
            Self::ListenBrainz => ScraperDescriptor { service: self, id: "listenbrainz", auth: AuthKind::ApiKey, requires_tos_optin: false, keychain_entry: "tulipix.api.listenbrainz" },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScraperConfig {
    /// True only after the user explicitly accepts the TOS card in
    /// Settings → Privacy. Default false — scrapers refuse to run.
    pub tos_accepted: std::collections::BTreeSet<String>,
    /// Disabled-by-default services the user has turned on.
    pub enabled: std::collections::BTreeSet<String>,
}

pub fn require_enabled(cfg: &ScraperConfig, service: ScraperService) -> Result<()> {
    let d = service.descriptor();
    if d.requires_tos_optin && !cfg.tos_accepted.contains(d.id) {
        return Err(anyhow!("scraper {} requires TOS acceptance", d.id));
    }
    if !cfg.enabled.contains(d.id) && d.requires_tos_optin {
        return Err(anyhow!("scraper {} is disabled by default", d.id));
    }
    Ok(())
}

/// Fallback order for music metadata: MusicBrainz primary, Discogs +
/// ListenBrainz as fallbacks when MusicBrainz misses.
pub fn music_metadata_fallback() -> &'static [ScraperService] {
    &[ScraperService::Discogs, ScraperService::ListenBrainz]
}

/// Anime metadata: AniDB primary (more episode coverage), AniList second.
pub fn anime_metadata_fallback() -> &'static [ScraperService] {
    &[ScraperService::AniDb, ScraperService::AniList]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn nominatim_caps_at_one_per_sec() {
        let ep = MirrorEndpoint { service_id: "nominatim".into(), url: MirrorService::Nominatim.default_url().into() };
        let b = RateBudget::for_endpoint(MirrorService::Nominatim, &ep);
        assert_eq!(b.acquire(0), Duration::ZERO);
        let w = b.acquire(500);
        assert_eq!(w, Duration::from_millis(500));
        assert_eq!(b.acquire(2000), Duration::ZERO);
    }
    #[test] fn self_host_url_bypasses_rate_limit() {
        let ep = MirrorEndpoint { service_id: "nominatim".into(), url: "https://nominatim.my-lan.local".into() };
        let b = RateBudget::for_endpoint(MirrorService::Nominatim, &ep);
        assert!(b.min_interval.is_none());
        assert_eq!(b.acquire(0), Duration::ZERO);
        assert_eq!(b.acquire(1), Duration::ZERO);
    }
    #[test] fn defaults_round_trip_each_service() {
        let defaults = MirrorEndpoint::defaults();
        assert_eq!(defaults.len(), 4);
        assert!(defaults.iter().any(|e| e.service_id == "nominatim" && e.url.contains("openstreetmap")));
        assert!(defaults.iter().any(|e| e.service_id == "tmdb-image-base" && e.url.contains("tmdb.org")));
    }
    #[test] fn scraper_descriptors_have_keychain_prefix() {
        for s in [ScraperService::Discogs, ScraperService::AniDb, ScraperService::AniList, ScraperService::Subscene, ScraperService::Addic7ed, ScraperService::Trakt, ScraperService::ListenBrainz] {
            let d = s.descriptor();
            assert!(d.keychain_entry.starts_with("tulipix.api."), "{}", d.id);
        }
    }
    #[test] fn subscene_requires_tos_optin() {
        let cfg = ScraperConfig::default();
        assert!(require_enabled(&cfg, ScraperService::Subscene).is_err());
        let mut cfg = ScraperConfig::default();
        cfg.tos_accepted.insert("subscene".into());
        cfg.enabled.insert("subscene".into());
        require_enabled(&cfg, ScraperService::Subscene).unwrap();
    }
    #[test] fn open_scraper_runs_without_optin() {
        let cfg = ScraperConfig::default();
        require_enabled(&cfg, ScraperService::Discogs).unwrap();
    }
    #[test] fn fallback_chains_documented() {
        assert!(music_metadata_fallback().contains(&ScraperService::Discogs));
        assert!(anime_metadata_fallback().contains(&ScraperService::AniDb));
    }
}
