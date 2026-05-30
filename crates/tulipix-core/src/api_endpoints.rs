//! Configurable backend endpoints.
//!
//! Three switches live here, all opt-in:
//!   * `sync_url`        — Account mode endpoint override (self-host vs hosted).
//!     Validation = HTTPS scheme + /health ping returning 200.
//!   * `update_channel`  — Appcast / model-manifest URL. Air-gap + mirror
//!     support. Updates must carry an Ed25519 detached signature pinned to
//!     the public key shipped in the binary; verification is performed
//!     before any artifact is installed.
//!   * `sentry_dsn`      — Custom crash service. Default is the
//!     Tulipix-managed DSN (off until the user opts in via Settings →
//!     Privacy → Crash reporting).
//!
//! This module owns parsing, defaults, and pre-flight checks. The actual
//! HTTP / signature work happens behind the `reqwest`/sodium calls in the
//! sync, updater, and crash crates.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_SYNC_URL:        &str = "https://sync.tulipix.app";
pub const DEFAULT_UPDATE_CHANNEL:  &str = "https://updates.tulipix.app/appcast.json";
pub const DEFAULT_SENTRY_DSN:      &str = ""; // empty = disabled until user opts in

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointConfig {
    #[serde(default = "default_sync_url")]
    pub sync_url: String,
    #[serde(default = "default_update_channel")]
    pub update_channel: String,
    #[serde(default = "default_sentry_dsn")]
    pub sentry_dsn: String,
    /// Pinned Ed25519 public key for `update_channel`. Hex-encoded. When
    /// empty the updater refuses to install — air-gapped mirrors must
    /// supply the key out of band.
    #[serde(default)]
    pub update_signing_pubkey_hex: String,
}

fn default_sync_url()       -> String { DEFAULT_SYNC_URL.into() }
fn default_update_channel() -> String { DEFAULT_UPDATE_CHANNEL.into() }
fn default_sentry_dsn()     -> String { DEFAULT_SENTRY_DSN.into() }

