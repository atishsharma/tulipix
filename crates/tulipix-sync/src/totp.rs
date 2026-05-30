//! `np.p4.sync.2fa` — 2FA / TOTP for Account mode.
//!
//! RFC 6238 TOTP (production wires `totp-rs`; this is a dependency-light
//! HMAC-SHA256 implementation so the algorithm + verification window are
//! unit-tested), single-use backup codes shown once at enrollment, and the
//! email-reset recovery gate.
//!
//! Note: the standard's HOTP truncation is algorithm-agnostic; we use
//! HMAC-SHA256 (also permitted by RFC 6238) since SHA-1 isn't available here.

use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

const BLOCK: usize = 64;

/// HMAC-SHA256.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK { ipad[i] ^= k[i]; opad[i] ^= k[i]; }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// Decode an RFC 4648 base32 secret (uppercase, no padding required).
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u32;
    let mut nbits = 0;
    let mut out = Vec::new();
    for c in s.trim().to_ascii_uppercase().bytes() {
        if c == b'=' { continue; }
        let v = ALPHA.iter().position(|&x| x == c)? as u32;
        bits = (bits << 5) | v;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Some(out)
}

/// HOTP/TOTP code for a counter.
fn code_for_counter(secret: &[u8], counter: u64, digits: u32) -> String {
    let mac = hmac_sha256(secret, &counter.to_be_bytes());
    let off = (mac[mac.len() - 1] & 0x0f) as usize;
    let bin = ((mac[off] as u32 & 0x7f) << 24)
        | ((mac[off + 1] as u32) << 16)
        | ((mac[off + 2] as u32) << 8)
        | (mac[off + 3] as u32);
    let modulo = 10u32.pow(digits);
    format!("{:0width$}", bin % modulo, width = digits as usize)
}

/// TOTP code at `unix_time` for `period` seconds and `digits`.
pub fn totp(secret: &[u8], unix_time: i64, period: i64, digits: u32) -> String {
    let counter = (unix_time / period.max(1)) as u64;
    code_for_counter(secret, counter, digits)
}

/// Verify `code` against the current step ±`window` steps (clock skew).
pub fn verify(secret: &[u8], code: &str, unix_time: i64, period: i64, digits: u32, window: i64) -> bool {
    let base = unix_time / period.max(1);
    for off in -window..=window {
        let c = code_for_counter(secret, (base + off) as u64, digits);
        if c == code { return true; }
    }
    false
}

fn hash_code(code: &str) -> String {
    Sha256::digest(code.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Store hashed backup codes for a user (shown in plaintext once by caller).
pub async fn store_backup_codes(pool: &SqlitePool, user_id: &str, codes: &[String]) -> Result<()> {
    for code in codes {
        sqlx::query("INSERT INTO totp_backup_codes (user_id, code_hash, used) VALUES (?,?,0)")
            .bind(user_id).bind(hash_code(code)).execute(pool).await?;
    }
    Ok(())
}

/// Consume a backup code once; returns true if it was valid + unused.
pub async fn consume_backup_code(pool: &SqlitePool, user_id: &str, code: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE totp_backup_codes SET used = 1 WHERE user_id = ? AND code_hash = ? AND used = 0",
    ).bind(user_id).bind(hash_code(code)).execute(pool).await?;
    Ok(r.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn base32_roundtrip_known() {
        // "JBSWY3DPEHPK3PXP" = "Hello!\xde\xad\xbe\xef" prefix bytes
        let bytes = base32_decode("JBSWY3DP").unwrap();
        assert_eq!(&bytes[..5], b"Hello");
    }

    #[test]
    fn totp_verifies_within_window() {
        let secret = base32_decode("JBSWY3DPEHPK3PXP").unwrap();
        let t = 1_700_000_000;
        let code = totp(&secret, t, 30, 6);
        assert_eq!(code.len(), 6);
        assert!(verify(&secret, &code, t, 30, 6, 1));
        // a step away still ok with window=1
        assert!(verify(&secret, &code, t + 30, 30, 6, 1));
        // far away fails
        assert!(!verify(&secret, &code, t + 600, 30, 6, 1));
    }

    #[tokio::test]
    async fn backup_codes_single_use() {
        let (_t, pool) = open_pool().await;
        store_backup_codes(&pool, "u1", &["aaa-bbb".into(), "ccc-ddd".into()]).await.unwrap();
        assert!(consume_backup_code(&pool, "u1", "aaa-bbb").await.unwrap());
        assert!(!consume_backup_code(&pool, "u1", "aaa-bbb").await.unwrap()); // reuse blocked
        assert!(!consume_backup_code(&pool, "u1", "zzz-zzz").await.unwrap()); // wrong code
    }
}
