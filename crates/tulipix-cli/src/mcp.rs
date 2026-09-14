//! `tulipix-cli mcp` — the library, served to an outside agent over stdio.
//!
//! The protocol is in `tulipix_core::mcp`; this is where the answers come
//! from. The databases are opened **read-only**, one per section and only
//! when a tool actually needs it, so:
//!
//!   * nothing here can migrate a schema under the running app;
//!   * a library that has never been opened is not brought into being by an
//!     agent asking about it;
//!   * an agent that only ever searches never opens the other three files.
//!
//! The two write tools are the exception and say so: they take a writable
//! pool, and only after the user has switched writing on in Settings — which
//! `tulipix_core::mcp` has already checked before the call reaches here.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tulipix_core::db::DbHandle;
use tulipix_core::mcp::{McpServer, RpcRequest, RpcResponse, ToolHandler};

/// The sections an MCP tool may reach. Not every database in the data folder:
/// the money one is nobody's business but the user's, and the tools queue and
/// the cloud bookkeeping are not a library.
const SECTIONS: [&str; 4] = ["photos", "videos", "music", "books"];

#[derive(Default)]
pub struct Library {
    read: Mutex<HashMap<String, SqlitePool>>,
    write: Mutex<HashMap<String, SqlitePool>>,
}

impl Library {
    /// The section's database, read-only, opened once per run.
    async fn read(&self, section: &str) -> Result<SqlitePool> {
        if !SECTIONS.contains(&section) {
            anyhow::bail!("there is no {section} library");
        }
        let mut g = self.read.lock().await;
        if let Some(p) = g.get(section) {
            return Ok(p.clone());
        }
        let pool = DbHandle::open(section)?.read_only_pool().await?;
        g.insert(section.to_string(), pool.clone());
        Ok(pool)
    }

    /// The section's database, writable. Only the two write tools reach this,
    /// and only after Settings has allowed it.
    async fn write(&self, section: &str) -> Result<SqlitePool> {
        if !SECTIONS.contains(&section) {
            anyhow::bail!("there is no {section} library");
        }
        let handle = DbHandle::open(section)?;
        if !handle.path.exists() {
            anyhow::bail!("there is no {section} library on this computer yet");
        }
        let mut g = self.write.lock().await;
        if let Some(p) = g.get(section) {
            return Ok(p.clone());
        }
        let pool = handle.pool().await?;
        g.insert(section.to_string(), pool.clone());
        Ok(pool)
    }
}

fn arg_i64(args: &Value, key: &str) -> Result<i64> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .with_context(|| format!("{key} is missing, and it has to be a number"))
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{key} is missing"))
}

/// 1..=200, 25 when not asked for. A tool an agent can ask for a million rows
/// from is a tool that hangs the agent.
fn arg_limit(args: &Value) -> i64 {
    args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25).clamp(1, 200)
}

/// `%` and `_` are SQL wildcards, and a search for "100%_final.jpg" should
/// mean those characters rather than "anything". `\` is the escape, declared
/// with ESCAPE on every LIKE below.
fn like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 8);
    out.push('%');
    for c in query.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[async_trait::async_trait]
impl ToolHandler for Library {
    async fn call(&self, name: &str, args: &Value) -> Result<Value> {
        match name {
            "library.sections" => self.sections().await,
            "library.search" => {
                let query = arg_str(args, "query")?;
                let limit = arg_limit(args);
                let only = args.get("section").and_then(|v| v.as_str());
                self.search(query, only, limit).await
            }
            "photos.list-recent" => self.recent_photos(arg_limit(args)).await,
            "photos.get" => self.photo(arg_i64(args, "id")?).await,
            "videos.get" => self.video(arg_i64(args, "id")?).await,
            "music.get" => self.track(arg_i64(args, "id")?).await,
            "photos.tag" => self.tag_photo(arg_i64(args, "id")?, arg_str(args, "tag")?).await,
            "photos.star" => {
                let starred = args
                    .get("starred")
                    .and_then(|v| v.as_bool())
                    .context("starred is missing, and it has to be true or false")?;
                self.star_photo(arg_i64(args, "id")?, starred).await
            }
            other => anyhow::bail!("unknown tool: {other}"),
        }
    }
}

impl Library {
    /// Which libraries exist, and how big each is. A section whose database is
    /// not there is reported as absent rather than as an error: "you have no
    /// books library" is the answer, not a failure.
    async fn sections(&self) -> Result<Value> {
        let mut out = Vec::new();
        for section in SECTIONS {
            let entry = match self.read(section).await {
                Ok(pool) => {
                    let items: i64 =
                        sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE missing_since IS NULL")
                            .fetch_one(&pool)
                            .await
                            .unwrap_or(0);
                    json!({ "section": section, "present": true, "items": items })
                }
                Err(_) => json!({ "section": section, "present": false, "items": 0 }),
            };
            out.push(entry);
        }
        Ok(json!({ "sections": out }))
    }

