use anyhow::Result;

use super::EmbyClient;
use super::models::{AuthenticateByNameRequest, AuthenticateByNameResponse};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicSystemInfo {
    #[serde(default)]
    version: String,
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
            && !info.version.is_empty()
        {
            super::set_server_version(&info.version);
            let version = info.version.clone();
            let _ = crate::config::Settings::update(|s| s.server.version = version);
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
