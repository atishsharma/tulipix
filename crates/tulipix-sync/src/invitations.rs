//! `np.p4.sync.invitations` — user invitation flow.
//!
//! An admin invites by email + role; an opaque token is minted and emailed.
//! Invites expire after 7 days. This owns issue / accept / revoke and the
//! expiry-aware state resolution (a pending-but-past-expiry invite reads as
//! expired without a writer touching it).

use anyhow::Result;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub const INVITE_TTL_SECS: i64 = 7 * 86_400;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Deterministic opaque token from (email, salt). Real backend uses a CSPRNG;
/// this keeps it testable while non-guessable from the email alone.
pub fn mint_token(email: &str, salt: &str) -> String {
    let d = Sha256::digest(format!("{email}:{salt}").as_bytes());
    d.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub async fn invite(pool: &SqlitePool, email: &str, role: &str, by: &str, token: &str) -> Result<i64> {
    let t = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO invitations (email, role, token, state, invited_by, created, expires)
         VALUES (?,?,?,'pending',?,?,?) RETURNING id",
    ).bind(email).bind(role).bind(token).bind(by).bind(t).bind(t + INVITE_TTL_SECS).fetch_one(pool).await?)
}

/// Accept a token if pending and unexpired. Returns true on success.
pub async fn accept(pool: &SqlitePool, token: &str, now_unix: i64) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE invitations SET state = 'accepted' WHERE token = ? AND state = 'pending' AND expires > ?",
    ).bind(token).bind(now_unix).execute(pool).await?;
    Ok(r.rows_affected() == 1)
}

pub async fn revoke(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE invitations SET state = 'revoked' WHERE id = ? AND state = 'pending'")
        .bind(id).execute(pool).await?;
    Ok(())
}

/// Effective state, accounting for expiry without mutating the row.
pub async fn effective_state(pool: &SqlitePool, token: &str, now_unix: i64) -> Result<Option<String>> {
    let row: Option<(String, i64)> = sqlx::query_as("SELECT state, expires FROM invitations WHERE token = ?")
        .bind(token).fetch_optional(pool).await?;
    Ok(row.map(|(state, expires)| {
        if state == "pending" && now_unix >= expires { "expired".to_string() } else { state }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn invite_accept_and_expiry() {
        let (_t, pool) = open_pool().await;
        let tok = mint_token("a@x.com", "salt");
        invite(&pool, "a@x.com", "member", "admin@x", &tok).await.unwrap();
        // expired path: now past expiry → cannot accept, reads expired
        assert!(!accept(&pool, &tok, now() + INVITE_TTL_SECS + 1).await.unwrap());
        assert_eq!(effective_state(&pool, &tok, now() + INVITE_TTL_SECS + 1).await.unwrap().as_deref(), Some("expired"));
        // fresh accept works
        assert!(accept(&pool, &tok, now()).await.unwrap());
        assert_eq!(effective_state(&pool, &tok, now()).await.unwrap().as_deref(), Some("accepted"));
        // double-accept fails
        assert!(!accept(&pool, &tok, now()).await.unwrap());
    }

    #[test]
    fn token_non_guessable() {
        assert_ne!(mint_token("a@x", "s1"), mint_token("a@x", "s2"));
        assert_eq!(mint_token("a@x", "s1").len(), 32);
    }
}
