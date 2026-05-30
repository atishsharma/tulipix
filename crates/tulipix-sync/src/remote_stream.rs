//! WAN remote streaming — host's libmpv-backed media is reachable from any
//! signed-in device via an HTTPS endpoint at `<sync-base>/stream/<token>`.
//!
//! This module owns the bearer-token lifecycle, the bandwidth-probe → HLS
//! variant decision, and the per-token usage ledger. The actual TLS server
//! lives in the sync backend; the local side hands the backend a signed
//! manifest of what it's willing to serve and a fresh token.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamToken {
    pub token: String,
    /// SHA-256 of the manifest the host signed at issue time. The backend
    /// rejects mismatching playback requests so a leaked URL can't re-target
    /// to a different file.
    pub manifest_sha256: String,
    pub issued_at: i64,
    pub expires_at: i64,
    /// Scope — which item the token grants. Tokens are single-asset by
    /// default so revoking one doesn't black-hole the whole library.
    pub item_id: i64,
    pub user_id: u64,
}

impl StreamToken {
    pub fn is_valid(&self, now: i64) -> bool {
        now >= self.issued_at && now < self.expires_at
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Mints a fresh token over the given manifest. The token itself is a
/// 256-bit random hex string; we don't sign it directly (the backend does
/// that out-of-band over its mTLS link), so this just builds the local
/// record. `entropy` is supplied by the caller so unit tests can run
/// deterministic seeds without `rand` in the dep tree.
pub fn issue_token(
    manifest_json: &str,
    item_id: i64,
    user_id: u64,
    ttl_secs: i64,
    entropy: &[u8; 32],
) -> StreamToken {
    let mut h = Sha256::new();
    h.update(manifest_json.as_bytes());
    let manifest_sha = hex(&h.finalize());
    let token = hex(entropy);
    let issued_at = now_secs();
    StreamToken {
        token,
        manifest_sha256: manifest_sha,
        issued_at,
        expires_at: issued_at + ttl_secs.max(60),
        item_id,
        user_id,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Debug, Default)]
pub struct TokenStore {
    pub tokens: HashMap<String, StreamToken>,
}

impl TokenStore {
    pub fn insert(&mut self, t: StreamToken) {
        self.tokens.insert(t.token.clone(), t);
    }
    pub fn revoke(&mut self, token: &str) -> bool {
        self.tokens.remove(token).is_some()
    }
    pub fn lookup(&self, token: &str) -> Option<&StreamToken> {
        self.tokens.get(token)
    }
    pub fn purge_expired(&mut self, now: i64) -> usize {
        let before = self.tokens.len();
        self.tokens.retain(|_, t| t.is_valid(now));
        before - self.tokens.len()
    }
}

/// Bandwidth probe — short-circuit version of `tulipix-videos::abr::BandwidthProbe`
/// so the WAN streaming layer doesn't pull the videos crate in. Both share
/// the same harmonic-mean intuition.
#[derive(Debug, Default)]
pub struct WanProbe {
    samples_bps: Vec<i64>,
    cap: usize,
}

impl WanProbe {
    pub fn new(window: usize) -> Self {
        Self {
            samples_bps: Vec::with_capacity(window),
            cap: window.max(1),
        }
    }

    pub fn record(&mut self, segment_bytes: i64, downloaded_ms: i64) {
        if downloaded_ms <= 0 || segment_bytes <= 0 {
            return;
        }
        let bps = (segment_bytes as f64 * 8.0 * 1000.0 / downloaded_ms as f64) as i64;
        if self.samples_bps.len() == self.cap {
            self.samples_bps.remove(0);
        }
        self.samples_bps.push(bps);
    }

    pub fn estimate_bps(&self) -> Option<i64> {
        if self.samples_bps.is_empty() {
            return None;
        }
        let inv_sum: f64 = self
            .samples_bps
            .iter()
            .map(|s| 1.0 / (*s as f64).max(1.0))
            .sum();
        Some((self.samples_bps.len() as f64 / inv_sum) as i64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamManifest {
    pub item_id: i64,
    pub duration_s: f64,
    pub variants: Vec<ManifestVariant>,
    /// Cap-gating record so the backend can re-check before serving — the
    /// host might have downgraded the subscription tier since the token was
    /// minted.
    pub required_cap: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestVariant {
    pub label: String,
    pub width: i32,
    pub height: i32,
    pub bitrate_bps: i64,
    pub playlist_url: String,
}

impl StreamManifest {
    pub fn pick_variant(&self, probe_bps: i64) -> Option<&ManifestVariant> {
        // 1.4× headroom (same as the LAN ladder).
        let mut best: Option<&ManifestVariant> = None;
        for v in &self.variants {
            let need = (v.bitrate_bps as f64 * 1.4) as i64;
            if need <= probe_bps {
                best = Some(v);
            }
        }
        best
    }
}

/// Header value the backend echoes into its log. Format chosen so an admin
/// can grep one tail line and trace it back to a specific token + item.
pub fn authz_header(token: &StreamToken) -> String {
    format!("Bearer {} item={} user={}", token.token, token.item_id, token.user_id)
}

pub fn parse_authz_header(header: &str) -> Result<&str> {
    let rest = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow!("missing Bearer prefix"))?;
    let end = rest.find(' ').unwrap_or(rest.len());
    Ok(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_manifest() -> StreamManifest {
        StreamManifest {
            item_id: 1,
            duration_s: 7200.0,
            required_cap: "sync.remote-stream".into(),
            variants: vec![
                ManifestVariant {
                    label: "480p".into(),
                    width: 854,
                    height: 480,
                    bitrate_bps: 1_200_000,
                    playlist_url: "https://x/480.m3u8".into(),
                },
                ManifestVariant {
                    label: "1080p".into(),
                    width: 1920,
                    height: 1080,
                    bitrate_bps: 6_000_000,
                    playlist_url: "https://x/1080.m3u8".into(),
                },
            ],
        }
    }

    #[test]
    fn token_is_valid_within_window() {
        let entropy = [0xABu8; 32];
        let t = issue_token("{}", 1, 42, 3600, &entropy);
        assert!(t.is_valid(t.issued_at + 10));
        assert!(!t.is_valid(t.expires_at + 1));
    }

    #[test]
    fn token_ttl_floor_60s() {
        let entropy = [0u8; 32];
        let t = issue_token("{}", 1, 42, 10, &entropy);
        assert!(t.expires_at - t.issued_at >= 60);
    }

    #[test]
    fn token_manifest_sha_changes_with_manifest() {
        let entropy = [0u8; 32];
        let a = issue_token(r#"{"a":1}"#, 1, 42, 600, &entropy);
        let b = issue_token(r#"{"a":2}"#, 1, 42, 600, &entropy);
        assert_ne!(a.manifest_sha256, b.manifest_sha256);
    }

    #[test]
    fn token_store_insert_revoke_lookup() {
        let mut s = TokenStore::default();
        let t = issue_token("{}", 1, 1, 600, &[1u8; 32]);
        let tok = t.token.clone();
        s.insert(t);
        assert!(s.lookup(&tok).is_some());
        assert!(s.revoke(&tok));
        assert!(s.lookup(&tok).is_none());
        assert!(!s.revoke("doesnotexist"));
    }

    #[test]
    fn purge_drops_expired() {
        let mut s = TokenStore::default();
        let mut t = issue_token("{}", 1, 1, 600, &[1u8; 32]);
        t.expires_at = 100;
        s.insert(t);
        let dropped = s.purge_expired(500);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn wan_probe_harmonic_mean() {
        let mut p = WanProbe::new(4);
        p.record(1_000_000, 1_000);
        p.record(1_000_000, 5_000);
        let est = p.estimate_bps().unwrap();
        assert!(est > 1_000_000 && est < 4_000_000);
    }

    #[test]
    fn manifest_picks_variant_with_headroom() {
        let m = fixture_manifest();
        let v = m.pick_variant(10_000_000).unwrap();
        assert_eq!(v.label, "1080p");
        let v = m.pick_variant(2_000_000).unwrap();
        assert_eq!(v.label, "480p");
        assert!(m.pick_variant(1_000_000).is_none(), "no rung fits");
    }

    #[test]
    fn authz_header_round_trip() {
        let t = issue_token("{}", 7, 99, 600, &[0x01u8; 32]);
        let header = authz_header(&t);
        assert!(header.starts_with("Bearer "));
        let parsed = parse_authz_header(&header).unwrap();
        assert_eq!(parsed, t.token);
    }

    #[test]
    fn parse_authz_rejects_bad_format() {
        assert!(parse_authz_header("Basic abc").is_err());
    }
}
