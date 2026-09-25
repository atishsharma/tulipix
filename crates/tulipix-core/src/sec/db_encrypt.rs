//! Encryption at rest for the library databases, through SQLCipher.
//!
//! What is encrypted is the *index* — what you have, where it is, what you
//! did with it. Never the files themselves: those are yours, where you put
//! them, and Tulipix does not move or rewrite them.
//!
//! The key is 32 random bytes from the OS, made once and kept in the system
//! keychain. No PIN at launch: the user asked for a library that opens when
//! they log in, not one that asks twice. What that buys is a stolen disk —
//! the databases are unreadable without the keychain — and what it does not
//! buy is protection from someone already logged in as you.
//!
//! Switching it on or off does not happen while the app is running. The
//! switch records what the user wants; `apply_pending` runs at startup,
//! before a single pool is open, and rewrites each database through
//! `sqlcipher_export`. A half-migrated library is the one outcome worth
//! designing against, so every file is written beside the original and
//! renamed over it only once it is whole.

use anyhow::{Context, Result};

use crate::{api_keys, paths, settings::Settings};

/// The Settings switch.
pub const FLAG: &str = "db-encrypt";
/// The keychain entry holding the key, as hex.
pub const KEY_SERVICE: &str = "db_key";
/// Every plaintext SQLite file opens with this. Anything else is encrypted —
/// SQLCipher encrypts the header too.
pub const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Whether this build can encrypt at all.
///
/// SQLCipher is a different SQLite, compiled in, so it is a build-time
/// choice: `--features db-encrypt`. Without it the switch reports that
/// plainly rather than pretending to protect anything.
pub const fn supported() -> bool {
    cfg!(feature = "db-encrypt")
}

pub fn why_not() -> &'static str {
    "This build was made without SQLCipher. Rebuild with --features db-encrypt to encrypt the library index."
}

/// What the user asked for.
pub fn wanted() -> bool {
    Settings::load().map(|s| s.flag(FLAG, false)).unwrap_or(false)
}

/// Whether a file on disk is encrypted: a plaintext SQLite database starts
/// with a known 16 bytes, and SQLCipher encrypts the header along with
/// everything else. A file that is not there yet counts as not encrypted —
/// it will be created keyed if encryption is on.
pub fn file_is_encrypted(path: &std::path::Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    use std::io::Read;
    let mut head = [0u8; 16];
    match f.read_exact(&mut head) {
        Ok(()) => &head != SQLITE_MAGIC,
        // Shorter than a header: an empty file SQLite has not written yet.
        Err(_) => false,
    }
}

/// The key, made on first use. Hex, because that is the form the `PRAGMA`
/// takes and the form a keychain entry can hold as text.
///
/// Never generated twice: a second key would make every existing database
/// unreadable, so a keychain that already has one always wins.
pub fn key_hex() -> Result<String> {
    if let Some(k) = api_keys::fetch(KEY_SERVICE)?.filter(|k| k.len() == 64) {
        return Ok(k);
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).context("this system has no working random source")?;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    api_keys::store(KEY_SERVICE, &hex).context("could not put the database key in the keychain")?;
    Ok(hex)
}

/// The value for `PRAGMA key`, quoted as SQLCipher wants a raw key:
/// `"x'<64 hex chars>'"`. sqlx writes `PRAGMA key = <value>;`, so the quoting
/// has to be inside the value; it sends `key` before every other pragma,
/// which is the order SQLCipher requires.
pub fn pragma_value(key_hex: &str) -> String {
    format!("\"x'{key_hex}'\"")
}

/// The same key as a SQL blob literal, for `ATTACH … KEY`. No double quotes
/// here: in SQL those mean an identifier, and the KEY clause wants a value.
///
/// Only `convert` calls it, and `convert` only exists with the feature on —
/// but the tests below check it either way, and `dead_code` does not count a
/// test as a use.
#[cfg_attr(not(feature = "db-encrypt"), allow(dead_code))]
fn blob_literal(key_hex: &str) -> String {
    format!("x'{key_hex}'")
}

/// A path inside a SQL string literal. Rare, and silently wrong if left out:
/// a folder with an apostrophe in it would end the literal early.
#[cfg_attr(not(feature = "db-encrypt"), allow(dead_code))]
fn sql_path(path: &std::path::Path) -> String {
    path.display().to_string().replace('\'', "''")
}

/// The key to open databases with right now, or `None` when they are
/// plaintext. Read once per process: it is a keychain round trip, and the
/// answer cannot change while the app runs — the switch only takes effect at
/// the next start.
pub fn active_key() -> Option<String> {
    static KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        if !supported() || !wanted() {
            return None;
        }
        match key_hex() {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::error!(error = %e, "database key unavailable; opening unencrypted");
                None
            }
        }
    })
    .clone()
}

/// What the Settings row shows.
pub enum State {
    /// This build has no SQLCipher.
    Unsupported,
    Off,
    /// On, and every database is encrypted.
    On,
    /// The switch and the files disagree, which means a restart has not
    /// happened yet.
    PendingOn,
    PendingOff,
}

pub fn state() -> State {
    if !supported() {
        return State::Unsupported;
    }
    let want = wanted();
    let any_encrypted = section_paths().iter().any(|p| file_is_encrypted(p));
    let all_encrypted = {
        let paths = section_paths();
        let existing: Vec<_> = paths.into_iter().filter(|p| p.exists()).collect();
        !existing.is_empty() && existing.iter().all(|p| file_is_encrypted(p))
    };
    match (want, all_encrypted, any_encrypted) {
        (true, true, _) => State::On,
        (true, false, _) => State::PendingOn,
        (false, _, true) => State::PendingOff,
        (false, _, false) => State::Off,
    }
}

