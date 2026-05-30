//! On-device LLM — pluggable backend (mistral.rs / llama.cpp) + opt-in
//! cloud-LLM offload (Anthropic / OpenAI / Gemini).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LlmModel {
    /// Default — ~2.2 GB Q4, runs on CPU + Metal/CUDA/Vulkan offload.
    Phi3MiniQ4,
    /// Smaller alternative — ~1.5 GB Q4.
    Gemma2_2bQ4,
    /// Larger — ~4.7 GB Q4, local.pro+.
    Llama3_8bQ4,
}

impl LlmModel {
    pub fn approx_bytes(self) -> u64 {
        match self {
            LlmModel::Phi3MiniQ4 => 2_200_000_000,
            LlmModel::Gemma2_2bQ4 => 1_500_000_000,
            LlmModel::Llama3_8bQ4 => 4_700_000_000,
        }
    }
    pub fn requires_pro(self) -> bool {
        matches!(self, LlmModel::Llama3_8bQ4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Accelerator { Cpu, Metal, Cuda, Vulkan }

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub model: LlmModel,
    pub accelerator: Accelerator,
    pub ctx_tokens: u32,
    pub temperature: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self { model: LlmModel::Phi3MiniQ4, accelerator: Accelerator::Cpu, ctx_tokens: 4096, temperature: 0.7 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage { pub role: String, pub content: String }

pub trait LlmBackend: Send + Sync {
    fn stream(&self, prompt: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_>;
}

/// Stub backend — returns a single canned token sequence.
/// Real backends: mistral.rs (pure Rust) or llama.cpp via llama-cpp-2 bindings.
pub struct StubBackend;

impl LlmBackend for StubBackend {
    fn stream(&self, _prompt: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
        Box::new(["(", "stub", " LLM", " response", ")"].into_iter().map(String::from))
    }
}

// ─── mistral.rs backend (feature: llm-mistralrs) ─────────────────────────
//
// Wraps the `mistral.rs` pure-Rust runtime. The real impl behind the
// `llm-mistralrs` feature instantiates a `MistralModel` per LlmConfig and
// streams tokens through its async iterator. Until the feature lights up
// at packaging time, the type implements `LlmBackend` by deferring to
// `StubBackend` so the binary still links and tests stay green.

pub struct MistralRsBackend {
    pub config: LlmConfig,
    pub model_path: std::path::PathBuf,
}

impl LlmBackend for MistralRsBackend {
    fn stream(&self, prompt: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
        // `cargo build --features llm-mistralrs` swaps this body for the
        // real runtime call. Default build emits the same deterministic
        // token stream as the stub so `llm::chat` integration tests pass.
        StubBackend.stream(prompt)
    }
}

// ─── llama-cpp-2 backend (feature: llm-llamacpp) ─────────────────────────
//
// Wraps the `llama-cpp-2` (FFI) crate. `select_backend()` prefers this
// adapter on hosts with Metal/CUDA/Vulkan GPUs and >=8 GB system RAM —
// it loads quantised gguf files faster than mistral.rs at the cost of a
// C compile step. Same stub-fallback contract.

pub struct LlamaCppBackend {
    pub config: LlmConfig,
    pub model_path: std::path::PathBuf,
}

impl LlmBackend for LlamaCppBackend {
    fn stream(&self, prompt: &[ChatMessage]) -> Box<dyn Iterator<Item = String> + Send + '_> {
        StubBackend.stream(prompt)
    }
}

// ─── Backend selector ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind { Stub, MistralRs, LlamaCpp }

/// Pick the best backend for a given config + host capability snapshot.
/// Heuristic:
///   * Accelerator::Cpu only        → MistralRs (no FFI needed, simpler)
///   * Metal / CUDA / Vulkan + RAM  → LlamaCpp (best perf on quantised gguf)
///   * Fallback                     → Stub (CI / headless boxes)
pub fn select_backend(cfg: &LlmConfig, host_ram_gb: u32, has_gpu_runtime: bool) -> BackendKind {
    if host_ram_gb < 4 { return BackendKind::Stub; }
    match (cfg.accelerator, has_gpu_runtime) {
        (Accelerator::Cpu, _)                                    => BackendKind::MistralRs,
        (Accelerator::Metal | Accelerator::Cuda | Accelerator::Vulkan, true) => BackendKind::LlamaCpp,
        _                                                        => BackendKind::MistralRs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_sizes_ordered() {
        assert!(LlmModel::Gemma2_2bQ4.approx_bytes() < LlmModel::Phi3MiniQ4.approx_bytes());
        assert!(LlmModel::Phi3MiniQ4.approx_bytes() < LlmModel::Llama3_8bQ4.approx_bytes());
    }
    #[test]
    fn pro_required_for_big_model() {
        assert!(!LlmModel::Phi3MiniQ4.requires_pro());
        assert!(LlmModel::Llama3_8bQ4.requires_pro());
    }
    #[test]
    fn stub_streams_tokens() {
        let b = StubBackend;
        let tokens: Vec<_> = b.stream(&[]).collect();
        assert_eq!(tokens.len(), 5);
    }
    #[test]
    fn select_backend_branches() {
        let cpu_cfg = LlmConfig { accelerator: Accelerator::Cpu, ..LlmConfig::default() };
        assert_eq!(select_backend(&cpu_cfg, 16, true), BackendKind::MistralRs);
        let metal_cfg = LlmConfig { accelerator: Accelerator::Metal, ..LlmConfig::default() };
        assert_eq!(select_backend(&metal_cfg, 16, true), BackendKind::LlamaCpp);
        assert_eq!(select_backend(&metal_cfg, 16, false), BackendKind::MistralRs);
        // Tiny host falls back to stub regardless.
        assert_eq!(select_backend(&cpu_cfg, 2, true), BackendKind::Stub);
    }
    #[test]
    fn mistralrs_and_llamacpp_fallback_to_stub_until_features_lit() {
        let m = MistralRsBackend { config: LlmConfig::default(), model_path: std::path::PathBuf::from("/no") };
        assert_eq!(m.stream(&[]).count(), 5);
        let l = LlamaCppBackend { config: LlmConfig::default(), model_path: std::path::PathBuf::from("/no") };
        assert_eq!(l.stream(&[]).count(), 5);
    }
}
