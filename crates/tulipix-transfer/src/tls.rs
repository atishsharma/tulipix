//! HTTPS with a certificate this app signs itself.
//!
//! The point of TLS here is not secrecy — the traffic is on the LAN and every
//! route is behind a PIN or a device token already. It is that a browser hands
//! out camera access, and a few other things worth having, only on a secure
//! origin. Over `http://192.168.x.x` the phone will not open its camera at all,
//! which is what kept the in-page QR scanner from working.
//!
//! # Trust on first use, not a certificate to install
//!
//! No browser silently accepts a certificate it cannot chain to a root it knows,
//! and no hostname changes that — `tulipix.local` included, and no public CA will
//! ever issue for a `.local` name anyway. There are two ways out, and the default
//! is the second one:
//!
//! 1. Install this app's root on the phone. Produces a real padlock, and used to
//!    be the advice on the gateway page. It is also a genuinely bad thing to ask
//!    of an Android phone: a user-installed CA makes the device fail Play
//!    Integrity's basic verdict, and banking and payment apps refuse to run on a
//!    phone carrying one. Trading someone's banking apps for a padlock on a LAN
//!    file transfer is not a trade worth offering, so it is now the opt-in path.
//! 2. Accept the warning once — SSH's model. The browser stores an exception
//!    pinned to this exact certificate, and from then on a *different*
//!    certificate on this address raises the warning again, which is the property
//!    that matters: not "was this signed by someone", but "is this the same
//!    machine I accepted before". The app shows the certificate's SHA-256
//!    fingerprint so the first acceptance can be checked rather than guessed.
//!
//! TOFU only means anything if the certificate is stable, so the leaf is
//! persisted beside the root and reused across runs — see [`load_or_create_leaf`].
//! A leaf regenerated per start would re-raise the warning on every restart and
//! train the one person who could notice a real change into clicking through it.
//!
//! # One port, two schemes
//!
//! The CA has to be fetchable *before* it is trusted, so serving it over the
//! same HTTPS the browser is complaining about would be a circle. Rather than
//! bind a second port and put two addresses in front of the user, the listener
//! peeks the first byte: a TLS handshake starts with 0x16, and anything else is
//! plaintext.
//!
//! The plaintext side is not a redirect to the TLS side — that was the bug. A
//! phone sent straight to https on its first visit meets the browser's
//! interstitial, and from behind an interstitial there is no way to reach the
//! page that explains it. So plaintext answers with the gateway
//! (`web/trust.html`), which probes `/api/ping` over TLS and crosses over by
//! itself once the handshake succeeds — immediately for a phone that already
//! trusts us, and the moment the install finishes for one that does not. The
//! address handed out by `TransferService::base_url` is `http` for exactly this
//! reason.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// The name advertised over mDNS and carried in the certificate. A phone that
/// resolves `.local` can use it; one that cannot still has the IP, which is in
/// the same certificate.
pub const HOST: &str = "tulipix.local";

/// How long the root is good for. Long, because re-installing it on every phone
/// in the house is the one part of this that cannot be automated.
const CA_YEARS: i32 = 10;
/// The leaf now outlives the process, so this is how long a phone can go between
/// warnings. Not stretched to the root's ten: a certificate that chains to a
/// locally installed root is exempt from the 398-day limit browsers enforce on
/// public ones, but only just, and there is no reason to sit on that edge.
const LEAF_YEARS: i32 = 2;

/// The CA, as it sits on disk between runs.
pub struct Authority {
    /// PEM, which is what a phone wants to download.
    pub ca_pem: String,
    /// DER, which is what Android's installer prefers.
    pub ca_der: Vec<u8>,
    key_pem: String,
}

fn ca_dir() -> Option<PathBuf> {
    tulipix_core::paths::data_dir().map(|d| d.join("transfer"))
}

