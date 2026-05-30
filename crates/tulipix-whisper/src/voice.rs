//! Voice search & dictation — whisper-streaming mic capture, routed to the
//! command palette or chat overlay. Push-to-talk vs always-listen toggle is
//! a user setting; always-listen defaults OFF (privacy).
//!
//! Architecture (kept stub-friendly so the crate builds without cpal/whisper
//! linked in dev): a `MicSource` produces 16 kHz mono PCM frames, a
//! `StreamingTranscriber` turns frames into partial / final `VoiceEvent`s, a
//! `VoiceSession` glues them and emits routing hints (`Route::Palette` or
//! `Route::Chat`) based on a wake-word or push-to-talk gate.
//!
//! Cap-gated `ai.voice`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tulipix_core::caps::{is_allowed, Cap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListenMode {
    /// Hotkey-held capture only — default.
    PushToTalk,
    /// Continuous mic capture, gated on a wake-word ("hey tulipix" by default).
    AlwaysListen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Route {
    /// Send transcript to the Cmd/Ctrl+K command palette.
    Palette,
    /// Send transcript to the Cmd/Ctrl+J chat overlay.
    Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    pub mode: ListenMode,
    pub wake_word: String,
    pub route: Route,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            mode: ListenMode::PushToTalk,
            wake_word: "hey tulipix".into(),
            route: Route::Palette,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    /// Streaming partial — UI shows greyed-out caret text.
    Partial(String),
    /// Final transcript with the route decided by the session.
    Final { text: String, route: Route },
    /// Mic opened / closed for HUD purposes.
    Started,
    Stopped,
}

pub trait MicSource: Send + Sync {
    /// Pull the next 16 kHz mono frame. None = end-of-stream.
    fn next_frame(&mut self) -> Option<Vec<i16>>;
}

pub trait StreamingTranscriber: Send + Sync {
    /// Push a PCM frame; may yield 0..n partials and at most one final.
    fn push(&mut self, frame: &[i16]) -> Vec<VoiceEvent>;
    /// Flush any pending partial into a final.
    fn flush(&mut self) -> Option<VoiceEvent>;
}

/// Streaming session — owns the mic + transcriber, routes events, enforces cap.
pub struct VoiceSession<M: MicSource, T: StreamingTranscriber> {
    mic: M,
    asr: T,
    cfg: VoiceConfig,
    active: Arc<Mutex<bool>>,
}

impl<M: MicSource, T: StreamingTranscriber> VoiceSession<M, T> {
    pub fn new(mic: M, asr: T, cfg: VoiceConfig) -> Self {
        Self { mic, asr, cfg, active: Arc::new(Mutex::new(false)) }
    }

    /// Open the mic. In push-to-talk mode the caller flips `active` true while
    /// the hotkey is held; in always-listen mode it stays true.
    pub fn start(&self) -> Result<()> {
        if !is_allowed(Cap::AiVoice) { anyhow::bail!("ai.voice denied"); }
        *self.active.lock().unwrap() = true;
        Ok(())
    }

    pub fn stop(&self) { *self.active.lock().unwrap() = false; }

    pub fn is_active(&self) -> bool { *self.active.lock().unwrap() }

    /// Drive one tick of capture → transcribe → route. Returns the events
    /// produced this tick (empty if mic is closed / not active).
    pub fn tick(&mut self) -> Vec<VoiceEvent> {
        if !*self.active.lock().unwrap() { return vec![]; }
        let Some(frame) = self.mic.next_frame() else { return vec![]; };
        let mut out = self.asr.push(&frame);
        for ev in out.iter_mut() {
            if let VoiceEvent::Final { text, route } = ev {
                *route = self.decide_route(text);
            }
        }
        out
    }

    fn decide_route(&self, text: &str) -> Route {
        match self.cfg.mode {
            ListenMode::PushToTalk => self.cfg.route,
            ListenMode::AlwaysListen => {
                let lower = text.to_lowercase();
                if lower.contains(&self.cfg.wake_word.to_lowercase()) {
                    self.cfg.route
                } else {
                    // Always-listen text outside wake-word window is dropped
                    // by the caller; we still tag the default route.
                    Route::Palette
                }
            }
        }
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
local_pro   = ["ai.voice"]
"#;

    struct FixedMic { frames: Vec<Vec<i16>>, i: usize }
    impl MicSource for FixedMic {
        fn next_frame(&mut self) -> Option<Vec<i16>> {
            let f = self.frames.get(self.i)?.clone();
            self.i += 1;
            Some(f)
        }
    }
    struct CannedAsr { partials: Vec<String>, final_text: Option<String>, i: usize }
    impl StreamingTranscriber for CannedAsr {
        fn push(&mut self, _frame: &[i16]) -> Vec<VoiceEvent> {
            if let Some(p) = self.partials.get(self.i).cloned() {
                self.i += 1;
                return vec![VoiceEvent::Partial(p)];
            }
            if let Some(f) = self.final_text.take() {
                return vec![VoiceEvent::Final { text: f, route: Route::Palette }];
            }
            vec![]
        }
        fn flush(&mut self) -> Option<VoiceEvent> { None }
    }

    #[test]
    fn cap_gate_blocks_start() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        let s = VoiceSession::new(
            FixedMic { frames: vec![vec![0; 320]], i: 0 },
            CannedAsr { partials: vec![], final_text: None, i: 0 },
            VoiceConfig::default(),
        );
        assert!(s.start().is_err());
    }

    #[test]
    fn always_listen_requires_wake_word() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let mut cfg = VoiceConfig::default();
        cfg.mode = ListenMode::AlwaysListen;
        cfg.route = Route::Chat;
        let s = VoiceSession::new(
            FixedMic { frames: vec![vec![0; 320]], i: 0 },
            CannedAsr { partials: vec![], final_text: None, i: 0 },
            cfg,
        );
        assert_eq!(s.decide_route("hey tulipix find sunset photos"), Route::Chat);
        assert_eq!(s.decide_route("idle chatter"), Route::Palette);
    }

    #[test]
    fn push_to_talk_streams_partials_then_final() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let mut s = VoiceSession::new(
            FixedMic { frames: vec![vec![0; 320]; 3], i: 0 },
            CannedAsr {
                partials: vec!["fin".into(), "find sun".into()],
                final_text: Some("find sunset photos".into()),
                i: 0,
            },
            VoiceConfig::default(),
        );
        s.start().unwrap();
        let t1 = s.tick(); assert!(matches!(t1[0], VoiceEvent::Partial(_)));
        let t2 = s.tick(); assert!(matches!(t2[0], VoiceEvent::Partial(_)));
        let t3 = s.tick();
        match &t3[0] {
            VoiceEvent::Final { text, route } => {
                assert_eq!(text, "find sunset photos");
                assert_eq!(*route, Route::Palette);
            }
            other => panic!("expected Final, got {other:?}"),
        }
    }
}
