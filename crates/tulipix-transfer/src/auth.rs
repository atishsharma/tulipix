//! Three mechanisms, each doing one job: a PIN the user reads off the screen, a
//! single-use pairing key carried by the QR, and a 48-hour device token so a
//! phone that has already paired does not retype anything.

use std::collections::HashMap;

/// Wrong PINs from one address before that address is done for the session.
pub const MAX_ATTEMPTS: u32 = 5;
/// How long the QR's pairing key stays good.
///
/// Ten minutes, not one. A minute is plenty to walk to the phone, but the first
/// connection from any device now goes through installing the root certificate
/// first — a trip through the phone's Settings that takes several. At sixty
/// seconds the key was always dead by the time the gateway crossed over, so the
/// one flow that most needed pairing to be automatic was the one that always
/// landed on the PIN form.
const PAIRING_TTL: u64 = 600;
/// "48 hours, then re-token."
pub const TOKEN_TTL: u64 = 48 * 3600;
/// Phones paired at once. The eleventh is refused rather than quietly evicting
/// one of the ten: whoever is holding the desktop decides which device loses its
/// place, and the Connection card is where they do it.
pub const MAX_DEVICES: usize = 10;
/// A device name has to fit inside a 56px circle's caption, so it is short by
/// construction rather than elided at draw time.
pub const NAME_MAX: usize = 15;
/// Unspent pairing keys held at once. Four redraws' worth, which at one every
/// five minutes is more than the ten a key lives — the cap exists so a desktop
/// left sharing overnight cannot grow the list without bound.
const MAX_PAIRING_KEYS: usize = 4;
/// How long a device keeps counting as busy after the last byte moved. Long
/// enough to bridge the gaps between chunks on a slow link, short enough that the
/// ring goes out promptly when the transfer really has stopped.
pub const ACTIVE_WINDOW: u64 = 3;

pub enum PinResult {
    Ok(Token),
    Wrong,
    LockedOut,
}

#[derive(Clone, Debug, Default)]
pub struct Token {
    pub value: String,
    /// What the User-Agent implies, in full: "Android · Chrome".
    pub label: String,
    /// Just the device type — "Android", "iPhone", "Windows". The Connection
    /// card's default caption, and what picks the icon.
    pub kind: String,
    /// The address it paired from, so a device on the list can be identified
    /// against the router when two phones report the same type.
    pub ip: String,
    /// A name typed on the desktop. Empty means "call it by its type".
    pub name: String,
    /// The PIN that was on screen when this device paired. Stored per device
    /// rather than read live: the session PIN is regenerated at every start, so
    /// showing today's PIN beside a phone that paired yesterday would be a lie.
    pub pin: String,
    pub issued: u64,
    pub expires: u64,
    pub last_seen: u64,
    /// Bytes were moving for this device until this second. Not persisted: a
    /// transfer cannot survive a restart, so a stored value could only ever be
    /// wrong.
    pub active_until: u64,
}

impl Token {
    /// A bare token. The descriptive fields are filled in by [`Auth::remember`],
    /// which is the only place that knows the request they came from.
    pub fn issue(now: u64) -> Self {
        Self {
            value: random_hex(32),
            issued: now,
            expires: now + TOKEN_TTL,
            last_seen: now,
            ..Self::default()
        }
    }

    pub fn valid_at(&self, now: u64) -> bool {
        now < self.expires
    }

    /// What to call it: the typed name, or the device type it reported.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            if self.kind.is_empty() { "Device type" } else { &self.kind }
        } else {
            &self.name
        }
    }

    /// Seconds left before it has to type the PIN again. Saturating, so an
    /// already-expired token reports zero rather than wrapping.
    pub fn remaining(&self, now: u64) -> u64 {
        self.expires.saturating_sub(now)
    }

    /// A file is moving to or from this device right now.
    pub fn busy(&self, now: u64) -> bool {
        self.active_until > now
    }
}

pub struct Auth {
    pin: String,
    attempts: HashMap<String, u32>,
    /// Unspent pairing keys, oldest first: `(key, issued)`.
    ///
    /// A list, not one slot. The QR is redrawn every few minutes so the code on
    /// screen always has life left, and with a single slot each redraw silently
    /// destroyed the key a phone was still holding — walk away to install a
    /// certificate, come back, and the scan that started it all had been
    /// invalidated by the desktop redrawing behind you. Every key still inside
    /// its own ten minutes stays valid; each is still single-use.
    pairing: Vec<(String, u64)>,
    tokens: HashMap<String, Token>,
}

