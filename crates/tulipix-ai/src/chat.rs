//! Chat assistant session with tool-calling against the local library.
//!
//! Wires the `llm::LlmBackend` token stream to a tool-call dispatcher
//! exposing four grounded tools:
//!   * `photos.semantic_search(query)`    — CLIP text→image cosine top-k
//!   * `photos.face_lookup(name)`         — face-cluster id lookup
//!   * `photos.exif_filter(filters)`      — EXIF column predicate match
//!   * `photos.gps_lookup(query)`         — coarse reverse-geocode lookup
//!
//! Each tool is a `BoxedTool` implementing `Tool::invoke(args) -> ToolResult`.
//! The LLM emits JSON-line tool requests; this module parses, dispatches,
//! and feeds the result back into the next turn.

use crate::llm::{ChatMessage, LlmBackend};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn schema(&self) -> &'static str;
    fn invoke(&self, args: &Value) -> Result<Value>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall { pub tool: String, pub args: Value }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReply { pub tool: String, pub result: Value }

#[derive(Debug, Clone)]
pub enum Turn { Text(String), Call(ToolCall), Reply(ToolReply) }

pub struct ChatSession {
    backend: Box<dyn LlmBackend>,
    tools: HashMap<String, Box<dyn Tool>>,
    history: Vec<ChatMessage>,
    max_steps: u32,
}

impl ChatSession {
    pub fn new(backend: Box<dyn LlmBackend>) -> Self {
        Self { backend, tools: HashMap::new(), history: Vec::new(), max_steps: 6 }
    }
    pub fn register(&mut self, tool: Box<dyn Tool>) { self.tools.insert(tool.name().into(), tool); }
    pub fn history(&self) -> &[ChatMessage] { &self.history }

    /// Drive one user turn. Streams LLM tokens, parses any `tool:` JSON
    /// lines, dispatches, feeds the reply back, and repeats up to
    /// `max_steps` rounds before giving up. Returns the full visible
    /// transcript of this turn.
    pub fn ask(&mut self, user: &str) -> Result<Vec<Turn>> {
        self.history.push(ChatMessage { role: "user".into(), content: user.into() });
        let mut transcript = Vec::new();
        for _ in 0..self.max_steps {
            let raw: String = self.backend.stream(&self.history).collect::<Vec<_>>().join("");
            match parse_tool_call(&raw) {
                Some(call) => {
                    transcript.push(Turn::Call(call.clone()));
                    let tool = self.tools.get(&call.tool).ok_or_else(|| anyhow!("unknown tool {}", call.tool))?;
                    let result = tool.invoke(&call.args)?;
                    let reply = ToolReply { tool: call.tool.clone(), result };
                    transcript.push(Turn::Reply(reply.clone()));
                    self.history.push(ChatMessage { role: "tool".into(), content: serde_json::to_string(&reply)? });
                }
                None => {
                    transcript.push(Turn::Text(raw.clone()));
                    self.history.push(ChatMessage { role: "assistant".into(), content: raw });
                    return Ok(transcript);
                }
            }
        }
        Err(anyhow!("max tool-call steps ({}) exceeded", self.max_steps))
    }
}

fn parse_tool_call(text: &str) -> Option<ToolCall> {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("tool:") {
            if let Ok(call) = serde_json::from_str::<ToolCall>(rest.trim()) { return Some(call); }
        }
    }
    None
}

// ─── Built-in tool stubs ────────────────────────────────────────────────
// Real wiring lives in tulipix-photos / tulipix-core::geo. Here we keep the
// dispatch contract so the chat overlay can exercise it end-to-end.

pub struct ClipSemanticSearch;
impl Tool for ClipSemanticSearch {
    fn name(&self) -> &'static str { "photos.semantic_search" }
    fn schema(&self) -> &'static str { r#"{ "query": "string", "k": "int" }"# }
    fn invoke(&self, args: &Value) -> Result<Value> {
        let _q = args.get("query").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("query missing"))?;
        Ok(serde_json::json!({ "hits": [] }))
    }
}

