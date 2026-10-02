use anyhow::Result;

use super::EmbyClient;
use super::models::{BaseItem, QueryResult, Recommendation};

/// Extra fields every browse request asks for, on top of Emby's defaults.
const FIELDS: &str =
    "Overview,ProductionYear,OfficialRating,CommunityRating,PrimaryImageAspectRatio";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Primary,
    Backdrop,
    Thumb,
}

impl ImageKind {
    fn path_segment(self) -> &'static str {
        match self {
            ImageKind::Primary => "Primary",
            ImageKind::Backdrop => "Backdrop",
            ImageKind::Thumb => "Thumb",
        }
    }
}

/// An image to fetch: which item owns it, and the tag that versions it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub item_id: String,
    pub kind: ImageKind,
    pub tag: String,
}

/// Parameters for the generic `/Users/{id}/Items` listing, shared by the
/// library grid and search.
#[derive(Debug, Clone, Default)]
pub struct ItemQuery {
    pub parent_id: Option<String>,
    pub include_types: Option<&'static str>,
    pub search_term: Option<String>,
    pub recursive: bool,
    /// Emby `SortBy` value. Unset means `SortName`, except for searches,
    /// which then keep Emby's relevance order.
    pub sort_by: Option<&'static str>,
    pub descending: bool,
    /// Emby `Filters` values, e.g. `IsUnplayed`, `IsFavorite`.
    pub filters: Vec<&'static str>,
    pub genre_id: Option<String>,
    pub tag_id: Option<String>,
    pub start: usize,
    pub limit: usize,
}

impl ItemQuery {
    fn to_query_string(&self) -> String {
        let mut params = vec![("Fields", FIELDS.to_string())];
        let sort_by = match (self.sort_by, &self.search_term) {
            (Some(sort_by), _) => Some(sort_by),
            (None, Some(_)) => None,
            (None, None) => Some("SortName"),
        };
        if let Some(sort_by) = sort_by {
            params.push(("SortBy", sort_by.to_string()));
            let order = if self.descending {
                "Descending"
            } else {
                "Ascending"
            };
            params.push(("SortOrder", order.to_string()));
        }
        params.extend([
            ("StartIndex", self.start.to_string()),
            ("Limit", self.limit.to_string()),
            ("Recursive", self.recursive.to_string()),
            ("EnableTotalRecordCount", "true".to_string()),
        ]);
        if let Some(parent_id) = &self.parent_id {
            params.push(("ParentId", parent_id.clone()));
        }
        if let Some(types) = self.include_types {
            params.push(("IncludeItemTypes", types.to_string()));
        }
        if let Some(term) = &self.search_term {
            params.push(("SearchTerm", term.clone()));
        }
        if !self.filters.is_empty() {
            params.push(("Filters", self.filters.join(",")));
        }
        if let Some(genre_id) = &self.genre_id {
            params.push(("GenreIds", genre_id.clone()));
        }
        if let Some(tag_id) = &self.tag_id {
            params.push(("TagIds", tag_id.clone()));
        }
        params
            .iter()
            .map(|(key, value)| format!("{key}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    }
}

impl EmbyClient {
    /// The user's top-level libraries.
    pub async fn views(&self, user_id: &str) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> =
            self.get(&format!("/emby/Users/{user_id}/Views")).await?;
        Ok(result.items)
    }

    /// Partially watched videos, most recent first, optionally within one
    /// library.
    pub async fn resume(
        &self,
        user_id: &str,
        parent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Users/{user_id}/Items/Resume?Limit={limit}&MediaTypes=Video&Recursive=true&Fields={FIELDS}{}",
                parent_param(parent_id)
            ))
            .await?;
        Ok(result.items)
    }

