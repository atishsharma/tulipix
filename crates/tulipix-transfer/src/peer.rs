//! Talking to another tulipix: the pairing handshake, and the client half of
//! the routes the phone already uses.
//!
//! This is the only module in the crate that makes outbound requests. Every
//! other module either serves them or plans them.

use std::path::Path;

use anyhow::{anyhow, Result};
use tokio::io::AsyncWriteExt;

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

/// One file another tulipix is offering, as its `/api/files` reports it.
#[derive(Clone, Debug)]
pub struct RemoteFile {
    pub id: u64,
    pub name: String,
    pub bytes: u64,
}

/// A connection to another tulipix we already hold a token for.
pub struct Peer {
    base: String,
    token: String,
    http: reqwest::Client,
}

/// Ask a machine to pair. Returns the digits to show, and their fingerprint.
pub async fn pair(base: &str, our_fingerprint: &str) -> Result<(String, String)> {
    let res = reqwest::Client::new()
        .post(format!("{base}/api/peer/pair"))
        .json(&serde_json::json!({ "fingerprint": our_fingerprint }))
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(anyhow!("pairing refused: {}", res.status()));
    }
    let body: serde_json::Value = res.json().await?;
    let code = body["code"]
        .as_str()
        .ok_or_else(|| anyhow!("no code in reply"))?
        .to_string();
    let theirs = body["fingerprint"]
        .as_str()
        .ok_or_else(|| anyhow!("no fingerprint in reply"))?
        .to_string();
    Ok((code, theirs))
}

/// A person confirmed the digits. Collect the token.
pub async fn confirm(base: &str, their_fingerprint: &str) -> Result<String> {
    let res = reqwest::Client::new()
        .post(format!("{base}/api/peer/pair/confirm"))
        .json(&serde_json::json!({ "fingerprint": their_fingerprint }))
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(anyhow!("pairing not confirmed: {}", res.status()));
    }
    let body: serde_json::Value = res.json().await?;
    Ok(body["token"]
        .as_str()
        .ok_or_else(|| anyhow!("no token in reply"))?
        .to_string())
}

/// A client that carries the device token on every request, which is exactly
/// what the browser's cookie does.
pub fn connect(base: &str, token: &str) -> Result<Peer> {
    Ok(Peer {
        base: base.trim_end_matches('/').to_string(),
        token: token.to_string(),
        http: reqwest::Client::builder().build()?,
    })
}

impl Peer {
    /// The cookie is named `tx`, not `t` — see `COOKIE` in `server.rs`.
    fn cookie(&self) -> String {
        format!("tx={}", self.token)
    }