/// The database files this touches. The section databases and nothing else:
/// the thumbnail cache holds no index, and the settings file is not a
/// database.
pub fn section_paths() -> Vec<std::path::PathBuf> {
    const FILES: [&str; 14] = [
        "photos", "videos", "music", "books", "cloud", "podcasts", "radio", "youtube", "tools",
        "transfers", "feeds", "journal", "kitchen", "papers",
    ];
    FILES.iter().filter_map(|s| paths::db_path(s)).collect()
}

/// Bring every database into line with the switch. Runs at startup, before
/// any pool is open; a no-op when they already agree, which is every start
/// but the one after the switch is flipped.
///
/// Returns how many databases it rewrote.
pub fn apply_pending() -> Result<usize> {
    if !supported() {
        return Ok(0);
    }
    let want = wanted();
    let mut changed = 0;
    for path in section_paths() {
        if !path.exists() {
            continue;
        }
        let is_enc = file_is_encrypted(&path);
        if is_enc == want {
            continue;
        }
        let key = key_hex()?;
        let what = if want { "encrypt" } else { "decrypt" };
        match convert(&path, &key, want) {
            Ok(()) => {
                tracing::info!(path = %path.display(), what, "database converted");
                changed += 1;
            }
            // One database that will not convert must not stop the app or the
            // other nine: it is left exactly as it was and said out loud.
            Err(e) => tracing::error!(path = %path.display(), what, error = %e, "database conversion failed"),
        }
    }
    Ok(changed)
}

/// Rewrite one database, encrypted or plain, through `sqlcipher_export`.
///
/// In process: SQLCipher is compiled into this build, so there is no reason to
/// need its command-line tool installed as well. The new file is written
/// beside the old one and renamed over it only once it is whole, so an
/// interrupted run leaves the original untouched. The old WAL and shared
/// memory go with the file they belonged to.
#[cfg(feature = "db-encrypt")]
fn convert(path: &std::path::Path, key_hex: &str, to_encrypted: bool) -> Result<()> {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};

    let tmp = path.with_extension("db.converting");
    let _ = std::fs::remove_file(&tmp);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("no runtime to convert the database on")?;
    let outcome = rt.block_on(async {
        // Opening the source: keyed when it is the encrypted one.
        let mut opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(false);
        if !to_encrypted {
            opts = opts.pragma("key", pragma_value(key_hex));
        }
        let mut conn = opts.connect().await?;
        // The target: keyed when it is the one being written.
        let key = if to_encrypted { blob_literal(key_hex) } else { "''".to_string() };
        sqlx::raw_sql(&format!(
            "ATTACH DATABASE '{}' AS target KEY {key}",
            sql_path(&tmp)
        ))
        .execute(&mut conn)
        .await?;
        sqlx::raw_sql("SELECT sqlcipher_export('target')")
            .execute(&mut conn)
            .await?;
        sqlx::raw_sql("DETACH DATABASE target")
            .execute(&mut conn)
            .await?;
        conn.close().await?;
        Ok::<(), anyhow::Error>(())
    });
    if let Err(e) = outcome {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("convert {}", path.display()));
    }
    if !tmp.exists() {
        anyhow::bail!("sqlcipher_export wrote nothing for {}", path.display());
    }
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(not(feature = "db-encrypt"))]
fn convert(_path: &std::path::Path, _key_hex: &str, _to_encrypted: bool) -> Result<()> {
    anyhow::bail!("{}", why_not())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pragma_is_quoted_the_way_sqlcipher_wants_a_raw_key() {
        let v = pragma_value("00ff");
        assert_eq!(v, "\"x'00ff'\"");
        // sqlx writes `PRAGMA key = <value>;`, so the quoting has to be in the
        // value itself. ATTACH is the other way round: a value, not an
        // identifier, so no double quotes.
        assert!(v.starts_with('"') && v.ends_with('"'));
        assert_eq!(blob_literal("00ff"), "x'00ff'");
    }

    #[test]
    fn an_apostrophe_in_a_path_cannot_end_the_sql_literal() {
        let p = std::path::Path::new("/home/o'brien/photos.db");
        assert_eq!(sql_path(p), "/home/o''brien/photos.db");
    }

    #[test]
    fn a_plaintext_header_is_recognised_and_anything_else_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.db");
        std::fs::write(&plain, b"SQLite format 3\0rest of the file").unwrap();
        assert!(!file_is_encrypted(&plain));

        let enc = dir.path().join("enc.db");
        std::fs::write(&enc, b"\x91\x2a\x77\x01random ciphertext bytes").unwrap();
        assert!(file_is_encrypted(&enc));

        // Not there, and too short to have a header: both count as plaintext,
        // because both are about to be created.
        assert!(!file_is_encrypted(&dir.path().join("missing.db")));
        let stub = dir.path().join("stub.db");
        std::fs::write(&stub, b"short").unwrap();
        assert!(!file_is_encrypted(&stub));
    }

    #[test]
    fn a_build_without_sqlcipher_says_so_and_converts_nothing() {
        if supported() {
            return;
        }
        assert_eq!(apply_pending().unwrap(), 0);
        assert!(convert(std::path::Path::new("/tmp/x.db"), "00", true).is_err());
        assert!(matches!(state(), State::Unsupported));
    }

    #[test]
    fn the_sections_it_touches_are_databases_and_never_the_settings_file() {
        for p in section_paths() {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.ends_with(".db"), "{name}");
            assert_ne!(name, "settings.json");
        }
    }
}
