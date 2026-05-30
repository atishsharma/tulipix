//! `np.p4.sync.toggle` — Settings → Account mode toggle.
//!
//! Account mode is opt-in: local-only by default. This owns the mode state +
//! the precondition gate (can't enable without being signed in) and persists
//! the choice in `sync_state` so it survives relaunch.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode { LocalOnly, Account }

pub const STATE_KEY: &str = "account_mode";

/// Can Account mode be enabled? Requires a completed sign-in.
pub fn can_enable(signed_in: bool) -> bool { signed_in }

pub async fn set_mode(pool: &SqlitePool, mode: Mode, signed_in: bool) -> Result<Mode> {
    let effective = match mode {
        Mode::Account if !can_enable(signed_in) => Mode::LocalOnly, // refuse, stay local
        m => m,
    };
    let v = match effective { Mode::LocalOnly => "local", Mode::Account => "account" };
    sqlx::query("INSERT INTO sync_state (key, value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(STATE_KEY).bind(v).execute(pool).await?;
    Ok(effective)
}

pub async fn get_mode(pool: &SqlitePool) -> Result<Mode> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM sync_state WHERE key = ?")
        .bind(STATE_KEY).fetch_optional(pool).await?;
    Ok(match v.as_deref() { Some("account") => Mode::Account, _ => Mode::LocalOnly })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn defaults_local_and_requires_signin() {
        let (_t, pool) = open_pool().await;
        assert_eq!(get_mode(&pool).await.unwrap(), Mode::LocalOnly);
        // refuses without sign-in
        assert_eq!(set_mode(&pool, Mode::Account, false).await.unwrap(), Mode::LocalOnly);
        assert_eq!(get_mode(&pool).await.unwrap(), Mode::LocalOnly);
        // succeeds when signed in
        assert_eq!(set_mode(&pool, Mode::Account, true).await.unwrap(), Mode::Account);
        assert_eq!(get_mode(&pool).await.unwrap(), Mode::Account);
    }
}
