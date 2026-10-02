use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateByNameRequest {
    pub username: String,
    pub pw: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateByNameResponse {
    pub access_token: String,
    #[allow(dead_code)]
    pub server_id: String,
    pub user: EmbyUser,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EmbyUser {
    pub id: String,
    pub name: String,
}

/// What we tell Emby our client can do. Deliberately permissive — mpv/
/// ffmpeg handle far more codecs natively than a browser-based client, so
/// this favors direct play over server-side transcoding wherever possible.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceProfile {
    pub max_streaming_bitrate: Option<i64>,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

impl Default for DeviceProfile {
    fn default() -> Self {
        DeviceProfile {
            max_streaming_bitrate: None,
            direct_play_profiles: vec![
                DirectPlayProfile {
                    container: "mkv,mp4,mov,avi,webm,ts,m2ts,flv".to_string(),
                    profile_type: "Video".to_string(),
                    video_codec: Some("h264,hevc,av1,vp9,vp8,mpeg4,mpeg2video,vc1".to_string()),
                    audio_codec: Some(
                        "aac,ac3,eac3,dts,truehd,flac,opus,vorbis,mp3,pcm".to_string(),
                    ),
                },
                DirectPlayProfile {
                    container: "mp3,flac,ogg,wav,aac,m4a".to_string(),
                    profile_type: "Audio".to_string(),
                    video_codec: None,
                    audio_codec: Some("aac,flac,opus,vorbis,mp3,pcm".to_string()),
                },
            ],
            // Only used as a fallback when direct play genuinely isn't
            // possible (e.g. a codec mpv can't decode at all).
            transcoding_profiles: vec![TranscodingProfile {
                container: "ts".to_string(),
                profile_type: "Video".to_string(),
                video_codec: "h264".to_string(),
                audio_codec: "aac".to_string(),
                protocol: "hls".to_string(),
            }],
            // mpv renders every subtitle format itself, embedded in the
            // container; anything missing here makes Emby fall back to a
            // burn-in transcode.
            subtitle_profiles: [
                "ass", "ssa", "srt", "subrip", "vtt", "webvtt", "pgs", "pgssub", "dvdsub",
                "vobsub", "dvbsub", "mov_text", "sub", "smi",
            ]
            .into_iter()
            .map(|format| SubtitleProfile {
                format: format.to_string(),
                method: "Embed".to_string(),
            })
            .collect(),
        }
    }
}

impl DeviceProfile {
    /// Forces Emby to transcode (no direct-play profiles), to HLS H.264/AAC
    /// under `max_bitrate`. Subtitles are requested as external files.
    pub fn transcode_only(max_bitrate: Option<i64>) -> Self {
        DeviceProfile {
            max_streaming_bitrate: max_bitrate,
            direct_play_profiles: Vec::new(),
            subtitle_profiles: ["ass", "ssa", "srt", "subrip", "vtt"]
                .into_iter()
                .map(|format| SubtitleProfile {
                    format: format.to_string(),
                    method: "External".to_string(),
                })
                .collect(),
            ..DeviceProfile::default()
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DirectPlayProfile {
    pub container: String,
    #[serde(rename = "Type")]
    pub profile_type: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodingProfile {
    pub container: String,
    #[serde(rename = "Type")]
    pub profile_type: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub protocol: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SubtitleProfile {
    pub format: String,
    pub method: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoRequest {
    pub user_id: String,
    pub device_profile: DeviceProfile,
    pub max_streaming_bitrate: Option<i64>,
    pub start_time_ticks: i64,
    pub media_source_id: Option<String>,
    pub auto_open_live_stream: bool,
    pub enable_direct_play: bool,
    pub enable_direct_stream: bool,
    pub enable_transcoding: bool,
    pub audio_stream_index: Option<i32>,
    /// -1 asks for no subtitle in the stream (we fetch subtitles ourselves).
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    pub media_sources: Vec<MediaSource>,
    pub play_session_id: String,
}

// Models the real PlaybackInfo response shape faithfully; only `id` and
// `media_streams` are consumed by M8's playback orchestration so far —
// the rest (direct-play/transcode flags, subtitle delivery info) are for
// M9 (subtitle rendering) and smarter media-source selection later.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSource {
    pub id: String,
    pub path: Option<String>,
    pub container: Option<String>,
    pub supports_direct_play: bool,
    pub supports_direct_stream: bool,
    pub supports_transcoding: bool,
    pub transcoding_url: Option<String>,
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
    /// A link elsewhere (trailers), played from `path` directly.
    #[serde(default)]
    pub is_remote: bool,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    pub index: i32,
    #[serde(rename = "Type")]
    pub stream_type: String,
    pub codec: Option<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub is_forced: bool,
    pub delivery_method: Option<String>,
    pub delivery_url: Option<String>,
    #[serde(default)]
    pub is_external: bool,
    #[serde(default)]
    pub is_text_subtitle_stream: bool,
    #[serde(default)]
    pub display_title: Option<String>,
}

/// One chapter marker. `marker_type` is `Chapter`, or `IntroStart` /
/// `IntroEnd` / `CreditsStart` for Emby's detected intro and credits.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ChapterInfo {
    pub start_position_ticks: i64,
    pub name: Option<String>,
    pub marker_type: Option<String>,
}

/// The generic item shape every browse endpoint returns (Emby's
/// `BaseItemDto`), trimmed to what the UI shows.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct BaseItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub collection_type: Option<String>,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season_id: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub production_year: Option<i32>,
    pub overview: Option<String>,
    pub run_time_ticks: Option<i64>,
    pub official_rating: Option<String>,
    pub community_rating: Option<f64>,
    pub image_tags: HashMap<String, String>,
    pub backdrop_image_tags: Vec<String>,
    pub series_primary_image_tag: Option<String>,
    pub parent_backdrop_item_id: Option<String>,
    pub parent_backdrop_image_tags: Vec<String>,
    pub parent_thumb_item_id: Option<String>,
    pub parent_thumb_image_tag: Option<String>,
    pub user_data: Option<UserItemData>,
    pub chapters: Vec<ChapterInfo>,
    pub people: Vec<Person>,
    pub local_trailer_count: Option<i32>,
    // Music.
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub album_artist: Option<String>,
    pub album_primary_image_tag: Option<String>,
    pub artist_items: Vec<NameId>,
    pub album_artists: Vec<NameId>,
    pub child_count: Option<i32>,
    pub remote_trailers: Vec<RemoteTrailer>,
    /// A person's role in the title they're listed for (cast cards only).
    #[serde(skip)]
    pub role: Option<String>,
}

/// A linked item by name and id (album artists).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct NameId {
    pub name: String,
    pub id: String,
}

/// A trailer hosted elsewhere (usually a YouTube page).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct RemoteTrailer {
    pub url: String,
    pub name: Option<String>,
}

/// Cast/crew entry on an item (`Fields=People`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub primary_image_tag: Option<String>,
}

impl Person {
    /// As a card-able item, so cast rows reuse the normal card widgets.
    pub fn as_item(&self) -> BaseItem {
        let mut image_tags = HashMap::new();
        if let Some(tag) = &self.primary_image_tag {
            image_tags.insert("Primary".to_string(), tag.clone());
        }
        BaseItem {
            id: self.id.clone(),
            name: self.name.clone(),
            item_type: "Person".into(),
            image_tags,
            role: self
                .role
                .clone()
                .filter(|r| !r.is_empty())
                .or(self.kind.clone()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserItemData {
    pub playback_position_ticks: i64,
    pub played: bool,
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<i32>,
    pub is_favorite: bool,
}

/// One "Because you watched X" row from `/Movies/Recommendations`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Recommendation {
    #[serde(default)]
    pub recommendation_type: String,
    #[serde(default)]
    pub baseline_item_name: Option<String>,
    #[serde(default)]
    pub items: Vec<BaseItem>,
}

impl Recommendation {
    /// Row heading in Emby web's wording.
    pub fn title(&self) -> String {
        let baseline = self.baseline_item_name.as_deref().unwrap_or_default();
        match self.recommendation_type.as_str() {
            "SimilarToRecentlyPlayed" => format!("Because you watched {baseline}"),
            "SimilarToLikedItem" => format!("Because you like {baseline}"),
            "HasDirectorFromRecentlyPlayed" | "HasLikedDirector" => {
                format!("Directed by {baseline}")
            }
            "HasActorFromRecentlyPlayed" | "HasLikedActor" => format!("Starring {baseline}"),
            _ if !baseline.is_empty() => format!("Because of {baseline}"),
            _ => "Recommended".to_string(),
        }
    }
}

/// Emby's paged list envelope.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub total_record_count: usize,
}

/// Body of `/Sessions/Playing` and `/Sessions/Playing/Progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProgressRequest {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume_level: i32,
    pub can_seek: bool,
    /// `DirectStream` or `Transcode`.
    pub play_method: &'static str,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    /// `TimeUpdate`, `Pause`, `Unpause`, `AudioTrackChange`, ...; omitted
    /// on the initial Playing report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_name: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StoppedRequest {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position_ticks: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_item_deserializes_an_episode_with_user_data() {
        let raw = serde_json::json!({
            "Id": "ep4",
            "Name": "Episode 4",
            "Type": "Episode",
            "SeriesId": "s1",
            "SeriesName": "Futsutsuka na Akujo",
            "IndexNumber": 4,
            "ParentIndexNumber": 1,
            "RunTimeTicks": 14_200_000_000i64,
            "ImageTags": {"Primary": "tag-p"},
            "SeriesPrimaryImageTag": "tag-sp",
            "ParentBackdropItemId": "s1",
            "ParentBackdropImageTags": ["tag-b"],
            "UserData": {"PlaybackPositionTicks": 6_000_000_000i64, "Played": false, "PlayedPercentage": 42.2},
            "SomethingNew": {"ignored": true}
        });
        let item: BaseItem = serde_json::from_value(raw).unwrap();
        assert_eq!(item.item_type, "Episode");
        assert_eq!(item.index_number, Some(4));
        assert_eq!(item.image_tags["Primary"], "tag-p");
        assert_eq!(item.parent_backdrop_image_tags, ["tag-b"]);
        let user_data = item.user_data.unwrap();
        assert_eq!(user_data.playback_position_ticks, 6_000_000_000);
        assert!(!user_data.played);
    }

    #[test]
    fn query_result_deserializes_a_sparse_series_list() {
        let raw = serde_json::json!({
            "Items": [{"Id": "s1", "Name": "Show", "Type": "Series",
                       "UserData": {"UnplayedItemCount": 3}}],
            "TotalRecordCount": 120
        });
        let result: QueryResult<BaseItem> = serde_json::from_value(raw).unwrap();
        assert_eq!(result.total_record_count, 120);
        let series = &result.items[0];
        assert!(series.backdrop_image_tags.is_empty());
        assert_eq!(
            series.user_data.as_ref().unwrap().unplayed_item_count,
            Some(3)
        );
    }

    #[test]
    fn recommendation_rows_are_titled_like_emby_web() {
        let raw = serde_json::json!([
            {"RecommendationType": "SimilarToRecentlyPlayed", "BaselineItemName": "Godzilla",
             "CategoryId": "1", "Items": [{"Id": "m1", "Name": "Kong", "Type": "Movie"}]},
            {"RecommendationType": "SomethingNew", "Items": []}
        ]);
        let rows: Vec<Recommendation> = serde_json::from_value(raw).unwrap();
        assert_eq!(rows[0].title(), "Because you watched Godzilla");
        assert_eq!(rows[0].items[0].name, "Kong");
        assert_eq!(rows[1].title(), "Recommended");
    }

    #[test]
    fn device_profile_serializes_with_pascal_case_keys() {
        let profile = DeviceProfile::default();
        let json = serde_json::to_value(&profile).unwrap();
        assert!(json.get("DirectPlayProfiles").is_some());
        assert!(json.get("TranscodingProfiles").is_some());
        assert!(json.get("SubtitleProfiles").is_some());
        let first_direct_play = &json["DirectPlayProfiles"][0];
        assert_eq!(first_direct_play["Type"], "Video");
        assert!(
            first_direct_play["Container"]
                .as_str()
                .unwrap()
                .contains("mkv")
        );
    }

    #[test]
    fn authenticate_request_serializes_expected_shape() {
        let req = AuthenticateByNameRequest {
            username: "alvi".to_string(),
            pw: "hunter2".to_string(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["Username"], "alvi");
        assert_eq!(json["Pw"], "hunter2");
    }

    #[test]
    fn media_source_deserializes_from_real_shaped_response() {
        let raw = serde_json::json!({
            "Id": "abc123",
            "Path": "/mnt/media/movie.mkv",
            "Container": "mkv",
            "SupportsDirectPlay": true,
            "SupportsDirectStream": true,
            "SupportsTranscoding": false,
            "TranscodingUrl": null,
            "MediaStreams": [
                {
                    "Index": 1,
                    "Type": "Audio",
                    "Codec": "aac",
                    "Language": "eng",
                    "IsDefault": true,
                    "IsForced": false,
                    "DeliveryMethod": null,
                    "DeliveryUrl": null
                }
            ]
        });
        let source: MediaSource = serde_json::from_value(raw).unwrap();
        assert_eq!(source.id, "abc123");
        assert!(source.supports_direct_play);
        assert_eq!(source.media_streams.len(), 1);
        assert_eq!(source.media_streams[0].stream_type, "Audio");
    }
}