/// Load the root from disk, or make one and write it there.
///
/// Regenerating per run would be a quiet disaster: every phone in the house
/// would have to re-install the root after every restart, and the old one would
/// sit in their trust stores forever. So a failure to persist is a failure to
/// use a CA at all — see [`Identity::load`], which falls back rather than
/// handing out a root nobody can keep.
pub fn load_or_create_ca(dir: &Path) -> anyhow::Result<Authority> {
    let cert_path = dir.join("ca.pem");
    let key_path = dir.join("ca.key.pem");

    if let (Ok(ca_pem), Ok(key_pem)) =
        (std::fs::read_to_string(&cert_path), std::fs::read_to_string(&key_path))
    {
        // Parse it back to prove it is usable before trusting the files; a
        // truncated write from a previous crash would otherwise fail later, at
        // handshake time, where there is nothing to be done about it.
        if rcgen::KeyPair::from_pem(&key_pem).is_ok() {
            if let Some(der) = pem_to_der(&ca_pem) {
                return Ok(Authority { ca_pem, ca_der: der, key_pem });
            }
        }
        tracing::warn!("transfer: the stored CA is unreadable, making a new one");
    }

    let key = rcgen::KeyPair::generate()?;
    let mut params = ca_params()?;
    params.not_after = rcgen::date_time_ymd(current_year() + CA_YEARS, 1, 1);
    let cert = params.self_signed(&key)?;

    let ca_pem = cert.pem();
    let key_pem = key.serialize_pem();

    std::fs::create_dir_all(dir)?;
    std::fs::write(&cert_path, &ca_pem)?;
    std::fs::write(&key_path, &key_pem)?;
    // The private half of a root CA. Anything that can read it can mint a
    // certificate for any name this machine's browsers trust.
    restrict(&key_path);

    Ok(Authority { ca_der: cert.der().to_vec(), ca_pem, key_pem })
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {
    // Windows inherits the user profile's ACL, which is already user-only.
}

/// The root's parameters, built the same way every time.
///
/// Rebuilt on load rather than parsed back out of the stored certificate:
/// rcgen can only read one back with its `x509-parser` feature, which is a whole
/// ASN.1 stack pulled in to recover three fields this function already knows.
/// Both callers come through here, so the issuer name on a leaf cannot drift
/// from the subject on the root — which is the only thing that has to agree for
/// a browser to build the chain.
fn ca_params() -> anyhow::Result<rcgen::CertificateParams> {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Constrained(0));
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    params.distinguished_name = dn("Tulipix Local CA");
    Ok(params)
}

fn dn(name: &str) -> rcgen::DistinguishedName {
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, name);
    dn.push(rcgen::DnType::OrganizationName, "Tulipix");
    dn
}

/// This year, near enough. An average Gregorian year is off by a day or so over
/// a decade, which does not matter for something used only to pick a validity
/// window — and getting it this way keeps `time` out of every signature here
/// when rcgen already owns that dependency.
fn current_year() -> i32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    1970 + (secs / 31_556_952) as i32
}

/// The leaf, as it sits on disk between runs.
pub struct Leaf {
    der: Vec<u8>,
    key_pem: String,
    /// Every name and address it is good for.
    names: Vec<String>,
}

/// Load the leaf from disk, or sign a new one and write it there.
///
/// The stored one is reused whenever it still covers every name the machine
/// answers on now, and the name set only ever grows — plugging in a second
/// adapter adds an address rather than replacing the certificate that every
/// phone in the house has already accepted.
///
/// Reuse is what makes trust-on-first-use mean anything. A browser's exception is
/// pinned to the certificate it was shown, so signing a fresh leaf each start
/// would put the warning back on every restart, and a warning that appears every
/// time is one nobody reads — including the time it means something.
pub fn load_or_create_leaf(dir: &Path, ca: &Authority, want: &[String]) -> anyhow::Result<Leaf> {
    let cert_path = dir.join("leaf.pem");
    let key_path = dir.join("leaf.key.pem");
    let names_path = dir.join("leaf.names");

    // The SAN list is kept beside the certificate rather than read back out of
    // it: recovering it would mean rcgen's `x509-parser` feature, a whole ASN.1
    // stack for a list this function wrote itself.
    let stored: Vec<String> = std::fs::read_to_string(&names_path)
        .map(|s| s.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();

    if want.iter().all(|n| stored.contains(n))
        && let (Ok(pem), Ok(key_pem)) =
            (std::fs::read_to_string(&cert_path), std::fs::read_to_string(&key_path))
        && rcgen::KeyPair::from_pem(&key_pem).is_ok()
        && let Some(der) = pem_to_der(&pem)
    {
        return Ok(Leaf { der, key_pem, names: stored });
    }

    let mut names = stored;
    names.extend(want.iter().cloned());
    names.sort();
    names.dedup();

    let ca_key = rcgen::KeyPair::from_pem(&ca.key_pem)?;
    let issuer = rcgen::Issuer::new(ca_params()?, ca_key);

    let mut params = rcgen::CertificateParams::new(names.clone())?;
    params.distinguished_name = dn(HOST);
    params.not_after = rcgen::date_time_ymd(current_year() + LEAF_YEARS, 1, 1);
    params.use_authority_key_identifier_extension = true;
    params.key_usages =
        vec![rcgen::KeyUsagePurpose::DigitalSignature, rcgen::KeyUsagePurpose::KeyEncipherment];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];

    let key = rcgen::KeyPair::generate()?;
    let cert = params.signed_by(&key, &issuer)?;
    let (pem, key_pem) = (cert.pem(), key.serialize_pem());

    std::fs::create_dir_all(dir)?;
    std::fs::write(&cert_path, &pem)?;
    std::fs::write(&key_path, &key_pem)?;
    std::fs::write(&names_path, names.join("\n"))?;
    restrict(&key_path);

    Ok(Leaf { der: cert.der().to_vec(), key_pem, names })
}