    /// The next unwatched episode of each series in progress, optionally
    /// within one library.
    pub async fn next_up(
        &self,
        user_id: &str,
        parent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Shows/NextUp?UserId={user_id}&Limit={limit}&Fields={FIELDS}{}",
                parent_param(parent_id)
            ))
            .await?;
        Ok(result.items)
    }

    /// Genres used in a library.
    pub async fn genres(
        &self,
        user_id: &str,
        parent_id: &str,
        include_types: &str,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Genres?UserId={user_id}&ParentId={parent_id}&IncludeItemTypes={include_types}&Recursive=true&SortBy=SortName"
            ))
            .await?;
        Ok(result.items)
    }

    /// Tags used in a library.
    pub async fn tags(
        &self,
        user_id: &str,
        parent_id: &str,
        include_types: &str,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Tags?UserId={user_id}&ParentId={parent_id}&IncludeItemTypes={include_types}&Recursive=true&SortBy=SortName"
            ))
            .await?;
        Ok(result.items)
    }

    /// "Because you watched X"-style rows for a movie library.
    pub async fn movie_recommendations(
        &self,
        user_id: &str,
        parent_id: &str,
        categories: usize,
        items_per_category: usize,
    ) -> Result<Vec<Recommendation>> {
        self.get(&format!(
            "/emby/Movies/Recommendations?UserId={user_id}&ParentId={parent_id}&CategoryLimit={categories}&ItemLimit={items_per_category}&Fields={FIELDS}"
        ))
        .await
    }

    /// Recently added items in one library; episodes are grouped under
    /// their series, as Emby's own home screen does.
    pub async fn latest(
        &self,
        user_id: &str,
        parent_id: &str,
        limit: usize,
    ) -> Result<Vec<BaseItem>> {
        // This endpoint returns a bare array rather than a QueryResult.
        self.get(&format!(
            "/emby/Users/{user_id}/Items/Latest?ParentId={parent_id}&Limit={limit}&GroupItems=true&Fields={FIELDS}"
        ))
        .await
    }

    pub async fn items(&self, user_id: &str, query: &ItemQuery) -> Result<QueryResult<BaseItem>> {
        self.get(&format!(
            "/emby/Users/{user_id}/Items?{}",
            query.to_query_string()
        ))
        .await
    }

    pub async fn seasons(&self, series_id: &str, user_id: &str) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Shows/{series_id}/Seasons?UserId={user_id}&Fields={FIELDS}"
            ))
            .await?;
        Ok(result.items)
    }

    pub async fn episodes(
        &self,
        series_id: &str,
        season_id: &str,
        user_id: &str,
    ) -> Result<Vec<BaseItem>> {
        let result: QueryResult<BaseItem> = self
            .get(&format!(
                "/emby/Shows/{series_id}/Episodes?SeasonId={season_id}&UserId={user_id}&Fields={FIELDS}"
            ))
            .await?;
        Ok(result.items)
    }

    pub async fn item(&self, user_id: &str, item_id: &str) -> Result<BaseItem> {
        self.get(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields={FIELDS}"
        ))
        .await
    }

    /// Server-relative image path, resized server-side to `max_width`.
    pub fn image_path(image: &ImageRef, max_width: u32) -> String {
        format!(
            "/emby/Items/{}/Images/{}?maxWidth={max_width}&tag={}&quality=90",
            image.item_id,
            image.kind.path_segment(),
            encode(&image.tag)
        )
    }
}

impl BaseItem {
    /// Portrait poster. Episodes' own Primary is a 16:9 still, so they use
    /// the series poster first; everything else uses its own.
    pub fn poster(&self) -> Option<ImageRef> {
        let own = self
            .image_tags
            .get("Primary")
            .map(|tag| image(&self.id, ImageKind::Primary, tag));
        let series = match (&self.series_id, &self.series_primary_image_tag) {
            (Some(series_id), Some(tag)) => Some(image(series_id, ImageKind::Primary, tag)),
            _ => None,
        };
        if self.item_type == "Episode" {
            series.or(own)
        } else {
            own.or(series)
        }
    }

    /// Wide fan art: the item's own, else its parent's (episodes/seasons).
    pub fn backdrop(&self) -> Option<ImageRef> {
        if let Some(tag) = self.backdrop_image_tags.first() {
            return Some(image(&self.id, ImageKind::Backdrop, tag));
        }
        match (
            &self.parent_backdrop_item_id,
            self.parent_backdrop_image_tags.first(),
        ) {
            (Some(parent_id), Some(tag)) => Some(image(parent_id, ImageKind::Backdrop, tag)),
            _ => None,
        }
    }

    /// 16:9 card art. Episodes use their own still (their Primary image);
    /// everything else prefers a Thumb, then falls back to a backdrop.
    pub fn landscape(&self) -> Option<ImageRef> {
        if self.item_type == "Episode"
            && let Some(tag) = self.image_tags.get("Primary")
        {
            return Some(image(&self.id, ImageKind::Primary, tag));
        }
        if let Some(tag) = self.image_tags.get("Thumb") {
            return Some(image(&self.id, ImageKind::Thumb, tag));
        }
        if let (Some(parent_id), Some(tag)) =
            (&self.parent_thumb_item_id, &self.parent_thumb_image_tag)
        {
            return Some(image(parent_id, ImageKind::Thumb, tag));
        }
        self.backdrop()
    }

    pub fn resume_ticks(&self) -> i64 {
        self.user_data
            .as_ref()
            .filter(|data| !data.played)
            .map_or(0, |data| data.playback_position_ticks)
    }

    /// Watched fraction (0..=1) for progress bars, if partially watched.
    pub fn progress(&self) -> Option<f64> {
        let ticks = self.resume_ticks();
        let runtime = self.run_time_ticks?;
        (ticks > 0 && runtime > 0).then(|| (ticks as f64 / runtime as f64).clamp(0.0, 1.0))
    }

    pub fn played(&self) -> bool {
        self.user_data.as_ref().is_some_and(|data| data.played)
    }

