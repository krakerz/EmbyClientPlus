use anyhow::Result;

use super::EmbyClient;
use super::models::{AuthenticateByNameRequest, AuthenticateByNameResponse};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    #[serde(default)]
    version: String,
    #[serde(default)]
    id: String,
}

/// Uses (and saves) the server's version for browser mode's identity, and
/// its id for telling servers apart.
fn remember_server_info(info: PublicSystemInfo) {
    if info.version.is_empty() && info.id.is_empty() {
        return;
    }
    if !info.version.is_empty() {
        super::set_server_version(&info.version);
        tracing::info!("server version {}", info.version);
    }
    let result = crate::config::Settings::update(|s| {
        if !info.version.is_empty() {
            s.server.version = info.version;
        }
        if !info.id.is_empty() {
            s.server.server_id = Some(info.id);
        }
    });
    if let Err(e) = result {
        tracing::warn!("couldn't save the server's version and id: {e:#}");
    }
}

/// Asks the server at `base_url` for its version and id (no sign-in
/// needed), at startup: the saved ones may be missing or out of date after
/// an upgrade.
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
        Ok(info) => remember_server_info(info),
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
            remember_server_info(info);
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