/// SHA-256 of a certificate, in the colon-separated hex every browser's
/// certificate viewer prints. What the desktop shows so the first acceptance on a
/// phone can be checked against something rather than taken on faith.
pub fn fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    let mut out = String::with_capacity(95);
    for (i, b) in digest.as_ref().iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// Everything the listener needs: the TLS config, the CA bytes to hand out, and
/// the leaf's fingerprint to show.
pub struct Identity {
    pub config: Arc<ServerConfig>,
    pub ca_pem: String,
    pub ca_der: Vec<u8>,
    /// SHA-256 of the leaf, colon-separated — the thing a phone is accepting.
    pub fingerprint: String,
    /// Every name and address the leaf is good for, for the log line.
    pub names: Vec<String>,
}

impl Identity {
    /// A leaf for `tulipix.local`, localhost and every address this machine
    /// answers on, signed by the persisted root and itself persisted.
    pub fn load(ips: &[IpAddr]) -> anyhow::Result<Self> {
        let dir = ca_dir().ok_or_else(|| anyhow::anyhow!("no data directory for the CA"))?;
        let ca = load_or_create_ca(&dir)?;

        let mut want: Vec<String> = vec![HOST.to_string(), "localhost".to_string()];
        for ip in ips {
            want.push(ip.to_string());
        }
        want.sort();
        want.dedup();

        let leaf = load_or_create_leaf(&dir, &ca, &want)?;
        let fingerprint = fingerprint(&leaf.der);
        let names = leaf.names;

        // The root goes out with the leaf so a phone that *did* install it gets a
        // padlock rather than a chain it cannot complete.
        let chain =
            vec![CertificateDer::from(leaf.der), CertificateDer::from(ca.ca_der.clone())];
        let leaf_key = rcgen::KeyPair::from_pem(&leaf.key_pem)?;
        let key = PrivateKeyDer::try_from(leaf_key.serialize_der())
            .map_err(|e| anyhow::anyhow!("leaf key rejected by rustls: {e}"))?;

        // Named provider rather than `ServerConfig::builder()`, which reads a
        // process-wide default and panics when none is installed — which is what
        // happens the moment anything else in the workspace pulls rustls in with
        // aws-lc-rs beside our ring. `panic = "abort"` in release turns that
        // into the app disappearing, so it is not a risk worth carrying for the
        // three words it saves.
        let config = ServerConfig::builder_with_provider(Arc::new(
            tokio_rustls::rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;

        // axum is built with `http1` only here. Saying so keeps a browser from
        // negotiating h2 over ALPN and then finding nothing that speaks it.
        let mut config = config;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];

        Ok(Self {
            config: Arc::new(config),
            ca_pem: ca.ca_pem,
            ca_der: ca.ca_der,
            fingerprint,
            names,
        })
    }
}

/// PEM body → DER. One certificate, no headers beyond the BEGIN/END lines,
/// which is what rcgen emits — this is not a general PEM reader.
fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END"))
        .collect();
    base64_decode(&body)
}

