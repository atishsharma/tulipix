//! Smart albums — rule expressions compiled to SQL.
//!
//! A "rule" is a JSON AST stored in `albums.smart_rule`. The compiler walks
//! the AST and emits a parameterised SQL fragment; values are bound through
//! `sqlx` so input never touches the query string directly.
//!
//! Supported predicates:
//!   • field op value         e.g. {"tag":"beach"}, {"year":{">=":2024}}
//!   • {"and":[…]} / {"or":[…]} / {"not": …}
//!
//! Fields:
//!   - tag        → photo has tag (joins through item_tags + tags)
//!   - person     → photo has named person (joins through faces + people)
//!   - year/month → on `photo_meta.taken_at`
//!   - camera     → `photo_meta.camera_model` LIKE
//!   - starred    → boolean

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Sqlite, SqlitePool};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Op {
    Eq(Value),
    Cmp { op: String, value: Value },
}

#[derive(Debug)]
pub struct Compiled {
    pub sql: String,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone)]
pub enum Param {
    Text(String),
    Int(i64),
}

/// Compile a JSON AST into a SQL `WHERE` fragment selecting `items.id`. The
/// caller can splice it into a join over `items + photo_meta`.
pub fn compile(rule: &Value) -> Result<Compiled> {
    let mut params = Vec::new();
    let sql = compile_node(rule, &mut params)?;
    Ok(Compiled { sql, params })
}

fn compile_node(node: &Value, p: &mut Vec<Param>) -> Result<String> {
    let obj = node.as_object().ok_or_else(|| anyhow!("rule must be an object"))?;
    if let Some(arr) = obj.get("and").and_then(|v| v.as_array()) {
        let parts: Result<Vec<_>> = arr.iter().map(|n| compile_node(n, p)).collect();
        return Ok(format!("({})", parts?.join(" AND ")));
    }
    if let Some(arr) = obj.get("or").and_then(|v| v.as_array()) {
        let parts: Result<Vec<_>> = arr.iter().map(|n| compile_node(n, p)).collect();
        return Ok(format!("({})", parts?.join(" OR ")));
    }
    if let Some(inner) = obj.get("not") {
        return Ok(format!("(NOT {})", compile_node(inner, p)?));
    }
    // Leaf: exactly one field key.
    if obj.len() != 1 { return Err(anyhow!("leaf must have exactly one key")); }
    let (field, val) = obj.iter().next().unwrap();
    compile_leaf(field, val, p)
}

fn compile_leaf(field: &str, val: &Value, p: &mut Vec<Param>) -> Result<String> {
    let (op, value) = parse_op(val)?;
    match field {
        "tag" => match value {
            Value::String(s) => {
                p.push(Param::Text(s));
                Ok(format!("items.id IN (SELECT it.item_id FROM item_tags it JOIN tags t ON t.id = it.tag_id WHERE t.name = ?{})", noop(&op)?))
            }
            _ => Err(anyhow!("tag value must be string")),
        },
        "person" => match value {
            Value::String(s) => {
                p.push(Param::Text(s));
                Ok(format!("items.id IN (SELECT f.item_id FROM faces f JOIN people pe ON pe.id = f.person_id WHERE pe.name = ?{})", noop(&op)?))
            }
            _ => Err(anyhow!("person value must be string")),
        },
        "year" => num_cmp("CAST(strftime('%Y', datetime(photo_meta.taken_at, 'unixepoch')) AS INTEGER)", &op, value, p),
        "month" => num_cmp("CAST(strftime('%m', datetime(photo_meta.taken_at, 'unixepoch')) AS INTEGER)", &op, value, p),
        "starred" => match value {
            Value::Bool(b) => Ok(format!("photo_meta.starred = {}", if b { 1 } else { 0 })),
            _ => Err(anyhow!("starred takes bool")),
        },
        "camera" => match value {
            Value::String(s) => {
                p.push(Param::Text(format!("%{}%", s)));
                Ok("photo_meta.camera_model LIKE ?".into())
            }
            _ => Err(anyhow!("camera takes string")),
        },
        other => Err(anyhow!("unknown field: {other}")),
    }
}

