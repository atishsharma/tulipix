use anyhow::Result;
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    AppDefault,
    UserKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeySetting {
    pub service: String,
    pub source: KeySource,
    pub user_key_present: bool,
}

pub const SERVICES: &[&str] = &[
    "tmdb",
    "tvdb",
    "opensubtitles",
    "lrclib",
    "musicbrainz",
    "coverart",
    "libretranslate",
    "lastfm",
    "google_oauth",
    "github_oauth",
    "ytdlp_cookies",
];

fn entry(service: &str) -> Result<Entry> {
    if !SERVICES.contains(&service) {
        anyhow::bail!("unknown api service: {service}");
    }
    Ok(Entry::new(&format!("tulipix.api.{service}"), "default")?)
}

/// Runs a keyring call on a thread of its own.
///
/// The Linux store speaks to the Secret Service through zbus, which this tree
/// builds with its tokio feature, and zbus's blocking calls start a runtime of
/// their own and `block_on` it. On a thread already driving tokio -- any async
/// bridge call, which is where the TMDB and OpenSubtitles keys are read from --
/// that panics with "Cannot start a runtime from within a runtime", and with
/// `panic = "abort"` it closes the app. A plain thread has no runtime to
/// collide with. It is one D-Bus round trip, so the thread costs nothing that
/// matters; the entry is built on it too, since opening the store may already
/// reach the bus.
fn off_runtime<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(f)
        .join()
        .unwrap_or_else(|p| std::panic::resume_unwind(p))
}

/// Never written to settings.json — explicit guard.
pub fn assert_never_in_settings_json(key: &str) -> Result<()> {
    if key.starts_with("api.") || key.contains("token") || key.contains("secret") {
        anyhow::bail!("attempt to write secret to settings.json: {key}");
    }
    Ok(())
}

pub fn store(service: &str, value: &str) -> Result<()> {
    let (service, value) = (service.to_owned(), value.to_owned());
    off_runtime(move || {
        entry(&service)?.set_password(&value)?;
        Ok(())
    })
}

pub fn fetch(service: &str) -> Result<Option<String>> {
    let service = service.to_owned();
    off_runtime(move || match entry(&service)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    })
}

pub fn delete(service: &str) -> Result<()> {
    let service = service.to_owned();
    off_runtime(move || match entry(&service)?.delete_credential() {
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    })
}

#[derive(Debug, Clone, Copy)]
pub struct QuotaState {
    pub used: i64,
    pub limit: i64, // -1 unlimited, 0 blocked
}

impl QuotaState {
    pub fn remaining(&self) -> Option<i64> {
        if self.limit < 0 { None } else { Some((self.limit - self.used).max(0)) }
    }
    pub fn exhausted(&self) -> bool {
        self.limit == 0 || (self.limit > 0 && self.used >= self.limit)
    }
}

#[derive(Debug, Default)]
struct DailyCounter {
    day_epoch: u64,
    used: HashMap<String, i64>,
}

static COUNTER: OnceLock<RwLock<DailyCounter>> = OnceLock::new();
fn counter() -> &'static RwLock<DailyCounter> {
    COUNTER.get_or_init(|| RwLock::new(DailyCounter::default()))
}

fn current_day_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
        / 86_400
}

fn roll_if_new_day(c: &mut DailyCounter) {
    let today = current_day_epoch();
    if c.day_epoch != today {
        c.day_epoch = today;
        c.used.clear();
    }
}

/// Quota check for the current tier. App-default keys are rate-limited by tier;
/// user-supplied keys bypass the tier cap (caller passes `KeySource::UserKey`).
pub fn quota_state(service: &str, source: KeySource) -> QuotaState {
    if matches!(source, KeySource::UserKey) {
        return QuotaState { used: 0, limit: -1 };
    }
    let mut c = counter().write().unwrap();
    roll_if_new_day(&mut c);
    let used = *c.used.get(service).unwrap_or(&0);
    drop(c);
    let limit = crate::caps::quota_for_service(service).unwrap_or(0);
    QuotaState { used, limit }
}

type QuotaListener = Box<dyn Fn(&str, QuotaState) + Send + Sync>;
static EXCEEDED: OnceLock<RwLock<Vec<QuotaListener>>> = OnceLock::new();
fn exceeded_listeners() -> &'static RwLock<Vec<QuotaListener>> {
    EXCEEDED.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn on_quota_exceeded<F: Fn(&str, QuotaState) + Send + Sync + 'static>(f: F) {
    exceeded_listeners().write().unwrap().push(Box::new(f));
}

/// Account one unit of usage against the app-default quota. Returns `Err` if the
/// quota is exhausted; fires `on_quota_exceeded` callbacks (single per (service,day)).
pub fn account_usage(service: &str, source: KeySource) -> Result<QuotaState> {
    if matches!(source, KeySource::UserKey) {
        return Ok(QuotaState { used: 0, limit: -1 });
    }
    let mut c = counter().write().unwrap();
    roll_if_new_day(&mut c);
    let used = *c.used.get(service).unwrap_or(&0);
    let limit = crate::caps::quota_for_service(service).unwrap_or(0);
    let next_state = QuotaState { used: used + 1, limit };
    if next_state.exhausted() && limit >= 0 {
        drop(c);
        for cb in exceeded_listeners().read().unwrap().iter() {
            cb(service, next_state);
        }
        anyhow::bail!("quota exhausted for {service} (limit {limit})");
    }
    c.used.insert(service.to_string(), used + 1);
    Ok(next_state)
}

#[cfg(test)]
mod tests {
    use super::off_runtime;

    /// What zbus's blocking calls do under its tokio feature: a runtime of
    /// their own, blocked on. No D-Bus needed to reproduce the crash.
    fn nested_block_on() -> u8 {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async { 7 })
    }

    // The crash a video's TMDB key lookup hit: the keyring's blocking call on a
    // thread already driving tokio.
    #[tokio::test(flavor = "multi_thread")]
    #[should_panic(expected = "Cannot start a runtime from within a runtime")]
    async fn a_blocking_keyring_call_on_the_runtime_is_the_crash() {
        nested_block_on();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn off_runtime_takes_the_same_call_somewhere_it_can_block() {
        assert_eq!(off_runtime(nested_block_on), 7);
    }
}