impl Auth {
    pub fn new(_now: u64) -> Self {
        Self {
            pin: random_pin(),
            attempts: HashMap::new(),
            pairing: Vec::new(),
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
            PinResult::Ok(Token::issue(now))
        } else {
            *tries += 1;
            PinResult::Wrong
        }
    }

    /// A fresh key for the QR.
    ///
    /// Added alongside whatever is still unspent rather than replacing it — see
    /// [`Auth::pairing`]. Expired entries are dropped here, so the list is
    /// pruned by the same clock that fills it, and it is capped so a desktop
    /// left running overnight cannot accumulate one per redraw.
    pub fn new_pairing_key(&mut self, now: u64) -> String {
        let key = random_hex(16);
        self.pairing.retain(|(_, issued)| now.saturating_sub(*issued) < PAIRING_TTL);
        if self.pairing.len() >= MAX_PAIRING_KEYS {
            self.pairing.remove(0);
        }
        self.pairing.push((key.clone(), now));
        key
    }

    /// Is the code currently on screen still worth showing?
    ///
    /// False once it has been spent or once it is closer to expiry than half its
    /// life — five minutes, which is what the code on screen is guaranteed to
    /// have left. Both cases used to be invisible: the QR was drawn once per address
    /// and never again, so the first phone to scan it consumed the key and every
    /// later scan — including the same phone re-pairing after being forgotten —
    /// landed on the PIN form with the desktop still showing a dead code.
    pub fn pairing_live(&self, now: u64) -> bool {
        self.pairing.iter().any(|(_, issued)| now.saturating_sub(*issued) < PAIRING_TTL / 2)
    }

    pub fn try_pairing_key(&mut self, given: &str, now: u64) -> Option<Token> {
        let at = self.pairing.iter().position(|(key, issued)| {
            now.saturating_sub(*issued) < PAIRING_TTL
                && constant_time_eq(given.as_bytes(), key.as_bytes())
        })?;
        self.pairing.remove(at); // single use
        Some(Token::issue(now))
    }

    /// File a freshly issued token under what the request said about itself.
    ///
    /// `None` when ten devices are already paired — see [`MAX_DEVICES`]. Checked
    /// here rather than in the handlers so the PIN form and the QR cannot
    /// disagree about the limit.
    pub fn remember(&mut self, mut token: Token, label: &str, kind: &str, ip: &str) -> Option<Token> {
        if self.tokens.len() >= MAX_DEVICES && !self.tokens.contains_key(&token.value) {
            return None;
        }
        token.label = label.to_string();
        token.kind = kind.to_string();
        token.ip = ip.to_string();
        token.pin = self.pin.clone();
        self.tokens.insert(token.value.clone(), token.clone());
        Some(token)
    }

    /// Rename a paired device. Returns the stored name, which is the given one
    /// trimmed and cut to [`NAME_MAX`] characters — the cut happens here so the
    /// desktop field and the phone list cannot end up with different names.
    pub fn rename(&mut self, value: &str, name: &str) -> Option<String> {
        let name = clamp_name(name);
        let token = self.tokens.get_mut(value)?;
        token.name = name.clone();
        Some(name)
    }

    /// Note that this device is moving bytes, as of `now`. Called per chunk on
    /// both directions, which is what makes the ring go out on its own when the
    /// transfer ends — there is no "finished" event to miss.
    pub fn mark_active(&mut self, value: &str, now: u64) {
        if let Some(token) = self.tokens.get_mut(value) {
            token.active_until = now + ACTIVE_WINDOW;
        }
    }

