//! Request signing for the Stream backend.
//!
//! Ported from MovieBox-Tui (`src/v3/crypto.rs`, MIT OR Apache-2.0,
//! https://github.com/mesamirh/MovieBox-Tui). Changes from upstream:
//!
//! * `rand` dependency dropped — the device-fingerprint and IP pickers only
//!   need to spread values across small arrays, so a seeded xorshift over the
//!   clock does the job without pulling a crate into the tree.
//! * `build_canonical_string` no longer panics on an unparseable URL. Hosts are
//!   user-editable now (Stream tab → Hosts), so a bad entry must degrade to a
//!   failed request, never take the app down.

use base64::Engine;
// digest 0.11 (hmac 0.13) moved `new_from_slice` off `Mac` and onto `KeyInit`.
use hmac::{Hmac, KeyInit, Mac};
use md5::{Digest, Md5};
use std::collections::BTreeMap;
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

const SECRET_KEY_DEFAULT: &str = "76iRl07s0xSN9jqmEWAt79EBJZulIQIsV64FZr2O";
const SIGNATURE_BODY_MAX_BYTES: usize = 102_400;

type HmacMd5 = Hmac<Md5>;

/// Runtime override for the request-signing secret. `None` = use the built-in
/// default. The key is a single app-wide value (MovieBox rotates it in new APK
/// builds), so a process-global fits the domain — every client signs alike.
/// Set from the Stream tab → Servers → Signing key editor via [`set_sign_key`].
static SIGN_KEY: RwLock<Option<String>> = RwLock::new(None);

/// The compiled-in default signing secret — the "Reset" value in the editor.
pub fn default_sign_key() -> &'static str {
    SECRET_KEY_DEFAULT
}

/// Install a runtime signing key. A blank value clears the override, reverting
/// to [`default_sign_key`]. Takes effect on the next signed request.
pub fn set_sign_key(key: &str) {
    let k = key.trim();
    let mut slot = SIGN_KEY.write().unwrap_or_else(|e| e.into_inner());
    *slot = if k.is_empty() { None } else { Some(k.to_string()) };
}

/// The secret in force right now — the override if set, else the default.
fn active_sign_key() -> String {
    SIGN_KEY
        .read()
        .ok()
        .and_then(|g| g.clone())
        .unwrap_or_else(|| SECRET_KEY_DEFAULT.to_string())
}

// ---- entropy (rand-free) ----

/// xorshift64* seeded off the clock. Not cryptographic — only used to pick
/// array indices for the device fingerprint, where any spread will do.
fn rand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0);
    let mut x = SEED.load(Ordering::Relaxed);
    if x == 0 {
        x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15; // xorshift is a fixed point at zero
        }
    }
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    SEED.store(x, Ordering::Relaxed);
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Uniform-ish index in `0..n` (`n` is always a small literal length here).
fn pick(n: usize) -> usize {
    if n == 0 { 0 } else { (rand_u64() % n as u64) as usize }
}

// ---- hashing helpers ----

fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn b64_decode(val: &str) -> Vec<u8> {
    let mut padded = val.to_string();
    let padding = (4 - padded.len() % 4) % 4;
    padded.push_str(&"=".repeat(padding));
    base64::engine::general_purpose::STANDARD
        .decode(padded)
        .unwrap_or_default()
}

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn generate_x_client_token(ts: u64) -> String {
    let ts_str = ts.to_string();
    let reversed_ts: String = ts_str.chars().rev().collect();
    format!("{},{}", ts_str, md5_hex(reversed_ts.as_bytes()))
}

/// Query params sorted by key (stable order is what the signature covers).
/// Repeated keys keep their relative order.
fn sorted_query_string(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return String::new();
    };
    let mut params: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, v) in parsed.query_pairs() {
        params.entry(k.into_owned()).or_default().push(v.into_owned());
    }
    let mut parts = Vec::new();
    for (key, values) in params {
        for val in values {
            parts.push(format!("{key}={val}"));
        }
    }
    parts.join("&")
}

/// Path (+ sorted query) the signature is computed over. Falls back to a manual
/// split when the URL will not parse, so a malformed user host cannot panic.
fn canonical_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => {
            let query = sorted_query_string(url);
            if query.is_empty() {
                parsed.path().to_string()
            } else {
                format!("{}?{}", parsed.path(), query)
            }
        }
        Err(_) => {
            // "scheme://host/rest" → "/rest"; anything else is used as-is.
            let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
            match after_scheme.find('/') {
                Some(i) => after_scheme[i..].to_string(),
                None => "/".to_string(),
            }
        }
    }
}