pub struct FaceLookup;
impl Tool for FaceLookup {
    fn name(&self) -> &'static str { "photos.face_lookup" }
    fn schema(&self) -> &'static str { r#"{ "name": "string" }"# }
    fn invoke(&self, args: &Value) -> Result<Value> {
        let _n = args.get("name").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("name missing"))?;
        Ok(serde_json::json!({ "cluster_ids": [] }))
    }
}

pub struct ExifFilter;
impl Tool for ExifFilter {
    fn name(&self) -> &'static str { "photos.exif_filter" }
    fn schema(&self) -> &'static str { r#"{ "predicate": "string" }"# }
    fn invoke(&self, args: &Value) -> Result<Value> {
        let _p = args.get("predicate").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("predicate missing"))?;
        Ok(serde_json::json!({ "matches": [] }))
    }
}

pub struct GpsLookup;
impl Tool for GpsLookup {
    fn name(&self) -> &'static str { "photos.gps_lookup" }
    fn schema(&self) -> &'static str { r#"{ "query": "string" }"# }
    fn invoke(&self, args: &Value) -> Result<Value> {
        let _q = args.get("query").and_then(|v| v.as_str()).ok_or_else(|| anyhow!("query missing"))?;
        Ok(serde_json::json!({ "matches": [] }))
    }
}

pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![Box::new(ClipSemanticSearch), Box::new(FaceLookup), Box::new(ExifFilter), Box::new(GpsLookup)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmBackend, ChatMessage};

    struct OneShot(String);
    impl LlmBackend for OneShot {
        fn stream(&self, _: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
            Box::new(std::iter::once(self.0.clone()))
        }
    }

    #[test] fn parse_tool_call_picks_first_marker_line() {
        let text = r#"hmm, let me check
tool: {"tool":"photos.face_lookup","args":{"name":"Mom"}}
"#;
        let c = parse_tool_call(text).unwrap();
        assert_eq!(c.tool, "photos.face_lookup");
        assert_eq!(c.args.get("name").unwrap().as_str(), Some("Mom"));
    }
    #[test] fn no_tool_call_text_returns_none() {
        assert!(parse_tool_call("just plain text").is_none());
    }
    #[test] fn session_dispatches_then_finishes() {
        let mut s = ChatSession::new(Box::new(OneShot("plain reply".into())));
        for t in default_tools() { s.register(t); }
        let transcript = s.ask("how many cats?").unwrap();
        assert!(matches!(transcript[0], Turn::Text(ref t) if t == "plain reply"));
    }
    #[test] fn session_routes_to_face_lookup_tool() {
        struct ToolThenText(std::sync::atomic::AtomicU32);
        impl LlmBackend for ToolThenText {
            fn stream(&self, _: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
                let n = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n == 0 {
                    Box::new(std::iter::once(r#"tool: {"tool":"photos.face_lookup","args":{"name":"Mom"}}"#.to_string()))
                } else {
                    Box::new(std::iter::once("done".to_string()))
                }
            }
        }
        let mut s = ChatSession::new(Box::new(ToolThenText(std::sync::atomic::AtomicU32::new(0))));
        for t in default_tools() { s.register(t); }
        let transcript = s.ask("who is mom?").unwrap();
        assert!(matches!(transcript[0], Turn::Call(_)));
        assert!(matches!(transcript[1], Turn::Reply(_)));
        assert!(matches!(transcript[2], Turn::Text(_)));
    }
    #[test] fn unknown_tool_errors() {
        struct Bad;
        impl LlmBackend for Bad {
            fn stream(&self, _: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
                Box::new(std::iter::once(r#"tool: {"tool":"photos.nope","args":{}}"#.to_string()))
            }
        }
        let mut s = ChatSession::new(Box::new(Bad));
        for t in default_tools() { s.register(t); }
        let err = s.ask("?").unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }
}
