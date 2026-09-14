//! MCP (Model Context Protocol) server — the library, offered to an outside
//! agent (Claude Desktop, an IDE assistant) over JSON-RPC on stdio.
//!
//! Out of process on purpose. `tulipix-cli mcp` opens the same SQLite files
//! the app does, read-only, and speaks one request per line on stdin. Nothing
//! talks to the running app, so there is no port to open, nothing to leave
//! listening, and an agent reading the library cannot migrate a schema under
//! it.
//!
//! Two gates, both the user's:
//!
//!   * `mcp-server` in Settings. Off, and the server answers every method with
//!     "switched off" — including `initialize`, so an agent configured against
//!     a Tulipix that has it off says so at once rather than listing tools it
//!     cannot call.
//!   * `mcp.write` in Settings, for the tools that change something. Off by
//!     default, and separate, because "read my library" and "edit my library"
//!     are different questions.

use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Settings switch that lets the server run at all.
pub const ENABLED_FLAG: &str = "mcp-server";
/// The Settings switch for the tools that write.
pub const WRITE_FLAG: &str = "mcp.write";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind {
    Read,
    Write,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub kind: ToolKind,
    pub description: &'static str,
    /// JSON Schema for the tool's arguments, as MCP's `inputSchema`. An agent
    /// that cannot see the shape of the arguments cannot call the tool.
    pub schema: fn() -> Value,
}

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    pub fn err(id: Value, code: i32, msg: impl Into<String>) -> Self {
        Self { jsonrpc: "2.0", id, result: None, error: Some(RpcError { code, message: msg.into() }) }
    }
}

fn obj(props: Value, required: &[&str]) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

fn string_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "string", "description": desc })
}

fn int_prop(desc: &str) -> Value {
    serde_json::json!({ "type": "integer", "description": desc })
}

pub const BUILTIN_TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "library.search",
        kind: ToolKind::Read,
        description: "Search the library by file name across photos, videos, music and books.",
        schema: || {
            obj(
                serde_json::json!({
                    "query": string_prop("Part of a file or folder name."),
                    "section": string_prop("photos | videos | music | books. Omitted searches all four."),
                    "limit": int_prop("How many results, 1-200. Default 25."),
                }),
                &["query"],
            )
        },
    },
    ToolSpec {
        name: "library.sections",
        kind: ToolKind::Read,
        description: "Which libraries exist on this computer, and how many items each holds.",
        schema: || obj(serde_json::json!({}), &[]),
    },
    ToolSpec {
        name: "photos.list-recent",
        kind: ToolKind::Read,
        description: "The most recently added photos.",
        schema: || {
            obj(
                serde_json::json!({ "limit": int_prop("How many, 1-200. Default 25.") }),
                &[],
            )
        },
    },
    ToolSpec {
        name: "photos.get",
        kind: ToolKind::Read,
        description: "One photo: its path, when it was taken, the camera, size, stars and tags.",
        schema: || obj(serde_json::json!({ "id": int_prop("The photo's item id.") }), &["id"]),
    },
    ToolSpec {
        name: "videos.get",
        kind: ToolKind::Read,
        description: "One video: its path, title, year, overview, and where it was left off.",
        schema: || obj(serde_json::json!({ "id": int_prop("The video's item id.") }), &["id"]),
    },
    ToolSpec {
        name: "music.get",
        kind: ToolKind::Read,
        description: "One track: title, artist, album, year, rating and play count.",
        schema: || obj(serde_json::json!({ "id": int_prop("The track's item id.") }), &["id"]),
    },
    ToolSpec {
        name: "photos.tag",
        kind: ToolKind::Write,
        description: "Add a tag to a photo.",
        schema: || {
            obj(
                serde_json::json!({
                    "id": int_prop("The photo's item id."),
                    "tag": string_prop("The tag to add."),
                }),
                &["id", "tag"],
            )
        },
    },
    ToolSpec {
        name: "photos.star",
        kind: ToolKind::Write,
        description: "Star or unstar a photo. Photos are starred, not rated out of five.",
        schema: || {
            obj(
                serde_json::json!({
                    "id": int_prop("The photo's item id."),
                    "starred": { "type": "boolean", "description": "true to star, false to unstar." },
                }),
                &["id", "starred"],
            )
        },
    },
];

pub fn tool(name: &str) -> Option<&'static ToolSpec> {
    BUILTIN_TOOLS.iter().find(|t| t.name == name)
}

/// What the server can actually do. The protocol lives here; the answers live
/// wherever the databases are, which is `tulipix-cli`.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// Run one tool. The returned value is the tool's own JSON; the server
    /// wraps it in MCP's content envelope.
    async fn call(&self, name: &str, args: &Value) -> anyhow::Result<Value>;
}

pub struct McpServer<H: ToolHandler> {
    pub handler: H,
    /// Read once, at start. A switch flipped mid-session does not reach a
    /// process that is already talking to an agent, and re-reading the file on
    /// every request would mean a file read per tool call.
    enabled: bool,
    writes_allowed: bool,
}

impl<H: ToolHandler> McpServer<H> {
    /// Read the two switches out of the user's settings. A settings file that
    /// cannot be read leaves both off: the safe end of both switches.
    pub fn new(handler: H) -> Self {
        let s = Settings::load().unwrap_or_default();
        Self {
            enabled: s.flag(ENABLED_FLAG, false),
            writes_allowed: s.flag(WRITE_FLAG, false),
            handler,
        }
    }