pub fn build_canonical_string(
    method: &str,
    accept: Option<&str>,
    content_type: Option<&str>,
    url: &str,
    body: Option<&str>,
    timestamp_ms: u64,
) -> String {
    let (body_hash, body_length) = match body.map(|b| b.as_bytes()) {
        Some(bytes) => {
            let len = bytes.len();
            let truncated = &bytes[..len.min(SIGNATURE_BODY_MAX_BYTES)];
            (md5_hex(truncated), len.to_string())
        }
        None => (String::new(), String::new()),
    };

    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(),
        accept.unwrap_or(""),
        content_type.unwrap_or(""),
        body_length,
        timestamp_ms,
        body_hash,
        canonical_url(url)
    )
}

pub fn generate_x_tr_signature(
    method: &str,
    accept: Option<&str>,
    content_type: Option<&str>,
    url: &str,
    body: Option<&str>,
    timestamp_ms: u64,
) -> String {
    let canonical = build_canonical_string(method, accept, content_type, url, body, timestamp_ms);
    let secret_bytes = b64_decode(&active_sign_key());
    let mut mac = HmacMd5::new_from_slice(&secret_bytes).expect("HMAC accepts a key of any size");
    mac.update(canonical.as_bytes());
    format!("{}|2|{}", timestamp_ms, b64_encode(&mac.finalize().into_bytes()))
}

pub fn build_signed_headers(
    method: &str,
    url: &str,
    body: Option<&str>,
    auth_token: Option<&str>,
    user_agent: &str,
    client_info: &str,
    spoofed_ip: &str,
) -> reqwest::header::HeaderMap {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let accept = "application/json";
    let content_type = "application/json";
    let mut headers = reqwest::header::HeaderMap::new();

    // A header value that will not parse means the fingerprint generator is
    // broken, not that the request should die — skip the header and continue.
    let mut put = |name: &'static str, value: &str| {
        if let Ok(v) = value.parse() {
            headers.insert(name, v);
        }
    };
    put("User-Agent", user_agent);
    put("Accept", accept);
    put("Content-Type", content_type);
    put("Connection", "keep-alive");
    put("X-Client-Token", &generate_x_client_token(ts));
    put(
        "x-tr-signature",
        &generate_x_tr_signature(method, Some(accept), Some(content_type), url, body, ts),
    );
    put("X-Client-Info", client_info);
    put("X-Client-Status", "0");
    put("X-Forwarded-For", spoofed_ip);
    if let Some(token) = auth_token {
        put("Authorization", &format!("Bearer {token}"));
    }
    headers
}

/// One randomised Android device fingerprint: `(user_agent, client_info)`.
/// Generated once per client so a session looks like a single device.
pub(crate) fn generate_client_info_and_ua() -> (String, String) {
    let android_versions = [
        ("9", "PQ3A.190605.03081104"),
        ("10", "QP1A.191005.007.A3"),
        ("11", "RP1A.200720.011"),
        ("12", "S1B.220414.015"),
        ("13", "TQ2A.230405.003"),
    ];
    let redmi_devices = [
        ("23078RKD5C", "Redmi"),
        ("2201117TY", "Redmi"),
        ("2201117TG", "Redmi"),
        ("22101316G", "Redmi"),
        ("21121210G", "Redmi"),
        ("M2012K11AG", "Redmi"),
        ("M2007J20CG", "Redmi"),
    ];
    let version_codes = [50020042, 50020043, 50020044, 50020045, 50020046];
    let network_types = ["NETWORK_WIFI", "NETWORK_MOBILE"];
    let timezones = [
        "Asia/Kolkata",
        "Asia/Shanghai",
        "Asia/Tokyo",
        "America/New_York",
        "Europe/London",
    ];

    let android = android_versions[pick(android_versions.len())];
    let device = redmi_devices[pick(redmi_devices.len())];
    let version_code = version_codes[pick(version_codes.len())];
    let network = network_types[pick(network_types.len())];
    let timezone = timezones[pick(timezones.len())];
    let gaid = random_uuid();
    let device_id = random_hex(32);

    let user_agent = format!(
        "com.community.oneroom/{} (Linux; U; Android {}; en_US; {}; Build/{}; Cronet/135.0.7012.3)",
        version_code, android.0, device.0, android.1
    );
    let client_info = format!(
        r#"{{"package_name":"com.community.oneroom","version_name":"3.0.03.0529.03","version_code":{},"os":"android","os_version":"{}","install_ch":"ps","device_id":"{}","install_store":"ps","gaid":"{}","brand":"{}","model":"{}","system_language":"en","net":"{}","region":"US","timezone":"{}","sp_code":"40401","X-Play-Mode":"2"}}"#,
        version_code, android.0, device_id, gaid, device.1, device.0, network, timezone
    );
    (user_agent, client_info)
}

