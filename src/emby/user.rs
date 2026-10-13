//! The signed-in user's own account: their settings (Emby's
//! `UserConfiguration`), password and picture. Nothing here needs admin
//! rights.

use anyhow::{Context, Result};
use base64::Engine;

use super::EmbyClient;

impl EmbyClient {
    /// The user as Emby returns it (`UserDto`), kept whole so saving the
    /// configuration sends back every field, including ones this app
    /// doesn't know.
    pub async fn user(&self, user_id: &str) -> Result<serde_json::Value> {
        self.get(&format!("/emby/Users/{user_id}")).await
    }

    /// Replaces the user's configuration (Emby takes the whole object).
    pub async fn save_configuration(
        &self,
        user_id: &str,
        configuration: &serde_json::Value,
    ) -> Result<()> {
        self.post_empty(
            &format!("/emby/Users/{user_id}/Configuration"),
            configuration,
        )
        .await
    }

    pub async fn change_password(&self, user_id: &str, current: &str, new: &str) -> Result<()> {
        self.post_empty(
            &format!("/emby/Users/{user_id}/Password"),
            &serde_json::json!({ "Id": user_id, "CurrentPw": current, "NewPw": new }),
        )
        .await
    }

    /// Sets the user's picture; Emby wants it base64-encoded.
    pub async fn set_user_picture(&self, user_id: &str, bytes: &[u8], mime: &str) -> Result<()> {
        let path = format!("/emby/Users/{user_id}/Images/Primary");
        self.http
            .post(self.url(&path))
            .headers(self.headers()?)
            .header(reqwest::header::CONTENT_TYPE, mime)
            .body(base64::engine::general_purpose::STANDARD.encode(bytes))
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?
            .error_for_status()
            .with_context(|| format!("POST {path} returned an error status"))?;
        Ok(())
    }

    pub async fn delete_user_picture(&self, user_id: &str) -> Result<()> {
        self.delete(&format!("/emby/Users/{user_id}/Images/Primary"))
            .await
    }
}
