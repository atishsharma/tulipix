//! Casting backends — Chromecast, AirPlay 2, DLNA — plus shared concerns
//! (subtitle injection, remote volume, multi-room NTP sync).
//!
//! Every backend implements [`Receiver`] so the UI talks to one trait and the
//! Cast button on the playback overlay enumerates all backends at once.

pub mod airplay;
pub mod chromecast;
pub mod dlna;
pub mod multiroom;
pub mod subs;
pub mod volume;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Chromecast,
    AirPlay,
    Dlna,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReceiverInfo {
    pub id: String,
    pub friendly_name: String,
    pub backend: BackendKind,
    pub host: String,
    pub port: u16,
    pub model: Option<String>,
    pub supports_video: bool,
    pub supports_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybackRequest {
    pub stream_url: String,
    pub mime: String,
    pub title: String,
    pub start_position_s: f64,
    pub subtitle_url: Option<String>,
}

#[async_trait]
pub trait Receiver: Send + Sync {
    fn info(&self) -> &ReceiverInfo;
    async fn play(&self, req: &PlaybackRequest) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn seek(&self, position_s: f64) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn set_volume(&self, value: f32) -> Result<()>;
    async fn set_muted(&self, muted: bool) -> Result<()>;
}
