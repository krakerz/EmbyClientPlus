use anyhow::Result;

use super::EmbyClient;
use super::models::{BaseItem, QueryResult};

fn hide_from_resume_path(user_id: &str, item_id: &str) -> String {
    format!("/emby/Users/{user_id}/Items/{item_id}/HideFromResume?Hide=true")
}

impl EmbyClient {
    /// Takes an item off the user's Continue Watching rows (its resume
    /// point stays, so it still resumes from where it was).
    pub async fn hide_from_resume(&self, user_id: &str, item_id: &str) -> Result<()> {
        self.post_empty(
            &hide_from_resume_path(user_id, item_id),
            &serde_json::json!({}),
        )
        .await
    }

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

    /// The episodes of a series that exist as files: Emby lists unaired and
    /// missing ones too (`LocationType` "Virtual"), which can't be downloaded.
    pub async fn downloadable_episodes(
        &self,
        series_id: &str,
        user_id: &str,
    ) -> Result<Vec<BaseItem>> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Located {
            #[serde(flatten)]
            item: BaseItem,
            location_type: Option<String>,
        }
        let result: QueryResult<Located> = self
            .get(&format!(
                "/emby/Shows/{series_id}/Episodes?UserId={user_id}&SortBy=ParentIndexNumber,IndexNumber&SortOrder=Ascending&Fields=Overview,PremiereDate,DateCreated"
            ))
            .await?;
        Ok(result
            .items
            .into_iter()
            .filter(|e| e.location_type.as_deref() != Some("Virtual"))
            .map(|e| e.item)
            .collect())
    }

    /// One item with its chapters (and intro/credits markers), for playback.
    pub async fn item_with_chapters(&self, user_id: &str, item_id: &str) -> Result<BaseItem> {
        self.get(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields=Chapters,Overview"
        ))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::hide_from_resume_path;

    #[test]
    fn hide_from_resume_sends_hide_as_a_query_parameter() {
        assert_eq!(
            hide_from_resume_path("u1", "i2"),
            "/emby/Users/u1/Items/i2/HideFromResume?Hide=true"
        );
    }
}