fn random_hex(len: usize) -> String {
    (0..len).map(|_| format!("{:x}", pick(16))).collect()
}

fn random_uuid() -> String {
    format!(
        "{}-{}-{}-{}-{}",
        random_hex(8),
        random_hex(4),
        random_hex(4),
        random_hex(4),
        random_hex(12)
    )
}

pub(crate) fn random_spoofed_ip() -> String {
    const PREFIXES: &[&str] = &[
        "103.241", "49.36", "117.195", "106.198", "122.162", "157.32", "182.70", "103.58",
        "27.60", "59.90",
    ];
    let prefix = PREFIXES[pick(PREFIXES.len())];
    format!("{}.{}.{}", prefix, 1 + pick(253), 1 + pick(253))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_token_is_ts_plus_md5_of_reversed_ts() {
        let token = generate_x_client_token(1629876543210);
        let parts: Vec<&str> = token.split(',').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "1629876543210");
        assert_eq!(parts[1].len(), 32);
    }

    #[test]
    fn canonical_string_has_seven_lines_and_sorted_query() {
        let canonical = build_canonical_string(
            "GET",
            Some("application/json"),
            Some("application/json"),
            "https://api.example.com/test?b=2&a=1",
            None,
            1000,
        );
        let parts: Vec<&str> = canonical.split('\n').collect();
        assert_eq!(parts.len(), 7);
        assert_eq!(parts[0], "GET");
        assert_eq!(parts[3], ""); // no body → empty length
        assert_eq!(parts[4], "1000");
        assert_eq!(parts[5], ""); // no body → empty hash
        assert_eq!(parts[6], "/test?a=1&b=2");
    }

    #[test]
    fn body_contributes_length_and_hash() {
        let c = build_canonical_string("POST", None, None, "https://x.test/p", Some("{}"), 5);
        let parts: Vec<&str> = c.split('\n').collect();
        assert_eq!(parts[3], "2");
        assert_eq!(parts[5], md5_hex(b"{}"));
    }

    /// A user can type anything into the Hosts editor; signing must not panic.
    #[test]
    fn malformed_url_does_not_panic() {
        let c = build_canonical_string("GET", None, None, "not a url at all/x?q=1", None, 1);
        assert!(c.ends_with("/x?q=1"));
        let sig = generate_x_tr_signature("GET", None, None, "::::", None, 1);
        assert!(sig.starts_with("1|2|"));
    }

    #[test]
    fn signature_is_stable_for_same_inputs() {
        let a = generate_x_tr_signature("GET", None, None, "https://x.test/a", None, 7);
        let b = generate_x_tr_signature("GET", None, None, "https://x.test/a", None, 7);
        assert_eq!(a, b);
        let c = generate_x_tr_signature("GET", None, None, "https://x.test/b", None, 7);
        assert_ne!(a, c);
    }

    #[test]
    fn fingerprint_looks_like_an_android_client() {
        let (ua, info) = generate_client_info_and_ua();
        assert!(ua.starts_with("com.community.oneroom/"));
        assert!(ua.contains("Android"));
        assert!(info.contains(r#""os":"android""#));
        // device_id is 32 hex chars
        let id = info.split(r#""device_id":""#).nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn spoofed_ip_stays_in_range() {
        for _ in 0..200 {
            let ip = random_spoofed_ip();
            let last: u32 = ip.rsplit('.').next().unwrap().parse().unwrap();
            assert!((1..=253).contains(&last), "out of range: {ip}");
        }
    }
}
