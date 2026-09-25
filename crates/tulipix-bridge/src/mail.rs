//! Newsletters from the user's own mailbox: one folder, read over IMAP.
//!
//! The deck wanted an email-to-feed address, which needs a mail server that
//! is not this computer. This is the local shape of the same thing: the user
//! points their newsletters at a folder in the mail they already have, and
//! Feeds reads that folder. Read-only — the folder is opened with EXAMINE and
//! messages fetched with BODY.PEEK, so nothing is marked read, moved or
//! deleted on the server. The password lives in the system keychain.
//!
//! IMAP is spoken by hand: five commands over TLS, which is less than any
//! client crate would bring. MIME is not — `mail-parser` reads the message.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::NaiveDate;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::feeds::Item;

/// Where the password is kept, in the keychain `api_keys` uses.
pub const KEYCHAIN: &str = "feeds_mail";

pub struct Account {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub folder: String,
}

type Conn = BufReader<tokio_rustls::client::TlsStream<TcpStream>>;

async fn connect(host: &str, port: u16) -> Result<Conn> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // Named, not the process default: two providers are compiled in, and
    // rustls refuses to guess between them.
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("{host} did not answer"))?
        .with_context(|| format!("could not reach {host}:{port}"))?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| anyhow!("{host} is not a server name"))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tcp)
        .await
        .with_context(|| format!("{host} would not talk TLS on port {port}"))?;
    Ok(BufReader::new(tls))
}

/// One response line, with the literals (`{n}` then n bytes) it carried.
struct Line {
    text: String,
    literals: Vec<Vec<u8>>,
}

/// `{123}` (or `{123+}`) at the end of a line: a literal of that many bytes.
fn literal_len(s: &str) -> Option<usize> {
    let s = s.strip_suffix('}')?;
    let at = s.rfind('{')?;
    s[at + 1..].trim_end_matches('+').parse().ok()
}

async fn read_line(c: &mut Conn) -> Result<Line> {
    let mut line = Line { text: String::new(), literals: Vec::new() };
    loop {
        let mut buf = Vec::new();
        let n = tokio::time::timeout(Duration::from_secs(60), c.read_until(b'\n', &mut buf))
            .await
            .map_err(|_| anyhow!("the mail server stopped answering"))??;
        if n == 0 {
            bail!("the mail server hung up");
        }
        let text = String::from_utf8_lossy(&buf);
        let text = text.trim_end_matches(['\r', '\n']);
        line.text.push_str(text);
        let Some(len) = literal_len(text) else { return Ok(line) };
        // A newsletter is not 50 MB; a server claiming one is broken.
        if len > 50 << 20 {
            bail!("the mail server sent a {len}-byte message");
        }
        let mut data = vec![0u8; len];
        c.read_exact(&mut data).await?;
        line.literals.push(data);
    }
}

async fn command(c: &mut Conn, tag: &str, cmd: &str) -> Result<Vec<Line>> {
    let w = c.get_mut();
    w.write_all(format!("{tag} {cmd}\r\n").as_bytes()).await?;
    w.flush().await?;
    let mut out = Vec::new();
    loop {
        let l = read_line(c).await?;
        if let Some(rest) = l.text.strip_prefix(tag).and_then(|r| r.strip_prefix(' ')) {
            if rest.starts_with("OK") {
                return Ok(out);
            }
            let why = rest.trim_start_matches("NO").trim_start_matches("BAD").trim();
            bail!("{}", if why.is_empty() { "the mail server said no" } else { why });
        }
        out.push(l);
    }
}

