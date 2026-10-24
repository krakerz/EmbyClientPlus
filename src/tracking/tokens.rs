//! Service tokens in the keyring (`crate::auth`, entry `tracking:<service>`).

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::ServiceId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access: String,
    #[serde(default)]
    pub refresh: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: Option<i64>,
    pub username: String,
    /// Kitsu's numeric user id (library entries are filtered by it).
    #[serde(default)]
    pub user_id: Option<String>,
}

impl Tokens {
    pub fn expires_within(&self, seconds: i64) -> bool {
        self.expires_at.is_some_and(|at| at - now() <= seconds)
    }
}

fn key(service: ServiceId) -> String {
    format!("tracking:{}", service.key())
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `expires_in` (seconds from now) as an absolute time.
pub fn expires_at(expires_in: Option<i64>) -> Option<i64> {
    expires_in.map(|seconds| now() + seconds)
}

pub fn load(service: ServiceId) -> Option<Tokens> {
    let raw = match crate::auth::get_token(&key(service)) {
        Ok(raw) => raw?,
        Err(e) => {
            tracing::warn!("{} tokens unreadable: {e:#}", service.label());
            return None;
        }
    };
    serde_json::from_str(&raw)
        .inspect_err(|e| tracing::warn!("{} tokens unparsable: {e}", service.label()))
        .ok()
}

pub fn save(service: ServiceId, tokens: &Tokens) -> Result<()> {
    crate::auth::store_token(&key(service), &serde_json::to_string(tokens)?)
}

pub fn delete(service: ServiceId) -> Result<()> {
    crate::auth::delete_token(&key(service))
}
