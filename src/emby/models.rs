use serde::{Deserialize, Serialize};

// Only constructed by the currently-unwired auth::authenticate_by_name
// fallback (see its doc comment) — real login goes through the embedded
// webview instead.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateByNameRequest {
    pub username: String,
    pub pw: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateByNameResponse {
    pub access_token: String,
    pub server_id: String,
    pub user: EmbyUser,
}

#[allow(dead_code)]
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
            subtitle_profiles: vec![
                SubtitleProfile {
                    format: "srt".to_string(),
                    method: "External".to_string(),
                },
                SubtitleProfile {
                    format: "ass".to_string(),
                    method: "External".to_string(),
                },
                SubtitleProfile {
                    format: "vtt".to_string(),
                    method: "External".to_string(),
                },
            ],
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
}

/// Minimal item lookup — just enough to resolve an episode's parent
/// SeriesId for the per-series override key. `id`/`item_type` aren't
/// read yet (only `series_id` is), kept since the API returns them and
/// they're cheap to have on hand.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemDto {
    pub id: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub series_id: Option<String>,
}

/// One chapter marker on an item, as Emby reports it — used for the seek
/// bar's tick marks (`playback/mod.rs`), which only need `start_position_
/// ticks`. `name` isn't displayed anywhere yet (no chapter tooltip/label
/// in this pass) but is cheap to keep modeled faithfully against the real
/// API shape for whenever that's wanted.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChapterInfo {
    pub start_position_ticks: i64,
    #[allow(dead_code)]
    #[serde(default)]
    pub name: Option<String>,
}

/// Just enough of an item (episode or movie) to build the OSD title, seek
/// bar chapter ticks, and — for episodes — locate this item's neighbors
/// within its series for the previous/next-episode buttons.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub index_number: Option<i32>,
    #[serde(default)]
    pub parent_index_number: Option<i32>,
    #[serde(default)]
    pub series_name: Option<String>,
    #[serde(default)]
    pub chapters: Vec<ChapterInfo>,
}

/// Emby's standard list-response envelope (`{"Items": [...], ...}`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsResponse {
    pub items: Vec<ItemSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayingRequest {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub can_seek: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProgressRequest {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub can_seek: bool,
    pub event_name: String,
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

    #[test]
    fn items_response_deserializes_episode_list_with_chapters() {
        let raw = serde_json::json!({
            "Items": [
                {
                    "Id": "ep1",
                    "Name": "Pilot",
                    "IndexNumber": 1,
                    "ParentIndexNumber": 1,
                    "SeriesName": "Some Show",
                    "Chapters": [
                        {"StartPositionTicks": 0, "Name": "Intro"},
                        {"StartPositionTicks": 6000000000i64, "Name": null}
                    ]
                }
            ],
            "TotalRecordCount": 1
        });
        let response: ItemsResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(response.items.len(), 1);
        let ep = &response.items[0];
        assert_eq!(ep.id, "ep1");
        assert_eq!(ep.index_number, Some(1));
        assert_eq!(ep.series_name.as_deref(), Some("Some Show"));
        assert_eq!(ep.chapters.len(), 2);
        assert_eq!(ep.chapters[0].name.as_deref(), Some("Intro"));
        assert_eq!(ep.chapters[1].start_position_ticks, 6_000_000_000);
    }
}