    async fn search(&self, query: &str, only: Option<&str>, limit: i64) -> Result<Value> {
        let wanted: Vec<&str> = match only {
            Some(s) if SECTIONS.contains(&s) => vec![s],
            Some(s) => anyhow::bail!("there is no {s} library — try photos, videos, music or books"),
            None => SECTIONS.to_vec(),
        };
        let pattern = like_pattern(query);
        let mut hits = Vec::new();
        for section in wanted {
            let Ok(pool) = self.read(section).await else { continue };
            let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
                "SELECT id, abs_path, size, added FROM items \
                 WHERE abs_path LIKE ? ESCAPE '\\' AND missing_since IS NULL \
                 ORDER BY added DESC LIMIT ?",
            )
            .bind(&pattern)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
            for (id, path, size, added) in rows {
                hits.push(json!({
                    "id": id,
                    "section": section,
                    "path": path,
                    "size_bytes": size,
                    "added_unix": added,
                }));
            }
        }
        hits.truncate(limit as usize);
        Ok(json!({ "query": query, "count": hits.len(), "results": hits }))
    }

    async fn recent_photos(&self, limit: i64) -> Result<Value> {
        let pool = self.read("photos").await?;
        let rows: Vec<(i64, String, Option<i64>, i64)> = sqlx::query_as(
            "SELECT i.id, i.abs_path, m.taken_at, i.added \
             FROM items i LEFT JOIN photo_meta m ON m.item_id = i.id \
             WHERE i.missing_since IS NULL AND (m.deleted_at IS NULL) \
             ORDER BY i.added DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await?;
        Ok(json!({
            "count": rows.len(),
            "photos": rows
                .into_iter()
                .map(|(id, path, taken_at, added)| json!({
                    "id": id, "path": path, "taken_at_unix": taken_at, "added_unix": added,
                }))
                .collect::<Vec<_>>(),
        }))
    }

    async fn photo(&self, id: i64) -> Result<Value> {
        let pool = self.read("photos").await?;
        let row: Option<(String, Option<i64>, Option<String>, Option<String>, Option<i64>, Option<i64>, i64)> =
            sqlx::query_as(
                "SELECT i.abs_path, m.taken_at, m.camera_make, m.camera_model, m.width, m.height, \
                        COALESCE(m.starred, 0) \
                 FROM items i LEFT JOIN photo_meta m ON m.item_id = i.id WHERE i.id = ?",
            )
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        let (path, taken_at, make, model, width, height, starred) =
            row.with_context(|| format!("no photo with id {id}"))?;
        let tags: Vec<String> = sqlx::query_scalar(
            "SELECT t.name FROM item_tags it JOIN tags t ON t.id = it.tag_id \
             WHERE it.item_id = ? ORDER BY t.name",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap_or_default();
        Ok(json!({
            "id": id,
            "path": path,
            "taken_at_unix": taken_at,
            "camera": match (make, model) {
                (Some(a), Some(b)) => Some(format!("{a} {b}").trim().to_string()),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
            "width": width,
            "height": height,
            "starred": starred != 0,
            "tags": tags,
        }))
    }

    async fn video(&self, id: i64) -> Result<Value> {
        let pool = self.read("videos").await?;
        let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        let path = path.with_context(|| format!("no video with id {id}"))?;

        // A film and an episode are different rows in different tables; an
        // item is one or the other or neither (a loose file).
        let movie: Option<(String, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT title, year, overview FROM movies WHERE item_id = ?")
                .bind(id)
                .fetch_optional(&pool)
                .await
                .unwrap_or(None);
        let episode: Option<(String, i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT s.title, e.season, e.episode, e.title \
             FROM episodes e JOIN shows s ON s.id = e.show_id WHERE e.item_id = ?",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);
        let progress: Option<(f64, Option<f64>, i64)> = sqlx::query_as(
            "SELECT position_s, duration_s, finished FROM watch_progress WHERE item_id = ?",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);

        let mut out = json!({ "id": id, "path": path });
        if let Some((title, year, overview)) = movie {
            out["kind"] = json!("movie");
            out["title"] = json!(title);
            out["year"] = json!(year);
            out["overview"] = json!(overview);
        } else if let Some((show, season, number, title)) = episode {
            out["kind"] = json!("episode");
            out["show"] = json!(show);
            out["season"] = json!(season);
            out["episode"] = json!(number);
            out["title"] = json!(title);
        } else {
            out["kind"] = json!("file");
        }
        if let Some((position_s, duration_s, finished)) = progress {
            out["position_s"] = json!(position_s);
            out["duration_s"] = json!(duration_s);
            out["finished"] = json!(finished != 0);
        }
        Ok(out)
    }

    async fn track(&self, id: i64) -> Result<Value> {
        let pool = self.read("music").await?;
        type Row = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<f64>,
            i64,
            i64,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT i.abs_path, t.title, ar.name, al.title, t.year, t.duration_s, \
                    COALESCE(t.rating, 0), COALESCE(t.play_count, 0) \
             FROM items i \
             LEFT JOIN track_meta t ON t.item_id = i.id \
             LEFT JOIN artists ar   ON ar.id = t.artist_id \
             LEFT JOIN albums al    ON al.id = t.album_id \
             WHERE i.id = ?",
        )
        .bind(id)
        .fetch_optional(&pool)
        .await?;
        let (path, title, artist, album, year, duration_s, rating, plays) =
            row.with_context(|| format!("no track with id {id}"))?;
        Ok(json!({
            "id": id,
            "path": path,
            "title": title,
            "artist": artist,
            "album": album,
            "year": year,
            "duration_s": duration_s,
            "rating": rating,
            "play_count": plays,
        }))
    }

    async fn tag_photo(&self, id: i64, tag: &str) -> Result<Value> {
        let pool = self.write("photos").await?;
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM items WHERE id = ?")
            .bind(id)
            .fetch_optional(&pool)
            .await?;
        exists.with_context(|| format!("no photo with id {id}"))?;
        sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
            .bind(tag)
            .execute(&pool)
            .await?;
        let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
            .bind(tag)
            .fetch_one(&pool)
            .await?;
        // `source` says an agent put it there, so the user can tell it from
        // their own tagging and from the on-device taggers.
        sqlx::query(
            "INSERT INTO item_tags (item_id, tag_id, confidence, source) VALUES (?, ?, 1.0, 'mcp') \
             ON CONFLICT(item_id, tag_id) DO NOTHING",
        )
        .bind(id)
        .bind(tag_id)
        .execute(&pool)
        .await?;
        Ok(json!({ "id": id, "tag": tag, "tagged": true }))
    }

    async fn star_photo(&self, id: i64, starred: bool) -> Result<Value> {
        let pool = self.write("photos").await?;
        let n = sqlx::query("UPDATE photo_meta SET starred = ? WHERE item_id = ?")
            .bind(i64::from(starred))
            .bind(id)
            .execute(&pool)
            .await?
            .rows_affected();
        if n == 0 {
            anyhow::bail!("no photo with id {id}, or it has not been scanned yet");
        }
        Ok(json!({ "id": id, "starred": starred }))
    }
}

/// One line in, one line out, until stdin closes.
///
/// A line that is not JSON is answered with a parse error rather than being
/// fatal: an agent recovers from an error object, and not from the server
/// exiting. A notification (no `id`) is acted on and not answered, which is
/// what JSON-RPC says and what the clients expect.
pub async fn serve() -> Result<()> {
    use std::io::{BufRead, Write};

    let server = Arc::new(McpServer::new(Library::default()));
    if !server.enabled() {
        // Still served, so the agent gets a clear answer on its first call
        // rather than a pipe that says nothing. Every method comes back with
        // "switched off".
        eprintln!(
            "tulipix mcp: the MCP server is switched off. \
             Turn it on in Settings › Advanced › MCP server."
        );
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(req) => {
                let notification = req.id.is_null();
                let r = server.handle(req).await;
                if notification {
                    continue;
                }
                r
            }
            Err(e) => RpcResponse::err(Value::Null, -32700, format!("parse error: {e}")),
        };
        let mut bytes = serde_json::to_vec(&resp)?;
        bytes.push(b'\n');
        stdout.write_all(&bytes)?;
        stdout.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_wildcards_in_a_query_are_escaped_so_they_mean_themselves() {
        assert_eq!(like_pattern("holiday"), "%holiday%");
        assert_eq!(like_pattern("100%_final"), "%100\\%\\_final%");
        assert_eq!(like_pattern("back\\slash"), "%back\\\\slash%");
    }

    #[test]
    fn a_limit_is_always_between_one_and_two_hundred() {
        assert_eq!(arg_limit(&json!({})), 25);
        assert_eq!(arg_limit(&json!({ "limit": 5 })), 5);
        assert_eq!(arg_limit(&json!({ "limit": 0 })), 1);
        assert_eq!(arg_limit(&json!({ "limit": 100000 })), 200);
        assert_eq!(arg_limit(&json!({ "limit": "lots" })), 25);
    }

    #[test]
    fn missing_arguments_say_which_one() {
        assert!(arg_i64(&json!({}), "id").unwrap_err().to_string().contains("id"));
        assert!(arg_str(&json!({ "tag": "  " }), "tag").is_err());
        assert_eq!(arg_str(&json!({ "tag": " beach " }), "tag").unwrap(), "beach");
    }

    #[tokio::test]
    async fn a_section_outside_the_four_is_refused_rather_than_opened() {
        let lib = Library::default();
        assert!(lib.read("finances").await.is_err());
        assert!(lib.write("finances").await.is_err());
        let err = lib.search("x", Some("finances"), 10).await.unwrap_err();
        assert!(err.to_string().contains("no finances library"), "{err}");
    }
}