/// Standard base64, no padding tolerance beyond '='. Small enough to write that
/// pulling a crate in for it would cost more than it saves.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = TABLE.iter().position(|&t| t == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

// ── the listener ────────────────────────────────────────────────────────────

/// What a plaintext connection gets. Held behind an Arc so every connection
/// shares one copy of the certificate rather than cloning it per request.
pub struct Plain {
    pub ca_pem: String,
    pub ca_der: Vec<u8>,
    /// Printed on the gateway so the fingerprint the browser is about to show can
    /// be compared with one that came from somewhere else. Over plaintext it
    /// proves nothing on its own — but the desktop shows the same string, and
    /// that copy is the one worth reading.
    pub fingerprint: String,
}

impl Plain {
    /// A hand-written HTTP/1.1 reply. Three responses and no routing worth the
    /// name, so running a second axum stack — and a second port to put in front
    /// of the user — would be a lot of machinery for one static page and two
    /// file downloads.
    async fn serve(&self, mut stream: TcpStream) {
        let mut buf = [0u8; 2048];
        let Ok(n) = stream.read(&mut buf).await else { return };
        let head = String::from_utf8_lossy(&buf[..n]);
        let target = head.split_whitespace().nth(1).unwrap_or("/");
        let path = target.split('?').next().unwrap_or("/");

        let out = match path {
            "/tulipix-ca.pem" => raw(
                "200 OK",
                "application/x-pem-file",
                self.ca_pem.as_bytes(),
                Some("tulipix-ca.pem"),
            ),
            "/tulipix-ca.crt" => {
                raw("200 OK", "application/x-x509-ca-cert", &self.ca_der, Some("tulipix-ca.crt"))
            }
            // One command for a Linux desktop. The certificate is baked into the
            // script rather than fetched by it, so there is nothing to download
            // twice and nothing to get out of step.
            "/tulipix-ca-install.sh" => raw(
                "200 OK",
                "text/x-shellscript; charset=utf-8",
                install_script(&self.ca_pem).as_bytes(),
                Some("tulipix-ca-install.sh"),
            ),
            // The gateway asks for these, and the gateway is only ever served
            // over plaintext — so answering them here is not a nicety. Without
            // it the page loads with a broken image and no favicon, which is
            // exactly what a phone should not see from a page asking it to
            // trust something.
            "/logo.png" => raw("200 OK", "image/png", LOGO, None),
            // Everything else — "/" and "/trust" alike — is the gateway.
            //
            // Not a redirect. Sending a first-time phone straight to https is
            // what put the browser's interstitial in front of it, and a phone
            // looking at an interstitial has no way to reach the page that
            // explains the interstitial. So the plaintext side answers with the
            // gateway, which upgrades itself once it can: the page probes the
            // TLS port and only then moves across, carrying the query string —
            // including the QR's `?k=` — with it.
            _ => raw(
                "200 OK",
                "text/html; charset=utf-8",
                gateway(&self.fingerprint).as_bytes(),
                None,
            ),
        };
        let _ = stream.write_all(&out).await;
        let _ = stream.shutdown().await;
    }
}

