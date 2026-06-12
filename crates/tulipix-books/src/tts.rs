//! `np.p4.books.tts` — local AI text-to-speech (instant audiobook).
//!
//! Piper / Kokoro ONNX read EPUB/PDF/text aloud offline. Inference runs in the
//! worker; this owns the engine selection, the per-engine model id, and the
//! sentence chunking that keeps each synth call small enough for low latency
//! and clean sentence-boundary pauses.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Engine { Piper, Kokoro }

impl Engine {
    pub fn default_voice(self) -> &'static str {
        match self { Engine::Piper => "en_US-amy-medium", Engine::Kokoro => "af_sky" }
    }
    pub fn model_file(self, voice: &str) -> String {
        match self {
            Engine::Piper => format!("{voice}.onnx"),
            Engine::Kokoro => format!("kokoro-v0_19.onnx#{voice}"),
        }
    }
}

/// Split text into TTS-sized chunks at sentence boundaries, never exceeding
/// `max_chars`. Sentence enders: `. ! ?` followed by whitespace.
pub fn chunk_sentences(text: &str, max_chars: usize) -> Vec<String> {
    let max = max_chars.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() { out.push(t.to_string()); }
        cur.clear();
    };
    let bytes: Vec<char> = text.chars().collect();
    for (i, &c) in bytes.iter().enumerate() {
        cur.push(c);
        let is_end = matches!(c, '.' | '!' | '?') && bytes.get(i + 1).is_none_or(|n| n.is_whitespace());
        if is_end || cur.chars().count() >= max {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_sentences() {
        let c = chunk_sentences("Hello world. How are you? I am fine!", 1000);
        assert_eq!(c, vec!["Hello world.", "How are you?", "I am fine!"]);
    }

    #[test]
    fn respects_max_chars() {
        let c = chunk_sentences("aaaaaaaaaa bbbbbbbbbb cccccc", 10);
        assert!(c.iter().all(|s| s.chars().count() <= 10));
        assert!(c.len() >= 3);
    }

    #[test]
    fn voice_defaults() {
        assert_eq!(Engine::Piper.default_voice(), "en_US-amy-medium");
        assert!(Engine::Piper.model_file("en_US-amy-medium").ends_with(".onnx"));
    }
}
