//! Per-OS keep-display-awake API.
//!
//! Each backend acquires a token (a system handle) when playback starts and
//! releases it on pause/stop/quit. The Rust layer just tracks the desired
//! state + which token is currently held; the actual FFI lives in the
//! platform crate behind these trait methods.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhibitBackend { IoKit, SetThreadExecutionState, Logind, GnomeSession, Mock }

impl InhibitBackend {
    pub fn for_target() -> Self {
        if cfg!(target_os = "macos")     { Self::IoKit }
        else if cfg!(target_os = "windows") { Self::SetThreadExecutionState }
        else if cfg!(target_os = "linux") { Self::Logind }
        else { Self::Mock }
    }
}

pub trait DisplayAwake {
    fn acquire(&mut self, reason: &str) -> Result<u32, String>;
    fn release(&mut self, token: u32) -> Result<(), String>;
    fn is_held(&self, token: u32) -> bool;
}

#[derive(Debug, Default)]
pub struct MockDisplayAwake {
    next_token: u32,
    held: std::collections::HashSet<u32>,
    pub last_reason: Option<String>,
}

impl DisplayAwake for MockDisplayAwake {
    fn acquire(&mut self, reason: &str) -> Result<u32, String> {
        self.next_token = self.next_token.wrapping_add(1);
        self.held.insert(self.next_token);
        self.last_reason = Some(reason.to_string());
        Ok(self.next_token)
    }
    fn release(&mut self, token: u32) -> Result<(), String> {
        if !self.held.remove(&token) {
            return Err(format!("token {token} not held"));
        }
        Ok(())
    }
    fn is_held(&self, token: u32) -> bool { self.held.contains(&token) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InhibitState { Idle, Held }

pub struct SleepInhibitor<A: DisplayAwake> {
    backend: A,
    token: Option<u32>,
}

impl<A: DisplayAwake> SleepInhibitor<A> {
    pub fn new(backend: A) -> Self { Self { backend, token: None } }

    pub fn state(&self) -> InhibitState {
        if self.token.is_some() { InhibitState::Held } else { InhibitState::Idle }
    }

    /// Acquire if not already held. Idempotent.
    pub fn acquire(&mut self, reason: &str) -> Result<(), String> {
        if self.token.is_some() { return Ok(()); }
        let t = self.backend.acquire(reason)?;
        self.token = Some(t);
        Ok(())
    }

    pub fn release(&mut self) -> Result<(), String> {
        if let Some(t) = self.token.take() {
            self.backend.release(t)?;
        }
        Ok(())
    }

    /// Bind state to play-state. Acquires on `playing=true`, releases otherwise.
    pub fn sync_with_play(&mut self, playing: bool, reason: &str) -> Result<(), String> {
        if playing { self.acquire(reason) } else { self.release() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_is_idempotent() {
        let mut s = SleepInhibitor::new(MockDisplayAwake::default());
        s.acquire("playing").unwrap();
        s.acquire("playing").unwrap(); // second call no-op
        assert_eq!(s.state(), InhibitState::Held);
        s.release().unwrap();
        s.release().unwrap();
        assert_eq!(s.state(), InhibitState::Idle);
    }

    #[test]
    fn sync_with_play_flips_state() {
        let mut s = SleepInhibitor::new(MockDisplayAwake::default());
        s.sync_with_play(true, "movie").unwrap();
        assert_eq!(s.state(), InhibitState::Held);
        s.sync_with_play(false, "paused").unwrap();
        assert_eq!(s.state(), InhibitState::Idle);
    }

    #[test]
    fn backend_dispatch_known_per_os() {
        let b = InhibitBackend::for_target();
        // backend variant is non-null on every supported target.
        assert!(matches!(b, InhibitBackend::IoKit | InhibitBackend::SetThreadExecutionState | InhibitBackend::Logind | InhibitBackend::Mock));
    }
}
