//! `np.p4.cross.p2p` — P2P "Direct Drop".
//!
//! Send big files/albums directly to another peer via WebRTC / Magic Wormhole,
//! bypassing the sync server. Magic Wormhole pairs peers with a short code
//! `<nameplate>-<word>-<word>` (e.g. `7-crossover-clockwork`). This owns code
//! generation/validation and the transfer session state machine.

use serde::{Deserialize, Serialize};

/// A short subset of the PGP word list (even/odd alternation in real wormhole;
/// here a flat list is enough for code formatting + validation).
pub const WORDLIST: &[&str] = &[
    "aardvark","absurd","accrue","acme","adrift","adult","afflict","ahead",
    "crossover","clockwork","stairway","sentence","tactics","tunnel","unicorn","vapor",
];

/// Format a wormhole code from a numeric nameplate + word indices.
pub fn make_code(nameplate: u32, w1: usize, w2: usize) -> String {
    let a = WORDLIST[w1 % WORDLIST.len()];
    let b = WORDLIST[w2 % WORDLIST.len()];
    format!("{nameplate}-{a}-{b}")
}

/// Validate a code's shape: `<digits>-<word>-<word>` with known words.
pub fn validate_code(code: &str) -> bool {
    let parts: Vec<&str> = code.split('-').collect();
    if parts.len() != 3 { return false; }
    let np_ok = !parts[0].is_empty() && parts[0].chars().all(|c| c.is_ascii_digit());
    let words_ok = parts[1..].iter().all(|w| WORDLIST.contains(w));
    np_ok && words_ok
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase { Idle, WaitingForPeer, Connected, Transferring, Done, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transfer {
    pub phase: Phase,
    pub bytes_total: u64,
    pub bytes_sent: u64,
}

impl Transfer {
    pub fn new(bytes_total: u64) -> Self {
        Self { phase: Phase::Idle, bytes_total, bytes_sent: 0 }
    }

    pub fn peer_connected(&mut self) {
        if matches!(self.phase, Phase::Idle | Phase::WaitingForPeer) { self.phase = Phase::Connected; }
    }

    /// Record progress; flips to Transferring then Done at completion.
    pub fn advance(&mut self, sent: u64) {
        if self.phase == Phase::Connected { self.phase = Phase::Transferring; }
        if self.phase == Phase::Transferring {
            self.bytes_sent = (self.bytes_sent + sent).min(self.bytes_total);
            if self.bytes_sent >= self.bytes_total && self.bytes_total > 0 { self.phase = Phase::Done; }
        }
    }

    pub fn fraction(&self) -> f64 {
        if self.bytes_total == 0 { 0.0 } else { self.bytes_sent as f64 / self.bytes_total as f64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_make_and_validate() {
        let c = make_code(7, 8, 9); // crossover, clockwork
        assert_eq!(c, "7-crossover-clockwork");
        assert!(validate_code(&c));
        assert!(!validate_code("7-crossover"));        // too few parts
        assert!(!validate_code("x-crossover-clockwork")); // bad nameplate
        assert!(!validate_code("7-notaword-clockwork")); // unknown word
    }

    #[test]
    fn transfer_runs_to_done() {
        let mut t = Transfer::new(1000);
        t.peer_connected();
        assert_eq!(t.phase, Phase::Connected);
        t.advance(400);
        assert_eq!(t.phase, Phase::Transferring);
        assert!((t.fraction() - 0.4).abs() < 1e-9);
        t.advance(600);
        assert_eq!(t.phase, Phase::Done);
        assert_eq!(t.bytes_sent, 1000);
    }
}
