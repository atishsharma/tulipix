//! Encrypt or decrypt a file with a passphrase.
//!
//! `age` rather than a hand-rolled AES: the file format is specified, the
//! implementation is audited, and someone who encrypts a file here can open it
//! anywhere the `age` command exists. A passphrase goes through scrypt, so a
//! weak one costs an attacker real time rather than a dictionary pass.
//!
//! Streaming both ways. These are media files; reading one into memory to
//! encrypt it is how a four-gigabyte video becomes an out-of-memory crash.

use std::io::{BufReader, BufWriter, Read, Write};

use anyhow::{Context, Result, anyhow};

use age::secrecy::SecretString;

/// The extension an encrypted file gets, and the one `decrypt` strips.
pub const EXT: &str = "age";

/// Encrypt `input` to `output`. Returns the number of plaintext bytes read.
pub fn encrypt(
    input: &str,
    output: &str,
    passphrase: &str,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<u64> {
    if passphrase.is_empty() {
        return Err(anyhow!("a passphrase is required"));
    }
    let source = std::fs::File::open(input).with_context(|| format!("cannot read {input}"))?;
    let sink = std::fs::File::create(output).with_context(|| format!("cannot write {output}"))?;

    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(passphrase.to_owned()));
    let mut writer = encryptor.wrap_output(BufWriter::new(sink))?;
    let read = pump(&mut BufReader::new(source), &mut writer, progress)?;
    // `finish` writes the last chunk and the authentication tag. Dropping the
    // writer without it produces a file that looks complete and is not.
    writer.finish()?.flush()?;
    Ok(read)
}

/// Decrypt `input` to `output`. Returns the number of plaintext bytes written.
pub fn decrypt(
    input: &str,
    output: &str,
    passphrase: &str,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<u64> {
    if passphrase.is_empty() {
        return Err(anyhow!("a passphrase is required"));
    }
    let source = std::fs::File::open(input).with_context(|| format!("cannot read {input}"))?;
    let decryptor = age::Decryptor::new(BufReader::new(source))
        .map_err(|e| anyhow!("{input} is not an age file: {e}"))?;

    let identity = age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()));
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        // The library cannot tell a wrong passphrase from a corrupt file, and
        // neither can we — but one of those is overwhelmingly more likely, and
        // saying so is more use than repeating the library's wording.
        .map_err(|_| anyhow!("wrong passphrase, or the file is damaged"))?;

    let sink = std::fs::File::create(output).with_context(|| format!("cannot write {output}"))?;
    let mut sink = BufWriter::new(sink);
    let written = pump(&mut reader, &mut sink, progress)?;
    sink.flush()?;
    Ok(written)
}

/// The name an encrypted file takes, and the name it gives back.
///
/// `holiday.mkv` becomes `holiday.mkv.age`, and that goes back to
/// `holiday.mkv` — the extension is appended rather than replaced so the
/// original name survives the round trip whole.
pub fn encrypted_name(input: &str) -> String {
    format!("{input}.{EXT}")
}

pub fn decrypted_name(input: &str) -> String {
    input
        .strip_suffix(&format!(".{EXT}"))
        .map(str::to_string)
        // An .age file that was renamed still has to go somewhere.
        .unwrap_or_else(|| format!("{input}.out"))
}

/// Copy in 64 KiB chunks, reporting bytes as it goes. Returns early — without
/// an error — when `progress` says to stop, because a cancel is not a failure.
fn pump(
    from: &mut dyn Read,
    to: &mut dyn Write,
    progress: &mut dyn FnMut(u64) -> bool,
) -> Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        total += n as u64;
        if !progress(total) {
            break;
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_round_trip_gives_back_the_same_bytes() {
        let d = tempfile::tempdir().unwrap();
        let plain = d.path().join("secret.txt");
        let sealed = d.path().join("secret.txt.age");
        let opened = d.path().join("back.txt");
        std::fs::write(&plain, b"the account number is 12345").unwrap();

        let mut noop = |_: u64| true;
        encrypt(
            plain.to_str().unwrap(),
            sealed.to_str().unwrap(),
            "correct horse",
            &mut noop,
        )
        .unwrap();
        // The ciphertext must not simply be the plaintext with a header on it.
        let body = std::fs::read(&sealed).unwrap();
        assert!(!body.windows(5).any(|w| w == b"12345"));

        decrypt(
            sealed.to_str().unwrap(),
            opened.to_str().unwrap(),
            "correct horse",
            &mut noop,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&opened).unwrap(),
            b"the account number is 12345"
        );
    }

    #[test]
    fn the_wrong_passphrase_fails_rather_than_writing_rubbish() {
        let d = tempfile::tempdir().unwrap();
        let plain = d.path().join("a.txt");
        let sealed = d.path().join("a.txt.age");
        let opened = d.path().join("b.txt");
        std::fs::write(&plain, b"hello").unwrap();

        let mut noop = |_: u64| true;
        encrypt(
            plain.to_str().unwrap(),
            sealed.to_str().unwrap(),
            "right",
            &mut noop,
        )
        .unwrap();
        assert!(
            decrypt(
                sealed.to_str().unwrap(),
                opened.to_str().unwrap(),
                "wrong",
                &mut noop
            )
            .is_err()
        );
        // And it must not have created the output on the way to failing.
        assert!(!opened.exists());
    }

    #[test]
    fn names_survive_the_round_trip() {
        assert_eq!(encrypted_name("/v/holiday.mkv"), "/v/holiday.mkv.age");
        assert_eq!(decrypted_name("/v/holiday.mkv.age"), "/v/holiday.mkv");
        assert_eq!(decrypted_name("/v/mystery"), "/v/mystery.out");
    }

    #[test]
    fn an_empty_passphrase_is_refused_before_anything_is_opened() {
        let mut noop = |_: u64| true;
        assert!(encrypt("/nope", "/nope.age", "", &mut noop).is_err());
    }
}
