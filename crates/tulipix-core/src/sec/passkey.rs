//! Unlocking with something other than a PIN: a fingerprint, or a security
//! key you touch.
//!
//! Both go through a program that is already on the system rather than a
//! library linked into Tulipix, because both talk to hardware through a
//! daemon that already exists and already has the user's enrolments:
//!
//!   * **Fingerprint** — fprintd, through `fprintd-list` and `fprintd-verify`.
//!     The fingers are the ones enrolled for the desktop login; Tulipix never
//!     sees a print, never stores one, and never enrols one. On macOS and
//!     Windows there is no such program, and this reports that plainly rather
//!     than pretending.
//!   * **Security key** — libfido2's `fido2-token`, `fido2-cred` and
//!     `fido2-assert`. Enrolling makes one credential on the key, scoped to
//!     `tulipix.local`, and the credential id is what is kept here. Nothing
//!     secret is stored: the id is public, and the key will not produce an
//!     assertion for it without being present and touched.
//!
//! ponytail: a successful `fido2-assert` is taken as proof, rather than the
//! signature being verified here against the stored public key — that needs
//! ES256/EdDSA verification and a crypto dependency this tree does not carry.
//! What it actually proves is that the physical key was plugged in and
//! touched, which is the thing being asked. Forging it means replacing
//! `fido2-assert` on this machine, and an attacker who can do that already
//! has everything. Upgrade path: keep the public key from `fido2-cred` (it is
//! already stored) and verify the assertion with p256 when something else
//! pulls that crate in.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use crate::paths;

