//! `np.p4.sync.audit-log` — admin audit log.
//!
//! Records security-relevant events (invite / role-change / share / delete /
//! tier-flip / config) for Settings → Admin → Audit Log, with event-type
//! filtering and CSV export. Append-only; no update/delete API.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub actor: String,
    pub event: String,
    pub target: Option<String>,
    pub detail: Option<String>,
    pub at: i64,
}

pub const EVENTS: &[&str] = &["invite", "role-change", "share", "delete", "tier-flip", "config"];

pub async fn record(pool: &SqlitePool, actor: &str, event: &str, target: Option<&str>, detail: Option<&str>, at: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("INSERT INTO audit_log (actor, event, target, detail, at) VALUES (?,?,?,?,?) RETURNING id")
        .bind(actor).bind(event).bind(target).bind(detail).bind(at).fetch_one(pool).await?)
}

/// Newest entries, optionally filtered to one `event` type.
pub async fn query(pool: &SqlitePool, event: Option<&str>, limit: i64) -> Result<Vec<AuditEntry>> {
    let rows: Vec<(i64, String, String, Option<String>, Option<String>, i64)> = match event {
        Some(e) => sqlx::query_as(
            "SELECT id, actor, event, target, detail, at FROM audit_log WHERE event = ? ORDER BY at DESC, id DESC LIMIT ?",
        ).bind(e).bind(limit).fetch_all(pool).await?,
        None => sqlx::query_as(
            "SELECT id, actor, event, target, detail, at FROM audit_log ORDER BY at DESC, id DESC LIMIT ?",
        ).bind(limit).fetch_all(pool).await?,
    };
    Ok(rows.into_iter().map(|(id, actor, event, target, detail, at)| AuditEntry { id, actor, event, target, detail, at }).collect())
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Render entries as CSV (header + rows) for export.
pub fn to_csv(entries: &[AuditEntry]) -> String {
    let mut out = String::from("id,actor,event,target,detail,at\n");
    for e in entries {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            e.id, csv_escape(&e.actor), csv_escape(&e.event),
            csv_escape(e.target.as_deref().unwrap_or("")),
            csv_escape(e.detail.as_deref().unwrap_or("")),
            e.at,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn record_filter_and_csv() {
        let (_t, pool) = open_pool().await;
        record(&pool, "admin", "invite", Some("a@x"), None, 100).await.unwrap();
        record(&pool, "admin", "delete", Some("file.jpg"), Some("reason, with comma"), 200).await.unwrap();
        record(&pool, "admin", "invite", Some("b@x"), None, 300).await.unwrap();
        let invites = query(&pool, Some("invite"), 10).await.unwrap();
        assert_eq!(invites.len(), 2);
        assert_eq!(invites[0].target.as_deref(), Some("b@x")); // newest first
        let all = query(&pool, None, 10).await.unwrap();
        assert_eq!(all.len(), 3);
        let csv = to_csv(&all);
        assert!(csv.starts_with("id,actor,event,target,detail,at\n"));
        assert!(csv.contains("\"reason, with comma\"")); // comma escaped
    }
}