    /// For tests, and for a caller that has already read the settings.
    pub fn with_switches(handler: H, enabled: bool, writes_allowed: bool) -> Self {
        Self { handler, enabled, writes_allowed }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The tools this session will actually run: every read tool, and the
    /// write tools only when writing is switched on. A tool an agent is told
    /// about and then refused is worse than one it never saw.
    pub fn visible_tools(&self) -> Vec<&'static ToolSpec> {
        BUILTIN_TOOLS
            .iter()
            .filter(|t| self.writes_allowed || matches!(t.kind, ToolKind::Read))
            .collect()
    }

    /// Handle one JSON-RPC envelope. The caller frames it: newline-terminated
    /// for stdio.
    pub async fn handle(&self, req: RpcRequest) -> RpcResponse {
        if !self.enabled {
            return RpcResponse::err(
                req.id,
                -32099,
                "Tulipix's MCP server is switched off. Turn it on in Settings › Advanced › MCP server.",
            );
        }
        match req.method.as_str() {
            "initialize" => RpcResponse::ok(
                req.id,
                serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": { "name": "tulipix", "version": env!("CARGO_PKG_VERSION") },
                    "capabilities": { "tools": { "listChanged": false } },
                }),
            ),
            "tools/list" => {
                let tools: Vec<_> = self
                    .visible_tools()
                    .into_iter()
                    .map(|t| {
                        serde_json::json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": (t.schema)(),
                        })
                    })
                    .collect();
                RpcResponse::ok(req.id, serde_json::json!({ "tools": tools }))
            }
            "tools/call" => {
                let Some(name) = req.params.get("name").and_then(|v| v.as_str()) else {
                    return RpcResponse::err(req.id, -32602, "missing tool name");
                };
                let Some(spec) = tool(name) else {
                    return RpcResponse::err(req.id, -32601, format!("unknown tool: {name}"));
                };
                if matches!(spec.kind, ToolKind::Write) && !self.writes_allowed {
                    return RpcResponse::err(
                        req.id,
                        -32099,
                        format!(
                            "{name} changes the library, and \"Let agents change things\" is off \
                             in Settings › Advanced › MCP server."
                        ),
                    );
                }
                let args = req.params.get("arguments").cloned().unwrap_or(Value::Null);
                match self.handler.call(name, &args).await {
                    // MCP wants content parts. The JSON goes in a text part,
                    // which is what every client can read.
                    Ok(v) => RpcResponse::ok(
                        req.id,
                        serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
                            }],
                        }),
                    ),
                    // An error is a result with `isError`, not a protocol
                    // error: the agent should see what went wrong and try
                    // something else, not lose the connection.
                    Err(e) => RpcResponse::ok(
                        req.id,
                        serde_json::json!({
                            "isError": true,
                            "content": [{ "type": "text", "text": e.to_string() }],
                        }),
                    ),
                }
            }
            // Notifications carry no id and want no answer.
            m if m.starts_with("notifications/") => {
                RpcResponse::ok(req.id, serde_json::json!({}))
            }
            other => RpcResponse::err(req.id, -32601, format!("method not found: {other}")),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl ToolHandler for Echo {
        async fn call(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
            if name == "photos.get" && args.get("id").is_none() {
                anyhow::bail!("which photo?");
            }
            Ok(serde_json::json!({ "called": name, "args": args }))
        }
    }

    fn rpc(method: &str, params: Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".into(),
            id: Value::from(1),
            method: method.into(),
            params,
        }
    }

    fn server(enabled: bool, writes: bool) -> McpServer<Echo> {
        McpServer::with_switches(Echo, enabled, writes)
    }

    #[tokio::test]
    async fn switched_off_refuses_even_initialize() {
        let r = server(false, false).handle(rpc("initialize", Value::Null)).await;
        assert!(r.error.is_some());
        assert!(r.error.unwrap().message.contains("switched off"));
    }

    #[tokio::test]
    async fn the_tool_list_hides_the_write_tools_until_writing_is_allowed() {
        let names = |s: &McpServer<Echo>| -> Vec<&'static str> {
            s.visible_tools().into_iter().map(|t| t.name).collect()
        };
        let read_only = names(&server(true, false));
        assert!(read_only.contains(&"library.search"));
        assert!(!read_only.contains(&"photos.tag"));
        let with_writes = names(&server(true, true));
        assert!(with_writes.contains(&"photos.tag"));
        assert!(with_writes.len() > read_only.len());
    }

    #[tokio::test]
    async fn every_tool_advertises_an_input_schema() {
        let r = server(true, true).handle(rpc("tools/list", Value::Null)).await;
        let tools = r.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), BUILTIN_TOOLS.len());
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        }
    }

    #[tokio::test]
    async fn a_write_tool_is_refused_while_writing_is_off_and_runs_once_it_is_on() {
        let params = serde_json::json!({ "name": "photos.tag", "arguments": { "id": 1, "tag": "x" } });
        let r = server(true, false).handle(rpc("tools/call", params.clone())).await;
        assert!(r.error.is_some());
        let r = server(true, true).handle(rpc("tools/call", params)).await;
        assert!(r.error.is_none());
        assert!(r.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("photos.tag"));
    }

    #[tokio::test]
    async fn a_tool_that_fails_is_a_result_with_is_error_not_a_dropped_connection() {
        let params = serde_json::json!({ "name": "photos.get", "arguments": {} });
        let r = server(true, false).handle(rpc("tools/call", params)).await;
        assert!(r.error.is_none(), "a tool failure must not be a protocol error");
        let result = r.result.unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("which photo"));
    }

    #[tokio::test]
    async fn an_unknown_tool_and_an_unknown_method_are_both_protocol_errors() {
        let s = server(true, true);
        let params = serde_json::json!({ "name": "photos.burn" });
        assert!(s.handle(rpc("tools/call", params)).await.error.is_some());
        assert!(s.handle(rpc("nonsense", Value::Null)).await.error.is_some());
    }
}
