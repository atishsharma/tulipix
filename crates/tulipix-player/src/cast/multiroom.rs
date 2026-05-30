//! Multi-room audio — same audio stream, multiple receivers, NTP-aligned.
//!
//! The trick is that every receiver reports its own clock via the casting
//! channel (Chromecast's MEDIA_STATUS, AirPlay's PTP timestamps). We pick a
//! reference receiver, advance every other receiver's playback to match the
//! reference, and clamp the corrections so drift correction stays
//! inaudible (~2 ms snap floor).
//!
//! The protocol-side time-sync (RTSP PTP for AirPlay, Cast's `mediaTime`
//! for Chromecast) is plumbed in by the per-backend module; this file owns
//! the room model + sync arithmetic.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct RoomId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct ReceiverId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverClock {
    pub receiver: ReceiverId,
    /// Wall-clock at the moment we received the heartbeat (ms since UNIX).
    pub heartbeat_at_ms: i64,
    /// Receiver-reported playback position (ms).
    pub reported_position_ms: i64,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Room {
    pub id: RoomId,
    pub reference: Option<ReceiverId>,
    pub members: BTreeMap<ReceiverId, ReceiverClock>,
}

impl Room {
    pub fn new(id: RoomId) -> Self {
        Self {
            id,
            reference: None,
            members: BTreeMap::new(),
        }
    }

    pub fn join(&mut self, clock: ReceiverClock) {
        let id = clock.receiver;
        if self.reference.is_none() {
            self.reference = Some(id);
        }
        self.members.insert(id, clock);
    }

    pub fn leave(&mut self, id: ReceiverId) {
        self.members.remove(&id);
        if self.reference == Some(id) {
            // Promote the lowest-id remaining member.
            self.reference = self.members.keys().next().copied();
        }
    }

    pub fn update(&mut self, clock: ReceiverClock) {
        self.members.insert(clock.receiver, clock);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncAction {
    InSync,
    Adjust { target_ms: i64 },
    Snap { target_ms: i64 },
}

/// Per-receiver correction. `now_ms` is the host wall-clock at evaluation
/// time. The reference's predicted position is `ref.reported_position +
/// (now - ref.heartbeat_at)` — assumes 1.0 playback rate.
pub fn correction_for(
    room: &Room,
    receiver: ReceiverId,
    now_ms: i64,
    snap_threshold_ms: i64,
    adjust_threshold_ms: i64,
) -> SyncAction {
    let Some(ref_id) = room.reference else {
        return SyncAction::InSync;
    };
    if receiver == ref_id {
        return SyncAction::InSync;
    }
    let (Some(ref_clock), Some(me)) = (room.members.get(&ref_id), room.members.get(&receiver)) else {
        return SyncAction::InSync;
    };
    let ref_position = ref_clock.reported_position_ms + (now_ms - ref_clock.heartbeat_at_ms);
    let my_position = me.reported_position_ms + (now_ms - me.heartbeat_at_ms);
    let diff = ref_position - my_position;
    if diff.abs() <= adjust_threshold_ms {
        SyncAction::InSync
    } else if diff.abs() <= snap_threshold_ms {
        SyncAction::Adjust { target_ms: ref_position }
    } else {
        SyncAction::Snap { target_ms: ref_position }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock(id: u32, heartbeat: i64, position: i64) -> ReceiverClock {
        ReceiverClock {
            receiver: ReceiverId(id),
            heartbeat_at_ms: heartbeat,
            reported_position_ms: position,
        }
    }

    #[test]
    fn first_joiner_becomes_reference() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(7, 0, 0));
        assert_eq!(r.reference, Some(ReceiverId(7)));
    }

    #[test]
    fn reference_promotion_on_leave() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(7, 0, 0));
        r.join(clock(10, 0, 0));
        r.leave(ReceiverId(7));
        assert_eq!(r.reference, Some(ReceiverId(10)));
        r.leave(ReceiverId(10));
        assert_eq!(r.reference, None);
    }

    #[test]
    fn reference_against_self_is_in_sync() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(7, 0, 1_000));
        assert_eq!(
            correction_for(&r, ReceiverId(7), 1_000, 200, 20),
            SyncAction::InSync
        );
    }

    #[test]
    fn drift_under_adjust_threshold_is_in_sync() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(1, 1_000, 5_000));
        r.join(clock(2, 1_000, 5_010)); // 10 ms ahead
        let act = correction_for(&r, ReceiverId(2), 1_000, 200, 20);
        assert_eq!(act, SyncAction::InSync);
    }

    #[test]
    fn drift_in_adjust_band_nudges() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(1, 1_000, 5_000));
        r.join(clock(2, 1_000, 5_100));
        let act = correction_for(&r, ReceiverId(2), 1_000, 200, 20);
        assert_eq!(act, SyncAction::Adjust { target_ms: 5_000 });
    }

    #[test]
    fn drift_over_snap_threshold_snaps() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(1, 1_000, 10_000));
        r.join(clock(2, 1_000, 6_000));
        let act = correction_for(&r, ReceiverId(2), 1_000, 200, 20);
        assert_eq!(act, SyncAction::Snap { target_ms: 10_000 });
    }

    #[test]
    fn extrapolation_uses_elapsed_wall_clock() {
        let mut r = Room::new(RoomId(1));
        r.join(clock(1, 1_000, 0));
        r.join(clock(2, 1_000, 0));
        // 30 ms later, both have advanced 30 ms in lock-step.
        let act = correction_for(&r, ReceiverId(2), 1_030, 200, 20);
        assert_eq!(act, SyncAction::InSync);
    }
}
