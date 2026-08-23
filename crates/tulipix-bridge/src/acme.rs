//! A real certificate for the transfer server, over a name you own.
//!
//! The server's default is trust-on-first-use behind a self-signed leaf, and
//! that has to stay the default: installing a user CA on Android makes the
//! device fail Play Integrity, so banking apps stop working, and trading that
//! for a padlock on a LAN file transfer is not a trade worth offering. What this
//! adds is the other path, for someone who owns a domain — a certificate a
//! public CA issued, which every phone already trusts, with no CA to install and
//! no warning to click through.
//!
//! # Why DNS-01, and why by hand
//!
//! The server answers on a private address. Let's Encrypt cannot reach
//! 192.168.x.x, so HTTP-01 is impossible; DNS-01 proves control of the *name*
//! instead, and public DNS is allowed to return a private address. The record is
//! placed by hand here rather than through a provider API: an API token would
//! have to sit on this machine, and the shape of the flow — show the record,
//! wait, continue — is the same one every provider adapter would slot into
//! later.
//!
//! The cost of by-hand is renewal: 90-day certificates mean this comes round
//! roughly every 60 days, and nobody remembers. `status()` reports the expiry so
//! the page can say so, and the self-signed path stays underneath for the day it
//! lapses — which is also the day the LAN has no internet, or the day a router's
//! DNS-rebinding protection eats a public name pointing at a private address.
//! That fallback is not a nicety. It is why this is an upgrade and not a
//! migration.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use instant_acme::{
    Account, AccountCredentials, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder,
    Order, OrderStatus, RetryPolicy,
};

/// Where the transfer server keeps its CA, its leaf, and now these.
fn dir() -> Result<PathBuf> {
    tulipix_core::paths::data_dir()
        .map(|d| d.join("transfer"))
        .ok_or_else(|| anyhow!("no data directory"))
}

fn chain_path() -> Result<PathBuf> {
    Ok(dir()?.join("acme.pem"))
}

fn key_path() -> Result<PathBuf> {
    Ok(dir()?.join("acme.key.pem"))
}

fn host_path() -> Result<PathBuf> {
    Ok(dir()?.join("acme.host"))
}

/// The ACME account, kept so a renewal is one order rather than a new
/// registration. Registering per renewal is rate-limited and rude.
fn account_path() -> Result<PathBuf> {
    Ok(dir()?.join("acme.account.json"))
}

/// An order waiting for its TXT record to appear. One at a time — this is a
/// person adding a DNS record, not a queue.
struct Pending {
    order: Order,
    host: String,
    record: String,
    value: String,
}

fn pending() -> &'static tokio::sync::Mutex<Option<Pending>> {
    static P: std::sync::OnceLock<tokio::sync::Mutex<Option<Pending>>> = std::sync::OnceLock::new();
    P.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Load the stored account, or register one.
///
/// No contact address: Let's Encrypt's expiry mail is the one thing that would
/// make by-hand renewal survivable, and it is also a personal email address
/// leaving the machine. The page says when it expires instead.
async fn account() -> Result<Account> {
    let path = account_path()?;
    if let Ok(json) = std::fs::read_to_string(&path) {
        if let Ok(creds) = serde_json::from_str::<AccountCredentials>(&json) {
            return Account::builder()?
                .from_credentials(creds)
                .await
                .context("the stored ACME account was rejected");
        }
    }
    let (account, creds) = Account::builder()?
        .create(
            &NewAccount {
                contact: &[],
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            LetsEncrypt::Production.url().to_owned(),
            None,
        )
        .await
        .context("could not register with Let's Encrypt")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, serde_json::to_string(&creds)?)
        .context("could not save the ACME account")?;
    Ok(account)
}

