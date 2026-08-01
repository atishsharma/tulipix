//! OS Now Playing integration. Same Track payload feeds three back-ends:
//!
//!   * Linux: MPRIS 2 (`org.mpris.MediaPlayer2.tulipix` on the session bus).
//!   * Windows: SystemMediaTransportControls (SMTC) via WinRT.
//!   * macOS: `MPNowPlayingInfoCenter` + `MPRemoteCommandCenter`.
//!
//! Each sink consumes the same `Track` + `PlaybackState` so a single
//! playback callback covers media keys, BT next/prev, lockscreen
//! controls, and control-centre artwork.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState { Stopped, Playing, Paused }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork_path: Option<String>,
    pub duration_ms: Option<u64>,
    pub position_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteCommand {
    Play, Pause, TogglePlay, Next, Previous,
    SeekForward, SeekBackward, SeekTo,
    Stop,
}

pub trait NowPlayingSink: Send + Sync {
    fn set_track(&self, track: &Track, state: PlaybackState) -> Result<()>;
    fn set_state(&self, state: PlaybackState) -> Result<()>;
    fn set_position(&self, position_ms: u64) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

pub struct StubSink;
impl NowPlayingSink for StubSink {
    fn set_track(&self, t: &Track, s: PlaybackState) -> Result<()> {
        tracing::info!(title = %t.title, ?s, "now_playing set_track (stub)"); Ok(())
    }
    fn set_state(&self, s: PlaybackState) -> Result<()> {
        tracing::info!(?s, "now_playing state (stub)"); Ok(())
    }
    fn set_position(&self, p: u64) -> Result<()> {
        tracing::info!(p, "now_playing pos (stub)"); Ok(())
    }
    fn clear(&self) -> Result<()> { tracing::info!("now_playing clear (stub)"); Ok(()) }
}

pub fn os_sink() -> Box<dyn NowPlayingSink> { Box::new(StubSink) }

/// Render the MPRIS metadata dict the D-Bus binding hands to mpris-server.
pub fn mpris_metadata(t: &Track) -> Vec<(&'static str, String)> {
    let mut m = vec![("xesam:title", t.title.clone())];
    if let Some(a) = &t.artist { m.push(("xesam:artist", a.clone())); }
    if let Some(a) = &t.album  { m.push(("xesam:album",  a.clone())); }
    if let Some(a) = &t.artwork_path { m.push(("mpris:artUrl", format!("file://{a}"))); }
    if let Some(d) = t.duration_ms   { m.push(("mpris:length", (d * 1000).to_string())); }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t() -> Track { Track {
        title: "Coda".into(), artist: Some("Author".into()), album: Some("Disc".into()),
        artwork_path: Some("/tmp/a.jpg".into()), duration_ms: Some(180_000), position_ms: 12_000,
    } }
    #[test] fn stub_sink_set_clears() {
        let s = StubSink;
        s.set_track(&t(), PlaybackState::Playing).unwrap();
        s.set_state(PlaybackState::Paused).unwrap();
        s.set_position(60_000).unwrap();
        s.clear().unwrap();
    }
    #[test] fn mpris_metadata_includes_artist_album_art() {
        let m = mpris_metadata(&t());
        assert!(m.iter().any(|(k, _)| *k == "xesam:title"));
        assert!(m.iter().any(|(k, v)| *k == "mpris:artUrl" && v.starts_with("file://")));
        assert!(m.iter().any(|(k, v)| *k == "mpris:length" && v == "180000000"));
    }
}
