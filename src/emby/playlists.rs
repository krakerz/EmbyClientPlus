//! Emby playlists: listing, creating and editing.

use anyhow::Result;
use serde::Deserialize;

use super::EmbyClient;
use super::browse::{FIELDS, encode};
use super::models::{BaseItem, QueryResult};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Created {
    id: String,
}

impl EmbyClient {
    /// The user's audio playlists, by name.
    pub async fn playlists(&self, user_id: &str) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Users/{user_id}/Items?IncludeItemTypes=Playlist&Recursive=true&SortBy=SortName&Fields={FIELDS},ChildCount"
            ))
            .await?;
        // Video playlists live in the same list; music only wants its own.
        Ok(result
            .items
            .into_iter()
            .filter(|p| p.media_type.as_deref() != Some("Video"))
            .collect())
    }

    /// A playlist's entries in order, each carrying its `PlaylistItemId`.
    pub async fn playlist_items(&self, user_id: &str, playlist_id: &str) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Playlists/{playlist_id}/Items?UserId={user_id}&Fields={FIELDS}"
            ))
            .await?;
        Ok(result.items)
    }

    /// Creates an audio playlist holding `item_ids`; returns its id.
    pub async fn create_playlist(&self, name: &str, item_ids: &[&str]) -> Result<String> {
        let created: Created = self
            .post(
                &format!(
                    "/emby/Playlists?Name={}&Ids={}&MediaType=Audio",
                    encode(name),
                    item_ids.join(",")
                ),
                &serde_json::json!({}),
            )
            .await?;
        Ok(created.id)
    }

    pub async fn add_to_playlist(
        &self,
        user_id: &str,
        playlist_id: &str,
        item_ids: &[&str],
    ) -> Result<()> {
        self.post_empty(
            &format!(
                "/emby/Playlists/{playlist_id}/Items?Ids={}&UserId={user_id}",
                item_ids.join(",")
            ),
            &serde_json::json!({}),
        )
        .await
    }

    /// Removes entries by their `PlaylistItemId`.
    pub async fn remove_from_playlist(&self, playlist_id: &str, entry_ids: &[&str]) -> Result<()> {
        self.post_empty(
            &format!(
                "/emby/Playlists/{playlist_id}/Items/Delete?EntryIds={}",
                entry_ids.join(",")
            ),
            &serde_json::json!({}),
        )
        .await
    }

    /// Moves the entry `entry_id` to `index` (0-based).
    pub async fn move_in_playlist(
        &self,
        playlist_id: &str,
        entry_id: &str,
        index: usize,
    ) -> Result<()> {
        self.post_empty(
            &format!("/emby/Playlists/{playlist_id}/Items/{entry_id}/Move/{index}"),
            &serde_json::json!({}),
        )
        .await
    }

    /// Emby's Instant Mix: songs like `item_id` (a song, album or artist).
    pub async fn instant_mix(
        &self,
        user_id: &str,
        item_id: &str,
        limit: usize,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Items/{item_id}/InstantMix?UserId={user_id}&Limit={limit}&Fields={FIELDS}"
            ))
            .await?;
        Ok(result.items)
    }
}