    pub fn is_playable(&self) -> bool {
        matches!(
            self.item_type.as_str(),
            "Movie" | "Episode" | "Video" | "MusicVideo"
        )
    }

    /// "S1:E4 · Name" for episodes, the plain name otherwise.
    pub fn episode_label(&self) -> String {
        match (self.parent_index_number, self.index_number) {
            (Some(season), Some(episode)) if self.item_type == "Episode" => {
                format!("S{season}:E{episode} · {}", self.name)
            }
            (None, Some(episode)) if self.item_type == "Episode" => {
                format!("E{episode} · {}", self.name)
            }
            _ => self.name.clone(),
        }
    }
}

fn parent_param(parent_id: Option<&str>) -> String {
    parent_id.map_or_else(String::new, |id| format!("&ParentId={}", encode(id)))
}

fn image(item_id: &str, kind: ImageKind, tag: &str) -> ImageRef {
    ImageRef {
        item_id: item_id.to_string(),
        kind,
        tag: tag.to_string(),
    }
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emby::models::UserItemData;

    fn episode() -> BaseItem {
        BaseItem {
            id: "ep".into(),
            name: "The Duel".into(),
            item_type: "Episode".into(),
            series_id: Some("series".into()),
            series_primary_image_tag: Some("sp".into()),
            parent_backdrop_item_id: Some("series".into()),
            parent_backdrop_image_tags: vec!["sb".into()],
            index_number: Some(4),
            parent_index_number: Some(1),
            run_time_ticks: Some(1000),
            ..Default::default()
        }
    }

    #[test]
    fn query_string_encodes_search_terms() {
        let query = ItemQuery {
            search_term: Some("Akujo & co".into()),
            include_types: Some("Movie,Series"),
            recursive: true,
            limit: 50,
            ..Default::default()
        };
        let qs = query.to_query_string();
        assert!(qs.contains("SearchTerm=Akujo%20%26%20co"));
        assert!(qs.contains("IncludeItemTypes=Movie,Series"));
        assert!(qs.contains("Recursive=true"));
        assert!(qs.contains("Limit=50"));
        assert!(!qs.contains("ParentId"));
        // Searches keep Emby's relevance order.
        assert!(!qs.contains("SortBy"));
        let listing = ItemQuery::default().to_query_string();
        assert!(listing.contains("SortBy=SortName&SortOrder=Ascending"));
    }

    #[test]
    fn query_string_carries_sort_and_filters() {
        let query = ItemQuery {
            sort_by: Some("DateCreated"),
            descending: true,
            filters: vec!["IsFavorite", "IsUnplayed"],
            genre_id: Some("7424".into()),
            ..Default::default()
        };
        let qs = query.to_query_string();
        assert!(qs.contains("SortBy=DateCreated&SortOrder=Descending"));
        assert!(qs.contains("Filters=IsFavorite,IsUnplayed"));
        assert!(qs.contains("GenreIds=7424"));
        assert!(!qs.contains("TagIds"));
    }

    #[test]
    fn image_path_includes_size_and_tag() {
        let path = EmbyClient::image_path(&image("abc", ImageKind::Backdrop, "t1"), 1280);
        assert_eq!(
            path,
            "/emby/Items/abc/Images/Backdrop?maxWidth=1280&tag=t1&quality=90"
        );
    }

    #[test]
    fn episode_without_images_falls_back_to_series_art() {
        let ep = episode();
        assert_eq!(ep.poster(), Some(image("series", ImageKind::Primary, "sp")));
        assert_eq!(
            ep.backdrop(),
            Some(image("series", ImageKind::Backdrop, "sb"))
        );
        assert_eq!(ep.landscape(), ep.backdrop());
    }

    #[test]
    fn episode_still_wins_for_landscape_cards() {
        let mut ep = episode();
        ep.image_tags.insert("Primary".into(), "own".into());
        assert_eq!(ep.landscape(), Some(image("ep", ImageKind::Primary, "own")));
        // Portrait slots still get the series poster, not the cropped still.
        assert_eq!(ep.poster(), Some(image("series", ImageKind::Primary, "sp")));
    }

    #[test]
    fn progress_ignores_played_items() {
        let mut ep = episode();
        ep.user_data = Some(UserItemData {
            playback_position_ticks: 250,
            ..Default::default()
        });
        assert_eq!(ep.progress(), Some(0.25));
        assert_eq!(ep.resume_ticks(), 250);
        ep.user_data.as_mut().unwrap().played = true;
        assert_eq!(ep.progress(), None);
        assert_eq!(ep.resume_ticks(), 0);
    }

    #[test]
    fn episode_label_formats_season_and_episode() {
        assert_eq!(episode().episode_label(), "S1:E4 · The Duel");
        let movie = BaseItem {
            name: "Heat".into(),
            item_type: "Movie".into(),
            index_number: Some(2),
            ..Default::default()
        };
        assert_eq!(movie.episode_label(), "Heat");
    }
}
