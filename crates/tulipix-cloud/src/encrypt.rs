//! `np.p4.cloud.encrypt` — rclone config encrypted at rest; password in the OS
//! keychain.
//!
//! rclone can AES-encrypt its config when given a password via
//! `RCLONE_CONFIG_PASS`. We never store that password on disk — it lives in
//! the OS keychain (keyring). This owns the keychain entry naming, the env the
//! subprocess gets, and the encrypt/decrypt argv.

pub const KEYCHAIN_SERVICE: &str = "tulipix.cloud";
pub const CONFIG_PASS_ENTRY: &str = "rclone-config-pass";

/// keyring [`keyring::Entry`] for the rclone config password.
pub fn config_pass_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, CONFIG_PASS_ENTRY)
}

/// Environment pairs to pass the rclone subprocess so it can decrypt config
/// without a TTY prompt. `pass` is read from the keychain by the caller.
pub fn pass_env(pass: &str) -> Vec<(String, String)> {
    vec![("RCLONE_CONFIG_PASS".into(), pass.into())]
}

/// argv to turn on config encryption (rclone re-writes config encrypted).
pub fn encrypt_args() -> Vec<String> {
    vec!["config".into(), "encryption".into(), "set".into()]
}

/// argv to remove config encryption.
pub fn decrypt_args() -> Vec<String> {
    vec!["config".into(), "encryption".into(), "remove".into()]
}

/// Quick check that decrypted config text looks like rclone INI (has a
/// `[remote]` section header). Used to detect a wrong password.
pub fn looks_decrypted(config_text: &str) -> bool {
    config_text.lines().any(|l| {
        let t = l.trim();
        t.starts_with('[') && t.ends_with(']') && t.len() > 2
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_carries_pass() {
        let e = pass_env("hunter2");
        assert_eq!(e[0], ("RCLONE_CONFIG_PASS".to_string(), "hunter2".to_string()));
    }

    #[test]
    fn detects_decrypted_ini() {
        assert!(looks_decrypted("[gdrive]\ntype = drive\n"));
        assert!(!looks_decrypted("\u{0}garbage-ciphertext"));
        assert_eq!(encrypt_args()[2], "set");
    }
}
