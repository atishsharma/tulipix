//! `np.p4.music.motion` — spring-physics transitions.
//!
//! Shared critically-dampable spring used for album crossfade on track change,
//! queue reorder, and panel ease. Respects reduce-motion: when set, the spring
//! snaps to target instantly so the same call sites work either way.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    /// Angular frequency (stiffness). Higher = snappier.
    pub stiffness: f64,
    /// Damping ratio: 1.0 = critical (no overshoot), <1 bouncy.
    pub damping_ratio: f64,
    pub reduce_motion: bool,
}

impl Spring {
    pub fn new(value: f64) -> Self {
        Self { value, velocity: 0.0, target: value, stiffness: 12.0, damping_ratio: 1.0, reduce_motion: false }
    }

    pub fn set_target(&mut self, target: f64) { self.target = target; }

    /// Step the spring by `dt` seconds (semi-implicit Euler). Returns the new
    /// value. Snaps instantly under reduce-motion.
    pub fn step(&mut self, dt: f64) -> f64 {
        if self.reduce_motion {
            self.value = self.target; self.velocity = 0.0;
            return self.value;
        }
        let k = self.stiffness * self.stiffness;          // spring constant
        let c = 2.0 * self.damping_ratio * self.stiffness; // damping coefficient
        let force = -k * (self.value - self.target) - c * self.velocity;
        self.velocity += force * dt;
        self.value += self.velocity * dt;
        self.value
    }

    /// At rest near target with negligible velocity.
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 1e-3 && self.velocity.abs() < 1e-3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_converges_to_target() {
        let mut s = Spring::new(0.0);
        s.set_target(1.0);
        for _ in 0..2000 { s.step(0.016); if s.is_settled() { break; } }
        assert!(s.is_settled());
        assert!((s.value - 1.0).abs() < 1e-2);
    }

    #[test]
    fn reduce_motion_snaps() {
        let mut s = Spring::new(0.0);
        s.reduce_motion = true;
        s.set_target(5.0);
        assert_eq!(s.step(0.016), 5.0);
        assert!(s.is_settled());
    }
}
