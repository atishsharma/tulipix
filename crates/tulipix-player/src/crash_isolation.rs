//! mpv process isolation via IPC socket.
//!
//! Runs `mpv --idle --input-ipc-server=<socket>` in a child process so a
//! libmpv crash never tears the Slint window down. Commands go in as JSON
//! lines (`{"command":["loadfile","/x.mkv"]}`); events come back the same way.
//! The supervisor tracks the child's PID, restarts it with backoff on exit,
//! and exposes a clean state machine for the controller.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Child;

pub const MAX_RESTART_ATTEMPTS: u32 = 5;
pub const INITIAL_BACKOFF_MS:   u32 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorState {
    Stopped,
    Starting,
    Running,
    Restarting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorStatus {
    pub state: SupervisorState,
    pub pid: Option<u32>,
    pub restart_attempts: u32,
    pub last_error: Option<String>,
}

pub struct MpvSupervisor {
    socket: PathBuf,
    child: Option<Child>,
    state: SupervisorState,
    attempts: u32,
    last_error: Option<String>,
}

impl MpvSupervisor {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            child: None,
            state: SupervisorState::Stopped,
            attempts: 0,
            last_error: None,
        }
    }

    pub fn socket_path(&self) -> &Path { &self.socket }

    pub fn status(&self) -> SupervisorStatus {
        SupervisorStatus {
            state: self.state,
            pid: self.child.as_ref().map(|c| c.id()),
            restart_attempts: self.attempts,
            last_error: self.last_error.clone(),
        }
    }

    /// Build the argv mpv should be launched with. Pure — does not spawn.
    pub fn argv(&self) -> Vec<String> {
        let socket = self.socket.to_string_lossy().into_owned();
        vec![
            "--idle".into(),
            "--no-terminal".into(),
            "--force-window=no".into(),
            "--keep-open=yes".into(),
            format!("--input-ipc-server={}", socket),
        ]
    }

    /// Called when the child exits. Returns the milliseconds the supervisor
    /// should wait before re-spawning, or `None` once `MAX_RESTART_ATTEMPTS`
    /// has been exhausted (caller flips to `Failed`).
    pub fn on_child_exit(&mut self, error: Option<String>) -> Option<u32> {
        self.last_error = error;
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts > MAX_RESTART_ATTEMPTS {
            self.state = SupervisorState::Failed;
            return None;
        }
        self.state = SupervisorState::Restarting;
        let backoff = INITIAL_BACKOFF_MS.saturating_mul(1u32 << (self.attempts - 1).min(6));
        Some(backoff.min(10_000))
    }

    /// Mark the supervisor as running with the given child handle. The Slint
    /// integration calls this immediately after `Command::spawn`.
    pub fn mark_running(&mut self, child: Child) {
        self.child = Some(child);
        self.state = SupervisorState::Running;
        self.last_error = None;
    }

    pub fn reset_attempts(&mut self) {
        self.attempts = 0;
        self.last_error = None;
        self.state = SupervisorState::Stopped;
    }

    /// Format an mpv command into the JSON-line wire format.
    pub fn encode_command(cmd: &[&str]) -> String {
        let mut s = String::from("{\"command\":[");
        for (i, part) in cmd.iter().enumerate() {
            if i > 0 { s.push(','); }
            s.push('"');
            s.push_str(&part.replace('\\', "\\\\").replace('"', "\\\""));
            s.push('"');
        }
        s.push_str("]}\n");
        s
    }
}

pub fn parse_event(line: &str) -> Result<Option<String>> {
    let v: serde_json::Value = serde_json::from_str(line.trim())?;
    Ok(v.get("event").and_then(|e| e.as_str()).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_contains_socket() {
        let s = MpvSupervisor::new("/tmp/mpv.sock");
        let argv = s.argv();
        assert!(argv.iter().any(|a| a.contains("/tmp/mpv.sock")));
        assert!(argv.iter().any(|a| a == "--idle"));
    }

    #[test]
    fn exit_backoff_grows_exponentially_then_caps() {
        let mut s = MpvSupervisor::new("/tmp/x");
        let mut last = 0u32;
        for _ in 0..MAX_RESTART_ATTEMPTS {
            let d = s.on_child_exit(Some("boom".into())).unwrap();
            assert!(d >= last);
            last = d;
        }
        let exhausted = s.on_child_exit(Some("boom".into()));
        assert!(exhausted.is_none());
        assert_eq!(s.state, SupervisorState::Failed);
    }

    #[test]
    fn encode_command_quotes_correctly() {
        let s = MpvSupervisor::encode_command(&["loadfile", "/x.mkv"]);
        assert_eq!(s, "{\"command\":[\"loadfile\",\"/x.mkv\"]}\n");
    }

    #[test]
    fn encode_command_escapes_quotes_and_backslashes() {
        let s = MpvSupervisor::encode_command(&["show-text", "a\"b\\c"]);
        assert!(s.contains("\\\""));
        assert!(s.contains("\\\\"));
    }

    #[test]
    fn parse_event_finds_field() {
        let e = parse_event("{\"event\":\"file-loaded\"}").unwrap();
        assert_eq!(e.as_deref(), Some("file-loaded"));
        let e = parse_event("{\"data\":42}").unwrap();
        assert!(e.is_none());
    }

    #[test]
    fn reset_attempts_returns_to_stopped() {
        let mut s = MpvSupervisor::new("/tmp/x");
        let _ = s.on_child_exit(Some("e".into()));
        s.reset_attempts();
        assert_eq!(s.state, SupervisorState::Stopped);
        assert_eq!(s.attempts, 0);
    }
}
