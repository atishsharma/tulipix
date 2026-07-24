//! Host health check for the Servers modal.
//!
//! The host list is a plain text box: paste five addresses and there is no way
//! to tell which of them actually answers, or which is fastest from where you
//! are. This times a real signed request against each one, so the modal can say
//! so and offer to reorder by what works.

use std::time::{Duration, Instant};

use super::{StreamClient, StreamError};

/// A host that takes longer than this is treated as down — the tab falls
/// through to the next host well before this, so a slower answer is useless.
const TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostHealth {
    pub host: String,
    /// Answered, and issued a session token.
    pub ok: bool,
    /// Round trip in milliseconds; only meaningful when `ok`.
    pub ms: u64,
    /// Plain-language outcome for the modal's row.
    pub note: String,
}

impl HostHealth {
    /// "142 ms" / "refused a session" — what the row shows on the right.
    pub fn label(&self) -> String {
        if self.ok { format!("{} ms", self.ms) } else { self.note.clone() }
    }
}

/// Time one host end to end: build a client over it alone and warm it up, which
/// is exactly what the tab does on its first request.
pub async fn probe(host: &str) -> HostHealth {
    let started = Instant::now();
    let mut out = HostHealth { host: host.to_string(), ..Default::default() };

    let client = match StreamClient::new(vec![host.to_string()]) {
        Ok(c) => c,
        Err(_) => {
            out.note = "not a usable address".into();
            return out;
        }
    };
    match tokio::time::timeout(TIMEOUT, client.init()).await {
        Ok(Ok(())) => {
            out.ok = true;
            out.ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            out.note = "answered".into();
        }
        Ok(Err(StreamError::MissingToken)) => out.note = "refused a session".into(),
        Ok(Err(StreamError::ApiStatus(s))) => out.note = format!("returned {s}"),
        Ok(Err(_)) => out.note = "no answer".into(),
        Err(_) => out.note = "timed out".into(),
    }
    out
}

/// Probe every host at once. Order matches the input, so the modal can line the
/// results up against the text box.
pub async fn probe_all(hosts: &[String]) -> Vec<HostHealth> {
    let tasks: Vec<_> = hosts
        .iter()
        .map(|h| {
            let h = h.clone();
            tokio::spawn(async move { probe(&h).await })
        })
        .collect();
    let mut out = Vec::with_capacity(tasks.len());
    for (idx, t) in tasks.into_iter().enumerate() {
        out.push(t.await.unwrap_or_else(|_| HostHealth {
            host: hosts.get(idx).cloned().unwrap_or_default(),
            note: "check failed".into(),
            ..Default::default()
        }));
    }
    out
}

/// The same hosts, fastest working one first. Dead hosts keep their relative
/// order at the back — they are still the user's list, not ours to drop.
pub fn rank(results: &[HostHealth]) -> Vec<String> {
    let mut idx: Vec<usize> = (0..results.len()).collect();
    idx.sort_by_key(|&i| (!results[i].ok, if results[i].ok { results[i].ms } else { 0 }, i as u64));
    idx.into_iter().map(|i| results[i].host.clone()).collect()
}

/// One line for the modal: "4 of 6 answered · fastest api5 (142 ms)".
pub fn summary(results: &[HostHealth]) -> String {
    let ok = results.iter().filter(|r| r.ok).count();
    if results.is_empty() {
        return String::new();
    }
    let fastest = results
        .iter()
        .filter(|r| r.ok)
        .min_by_key(|r| r.ms)
        .map(|r| format!(" · fastest {} ({} ms)", short_host(&r.host), r.ms))
        .unwrap_or_default();
    format!("{ok} of {} answered{fastest}", results.len())
}

/// "https://api5.aoneroom.com" → "api5.aoneroom.com".
fn short_host(host: &str) -> &str {
    host.split_once("://").map(|(_, rest)| rest).unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(host: &str, ok: bool, ms: u64) -> HostHealth {
        HostHealth { host: host.into(), ok, ms, note: if ok { "answered".into() } else { "no answer".into() } }
    }

    #[test]
    fn rank_puts_the_fastest_working_host_first_and_keeps_dead_ones() {
        let results = vec![
            h("https://slow", true, 900),
            h("https://dead", false, 0),
            h("https://fast", true, 120),
            h("https://alsodead", false, 0),
        ];
        assert_eq!(
            rank(&results),
            vec!["https://fast", "https://slow", "https://dead", "https://alsodead"],
            "dead hosts stay, in their original order"
        );
        assert!(rank(&[]).is_empty());
    }

    #[test]
    fn summary_counts_and_names_the_fastest() {
        let results = vec![h("https://a.test", true, 300), h("https://b.test", false, 0)];
        assert_eq!(summary(&results), "1 of 2 answered · fastest a.test (300 ms)");
        // Nothing answered: no "fastest" clause to show.
        assert_eq!(summary(&[h("https://a.test", false, 0)]), "0 of 1 answered");
        assert_eq!(summary(&[]), "");
    }

    #[test]
    fn label_reads_as_a_time_or_a_reason() {
        assert_eq!(h("x", true, 142).label(), "142 ms");
        assert_eq!(h("x", false, 0).label(), "no answer");
    }
}
