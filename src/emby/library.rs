use anyhow::Result;

use super::EmbyClient;
use super::models::{BaseItem, QueryResult};

impl EmbyClient {
    /// Every episode of a series in watch order, for finding the previous
    /// and next episode around the one playing.
    pub async fn series_episodes(&self, series_id: &str, user_id: &str) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Shows/{series_id}/Episodes?UserId={user_id}&SortBy=ParentIndexNumber,IndexNumber&SortOrder=Ascending&Fields=Overview,PremiereDate,DateCreated"
            ))
            .await?;
        Ok(result.items)
    }

    /// One item with its chapters (and intro/credits markers), for playback.
    pub async fn item_with_chapters(&self, user_id: &str, item_id: &str) -> Result<BaseItem> {
        self.get(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields=Chapters,Overview"
        ))
        .await
    }
}
