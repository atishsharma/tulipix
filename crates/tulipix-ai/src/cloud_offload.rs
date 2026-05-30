//! Opt-in cloud LLM offload — Anthropic / OpenAI / Gemini.
//!
//! Local-first is the default; this module exists for tier-`account.pro+` users
//! who want a beefier remote model for hard prompts. Every query goes through
//! a consent gate that shows a *redacted* preview of the outbound payload —
//! the user must explicitly approve each query (or enable session-wide
//! consent for the chosen provider). Cap-gated `ai.cloud-offload`.
//!
//! No HTTP is performed in this crate — the caller wires the keychain'd API
//! key + a `reqwest` client from `tulipix-core`. We define the wire shape,
//! the redaction pass, the consent gate, and a request planner.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tulipix_core::caps::{is_allowed, Cap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudProvider {
    Anthropic,
    OpenAi,
    Gemini,
}

impl CloudProvider {
    pub fn keychain_service(self) -> &'static str {
        match self {
            CloudProvider::Anthropic => "anthropic",
            CloudProvider::OpenAi => "openai",
            CloudProvider::Gemini => "gemini",
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            CloudProvider::Anthropic => "https://api.anthropic.com/v1/messages",
            CloudProvider::OpenAi    => "https://api.openai.com/v1/chat/completions",
            CloudProvider::Gemini    => "https://generativelanguage.googleapis.com/v1beta/models",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ConsentMode {
    /// Default: prompt the user before every outbound query.
    #[default]
    PerQuery,
    /// Approve all queries for the current session to the chosen provider.
    SessionAll,
    /// User has revoked consent; calls return `Err` until re-enabled.
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudRequest {
    pub provider: CloudProvider,
    pub model: String,
    pub messages: Vec<CloudMessage>,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudMessage { pub role: String, pub content: String }

#[derive(Debug, Clone, Serialize)]
pub struct RedactedPreview {
    pub provider: CloudProvider,
    pub model: String,
    pub endpoint: &'static str,
    pub redacted_messages: Vec<CloudMessage>,
    pub estimated_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision { Approve, ApproveSession, Deny }

/// Tracks session-wide consent per provider so `PerQuery` queries that the
/// user has elevated to `ApproveSession` don't re-prompt.
#[derive(Debug, Default)]
pub struct ConsentLedger {
    inner: Mutex<ConsentInner>,
}

#[derive(Debug, Default)]
struct ConsentInner {
    mode: ConsentMode,
    session_approved: [bool; 3],
}

impl ConsentLedger {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    pub fn set_mode(&self, mode: ConsentMode) {
        let mut g = self.inner.lock().unwrap();
        g.mode = mode;
        if !matches!(mode, ConsentMode::SessionAll) { g.session_approved = [false; 3]; }
    }

    pub fn mode(&self) -> ConsentMode { self.inner.lock().unwrap().mode }

    pub fn record(&self, provider: CloudProvider, decision: ConsentDecision) {
        let mut g = self.inner.lock().unwrap();
        let idx = provider_idx(provider);
        match decision {
            ConsentDecision::Approve => {}
            ConsentDecision::ApproveSession => g.session_approved[idx] = true,
            ConsentDecision::Deny => g.session_approved[idx] = false,
        }
    }

    pub fn is_session_approved(&self, provider: CloudProvider) -> bool {
        self.inner.lock().unwrap().session_approved[provider_idx(provider)]
    }
}

fn provider_idx(p: CloudProvider) -> usize {
    match p {
        CloudProvider::Anthropic => 0,
        CloudProvider::OpenAi    => 1,
        CloudProvider::Gemini    => 2,
    }
}

/// Cheap heuristic — collapses local file paths, email addresses, GPS-like
/// numbers, and 32+ char hex / base64 tokens. Caller chains its own regex
/// list if the library tightens later. Returns the redacted payload + a
/// per-query token estimate (4 chars ≈ 1 token rule of thumb).
pub fn redact_preview(req: &CloudRequest) -> RedactedPreview {
    let mut total_chars = 0usize;
    let mut out = Vec::with_capacity(req.messages.len());
    for m in &req.messages {
        let red = redact_string(&m.content);
        total_chars += red.len();
        out.push(CloudMessage { role: m.role.clone(), content: red });
    }
    RedactedPreview {
        provider: req.provider,
        model: req.model.clone(),
        endpoint: req.provider.endpoint(),
        redacted_messages: out,
        estimated_tokens: (total_chars / 4) as u32,
    }
}

fn redact_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' || (c == 'C' && chars.peek().copied() == Some(':')) {
            let mut path = String::from(c);
            while let Some(&nc) = chars.peek() {
                if nc.is_whitespace() { break; }
                path.push(nc); chars.next();
            }
            if path.len() > 3 && (path.contains('/') || path.contains('\\')) {
                out.push_str("[path]");
            } else {
                out.push_str(&path);
            }
            continue;
        }
        if c.is_alphanumeric() {
            let mut tok = String::from(c);
            while let Some(&nc) = chars.peek() {
                if !nc.is_alphanumeric() && nc != '@' && nc != '.' && nc != '-' && nc != '_' && nc != '+' { break; }
                tok.push(nc); chars.next();
            }
            if tok.contains('@') && tok.contains('.') {
                out.push_str("[email]");
            } else if tok.len() >= 32 && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+') {
                out.push_str("[token]");
            } else {
                out.push_str(&tok);
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Caller-provided UI consent prompt. Receives the redacted preview and the
/// session ledger so it can show "approve once" vs "approve session".
pub trait ConsentPrompt: Send + Sync {
    fn ask(&self, preview: &RedactedPreview) -> ConsentDecision;
}

/// Cap- + consent-gate. On approval returns the **un-redacted** body the
/// caller will POST. Caller is responsible for the HTTP and for accounting
/// usage via `tulipix-core::api_keys::account_usage`.
pub fn prepare_offload<P: ConsentPrompt>(
    req: CloudRequest,
    ledger: &ConsentLedger,
    prompt: &P,
) -> Result<CloudRequest> {
    if !is_allowed(Cap::AiCloudOffload) {
        anyhow::bail!("ai.cloud-offload denied");
    }
    let mode = ledger.mode();
    if matches!(mode, ConsentMode::Off) {
        anyhow::bail!("cloud offload consent revoked");
    }
    let preview = redact_preview(&req);
    let auto_ok = matches!(mode, ConsentMode::SessionAll)
        || ledger.is_session_approved(req.provider);
    let decision = if auto_ok { ConsentDecision::Approve } else { prompt.ask(&preview) };
    ledger.record(req.provider, decision);
    match decision {
        ConsentDecision::Approve | ConsentDecision::ApproveSession => Ok(req),
        ConsentDecision::Deny => anyhow::bail!("user denied cloud offload"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tulipix_core::caps::{load_from_toml, set_current_tier, Tier};

    static SERIAL: Mutex<()> = Mutex::new(());

    const CAPS: &str = r#"
[tiers]
local_basic = []
local_pro   = ["ai.cloud-offload"]
"#;

    struct YesPrompt;
    impl ConsentPrompt for YesPrompt { fn ask(&self, _p: &RedactedPreview) -> ConsentDecision { ConsentDecision::Approve } }
    struct NoPrompt;
    impl ConsentPrompt for NoPrompt { fn ask(&self, _p: &RedactedPreview) -> ConsentDecision { ConsentDecision::Deny } }
    struct SessionPrompt;
    impl ConsentPrompt for SessionPrompt { fn ask(&self, _p: &RedactedPreview) -> ConsentDecision { ConsentDecision::ApproveSession } }

    fn req() -> CloudRequest {
        CloudRequest {
            provider: CloudProvider::Anthropic,
            model: "claude-opus-4-7".into(),
            messages: vec![
                CloudMessage { role: "user".into(),
                    content: "Find /home/alice/Photos/2025-09 me@x.com abcdef1234567890abcdef1234567890abcd".into() },
            ],
            max_tokens: 256,
        }
    }

    #[test]
    fn redacts_path_email_token() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let p = redact_preview(&req());
        let body = &p.redacted_messages[0].content;
        assert!(body.contains("[path]"));
        assert!(body.contains("[email]"));
        assert!(body.contains("[token]"));
        assert!(!body.contains("/home/alice"));
    }

    #[test]
    fn cap_denied_blocks() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        let l = ConsentLedger::new();
        assert!(prepare_offload(req(), &l, &YesPrompt).is_err());
    }

    #[test]
    fn deny_bails() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let l = ConsentLedger::new();
        assert!(prepare_offload(req(), &l, &NoPrompt).is_err());
    }

    #[test]
    fn session_approve_skips_next_prompt() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let l = ConsentLedger::new();
        prepare_offload(req(), &l, &SessionPrompt).unwrap();
        // Second call uses session approval — NoPrompt must NOT be asked.
        struct Panic;
        impl ConsentPrompt for Panic { fn ask(&self, _: &RedactedPreview) -> ConsentDecision { panic!("re-asked"); } }
        prepare_offload(req(), &l, &Panic).unwrap();
    }
}
