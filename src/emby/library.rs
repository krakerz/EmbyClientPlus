use anyhow::Result;

use super::EmbyClient;
use super::models::{ItemSummary, ItemsResponse};

impl EmbyClient {
    /// Every episode in a series, ordered by season then episode number —
    /// used to locate the currently-playing episode's neighbors for the
    /// previous/next-episode overlay buttons, and (via the matched entry
    /// itself) its title/chapters. Requesting `Fields=Chapters` here means
    /// a single call covers both needs, no separate per-episode lookup.
    pub async fn get_series_episodes(
        &self,
        series_id: &str,
        user_id: &str,
    ) -> Result<Vec<ItemSummary>> {
        let response: ItemsResponse = self
            .get(&format!(
                "/emby/Shows/{series_id}/Episodes?UserId={user_id}&Fields=Chapters&SortBy=ParentIndexNumber,IndexNumber&SortOrder=Ascending"
            ))
            .await?;
        Ok(response.items)
    }

    /// A single item's summary (title + chapters) — the movie-or-no-
    /// series fallback when there's no episode list to pull this from.
    pub async fn get_item_summary(&self, user_id: &str, item_id: &str) -> Result<ItemSummary> {
        self.get(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields=Chapters"
        ))
        .await
    }
}