    /// Paired devices, including expired ones? No: valid only, and that is what
    /// the count is measured against.
    pub fn device_count(&self, now: u64) -> usize {
        self.tokens.values().filter(|t| t.valid_at(now)).count()
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

    /// Every still-valid device, newest first. The whole token: the Connection
    /// card shows the name, the type, the address, the PIN it paired with and how
    /// long it has left, and all five come from here.
    pub fn devices(&self, now: u64) -> Vec<Token> {
        let mut out: Vec<Token> = self.tokens.values().filter(|t| t.valid_at(now)).cloned().collect();
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
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

/// The device type a User-Agent implies. The Connection card's default caption,
/// and what chooses the icon on the round button.
pub fn kind_for(user_agent: &str) -> &'static str {
    if user_agent.contains("Android") {
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
    }
}

/// A device name as it will be stored: trimmed, and cut to [`NAME_MAX`]
/// characters rather than bytes — fifteen emoji is fifteen characters.
pub fn clamp_name(raw: &str) -> String {
    raw.trim().chars().take(NAME_MAX).collect::<String>().trim_end().to_string()
}

/// A human label for the paired-devices list, from the User-Agent. Nothing here
/// is trusted — it is a display string only, never a decision.
pub fn label_for(user_agent: &str) -> String {
    let os = kind_for(user_agent);
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
    fn a_spent_or_ageing_key_reports_itself_dead_so_the_qr_gets_redrawn() {
        let mut a = Auth::new(at(0));
        assert!(!a.pairing_live(at(0)), "live before a key was ever minted");

        let key = a.new_pairing_key(at(0));
        assert!(a.pairing_live(at(0)));
        // Past half the TTL it is stale: the code on screen must always have
        // enough life left to scan it and finish a certificate install.
        assert!(a.pairing_live(at(299)));
        assert!(!a.pairing_live(at(301)));

        // Spending it is the case that actually bit — pair, forget, scan again.
        let mut b = Auth::new(at(0));
        let key2 = b.new_pairing_key(at(0));
        assert!(b.try_pairing_key(&key2, at(1)).is_some());
        assert!(!b.pairing_live(at(1)), "a spent key still claimed to be live");
        let _ = key;
    }

    #[test]
    fn redrawing_the_qr_does_not_invalidate_a_key_someone_is_still_holding() {
        let mut a = Auth::new(at(0));
        // Scanned, then walked off to install a certificate.
        let scanned = a.new_pairing_key(at(0));
        // The desktop redraws behind them, twice, because the code on screen has
        // to keep enough life left to be worth scanning.
        let _ = a.new_pairing_key(at(300));
        let _ = a.new_pairing_key(at(360));
        // They come back. The scan that started this still works.
        assert!(
            a.try_pairing_key(&scanned, at(420)).is_some(),
            "a redraw threw away the key that had already been scanned"
        );
        // Still single-use, redraws or not.
        assert!(a.try_pairing_key(&scanned, at(430)).is_none());
    }

    #[test]
    fn unspent_keys_do_not_pile_up_forever() {
        let mut a = Auth::new(at(0));
        for n in 0..12 {
            let _ = a.new_pairing_key(at(n * 10));
        }
        assert!(a.pairing.len() <= MAX_PAIRING_KEYS, "the key list grew without bound");
        // And an expired one is gone rather than merely capped away.
        let old = a.new_pairing_key(at(1_000));
        let _ = a.new_pairing_key(at(1_000 + PAIRING_TTL + 1));
        assert!(a.try_pairing_key(&old, at(1_000 + PAIRING_TTL + 2)).is_none());
    }

    #[test]
    fn pairing_key_expires_after_ten_minutes() {
        let mut a = Auth::new(at(0));
        let key = a.new_pairing_key(at(0));
        // Long enough to cover a first-run certificate install, which is the
        // whole reason it is not sixty seconds.
        assert!(a.try_pairing_key(&key, at(599)).is_some());

        let mut b = Auth::new(at(0));
        let key = b.new_pairing_key(at(0));
        assert!(b.try_pairing_key(&key, at(601)).is_none(), "expired key accepted");
    }

    #[test]
    fn device_token_lives_forty_eight_hours() {
        const H: u64 = 3600;
        let issued = at(0);
        let tok = Token::issue(issued);
        assert!(tok.valid_at(issued + 47 * H), "rejected inside the window");
        assert!(!tok.valid_at(issued + 49 * H), "accepted past the window");
        // Exactly 48h is the boundary: expired, not valid.
        assert!(!tok.valid_at(issued + 48 * H));
    }

    /// One paired device, as the handlers would file it.
    fn pair(a: &mut Auth, ip: &str, now: u64) -> Token {
        a.remember(Token::issue(now), "Android · Chrome", "Android", ip)
            .expect("device list was full")
    }

    #[test]
    fn a_remembered_token_opens_the_api_until_it_expires() {
        const H: u64 = 3600;
        let mut a = Auth::new(at(0));
        let tok = pair(&mut a, "1.2.3.4", at(0));
        assert!(a.check(&tok.value, at(1)).is_some());
        assert!(a.check(&tok.value, at(0) + 49 * H).is_none(), "expired token accepted");
        assert!(a.check("not-a-token", at(1)).is_none());
    }

    #[test]
    fn forgetting_a_device_refuses_it_immediately() {
        let mut a = Auth::new(at(0));
        let tok = pair(&mut a, "1.2.3.4", at(0));
        assert_eq!(a.devices(at(1)).len(), 1);
        a.forget(&tok.value);
        assert!(a.check(&tok.value, at(1)).is_none());
        assert!(a.devices(at(1)).is_empty());
    }

    #[test]
    fn a_paired_device_carries_what_the_card_shows() {
        let mut a = Auth::new(at(0));
        let pin = a.pin().to_string();
        let tok = pair(&mut a, "192.168.1.31", at(0));
        assert_eq!(tok.kind, "Android");
        assert_eq!(tok.ip, "192.168.1.31");
        // The PIN it paired with, not whatever the next session generates.
        assert_eq!(tok.pin, pin);
        // No name typed yet, so the button is captioned by type.
        assert_eq!(tok.display_name(), "Android");
        assert_eq!(tok.remaining(at(0) + 3600), TOKEN_TTL - 3600);
    }

    #[test]
    fn the_eleventh_device_is_refused_rather_than_evicting_one_of_the_ten() {
        let mut a = Auth::new(at(0));
        for i in 0..MAX_DEVICES {
            assert!(
                a.remember(Token::issue(at(0)), "Android · Chrome", "Android", &format!("10.0.0.{i}"))
                    .is_some(),
                "device {i} was refused inside the limit"
            );
        }
        assert_eq!(a.device_count(at(1)), MAX_DEVICES);
        assert!(
            a.remember(Token::issue(at(0)), "Android · Chrome", "Android", "10.0.0.99").is_none(),
            "an eleventh device was let in"
        );
        // Forgetting one makes room again — that is the whole point of refusing
        // rather than evicting.
        let first = a.devices(at(1))[0].value.clone();
        a.forget(&first);
        assert!(a.remember(Token::issue(at(0)), "Android · Chrome", "Android", "10.0.0.99").is_some());
    }

    #[test]
    fn a_transferring_device_reads_busy_until_the_bytes_stop() {
        let mut a = Auth::new(at(0));
        let tok = pair(&mut a, "1.2.3.4", at(0));
        assert!(!a.devices(at(1))[0].busy(at(1)), "busy before anything moved");

        a.mark_active(&tok.value, at(10));
        assert!(a.devices(at(11))[0].busy(at(11)));
        // The window lapses on its own: there is no end-of-transfer event to
        // miss, which is the whole reason it is a window.
        assert!(!a.devices(at(20))[0].busy(at(10 + ACTIVE_WINDOW)));
        // An unknown token is ignored rather than creating an entry.
        a.mark_active("not-a-token", at(10));
        assert_eq!(a.devices(at(11)).len(), 1);
    }

    #[test]
    fn a_device_name_is_trimmed_and_cut_to_fifteen_characters() {
        let mut a = Auth::new(at(0));
        let tok = pair(&mut a, "1.2.3.4", at(0));
        assert_eq!(a.rename(&tok.value, "  Living room tablet  ").as_deref(), Some("Living room tab"));
        assert_eq!(a.devices(at(1))[0].display_name(), "Living room tab");
        // Cleared back to empty: the caption falls back to the device type.
        a.rename(&tok.value, "   ");
        assert_eq!(a.devices(at(1))[0].display_name(), "Android");
        // Characters, not bytes.
        assert_eq!(clamp_name("अआइईउऊऋएऐओऔकखगघचछजझञट").chars().count(), NAME_MAX);
        assert!(a.rename("not-a-token", "Nope").is_none());
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
