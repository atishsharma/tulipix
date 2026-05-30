//! tulipix-ai — ONNX wrappers + on-device LLM bindings.
//!
//! Local-first AI: Whisper (subs), CLIP (embeddings), SCRFD (faces),
//! YOLO (objects), LaMa (inpaint), SAM (segment), RealESRGAN (upscale),
//! DeOldify (colorize). All run via ONNX Runtime.
//!
//! LLM bindings (mistral.rs / llama.cpp) live in `llm`.

pub mod captions;
pub mod chat;
pub mod cloud_offload;
pub mod llm;
pub mod onboarding_models;

#[derive(Debug, Clone)]
pub struct OnnxModel {
    pub id: String,
    pub file: String,
    pub kind: ModelKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind { Clip, ScrFd, Yolo, Lama, Sam, RealEsrgan, DeOldify, Whisper }
