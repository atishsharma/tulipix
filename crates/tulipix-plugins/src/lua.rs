use crate::{Plugin, PluginManifest, ScrapeRequest, ScrapeResult};
use anyhow::{Context, Result, bail};
use mlua::{Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value};

/// Lua plugin contract: source must define a global function
///     function scrape(req) -> result_table
/// where `req` is `{ query = "...", kind = "..." }` and `result_table` is
/// `{ title = ..., year = ..., fields = { ... } }`. Stdlib is restricted —
/// no `io`, `os`, `package`, or `debug` access.
pub struct LuaPlugin {
    manifest: PluginManifest,
    lua: Lua,
}

impl LuaPlugin {
    pub fn load(manifest: PluginManifest, bytes: &[u8]) -> Result<Self> {
        let lua = Lua::new_with(
            StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
            LuaOptions::default(),
        )?;
        let src = std::str::from_utf8(bytes).context("lua source not utf-8")?;
        let chunk_name = format!("={}", manifest.id);
        lua.load(src).set_name(chunk_name).exec().context("lua plugin load failed")?;
        let globals = lua.globals();
        if !globals.contains_key("scrape")? {
            bail!("lua plugin {} missing global `scrape` function", manifest.id);
        }
        Ok(Self { manifest, lua })
    }
}

impl Plugin for LuaPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
    fn scrape(&mut self, req: &ScrapeRequest) -> Result<ScrapeResult> {
        let req_value = self.lua.to_value(req)?;
        let scrape_fn: mlua::Function = self.lua.globals().get("scrape")?;
        let out: Value = scrape_fn.call(req_value)?;
        let table: Table = match out {
            Value::Table(t) => t,
            other => bail!("scrape() must return a table, got {}", other.type_name()),
        };
        let result: ScrapeResult = self.lua.from_value(Value::Table(table))?;
        Ok(result)
    }
}
