//! Sandboxed plugin engine for community-built metadata scrapers and extensions.
//!
//! Two runtimes:
//! - WASM (wasmtime) — full sandbox, language-agnostic, capability-gated host calls.
//! - Lua  (mlua)     — fast scripting, restricted stdlib, ideal for scrapers.

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub mod host;
#[cfg(feature = "lua")]
pub mod lua;
#[cfg(feature = "wasm")]
pub mod wasm;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub kind: PluginKind,
    pub entry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    Wasm,
    Lua,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeRequest {
    pub query: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeResult {
    pub title: Option<String>,
    pub year: Option<u32>,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

pub trait Plugin: Send {
    fn manifest(&self) -> &PluginManifest;
    fn scrape(&mut self, req: &ScrapeRequest) -> Result<ScrapeResult>;
}

pub fn load(manifest: PluginManifest, bytes: &[u8]) -> Result<Box<dyn Plugin>> {
    match manifest.kind {
        #[cfg(feature = "wasm")]
        PluginKind::Wasm => Ok(Box::new(wasm::WasmPlugin::load(manifest, bytes)?)),
        #[cfg(feature = "lua")]
        PluginKind::Lua => Ok(Box::new(lua::LuaPlugin::load(manifest, bytes)?)),
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!("runtime feature not compiled"),
    }
}
