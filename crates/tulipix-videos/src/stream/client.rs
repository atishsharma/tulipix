//! Signed HTTP client for the Stream backend.
//!
//! Ported from MovieBox-Tui (`src/v3/client.rs`, MIT OR Apache-2.0,
//! https://github.com/mesamirh/MovieBox-Tui). Changes from upstream: the host
//! pool is a runtime `Vec<String>` supplied by the caller rather than a
//! `const` array, so the Stream tab's Hosts editor can replace it without a
//! rebuild. `DEFAULT_HOSTS` keeps the upstream list as the first-run value.

use super::crypto::build_signed_headers;
use reqwest::Response;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// First-run host pool. Users can edit, reorder, or replace these entirely
/// from the Stream tab; `stream::hosts::load()` falls back here when unset.
pub const DEFAULT_HOSTS: &[&str] = &[
    "https://api6.aoneroom.com",
    "https://api5.aoneroom.com",
    "https://api4.aoneroom.com",
    "https://api4sg.aoneroom.com",
    "https://api3.aoneroom.com",
    "https://api6sg.aoneroom.com",
    "https://api.inmoviebox.com",
];

/// Statuses that mean "this host is unhappy, try the next one" rather than
/// "the request itself is bad".
const RETRY_STATUS_CODES: &[u16] = &[403, 406, 407, 429, 500, 502, 503, 504];

#[derive(thiserror::Error, Debug)]
pub enum StreamError {
    #[error("network error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("server returned status {0}")]
    ApiStatus(u16),
    #[error("no configured host answered")]
    HostsExhausted,
    #[error("no hosts configured")]
    NoHosts,
    #[error("bad response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("server did not issue a session token")]
    MissingToken,
}

#[derive(Clone)]
pub struct StreamClient {
    client: reqwest::Client,
    hosts: Arc<Vec<String>>,
    runtime_token: Arc<RwLock<Option<String>>>,
    active_base_idx: Arc<RwLock<usize>>,
    user_agent: String,
    client_info: String,
    spoofed_ip: String,
}

impl StreamClient {
    /// Build a client over `hosts`, tried in order and remembered by index so a
    /// working host stays first for subsequent calls. Blank entries are dropped.
    pub fn new(hosts: Vec<String>) -> Result<Self, StreamError> {
        let hosts: Vec<String> = hosts
            .into_iter()
            .map(|h| h.trim().trim_end_matches('/').to_string())
            .filter(|h| !h.is_empty())
            .collect();
        if hosts.is_empty() {
            return Err(StreamError::NoHosts);
        }
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        let (user_agent, client_info) = super::crypto::generate_client_info_and_ua();
        Ok(Self {
            client,
            hosts: Arc::new(hosts),
            runtime_token: Arc::new(RwLock::new(None)),
            active_base_idx: Arc::new(RwLock::new(0)),
            user_agent,
            client_info,
            spoofed_ip: super::crypto::random_spoofed_ip(),
        })
    }

    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    /// Host that answered most recently — shown in the Stream tab status line.
    pub async fn active_host(&self) -> String {
        let idx = *self.active_base_idx.read().await;
        self.hosts.get(idx).cloned().unwrap_or_default()
    }

    /// One warm-up call so the server hands back the session token that every
    /// later request is signed with.
    pub async fn init(&self) -> Result<(), StreamError> {
        let _ = self
            .get("/wefeed-mobile-bff/tab-operating?page=1&tabId=0&version=")
            .await?;
        if self.runtime_token.read().await.is_none() {
            return Err(StreamError::MissingToken);
        }
        Ok(())
    }

    /// The session token arrives in an `x-user` response header, not the body.
    async fn absorb_x_user(&self, headers: &reqwest::header::HeaderMap) {
        let Some(raw) = headers.get("x-user").and_then(|v| v.to_str().ok()) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let Some(token) = json.get("token").and_then(|t| t.as_str()) else {
            return;
        };
        if !token.is_empty() {
            *self.runtime_token.write().await = Some(token.to_string());
        }
    }

    pub async fn get(&self, path_and_query: &str) -> Result<Value, StreamError> {
        self.request("GET", path_and_query, None).await
    }

    pub async fn post(&self, path_and_query: &str, body: &Value) -> Result<Value, StreamError> {
        let body_str = serde_json::to_string(body)?;
        self.request("POST", path_and_query, Some(&body_str)).await
    }

    /// Try each host in turn starting from the last one that worked. Retryable
    /// statuses and transport errors fall through to the next host; the first
    /// clean response wins and pins the index.
    async fn request(
        &self,
        method: &str,
        path_and_query: &str,
        body: Option<&str>,
    ) -> Result<Value, StreamError> {
        let start_idx = *self.active_base_idx.read().await;
        let mut last_status: Option<u16> = None;

        for i in 0..self.hosts.len() {
            let idx = (start_idx + i) % self.hosts.len();
            let url = format!("{}{}", self.hosts[idx], path_and_query);

            let token = self.runtime_token.read().await.clone();
            let headers = build_signed_headers(
                method,
                &url,
                body,
                token.as_deref(),
                &self.user_agent,
                &self.client_info,
                &self.spoofed_ip,
            );

            let mut builder = match method {
                "POST" => self.client.post(&url),
                _ => self.client.get(&url),
            };
            builder = builder.headers(headers);
            if let Some(b) = body {
                builder = builder.body(b.to_string());
            }

            let Ok(resp) = builder.send().await else { continue };
            self.absorb_x_user(resp.headers()).await;
            let status = resp.status().as_u16();
            if RETRY_STATUS_CODES.contains(&status) {
                last_status = Some(status);
                continue;
            }
            match self.parse_response(resp).await {
                Ok(val) => {
                    *self.active_base_idx.write().await = idx;
                    return Ok(val);
                }
                Err(StreamError::ApiStatus(s)) => {
                    last_status = Some(s);
                    continue;
                }
                Err(_) => continue,
            }
        }
        // Report the real status when we saw one — "403 from every host" is a
        // far more actionable message than a bare "exhausted".
        match last_status {
            Some(s) => Err(StreamError::ApiStatus(s)),
            None => Err(StreamError::HostsExhausted),
        }
    }

    /// Responses wrap the payload in `{"data": …}`; unwrap it when present.
    async fn parse_response(&self, resp: Response) -> Result<Value, StreamError> {
        let status = resp.status();
        if !status.is_success() {
            return Err(StreamError::ApiStatus(status.as_u16()));
        }
        let body_val: Value = serde_json::from_str(&resp.text().await?)?;
        Ok(match body_val.get("data") {
            Some(data) => data.clone(),
            None => body_val,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_drops_blank_hosts() {
        let c = StreamClient::new(vec![
            "  https://a.test/  ".into(),
            "".into(),
            "   ".into(),
            "https://b.test".into(),
        ])
        .unwrap();
        assert_eq!(c.hosts(), ["https://a.test", "https://b.test"]);
    }

    #[test]
    fn empty_host_list_is_rejected() {
        assert!(matches!(
            StreamClient::new(vec!["".into(), "  ".into()]),
            Err(StreamError::NoHosts)
        ));
        assert!(matches!(StreamClient::new(vec![]), Err(StreamError::NoHosts)));
    }

    #[test]
    fn default_hosts_are_all_absolute_urls() {
        for h in DEFAULT_HOSTS {
            assert!(h.starts_with("https://"), "{h}");
            assert!(!h.ends_with('/'), "{h}");
        }
    }
}
