use anyhow::Result;

use super::EmbyClient;
use super::models::{AuthenticateByNameRequest, AuthenticateByNameResponse};

impl EmbyClient {
    /// Fallback login path only — the primary path is reading the access
    /// token out of the embedded webview's `localStorage` after the user
    /// logs in through Emby's own web client (see `webview_bridge`).
    #[allow(dead_code)] // fallback path, not yet wired up anywhere — see doc comment above
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
}
