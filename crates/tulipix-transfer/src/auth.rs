//! Three mechanisms, each doing one job: a PIN the user reads off the screen, a
//! single-use pairing key carried by the QR, and a 48-hour device token so a
//! phone that has already paired does not retype anything.

use std::collections::HashMap;

/// Wrong PINs from one address before that address is done for the session.
pub const MAX_ATTEMPTS: u32 = 5;
/// How long the QR's pairing key stays good. Long enough to walk to the phone.
const PAIRING_TTL: u64 = 60;
/// "48 hours, then re-token."
pub const TOKEN_TTL: u64 = 48 * 3600;

pub enum PinResult {
    Ok(Token),
    Wrong,
    LockedOut,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub value: String,
    pub label: String,
    pub issued: u64,
    pub expires: u64,
    pub last_seen: u64,
}

impl Token {
    pub fn issue(label: &str, now: u64) -> Self {
        Self {
            value: random_hex(32),
            label: label.to_string(),
            issued: now,
            expires: now + TOKEN_TTL,
            last_seen: now,
        }
    }

    pub fn valid_at(&self, now: u64) -> bool {
        now < self.expires
    }
}

pub struct Auth {
    pin: String,
    attempts: HashMap<String, u32>,
    pairing: Option<(String, u64)>, // (key, issued)
    tokens: HashMap<String, Token>,
}

impl Auth {
    pub fn new(_now: u64) -> Self {
        Self {
            pin: random_pin(),
            attempts: HashMap::new(),
            pairing: None,
            tokens: HashMap::new(),
        }
    }

    pub fn pin(&self) -> &str {
        &self.pin
    }

    /// Wrong-PIN count for one address, for the Send pane's attempt line.
    pub fn attempts_from(&self, ip: &str) -> u32 {
        self.attempts.get(ip).copied().unwrap_or(0)
    }

    /// Every address that has guessed wrong, worst first.
    ///
    /// Surfaced in the Send pane rather than kept as a counter nobody reads:
    /// somebody grinding the PIN on your network is exactly the thing worth
    /// noticing while it is happening, and the lockout is silent from this end.
    pub fn wrong_attempts(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = self
            .attempts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(ip, n)| (ip.clone(), *n))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    pub fn try_pin(&mut self, ip: &str, given: &str, now: u64) -> PinResult {
        let tries = self.attempts.entry(ip.to_string()).or_insert(0);
        if *tries >= MAX_ATTEMPTS {
            return PinResult::LockedOut;
        }
        if constant_time_eq(given.as_bytes(), self.pin.as_bytes()) {
            PinResult::Ok(Token::issue("device", now))
        } else {
            *tries += 1;
            PinResult::Wrong
        }
    }

    /// A fresh key for the QR. Replaces any previous one: only the code
    /// currently on screen should work.
    pub fn new_pairing_key(&mut self, now: u64) -> String {
        let key = random_hex(16);
        self.pairing = Some((key.clone(), now));
        key
    }

    pub fn try_pairing_key(&mut self, given: &str, now: u64) -> Option<Token> {
        let (key, issued) = self.pairing.as_ref()?;
        let fresh = now.saturating_sub(*issued) < PAIRING_TTL;
        let matches = constant_time_eq(given.as_bytes(), key.as_bytes());
        if !(fresh && matches) {
            return None;
        }
        self.pairing = None; // single use
        Some(Token::issue("device", now))
    }

    /// File a freshly issued token under the label the User-Agent implies.
    pub fn remember(&mut self, mut token: Token, label: &str) -> Token {
        token.label = label.to_string();
        self.tokens.insert(token.value.clone(), token.clone());
        token
    }

    /// Reinstated from `transfers.db` at start, so a phone paired an hour before
    /// the app restarted does not have to retype the PIN.
    pub fn restore(&mut self, token: Token) {
        self.tokens.insert(token.value.clone(), token);
    }

    /// The token behind a cookie, if it is one we issued and has not expired.
    /// Touches `last_seen` so the paired-devices list stays honest. Returns a
    /// clone rather than a borrow: every caller holds a mutex guard and would
    /// otherwise keep it borrowed for the rest of the handler.
    pub fn check(&mut self, value: &str, now: u64) -> Option<Token> {
        let token = self.tokens.get_mut(value)?;
        if !token.valid_at(now) {
            return None;
        }
        token.last_seen = now;
        Some(token.clone())
    }

    pub fn forget(&mut self, value: &str) {
        self.tokens.remove(value);
    }

    /// (token, label, last_seen) for every still-valid device, newest first.
    pub fn devices(&self, now: u64) -> Vec<(String, String, u64)> {
        let mut out: Vec<(String, String, u64)> = self
            .tokens
            .values()
            .filter(|t| t.valid_at(now))
            .map(|t| (t.value.clone(), t.label.clone(), t.last_seen))
            .collect();
        out.sort_by(|a, b| b.2.cmp(&a.2));
        out
    }
}

/// Compares without leaking where the mismatch was through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random_pin() -> String {
    let n = random_u64() % 1_000_000;
    format!("{n:06}")
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("system RNG unavailable");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// `getrandom` is already in the tree under rustls; used rather than adding an
/// RNG crate. If it ever fails, refuse to produce a key rather than fall back
/// to something guessable.
fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("system RNG unavailable");
    u64::from_le_bytes(buf)
}