/// The relying party. Never leaves this machine: a security key scopes its
/// credential to this name, and a credential made for Tulipix cannot be used
/// against a website.
pub const RP_ID: &str = "tulipix.local";
pub const RP_NAME: &str = "Tulipix";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasskeyKind {
    /// The desktop's fingerprint reader, through fprintd.
    Fingerprint,
    /// A FIDO2 security key (YubiKey, Titan, SoloKey, Nitrokey).
    SecurityKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PasskeyCredential {
    /// Base64 credential id for a security key. Empty for a fingerprint,
    /// which has no credential: the reader holds the enrolment.
    #[serde(default)]
    pub id: String,
    pub kind: PasskeyKind,
    /// What Settings shows: "YubiKey 5 NFC", or the user whose finger it is.
    pub label: String,
    /// Base64 public key from `fido2-cred`. Not used to verify yet (see the
    /// note at the top), and kept because throwing it away would mean
    /// re-enrolling every key the day it is.
    #[serde(default)]
    pub public_key: String,
    pub created_at: u64,
}

/// Where the enrolments are kept. Nothing in it is secret.
pub fn store_path() -> Option<PathBuf> {
    paths::config_dir().map(|d| d.join("passkeys.json"))
}

pub fn list() -> Vec<PasskeyCredential> {
    let Some(p) = store_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(p) else { return Vec::new() };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_all(creds: &[PasskeyCredential]) -> Result<()> {
    let path = store_path().context("no configuration directory on this system")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(creds)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn add(cred: PasskeyCredential) -> Result<()> {
    let mut all = list();
    // A fingerprint is one enrolment however many fingers are behind it; a
    // security key is one per credential id.
    let already = all.iter().any(|c| match cred.kind {
        PasskeyKind::Fingerprint => matches!(c.kind, PasskeyKind::Fingerprint),
        PasskeyKind::SecurityKey => c.id == cred.id,
    });
    if already {
        anyhow::bail!("that one is already set up");
    }
    all.push(cred);
    write_all(&all)
}

pub fn remove(id: &str, kind: PasskeyKind) -> Result<()> {
    let mut all = list();
    let before = all.len();
    all.retain(|c| !(c.kind == kind && c.id == id));
    if all.len() == before {
        anyhow::bail!("that one is not set up");
    }
    write_all(&all)
}

pub fn has(kind: PasskeyKind) -> bool {
    list().iter().any(|c| c.kind == kind)
}

/// Whether anything at all is enrolled — what Settings and the lock screen
/// ask before offering the button.
pub fn any_enrolled() -> bool {
    !list().is_empty()
}

/// A program on PATH. Both back ends are optional system packages, so "is it
/// there" is a real question with a real answer for the user.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(name).is_file()))
        .unwrap_or(false)
}

// ── fingerprint, through fprintd ────────────────────────────────────────────

pub mod fingerprint {
    use super::*;

    /// fprintd is Linux-only. Nothing here pretends otherwise: Touch ID and
    /// Windows Hello need a platform API, not a program, and this build has
    /// neither.
    pub fn available() -> bool {
        cfg!(target_os = "linux") && on_path("fprintd-verify") && on_path("fprintd-list")
    }

    pub fn why_not() -> &'static str {
        if !cfg!(target_os = "linux") {
            "Fingerprint unlock needs fprintd, which is Linux only. This build has no Touch ID or Windows Hello."
        } else {
            "fprintd is not installed. Your distribution packages it as fprintd."
        }
    }

    /// Whether the current user has a finger enrolled with fprintd. Tulipix
    /// never enrols one — that is the desktop's own settings — so this is the
    /// question that decides whether the option is offered at all.
    pub fn enrolled() -> bool {
        let Some(user) = current_user() else { return false };
        let Ok(out) = Command::new("fprintd-list").arg(&user).output() else {
            return false;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        parse_enrolled(&text)
    }

    /// Ask for a finger, and wait. Blocking, and it can take as long as the
    /// user takes: the caller runs it off whatever thread is drawing.
    pub fn verify() -> Result<bool> {
        if !available() {
            anyhow::bail!("{}", why_not());
        }
        let out = Command::new("fprintd-verify")
            .output()
            .context("could not run fprintd-verify")?;
        // fprintd-verify exits non-zero on a no-match as well as on an error,
        // so the reason comes from what it printed.
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(parse_verify(&text, out.status.success()))
    }

    fn current_user() -> Option<String> {
        std::env::var("USER").ok().or_else(|| std::env::var("LOGNAME").ok())
    }

    /// `fprintd-list` prints "Fingerprints for user alice on …" and then a
    /// line per finger, or "has no fingers enrolled".
    pub(super) fn parse_enrolled(text: &str) -> bool {
        let lower = text.to_ascii_lowercase();
        if lower.contains("no fingers enrolled") || lower.contains("has no fingerprints") {
            return false;
        }
        lower.contains("- #")
    }

    /// "Verify result: verify-match" is the only success. A match that the
    /// process reported as a failure is still a match — some versions exit
    /// non-zero after a retry — and a zero exit with no match is not one.
    pub(super) fn parse_verify(text: &str, success: bool) -> bool {
        let lower = text.to_ascii_lowercase();
        if lower.contains("verify-match") {
            return true;
        }
        if lower.contains("verify-no-match") || lower.contains("verify-retry") {
            return false;
        }
        success
    }
}

// ── security key, through libfido2 ──────────────────────────────────────────

pub mod security_key {
    use super::*;
    use std::io::Write;

    pub fn available() -> bool {
        on_path("fido2-token") && on_path("fido2-cred") && on_path("fido2-assert")
    }

    pub fn why_not() -> &'static str {
        "Security-key unlock needs libfido2's tools (fido2-token, fido2-cred, fido2-assert). \
         Your distribution packages them as libfido2 or fido2-tools."
    }

    /// The keys plugged in now, as `(device path, description)`.
    /// `fido2-token -L` prints `/dev/hidraw4: vendor=0x1050, product=0x0407 (Yubico YubiKey)`.
    pub fn devices() -> Vec<(String, String)> {
        let Ok(out) = Command::new("fido2-token").arg("-L").output() else {
            return Vec::new();
        };
        parse_devices(&String::from_utf8_lossy(&out.stdout))
    }

    pub(super) fn parse_devices(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|line| {
                let (path, rest) = line.split_once(": ")?;
                let path = path.trim();
                if path.is_empty() {
                    return None;
                }
                // The friendly name is in brackets at the end, when there is
                // one; the vendor/product pair is the fallback.
                let label = match (rest.rfind('('), rest.rfind(')')) {
                    (Some(a), Some(b)) if b > a + 1 => rest[a + 1..b].trim().to_string(),
                    _ => rest.trim().to_string(),
                };
                Some((path.to_string(), label))
            })
            .collect()
    }

    /// Make one credential on the key. The user is asked to touch it.
    ///
    /// `fido2-cred -M` reads four lines on stdin — a client-data hash, the
    /// relying party id, the user name, and a user id — and writes the
    /// credential out, which `-V` turns into the id and public key.
    pub fn enrol(device: &str, label: &str) -> Result<PasskeyCredential> {
        if !available() {
            anyhow::bail!("{}", why_not());
        }
        let input = format!(
            "{}\n{RP_ID}\n{RP_NAME}\n{}\n",
            b64(&random_bytes(32)?),
            b64(&random_bytes(32)?)
        );
        let out = run_with_input(&["fido2-cred", "-M", "-r", device], &input)
            .context("could not make a credential on the key")?;
        let verified = run_with_input(&["fido2-cred", "-V", "-r"], &out)
            .context("the key made a credential the tools could not read back")?;
        let (id, public_key) = parse_cred(&verified)
            .context("the key's answer did not hold a credential id")?;
        Ok(PasskeyCredential {
            id,
            kind: PasskeyKind::SecurityKey,
            label: label.to_string(),
            public_key,
            created_at: crate::util::unix_secs(),
        })
    }

    /// Ask the key for an assertion over the stored credential. True only
    /// when the key was there and was touched.
    pub fn verify(cred: &PasskeyCredential) -> Result<bool> {
        if !available() {
            anyhow::bail!("{}", why_not());
        }
        let Some((device, _)) = devices().into_iter().next() else {
            anyhow::bail!("no security key is plugged in");
        };
        // `fido2-assert -G` reads the client-data hash, the relying party id
        // and the credential id.
        let input = format!("{}\n{RP_ID}\n{}\n", b64(&random_bytes(32)?), cred.id);
        match run_with_input(&["fido2-assert", "-G", &device], &input) {
            Ok(out) => Ok(!out.trim().is_empty()),
            Err(e) => {
                tracing::debug!(error = %e, "security key: assertion refused");
                Ok(false)
            }
        }
    }

    fn run_with_input(argv: &[&str], input: &str) -> Result<String> {
        use std::process::Stdio;
        let mut child = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not run {}", argv[0]))?;
        child
            .stdin
            .as_mut()
            .context("no stdin on the child")?
            .write_all(input.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            anyhow::bail!(
                "{} said: {}",
                argv[0],
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `fido2-cred -V` prints the credential id on the second line and the
    /// public key in the PEM block after it.
    pub(super) fn parse_cred(text: &str) -> Option<(String, String)> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        // Line 1 is the relying-party id hash; line 2 is the credential id.
        let _rp = lines.next()?;
        let id = lines.next()?.to_string();
        if id.is_empty() {
            return None;
        }
        // Everything after, which is the PEM public key, kept as one string.
        let public_key = lines.collect::<Vec<_>>().join("\n");
        Some((id, public_key))
    }
}

/// Bytes from the OS, or an error. `getrandom` is already in the tree, under
/// rustls. There is deliberately no fallback: a predictable challenge is worse
/// than no unlock at all, so a machine with no entropy source refuses rather
/// than enrolling something forgeable.
fn random_bytes(n: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).context("this system has no working random source")?;
    Ok(buf)
}

/// Standard Base64, which is what the libfido2 tools read and write.
fn b64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_challenge_is_thirty_two_bytes_and_not_the_same_twice() {
        let a = random_bytes(32).expect("system RNG");
        assert_eq!(a.len(), 32);
        assert_ne!(a, random_bytes(32).expect("system RNG"));
    }

    #[test]
    fn fprintd_list_says_whether_a_finger_is_enrolled() {
        use fingerprint::parse_enrolled;
        assert!(parse_enrolled(
            "Fingerprints for user alice on Synaptics (press):\n - #0: right-index-finger\n"
        ));
        assert!(!parse_enrolled("User alice has no fingers enrolled for Synaptics"));
        assert!(!parse_enrolled(""));
    }

    #[test]
    fn only_a_match_is_a_match() {
        use fingerprint::parse_verify;
        assert!(parse_verify("Verify result: verify-match (done)", true));
        // Some versions exit non-zero after a retry but still matched.
        assert!(parse_verify("Verify result: verify-match (done)", false));
        assert!(!parse_verify("Verify result: verify-no-match (done)", false));
        assert!(!parse_verify("Verify result: verify-retry-scan", true));
        // Nothing recognisable: the exit status is all there is.
        assert!(parse_verify("", true));
        assert!(!parse_verify("", false));
    }

    #[test]
    fn fido2_token_listing_reads_the_device_and_its_name() {
        use security_key::parse_devices;
        let d = parse_devices(
            "/dev/hidraw4: vendor=0x1050, product=0x0407 (Yubico YubiKey OTP+FIDO+CCID)\n\
             /dev/hidraw7: vendor=0x20a0, product=0x42b1\n",
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].0, "/dev/hidraw4");
        assert_eq!(d[0].1, "Yubico YubiKey OTP+FIDO+CCID");
        assert_eq!(d[1].0, "/dev/hidraw7");
        assert!(d[1].1.contains("0x20a0"));
        assert!(parse_devices("").is_empty());
    }

    #[test]
    fn a_verified_credential_gives_up_its_id_and_key() {
        use security_key::parse_cred;
        let text = "aGFzaA==\nY3JlZElk\n-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----\n";
        let (id, pk) = parse_cred(text).unwrap();
        assert_eq!(id, "Y3JlZElk");
        assert!(pk.contains("BEGIN PUBLIC KEY"));
        assert!(parse_cred("only-one-line").is_none());
    }

    #[test]
    fn one_fingerprint_enrolment_but_a_key_each() {
        // Pure bookkeeping, checked without touching the store on disk.
        let fp = PasskeyCredential {
            id: String::new(),
            kind: PasskeyKind::Fingerprint,
            label: "alice".into(),
            public_key: String::new(),
            created_at: 0,
        };
        let key = |id: &str| PasskeyCredential {
            id: id.into(),
            kind: PasskeyKind::SecurityKey,
            label: id.into(),
            public_key: String::new(),
            created_at: 0,
        };
        let all = vec![fp.clone(), key("a")];
        let dup = |c: &PasskeyCredential| {
            all.iter().any(|x| match c.kind {
                PasskeyKind::Fingerprint => matches!(x.kind, PasskeyKind::Fingerprint),
                PasskeyKind::SecurityKey => x.id == c.id,
            })
        };
        assert!(dup(&fp));
        assert!(dup(&key("a")));
        assert!(!dup(&key("b")));
    }
}
