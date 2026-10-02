use anyhow::Result;

use super::EmbyClient;
use super::models::{ProgressRequest, StoppedRequest};

impl EmbyClient {
    pub async fn report_playing(&self, request: &ProgressRequest) -> Result<()> {
        self.post_empty("/emby/Sessions/Playing", request).await
    }

    pub async fn report_progress(&self, request: &ProgressRequest) -> Result<()> {
        self.post_empty("/emby/Sessions/Playing/Progress", request)
            .await
    }

    pub async fn report_stopped(&self, request: &StoppedRequest) -> Result<()> {
        self.post_empty("/emby/Sessions/Playing/Stopped", request)
            .await
    }
}