/// A human label for the paired-devices list, from the User-Agent. Nothing here
/// is trusted — it is a display string only, never a decision.
pub fn label_for(user_agent: &str) -> String {
    let os = if user_agent.contains("Android") {
        "Android"
    } else if user_agent.contains("iPhone") {
        "iPhone"
    } else if user_agent.contains("iPad") {
        "iPad"
    } else if user_agent.contains("Windows") {
        "Windows"
    } else if user_agent.contains("Mac OS X") {
        "Mac"
    } else if user_agent.contains("Linux") {
        "Linux"
    } else {
        "Device"
    };
    // Order matters: Edge and Chrome both claim Safari, Edge also claims Chrome.
    let browser = if user_agent.contains("Edg/") {
        "Edge"
    } else if user_agent.contains("Firefox") {
        "Firefox"
    } else if user_agent.contains("Chrome") {
        "Chrome"
    } else if user_agent.contains("Safari") {
        "Safari"
    } else {
        "Browser"
    };
    format!("{os} · {browser}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> u64 {
        1_700_000_000 + secs
    }

    #[test]
    fn pin_is_six_digits_and_changes_each_session() {
        let a = Auth::new(at(0));
        assert_eq!(a.pin().len(), 6);
        assert!(a.pin().chars().all(|c| c.is_ascii_digit()));
        // Two sessions in a row producing the same PIN would mean the RNG is
        // not being reseeded.
        let same = (0..20).all(|_| Auth::new(at(0)).pin() == a.pin());
        assert!(!same, "PIN never changed across 20 sessions");
    }

    #[test]
    fn correct_pin_is_accepted_and_wrong_pin_is_not() {
        let mut a = Auth::new(at(0));
        let pin = a.pin().to_string();
        assert!(matches!(a.try_pin("1.2.3.4", &pin, at(1)), PinResult::Ok(_)));
        assert!(matches!(a.try_pin("1.2.3.4", "000000", at(2)), PinResult::Wrong));
    }

    #[test]
    fn five_wrong_pins_lock_that_ip_out_and_leave_others_alone() {
        let mut a = Auth::new(at(0));
        let pin = a.pin().to_string();
        for _ in 0..5 {
            assert!(matches!(a.try_pin("1.2.3.4", "000000", at(1)), PinResult::Wrong));
        }
        // Sixth try is refused even though the PIN is right.
        assert!(matches!(a.try_pin("1.2.3.4", &pin, at(1)), PinResult::LockedOut));
        // A different phone on the same network is unaffected.
        assert!(matches!(a.try_pin("5.6.7.8", &pin, at(1)), PinResult::Ok(_)));
    }

    #[test]
    fn wrong_guesses_are_reported_per_address_worst_first() {
        let mut a = Auth::new(at(0));
        assert!(a.wrong_attempts().is_empty(), "nothing to report before a guess");

        a.try_pin("1.2.3.4", "000000", at(1));
        for _ in 0..3 {
            a.try_pin("5.6.7.8", "000000", at(1));
        }
        let seen = a.wrong_attempts();
        assert_eq!(seen, vec![("5.6.7.8".to_string(), 3), ("1.2.3.4".to_string(), 1)]);

        // A correct PIN does not add a row.
        let pin = a.pin().to_string();
        a.try_pin("9.9.9.9", &pin, at(2));
        assert_eq!(a.wrong_attempts().len(), 2);
    }

    #[test]
    fn pairing_key_works_once_and_only_once() {
        let mut a = Auth::new(at(0));
        let key = a.new_pairing_key(at(0));
        assert!(a.try_pairing_key(&key, at(10)).is_some());
        assert!(a.try_pairing_key(&key, at(11)).is_none(), "key was reusable");
    }

    #[test]
    fn pairing_key_expires_after_sixty_seconds() {
        let mut a = Auth::new(at(0));
        let key = a.new_pairing_key(at(0));
        assert!(a.try_pairing_key(&key, at(59)).is_some());

        let mut b = Auth::new(at(0));
        let key = b.new_pairing_key(at(0));
        assert!(b.try_pairing_key(&key, at(61)).is_none(), "expired key accepted");
    }

    #[test]
    fn device_token_lives_forty_eight_hours() {
        const H: u64 = 3600;
        let issued = at(0);
        let tok = Token::issue("Android · Chrome", issued);
        assert!(tok.valid_at(issued + 47 * H), "rejected inside the window");
        assert!(!tok.valid_at(issued + 49 * H), "accepted past the window");
        // Exactly 48h is the boundary: expired, not valid.
        assert!(!tok.valid_at(issued + 48 * H));
    }

    #[test]
    fn a_remembered_token_opens_the_api_until_it_expires() {
        const H: u64 = 3600;
        let mut a = Auth::new(at(0));
        let tok = a.remember(Token::issue("device", at(0)), "Android · Chrome");
        assert!(a.check(&tok.value, at(1)).is_some());
        assert!(a.check(&tok.value, at(0) + 49 * H).is_none(), "expired token accepted");
        assert!(a.check("not-a-token", at(1)).is_none());
    }

    #[test]
    fn forgetting_a_device_refuses_it_immediately() {
        let mut a = Auth::new(at(0));
        let tok = a.remember(Token::issue("device", at(0)), "Android · Chrome");
        assert_eq!(a.devices(at(1)).len(), 1);
        a.forget(&tok.value);
        assert!(a.check(&tok.value, at(1)).is_none());
        assert!(a.devices(at(1)).is_empty());
    }

    #[test]
    fn user_agents_become_readable_labels() {
        assert_eq!(
            label_for("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/120 Safari/537.36"),
            "Android · Chrome"
        );
        // Edge claims both Chrome and Safari; the order of the checks matters.
        assert_eq!(
            label_for("Mozilla/5.0 (Windows NT 10.0) Chrome/120 Safari/537.36 Edg/120"),
            "Windows · Edge"
        );
        assert_eq!(label_for(""), "Device · Browser");
    }
}
