//! Exclusive audio output + per-track gain.
//!
//! Maps a desired mode (shared / exclusive) onto the right mpv `--ao=…`
//! backend per OS, and keeps a per-track gain table so a quiet music score
//! and a loud action track in the same library can both ride at a consistent
//! perceived level without rewriting files.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode { Shared, Exclusive }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioBackend {
    Wasapi,
    WasapiExclusive,
    CoreAudio,
    CoreAudioExclusive,
    Pulse,
    Pipewire,
    AlsaShared,
    AlsaExclusive,
    Auto,
}

impl AudioBackend {
    pub fn mpv_ao(self) -> &'static str {
        match self {
            Self::Wasapi | Self::WasapiExclusive => "wasapi",
            Self::CoreAudio | Self::CoreAudioExclusive => "coreaudio",
            Self::Pulse => "pulse",
            Self::Pipewire => "pipewire",
            Self::AlsaShared | Self::AlsaExclusive => "alsa",
            Self::Auto => "auto",
        }
    }

    pub fn is_exclusive(self) -> bool {
        matches!(self, Self::WasapiExclusive | Self::CoreAudioExclusive | Self::AlsaExclusive)
    }
}

pub fn pick_backend(mode: AudioMode) -> AudioBackend {
    if cfg!(target_os = "windows") {
        if matches!(mode, AudioMode::Exclusive) { AudioBackend::WasapiExclusive } else { AudioBackend::Wasapi }
    } else if cfg!(target_os = "macos") {
        if matches!(mode, AudioMode::Exclusive) { AudioBackend::CoreAudioExclusive } else { AudioBackend::CoreAudio }
    } else {
        match mode {
            AudioMode::Exclusive => AudioBackend::AlsaExclusive,
            AudioMode::Shared => AudioBackend::Pipewire,
        }
    }
}

/// Per-OS mpv options string list for the picked backend.
pub fn mpv_opts(backend: AudioBackend) -> Vec<(String, String)> {
    let mut opts = vec![("ao".into(), backend.mpv_ao().into())];
    if backend.is_exclusive() {
        match backend {
            AudioBackend::WasapiExclusive => opts.push(("audio-exclusive".into(), "yes".into())),
            AudioBackend::CoreAudioExclusive => opts.push(("audio-exclusive".into(), "yes".into())),
            AudioBackend::AlsaExclusive => opts.push(("audio-device".into(), "alsa/hw:0,0".into())),
            _ => {}
        }
    }
    opts
}

/// Per-track gain stored in linear amplitude (1.0 = unity). mpv applies this
/// via `--af=volume=<dB>` so we convert before pushing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GainTable {
    pub gains: HashMap<i64, f32>,
}

impl GainTable {
    pub fn set(&mut self, item_id: i64, linear: f32) { self.gains.insert(item_id, linear.clamp(0.1, 4.0)); }
    pub fn get(&self, item_id: i64) -> f32 { self.gains.get(&item_id).copied().unwrap_or(1.0) }
    pub fn db_for(&self, item_id: i64) -> f32 { 20.0 * self.get(item_id).log10() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_backend_picks_exclusive() {
        let b = pick_backend(AudioMode::Exclusive);
        assert!(b.is_exclusive());
    }

    #[test]
    fn pick_backend_shared_is_not_exclusive() {
        let b = pick_backend(AudioMode::Shared);
        assert!(!b.is_exclusive());
    }

    #[test]
    fn mpv_opts_emits_ao() {
        let opts = mpv_opts(pick_backend(AudioMode::Shared));
        assert!(opts.iter().any(|(k, _)| k == "ao"));
    }

    #[test]
    fn gain_table_clamps_and_returns_db() {
        let mut g = GainTable::default();
        g.set(1, 100.0);
        assert!((g.get(1) - 4.0).abs() < 1e-6);
        g.set(2, 0.0);
        assert!((g.get(2) - 0.1).abs() < 1e-6);
        assert_eq!(g.get(999), 1.0);
        let db = g.db_for(1);
        assert!((db - 20.0 * 4f32.log10()).abs() < 1e-4);
    }
}