/// "noop" returns "" for "=" and panics if a tag/person field gets a non-eq
/// comparator (we don't support `tag >= "x"`).
fn noop(op: &str) -> Result<&'static str> {
    match op {
        "=" => Ok(""),
        _ => Err(anyhow!("tag/person fields support equality only")),
    }
}

fn num_cmp(col: &str, op: &str, val: Value, p: &mut Vec<Param>) -> Result<String> {
    let n = match val {
        Value::Number(n) => n.as_i64().ok_or_else(|| anyhow!("expected integer"))?,
        _ => return Err(anyhow!("numeric field needs integer value")),
    };
    p.push(Param::Int(n));
    Ok(format!("{col} {op} ?"))
}

fn parse_op(val: &Value) -> Result<(String, Value)> {
    // {"tag":"beach"} → ("=", "beach")
    // {"year":{">=":2024}} → (">=", 2024)
    if let Value::Object(map) = val {
        let one = single_key(map)?;
        let (op, inner) = one;
        if !matches!(op.as_str(), "=" | "!=" | ">" | ">=" | "<" | "<=") {
            return Err(anyhow!("bad operator: {op}"));
        }
        return Ok((op, inner.clone()));
    }
    Ok(("=".to_string(), val.clone()))
}

fn single_key(map: &Map<String, Value>) -> Result<(String, &Value)> {
    if map.len() != 1 { return Err(anyhow!("op object must have one key")); }
    let (k, v) = map.iter().next().unwrap();
    Ok((k.clone(), v))
}

/// Execute a compiled rule on the photos pool and return matching item IDs.
pub async fn matches(pool: &SqlitePool, c: &Compiled) -> Result<Vec<i64>> {
    let sql = format!(
        "SELECT items.id FROM items
         LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.missing_since IS NULL
           AND items.section = 'photos'
           AND COALESCE(photo_meta.deleted_at, 0) = 0
           AND COALESCE(photo_meta.archived, 0) = 0
           AND {}",
        c.sql
    );
    let mut q = sqlx::query_as::<Sqlite, (i64,)>(sqlx::AssertSqlSafe(&*sql));
    for p in &c.params {
        q = match p {
            Param::Text(s) => q.bind(s.clone()),
            Param::Int(i) => q.bind(*i),
        };
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(i,)| i).collect())
}

pub async fn save_rule(pool: &SqlitePool, album_id: i64, rule: &Value) -> Result<()> {
    let body = serde_json::to_string(rule)?;
    sqlx::query("UPDATE albums SET smart_rule = ? WHERE id = ?")
        .bind(body).bind(album_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use serde_json::json;

    #[test]
    fn compile_tag_eq() {
        let c = compile(&json!({"tag":"beach"})).unwrap();
        assert!(c.sql.contains("item_tags"));
        assert_eq!(c.params.len(), 1);
    }

    #[test]
    fn compile_and_year_ge_tag() {
        let r = json!({"and":[{"tag":"beach"}, {"year":{">=":2024}}]});
        let c = compile(&r).unwrap();
        assert!(c.sql.contains(" AND "));
        assert_eq!(c.params.len(), 2);
    }

    #[test]
    fn rejects_unknown_field() {
        let r = json!({"xyz":"abc"});
        assert!(compile(&r).is_err());
    }

    #[tokio::test]
    async fn matches_runs_against_db() {
        let (_t, pool) = open_pool().await;
        // Insert items with starred = 1 / 0
        for (i, starred) in [(1, 1), (2, 0)] {
            let p = format!("/p/{i}.jpg");
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(&p).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(&p).fetch_one(&pool).await.unwrap();
            sqlx::query("INSERT INTO photo_meta (item_id, starred) VALUES (?, ?)")
                .bind(id).bind(starred).execute(&pool).await.unwrap();
        }
        let r = json!({"starred": true});
        let c = compile(&r).unwrap();
        let ids = matches(&pool, &c).await.unwrap();
        assert_eq!(ids.len(), 1);
    }
}
