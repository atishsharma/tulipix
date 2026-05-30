use crate::{Plugin, PluginManifest, ScrapeRequest, ScrapeResult};
use anyhow::{Context, Result, bail};
use wasmtime::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

/// Tulipix plugin ABI v0 — WASM modules must export:
///   memory
///   alloc(size: i32) -> i32          // returns offset into `memory`
///   scrape(ptr: i32, len: i32) -> i64 // hi32 = out_ptr, lo32 = out_len, both into `memory`
///
/// JSON in, JSON out. See `ScrapeRequest`/`ScrapeResult`.
pub struct WasmPlugin {
    manifest: PluginManifest,
    store: Store<()>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    scrape: TypedFunc<(i32, i32), i64>,
}

impl WasmPlugin {
    pub fn load(manifest: PluginManifest, bytes: &[u8]) -> Result<Self> {
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).context("invalid wasm module")?;
        let mut store = Store::new(&engine, ());
        let linker: Linker<()> = Linker::new(&engine);
        let instance: Instance = linker.instantiate(&mut store, &module).context("instantiate")?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("module missing exported `memory`")?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "alloc")
            .context("module missing `alloc(size: i32) -> i32`")?;
        let scrape = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "scrape")
            .context("module missing `scrape(ptr: i32, len: i32) -> i64`")?;
        Ok(Self { manifest, store, memory, alloc, scrape })
    }
}

impl Plugin for WasmPlugin {
    fn manifest(&self) -> &PluginManifest { &self.manifest }
    fn scrape(&mut self, req: &ScrapeRequest) -> Result<ScrapeResult> {
        let req_json = serde_json::to_vec(req)?;
        let len = i32::try_from(req_json.len()).context("request too large")?;
        let ptr = self.alloc.call(&mut self.store, len)?;
        if ptr <= 0 { bail!("plugin alloc returned {ptr}"); }
        self.memory
            .write(&mut self.store, ptr as usize, &req_json)
            .context("write request to wasm memory")?;
        let packed = self.scrape.call(&mut self.store, (ptr, len))?;
        let out_ptr = (packed >> 32) as i32;
        let out_len = (packed & 0xffff_ffff) as i32;
        if out_ptr <= 0 || out_len < 0 {
            bail!("plugin scrape returned ptr={out_ptr} len={out_len}");
        }
        let mut buf = vec![0u8; out_len as usize];
        self.memory
            .read(&self.store, out_ptr as usize, &mut buf)
            .context("read response from wasm memory")?;
        let result: ScrapeResult = serde_json::from_slice(&buf).context("decode scrape result json")?;
        Ok(result)
    }
}
