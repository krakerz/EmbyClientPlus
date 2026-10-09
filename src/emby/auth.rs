use anyhow::Result;

use super::EmbyClient;
use super::models::{AuthenticateByNameRequest, AuthenticateByNameResponse};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    #[serde(default)]
    version: String,
}

/// Uses (and saves) the server's version for browser mode's identity.
fn remember_server_version(version: &str) {
    if version.is_empty() {
        return;
    }
    super::set_server_version(version);
    tracing::info!("server version {version}");
    let version = version.to_string();
    if let Err(e) = crate::config::Settings::update(|s| s.server.version = version) {
        tracing::warn!("couldn't save the server version: {e:#}");
    }
}

/// Asks the server at `base_url` for its version (no sign-in needed), at
/// startup: the saved one may be missing or out of date after an upgrade.
pub async fn learn_server_version(base_url: &str) {
    let url = format!("{}/emby/System/Info/Public", base_url.trim_end_matches('/'));
    let info = async {
        reqwest::Client::new()
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json::<PublicSystemInfo>()
            .await
    };
    match info.await {
        Ok(info) => remember_server_version(&info.version),
        Err(e) => tracing::info!("couldn't ask the server for its version: {e}"),
    }
}

impl EmbyClient {
    /// Logs in and keeps the returned token on this client.
    pub async fn authenticate_by_name(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticateByNameResponse> {
        // The token records the version sent at sign-in: in browser mode
        // that's the server's own, as Emby's web app sends.
        if let Ok(info) = self
            .get::<PublicSystemInfo>("/emby/System/Info/Public")
            .await
        {
            remember_server_version(&info.version);
        }
        let request = AuthenticateByNameRequest {
            username: username.to_string(),
            pw: password.to_string(),
        };
        let response: AuthenticateByNameResponse = self
            .post("/emby/Users/AuthenticateByName", &request)
            .await?;
        self.set_token(response.access_token.clone());
        Ok(response)
    }

    /// Ends this device's server session; best-effort, the token is
    /// forgotten locally either way.
    pub async fn logout(&self) -> Result<()> {
        self.post_empty("/emby/Sessions/Logout", &serde_json::json!({}))
            .await
    }
}