    /// What this peer is offering right now.
    pub async fn list(&self) -> Result<Vec<RemoteFile>> {
        let res = self
            .http
            .get(format!("{}/api/files", self.base))
            .header("cookie", self.cookie())
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!("listing refused: {}", res.status()));
        }
        let body: serde_json::Value = res.json().await?;
        let rows = body
            .as_array()
            .or_else(|| body["files"].as_array())
            .ok_or_else(|| anyhow!("unexpected listing shape"))?;
        rows.iter()
            .map(|r| {
                Some(RemoteFile {
                    id: r["id"].as_u64()?,
                    name: r["name"].as_str()?.to_string(),
                    bytes: r["bytes"].as_u64().unwrap_or(0),
                })
            })
            .collect::<Option<Vec<_>>>()
            // Our own server wrote these rows, so one we cannot read means a
            // version mismatch worth surfacing — not a file worth hiding.
            .ok_or_else(|| anyhow!("unreadable file listing"))
    }

    /// Stream one file to disk. `from_byte` resumes a partial take through the
    /// same `Range` handling that lets a phone survive a screen lock.
    pub async fn download(&self, id: u64, to: &Path, from_byte: u64) -> Result<u64> {
        let mut req = self
            .http
            .get(format!("{}/dl/{id}", self.base))
            .header("cookie", self.cookie());
        if from_byte > 0 {
            req = req.header("range", format!("bytes={from_byte}-"));
        }
        let mut res = req.send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("download refused: {}", res.status()));
        }

        // The server does not always honour a Range: `range::parse` refuses an
        // offset at or past the end, and the handler then sends the whole file
        // with a 200. Appending that to what is already on disk would duplicate
        // the prefix and report success. So the response decides the mode, not
        // the request: 206 continues, 200 starts again.
        let resuming = res.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let mut file = if resuming {
            tokio::fs::OpenOptions::new().append(true).open(to).await?
        } else {
            tokio::fs::File::create(to).await?
        };
        let mut written = if resuming { from_byte } else { 0 };
        while let Some(chunk) = res.chunk().await? {
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        file.flush().await?;
        Ok(written)
    }
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

    #[tokio::test]
    async fn a_peer_lists_and_downloads_what_another_offers() {
        use std::io::Write;

        // The machine doing the offering.
        let dir = tempfile::tempdir().unwrap();
        let offered = dir.path().join("contract.pdf");
        let mut f = std::fs::File::create(&offered).unwrap();
        f.write_all(b"hello from the other machine").unwrap();
        drop(f);

        let inbox = tempfile::tempdir().unwrap();
        let run = crate::server::start(crate::server::Config {
            inbox: inbox.path().to_path_buf(),
            bind: "127.0.0.1:0".into(),
            pool: None,
            tls: false,
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", run.port);
        // `server::lock` is module-private, so this reaches for the same
        // `Mutex` through the plain std API instead of that helper.
        run.state.tray.lock().unwrap().add(&offered);

        // The machine doing the taking.
        let (code, theirs) = pair(&base, "our:own:fingerprint").await.unwrap();
        assert_eq!(code.len(), 6);
        assert!(run.approve_pairing(), "the person at the far machine says the digits match");
        let token = confirm(&base, &theirs).await.unwrap();

        let p = connect(&base, &token).unwrap();
        let files = p.list().await.unwrap();
        assert_eq!(files.len(), 1, "one file is on offer");
        assert_eq!(files[0].name, "contract.pdf");

        let landing = inbox.path().join("taken.pdf");
        let got = p.download(files[0].id, &landing, 0).await.unwrap();
        assert_eq!(got, 28, "every byte arrived");
        assert_eq!(
            std::fs::read(&landing).unwrap(),
            b"hello from the other machine"
        );

        run.stop().await;
    }

    #[tokio::test]
    async fn a_resume_past_the_end_starts_again_instead_of_duplicating() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let offered = dir.path().join("short.txt");
        let mut f = std::fs::File::create(&offered).unwrap();
        f.write_all(b"twelve bytes").unwrap();
        drop(f);

        let inbox = tempfile::tempdir().unwrap();
        let run = crate::server::start(crate::server::Config {
            inbox: inbox.path().to_path_buf(),
            bind: "127.0.0.1:0".into(),
            pool: None,
            tls: false,
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", run.port);
        run.state.tray.lock().unwrap().add(&offered);

        let (_code, theirs) = pair(&base, "our:fp").await.unwrap();
        assert!(run.approve_pairing());
        let token = confirm(&base, &theirs).await.unwrap();
        let p = connect(&base, &token).unwrap();
        let files = p.list().await.unwrap();

        // Ask to resume from beyond the end. The server answers 200 with the
        // whole file; the client must not append it to a file it already has.
        let landing = inbox.path().join("taken.txt");
        std::fs::write(&landing, b"twelve bytes").unwrap();
        let got = p.download(files[0].id, &landing, 99).await.unwrap();

        assert_eq!(got, 12, "the whole file, counted once");
        assert_eq!(
            std::fs::read(&landing).unwrap(),
            b"twelve bytes",
            "a rejected resume restarts the file rather than doubling it"
        );

        run.stop().await;
    }
}