/// An IMAP quoted string.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The raw messages in the folder since `since`, the newest `limit` of them.
/// `limit` 0 signs in and opens the folder only: the test a new account gets.
pub async fn fetch(a: &Account, since: NaiveDate, limit: usize) -> Result<Vec<Vec<u8>>> {
    let mut c = connect(&a.host, a.port).await?;
    read_line(&mut c).await?; // the greeting
    command(&mut c, "a1", &format!("LOGIN {} {}", quote(&a.user), quote(&a.password)))
        .await
        .map_err(|e| anyhow!("could not sign in: {e}"))?;
    command(&mut c, "a2", &format!("EXAMINE {}", quote(&a.folder)))
        .await
        .map_err(|e| anyhow!("could not open the folder {}: {e}", a.folder))?;
    let mut out = Vec::new();
    if limit > 0 {
        // IMAP dates are English whatever the locale: 1-Sep-2026.
        let found = command(&mut c, "a3", &format!("UID SEARCH SINCE {}", since.format("%-d-%b-%Y"))).await?;
        let mut uids: Vec<u32> = found
            .iter()
            .filter_map(|l| l.text.strip_prefix("* SEARCH"))
            .flat_map(|rest| rest.split_whitespace().filter_map(|u| u.parse().ok()))
            .collect();
        uids.sort_unstable();
        let skip = uids.len().saturating_sub(limit);
        for (i, uid) in uids[skip..].iter().enumerate() {
            let lines = command(&mut c, &format!("f{i}"), &format!("UID FETCH {uid} BODY.PEEK[]")).await?;
            if let Some(raw) = lines.into_iter().flat_map(|l| l.literals).max_by_key(Vec::len) {
                out.push(raw);
            }
        }
    }
    command(&mut c, "z", "LOGOUT").await.ok();
    Ok(out)
}

/// One newsletter: who sent it, and the issue as a feed item.
pub struct Letter {
    pub address: String,
    pub name: String,
    pub item: Item,
}

pub fn letter(raw: &[u8]) -> Option<Letter> {
    let m = mail_parser::MessageParser::default().parse(raw)?;
    let from = m.from()?.first()?;
    let address = from.address()?.trim().to_lowercase();
    let name = from.name().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| address.clone());
    // The plain part when it has some substance: newsletter HTML is tables,
    // tracking pixels and buttons, and the text part is usually the letter.
    let text = m.body_text(0).map(|t| t.into_owned()).filter(|t| t.split_whitespace().count() > 80);
    let html = match text {
        Some(t) => {
            let esc = t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            format!("<p>{}</p>", esc.replace("\r\n", "\n").split("\n\n").collect::<Vec<_>>().join("</p><p>"))
        }
        None => m.body_html(0).map(|h| h.into_owned())?,
    };
    let guid = m
        .message_id()
        .map(str::to_string)
        .unwrap_or_else(|| crate::papers::sha(raw));
    Some(Letter {
        item: Item {
            guid,
            url: String::new(),
            title: m.subject().unwrap_or_default().trim().to_string(),
            author: name.clone(),
            published: m.date().map(|d| d.to_timestamp()).unwrap_or(0),
            image: String::new(),
            html,
        },
        address,
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_quotes() {
        assert_eq!(literal_len("* 1 FETCH (UID 7 BODY[] {1234}"), Some(1234));
        assert_eq!(literal_len("* 1 FETCH (UID 7 BODY[] {12+}"), Some(12));
        assert_eq!(literal_len("a1 OK done"), None);
        assert_eq!(quote(r#"pa"ss\word"#), r#""pa\"ss\\word""#);
    }

    #[test]
    fn a_newsletter_becomes_an_item() {
        let body = "word ".repeat(100);
        let raw = format!(
            "From: Margin Notes <hello@margin.example>\r\nTo: me@example.com\r\nSubject: Platforms, ten years on\r\n\
             Message-ID: <abc@margin.example>\r\nDate: Sat, 19 Sep 2026 07:00:00 +0000\r\n\
             Content-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n\r\nSecond paragraph.\r\n"
        );
        let l = letter(raw.as_bytes()).unwrap();
        assert_eq!(l.address, "hello@margin.example");
        assert_eq!(l.name, "Margin Notes");
        assert_eq!(l.item.title, "Platforms, ten years on");
        assert_eq!(l.item.guid, "abc@margin.example");
        assert_eq!(l.item.published, 1_789_801_200);
        assert!(l.item.html.contains("</p><p>Second paragraph."));
    }
}
