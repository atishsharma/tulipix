//! `np.p4.cloud.remotes` — rclone config CRUD shelling to bundled rclone.
//!
//! Builds the `rclone config create/delete` argv and parses `rclone config
//! dump` (a JSON object keyed by remote name) into a local mirror in
//! `remotes`. The subprocess spawn lives in the app layer; argv + parse here.

use anyhow::Result;
use std::collections::BTreeMap;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `rclone config create <name> <backend> key=val ...` argv (non-interactive).
pub fn create_args(name: &str, backend: &str, opts: &[(&str, &str)]) -> Vec<String> {
    let mut a = vec!["config".into(), "create".into(), name.into(), backend.into()];
    for (k, v) in opts { a.push(format!("{k}={v}")); }
    a.push("--non-interactive".into());
    a
}

pub fn delete_args(name: &str) -> Vec<String> {
    vec!["config".into(), "delete".into(), name.into()]
}

pub fn dump_args() -> Vec<String> { vec!["config".into(), "dump".into()] }

/// Parse `rclone config dump` → list of (remote name, backend type).
pub fn parse_dump(json: &str) -> Vec<(String, String)> {
    let v: BTreeMap<String, serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    v.into_iter().map(|(name, cfg)| {
        let backend = cfg.get("type").and_then(|t| t.as_str()).unwrap_or("unknown").to_string();
        (name, backend)
    }).collect()
}

/// Mirror a remote into `cloud.db`.
pub async fn upsert(pool: &SqlitePool, name: &str, backend: &str, encrypted: bool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO remotes (name, backend, encrypted, created) VALUES (?,?,?,?)
         ON CONFLICT(name) DO UPDATE SET backend=excluded.backend, encrypted=excluded.encrypted
         RETURNING id",
    ).bind(name).bind(backend).bind(encrypted as i64).bind(now()).fetch_one(pool).await?)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as("SELECT name, backend FROM remotes ORDER BY name COLLATE NOCASE").fetch_all(pool).await?)
}

pub async fn remove(pool: &SqlitePool, name: &str) -> Result<()> {
    sqlx::query("DELETE FROM remotes WHERE name = ?").bind(name).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn create_argv_non_interactive() {
        let a = create_args("gdrive", "drive", &[("scope", "drive"), ("token", "{}")]);
        assert_eq!(a[..4], ["config", "create", "gdrive", "drive"]);
        assert!(a.contains(&"scope=drive".to_string()));
        assert!(a.contains(&"--non-interactive".to_string()));
    }

    #[test]
    fn dump_parse() {
        let json = r#"{"gdrive":{"type":"drive","scope":"drive"},"s3box":{"type":"s3"}}"#;
        let r = parse_dump(json);
        assert_eq!(r.len(), 2);
        assert!(r.contains(&("gdrive".to_string(), "drive".to_string())));
        assert!(r.contains(&("s3box".to_string(), "s3".to_string())));
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let (_t, pool) = open_pool().await;
        upsert(&pool, "gdrive", "drive", false).await.unwrap();
        upsert(&pool, "gdrive", "drive", true).await.unwrap(); // update, no dup
        assert_eq!(list(&pool).await.unwrap().len(), 1);
        remove(&pool, "gdrive").await.unwrap();
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
