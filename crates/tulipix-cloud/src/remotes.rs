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

/// One remote's stored config out of `rclone config dump`: its backend type
/// and every key it was configured with. Backs the Edit path, which has to
/// reopen the generated form with what is already there.
///
/// `type` is lifted out rather than left in the map — it names the backend, not
/// one of its options, and the form treats those differently.
pub fn parse_dump_one(json: &str, name: &str) -> Option<(String, BTreeMap<String, String>)> {
    let v: BTreeMap<String, serde_json::Value> = serde_json::from_str(json).ok()?;
    let cfg = v.get(name)?.as_object()?;
    let backend = cfg.get("type").and_then(|t| t.as_str())?.to_string();
    let opts = cfg
        .iter()
        .filter(|(k, _)| k.as_str() != "type")
        .map(|(k, val)| {
            // rclone dumps everything as strings, but a JSON number or bool
            // would still round-trip as its literal rather than as `"5"`.
            let s = match val {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    Some((backend, opts))
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

/// cloud.db id for a remote name (None if not mirrored yet).
pub async fn id_of(pool: &SqlitePool, name: &str) -> Result<Option<i64>> {
    Ok(sqlx::query_scalar("SELECT id FROM remotes WHERE name = ?").bind(name).fetch_optional(pool).await?)
}

/// Remote name for a cloud.db id.
pub async fn name_of(pool: &SqlitePool, id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT name FROM remotes WHERE id = ?").bind(id).fetch_optional(pool).await?)
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

    #[test]
    fn dump_one_splits_the_backend_off_from_its_options() {
        let json = r#"{"gdrive":{"type":"drive","scope":"drive","chunk":8},"s3box":{"type":"s3"}}"#;
        let (backend, opts) = parse_dump_one(json, "gdrive").unwrap();
        assert_eq!(backend, "drive");
        assert_eq!(opts.get("scope").map(String::as_str), Some("drive"));
        // A non-string value still arrives as its literal, not as JSON quoting.
        assert_eq!(opts.get("chunk").map(String::as_str), Some("8"));
        assert!(!opts.contains_key("type"), "type names the backend, not an option");
        assert!(parse_dump_one(json, "nope").is_none());
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
