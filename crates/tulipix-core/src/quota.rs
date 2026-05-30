use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Default, Deserialize, Clone)]
pub struct CapabilitiesFile {
    #[serde(default)]
    pub tiers: HashMap<String, Vec<String>>,
    #[serde(default)]
    pub quota: HashMap<String, HashMap<String, i64>>,
}

impl CapabilitiesFile {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
    }

    /// -1 = unlimited, 0 = blocked, otherwise per-day limit.
    pub fn quota_for(&self, service: &str, tier: &str) -> Option<i64> {
        self.quota.get(service)?.get(tier).copied()
    }
}
