use anyhow::Result;

use super::EmbyClient;
use super::models::{AuthenticateByNameRequest, AuthenticateByNameResponse};

impl EmbyClient {
    /// Logs in and keeps the returned token on this client.
    pub async fn authenticate_by_name(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticateByNameResponse> {
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
