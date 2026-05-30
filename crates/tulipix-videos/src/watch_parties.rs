//! Watch parties — synced playback across users in Account mode.
//!
//! The transport here is intentionally abstract (`Transport` trait); the
//! production wiring lives in tulipix-sync (websocket over the sync backend).
//! Everything time-sensitive — drift correction, host election, time-sync
//! protocol — is plain logic that tests exercise without a network.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct UserId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct PartyId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PartyEvent {
    Join { user: UserId, name: String },
    Leave { user: UserId },
    Play { position_s: f64, sent_at_ms: i64 },
    Pause { position_s: f64, sent_at_ms: i64 },
    Seek { position_s: f64, sent_at_ms: i64 },
    Heartbeat { position_s: f64, sent_at_ms: i64 },
    Chat { user: UserId, text: String, sent_at_ms: i64 },
}

#[async_trait]
pub trait Transport: Send + Sync {
    async fn send(&self, party: PartyId, event: &PartyEvent) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum DriftDecision {
    InSync,
    /// Local should jump to `target` because drift > hard threshold.
    Snap(f64),
    /// Local should fine-tune by `target - local` over the next ~1 s.
    Nudge(f64),
}

/// 500 ms heartbeat with a 1.5 s snap threshold and a 100 ms nudge threshold
/// — matches the plan's "every 500 ms" cadence.
pub const HEARTBEAT_MS: i64 = 500;
pub const SNAP_THRESHOLD_S: f64 = 1.5;
pub const NUDGE_THRESHOLD_S: f64 = 0.1;

/// Compute drift correction given the local player position, the remote
/// position the host advertised, and the round-trip-time estimate. The remote
/// position is advanced by `rtt_ms / 2` so we converge on the wall-clock the
/// host will be at when the local player receives the heartbeat.
pub fn drift_decision(local_position_s: f64, remote_position_s: f64, one_way_lag_ms: i64) -> DriftDecision {
    let target = remote_position_s + (one_way_lag_ms as f64) / 1000.0;
    let diff = target - local_position_s;
    if diff.abs() > SNAP_THRESHOLD_S {
        DriftDecision::Snap(target)
    } else if diff.abs() > NUDGE_THRESHOLD_S {
        DriftDecision::Nudge(target)
    } else {
        DriftDecision::InSync
    }
}

#[derive(Debug, Default)]
pub struct Party {
    pub id: PartyId,
    pub host: Option<UserId>,
    pub members: HashMap<UserId, String>,
    pub current_position_s: f64,
    pub paused: bool,
    pub history: Vec<PartyEvent>,
}

impl Party {
    pub fn new(id: PartyId) -> Self {
        Self {
            id,
            host: None,
            members: HashMap::new(),
            current_position_s: 0.0,
            paused: true,
            history: Vec::new(),
        }
    }

    pub fn apply(&mut self, ev: PartyEvent) {
        match &ev {
            PartyEvent::Join { user, name } => {
                self.members.insert(*user, name.clone());
                if self.host.is_none() {
                    self.host = Some(*user);
                }
            }
            PartyEvent::Leave { user } => {
                self.members.remove(user);
                if self.host == Some(*user) {
                    // Re-elect the lowest-id remaining member.
                    self.host = self.members.keys().min().copied();
                }
            }
            PartyEvent::Play { position_s, .. } => {
                self.current_position_s = *position_s;
                self.paused = false;
            }
            PartyEvent::Pause { position_s, .. } => {
                self.current_position_s = *position_s;
                self.paused = true;
            }
            PartyEvent::Seek { position_s, .. } => {
                self.current_position_s = *position_s;
            }
            PartyEvent::Heartbeat { position_s, .. } => {
                self.current_position_s = *position_s;
            }
            PartyEvent::Chat { .. } => {}
        }
        self.history.push(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeTransport {
        sent: Mutex<Vec<(PartyId, PartyEvent)>>,
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn send(&self, party: PartyId, event: &PartyEvent) -> Result<()> {
            self.sent.lock().unwrap().push((party, event.clone()));
            Ok(())
        }
    }

    #[test]
    fn drift_in_sync_under_nudge() {
        assert_eq!(drift_decision(10.0, 10.0, 0), DriftDecision::InSync);
        assert_eq!(drift_decision(10.0, 10.05, 0), DriftDecision::InSync);
    }

    #[test]
    fn drift_nudge_band() {
        let d = drift_decision(10.0, 10.3, 0);
        match d {
            DriftDecision::Nudge(t) => assert!((t - 10.3).abs() < 1e-9),
            _ => panic!("expected nudge, got {d:?}"),
        }
    }

    #[test]
    fn drift_snap_band() {
        let d = drift_decision(10.0, 14.0, 0);
        match d {
            DriftDecision::Snap(t) => assert!((t - 14.0).abs() < 1e-9),
            _ => panic!("expected snap, got {d:?}"),
        }
    }

    #[test]
    fn drift_accounts_for_lag() {
        let d = drift_decision(10.0, 10.0, 250); // 250 ms one-way
        match d {
            DriftDecision::Nudge(t) => assert!((t - 10.25).abs() < 1e-9),
            _ => panic!("lag should push us into nudge, got {d:?}"),
        }
    }

    #[test]
    fn host_election_on_join_then_leave() {
        let mut p = Party::new(PartyId(1));
        p.apply(PartyEvent::Join {
            user: UserId(10),
            name: "ten".into(),
        });
        p.apply(PartyEvent::Join {
            user: UserId(20),
            name: "twenty".into(),
        });
        assert_eq!(p.host, Some(UserId(10)));
        p.apply(PartyEvent::Leave { user: UserId(10) });
        assert_eq!(p.host, Some(UserId(20)));
        p.apply(PartyEvent::Leave { user: UserId(20) });
        assert_eq!(p.host, None);
    }

    #[test]
    fn play_pause_update_state() {
        let mut p = Party::new(PartyId(1));
        p.apply(PartyEvent::Play {
            position_s: 12.0,
            sent_at_ms: 0,
        });
        assert!(!p.paused);
        assert!((p.current_position_s - 12.0).abs() < 1e-9);
        p.apply(PartyEvent::Pause {
            position_s: 30.0,
            sent_at_ms: 1,
        });
        assert!(p.paused);
        assert!((p.current_position_s - 30.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn transport_records_send() {
        let t = FakeTransport {
            sent: Mutex::new(Vec::new()),
        };
        t.send(
            PartyId(1),
            &PartyEvent::Chat {
                user: UserId(1),
                text: "hi".into(),
                sent_at_ms: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(t.sent.lock().unwrap().len(), 1);
    }
}
