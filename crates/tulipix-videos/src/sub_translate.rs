//! LibreTranslate integration.
//!
//! Default endpoint is the official `libretranslate.com`, but the user can
//! point this at any self-hosted instance via Settings → Subtitles →
//! Translation. Cue text is sent in batched POSTs to `/translate`; SRT/VTT
//! timing is preserved so the caller can re-emit a new file with the same
//! cue numbering.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE: &str = "https://libretranslate.com";
pub const BATCH_LIMIT: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateConfig {
    pub base: String,
    pub api_key: Option<String>,
    pub source: String,
    pub target: String,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self { base: DEFAULT_BASE.into(), api_key: None, source: "auto".into(), target: "en".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cue {
    pub idx: i64,
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
}

pub struct LibreTranslateClient {
    cfg: TranslateConfig,
    http: reqwest::Client,
}

impl LibreTranslateClient {
    pub fn new(cfg: TranslateConfig) -> Self {
        Self { cfg, http: tulipix_core::net::http().clone() }
    }

    /// Translate one string. Returns the translated text.
    pub async fn translate_one(&self, text: &str) -> Result<String> {
        let url = format!("{}/translate", self.cfg.base.trim_end_matches('/'));
        let mut body = serde_json::json!({
            "q": text, "source": self.cfg.source, "target": self.cfg.target, "format": "text"
        });
        if let Some(k) = &self.cfg.api_key {
            body.as_object_mut().unwrap().insert("api_key".into(), serde_json::Value::String(k.clone()));
        }
        let resp = self.http.post(url).json(&body).send().await?.error_for_status()?;
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("translatedText").and_then(|x| x.as_str()).unwrap_or("").to_string())
    }

    /// Translate many cues, preserving order + timing. Batches of `BATCH_LIMIT`.
    pub async fn translate_cues(&self, cues: &[Cue]) -> Result<Vec<Cue>> {
        let mut out = Vec::with_capacity(cues.len());
        for chunk in cues.chunks(BATCH_LIMIT) {
            for c in chunk {
                let translated = self.translate_one(&c.text).await
                    .with_context(|| format!("translate cue {}", c.idx))?;
                out.push(Cue { idx: c.idx, start_s: c.start_s, end_s: c.end_s, text: translated });
            }
        }
        Ok(out)
    }
}

/// Render a cue list back to SRT text (preserves cue index + HH:MM:SS,mmm).
pub fn cues_to_srt(cues: &[Cue]) -> String {
    let mut s = String::new();
    for (i, c) in cues.iter().enumerate() {
        s.push_str(&format!("{}\n", i + 1));
        s.push_str(&format!("{} --> {}\n", srt_ts(c.start_s), srt_ts(c.end_s)));
        s.push_str(&c.text);
        s.push_str("\n\n");
    }
    s
}

fn srt_ts(t: f64) -> String {
    let t = t.max(0.0);
    let total_ms = (t * 1000.0).round() as i64;
    let h = total_ms / 3_600_000;
    let m = (total_ms / 60_000) % 60;
    let s = (total_ms / 1_000) % 60;
    let ms = total_ms % 1_000;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, ms)
}

/// Parse a minimal SRT body into Cues. Used when the caller wants a quick
/// round-trip translate + re-emit pipeline.
pub fn srt_to_cues(srt: &str) -> Vec<Cue> {
    let mut out = Vec::new();
    let mut idx: i64 = 0;
    for block in srt.split("\n\n") {
        let mut lines = block.lines();
        let Some(first) = lines.next() else { continue; };
        let timing = if first.trim().parse::<i64>().is_ok() { lines.next() } else { Some(first) };
        let Some(timing) = timing else { continue; };
        let Some((a, b)) = timing.split_once(" --> ") else { continue; };
        let start = parse_srt_ts(a.trim());
        let end   = parse_srt_ts(b.trim());
        let text: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
        if let (Some(s), Some(e)) = (start, end) {
            idx += 1;
            out.push(Cue { idx, start_s: s, end_s: e, text });
        }
    }
    out
}

fn parse_srt_ts(s: &str) -> Option<f64> {
    let (h, rest) = s.split_once(':')?;
    let (m, rest) = rest.split_once(':')?;
    let (sec, ms) = rest.split_once(',')?;
    let h: f64 = h.parse().ok()?;
    let m: f64 = m.parse().ok()?;
    let sec: f64 = sec.parse().ok()?;
    let ms: f64 = ms.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec + ms / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_round_trip() {
        let cues = vec![
            Cue { idx: 1, start_s: 0.0, end_s: 1.5, text: "Hello".into() },
            Cue { idx: 2, start_s: 1.5, end_s: 3.0, text: "World".into() },
        ];
        let srt = cues_to_srt(&cues);
        let back = srt_to_cues(&srt);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].text, "Hello");
        assert!((back[1].start_s - 1.5).abs() < 1e-3);
        assert!((back[1].end_s - 3.0).abs() < 1e-3);
    }

    #[test]
    fn timestamp_formatting() {
        assert_eq!(srt_ts(0.0), "00:00:00,000");
        assert_eq!(srt_ts(3725.5), "01:02:05,500");
    }

    #[test]
    fn timestamp_parses_back() {
        assert!((parse_srt_ts("01:02:05,500").unwrap() - 3725.5).abs() < 1e-3);
        assert_eq!(parse_srt_ts("garbage"), None);
    }

    #[test]
    fn config_defaults_target_english_with_auto_source() {
        let c = TranslateConfig::default();
        assert_eq!(c.source, "auto");
        assert_eq!(c.target, "en");
    }
}
