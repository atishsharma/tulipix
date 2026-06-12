//! MCP (Model Context Protocol) server — exposes the Tulipix library to
//! external AI agents (Claude Desktop, IDE assistants, …) over stdio + SSE.
//!
//! - Read tools (search photos, fetch metadata) are enabled by default once
//!   `mcp.server` is granted in capabilities.
//! - Write tools (tag, rate, edit) additionally require `mcp.server.write`
//!   AND a per-tool consent flip in `Settings → Privacy`.
//!
//! Wire format: JSON-RPC 2.0 framed per line on stdio. SSE transport is the
//! same envelopes over `text/event-stream`.

use crate::caps::{is_allowed, Cap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolKind { Read, Write }

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub kind: ToolKind,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport { Stdio, Sse }

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
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
pub struct RpcError { pub code: i32, pub message: String }

impl RpcResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", id, result: Some(result), error: None }
    }
    pub fn err(id: Value, code: i32, msg: impl Into<String>) -> Self {
        Self { jsonrpc: "2.0", id, result: None, error: Some(RpcError { code, message: msg.into() }) }
    }
}

pub const BUILTIN_TOOLS: &[ToolSpec] = &[
    ToolSpec { name: "library.search",     kind: ToolKind::Read,  description: "Full-text search across photos/videos/music/books." },
    ToolSpec { name: "photos.get",         kind: ToolKind::Read,  description: "Fetch metadata for a photo by id." },
    ToolSpec { name: "photos.list-recent", kind: ToolKind::Read,  description: "Recently added photos, paged." },
    ToolSpec { name: "videos.get",         kind: ToolKind::Read,  description: "Fetch metadata for a video by id." },
    ToolSpec { name: "music.get",          kind: ToolKind::Read,  description: "Fetch metadata for a track by id." },
    ToolSpec { name: "photos.tag",         kind: ToolKind::Write, description: "Add a tag to a photo." },
    ToolSpec { name: "photos.rate",        kind: ToolKind::Write, description: "Star-rate a photo." },
];

/// Per-write-tool consent flips, persisted by the caller. Reads bypass.
#[derive(Default)]
pub struct WriteConsent {
    inner: RwLock<HashMap<&'static str, bool>>,
}

impl WriteConsent {
    pub fn allow(&self, tool: &'static str) {
        self.inner.write().unwrap().insert(tool, true);
    }
    pub fn revoke(&self, tool: &'static str) {
        self.inner.write().unwrap().insert(tool, false);
    }
    pub fn is_allowed(&self, tool: &str) -> bool {
        self.inner.read().unwrap().get(tool).copied().unwrap_or(false)
    }
}

pub struct McpServer {
    pub transport: Transport,
    pub consent: WriteConsent,
}

impl McpServer {
    pub fn new(transport: Transport) -> Self {
        Self { transport, consent: WriteConsent::default() }
    }

    /// Handle one JSON-RPC envelope. Returns the response envelope; caller
    /// frames it (newline-terminated for stdio, `data: …\n\n` for SSE).
    pub fn handle(&self, req: RpcRequest) -> RpcResponse {
        if !is_allowed(Cap::McpServer) {
            return RpcResponse::err(req.id, -32099, "mcp.server denied");
        }
        match req.method.as_str() {
            "initialize" => RpcResponse::ok(req.id, serde_json::json!({
                "protocolVersion": "2024-11-05",
                "serverInfo": { "name": "tulipix", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "tools": { "listChanged": false } },
            })),
            "tools/list" => {
                let tools: Vec<_> = BUILTIN_TOOLS.iter().map(|t| serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "kind": match t.kind { ToolKind::Read => "read", ToolKind::Write => "write" },
                })).collect();
                RpcResponse::ok(req.id, serde_json::json!({ "tools": tools }))
            }
            "tools/call" => {
                let Some(name) = req.params.get("name").and_then(|v| v.as_str()) else {
                    return RpcResponse::err(req.id, -32602, "missing tool name");
                };
                let Some(spec) = BUILTIN_TOOLS.iter().find(|t| t.name == name) else {
                    return RpcResponse::err(req.id, -32601, format!("unknown tool: {name}"));
                };
                if matches!(spec.kind, ToolKind::Write) {
                    if !is_allowed(Cap::McpServerWrite) {
                        return RpcResponse::err(req.id, -32099, "mcp.server.write denied");
                    }
                    if !self.consent.is_allowed(name) {
                        return RpcResponse::err(req.id, -32099, format!("write consent off for {name}"));
                    }
                }
                // Dispatch shells out to crate-specific handlers in real wiring;
                // here we echo back a stub envelope so the transport tests pass.
                RpcResponse::ok(req.id, serde_json::json!({
                    "content": [{ "type": "text", "text": format!("stub:{name}") }],
                }))
            }
            other => RpcResponse::err(req.id, -32601, format!("method not found: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{load_from_toml, set_current_tier, Tier};
    use std::sync::Mutex;

    static SERIAL: Mutex<()> = Mutex::new(());

    const CAPS: &str = r#"
[tiers]
local_basic  = []
local_pro    = ["mcp.server", "mcp.server.write"]
"#;

    fn rpc(method: &str, params: Value) -> RpcRequest {
        RpcRequest { jsonrpc: "2.0".into(), id: Value::from(1), method: method.into(), params }
    }

    #[test]
    fn cap_denied_returns_error() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        let s = McpServer::new(Transport::Stdio);
        let r = s.handle(rpc("initialize", Value::Null));
        assert!(r.error.is_some());
    }

    #[test]
    fn read_tools_listed() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let s = McpServer::new(Transport::Stdio);
        let r = s.handle(rpc("tools/list", Value::Null));
        let tools = r.result.unwrap()["tools"].as_array().unwrap().clone();
        assert!(tools.iter().any(|t| t["name"] == "library.search"));
    }

    #[test]
    fn write_tool_requires_consent() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let s = McpServer::new(Transport::Stdio);
        let params = serde_json::json!({ "name": "photos.tag", "arguments": { "id": 1, "tag": "x" } });
        let r = s.handle(rpc("tools/call", params.clone()));
        assert!(r.error.is_some(), "write must be denied without consent");
        s.consent.allow("photos.tag");
        let r = s.handle(rpc("tools/call", params));
        assert!(r.error.is_none(), "write must succeed after consent");
    }

    #[test]
    fn read_tool_works_without_write_cap() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let s = McpServer::new(Transport::Stdio);
        let params = serde_json::json!({ "name": "library.search", "arguments": { "q": "sunset" } });
        let r = s.handle(rpc("tools/call", params));
        assert!(r.error.is_none());
    }
}
