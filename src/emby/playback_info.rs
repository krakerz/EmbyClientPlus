use anyhow::Result;

use super::EmbyClient;
use super::models::{DeviceProfile, PlaybackInfoRequest, PlaybackInfoResponse};

impl EmbyClient {
    /// Negotiates direct-play vs. transcode for an item. `device_profile`
    /// defaults to `DeviceProfile::default()` (deliberately permissive —
    /// mpv/ffmpeg direct-plays far more than a typical Emby client) unless
    /// the caller has a reason to constrain it (e.g. a user-set bitrate
    /// cap).
    pub async fn get_playback_info(
        &self,
        user_id: &str,
        item_id: &str,
        device_profile: DeviceProfile,
        max_streaming_bitrate: Option<i64>,
        start_time_ticks: i64,
    ) -> Result<PlaybackInfoResponse> {
        let request = PlaybackInfoRequest {
            user_id: user_id.to_string(),
            device_profile,
            max_streaming_bitrate,
            start_time_ticks,
            media_source_id: None,
            auto_open_live_stream: true,
        };
        self.post(&format!("/emby/Items/{item_id}/PlaybackInfo"), &request)
            .await
    }
}
