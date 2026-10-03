use anyhow::Result;

use super::EmbyClient;
use super::models::{DeviceProfile, PlaybackInfoRequest, PlaybackInfoResponse};

/// What to ask Emby for: the original file (`max_bitrate: None`), or a
/// transcode capped at `max_bitrate` bits/s.
#[derive(Debug, Clone, Default)]
pub struct StreamRequest {
    pub start_ticks: i64,
    pub max_bitrate: Option<i64>,
    /// Tallest transcoded picture (low bitrates); `None` keeps the source's.
    pub max_height: Option<u32>,
    /// Emby `MediaStream.Index` of the audio to transcode with.
    pub audio_stream_index: Option<i32>,
}

impl StreamRequest {
    fn to_request(&self, user_id: &str) -> PlaybackInfoRequest {
        let transcode = self.max_bitrate.is_some();
        PlaybackInfoRequest {
            user_id: user_id.to_string(),
            device_profile: if transcode {
                DeviceProfile::transcode_only(self.max_bitrate, self.max_height)
            } else {
                DeviceProfile::default()
            },
            max_streaming_bitrate: self.max_bitrate,
            // A transcode that starts mid-file has a timeline starting at 0,
            // which breaks positions; start it at 0 and let mpv seek instead.
            start_time_ticks: if transcode { 0 } else { self.start_ticks },
            media_source_id: None,
            auto_open_live_stream: true,
            enable_direct_play: !transcode,
            enable_direct_stream: !transcode,
            enable_transcoding: transcode,
            audio_stream_index: self.audio_stream_index,
            // Never burn subtitles in; mpv loads them as separate files.
            subtitle_stream_index: transcode.then_some(-1),
        }
    }
}

impl EmbyClient {
    pub async fn get_playback_info(
        &self,
        user_id: &str,
        item_id: &str,
        stream: &StreamRequest,
    ) -> Result<PlaybackInfoResponse> {
        self.post(
            &format!("/emby/Items/{item_id}/PlaybackInfo"),
            &stream.to_request(user_id),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_quality_allows_direct_play_only() {
        let request = StreamRequest::default().to_request("u");
        assert!(request.enable_direct_play && request.enable_direct_stream);
        assert!(!request.enable_transcoding);
        assert_eq!(request.subtitle_stream_index, None);
    }

    #[test]
    fn capped_quality_forces_a_transcode_without_burned_subtitles() {
        let request = StreamRequest {
            start_ticks: 42,
            max_bitrate: Some(6_000_000),
            max_height: None,
            audio_stream_index: Some(1),
        }
        .to_request("u");
        assert!(!request.enable_direct_play && !request.enable_direct_stream);
        assert!(request.enable_transcoding);
        assert_eq!(request.max_streaming_bitrate, Some(6_000_000));
        assert_eq!(request.subtitle_stream_index, Some(-1));
        assert_eq!(request.audio_stream_index, Some(1));
        assert_eq!(request.start_time_ticks, 0);
        assert!(request.device_profile.direct_play_profiles.is_empty());
    }

    #[test]
    fn low_bitrates_cap_the_picture_height() {
        let request = StreamRequest {
            max_bitrate: Some(1_000_000),
            max_height: Some(360),
            ..Default::default()
        }
        .to_request("u");
        let json = serde_json::to_value(&request).unwrap();
        let condition = &json["DeviceProfile"]["CodecProfiles"][0]["Conditions"][0];
        assert_eq!(condition["Property"], "Height");
        assert_eq!(condition["Value"], "360");
        let original = serde_json::to_value(StreamRequest::default().to_request("u")).unwrap();
        assert!(original["DeviceProfile"].get("CodecProfiles").is_none());
    }
}