impl Default for EndpointConfig {
    fn default() -> Self {
        Self {
            sync_url: default_sync_url(),
            update_channel: default_update_channel(),
            sentry_dsn: default_sentry_dsn(),
            update_signing_pubkey_hex: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind { Sync, Update, Sentry }

/// Cheap structural validation. Network probe lives in `health_check`.
pub fn validate_url(kind: EndpointKind, url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        if matches!(kind, EndpointKind::Sentry) { return Ok(()); } // empty disables crash uploader
        return Err(anyhow!("endpoint URL is empty"));
    }
    let lower = url.to_ascii_lowercase();
    if matches!(kind, EndpointKind::Sentry) {
        // Sentry DSN: https://<key>@host/<project>
        if !lower.starts_with("https://") { return Err(anyhow!("Sentry DSN must be HTTPS")); }
        if !url.contains('@') { return Err(anyhow!("Sentry DSN missing key (expected key@host/project)")); }
        return Ok(());
    }
    if !(lower.starts_with("https://") || lower.starts_with("http://localhost") || lower.starts_with("http://127.")) {
        return Err(anyhow!("endpoint must use HTTPS (localhost http is permitted)"));
    }
    Ok(())
}

pub fn validate(cfg: &EndpointConfig) -> Vec<String> {
    let mut warns = Vec::new();
    if let Err(e) = validate_url(EndpointKind::Sync, &cfg.sync_url) { warns.push(format!("sync_url: {e}")); }
    if let Err(e) = validate_url(EndpointKind::Update, &cfg.update_channel) { warns.push(format!("update_channel: {e}")); }
    if let Err(e) = validate_url(EndpointKind::Sentry, &cfg.sentry_dsn) { warns.push(format!("sentry_dsn: {e}")); }
    if cfg.update_channel != DEFAULT_UPDATE_CHANNEL && cfg.update_signing_pubkey_hex.is_empty() {
        warns.push("update_channel: custom mirror requires `update_signing_pubkey_hex` (Ed25519 32-byte hex)".into());
    }
    if !cfg.update_signing_pubkey_hex.is_empty() && parse_ed25519_pubkey(&cfg.update_signing_pubkey_hex).is_err() {
        warns.push("update_signing_pubkey_hex: must be exactly 64 hex chars (32-byte Ed25519 public key)".into());
    }
    warns
}

pub fn parse_ed25519_pubkey(hex: &str) -> Result<[u8; 32]> {
    let h = hex.trim();
    if h.len() != 64 { return Err(anyhow!("expected 64 hex chars, got {}", h.len())); }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let byte = u8::from_str_radix(&h[i*2..i*2+2], 16).map_err(|_| anyhow!("invalid hex at byte {i}"))?;
        out[i] = byte;
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthOutcome { Ok, BadStatus(u16), Unreachable }

/// Plug a transport for testability. Production wiring uses `reqwest`
/// from the sync crate; tests can pass a stub.
pub trait HealthProbe: Send + Sync {
    fn probe(&self, url: &str) -> HealthOutcome;
}

pub fn health_check(probe: &dyn HealthProbe, base_url: &str) -> Result<()> {
    let trimmed = base_url.trim_end_matches('/');
    let url = format!("{trimmed}/health");
    match probe.probe(&url) {
        HealthOutcome::Ok                 => Ok(()),
        HealthOutcome::BadStatus(code)    => Err(anyhow!("/health returned {code}")),
        HealthOutcome::Unreachable        => Err(anyhow!("/health unreachable")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct OkProbe;       impl HealthProbe for OkProbe       { fn probe(&self, _: &str) -> HealthOutcome { HealthOutcome::Ok } }
    struct BadProbe(u16); impl HealthProbe for BadProbe      { fn probe(&self, _: &str) -> HealthOutcome { HealthOutcome::BadStatus(self.0) } }
    struct DeadProbe;     impl HealthProbe for DeadProbe     { fn probe(&self, _: &str) -> HealthOutcome { HealthOutcome::Unreachable } }

    #[test] fn defaults_validate_clean_when_pubkey_supplied_or_default_url() {
        let cfg = EndpointConfig::default();
        assert!(validate(&cfg).is_empty(), "{:?}", validate(&cfg));
    }
    #[test] fn empty_sentry_dsn_is_allowed() {
        validate_url(EndpointKind::Sentry, "").unwrap();
        validate_url(EndpointKind::Sentry, "https://abc@sentry.io/123").unwrap();
        assert!(validate_url(EndpointKind::Sentry, "http://abc@sentry.io/123").is_err());
        assert!(validate_url(EndpointKind::Sentry, "https://sentry.io/123").is_err());
    }
    #[test] fn requires_https_for_sync_and_update() {
        assert!(validate_url(EndpointKind::Sync, "http://example.com").is_err());
        validate_url(EndpointKind::Sync, "https://example.com").unwrap();
        validate_url(EndpointKind::Sync, "http://localhost:8080").unwrap();
        validate_url(EndpointKind::Sync, "http://127.0.0.1:8080").unwrap();
    }
    #[test] fn custom_mirror_requires_pubkey() {
        let cfg = EndpointConfig { update_channel: "https://mirror.example/cast.json".into(), ..Default::default() };
        let w = validate(&cfg);
        assert!(w.iter().any(|s| s.contains("update_signing_pubkey_hex")));
    }
    #[test] fn pubkey_hex_parser_round_trips() {
        let pk = "a".repeat(64);
        let bytes = parse_ed25519_pubkey(&pk).unwrap();
        assert_eq!(bytes, [0xAA; 32]);
        assert!(parse_ed25519_pubkey("abc").is_err());
        assert!(parse_ed25519_pubkey(&"z".repeat(64)).is_err());
    }
    #[test] fn health_check_branches() {
        health_check(&OkProbe, "https://sync.example").unwrap();
        let e = health_check(&BadProbe(503), "https://sync.example").unwrap_err();
        assert!(e.to_string().contains("503"));
        let e = health_check(&DeadProbe, "https://sync.example").unwrap_err();
        assert!(e.to_string().contains("unreachable"));
    }
}