fn raw(status: &str, mime: &str, body: &[u8], download: Option<&str>) -> Vec<u8> {
    let mut head = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {mime}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    if let Some(name) = download {
        head.push_str(&format!("Content-Disposition: attachment; filename=\"{name}\"\r\n"));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// A TCP listener that sorts TLS from plaintext and only hands the former up to
/// axum. Implements [`axum::serve::Listener`], so the router above it is the
/// same one a plain `TcpListener` would have carried.
pub struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    plain: Arc<Plain>,
}

impl TlsListener {
    pub fn new(tcp: TcpListener, identity: &Identity) -> Self {
        Self {
            tcp,
            acceptor: TlsAcceptor::from(identity.config.clone()),
            plain: Arc::new(Plain {
                ca_pem: identity.ca_pem.clone(),
                ca_der: identity.ca_der.clone(),
                fingerprint: identity.fingerprint.clone(),
            }),
        }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, addr) = match self.tcp.accept().await {
                Ok(pair) => pair,
                // Per-connection errors are usually a resource limit; a tight
                // retry loop would turn one into a busy wait.
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            };

            // 0x16 is a TLS handshake record. Peeked rather than read, so the
            // byte is still there for the handshake that follows.
            let mut first = [0u8; 1];
            match stream.peek(&mut first).await {
                Ok(1) if first[0] == 0x16 => {}
                Ok(_) => {
                    let plain = self.plain.clone();
                    tokio::spawn(async move { plain.serve(stream).await });
                    continue;
                }
                Err(_) => continue,
            }

            match self.acceptor.accept(stream).await {
                Ok(tls) => return (tls, addr),
                // A failed handshake is the browser refusing the certificate.
                // Nothing to answer with — the interstitial is already up.
                Err(_) => continue,
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

/// The page a phone lands on the first time, reachable over plain HTTP so there
/// is no certificate warning standing between it and the explanation of the
/// certificate warning.
const TRUST_PAGE: &str = include_str!("web/trust.html");

/// The gateway with this run's fingerprint written into it.
///
/// A `replace` on a 12 KB string per first visit, against a template engine or a
/// second fetch the page would have to make. The phone hits this once.
pub fn gateway(fingerprint: &str) -> String {
    TRUST_PAGE.replace("{{FINGERPRINT}}", fingerprint)
}

/// The app mark, for the gateway's `<img>` and favicon.
const LOGO: &[u8] = include_bytes!("web/logo.png");

/// A one-command install for a Linux desktop.
///
/// Linux is the awkward one: browsers there do not read the system trust store.
/// Chrome, Chromium and Edge each read an NSS database at `~/.pki/nssdb`, and
/// every Firefox profile carries its own `cert9.db`. All of them are owned by
/// the user, so none of this needs root — which is the difference between a
/// paragraph of instructions and one line. This is the half of `mkcert -install`
/// that matters here; the certificate itself we already generate.
///
/// The system store is offered at the end but not attempted: it needs root, and
/// it only buys curl and friends, not the browser that prompted this.
fn install_script(ca_pem: &str) -> String {
    format!(
        r#"#!/bin/sh
# Trust the Tulipix Local CA on this machine.
#
# Installs into the NSS databases that browsers on Linux actually read. No root
# required. Safe to re-run: an existing copy is removed first.
set -eu

NAME="Tulipix Local CA"
PEM="$(mktemp)"
trap 'rm -f "$PEM"' EXIT
cat > "$PEM" <<'TULIPIX_CA_PEM'
{ca_pem}TULIPIX_CA_PEM

if ! command -v certutil >/dev/null 2>&1; then
  echo "certutil is missing. Install it, then run this again:"
  echo "  Debian/Ubuntu   sudo apt install libnss3-tools"
  echo "  Fedora          sudo dnf install nss-tools"
  echo "  Arch            sudo pacman -S nss"
  exit 1
fi

added=0
add_to() {{
  db="$1"
  certutil -d "sql:$db" -D -n "$NAME" >/dev/null 2>&1 || true
  if certutil -d "sql:$db" -A -t "C,," -n "$NAME" -i "$PEM" >/dev/null 2>&1; then
    echo "  trusted in $db"
    added=$((added + 1))
  fi
}}

# Chrome, Chromium, Edge, Brave, and anything else built on NSS.
mkdir -p "$HOME/.pki/nssdb"
[ -f "$HOME/.pki/nssdb/cert9.db" ] || certutil -d "sql:$HOME/.pki/nssdb" -N --empty-password >/dev/null 2>&1 || true
add_to "$HOME/.pki/nssdb"

# Every Firefox profile, including the Snap and Flatpak layouts.
for prof in   "$HOME"/.mozilla/firefox/*/   "$HOME"/snap/firefox/common/.mozilla/firefox/*/   "$HOME"/.var/app/org.mozilla.firefox/.mozilla/firefox/*/
do
  [ -f "$prof/cert9.db" ] || continue
  add_to "$prof"
done

if [ "$added" -eq 0 ]; then
  echo "Nothing was updated. Is a browser profile in an unusual place?"
  exit 1
fi

echo
echo "Done — restart the browser for it to notice."
echo
echo "For curl, wget and other command-line tools, the system store needs root:"
echo "  sudo cp tulipix-ca.crt /usr/local/share/ca-certificates/tulipix-ca.crt"
echo "  sudo update-ca-certificates       # Debian, Ubuntu"
echo "  sudo trust anchor tulipix-ca.crt  # Fedora, Arch"
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_what_rcgen_emits() {
        // A PEM body is the DER, base64'd, wrapped at 64 columns.
        let der: Vec<u8> = (0u8..=255).collect();
        let mut b64 = String::new();
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in der.chunks(3) {
            let n = chunk.len();
            let b = (chunk[0] as u32) << 16
                | (*chunk.get(1).unwrap_or(&0) as u32) << 8
                | (*chunk.get(2).unwrap_or(&0) as u32);
            for i in 0..4 {
                if i <= n {
                    b64.push(T[((b >> (18 - i * 6)) & 0x3f) as usize] as char);
                } else {
                    b64.push('=');
                }
            }
        }
        assert_eq!(base64_decode(&b64).as_deref(), Some(&der[..]));
    }

    #[test]
    fn a_pem_certificate_yields_its_der() {
        let key = rcgen::KeyPair::generate().unwrap();
        let params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        let cert = params.self_signed(&key).unwrap();
        assert_eq!(pem_to_der(&cert.pem()).as_deref(), Some(cert.der().as_ref()));
    }

    #[test]
    fn the_ca_is_reused_rather_than_reminted() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_ca(dir.path()).unwrap();
        let again = load_or_create_ca(dir.path()).unwrap();
        // A new root on every start would mean re-installing it on every phone
        // after every restart, which is the whole thing this is avoiding.
        assert_eq!(first.ca_pem, again.ca_pem, "the CA was regenerated");
        assert_eq!(first.ca_der, again.ca_der);
    }

    #[test]
    fn a_corrupt_ca_on_disk_is_replaced_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_or_create_ca(dir.path()).unwrap();
        std::fs::write(dir.path().join("ca.key.pem"), "not a key").unwrap();
        let second = load_or_create_ca(dir.path()).unwrap();
        assert_ne!(first.ca_pem, second.ca_pem, "the unreadable CA was kept");
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_leaf_survives_a_restart_so_a_phones_exception_does_too() {
        let dir = tempfile::tempdir().unwrap();
        let ca = load_or_create_ca(dir.path()).unwrap();
        let want = names(&["tulipix.local", "192.168.1.5"]);

        let first = load_or_create_leaf(dir.path(), &ca, &want).unwrap();
        let again = load_or_create_leaf(dir.path(), &ca, &want).unwrap();

        // The whole of trust-on-first-use rests on this: a browser's exception is
        // pinned to the certificate it was shown, so a different one here is the
        // warning coming back on every restart.
        assert_eq!(first.der, again.der, "the leaf was reminted");
        assert_eq!(fingerprint(&first.der), fingerprint(&again.der));
    }

    #[test]
    fn a_new_address_is_added_to_the_leaf_rather_than_replacing_the_old_ones() {
        let dir = tempfile::tempdir().unwrap();
        let ca = load_or_create_ca(dir.path()).unwrap();

        let first = load_or_create_leaf(dir.path(), &ca, &names(&["192.168.1.5"])).unwrap();
        let second = load_or_create_leaf(dir.path(), &ca, &names(&["10.0.0.9"])).unwrap();

        assert_ne!(first.der, second.der, "a name it did not cover was ignored");
        // Plugging in a second adapter must not invalidate the certificate every
        // phone on the first one has already accepted.
        assert!(second.names.contains(&"192.168.1.5".to_string()));
        assert!(second.names.contains(&"10.0.0.9".to_string()));

        // And once it covers everything, it settles again.
        let third = load_or_create_leaf(dir.path(), &ca, &names(&["192.168.1.5"])).unwrap();
        assert_eq!(second.der, third.der);
    }

    #[test]
    fn a_fingerprint_reads_like_the_one_a_browser_shows() {
        // 32 bytes as colon-separated uppercase hex — what every certificate
        // viewer prints, so the two can be compared without transcription.
        let fp = fingerprint(b"anything");
        assert_eq!(fp.len(), 95);
        assert_eq!(fp.split(':').count(), 32);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit() || c == ':'));
        assert!(!fp.chars().any(|c| c.is_ascii_lowercase()));
    }
}
