//! `np.p4.sync.oauth` — Google + GitHub OAuth via `tulipix://` deep link.
//!
//! Desktop OAuth with PKCE: build the provider authorize URL (redirecting to
//! the OS-registered `tulipix://auth` scheme), then parse + validate the
//! deep-link callback. State is checked to block CSRF; PKCE S256 challenge is
//! derived from the verifier here.

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider { Google, GitHub }

impl Provider {
    fn authorize_endpoint(self) -> &'static str {
        match self {
            Provider::Google => "https://accounts.google.com/o/oauth2/v2/auth",
            Provider::GitHub => "https://github.com/login/oauth/authorize",
        }
    }
    fn scope(self) -> &'static str {
        match self { Provider::Google => "openid email profile", Provider::GitHub => "read:user user:email" }
    }
}

pub const REDIRECT_URI: &str = "tulipix://auth";

/// base64url (no padding).
fn b64url(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        if chunk.len() > 1 { out.push(T[(n >> 6 & 63) as usize] as char); }
        if chunk.len() > 2 { out.push(T[(n & 63) as usize] as char); }
    }
    out
}

/// PKCE S256 challenge from a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    let d = Sha256::digest(verifier.as_bytes());
    b64url(&d)
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

/// Build the authorize URL (PKCE S256, deep-link redirect).
pub fn authorize_url(provider: Provider, client_id: &str, state: &str, verifier: &str) -> String {
    format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        provider.authorize_endpoint(), enc(client_id), enc(REDIRECT_URI), enc(provider.scope()),
        enc(state), pkce_challenge(verifier),
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct Callback { pub code: String, pub state: String }

/// Parse a `tulipix://auth?code=...&state=...` deep link and verify `state`
/// matches the value we sent (CSRF protection).
pub fn parse_callback(deep_link: &str, expected_state: &str) -> Result<Callback> {
    let q = deep_link.split_once('?').map(|(_, q)| q).ok_or_else(|| anyhow!("no query"))?;
    let mut code = None;
    let mut state = None;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            match k { "code" => code = Some(v.to_string()), "state" => state = Some(v.to_string()), _ => {} }
        }
    }
    let code = code.ok_or_else(|| anyhow!("missing code"))?;
    let state = state.ok_or_else(|| anyhow!("missing state"))?;
    if state != expected_state { return Err(anyhow!("state mismatch")); }
    Ok(Callback { code, state })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_pkce_and_deep_link() {
        let url = authorize_url(Provider::Google, "cid", "xyz", "verifier123");
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=tulipix%3A%2F%2Fauth"));
        assert!(url.contains("state=xyz"));
    }

    #[test]
    fn callback_state_must_match() {
        let cb = parse_callback("tulipix://auth?code=abc&state=xyz", "xyz").unwrap();
        assert_eq!(cb.code, "abc");
        assert!(parse_callback("tulipix://auth?code=abc&state=evil", "xyz").is_err());
        assert!(parse_callback("tulipix://auth?state=xyz", "xyz").is_err()); // no code
    }

    #[test]
    fn pkce_is_deterministic() {
        assert_eq!(pkce_challenge("verifier123"), pkce_challenge("verifier123"));
        assert_ne!(pkce_challenge("a"), pkce_challenge("b"));
    }
}