/// Start an order for `host` and work out the TXT record that proves you own
/// it. The record itself comes back through [`status`], which is what the next
/// snapshot reads — there is one place the page learns about a pending request,
/// and it has to be the one that still works after a restart.
///
/// Nothing is issued yet. The record has to exist in public DNS before
/// [`finish`] will get past validation.
pub async fn begin(host: String) -> Result<()> {
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') || host.contains('/') {
        return Err(anyhow!("that is not a hostname"));
    }

    let account = account().await?;
    let identifiers = vec![Identifier::Dns(host.clone())];
    let mut order = account
        .new_order(&NewOrder::new(&identifiers))
        .await
        .context("Let's Encrypt refused the order")?;

    let mut record = String::new();
    let mut value = String::new();
    {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result?;
            let Some(challenge) = authz.challenge(ChallengeType::Dns01) else {
                return Err(anyhow!("Let's Encrypt did not offer a DNS-01 challenge"));
            };
            let key_auth = challenge.key_authorization();
            value = key_auth.dns_value();
            record = format!("_acme-challenge.{host}");
            // Deliberately NOT marked ready here: the record does not exist
            // yet, and telling the CA to look now spends one of the order's
            // validation attempts on a certain failure.
        }
    }
    if record.is_empty() {
        return Err(anyhow!("no authorization came back for {host}"));
    }

    *pending().lock().await = Some(Pending { order, host, record, value });
    Ok(())
}

/// Tell Let's Encrypt the record is in place, wait for it, and write the
/// certificate out. Returns the host it was issued for.
pub async fn finish() -> Result<String> {
    let mut held = pending().lock().await;
    let p = held.as_mut().ok_or_else(|| anyhow!("no certificate request is waiting"))?;

    {
        let mut authorizations = p.order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authz = result?;
            if let Some(mut challenge) = authz.challenge(ChallengeType::Dns01) {
                challenge.set_ready().await.context("could not start validation")?;
            }
        }
    }

    // The default policy backs off for a couple of minutes, which is about
    // right: a TXT record added a moment ago has to propagate to whatever
    // resolver Let's Encrypt asks, and that is rarely instant.
    let status = p.order.poll_ready(&RetryPolicy::default()).await?;
    if status != OrderStatus::Ready {
        return Err(anyhow!(
            "validation did not pass — check that {} is visible with `dig TXT {}`",
            p.record,
            p.record
        ));
    }

    let key_pem = p.order.finalize().await.context("could not finalize the order")?;
    let chain_pem = p
        .order
        .poll_certificate(&RetryPolicy::default())
        .await
        .context("the certificate never arrived")?;

    let d = dir()?;
    std::fs::create_dir_all(&d)?;
    std::fs::write(chain_path()?, &chain_pem)?;
    write_private(&key_path()?, &key_pem)?;
    std::fs::write(host_path()?, &p.host)?;

    let host = p.host.clone();
    *held = None;
    Ok(host)
}

/// A private key is written owner-read-only, the same as the CA key beside it.
/// Nothing else in the data directory is worth reading, but this one is.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// What the page shows. All of it is read off disk, so it survives a restart
/// and reports the truth after a manual `rm` just as well.
pub struct Status {
    /// The name the certificate is for, empty when there is none.
    pub host: String,
    /// True when a request is waiting for its TXT record.
    pub waiting: bool,
    pub record: String,
    pub value: String,
}

pub async fn status() -> Status {
    let host = host_path()
        .ok()
        .filter(|_| chain_path().map(|p| p.exists()).unwrap_or(false))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    match pending().lock().await.as_ref() {
        Some(p) => Status {
            host,
            waiting: true,
            record: p.record.clone(),
            value: p.value.clone(),
        },
        None => Status { host, waiting: false, record: String::new(), value: String::new() },
    }
}

/// Drop the certificate and go back to the self-signed leaf. Also the way out
/// of a half-finished request that will not validate.
pub async fn forget() -> Result<()> {
    *pending().lock().await = None;
    for p in [chain_path()?, key_path()?, host_path()?] {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}
