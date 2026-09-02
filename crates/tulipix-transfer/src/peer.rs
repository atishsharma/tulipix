//! Talking to another tulipix: the pairing handshake, and the client half of
//! the routes the phone already uses.
//!
//! This is the only module in the crate that makes outbound requests. Every
//! other module either serves them or plans them.

/// Six digits both machines can derive independently from the two leaf
/// fingerprints, for a person to compare on two screens.
///
/// Sorted before hashing so the initiator and the receiver land on the same
/// number — the pairing is symmetric even though the request is not. Six digits
/// is a one-in-a-million forgery chance per attempt, against an attacker who
/// gets exactly one attempt before a human says the digits do not match.
pub fn pairing_code(a: &str, b: &str) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let digest = ring::digest::digest(&ring::digest::SHA256, format!("{lo}\n{hi}").as_bytes());
    let bytes = digest.as_ref();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) % 1_000_000;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP_A: &str = "3f:a1:9c:44:e2:7b:08:d5:1a:6e";
    const FP_B: &str = "b2:04:71:ee:5c:39:aa:10:7f:c3";

    #[test]
    fn both_machines_derive_the_same_digits() {
        assert_eq!(
            pairing_code(FP_A, FP_B),
            pairing_code(FP_B, FP_A),
            "the code cannot depend on who started the pairing"
        );
    }

    #[test]
    fn the_code_is_six_digits() {
        let code = pairing_code(FP_A, FP_B);
        assert_eq!(code.len(), 6, "six boxes on screen, always six digits");
        assert!(code.chars().all(|c| c.is_ascii_digit()), "digits only: {code}");
    }

    #[test]
    fn one_changed_character_changes_the_code() {
        let tampered = "3f:a1:9c:44:e2:7b:08:d5:1a:6f";
        assert_ne!(
            pairing_code(FP_A, FP_B),
            pairing_code(tampered, FP_B),
            "a man in the middle presents a different certificate and must show different digits"
        );
    }

    #[test]
    fn a_short_digest_still_pads_to_six() {
        // Whatever the hash lands on, the display width never varies.
        for i in 0..64 {
            let a = format!("{i:02x}");
            assert_eq!(pairing_code(&a, FP_B).len(), 6);
        }
    }
}
